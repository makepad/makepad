//! Codex CLI agent backend.
//!
//! Uses a local `codex` installation in non-interactive JSONL mode.

use crate::agent::*;
use crate::types::*;
use makepad_micro_serde::*;
use makepad_widgets::*;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;

enum CodexOutput {
    Stdout(String),
    Stderr(String),
    StdoutClosed,
}

struct CodexProcess {
    child: Child,
    receiver: Receiver<CodexOutput>,
    stdout_closed: bool,
}

impl CodexProcess {
    fn start(cli_path: &str, cwd: &str, args: &[String], stdin_text: &str) -> Result<Self, String> {
        let mut command = Command::new(cli_path);
        command
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for arg in args {
            command.arg(arg);
        }

        let mut child = command.spawn().map_err(|err| err.to_string())?;
        let mut stdin = child.stdin.take();
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "Codex stdout unavailable".to_string())?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| "Codex stderr unavailable".to_string())?;

        let (sender, receiver) = mpsc::channel::<CodexOutput>();

        let stdout_sender = sender.clone();
        thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                match line {
                    Ok(line) => {
                        if stdout_sender.send(CodexOutput::Stdout(line)).is_err() {
                            break;
                        }
                        SignalToUI::set_ui_signal();
                    }
                    Err(_) => break,
                }
            }
            let _ = stdout_sender.send(CodexOutput::StdoutClosed);
            SignalToUI::set_ui_signal();
        });

        thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines() {
                if let Ok(line) = line {
                    if sender.send(CodexOutput::Stderr(line)).is_err() {
                        break;
                    }
                    SignalToUI::set_ui_signal();
                }
            }
        });

        let process = Self {
            child,
            receiver,
            stdout_closed: false,
        };

        if let Some(mut stdin) = stdin.take() {
            let _ = stdin.write_all(stdin_text.as_bytes());
        }

        Ok(process)
    }

    fn kill(&mut self) {
        let _ = self.child.kill();
    }

    fn try_recv(&mut self) -> Option<CodexOutput> {
        match self.receiver.try_recv() {
            Ok(CodexOutput::StdoutClosed) => {
                self.stdout_closed = true;
                Some(CodexOutput::StdoutClosed)
            }
            Ok(output) => Some(output),
            Err(_) => None,
        }
    }

    fn try_wait(&mut self) -> Option<std::process::ExitStatus> {
        self.child.try_wait().ok().flatten()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodexMode {
    Exec,
    Plan,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CodexSessionState {
    Ready,
    Prompting,
    Error,
}

struct CodexSession {
    state: CodexSessionState,
    cwd: String,
    system_prompt: Option<String>,
    model: Option<String>,
    current_prompt: Option<PromptId>,
    process: Option<CodexProcess>,
    messages: Vec<Message>,
    last_emitted_text: String,
    stderr_text: String,
}

pub struct CodexAgent {
    cli_path: Option<String>,
    sessions: HashMap<LiveId, CodexSession>,
    pending_events: Vec<AgentEvent>,
    default_cwd: String,
    mode: CodexMode,
}

impl CodexAgent {
    pub fn new() -> Self {
        Self::with_mode(CodexMode::Exec)
    }

    pub fn new_plan() -> Self {
        Self::with_mode(CodexMode::Plan)
    }

    pub fn with_mode(mode: CodexMode) -> Self {
        let default_cwd = std::env::current_dir()
            .map(|path| path.to_string_lossy().to_string())
            .unwrap_or_else(|_| ".".to_string());
        Self {
            cli_path: Self::find_cli(),
            sessions: HashMap::new(),
            pending_events: Vec::new(),
            default_cwd,
            mode,
        }
    }

    pub fn is_available() -> bool {
        Self::find_cli().is_some()
    }

    pub fn find_cli() -> Option<String> {
        if let Ok(path) = std::env::var("CODEX_PATH") {
            if is_executable_path(&path) {
                return Some(path);
            }
        }

        let home = std::env::var("HOME").ok();
        let home_npm = home
            .as_ref()
            .map(|home| format!("{home}/.npm-global/bin/codex"));
        let home_local = home.as_ref().map(|home| format!("{home}/.local/bin/codex"));
        let candidates = [
            home_npm.as_deref(),
            home_local.as_deref(),
            Some("/usr/local/bin/codex"),
            Some("/opt/homebrew/bin/codex"),
            Some("codex"),
        ];

        for candidate in candidates.into_iter().flatten() {
            if candidate.contains('/') {
                if is_executable_path(candidate) {
                    return Some(candidate.to_string());
                }
            } else if Command::new(candidate).arg("--version").output().is_ok() {
                return Some(candidate.to_string());
            }
        }
        None
    }

    fn build_args(session: &CodexSession) -> Vec<String> {
        let mut args = vec![
            "exec".to_string(),
            "--json".to_string(),
            "--color".to_string(),
            "never".to_string(),
            "--sandbox".to_string(),
            "read-only".to_string(),
            "--ephemeral".to_string(),
        ];

        if let Some(model) = &session.model {
            args.push("--model".to_string());
            args.push(model.clone());
        }

        args.push("-".to_string());
        args
    }

    fn build_prompt(mode: CodexMode, session: &CodexSession, text: &str) -> String {
        let mut prompt = String::new();
        if let Some(system_prompt) = &session.system_prompt {
            prompt.push_str(system_prompt);
            prompt.push_str("\n\n");
        }
        if mode == CodexMode::Plan {
            prompt.push_str(
                "You are running in Codex plan mode. Produce a concise implementation plan and do not modify files, run write operations, or make commits.\n\n",
            );
        }
        if !session.messages.is_empty() {
            prompt.push_str("Conversation so far:\n");
            for message in &session.messages {
                let role = match message.role {
                    MessageRole::System => "system",
                    MessageRole::User => "user",
                    MessageRole::Assistant => "assistant",
                    MessageRole::Tool => "tool",
                };
                prompt.push_str(role);
                prompt.push_str(": ");
                prompt.push_str(&message.text());
                prompt.push('\n');
            }
            prompt.push('\n');
        }
        prompt.push_str("User request:\n");
        prompt.push_str(text);
        prompt
    }

    fn queue_event(&mut self, event: AgentEvent) {
        self.pending_events.push(event);
        SignalToUI::set_ui_signal();
    }

    fn handle_stdout_line(session: &mut CodexSession, line: &str) -> Vec<AgentEvent> {
        let mut events = Vec::new();
        let Some(prompt_id) = session.current_prompt else {
            return events;
        };
        let Ok(value) = JsonValue::deserialize_json(line) else {
            return events;
        };

        if is_error_event(&value) {
            if let Some(error) = json_string(value.key("message")).or_else(|| extract_text(&value))
            {
                events.push(AgentEvent::PromptError {
                    prompt_id,
                    error: error.to_string(),
                });
            }
            return events;
        }

        if let Some(text) = assistant_text(&value) {
            let delta = if text.starts_with(&session.last_emitted_text) {
                text[session.last_emitted_text.len()..].to_string()
            } else {
                text
            };
            if !delta.is_empty() {
                session.last_emitted_text.push_str(&delta);
                events.push(AgentEvent::TextDelta {
                    prompt_id,
                    text: delta,
                });
            }
        }

        events
    }

    fn finish_session(session: &mut CodexSession, success: bool) -> Vec<AgentEvent> {
        let mut events = Vec::new();
        let Some(prompt_id) = session.current_prompt.take() else {
            return events;
        };

        if success {
            let assistant_text = session.last_emitted_text.clone();
            if !assistant_text.is_empty() {
                session.messages.push(Message::assistant(&assistant_text));
            }
            events.push(AgentEvent::TurnComplete {
                prompt_id,
                stop_reason: StopReason::EndTurn,
            });
        } else {
            let error = if session.stderr_text.trim().is_empty() {
                "Codex exited before completing the turn.".to_string()
            } else {
                session.stderr_text.clone()
            };
            events.push(AgentEvent::PromptError { prompt_id, error });
        }

        session.state = CodexSessionState::Ready;
        session.last_emitted_text.clear();
        session.stderr_text.clear();
        events
    }

    fn drain_session(session: &mut CodexSession) -> Vec<AgentEvent> {
        let mut events = Vec::new();
        let mut stdout_closed = false;

        loop {
            let output = match session.process.as_mut() {
                Some(process) => process.try_recv(),
                None => None,
            };
            let Some(output) = output else {
                break;
            };
            match output {
                CodexOutput::Stdout(line) => {
                    events.extend(Self::handle_stdout_line(session, &line));
                }
                CodexOutput::Stderr(line) => {
                    if !session.stderr_text.is_empty() {
                        session.stderr_text.push('\n');
                    }
                    session.stderr_text.push_str(&line);
                }
                CodexOutput::StdoutClosed => {
                    stdout_closed = true;
                }
            }
        }

        if stdout_closed {
            let exit_status = session
                .process
                .as_mut()
                .and_then(|process| process.try_wait());
            if let Some(status) = exit_status {
                if session.current_prompt.is_some() {
                    events.extend(Self::finish_session(session, status.success()));
                }
                session.process = None;
            }
        }

        events
    }
}

impl Default for CodexAgent {
    fn default() -> Self {
        Self::new()
    }
}

impl Agent for CodexAgent {
    fn create_session(&mut self, _cx: &mut Cx, config: SessionConfig) -> SessionId {
        let session_id = SessionId::new();
        let cwd = config.cwd.unwrap_or_else(|| self.default_cwd.clone());
        let state = if self.cli_path.is_some() {
            CodexSessionState::Ready
        } else {
            CodexSessionState::Error
        };
        self.sessions.insert(
            session_id.0,
            CodexSession {
                state,
                cwd,
                system_prompt: config.system_prompt,
                model: config.model,
                current_prompt: None,
                process: None,
                messages: Vec::new(),
                last_emitted_text: String::new(),
                stderr_text: String::new(),
            },
        );
        if state == CodexSessionState::Ready {
            self.queue_event(AgentEvent::SessionReady { session_id });
        } else {
            self.queue_event(AgentEvent::SessionError {
                session_id,
                error: "Codex CLI not found. Set CODEX_PATH or install codex.".to_string(),
            });
        }
        session_id
    }

    fn send_prompt(&mut self, _cx: &mut Cx, session_id: SessionId, text: &str) -> PromptId {
        let prompt_id = PromptId::new();
        let Some(cli_path) = self.cli_path.clone() else {
            self.queue_event(AgentEvent::PromptError {
                prompt_id,
                error: "Codex CLI not found. Set CODEX_PATH or install codex.".to_string(),
            });
            return prompt_id;
        };

        let Some(session) = self.sessions.get_mut(&session_id.0) else {
            return prompt_id;
        };
        if session.state == CodexSessionState::Prompting {
            self.queue_event(AgentEvent::PromptError {
                prompt_id,
                error: "Codex is already handling a prompt.".to_string(),
            });
            return prompt_id;
        }

        let prompt = Self::build_prompt(self.mode, session, text);
        let args = Self::build_args(session);
        match CodexProcess::start(&cli_path, &session.cwd, &args, &prompt) {
            Ok(process) => {
                session.messages.push(Message::user(text));
                session.state = CodexSessionState::Prompting;
                session.current_prompt = Some(prompt_id);
                session.process = Some(process);
                session.last_emitted_text.clear();
                session.stderr_text.clear();
            }
            Err(error) => {
                self.queue_event(AgentEvent::PromptError { prompt_id, error });
            }
        }

        prompt_id
    }

    fn send_tool_result(
        &mut self,
        _cx: &mut Cx,
        _session_id: SessionId,
        _tool_use_id: &str,
        _result: &str,
        _is_error: bool,
    ) {
    }

    fn cancel_prompt(&mut self, _cx: &mut Cx, prompt_id: PromptId) {
        for session in self.sessions.values_mut() {
            if session.current_prompt == Some(prompt_id) {
                if let Some(process) = &mut session.process {
                    process.kill();
                }
                session.process = None;
                session.current_prompt = None;
                session.state = CodexSessionState::Ready;
                session.last_emitted_text.clear();
                session.stderr_text.clear();
                break;
            }
        }
    }

    fn handle_event(&mut self, _cx: &mut Cx, event: &Event) -> Vec<AgentEvent> {
        let mut events = Vec::new();
        if let Event::Signal = event {
            events.append(&mut self.pending_events);
            for session in self.sessions.values_mut() {
                events.extend(Self::drain_session(session));
            }
        }
        events
    }

    fn is_session_ready(&self, session_id: SessionId) -> bool {
        self.sessions
            .get(&session_id.0)
            .is_some_and(|session| session.state == CodexSessionState::Ready)
    }

    fn is_stateless(&self) -> bool {
        true
    }

    fn inject_history(&mut self, session_id: SessionId, messages: Vec<Message>) {
        if let Some(session) = self.sessions.get_mut(&session_id.0) {
            session.messages = messages;
        }
    }
}

fn json_string(value: Option<&JsonValue>) -> Option<&str> {
    value.and_then(JsonValue::string).map(String::as_str)
}

fn is_error_event(value: &JsonValue) -> bool {
    let event_type = json_string(value.key("type")).unwrap_or_default();
    event_type.contains("error") || event_type == "turn.failed"
}

fn assistant_text(value: &JsonValue) -> Option<String> {
    if let Some(delta) = json_string(value.key("delta")) {
        if !delta.is_empty() {
            return Some(delta.to_string());
        }
    }
    if let Some(message) = json_string(value.key("message")) {
        if is_assistant_message(value) && !message.is_empty() {
            return Some(message.to_string());
        }
    }
    if let Some(item) = value.key("item") {
        if is_assistant_message(item) {
            return extract_text(item).map(str::to_string);
        }
    }
    if is_assistant_message(value) {
        return extract_text(value).map(str::to_string);
    }
    None
}

fn is_assistant_message(value: &JsonValue) -> bool {
    json_string(value.key("role")) == Some("assistant")
        || json_string(value.key("author")).is_some_and(|author| author == "assistant")
        || json_string(value.key("type")).is_some_and(|event_type| {
            event_type.contains("assistant") || event_type.contains("message")
        })
}

fn extract_text(value: &JsonValue) -> Option<&str> {
    if let Some(text) = json_string(value.key("text")) {
        if !text.is_empty() {
            return Some(text);
        }
    }
    if let Some(content) = value.key("content") {
        match content {
            JsonValue::String(text) if !text.is_empty() => return Some(text.as_str()),
            JsonValue::Array(items) => {
                for item in items {
                    if let Some(text) = extract_text(item) {
                        return Some(text);
                    }
                }
            }
            _ => {}
        }
    }
    if let Some(output) = value.key("output") {
        if let Some(text) = extract_text(output) {
            return Some(text);
        }
    }
    None
}

fn is_executable_path(path: &str) -> bool {
    std::path::Path::new(path).is_file()
}
