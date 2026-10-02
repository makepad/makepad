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
//! Process plumbing lives in [`crate::providers::cli`], shared with the `grok` CLI
//! (same Messages-format stream, [`parse_stream_line`]) and `codex`.
//!
//! [`ClaudeCodeChatProvider::live`] keeps ONE process across turns instead
//! (`--input-format stream-json`: a user message per stdin line, a result
//! line per turn): the conversation stays in that process, so a turn sends
//! only what is new since the last reply and pays no process start. The
//! process is started by the first turn; one that died (or a system prompt
//! that changed) is replaced at the next turn, which then sends the whole
//! history, as a fresh conversation does.

use crate::chat_wire::{ChatRole, ProviderAvailability, ProviderKind};
use crate::providers::cli::{categorize_cli_error, cli_command, turn_dir, CliTurn};
use crate::providers::provider::{validate_tool_images, ChatProvider, ProviderEvent, ToolImage, TurnInput};
use makepad_strict_json::{self as json, Value};
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
    /// An MCP server whose tools the model calls natively: the
    /// `--mcp-config` JSON and the `--allowedTools` pattern.
    mcp: Option<(String, String)>,
    resume: Option<String>,
    turn: Option<(CliTurn, ParseState)>,
    /// Images for the next turn: sent as image blocks of a stream-json
    /// user message beside the rendered prompt.
    images: Vec<ToolImage>,
    /// Keep one process across turns ([`Self::live`]), and that process.
    live: bool,
    /// `--effort` (low … max), when set.
    effort: Option<String>,
    session: Option<LiveSession>,
}

/// The process of a live provider: its streams, the line writer to its
/// stdin, the system prompt it was started with, and the parse of the turn
/// in flight (none between turns).
struct LiveSession {
    cli: CliTurn,
    stdin: std::sync::mpsc::Sender<String>,
    system: String,
    parse: Option<ParseState>,
}

impl LiveSession {
    fn end(self) {
        drop(self.stdin);
        self.cli.kill_group();
    }
}

impl ClaudeCodeChatProvider {
    pub fn new(model: Option<String>) -> ClaudeCodeChatProvider {
        ClaudeCodeChatProvider { cli: find_cli(), model, mcp: None, resume: None, turn: None, images: Vec::new(), live: false, effort: None, session: None }
    }

    /// One process for the whole conversation (see the module doc).
    pub fn live(mut self) -> Self {
        self.live = true;
        self
    }

    /// How hard the model thinks (`--effort`: low, medium, high, xhigh,
    /// max): low for quick, small answers.
    pub fn effort(mut self, level: &str) -> Self {
        self.effort = Some(level.to_string());
        self
    }

    fn begin_live(&mut self, cli: PathBuf, input: &TurnInput) -> Result<(), String> {
        if self.session.as_mut().is_some_and(|s| s.system != input.system || !s.cli.alive()) {
            if let Some(session) = self.session.take() {
                session.end();
            }
        }
        let resuming = self.session.is_some();
        let mut prompt = render_prompt(input, resuming);
        if !input.dynamic_context.is_empty() {
            // The system stays as the process started with it: what changes
            // per turn goes in the turn.
            prompt = format!("[context]\n{}\n{prompt}", input.dynamic_context);
        }
        let line = image_message(&prompt, &std::mem::take(&mut self.images));
        if self.session.is_none() {
            let mut args = build_args_with_mcp(&self.model, &None, &input.system, self.mcp.as_ref());
            args.insert(1, "--input-format".into());
            args.insert(2, "stream-json".into());
            if let Some(effort) = &self.effort {
                args.insert(3, "--effort".into());
                args.insert(4, effort.clone());
            }
            let dir = turn_dir("claude");
            let mut command = cli_command(&cli, &dir);
            command.args(&args);
            if self.mcp.is_some() {
                command.env("MCP_TOOL_TIMEOUT", "900000");
            }
            let (cli, stdin) = CliTurn::spawn_live(command, "Claude Code", Some(dir))?;
            self.session = Some(LiveSession { cli, stdin, system: input.system.clone(), parse: None });
        }
        let session = self.session.as_mut().expect("started above");
        if session.stdin.send(line).is_err() {
            if let Some(session) = self.session.take() {
                session.end();
            }
            return Err("Claude Code CLI exited".to_string());
        }
        session.cli.finished = false;
        session.parse = Some(ParseState::default());
        Ok(())
    }

