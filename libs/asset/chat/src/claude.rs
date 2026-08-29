//! Server-side Claude Code as a chat provider — the "broker" seam.
//!
//! Runs the locally installed Claude Code CLI headless (`-p`,
//! `--output-format stream-json`), one process per turn, resuming the same
//! CLI conversation across turns via `--resume`. Chat-only: `--tools ""`
//! disables every native tool and `--strict-mcp-config` pins an empty MCP
//! set, so the ONLY tools this lane can express are the content tools of
//! [`crate::toolcall`], executed by the dispatcher against the Asset
//! Server. The prompt goes over stdin: since Claude Code 2.1 a trailing
//! positional after `--tools ""` is swallowed as a tool name and the CLI
//! exits with "Input must be provided" (observed live, 2.1.246).
//!
//! Credentials: none pass through this crate. The CLI authenticates itself
//! on the broker host; the chat wire above carries text and typed events
//! only. This is the structural half of "no provider credentials in
//! clients" — a client machine without the CLI simply reports
//! `Unavailable`, it never receives key material to run one.
//!
//! Process plumbing lives in [`crate::cli`], shared with the `grok` CLI
//! (same Messages-format stream, [`parse_stream_line`]) and `codex`.

use crate::cli::{categorize_cli_error, cli_command, turn_dir, CliTurn};
use crate::provider::{ChatProvider, ProviderEvent, TurnInput};
use crate::wire::{ChatRole, ProviderAvailability, ProviderKind};
use makepad_asset_client::json::{self, Value};
use std::path::PathBuf;

/// Stream-parse state, separated from process plumbing so the line parser
/// is a pure, testable function.
#[derive(Default)]
pub struct ParseState {
    /// Visible text already delivered as deltas (dedupe base for full
    /// `assistant` messages that repeat streamed content; the `Done` text).
    pub collected: String,
    /// The CLI's own conversation id, captured for `--resume`.
    pub session_id: Option<String>,
    /// A `<think>` marker has been emitted and not closed yet: the CLI is
    /// streaming reasoning (`thinking_delta`). It is forwarded inside the
    /// same textual think block the fleet Qwen lane emits, so every client
    /// renders reasoning one way and none of it lands in the history.
    pub thinking_open: bool,
}

pub struct ClaudeCodeChatProvider {
    cli: Option<PathBuf>,
    model: Option<String>,
    resume: Option<String>,
    turn: Option<(CliTurn, ParseState)>,
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

/// `CLAUDE_CODE_PATH`, else `claude` on `$PATH` or in the usual dirs.
pub fn find_cli() -> Option<PathBuf> {
    crate::cli::find_cli("CLAUDE_CODE_PATH", "claude", &[])
}

/// Build the argv (after the executable), pure for testing. The prompt is
/// NOT here — it is written to stdin (see the module doc). `--tools ""`
/// keeps the CLI chat-only.
pub fn build_args(model: &Option<String>, resume: &Option<String>, system: &str) -> Vec<String> {
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
    args
}

/// Render the prompt for one turn. With a resumable CLI conversation only
/// the NEW tail (messages after the last assistant reply) is sent — the
/// CLI already holds everything before it; on a fresh conversation (first
/// turn, or a broker restart resuming a persisted transcript) the WHOLE
/// bounded history is rendered, assistant replies included as labelled
/// `[assistant]` context — without them a resumed conversation would show
/// the model only one side of itself.
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
            ChatRole::Assistant => {
                out.push_str("[assistant]\n");
            }
        }
        out.push_str(&m.text);
        out.push('\n');
    }
    out
}

/// The rendered user-side prompt for one turn (codex reads it from stdin
/// via the `-` positional, grok from a file in the 0700 turn dir; same
/// text — never argv, where any user on the host could read it from the
/// process listing).
pub fn build_prompt_only(input: &TurnInput, resuming: bool) -> String {
    render_prompt(input, resuming)
}

fn close_think(state: &mut ParseState, events: &mut Vec<ProviderEvent>) {
    if state.thinking_open {
        events.push(ProviderEvent::Delta("</think>\n".to_string()));
        state.thinking_open = false;
    }
}

