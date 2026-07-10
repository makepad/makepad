//! Claude Code CLI agent backend.
//!
//! Uses a local `claude` installation in non-interactive stream-json mode.

use crate::agent::*;
use crate::types::*;
use makepad_micro_serde::*;
use makepad_widgets::*;
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;

enum ClaudeCodeOutput {
    Stdout(String),
    Stderr(String),
    StdoutClosed,
}

struct ClaudeCodeProcess {
    child: Child,
    receiver: Receiver<ClaudeCodeOutput>,
    stdout_closed: bool,
}

impl ClaudeCodeProcess {
    fn start(cli_path: &str, cwd: &str, args: &[String]) -> Result<Self, String> {
        let mut command = Command::new(cli_path);
        command
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for arg in args {
            command.arg(arg);
        }
        // Its own process group, so cancelling a prompt can take the CLI's
        // children (a shell, a game it launched) down with it instead of
        // orphaning them mid-task.
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }

        let mut child = command.spawn().map_err(|err| err.to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "Claude Code stdout unavailable".to_string())?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| "Claude Code stderr unavailable".to_string())?;

        let (sender, receiver) = mpsc::channel::<ClaudeCodeOutput>();

        let stdout_sender = sender.clone();
        thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                match line {
                    Ok(line) => {
                        if stdout_sender.send(ClaudeCodeOutput::Stdout(line)).is_err() {
                            break;
                        }
                        SignalToUI::set_ui_signal();
                    }
                    Err(_) => break,
                }
            }
            let _ = stdout_sender.send(ClaudeCodeOutput::StdoutClosed);
            SignalToUI::set_ui_signal();
        });

        thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines() {
                if let Ok(line) = line {
                    if sender.send(ClaudeCodeOutput::Stderr(line)).is_err() {
                        break;
                    }
                    SignalToUI::set_ui_signal();
                }
            }
        });

        Ok(Self {
            child,
            receiver,
            stdout_closed: false,
        })
    }

    fn kill(&mut self) {
        // Whole group, then reap. `Child::kill` alone leaves the CLI's own
        // children running and the CLI itself as a zombie.
        #[cfg(unix)]
        {
            extern "C" {
                fn kill(pid: i32, sig: i32) -> i32;
            }
            const SIGKILL: i32 = 9;
            unsafe {
                kill(-(self.child.id() as i32), SIGKILL);
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    fn try_recv(&mut self) -> Option<ClaudeCodeOutput> {
        match self.receiver.try_recv() {
            Ok(ClaudeCodeOutput::StdoutClosed) => {
                self.stdout_closed = true;
                Some(ClaudeCodeOutput::StdoutClosed)
            }
            Ok(output) => Some(output),
            Err(_) => None,
        }
    }

    fn try_wait(&mut self) -> Option<std::process::ExitStatus> {
        self.child.try_wait().ok().flatten()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ClaudeCodeSessionState {
    Ready,
    Prompting,
    Error,
}

struct ClaudeCodeSession {
    state: ClaudeCodeSessionState,
    cwd: String,
    system_prompt: Option<String>,
    model: Option<String>,
    allowed_tools: Vec<String>,
    permission_mode: Option<String>,
    settings_json: Option<String>,
    claude_session_id: Option<String>,
    current_prompt: Option<PromptId>,
    process: Option<ClaudeCodeProcess>,
    last_assistant_text: String,
    /// Whether this turn already produced assistant text. The `result` event
    /// carries the final text too, and must not re-emit what was streamed.
    emitted_text: bool,
    stderr_text: String,
}

pub struct ClaudeCodeAgent {
    cli_path: Option<String>,
    sessions: HashMap<LiveId, ClaudeCodeSession>,
    pending_events: Vec<AgentEvent>,
    default_cwd: String,
}

impl ClaudeCodeAgent {
    pub fn new() -> Self {
        let default_cwd = std::env::current_dir()
            .map(|path| path.to_string_lossy().to_string())
            .unwrap_or_else(|_| ".".to_string());
        Self {
            cli_path: Self::find_cli(),
            sessions: HashMap::new(),
            pending_events: Vec::new(),
            default_cwd,
        }
    }

    pub fn is_available() -> bool {
        Self::find_cli().is_some()
    }

    /// The backend's own session id, once the CLI has reported one. Persist this
    /// to resume the same conversation in a later run via
    /// [`SessionConfig::resume_session_id`].
    pub fn native_session_id(&self, session_id: SessionId) -> Option<&str> {
        self.sessions
            .get(&session_id.0)?
            .claude_session_id
            .as_deref()
    }

    pub fn find_cli() -> Option<String> {
        if let Ok(path) = std::env::var("CLAUDE_CODE_PATH") {
            if is_executable_path(&path) {
                return Some(path);
            }
        }

        let home = std::env::var("HOME").ok();
        let home_local = home
            .as_ref()
            .map(|home| format!("{home}/.local/bin/claude"));
        let candidates = [
            home_local.as_deref(),
            Some("/usr/local/bin/claude"),
            Some("/opt/homebrew/bin/claude"),
            Some("claude"),
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

    fn build_args(session: &ClaudeCodeSession, text: &str) -> Vec<String> {
        let mut args = vec![
            "-p".to_string(),
            "--verbose".to_string(),
            "--output-format".to_string(),
            "stream-json".to_string(),
            "--include-partial-messages".to_string(),
            "--strict-mcp-config".to_string(),
            "--mcp-config".to_string(),
            r#"{"mcpServers":{}}"#.to_string(),
            // Comma-joined, never space-separated: `--tools` is variadic and a
            // space-separated list would swallow the positional prompt below.
            // An empty string means "no tools", which is the chat-only default.
            "--tools".to_string(),
            session.allowed_tools.join(","),
        ];

        if let Some(permission_mode) = &session.permission_mode {
            args.push("--permission-mode".to_string());
            args.push(permission_mode.clone());
        }
        if let Some(settings_json) = &session.settings_json {
            args.push("--settings".to_string());
            args.push(settings_json.clone());
        }
        if let Some(model) = &session.model {
            args.push("--model".to_string());
            args.push(model.clone());
        }
        if let Some(system_prompt) = &session.system_prompt {
            args.push("--system-prompt".to_string());
            args.push(system_prompt.clone());
        }
        if let Some(claude_session_id) = &session.claude_session_id {
            args.push("--resume".to_string());
            args.push(claude_session_id.clone());
        }

        args.push(text.to_string());
        args
    }

    fn queue_event(&mut self, event: AgentEvent) {
        self.pending_events.push(event);
        SignalToUI::set_ui_signal();
    }

    fn handle_stdout_line(session: &mut ClaudeCodeSession, line: &str) -> Vec<AgentEvent> {
        let mut events = Vec::new();
        let Ok(value) = JsonValue::deserialize_json(line) else {
            return events;
        };

        if let Some(session_id) = json_string(value.key("session_id")) {
            session.claude_session_id = Some(session_id.to_string());
        }

        match json_string(value.key("type")) {
            Some("stream_event") => {
                let Some(prompt_id) = session.current_prompt else {
                    return events;
                };
                if let Some(text) = stream_event_text_delta(&value) {
                    session.last_assistant_text.push_str(&text);
                    session.emitted_text = true;
                    events.push(AgentEvent::TextDelta { prompt_id, text });
                }
            }
            Some("assistant") => {
                let Some(prompt_id) = session.current_prompt else {
                    return events;
                };
                if let Some(text) = assistant_text(&value) {
                    let delta = if text.starts_with(&session.last_assistant_text) {
                        text[session.last_assistant_text.len()..].to_string()
                    } else {
                        text.clone()
                    };
                    // Delta accounting restarts per assistant message, not per turn:
                    // `--include-partial-messages` streams one set of deltas for each
                    // message, and a turn that calls tools contains several. Carrying
                    // the accumulator across them makes `starts_with` fail on message
                    // two onwards, and the whole message gets emitted a second time.
                    session.last_assistant_text.clear();
                    if !delta.is_empty() {
                        session.emitted_text = true;
                        events.push(AgentEvent::TextDelta {
                            prompt_id,
                            text: delta,
                        });
                    }
                }
                // Claude Code runs its own tools, so these are informational: they
                // report what the agent is doing, and no tool result is expected back.
                for (tool_use_id, tool_name, tool_input) in assistant_tool_uses(&value) {
                    events.push(AgentEvent::ToolRequest {
                        prompt_id,
                        tool_use_id,
                        tool_name,
                        tool_input,
                    });
                }
            }
            Some("result") => {
                let Some(prompt_id) = session.current_prompt.take() else {
                    return events;
                };
                let result_text = json_string(value.key("result")).unwrap_or_default();
                let is_error = json_bool(value.key("is_error")).unwrap_or(false);
                if is_error {
                    events.push(AgentEvent::PromptError {
                        prompt_id,
                        error: result_text.to_string(),
                    });
                } else {
                    // Only when nothing was streamed — otherwise `result` would
                    // repeat the whole reply.
                    if !session.emitted_text && !result_text.is_empty() {
                        events.push(AgentEvent::TextDelta {
                            prompt_id,
                            text: result_text.to_string(),
                        });
                    }
                    events.push(AgentEvent::TurnComplete {
                        prompt_id,
                        stop_reason: StopReason::EndTurn,
                    });
                }
                session.state = ClaudeCodeSessionState::Ready;
                session.last_assistant_text.clear();
                session.emitted_text = false;
                session.stderr_text.clear();
            }
            _ => {}
        }

        events
    }

    fn drain_session(session: &mut ClaudeCodeSession) -> Vec<AgentEvent> {
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
                ClaudeCodeOutput::Stdout(line) => {
                    events.extend(Self::handle_stdout_line(session, &line));
                }
                ClaudeCodeOutput::Stderr(line) => {
                    if !session.stderr_text.is_empty() {
                        session.stderr_text.push('\n');
                    }
                    session.stderr_text.push_str(&line);
                }
                ClaudeCodeOutput::StdoutClosed => {
                    stdout_closed = true;
                }
            }
        }

        if stdout_closed {
            let exit_status = session
                .process
                .as_mut()
                .and_then(|process| process.try_wait());
            if session.current_prompt.is_some() {
                if let Some(status) = exit_status {
                    if let Some(prompt_id) = session.current_prompt.take() {
                        if status.success() {
                            events.push(AgentEvent::TurnComplete {
                                prompt_id,
                                stop_reason: StopReason::EndTurn,
                            });
                        } else {
                            let error = if session.stderr_text.trim().is_empty() {
                                format!("Claude Code exited with status {status}")
                            } else {
                                session.stderr_text.clone()
                            };
                            events.push(AgentEvent::PromptError { prompt_id, error });
                        }
                    }
                    session.state = ClaudeCodeSessionState::Ready;
                }
            }
            if exit_status.is_some() || session.current_prompt.is_none() {
                session.process = None;
            }
        }

        events
    }
}

impl Default for ClaudeCodeAgent {
    fn default() -> Self {
        Self::new()
    }
}

impl Agent for ClaudeCodeAgent {
    fn create_session(&mut self, _cx: &mut Cx, config: SessionConfig) -> SessionId {
        let session_id = SessionId::new();
        let cwd = config.cwd.unwrap_or_else(|| self.default_cwd.clone());
        let state = if self.cli_path.is_some() {
            ClaudeCodeSessionState::Ready
        } else {
            ClaudeCodeSessionState::Error
        };
        self.sessions.insert(
            session_id.0,
            ClaudeCodeSession {
                state,
                cwd,
                system_prompt: config.system_prompt,
                model: config.model,
                allowed_tools: config.allowed_tools,
                permission_mode: config.permission_mode,
                settings_json: config.settings_json,
                claude_session_id: config.resume_session_id,
                current_prompt: None,
                process: None,
                last_assistant_text: String::new(),
                emitted_text: false,
                stderr_text: String::new(),
            },
        );
        if state == ClaudeCodeSessionState::Ready {
            self.queue_event(AgentEvent::SessionReady { session_id });
        } else {
            self.queue_event(AgentEvent::SessionError {
                session_id,
                error: "Claude Code CLI not found. Set CLAUDE_CODE_PATH or install claude."
                    .to_string(),
            });
        }
        session_id
    }

    fn send_prompt(&mut self, _cx: &mut Cx, session_id: SessionId, text: &str) -> PromptId {
        let prompt_id = PromptId::new();
        let Some(cli_path) = self.cli_path.clone() else {
            self.queue_event(AgentEvent::PromptError {
                prompt_id,
                error: "Claude Code CLI not found. Set CLAUDE_CODE_PATH or install claude."
                    .to_string(),
            });
            return prompt_id;
        };

        let Some(session) = self.sessions.get_mut(&session_id.0) else {
            return prompt_id;
        };
        if session.state == ClaudeCodeSessionState::Prompting {
            self.queue_event(AgentEvent::PromptError {
                prompt_id,
                error: "Claude Code is already handling a prompt.".to_string(),
            });
            return prompt_id;
        }

        let args = Self::build_args(session, text);
        match ClaudeCodeProcess::start(&cli_path, &session.cwd, &args) {
            Ok(process) => {
                session.state = ClaudeCodeSessionState::Prompting;
                session.current_prompt = Some(prompt_id);
                session.process = Some(process);
                session.last_assistant_text.clear();
                session.emitted_text = false;
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
                session.state = ClaudeCodeSessionState::Ready;
                session.last_assistant_text.clear();
                session.emitted_text = false;
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
            .is_some_and(|session| session.state == ClaudeCodeSessionState::Ready)
    }
}

fn json_string(value: Option<&JsonValue>) -> Option<&str> {
    value.and_then(JsonValue::string).map(String::as_str)
}

fn json_bool(value: Option<&JsonValue>) -> Option<bool> {
    match value {
        Some(JsonValue::Bool(value)) => Some(*value),
        _ => None,
    }
}

fn assistant_text(value: &JsonValue) -> Option<String> {
    let message = value.key("message")?;
    let content = message.key("content")?;
    match content {
        JsonValue::String(text) => Some(text.clone()),
        JsonValue::Array(items) => {
            let mut out = String::new();
            for item in items {
                if json_string(item.key("type")) == Some("text") {
                    if let Some(text) = json_string(item.key("text")) {
                        out.push_str(text);
                    }
                }
            }
            (!out.is_empty()).then_some(out)
        }
        _ => None,
    }
}

/// Extract `(tool_use_id, tool_name, subject)` for each tool the assistant invoked.
/// `subject` is the most human-meaningful field of the tool input — the file being
/// edited, or the command being run — so a UI can say "editing player.gd".
fn assistant_tool_uses(value: &JsonValue) -> Vec<(String, String, String)> {
    let Some(message) = value.key("message") else {
        return Vec::new();
    };
    let Some(JsonValue::Array(items)) = message.key("content") else {
        return Vec::new();
    };
    items
        .iter()
        .filter(|item| json_string(item.key("type")) == Some("tool_use"))
        .map(|item| {
            let subject = item
                .key("input")
                .and_then(|input| {
                    ["file_path", "command", "pattern", "path", "url"]
                        .iter()
                        .find_map(|key| json_string(input.key(key)))
                })
                .unwrap_or_default();
            (
                json_string(item.key("id")).unwrap_or_default().to_string(),
                json_string(item.key("name")).unwrap_or_default().to_string(),
                subject.to_string(),
            )
        })
        .collect()
}

fn stream_event_text_delta(value: &JsonValue) -> Option<String> {
    let event = value.key("event")?;
    if json_string(event.key("type")) != Some("content_block_delta") {
        return None;
    }
    let delta = event.key("delta")?;
    if json_string(delta.key("type")) != Some("text_delta") {
        return None;
    }
    json_string(delta.key("text"))
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

fn is_executable_path(path: &str) -> bool {
    std::path::Path::new(path).is_file()
}
