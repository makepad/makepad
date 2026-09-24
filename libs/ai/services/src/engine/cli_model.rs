//! The person's own coding CLIs as the chat's model: Claude Code
//! (`claude -p`) and Codex (`codex exec`), through the hub's CLI providers.
//!
//! The CLIs are logged in by the person on this machine, so no key exists
//! anywhere in our stack. They run chat-only (no file or shell tools, an
//! empty private working directory per turn), so the app's tools reach
//! them as a TEXTUAL protocol: the system prompt carries the tool table
//! and asks for `<tool_call>{"name":…,"arguments":{…}}</tool_call>` blocks
//! at the end of a reply. This adapter parses those blocks when the turn
//! ends, hands them to the engine as ordinary [`ModelEvent::ToolCall`]s,
//! and sends the batch of results back as the next CLI turn (the CLI
//! resumes its own conversation, so only the new messages travel).
//!
//! Images ([`Model::attach_images`]) ride with the next turn: Claude Code
//! gets them as image blocks of a stream-json user message, Codex as
//! `--image` files.

use crate::engine::{Model, ModelEvent, ModelImage, ToolDefinition};
use makepad_ai_hub::chat_wire::{ChatMessage, ChatRole, ProviderAvailability};
use makepad_ai_hub::providers::claude::ClaudeCodeChatProvider;
use makepad_ai_hub::providers::codex_cli::CodexCliChatProvider;
use makepad_ai_hub::providers::provider::{ChatProvider, ProviderEvent, ToolImage, TurnInput};
use makepad_strict_json as json;

/// Which CLI answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CliKind {
    ClaudeCode,
    Codex,
}

impl CliKind {
    pub fn slug(self) -> &'static str {
        match self {
            CliKind::ClaudeCode => "claude-cli",
            CliKind::Codex => "codex-cli",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            CliKind::ClaudeCode => "Claude Code",
            CliKind::Codex => "Codex",
        }
    }

    pub fn from_slug(slug: &str) -> Option<CliKind> {
        match slug {
            "claude-cli" => Some(CliKind::ClaudeCode),
            "codex-cli" => Some(CliKind::Codex),
            _ => None,
        }
    }

    fn provider(self) -> Box<dyn ChatProvider + Send> {
        match self {
            CliKind::ClaudeCode => Box::new(ClaudeCodeChatProvider::new(None)),
            CliKind::Codex => Box::new(CodexCliChatProvider::new(None)),
        }
    }

    /// Whether the CLI is installed here: `None` when it is, else why not.
    pub fn unavailable(self) -> Option<String> {
        match self.provider().availability() {
            ProviderAvailability::Unavailable { reason } => Some(reason),
            _ => None,
        }
    }
}

const OPEN: &str = "<tool_call>";
const CLOSE: &str = "</tool_call>";
/// Tool calls one reply may make; more are refused back to the model.
const MAX_CALLS_PER_REPLY: usize = 16;

/// The protocol paragraph and the tool table, appended to the system.
fn tool_protocol(tools: &[ToolDefinition]) -> String {
    let mut out = String::from(
        "\n# How to use the tools\n\
         You act ONLY through the tools listed below; you have no file, shell or web access. \
         To call tools, end your reply with one or more blocks, each exactly:\n\
         <tool_call>{\"name\": \"<tool name>\", \"arguments\": {...}}</tool_call>\n\
         Write nothing after the last block, and never write a result yourself: the real results come in the next message. Several blocks in one reply run in order, and all their \
         results come back together in the next message as [tool result] sections; then continue. \
         Prefer several small calls in one reply over one call per reply. \
         When you need no tool, just answer in plain text (short).\n\
         Tools, one JSON object per line (name, description, JSON-schema parameters):\n",
    );
    for t in tools {
        out.push_str(
            &json::obj(vec![
                ("name", json::s(t.name.clone())),
                ("description", json::s(t.description.clone())),
                ("parameters", json::parse(t.parameters.as_bytes()).unwrap_or(json::Value::Obj(Vec::new()))),
            ])
            .to_json(),
        );
        out.push('\n');
    }
    out
}

/// One parsed block: `Ok((name, args-json))` or why it did not parse.
type ParsedCall = Result<(String, String), String>;

/// Where the next tool block starts: ours (`<tool_call>`), or the XML
/// form a model trained on native tool use sometimes writes instead
/// (`<invoke name="…"><parameter name="…">…</parameter></invoke>`, maybe
/// inside `<function_calls>`), which is read the same way.
fn next_block(text: &str) -> Option<usize> {
    [OPEN, "<function_calls>", "<invoke name="].iter().filter_map(|m| text.find(m)).min()
}

