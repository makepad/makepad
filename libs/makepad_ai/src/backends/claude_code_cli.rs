//! Claude Code CLI backend.
//!
//! V1 uses one headless `claude --print --output-format stream-json` process per
//! prompt. The command construction and stream parsing are intentionally split
//! into small helpers so tests do not need a real Claude installation.

use crate::agent::*;
use crate::types::*;
use makepad_widgets::*;
use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Read};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;

#[derive(Debug, Clone, Default)]
struct ClaudeCodeEnv {
    bin: Option<String>,
    model: Option<String>,
    permission_mode: Option<String>,
    path: Option<String>,
}

impl ClaudeCodeEnv {
    fn from_process_env() -> Self {
        Self {
            bin: std::env::var("CLAUDE_CODE_BIN")
                .ok()
                .filter(|s| !s.trim().is_empty()),
            model: std::env::var("CLAUDE_CODE_MODEL")
                .ok()
                .filter(|s| !s.trim().is_empty()),
            permission_mode: std::env::var("CLAUDE_CODE_PERMISSION_MODE")
                .ok()
                .filter(|s| !s.trim().is_empty()),
            path: std::env::var("PATH").ok(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ClaudeCommandSpec {
    program: String,
    args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ClaudeStreamEvent {
    Text(String),
    Done,
    Error(String),
    Ignored,
}

#[derive(Debug)]
struct ClaudeProcessEvent {
    session_id: SessionId,
    prompt_id: PromptId,
    event: ClaudeStreamEvent,
}

struct ClaudeCodeSession {
    ready: bool,
    config: SessionConfig,
    current_prompt: Option<PromptId>,
}

pub struct ClaudeCodeCliAgent {
    sessions: HashMap<LiveId, ClaudeCodeSession>,
    pending_events: VecDeque<AgentEvent>,
    process_sender: Sender<ClaudeProcessEvent>,
    process_receiver: Receiver<ClaudeProcessEvent>,
}

impl ClaudeCodeCliAgent {
    pub fn new() -> Self {
        let (process_sender, process_receiver) = mpsc::channel();
        Self {
            sessions: HashMap::new(),
            pending_events: VecDeque::new(),
            process_sender,
            process_receiver,
        }
    }

    pub fn is_available() -> bool {
        let env = ClaudeCodeEnv::from_process_env();
        find_claude_binary(&env, is_runtime_executable).is_some()
    }
}

impl Default for ClaudeCodeCliAgent {
    fn default() -> Self {
        Self::new()
    }
}

impl Agent for ClaudeCodeCliAgent {
    fn create_session(&mut self, _cx: &mut Cx, config: SessionConfig) -> SessionId {
        let session_id = SessionId::new();
        self.sessions.insert(
            session_id.0,
            ClaudeCodeSession {
                ready: true,
                config,
                current_prompt: None,
            },
        );
        self.pending_events
            .push_back(AgentEvent::SessionReady { session_id });
        session_id
    }

    fn send_prompt(&mut self, _cx: &mut Cx, session_id: SessionId, text: &str) -> PromptId {
        let prompt_id = PromptId::new();
        let env = ClaudeCodeEnv::from_process_env();
        let Some(program) = find_claude_binary(&env, is_runtime_executable) else {
            self.pending_events.push_back(AgentEvent::PromptError {
                prompt_id,
                error: "Claude Code CLI is not available. Set CLAUDE_CODE_BIN or install `claude` in PATH.".to_string(),
            });
            return prompt_id;
        };

        let (config, cwd) = match self.sessions.get_mut(&session_id.0) {
            Some(session) => {
                if session.current_prompt.is_some() {
                    self.pending_events.push_back(AgentEvent::PromptError {
                        prompt_id,
                        error: "Claude Code CLI backend only supports one in-flight prompt per session.".to_string(),
                    });
                    return prompt_id;
                }
                session.current_prompt = Some(prompt_id);
                (session.config.clone(), session.config.cwd.clone())
            }
            None => {
                self.pending_events.push_back(AgentEvent::PromptError {
                    prompt_id,
                    error: "Claude Code CLI session does not exist.".to_string(),
                });
                return prompt_id;
            }
        };

        let mut command_env = env;
        command_env.bin = Some(program);
        let spec = build_claude_command(&command_env, &config, text);
        spawn_claude_process(
            self.process_sender.clone(),
            session_id,
            prompt_id,
            spec,
            cwd,
        );
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
                session.current_prompt = None;
                self.pending_events.push_back(AgentEvent::PromptError {
                    prompt_id,
                    error: "Claude Code CLI prompt cancelled.".to_string(),
                });
                break;
            }
        }
    }

    fn handle_event(&mut self, _cx: &mut Cx, _event: &Event) -> Vec<AgentEvent> {
        let mut events = Vec::new();
        while let Some(event) = self.pending_events.pop_front() {
            events.push(event);
        }
        while let Ok(process_event) = self.process_receiver.try_recv() {
            if let Some(agent_event) =
                stream_event_to_agent_event(process_event.prompt_id, process_event.event)
            {
                match agent_event {
                    AgentEvent::TurnComplete { .. } | AgentEvent::PromptError { .. } => {
                        if let Some(session) = self.sessions.get_mut(&process_event.session_id.0) {
                            if session.current_prompt == Some(process_event.prompt_id) {
                                session.current_prompt = None;
                            }
                        }
                    }
                    _ => {}
                }
                events.push(agent_event);
            }
        }
        events
    }

    fn is_session_ready(&self, session_id: SessionId) -> bool {
        self.sessions
            .get(&session_id.0)
            .map(|session| session.ready)
            .unwrap_or(false)
    }
}

fn find_claude_binary<F>(env: &ClaudeCodeEnv, is_executable: F) -> Option<String>
where
    F: Fn(&str) -> bool,
{
    if let Some(bin) = env.bin.as_deref() {
        if is_executable(bin) {
            return Some(bin.to_string());
        }
    }

    let path = env.path.as_deref()?;
    for dir in std::env::split_paths(path) {
        let candidate = dir.join("claude");
        let candidate = candidate.to_string_lossy().to_string();
        if is_executable(&candidate) {
            return Some(candidate);
        }
    }
    None
}

fn build_claude_command(
    env: &ClaudeCodeEnv,
    config: &SessionConfig,
    prompt: &str,
) -> ClaudeCommandSpec {
    let program = env
        .bin
        .clone()
        .or_else(|| find_claude_binary(env, is_runtime_executable))
        .unwrap_or_else(|| "claude".to_string());

    let mut args = vec![
        "--print".to_string(),
        "--output-format".to_string(),
        "stream-json".to_string(),
        "--verbose".to_string(),
        "--include-partial-messages".to_string(),
    ];

    if let Some(model) = config
        .model
        .as_deref()
        .or(env.model.as_deref())
        .filter(|model| !model.trim().is_empty())
    {
        args.push("--model".to_string());
        args.push(model.to_string());
    }

    if let Some(permission_mode) = env
        .permission_mode
        .as_deref()
        .filter(|mode| is_safe_permission_mode(mode))
    {
        args.push("--permission-mode".to_string());
        args.push(permission_mode.to_string());
    }

    if let Some(system_prompt) = config
        .system_prompt
        .as_deref()
        .filter(|prompt| !prompt.trim().is_empty())
    {
        args.push("--append-system-prompt".to_string());
        args.push(system_prompt.to_string());
    }

    args.push(prompt.to_string());

    ClaudeCommandSpec { program, args }
}

fn is_safe_permission_mode(mode: &str) -> bool {
    matches!(
        mode,
        "default" | "acceptEdits" | "dontAsk" | "plan" | "auto"
    )
}

fn is_runtime_executable(path: &str) -> bool {
    let path = Path::new(path);
    if !path.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn parse_claude_stream_line(line: &str) -> ClaudeStreamEvent {
    let compact = line.split_whitespace().collect::<String>();
    if compact.contains(r#""type":"error""#) {
        return ClaudeStreamEvent::Error(
            extract_json_string_field(line, "message")
                .unwrap_or_else(|| "Claude Code CLI error".into()),
        );
    }
    if compact.contains(r#""type":"result""#) && compact.contains(r#""is_error":true"#) {
        return ClaudeStreamEvent::Error(
            extract_json_string_field(line, "result")
                .or_else(|| extract_json_string_field(line, "message"))
                .unwrap_or_else(|| "Claude Code CLI returned an error result".into()),
        );
    }
    if compact.contains(r#""type":"result""#) && compact.contains("success") {
        return ClaudeStreamEvent::Done;
    }
    if compact.contains("text_delta") {
        if let Some(text) = extract_json_string_field(line, "text") {
            return ClaudeStreamEvent::Text(text);
        }
    }
    ClaudeStreamEvent::Ignored
}

fn stream_event_to_agent_event(
    prompt_id: PromptId,
    event: ClaudeStreamEvent,
) -> Option<AgentEvent> {
    match event {
        ClaudeStreamEvent::Text(text) => Some(AgentEvent::TextDelta { prompt_id, text }),
        ClaudeStreamEvent::Done => Some(AgentEvent::TurnComplete {
            prompt_id,
            stop_reason: StopReason::EndTurn,
        }),
        ClaudeStreamEvent::Error(error) => Some(AgentEvent::PromptError { prompt_id, error }),
        ClaudeStreamEvent::Ignored => None,
    }
}

fn extract_json_string_field(line: &str, field: &str) -> Option<String> {
    let key = format!(r#""{}""#, field);
    let mut start = 0;
    while let Some(offset) = line[start..].find(&key) {
        let key_start = start + offset + key.len();
        let mut chars = line[key_start..].char_indices().peekable();
        while let Some((_, ch)) = chars.peek().copied() {
            if ch.is_whitespace() {
                chars.next();
            } else {
                break;
            }
        }
        if chars.next().map(|(_, ch)| ch) != Some(':') {
            start = key_start;
            continue;
        }
        while let Some((_, ch)) = chars.peek().copied() {
            if ch.is_whitespace() {
                chars.next();
            } else {
                break;
            }
        }
        let Some((quote_offset, '"')) = chars.next() else {
            start = key_start;
            continue;
        };
        let value_start = key_start + quote_offset + 1;
        return parse_json_string_value(&line[value_start..]);
    }
    None
}

fn parse_json_string_value(input: &str) -> Option<String> {
    let mut out = String::new();
    let mut escaped = false;
    let mut unicode_escape = String::new();
    let mut unicode_remaining = 0;

    for ch in input.chars() {
        if unicode_remaining > 0 {
            unicode_escape.push(ch);
            unicode_remaining -= 1;
            if unicode_remaining == 0 {
                if let Ok(code) = u32::from_str_radix(&unicode_escape, 16) {
                    if let Some(decoded) = char::from_u32(code) {
                        out.push(decoded);
                    }
                }
                unicode_escape.clear();
            }
            continue;
        }
        if escaped {
            match ch {
                '"' => out.push('"'),
                '\\' => out.push('\\'),
                '/' => out.push('/'),
                'b' => out.push('\u{0008}'),
                'f' => out.push('\u{000c}'),
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                'u' => unicode_remaining = 4,
                other => out.push(other),
            }
            escaped = false;
            continue;
        }
        match ch {
            '\\' => escaped = true,
            '"' => return Some(out),
            other => out.push(other),
        }
    }
    None
}

fn spawn_claude_process(
    sender: Sender<ClaudeProcessEvent>,
    session_id: SessionId,
    prompt_id: PromptId,
    spec: ClaudeCommandSpec,
    cwd: Option<String>,
) {
    thread::spawn(move || {
        let mut command = Command::new(&spec.program);
        command
            .args(&spec.args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if std::env::var("CLAUDE_CODE_INHERIT_API_KEY").ok().as_deref() != Some("1") {
            command.env_remove("ANTHROPIC_API_KEY");
        }
        if let Some(cwd) = cwd {
            command.current_dir(cwd);
        }

        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(err) => {
                send_process_event(
                    &sender,
                    session_id,
                    prompt_id,
                    ClaudeStreamEvent::Error(format!("failed to start Claude Code CLI: {err}")),
                );
                return;
            }
        };

        let mut stderr = child.stderr.take();
        let stderr_handle = thread::spawn(move || {
            let mut output = String::new();
            if let Some(mut stderr) = stderr.take() {
                let _ = stderr.read_to_string(&mut output);
            }
            output
        });

        if let Some(stdout) = child.stdout.take() {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                match line {
                    Ok(line) => {
                        let event = parse_claude_stream_line(&line);
                        if event != ClaudeStreamEvent::Ignored {
                            send_process_event(&sender, session_id, prompt_id, event);
                        }
                    }
                    Err(err) => {
                        send_process_event(
                            &sender,
                            session_id,
                            prompt_id,
                            ClaudeStreamEvent::Error(format!(
                                "failed to read Claude Code CLI output: {err}"
                            )),
                        );
                        break;
                    }
                }
            }
        }

        match child.wait() {
            Ok(status) if status.success() => {}
            Ok(status) => {
                let stderr = stderr_handle.join().unwrap_or_default();
                let message = if stderr.trim().is_empty() {
                    format!("Claude Code CLI exited with status {status}")
                } else {
                    stderr.trim().to_string()
                };
                send_process_event(
                    &sender,
                    session_id,
                    prompt_id,
                    ClaudeStreamEvent::Error(message),
                );
                return;
            }
            Err(err) => {
                send_process_event(
                    &sender,
                    session_id,
                    prompt_id,
                    ClaudeStreamEvent::Error(format!("failed to wait for Claude Code CLI: {err}")),
                );
                return;
            }
        }
        let _ = stderr_handle.join();
        send_process_event(&sender, session_id, prompt_id, ClaudeStreamEvent::Done);
    });
}

fn send_process_event(
    sender: &Sender<ClaudeProcessEvent>,
    session_id: SessionId,
    prompt_id: PromptId,
    event: ClaudeStreamEvent,
) {
    let _ = sender.send(ClaudeProcessEvent {
        session_id,
        prompt_id,
        event,
    });
    SignalToUI::set_ui_signal();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Agent;
    use std::path::PathBuf;

    #[test]
    fn claude_code_cli_public_export_compiles() {
        let agent = ClaudeCodeCliAgent::new();
        let _boxed: Box<dyn Agent> = Box::new(agent);
    }

    #[test]
    fn claude_code_cli_detects_configured_binary() {
        let env = ClaudeCodeEnv {
            bin: Some("/opt/claude/bin/claude".to_string()),
            ..Default::default()
        };
        let found = find_claude_binary(&env, |path| path == "/opt/claude/bin/claude");
        assert_eq!(found.as_deref(), Some("/opt/claude/bin/claude"));
    }

    #[test]
    fn claude_code_cli_detects_path_binary() {
        let env = ClaudeCodeEnv {
            path: Some(
                std::env::join_paths([PathBuf::from("/bin"), PathBuf::from("/usr/local/bin")])
                    .unwrap()
                    .to_string_lossy()
                    .to_string(),
            ),
            ..Default::default()
        };
        let expected = PathBuf::from("/usr/local/bin")
            .join("claude")
            .to_string_lossy()
            .to_string();
        let found = find_claude_binary(&env, |path| path == expected);
        assert_eq!(found.as_deref(), Some(expected.as_str()));
    }

    #[test]
    fn claude_code_cli_unavailable_without_binary() {
        let env = ClaudeCodeEnv {
            path: Some("/bin:/usr/local/bin".to_string()),
            ..Default::default()
        };
        assert!(find_claude_binary(&env, |_| false).is_none());
    }

    #[test]
    fn claude_code_cli_builds_headless_command() {
        let env = ClaudeCodeEnv {
            bin: Some("/mock/claude".to_string()),
            ..Default::default()
        };
        let spec = build_claude_command(&env, &SessionConfig::default(), "hello");

        assert_eq!(spec.program, "/mock/claude");
        assert!(spec.args.contains(&"--print".to_string()));
        assert!(spec
            .args
            .windows(2)
            .any(|w| w == ["--output-format", "stream-json"]));
        assert!(spec.args.contains(&"--verbose".to_string()));
        assert!(spec
            .args
            .contains(&"--include-partial-messages".to_string()));
        assert!(!spec.args.contains(&"--tmux".to_string()));
        assert_eq!(spec.args.last().map(String::as_str), Some("hello"));
    }

    #[test]
    fn claude_code_cli_builds_command_from_env_config() {
        let env = ClaudeCodeEnv {
            bin: Some("/mock/claude".to_string()),
            model: Some("sonnet".to_string()),
            permission_mode: Some("acceptEdits".to_string()),
            ..Default::default()
        };
        let config = SessionConfig {
            system_prompt: Some("system".to_string()),
            ..Default::default()
        };
        let spec = build_claude_command(&env, &config, "prompt");

        assert!(spec.args.windows(2).any(|w| w == ["--model", "sonnet"]));
        assert!(spec
            .args
            .windows(2)
            .any(|w| w == ["--permission-mode", "acceptEdits"]));
        assert!(spec
            .args
            .windows(2)
            .any(|w| w == ["--append-system-prompt", "system"]));
        assert!(!spec.args.iter().any(|arg| arg.contains("bypass")));

        let unsafe_env = ClaudeCodeEnv {
            bin: Some("/mock/claude".to_string()),
            permission_mode: Some("bypassPermissions".to_string()),
            ..Default::default()
        };
        let unsafe_spec = build_claude_command(&unsafe_env, &SessionConfig::default(), "prompt");
        assert!(!unsafe_spec.args.contains(&"--permission-mode".to_string()));
    }

    #[test]
    fn claude_code_cli_stream_json_text_deltas() {
        assert_eq!(
            parse_claude_stream_line(
                r#"{"type":"content_block_delta","delta":{"type":"text_delta","text":" world"}}"#
            ),
            ClaudeStreamEvent::Text(" world".to_string())
        );
        assert_eq!(
            parse_claude_stream_line(
                r#"{"type":"assistant","message":{"content":[{"type":"text","text":"world"}]}}"#
            ),
            ClaudeStreamEvent::Ignored
        );
    }

    #[test]
    fn claude_code_cli_stream_json_turn_complete() {
        assert_eq!(
            parse_claude_stream_line(r#"{"type":"result","subtype":"success"}"#),
            ClaudeStreamEvent::Done
        );
    }

    #[test]
    fn claude_code_cli_errors_surface_as_prompt_error() {
        let prompt_id = PromptId::new();
        let event = parse_claude_stream_line(r#"{"type":"error","message":"boom"}"#);
        let mapped = stream_event_to_agent_event(prompt_id, event);
        match mapped {
            Some(AgentEvent::PromptError { error, .. }) => assert_eq!(error, "boom"),
            other => panic!("expected PromptError, got {other:?}"),
        }
    }

    #[test]
    fn claude_code_cli_result_error_surfaces_as_prompt_error() {
        let event = parse_claude_stream_line(
            r#"{"type":"result","subtype":"success","is_error":true,"result":"Invalid API key"}"#,
        );
        assert_eq!(
            event,
            ClaudeStreamEvent::Error("Invalid API key".to_string())
        );
    }
}
