//! Server-side Claude Code as a chat provider — the "broker" seam.
//!
//! Runs the locally installed Claude Code CLI headless (`-p`,
//! `--output-format stream-json`), one process per turn, resuming the same
//! CLI conversation across turns via `--resume`. Chat-only: `--tools ""`
//! disables every native tool and `--strict-mcp-config` pins an empty MCP
//! set, so the ONLY tools this lane can express are the content tools of
//! [`crate::toolcall`], executed by the dispatcher against the Asset
//! Server.
//!
//! Credentials: none pass through this crate. The CLI authenticates itself
//! on the broker host; the chat wire above carries text and typed events
//! only. This is the structural half of "no provider credentials in
//! clients" — a client machine without the CLI simply reports
//! `Unavailable`, it never receives key material to run one.
//!
//! This is a deliberate headless sibling of `makepad_ai`'s Cx-coupled
//! `ClaudeCodeAgent` (same CLI contract, same stream-json shapes) for
//! server processes that have no UI event loop.

use crate::provider::{ChatProvider, ProviderEvent, TurnInput};
use crate::wire::{ChatRole, ProviderAvailability, ProviderKind};
use makepad_asset_client::json::{self, Value};
use std::io::BufRead;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{channel, Receiver, Sender};

enum IoLine {
    Out(String),
    Err(String),
    Exit,
}

struct ActiveTurn {
    child: Child,
    rx: Receiver<IoLine>,
    parse: ParseState,
    stderr_tail: String,
    finished: bool,
}

/// Stream-parse state, separated from process plumbing so the line parser
/// is a pure, testable function.
#[derive(Default)]
pub struct ParseState {
    /// Text already delivered as deltas (dedupe base for full `assistant`
    /// messages that repeat streamed content).
    pub collected: String,
    /// The CLI's own conversation id, captured for `--resume`.
    pub session_id: Option<String>,
}

pub struct ClaudeCodeChatProvider {
    cli: Option<PathBuf>,
    model: Option<String>,
    resume: Option<String>,
    turn: Option<ActiveTurn>,
}

impl ClaudeCodeChatProvider {
    pub fn new(model: Option<String>) -> ClaudeCodeChatProvider {
        ClaudeCodeChatProvider { cli: find_cli(), model, resume: None, turn: None }
    }

    /// The CLI conversation id, for persisting broker sessions.
    pub fn native_session_id(&self) -> Option<&str> {
        self.resume.as_deref()
    }
}

/// CLI discovery: explicit env override first, then the documented install
/// locations. No PATH probing subprocess here — availability must be cheap
/// and deterministic.
pub fn find_cli() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("CLAUDE_CODE_PATH") {
        let p = PathBuf::from(p);
        return if p.is_file() { Some(p) } else { None };
    }
    let mut candidates = Vec::new();
    if let Ok(home) = std::env::var("HOME") {
        candidates.push(PathBuf::from(home).join(".local/bin/claude"));
    }
    candidates.push(PathBuf::from("/usr/local/bin/claude"));
    candidates.push(PathBuf::from("/opt/homebrew/bin/claude"));
    candidates.into_iter().find(|p| p.is_file())
}

/// Build the argv (after the executable), pure for testing. The prompt is
/// the trailing positional; `--tools ""` keeps the CLI chat-only.
pub fn build_args(
    model: &Option<String>,
    resume: &Option<String>,
    system: &str,
    prompt: &str,
) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "-p".into(),
        "--verbose".into(),
        "--output-format".into(),
        "stream-json".into(),
        "--include-partial-messages".into(),
        "--strict-mcp-config".into(),
        "--mcp-config".into(),
        r#"{"mcpServers":{}}"#.into(),
        "--tools".into(),
        String::new(),
    ];
    if let Some(m) = model {
        args.push("--model".into());
        args.push(m.clone());
    }
    if let Some(r) = resume {
        args.push("--resume".into());
        args.push(r.clone());
    }
    if !system.is_empty() {
        args.push("--system-prompt".into());
        args.push(system.to_string());
    }
    args.push(prompt.to_string());
    args
}