/// The reply's visible text (without the tool blocks) and its calls.
pub fn split_reply(text: &str) -> (String, Vec<ParsedCall>) {
    let mut visible = String::new();
    let mut calls = Vec::new();
    let mut rest = text;
    while let Some(at) = next_block(rest) {
        visible.push_str(&rest[..at]);
        let block = &rest[at..];
        if let Some(after) = block.strip_prefix(OPEN) {
            let (body, next) = match after.find(CLOSE) {
                Some(end) => (&after[..end], &after[end + CLOSE.len()..]),
                // An unclosed last block: take the rest as its body.
                None => (after, ""),
            };
            calls.push(parse_call(body.trim()));
            rest = next;
        } else if let Some(after) = block.strip_prefix("<function_calls>") {
            // The wrapper: its invokes are read one by one.
            rest = after;
        } else {
            let (body, next) = match block.find("</invoke>") {
                Some(end) => (&block[..end], &block[end + "</invoke>".len()..]),
                None => (block, ""),
            };
            calls.push(parse_invoke(body));
            rest = next.trim_start().strip_prefix("</function_calls>").unwrap_or(next);
        }
    }
    visible.push_str(rest);
    (visible.trim().to_string(), calls)
}

/// `<invoke name="tool"><parameter name="k">v</parameter>…` (the closing
/// `</invoke>` already cut off). A value that reads as JSON (an object,
/// an array, a number, true/false) keeps its type; anything else is text.
fn parse_invoke(body: &str) -> ParsedCall {
    let name_start = body.find("name=\"").ok_or("an <invoke> without a name")? + 6;
    let name_end = body[name_start..].find('"').ok_or("an <invoke> without a name")? + name_start;
    let name = body[name_start..name_end].trim().to_string();
    let mut fields = Vec::new();
    let mut rest = &body[name_end..];
    while let Some(at) = rest.find("<parameter name=\"") {
        let after = &rest[at + "<parameter name=\"".len()..];
        let key_end = after.find('"').ok_or("a <parameter> without a name")?;
        let key = after[..key_end].to_string();
        let value_start = after[key_end..].find('>').ok_or("a <parameter> without a value")? + key_end + 1;
        let value_end = after[value_start..].find("</parameter>").map(|e| e + value_start).unwrap_or(after.len());
        let raw = &after[value_start..value_end];
        let value = match json::parse(raw.trim().as_bytes()) {
            Ok(v @ (json::Value::Obj(_) | json::Value::Arr(_) | json::Value::Int(_) | json::Value::F64(_) | json::Value::Bool(_))) => v,
            _ => json::Value::Str(raw.to_string()),
        };
        fields.push((key, value));
        rest = &after[value_end.min(after.len())..];
    }
    Ok((name, json::Value::Obj(fields).to_json()))
}

fn parse_call(body: &str) -> ParsedCall {
    let body = body.trim().trim_start_matches("```json").trim_start_matches("```").trim_end_matches("```").trim();
    // Models often put raw line breaks inside JSON strings (a multi-line
    // code value): escape them and read again before refusing the block.
    let value = json::parse(body.as_bytes()).or_else(|_| json::parse(escape_raw_controls(body).as_bytes())).map_err(|e| {
        let head: String = body.chars().take(160).collect();
        format!("the block is not JSON ({e}): {head}")
    })?;
    let name = value.get("name").and_then(|v| v.as_str()).ok_or("the block has no \"name\"")?.trim().to_string();
    let args = match value.get("arguments").or_else(|| value.get("args")) {
        None | Some(json::Value::Null) => "{}".to_string(),
        Some(json::Value::Str(text)) => match json::parse(text.as_bytes()) {
            Ok(obj @ json::Value::Obj(_)) => obj.to_json(),
            _ => return Err("\"arguments\" must be a JSON object".into()),
        },
        Some(obj @ json::Value::Obj(_)) => obj.to_json(),
        Some(_) => return Err("\"arguments\" must be a JSON object".into()),
    };
    Ok((name, args))
}

/// Raw control characters inside JSON strings, escaped.
fn escape_raw_controls(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 16);
    let (mut in_string, mut escaped) = (false, false);
    for c in text.chars() {
        if in_string {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_string = false;
            } else if c == '\n' {
                out.push_str("\\n");
                continue;
            } else if c == '\r' {
                continue;
            } else if c == '\t' {
                out.push_str("\\t");
                continue;
            }
        } else if c == '"' {
            in_string = true;
        }
        out.push(c);
    }
    out
}