/// Parse one stdout line of the Anthropic Messages stream protocol (Claude
/// Code `stream-json`, grok `streaming-messages-json`) into provider
/// events. Returns `(events, done)`; `done` means the CLI reported its
/// result.
pub fn parse_stream_line(v: &Value, state: &mut ParseState) -> (Vec<ProviderEvent>, bool) {
    if let Some(sid) = v.get("session_id").and_then(Value::as_str) {
        state.session_id = Some(sid.to_string());
    }
    let mut events = Vec::new();
    match v.get("type").and_then(Value::as_str) {
        Some("stream_event") => {
            let delta = v.get("event").and_then(|e| e.get("delta"));
            match delta.and_then(|d| d.get("type")).and_then(Value::as_str) {
                Some("text_delta") => {
                    if let Some(text) = delta.and_then(|d| d.get("text")).and_then(Value::as_str) {
                        close_think(state, &mut events);
                        state.collected.push_str(text);
                        events.push(ProviderEvent::Delta(text.to_string()));
                    }
                }
                Some("thinking_delta") => {
                    if let Some(text) =
                        delta.and_then(|d| d.get("thinking")).and_then(Value::as_str)
                    {
                        if !state.thinking_open {
                            events.push(ProviderEvent::Delta("<think>".to_string()));
                            state.thinking_open = true;
                        }
                        events.push(ProviderEvent::Delta(text.to_string()));
                    }
                }
                _ => {}
            }
        }
        Some("assistant") => {
            // A full assistant message may repeat already-streamed text;
            // deliver only the unseen suffix.
            close_think(state, &mut events);
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
            close_think(state, &mut events);
            let is_error = v.get("is_error").and_then(Value::as_bool).unwrap_or(false);
            if is_error {
                let msg = v
                    .get("result")
                    .and_then(Value::as_str)
                    .unwrap_or("the CLI reported an error")
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

/// Poll a Messages-format CLI turn: parse what arrived, end the turn on the
/// result line or on an early exit. Shared with the grok provider.
///
/// Vendor-reported error text (the `result` line's message) is mapped to a
/// fixed public category here and logged server-side; raw CLI output never
/// becomes a wire error.
pub fn poll_messages_turn(
    turn: &mut Option<(CliTurn, ParseState)>,
    resume: &mut Option<String>,
    what: &str,
) -> Vec<ProviderEvent> {
    let Some((cli, parse)) = turn.as_mut() else {
        return Vec::new();
    };
    let mut events = Vec::new();
    let drained = cli.drain();
    for line in drained.lines {
        if cli.finished {
            break;
        }
        if let Ok(v) = json::parse(line.as_bytes()) {
            let (mut evs, done) = parse_stream_line(&v, parse);
            for ev in &mut evs {
                if let ProviderEvent::Error(raw) = ev {
                    let public = categorize_cli_error(what, raw, false);
                    *ev = ProviderEvent::Error(public);
                }
            }
            events.append(&mut evs);
            if done {
                cli.finished = true;
            }
        }
    }
    if drained.exited && !cli.finished {
        events.push(ProviderEvent::Error(cli.exit_error(what)));
        cli.finished = true;
    }
    if cli.finished && (drained.exited || events.iter().any(|e| matches!(e, ProviderEvent::Done { .. } | ProviderEvent::Error(_)))) {
        if let Some((cli, mut parse)) = turn.take() {
            if let Some(sid) = parse.session_id.take() {
                *resume = Some(sid);
            }
            cli.wait();
        }
    }
    events
}

impl ChatProvider for ClaudeCodeChatProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::ClaudeCli
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
        let args = build_args(&self.model, &self.resume, &input.system);
        let dir = turn_dir("claude");
        let mut command = cli_command(&cli, &dir);
        command.args(&args);
        let turn = CliTurn::spawn(command, Some(prompt), "Claude Code", Some(dir))?;
        self.turn = Some((turn, ParseState::default()));
        Ok(())
    }

    fn poll(&mut self) -> Vec<ProviderEvent> {
        poll_messages_turn(&mut self.turn, &mut self.resume, "Claude Code")
    }

    fn cancel(&mut self) {
        if let Some((cli, _)) = self.turn.take() {
            cli.kill_group();
        }
    }
}

impl Drop for ClaudeCodeChatProvider {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_prompt_is_not_in_argv() {
        let args = build_args(&None, &Some("sid".into()), "be brief");
        assert_eq!(args[0], "-p");
        assert!(args.windows(2).any(|w| w[0] == "--resume" && w[1] == "sid"));
        assert!(args.windows(2).any(|w| w[0] == "--system-prompt" && w[1] == "be brief"));
        // `--tools ""` must be the last positional-looking pair or it eats
        // whatever follows; nothing follows.
        assert_eq!(args.last().map(String::as_str), Some("be brief"));
        let bare = build_args(&None, &None, "");
        assert_eq!(bare.last().map(String::as_str), Some(""));
        assert_eq!(bare[bare.len() - 2], "--tools");
    }

    #[test]
    fn thinking_streams_as_one_think_block() {
        let mut state = ParseState::default();
        let mut all = Vec::new();
        for line in [
            r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":"hm"}},"session_id":"s1"}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":"m."}}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"hi"}}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"hi there"}]}}"#,
            r#"{"type":"result","is_error":false,"result":"hi there"}"#,
        ] {
            let (events, _) = parse_stream_line(&json::parse(line.as_bytes()).unwrap(), &mut state);
            all.extend(events);
        }
        let text: String = all
            .iter()
            .filter_map(|e| match e {
                ProviderEvent::Delta(t) => Some(t.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(text, "<think>hmm.</think>\nhi there");
        assert!(matches!(all.last(), Some(ProviderEvent::Done { text }) if text == "hi there"));
        assert_eq!(state.session_id.as_deref(), Some("s1"));
    }
}