/// Render the prompt for one turn. With a resumable CLI conversation only
/// the NEW tail (messages after the last assistant reply) is sent; on a
/// fresh conversation the whole bounded history is rendered.
pub fn render_prompt(input: &TurnInput, resuming: bool) -> String {
    let messages: Vec<_> = if resuming {
        let last_assistant =
            input.messages.iter().rposition(|m| m.role == ChatRole::Assistant);
        match last_assistant {
            Some(i) => input.messages[i + 1..].iter().collect(),
            None => input.messages.iter().collect(),
        }
    } else {
        input.messages.iter().collect()
    };
    let mut out = String::new();
    for m in messages {
        match m.role {
            ChatRole::User => {
                out.push_str("[user]\n");
            }
            ChatRole::Tool => {
                out.push_str("[tool result]\n");
            }
            ChatRole::System => {
                out.push_str("[context]\n");
            }
            ChatRole::Assistant => continue,
        }
        out.push_str(&m.text);
        out.push('\n');
    }
    out
}

/// Parse one stdout line of the stream-json protocol into provider events.
/// Returns `(events, done)`; `done` means the CLI reported its result.
pub fn parse_stream_line(v: &Value, state: &mut ParseState) -> (Vec<ProviderEvent>, bool) {
    if let Some(sid) = v.get("session_id").and_then(Value::as_str) {
        state.session_id = Some(sid.to_string());
    }
    let mut events = Vec::new();
    match v.get("type").and_then(Value::as_str) {
        Some("stream_event") => {
            let delta = v
                .get("event")
                .and_then(|e| e.get("delta"))
                .filter(|d| d.get("type").and_then(Value::as_str) == Some("text_delta"))
                .and_then(|d| d.get("text"))
                .and_then(Value::as_str);
            if let Some(text) = delta {
                state.collected.push_str(text);
                events.push(ProviderEvent::Delta(text.to_string()));
            }
        }
        Some("assistant") => {
            // A full assistant message may repeat already-streamed text;
            // deliver only the unseen suffix.
            let mut full = String::new();
            if let Some(content) =
                v.get("message").and_then(|m| m.get("content")).and_then(Value::as_arr)
            {
                for block in content {
                    if block.get("type").and_then(Value::as_str) == Some("text") {
                        if let Some(t) = block.get("text").and_then(Value::as_str) {
                            full.push_str(t);
                        }
                    }
                }
            }
            if !full.is_empty() {
                if let Some(suffix) = full.strip_prefix(state.collected.as_str()) {
                    if !suffix.is_empty() {
                        events.push(ProviderEvent::Delta(suffix.to_string()));
                    }
                    state.collected = full;
                } else if state.collected.is_empty() {
                    events.push(ProviderEvent::Delta(full.clone()));
                    state.collected = full;
                }
            }
        }
        Some("result") => {
            let is_error = v.get("is_error").and_then(Value::as_bool).unwrap_or(false);
            if is_error {
                let msg = v
                    .get("result")
                    .and_then(Value::as_str)
                    .unwrap_or("Claude Code reported an error")
                    .to_string();
                events.push(ProviderEvent::Error(msg));
            } else {
                let text = if state.collected.is_empty() {
                    v.get("result").and_then(Value::as_str).unwrap_or("").to_string()
                } else {
                    state.collected.clone()
                };
                events.push(ProviderEvent::Done { text });
            }
            return (events, true);
        }
        _ => {}
    }
    (events, false)
}

