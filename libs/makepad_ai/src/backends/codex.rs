//! Codex CLI agent backend.
//!
//! Runs `codex exec --json` inside the game directory. Codex owns its native
//! tool loop; the app observes completed file changes and evaluates the game
//! through the same authoring transaction used by every other backend.

use crate::agent::*;
use crate::types::*;
use makepad_micro_serde::*;
use makepad_widgets::*;
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
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
}

impl CodexProcess {
    fn start(cli_path: &str, cwd: &str, args: &[String]) -> Result<Self, String> {
        let mut command = Command::new(cli_path);
        command
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .args(args);
        // Give each turn its own group so cancel also stops commands Codex
        // launched, rather than leaving a compiler or formatter behind.
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }

        let mut child = command.spawn().map_err(|error| error.to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "Codex stdout unavailable".to_string())?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| "Codex stderr unavailable".to_string())?;
        let (sender, receiver) = mpsc::channel();
        let stdout_sender = sender.clone();
        thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                match line {
                    Ok(line) => {
                        if stdout_sender.send(CodexOutput::Stdout(line)).is_err() {
                            return;
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
            for line in BufReader::new(stderr).lines().flatten() {
                if sender.send(CodexOutput::Stderr(line)).is_err() {
                    return;
                }
                SignalToUI::set_ui_signal();
            }
        });
        Ok(Self { child, receiver })
    }

    fn kill(&mut self) {
        #[cfg(unix)]
        {
            extern "C" {
                fn kill(pid: i32, signal: i32) -> i32;
            }
            const SIGKILL: i32 = 9;
            unsafe {
                kill(-(self.child.id() as i32), SIGKILL);
            }
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
    thread_id: Option<String>,
    current_prompt: Option<PromptId>,
    process: Option<CodexProcess>,
    /// stdout can close a few milliseconds before the child becomes
    /// waitable. Keep polling rather than dropping an unreaped child or
    /// leaving the session permanently Prompting after the one EOF signal.
    output_closed: bool,
    stderr_text: String,
    protocol_error: Option<String>,
    emitted_text: bool,
}

pub struct CodexAgent {
    cli_path: Option<String>,
    sessions: HashMap<LiveId, CodexSession>,
    pending_events: Vec<AgentEvent>,
    default_cwd: String,
}

impl CodexAgent {
    pub fn new() -> Self {
        Self {
            cli_path: Self::find_cli(),
            sessions: HashMap::new(),
            pending_events: Vec::new(),
            default_cwd: std::env::current_dir()
                .map(|path| path.to_string_lossy().to_string())
                .unwrap_or_else(|_| ".".to_string()),
        }
    }

    pub fn is_available() -> bool {
        Self::find_cli().is_some()
    }

    pub fn native_session_id(&self, session_id: SessionId) -> Option<&str> {
        self.sessions.get(&session_id.0)?.thread_id.as_deref()
    }

    pub fn find_cli() -> Option<String> {
        if let Ok(path) = std::env::var("CODEX_PATH") {
            if is_executable_path(&path) {
                return Some(path);
            }
        }
        let candidates = [
            "/usr/local/bin/codex",
            "/opt/homebrew/bin/codex",
            "codex",
        ];
        for candidate in candidates {
            if candidate.contains('/') {
                if is_executable_path(candidate) {
                    return Some(candidate.to_string());
                }
            } else if Command::new(candidate)
                .arg("--version")
                .output()
                .is_ok_and(|output| output.status.success())
            {
                return Some(candidate.to_string());
            }
        }
        None
    }

    fn build_args(session: &CodexSession, text: &str) -> Vec<String> {
        // `resume` is an exec subcommand. Exec-level policy flags must precede
        // it; putting --sandbox after `resume` is rejected by the CLI and,
        // worse, omitting it lets a resumed thread inherit a broader policy.
        let mut args = vec![
            "exec".to_string(),
            "--json".to_string(),
            "--color".to_string(),
            "never".to_string(),
            "--sandbox".to_string(),
            "workspace-write".to_string(),
            "--skip-git-repo-check".to_string(),
            // Preserve CLI authentication while excluding user MCP/config
            // and exec-policy rules that could silently broaden this child
            // agent's capabilities.
            "--ignore-user-config".to_string(),
            "--ignore-rules".to_string(),
            // Commands spawned by the model do not inherit API keys or the
            // rest of the app environment. Codex itself still authenticates
            // before applying this policy to its shell tool.
            "--config".to_string(),
            "shell_environment_policy.inherit=none".to_string(),
        ];
        // Unlike a prefix in the first user message, developer instructions
        // remain authoritative on resumed turns. Quote as a TOML basic string
        // because `--config` parses its value as TOML rather than raw text.
        if let Some(system) = &session.system_prompt {
            args.push("--config".to_string());
            args.push(format!(
                "developer_instructions={}",
                toml_basic_string(system)
            ));
        }
        if let Some(model) = &session.model {
            args.push("--model".to_string());
            args.push(model.clone());
        }
        if let Some(thread_id) = &session.thread_id {
            args.push("resume".to_string());
            args.push(thread_id.clone());
            args.push(text.to_string());
            return args;
        }
        args.push(text.to_string());
        args
    }

    fn queue_event(&mut self, event: AgentEvent) {
        self.pending_events.push(event);
        SignalToUI::set_ui_signal();
    }

    fn handle_stdout_line(session: &mut CodexSession, line: &str) -> Vec<AgentEvent> {
        let Ok(value) = JsonValue::deserialize_json(line) else {
            let error = format!("Codex emitted non-JSON output: {line}");
            append_diagnostic(&mut session.stderr_text, &error);
            session.protocol_error.get_or_insert(error);
            return Vec::new();
        };
        match json_string(value.key("type")) {
            Some("thread.started") => {
                if let Some(thread_id) = json_string(value.key("thread_id")) {
                    if let Some(expected) = session.thread_id.as_deref() {
                        if expected != thread_id {
                            let error = format!(
                                "Codex resumed unexpected thread {thread_id}; expected {expected}"
                            );
                            append_diagnostic(&mut session.stderr_text, &error);
                            session.protocol_error.get_or_insert(error);
                            return Vec::new();
                        }
                    } else {
                        session.thread_id = Some(thread_id.to_string());
                    }
                }
                Vec::new()
            }
            Some("item.started") | Some("item.completed") => {
                Self::item_events(session, &value)
            }
            Some("turn.completed") => {
                let Some(prompt_id) = session.current_prompt.take() else {
                    return Vec::new();
                };
                session.state = CodexSessionState::Ready;
                if let Some(error) = session.protocol_error.take() {
                    return vec![AgentEvent::PromptError { prompt_id, error }];
                }
                session.stderr_text.clear();
                vec![AgentEvent::TurnComplete {
                    prompt_id,
                    stop_reason: StopReason::EndTurn,
                }]
            }
            Some("turn.failed") => {
                let Some(prompt_id) = session.current_prompt.take() else {
                    return Vec::new();
                };
                session.state = CodexSessionState::Ready;
                let error = json_string(value.key("message"))
                    .or_else(|| value.key("error").and_then(|v| json_string(v.key("message"))))
                    .filter(|message| !message.is_empty())
                    .map(str::to_string)
                    .unwrap_or_else(|| "Codex turn failed".to_string());
                vec![AgentEvent::PromptError { prompt_id, error }]
            }
            // A stream-level error can be followed by the authoritative
            // turn.failed event. Record it, but do not make the session Ready
            // while its process and turn are still alive.
            Some("error") => {
                let error = json_string(value.key("message"))
                    .or_else(|| value.key("error").and_then(|v| json_string(v.key("message"))))
                    .unwrap_or("Codex stream error");
                append_diagnostic(&mut session.stderr_text, error);
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn item_events(session: &mut CodexSession, value: &JsonValue) -> Vec<AgentEvent> {
        let Some(prompt_id) = session.current_prompt else {
            return Vec::new();
        };
        let Some(item) = value.key("item") else {
            return Vec::new();
        };
        let completed = json_string(value.key("type")) == Some("item.completed");
        match json_string(item.key("type")) {
            Some("agent_message") if completed => {
                let Some(text) = json_string(item.key("text")).filter(|text| !text.is_empty()) else {
                    return Vec::new();
                };
                let text = if session.emitted_text {
                    format!("\n\n{text}")
                } else {
                    session.emitted_text = true;
                    text.to_string()
                };
                vec![AgentEvent::TextDelta { prompt_id, text }]
            }
            Some("command_execution") if !completed => {
                let command = json_string(item.key("command")).unwrap_or_default();
                vec![AgentEvent::ToolRequest {
                    prompt_id,
                    tool_use_id: json_string(item.key("id")).unwrap_or_default().to_string(),
                    tool_name: "Bash".to_string(),
                    tool_input: command.to_string(),
                }]
            }
            Some("file_change")
                if completed && json_string(item.key("status")) == Some("completed") => vec![AgentEvent::ToolRequest {
                prompt_id,
                tool_use_id: json_string(item.key("id")).unwrap_or_default().to_string(),
                // The app treats native CLI Edit events as informational and
                // lands the changed game source immediately.
                tool_name: "Edit".to_string(),
                tool_input: file_change_subjects(item),
            }],
            Some("error") if completed => {
                let warning = json_string(item.key("message"))
                    .or_else(|| item.key("error").and_then(|v| json_string(v.key("message"))))
                    .unwrap_or("Codex reported a non-fatal item error");
                append_diagnostic(&mut session.stderr_text, warning);
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn drain_session(session: &mut CodexSession) -> Vec<AgentEvent> {
        let mut events = Vec::new();
        let mut stdout_closed = false;
        loop {
            let output = session
                .process
                .as_mut()
                .and_then(|process| process.receiver.try_recv().ok());
            let Some(output) = output else {
                break;
            };
            match output {
                CodexOutput::Stdout(line) => events.extend(Self::handle_stdout_line(session, &line)),
                CodexOutput::Stderr(line) => {
                    if !session.stderr_text.is_empty() {
                        session.stderr_text.push('\n');
                    }
                    session.stderr_text.push_str(&line);
                }
                CodexOutput::StdoutClosed => stdout_closed = true,
            }
        }
        session.output_closed |= stdout_closed;
        if session.output_closed {
            let status = session
                .process
                .as_mut()
                .and_then(|process| process.child.try_wait().ok().flatten());
            if let (Some(status), Some(prompt_id)) = (status.as_ref(), session.current_prompt.take()) {
                if let Some(error) = session.protocol_error.take() {
                    events.push(AgentEvent::PromptError { prompt_id, error });
                } else if status.success() {
                    events.push(AgentEvent::TurnComplete {
                        prompt_id,
                        stop_reason: StopReason::EndTurn,
                    });
                } else {
                    let error = if session.stderr_text.trim().is_empty() {
                        format!("Codex exited with status {status}")
                    } else {
                        session.stderr_text.clone()
                    };
                    events.push(AgentEvent::PromptError { prompt_id, error });
                }
                session.state = CodexSessionState::Ready;
            }
            // Only drop after try_wait reaped it. A JSON turn-complete event
            // can arrive before process exit; `Child`'s Drop does not reap.
            if status.is_some() {
                session.process = None;
                session.output_closed = false;
                session.state = CodexSessionState::Ready;
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
        let state = if self.cli_path.is_some() {
            CodexSessionState::Ready
        } else {
            CodexSessionState::Error
        };
        self.sessions.insert(
            session_id.0,
            CodexSession {
                state,
                cwd: config.cwd.unwrap_or_else(|| self.default_cwd.clone()),
                system_prompt: config.system_prompt,
                model: config.model,
                thread_id: config.resume_session_id,
                current_prompt: None,
                process: None,
                output_closed: false,
                stderr_text: String::new(),
                protocol_error: None,
                emitted_text: false,
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
        // A terminal JSON event can precede process exit. Never overwrite its
        // Child handle; reap it first or ask the UI to retry next frame.
        if session.current_prompt.is_none() && session.process.is_some() {
            let exited = session
                .process
                .as_mut()
                .and_then(|process| process.child.try_wait().ok().flatten())
                .is_some();
            if exited {
                session.process = None;
                session.output_closed = false;
                session.state = CodexSessionState::Ready;
            } else {
                self.queue_event(AgentEvent::PromptError {
                    prompt_id,
                    error: "Codex is finishing the previous turn; try again in a moment."
                        .to_string(),
                });
                return prompt_id;
            }
        }
        if session.state == CodexSessionState::Prompting {
            self.queue_event(AgentEvent::PromptError {
                prompt_id,
                error: "Codex is already handling a prompt.".to_string(),
            });
            return prompt_id;
        }
        let args = Self::build_args(session, text);
        match CodexProcess::start(&cli_path, &session.cwd, &args) {
            Ok(process) => {
                session.state = CodexSessionState::Prompting;
                session.current_prompt = Some(prompt_id);
                session.process = Some(process);
                session.output_closed = false;
                session.stderr_text.clear();
                session.protocol_error = None;
                session.emitted_text = false;
            }
            Err(error) => self.queue_event(AgentEvent::PromptError { prompt_id, error }),
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
        // Native Codex tools complete within `codex exec`; ToolRequest events
        // are presentation-only, just like the Claude Code backend.
    }

    fn cancel_prompt(&mut self, _cx: &mut Cx, prompt_id: PromptId) {
        for session in self.sessions.values_mut() {
            if session.current_prompt == Some(prompt_id) {
                if let Some(process) = &mut session.process {
                    process.kill();
                }
                session.process = None;
                session.output_closed = false;
                session.current_prompt = None;
                session.state = CodexSessionState::Ready;
                session.stderr_text.clear();
                session.protocol_error = None;
                session.emitted_text = false;
                break;
            }
        }
    }

    fn handle_event(&mut self, _cx: &mut Cx, event: &Event) -> Vec<AgentEvent> {
        let mut events = Vec::new();
        if let Event::Signal = event {
            events.append(&mut self.pending_events);
        }
        // UI agents receive regular frame/input events. Poll on all of them so
        // a child that exits just after stdout EOF cannot miss its only wakeup.
        for session in self.sessions.values_mut() {
            events.extend(Self::drain_session(session));
        }
        events
    }

    fn is_session_ready(&self, session_id: SessionId) -> bool {
        self.sessions
            .get(&session_id.0)
            .is_some_and(|session| session.state == CodexSessionState::Ready)
    }
}

fn json_string(value: Option<&JsonValue>) -> Option<&str> {
    value.and_then(JsonValue::string).map(String::as_str)
}

fn file_change_subjects(item: &JsonValue) -> String {
    let Some(JsonValue::Array(changes)) = item.key("changes") else {
        return String::new();
    };
    changes
        .iter()
        .filter_map(|change| json_string(change.key("path")))
        .collect::<Vec<_>>()
        .join(", ")
}

fn append_diagnostic(target: &mut String, message: &str) {
    if !target.is_empty() {
        target.push('\n');
    }
    target.push_str(message);
}

fn toml_basic_string(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('"');
    for ch in value.chars() {
        match ch {
            '\\' => quoted.push_str("\\\\"),
            '"' => quoted.push_str("\\\""),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push_str("\\t"),
            ch if ch <= '\u{001f}' || ch == '\u{007f}' => {
                use std::fmt::Write;
                let _ = write!(quoted, "\\u{:04X}", ch as u32);
            }
            ch => quoted.push(ch),
        }
    }
    quoted.push('"');
    quoted
}

#[cfg(unix)]
fn is_executable_path(path: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable_path(path: &str) -> bool {
    std::path::Path::new(path).is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(prompt: PromptId) -> CodexSession {
        CodexSession {
            state: CodexSessionState::Prompting,
            cwd: ".".into(),
            system_prompt: Some("system".into()),
            model: None,
            thread_id: None,
            current_prompt: Some(prompt),
            process: None,
            output_closed: false,
            stderr_text: String::new(),
            protocol_error: None,
            emitted_text: false,
        }
    }

    #[test]
    fn jsonl_maps_thread_text_edit_and_completion() {
        let prompt = PromptId::new();
        let mut session = session(prompt);
        assert!(CodexAgent::handle_stdout_line(
            &mut session,
            r#"{"type":"thread.started","thread_id":"0199-test"}"#,
        )
        .is_empty());
        assert_eq!(session.thread_id.as_deref(), Some("0199-test"));

        let text = CodexAgent::handle_stdout_line(
            &mut session,
            r#"{"type":"item.completed","item":{"id":"i1","type":"agent_message","text":"done"}}"#,
        );
        assert!(matches!(
            text.as_slice(),
            [AgentEvent::TextDelta { text, .. }] if text == "done"
        ));
        let edit = CodexAgent::handle_stdout_line(
            &mut session,
            r#"{"type":"item.completed","item":{"id":"i2","type":"file_change","status":"completed","changes":[{"path":"game.splash","kind":"update"}]}}"#,
        );
        assert!(matches!(
            edit.as_slice(),
            [AgentEvent::ToolRequest { tool_name, tool_input, .. }]
                if tool_name == "Edit" && tool_input == "game.splash"
        ));
        let complete = CodexAgent::handle_stdout_line(
            &mut session,
            r#"{"type":"turn.completed","usage":{"input_tokens":1}}"#,
        );
        assert!(matches!(complete.as_slice(), [AgentEvent::TurnComplete { .. }]));
        assert_eq!(session.state, CodexSessionState::Ready);
    }

    #[test]
    fn first_turn_is_workspace_sandboxed_and_resume_uses_thread() {
        let prompt = PromptId::new();
        let mut session = session(prompt);
        let args = CodexAgent::build_args(&session, "make a game");
        assert!(args.windows(2).any(|pair| pair == ["--sandbox", "workspace-write"]));
        assert!(args.iter().any(|arg| arg == "--ignore-user-config"));
        assert!(args.iter().any(|arg| arg == "--ignore-rules"));
        assert!(args
            .iter()
            .any(|arg| arg == "shell_environment_policy.inherit=none"));
        assert_eq!(args.last().map(String::as_str), Some("make a game"));
        assert!(args.iter().any(|arg| {
            arg == "developer_instructions=\"system\""
        }));

        session.thread_id = Some("0199-test".into());
        let args = CodexAgent::build_args(&session, "add a car");
        let resume = args.iter().position(|arg| arg == "resume").unwrap();
        assert!(resume > args.iter().position(|arg| arg == "workspace-write").unwrap());
        assert!(resume > args.iter().position(|arg| arg == "--ignore-rules").unwrap());
        assert_eq!(args[resume + 1], "0199-test");
        assert!(args.iter().any(|arg| arg == "0199-test"));
        assert_eq!(args.last().map(String::as_str), Some("add a car"));
        assert!(args[..resume]
            .iter()
            .any(|arg| arg == "developer_instructions=\"system\""));
    }

    #[test]
    fn developer_instructions_are_toml_quoted() {
        assert_eq!(
            toml_basic_string("edit \"game.splash\"\nonly\tthis"),
            "\"edit \\\"game.splash\\\"\\nonly\\tthis\""
        );
    }

    #[test]
    fn warnings_and_failed_file_changes_do_not_finish_or_land_the_turn() {
        let prompt = PromptId::new();
        let mut session = session(prompt);
        let warning = CodexAgent::handle_stdout_line(
            &mut session,
            r#"{"type":"error","message":"temporary reconnect"}"#,
        );
        assert!(warning.is_empty());
        assert_eq!(session.current_prompt, Some(prompt));
        assert_eq!(session.state, CodexSessionState::Prompting);
        assert!(session.stderr_text.contains("temporary reconnect"));

        let failed_edit = CodexAgent::handle_stdout_line(
            &mut session,
            r#"{"type":"item.completed","item":{"id":"i2","type":"file_change","status":"failed","changes":[{"path":"game.splash","kind":"update"}]}}"#,
        );
        assert!(failed_edit.is_empty());

        let malformed = CodexAgent::handle_stdout_line(&mut session, "not-json");
        assert!(malformed.is_empty());
        let complete = CodexAgent::handle_stdout_line(
            &mut session,
            r#"{"type":"turn.completed","usage":{"input_tokens":1}}"#,
        );
        assert!(matches!(complete.as_slice(), [AgentEvent::PromptError { .. }]));
    }
}