/// How much of `raw` (from `from`) may be shown now: everything up to a
/// tool block, holding back a tail that might still become one.
fn safe_end(raw: &str, from: usize) -> usize {
    let tail = &raw[from..];
    if let Some(at) = next_block(tail) {
        return from + at;
    }
    // Hold back a suffix that is a prefix of a marker.
    for keep in (1.."<function_calls>".len()).rev() {
        if tail.len() >= keep {
            let cut = tail.len() - keep;
            let end = &tail[cut..];
            if tail.is_char_boundary(cut) && [OPEN, "<function_calls>", "<invoke name="].iter().any(|m| m.starts_with(end)) {
                return from + cut;
            }
        }
    }
    raw.len()
}

pub struct CliModel {
    kind: CliKind,
    provider: Box<dyn ChatProvider + Send>,
    system: String,
    history: Vec<ChatMessage>,
    dynamic: String,
    in_turn: bool,
    /// This turn's reply as the CLI wrote it, reasoning removed.
    raw: String,
    /// How much of `raw` was shown as deltas.
    shown: usize,
    /// Inside a `<think>` block the CLI forwards its reasoning in.
    thinking: bool,
    /// Results the engine owes for the round: (call id, tool name).
    awaiting: Vec<(String, String)>,
    /// Results in, by call id; refusals of unparsable blocks are here too.
    results: Vec<(String, String, String)>,
    images: Vec<ModelImage>,
    next_call: u64,
    queued: Vec<ModelEvent>,
}

impl CliModel {
    pub fn new(kind: CliKind) -> CliModel {
        CliModel {
            kind,
            provider: kind.provider(),
            system: String::new(),
            history: Vec::new(),
            dynamic: String::new(),
            in_turn: false,
            raw: String::new(),
            shown: 0,
            thinking: false,
            awaiting: Vec::new(),
            results: Vec::new(),
            images: Vec::new(),
            next_call: 0,
            queued: Vec::new(),
        }
    }

    fn begin(&mut self) {
        self.raw.clear();
        self.shown = 0;
        self.thinking = false;
        let images: Vec<ToolImage> = std::mem::take(&mut self.images)
            .into_iter()
            .map(|i| ToolImage { label: i.label, png: i.png })
            .collect();
        if let Err(e) = self.provider.attach_tool_images(images) {
            self.queued.push(ModelEvent::Error(format!("{}: {e}", self.kind.label())));
            return;
        }
        let mut input = TurnInput::new(self.system.clone(), self.history.clone());
        input.dynamic_context = self.dynamic.clone();
        match self.provider.begin_turn(&input) {
            Ok(()) => self.in_turn = true,
            Err(e) => self.queued.push(ModelEvent::Error(format!("{}: {e}", self.kind.label()))),
        }
    }

    /// A delta as the CLI streams it: reasoning to `Thinking`, the reply to
    /// `Delta` up to the first tool block.
    fn take_delta(&mut self, text: &str, out: &mut Vec<ModelEvent>) {
        let mut rest = text;
        while !rest.is_empty() {
            if self.thinking {
                match rest.find("</think>") {
                    Some(at) => {
                        if at > 0 {
                            out.push(ModelEvent::Thinking(rest[..at].to_string()));
                        }
                        self.thinking = false;
                        rest = rest[at + "</think>".len()..].trim_start_matches('\n');
                    }
                    None => {
                        out.push(ModelEvent::Thinking(rest.to_string()));
                        rest = "";
                    }
                }
            } else {
                match rest.find("<think>") {
                    Some(at) => {
                        self.raw.push_str(&rest[..at]);
                        self.thinking = true;
                        rest = &rest[at + "<think>".len()..];
                    }
                    None => {
                        self.raw.push_str(rest);
                        rest = "";
                    }
                }
            }
        }
        let end = safe_end(&self.raw, self.shown);
        if end > self.shown {
            out.push(ModelEvent::Delta(self.raw[self.shown..end].to_string()));
            self.shown = end;
        }
    }