impl ChatProvider for ClaudeCodeChatProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::ServerClaude
    }

    fn availability(&mut self) -> ProviderAvailability {
        match &self.cli {
            Some(p) => ProviderAvailability::Available {
                model: self.model.clone().unwrap_or_else(|| "claude-code".to_string()),
                detail: p.display().to_string(),
            },
            None => ProviderAvailability::Unavailable {
                reason: "Claude Code CLI not found on this host (set CLAUDE_CODE_PATH or install claude)"
                    .to_string(),
            },
        }
    }

    fn begin_turn(&mut self, input: &TurnInput) -> Result<(), String> {
        if self.turn.is_some() {
            return Err("a turn is already in flight".to_string());
        }
        let Some(cli) = self.cli.clone() else {
            return Err("Claude Code CLI not found".to_string());
        };
        let prompt = render_prompt(input, self.resume.is_some());
        let args = build_args(&self.model, &self.resume, &input.system, &prompt);
        let mut command = Command::new(cli);
        command.args(&args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        #[cfg(unix)]
        {
            // Own process group so cancel can address CLI descendants.
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child =
            command.spawn().map_err(|e| format!("failed to start Claude Code: {e}"))?;
        let (tx, rx) = channel();
        // Only the stdout reader signals Exit: stderr can hit EOF before the
        // final stdout `result` line is forwarded, and an early Exit would
        // mark the turn errored and drop the real result.
        spawn_reader(child.stdout.take(), tx.clone(), IoLine::Out, true);
        spawn_reader(child.stderr.take(), tx, IoLine::Err, false);
        self.turn = Some(ActiveTurn {
            child,
            rx,
            parse: ParseState::default(),
            stderr_tail: String::new(),
            finished: false,
        });
        Ok(())
    }

    fn poll(&mut self) -> Vec<ProviderEvent> {
        let Some(turn) = &mut self.turn else {
            return Vec::new();
        };
        let mut events = Vec::new();
        let mut close = false;
        while let Ok(line) = turn.rx.try_recv() {
            match line {
                IoLine::Out(line) => {
                    if turn.finished || line.trim().is_empty() {
                        continue;
                    }
                    if let Ok(v) = json::parse(line.as_bytes()) {
                        let (mut evs, done) = parse_stream_line(&v, &mut turn.parse);
                        events.append(&mut evs);
                        if done {
                            turn.finished = true;
                            close = true;
                        }
                    }
                }
                IoLine::Err(line) => {
                    if turn.stderr_tail.len() < 4096 {
                        turn.stderr_tail.push_str(&line);
                        turn.stderr_tail.push('\n');
                    }
                }
                IoLine::Exit => {
                    if !turn.finished {
                        let tail = turn.stderr_tail.trim();
                        events.push(ProviderEvent::Error(if tail.is_empty() {
                            "Claude Code exited without a result".to_string()
                        } else {
                            format!("Claude Code exited without a result: {tail}")
                        }));
                        turn.finished = true;
                    }
                    close = true;
                }
            }
        }
        if close {
            if let Some(mut turn) = self.turn.take() {
                if let Some(sid) = turn.parse.session_id.take() {
                    self.resume = Some(sid);
                }
                let _ = turn.child.wait();
            }
        }
        events
    }

    fn cancel(&mut self) {
        if let Some(mut turn) = self.turn.take() {
            let pid = turn.child.id();
            let _ = turn.child.kill();
            // Best-effort group kill for CLI descendants (we set a fresh
            // process group at spawn). std has no group-kill; /bin/kill does.
            #[cfg(unix)]
            {
                let _ = Command::new("/bin/kill")
                    .args(["-KILL", "--", &format!("-{pid}")])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
            let _ = turn.child.wait();
        }
    }
}

impl Drop for ClaudeCodeChatProvider {
    fn drop(&mut self) {
        self.cancel();
    }
}

fn spawn_reader<R: std::io::Read + Send + 'static>(
    stream: Option<R>,
    tx: Sender<IoLine>,
    wrap: fn(String) -> IoLine,
    signal_exit: bool,
) {
    let Some(stream) = stream else {
        return;
    };
    std::thread::spawn(move || {
        let reader = std::io::BufReader::new(stream);
        for line in reader.lines() {
            match line {
                Ok(line) => {
                    if tx.send(wrap(line)).is_err() {
                        return;
                    }
                }
                Err(_) => break,
            }
        }
        if signal_exit {
            let _ = tx.send(IoLine::Exit);
        }
    });
}