    fn poll_live(&mut self) -> Vec<ProviderEvent> {
        let Some(session) = self.session.as_mut() else { return Vec::new() };
        let drained = session.cli.drain();
        let mut events = Vec::new();
        if let Some(parse) = session.parse.as_mut() {
            for line in drained.lines {
                let Ok(v) = json::parse(line.as_bytes()) else { continue };
                if v.get("type").and_then(Value::as_str) == Some("result") {
                    if let Some(usage) = usage_note(&v) {
                        eprintln!("chat Claude Code turn: {usage}");
                    }
                }
                let (mut evs, done) = parse_stream_line(&v, parse);
                for ev in &mut evs {
                    if let ProviderEvent::Error(raw) = ev {
                        *ev = ProviderEvent::Error(categorize_cli_error("Claude Code", raw, false));
                    }
                }
                events.append(&mut evs);
                if done {
                    session.cli.finished = true;
                    break;
                }
            }
            if drained.exited && !session.cli.finished {
                events.push(ProviderEvent::Error(session.cli.exit_error("Claude Code")));
            }
            if session.cli.finished || drained.exited {
                session.parse = None;
            }
        }
        if drained.exited {
            if let Some(session) = self.session.take() {
                session.cli.wait();
            }
        }
        events
    }

    /// Give the model the tools of one MCP server (`config` is the
    /// `{"mcpServers":{...}}` JSON, `allowed` the `--allowedTools` pattern,
    /// e.g. `mcp__sandbox`). Its calls then run inside the CLI's own agent
    /// loop — many per turn, results and images native — instead of the
    /// session's one-call-per-process text protocol.
    pub fn with_mcp(mut self, config: String, allowed: String) -> Self {
        self.mcp = Some((config, allowed));
        self
    }

    /// The CLI conversation id, for persisting broker sessions.
    pub fn native_session_id(&self) -> Option<&str> {
        self.resume.as_deref()
    }
}

/// `CLAUDE_CODE_PATH`, else `claude` on `$PATH` or in the usual dirs.
pub fn find_cli() -> Option<PathBuf> {
    crate::providers::cli::find_cli("CLAUDE_CODE_PATH", "claude", &[])
}

/// Build the argv (after the executable), pure for testing. The prompt is
/// NOT here — it is written to stdin (see the module doc). `--tools ""`
/// keeps the CLI chat-only.
pub fn build_args(model: &Option<String>, resume: &Option<String>, system: &str) -> Vec<String> {
    build_args_with_mcp(model, resume, system, None)
}