    /// The turn ended: the calls it made, or the end of the turn.
    fn finish(&mut self, text: String, out: &mut Vec<ModelEvent>) {
        self.in_turn = false;
        // Codex has no deltas: its whole reply arrives here.
        if self.raw.trim().is_empty() && !text.trim().is_empty() {
            self.take_delta(&text, out);
        }
        let mut raw = std::mem::take(&mut self.raw);
        // A model used to native tool use may go on to write the results
        // itself: nothing after an invented result is real.
        if let Some(cut) = ["<function_results>", "[tool result]", "<tool_result>"].iter().filter_map(|m| raw.find(m)).min() {
            raw.truncate(cut);
        }
        let (visible, calls) = split_reply(&raw);
        // Show whatever text came after the last shown point and is not a
        // tool block (a reply with no calls shows all of it).
        if calls.is_empty() && raw.len() > self.shown {
            out.push(ModelEvent::Delta(raw[self.shown..].to_string()));
        }
        let _ = visible;
        self.history.push(ChatMessage::new(ChatRole::Assistant, raw.trim().to_string()));
        self.awaiting.clear();
        self.results.clear();
        let mut made = 0;
        for (index, call) in calls.into_iter().enumerate() {
            self.next_call += 1;
            let call_id = format!("x{}", self.next_call);
            match call {
                Ok((name, args)) if index < MAX_CALLS_PER_REPLY => {
                    self.awaiting.push((call_id.clone(), name.clone()));
                    out.push(ModelEvent::ToolCall { call_id, name, args });
                    made += 1;
                }
                Ok((name, _)) => self.results.push((call_id, name, format!("[refused] more than {MAX_CALLS_PER_REPLY} calls in one reply"))),
                Err(e) => {
                    makepad_platform::log!("{}: a <tool_call> block could not be read: {e}", self.kind.label());
                    self.results.push((call_id, "?".into(), format!("[refused] a <tool_call> block could not be read: {e}")))
                }
            }
        }
        if made == 0 && !self.results.is_empty() {
            // Only broken blocks: tell the model and let it try again,
            // inside the same engine turn (no call is owed).
            let batch = self.render_results();
            self.history.push(ChatMessage::new(ChatRole::Tool, batch));
            self.begin();
            return;
        }
        out.push(ModelEvent::TurnDone { tool_calls: made });
    }

    fn render_results(&mut self) -> String {
        let mut text = String::new();
        for (call_id, name, body) in self.results.drain(..) {
            text.push_str(&format!("[tool result] {name} ({call_id})\n{body}\n\n"));
        }
        text
    }
}

impl Model for CliModel {
    fn label(&self) -> String {
        self.kind.label().to_string()
    }

    fn configure(&mut self, system: &str, tools: &[ToolDefinition]) -> Result<(), String> {
        // Stateless per turn: the next turn carries the new table.
        self.system = format!("{}\n{}", system.trim_end(), tool_protocol(tools));
        Ok(())
    }

    fn send_user(&mut self, text: &str, dynamic_context: &str) {
        self.dynamic = dynamic_context.to_string();
        self.awaiting.clear();
        self.results.clear();
        self.history.push(ChatMessage::new(ChatRole::User, text));
        self.begin();
    }

    fn send_tool_result(&mut self, call_id: &str, text: &str, is_error: bool) {
        let Some(at) = self.awaiting.iter().position(|(id, _)| id == call_id) else { return };
        let (_, name) = self.awaiting.remove(at);
        let body = if is_error && !text.starts_with('[') { format!("[error] {text}") } else { text.to_string() };
        self.results.push((call_id.to_string(), name, body));
        if self.awaiting.is_empty() {
            let batch = self.render_results();
            self.history.push(ChatMessage::new(ChatRole::Tool, batch));
            self.begin();
        }
    }

    fn cancel(&mut self) {
        self.provider.cancel();
        self.in_turn = false;
        self.awaiting.clear();
        self.results.clear();
    }

    fn reset(&mut self) {
        self.cancel();
        // A fresh CLI conversation: a new provider forgets the resume id.
        self.provider = self.kind.provider();
        self.history.clear();
        self.images.clear();
    }

    fn poll(&mut self) -> Vec<ModelEvent> {
        let mut out = std::mem::take(&mut self.queued);
        if !self.in_turn {
            return out;
        }
        for event in self.provider.poll() {
            match event {
                ProviderEvent::Delta(text) => self.take_delta(&text, &mut out),
                ProviderEvent::Status { .. } | ProviderEvent::Serving(_) => {}
                ProviderEvent::FunctionCall { .. } => {}
                ProviderEvent::Done { text } => self.finish(text, &mut out),
                ProviderEvent::Error(e) => {
                    self.in_turn = false;
                    out.push(ModelEvent::Error(format!("{}: {e}", self.kind.label())));
                }
            }
        }
        out.extend(std::mem::take(&mut self.queued));
        out
    }

    fn can_rebind_mid_turn(&self) -> bool {
        true
    }

    fn attach_images(&mut self, images: Vec<ModelImage>) -> Result<(), String> {
        self.images.extend(images);
        if self.images.len() > 4 {
            let cut = self.images.len() - 4;
            self.images.drain(..cut);
        }
        Ok(())
    }
}