/// [`build_args`] with an MCP server whose tools are allowed without a
/// prompt (the built-in tools stay off).
pub fn build_args_with_mcp(model: &Option<String>, resume: &Option<String>, system: &str, mcp: Option<&(String, String)>) -> Vec<String> {
    let mut args: Vec<String> = vec![
        "-p".into(),
        "--verbose".into(),
        "--output-format".into(),
        "stream-json".into(),
        "--include-partial-messages".into(),
        "--strict-mcp-config".into(),
        "--mcp-config".into(),
        mcp.map_or(r#"{"mcpServers":{}}"#.to_string(), |(config, _)| config.clone()),
    ];
    if let Some((_, allowed)) = mcp {
        args.push("--allowedTools".into());
        args.push(allowed.clone());
    }
    args.push("--tools".into());
    args.push(String::new());
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

/// The stdin line for a turn that carries images: one stream-json user
/// message (`--input-format stream-json`) with the prompt as a text block
/// and each PNG as a base64 image block.
pub fn image_message(prompt: &str, images: &[ToolImage]) -> String {
    let mut content = vec![json::obj(vec![("type", json::s("text")), ("text", json::s(prompt))])];
    for image in images {
        let data = String::from_utf8(makepad_base64::base64_encode(&image.png, &makepad_base64::BASE64_STANDARD)).unwrap_or_default();
        content.push(json::obj(vec![("type", json::s("text")), ("text", json::s(format!("[image: {}]", image.label)))]));
        content.push(json::obj(vec![
            ("type", json::s("image")),
            ("source", json::obj(vec![("type", json::s("base64")), ("media_type", json::s("image/png")), ("data", json::s(data))])),
        ]));
    }
    let message = json::obj(vec![
        ("type", json::s("user")),
        ("message", json::obj(vec![("role", json::s("user")), ("content", Value::Arr(content))])),
    ]);
    let mut line = message.to_json();
    line.push('\n');
    line
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
            // A new text block after tool calls starts on its own line (an
            // MCP-tool turn narrates between calls).
            let event = v.get("event");
            if event.and_then(|e| e.get("type")).and_then(Value::as_str) == Some("content_block_start")
                && event.and_then(|e| e.get("content_block")).and_then(|b| b.get("type")).and_then(Value::as_str) == Some("text")
                && !state.collected.is_empty() && !state.collected.ends_with('\n')
            {
                state.collected.push('\n');
                events.push(ProviderEvent::Delta("\n".to_string()));
            }
            let delta = event.and_then(|e| e.get("delta"));
            match delta.and_then(|d| d.get("type")).and_then(Value::as_str) {
                Some("text_delta") => {
                    if let Some(text) = delta.and_then(|d| d.get("text")).and_then(Value::as_str) {
                        close_think(state, &mut events);
                        state.collected.push_str(text);
                        events.push(ProviderEvent::Delta(text.to_string()));
                    }
                }
                Some("thinking_delta") => {
                    // Redacted/summarised thinking arrives as empty deltas;
                    // an empty block is not worth a think marker.
                    if let Some(text) =
                        delta.and_then(|d| d.get("thinking")).and_then(Value::as_str).filter(|t| !t.is_empty())
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
        if self.turn.is_some() || self.session.as_ref().is_some_and(|s| s.parse.is_some()) {
            return Err("a turn is already in flight".to_string());
        }
        let Some(cli) = self.cli.clone() else {
            return Err("Claude Code CLI not found".to_string());
        };
        if self.live {
            return self.begin_live(cli, input);
        }
        let mut prompt = render_prompt(input, self.resume.is_some());
        let mut args = build_args_with_mcp(&self.model, &self.resume, &input.system_with_dynamic(), self.mcp.as_ref());
        let images = std::mem::take(&mut self.images);
        if !images.is_empty() {
            // Images need the structured input: one user message whose
            // content is the prompt plus the image blocks.
            args.insert(1, "--input-format".into());
            args.insert(2, "stream-json".into());
            prompt = image_message(&prompt, &images);
        }
        let dir = turn_dir("claude");
        let mut command = cli_command(&cli, &dir);
        command.args(&args);
        if self.mcp.is_some() {
            // A game tool may run a long generation; the CLI's default MCP
            // call timeout would abandon it.
            command.env("MCP_TOOL_TIMEOUT", "900000");
        }
        let turn = CliTurn::spawn(command, Some(prompt), "Claude Code", Some(dir))?;
        self.turn = Some((turn, ParseState::default()));
        Ok(())
    }

    fn poll(&mut self) -> Vec<ProviderEvent> {
        if self.live {
            return self.poll_live();
        }
        poll_messages_turn(&mut self.turn, &mut self.resume, "Claude Code")
    }

    fn tools_over_mcp(&self) -> bool {
        self.mcp.is_some()
    }

    fn attach_tool_images(&mut self, images: Vec<ToolImage>) -> Result<(), String> {
        validate_tool_images(&images)?;
        if self.turn.is_some() || self.session.as_ref().is_some_and(|s| s.parse.is_some()) {
            return Err("images must be attached between turns".into());
        }
        self.images = images;
        Ok(())
    }

    fn cancel(&mut self) {
        self.images.clear();
        if let Some((cli, _)) = self.turn.take() {
            cli.kill_group();
        }
        // A live process mid-turn is ended (its reply would arrive as the
        // next turn's); the next turn starts another with the history.
        if self.session.as_ref().is_some_and(|s| s.parse.is_some()) {
            if let Some(session) = self.session.take() {
                session.end();
            }
        }
    }
}

impl Drop for ClaudeCodeChatProvider {
    fn drop(&mut self) {
        self.cancel();
        if let Some(session) = self.session.take() {
            session.end();
        }
    }
}

/// A turn's token counts from its result line, for the log.
fn usage_note(result: &Value) -> Option<String> {
    let usage = result.get("usage")?;
    let n = |key: &str| usage.get(key).and_then(Value::as_i64).unwrap_or(0);
    Some(format!(
        "in {} + cache write {} + cache read {}, out {}",
        n("input_tokens"), n("cache_creation_input_tokens"), n("cache_read_input_tokens"), n("output_tokens")
    ))
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
    fn empty_thinking_opens_no_block_and_later_text_starts_a_new_line() {
        let mut state = ParseState::default();
        let mut all = Vec::new();
        for line in [
            r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":""}}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_start","content_block":{"type":"text","text":""}}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"Building."}}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":""}}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_start","content_block":{"type":"text","text":""}}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"Done."}}}"#,
        ] {
            let (events, _) = parse_stream_line(&json::parse(line.as_bytes()).unwrap(), &mut state);
            all.extend(events);
        }
        let text: String = all.iter().filter_map(|e| match e { ProviderEvent::Delta(t) => Some(t.as_str()), _ => None }).collect();
        assert_eq!(text, "Building.\nDone.");
    }

    #[test]
    fn an_mcp_server_is_passed_and_allowed_with_the_builtin_tools_off() {
        let mcp = (r#"{"mcpServers":{"sandbox":{"type":"http","url":"http://127.0.0.1:1/mcp"}}}"#.to_string(), "mcp__sandbox".to_string());
        let args = build_args_with_mcp(&Some("claude-opus-5-5".into()), &None, "sys", Some(&mcp));
        assert!(args.windows(2).any(|w| w[0] == "--mcp-config" && w[1] == mcp.0));
        assert!(args.windows(2).any(|w| w[0] == "--allowedTools" && w[1] == "mcp__sandbox"));
        assert!(args.windows(2).any(|w| w[0] == "--tools" && w[1].is_empty()));
        assert!(args.windows(2).any(|w| w[0] == "--model" && w[1] == "claude-opus-5-5"));
        let provider = ClaudeCodeChatProvider::new(None);
        assert!(!provider.tools_over_mcp());
        assert!(provider.with_mcp(mcp.0.clone(), mcp.1.clone()).tools_over_mcp());
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
