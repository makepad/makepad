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

use crate::engine::{Model, ModelEvent, ModelImage, PictureRoom, ToolDefinition};
use makepad_ai_hub::chat_wire::{ChatMessage, ChatRole, ProviderAvailability};
use makepad_ai_hub::providers::claude::ClaudeCodeChatProvider;
use makepad_ai_hub::providers::codex_cli::CodexCliChatProvider;
use makepad_ai_hub::providers::provider::{validate_tool_image_payloads, validate_tool_images, ChatProvider, ProviderEvent, ToolImage, TurnInput};
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
            CliKind::ClaudeCode => "Claude Code (this machine)",
            CliKind::Codex => "Codex (this machine)",
        }
    }

    pub fn from_slug(slug: &str) -> Option<CliKind> {
        match slug {
            "claude-cli" => Some(CliKind::ClaudeCode),
            "codex-cli" => Some(CliKind::Codex),
            _ => None,
        }
    }

    fn provider(self, model: Option<&str>) -> Box<dyn ChatProvider + Send> {
        match self {
            // The in-app chat's turns are tool-driven, with the person waiting.
            CliKind::ClaudeCode => Box::new(CliModel::claude_provider(model)),
            CliKind::Codex => Box::new(CodexCliChatProvider::new(None)),
        }
    }

    /// Whether the CLI is installed here: `None` when it is, else why not.
    pub fn unavailable(self) -> Option<String> {
        match self.provider(None).availability() {
            ProviderAvailability::Unavailable { .. } => Some("not installed".into()),
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
                ("parameters", t.schema_value()),
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
    // Hold back a suffix that is a prefix of a marker. A cut inside a
    // multi-byte character starts no (ASCII) marker and is skipped.
    for keep in (1.."<function_calls>".len()).rev() {
        if tail.len() >= keep {
            let cut = tail.len() - keep;
            let Some(end) = tail.get(cut..) else { continue };
            if [OPEN, "<function_calls>", "<invoke name="].iter().any(|m| m.starts_with(end)) {
                return from + cut;
            }
        }
    }
    raw.len()
}

pub struct CliModel {
    kind: CliKind,
    model: Option<String>,
    provider: Box<dyn ChatProvider + Send>,
    #[cfg(test)]
    provider_factory: Option<Box<dyn Fn() -> Box<dyn ChatProvider + Send>>>,
    system: String,
    history: Vec<ChatMessage>,
    turn_start: Option<usize>,
    history_images: Vec<(usize, ToolImage)>,
    replay_after: Option<&'static str>,
    dynamic: String,
    in_turn: bool,
    /// This turn's reply as the CLI wrote it, reasoning removed.
    raw: String,
    /// How much of `raw` was shown as deltas or handed on as calls.
    shown: usize,
    /// The model began writing results itself: the rest is ignored.
    cut: bool,
    /// Calls handed to the engine this turn.
    made: usize,
    /// The size last said of the tool block being written, in KB.
    writing_kb: Option<usize>,
    /// Inside a `<think>` block the CLI forwards its reasoning in.
    thinking: bool,
    /// Results the engine owes for the round: (call id, tool name).
    awaiting: Vec<(String, String)>,
    /// Results in, by call id; refusals of unparsable blocks are here too.
    results: Vec<(String, String, String, Vec<ModelImage>)>,
    input_images: Vec<ModelImage>,
    next_call: u64,
    queued: Vec<ModelEvent>,
}

impl CliModel {
    pub fn new(kind: CliKind) -> CliModel {
        Self::with_model(kind, None)
    }

    pub fn with_model(kind: CliKind, model: Option<&str>) -> CliModel {
        CliModel {
            kind,
            model: model.map(str::to_string),
            provider: kind.provider(model),
            #[cfg(test)]
            provider_factory: None,
            system: String::new(),
            history: Vec::new(),
            turn_start: None,
            history_images: Vec::new(),
            replay_after: None,
            dynamic: String::new(),
            in_turn: false,
            raw: String::new(),
            shown: 0,
            cut: false,
            made: 0,
            writing_kb: None,
            thinking: false,
            awaiting: Vec::new(),
            results: Vec::new(),
            input_images: Vec::new(),
            next_call: 0,
            queued: Vec::new(),
        }
    }

    pub(super) fn claude_provider(model: Option<&str>) -> ClaudeCodeChatProvider {
        ClaudeCodeChatProvider::new(model.map(str::to_string)).effort("low")
    }

    fn fresh_provider(&mut self) {
        #[cfg(test)]
        if let Some(factory) = &self.provider_factory {
            self.provider = factory();
            return;
        }
        self.provider = self.kind.provider(self.model.as_deref());
    }

    fn rollback(&mut self, reason: &'static str) {
        self.provider.cancel();
        if let Some(mark) = self.turn_start.take() {
            self.history.truncate(mark);
            self.history_images.truncate(self.history_images.partition_point(|(row, _)| *row < mark));
            self.fresh_provider();
            self.replay_after = Some(reason);
        }
        self.in_turn = false;
        self.input_images.clear();
        self.awaiting.clear();
        self.results.clear();
    }

    fn begin(&mut self, pictures: Vec<ModelImage>) {
        self.raw.clear();
        self.shown = 0;
        self.thinking = false;
        self.cut = false;
        self.made = 0;
        self.writing_kb = None;
        let mut images: Vec<_> = pictures.into_iter().map(tool_image).collect();
        let own = images.len();
        let mut omitted = 0;
        if self.replay_after.is_some() {
            for (_, image) in self.history_images.iter().rev() {
                images.push(ToolImage { label: format!("earlier: {}", image.label), png: image.png.clone() });
                if validate_tool_images(&images).is_err() {
                    images.pop();
                    omitted += 1;
                }
            }
            images[own..].reverse();
        }
        let row = self.history.len() - 1;
        let own_images = images[..own].to_vec();
        if let Err(e) = self.provider.attach_tool_images(images) {
            self.rollback("the failed turn");
            self.queued.push(ModelEvent::Error(format!("{}: {e}", self.kind.label())));
            return;
        }
        let mut input = TurnInput::new(self.system.clone(), self.history.clone());
        input.dynamic_context = self.dynamic.clone();
        match self.provider.begin_turn(&input) {
            Ok(()) => {
                self.history_images.extend(own_images.into_iter().map(|image| (row, image)));
                self.in_turn = true;
                let replay = self.replay_after.take();
                if omitted > 0 {
                    let noun = if omitted == 1 { "picture is" } else { "pictures are" };
                    self.queued.push(ModelEvent::Note(format!("After {} the model starts over from this chat: {omitted} earlier {noun} no longer in its view.", replay.unwrap())));
                }
            }
            Err(e) => {
                self.rollback("the failed turn");
                self.queued.push(ModelEvent::Error(format!("{}: {e}", self.kind.label())));
            }
        }
    }

    /// A delta as the CLI streams it: reasoning to `Thinking`, the reply
    /// to `Delta` and to tool calls as each block closes.
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
        self.scan(false, out);
    }

    /// Walk the reply from where the last scan stopped: show its text,
    /// and hand each tool block to the engine AS SOON AS IT CLOSES, so a
    /// reply of several calls changes the app while the model still
    /// writes the next one. `last`: the turn is over (an unclosed final
    /// block is read as it is).
    fn scan(&mut self, last: bool, out: &mut Vec<ModelEvent>) {
        if self.cut {
            return;
        }
        // A model used to native tool use may go on to write the results
        // itself: nothing after an invented result is real.
        let mut limit = self.raw.len();
        if let Some(at) = ["<function_results>", "[tool result]", "<tool_result>"].iter().filter_map(|m| self.raw[self.shown..].find(m)).min() {
            limit = self.shown + at;
            self.cut = true;
        }
        loop {
            let tail = &self.raw[self.shown..limit];
            let Some(at) = next_block(tail) else {
                let end = if last || self.cut { limit } else { safe_end(&self.raw[..limit], self.shown) };
                if end > self.shown {
                    out.push(ModelEvent::Delta(self.raw[self.shown..end].to_string()));
                    self.shown = end;
                }
                return;
            };
            if at > 0 {
                out.push(ModelEvent::Delta(tail[..at].to_string()));
                self.shown += at;
                continue;
            }
            // A block starts here.
            if tail.starts_with("<function_calls>") {
                self.shown += "<function_calls>".len();
                continue;
            }
            let (close, skip) = if tail.starts_with(OPEN) { (CLOSE, OPEN.len()) } else { ("</invoke>", 0) };
            let (body, used) = match tail[skip..].find(close) {
                Some(end) => (&tail[skip..skip + end], skip + end + close.len()),
                None if last || self.cut => (&tail[skip..], tail.len()),
                None => {
                    // The block is still being written, out of sight: say so.
                    let kb = (tail.len() - skip) / 1024;
                    if self.writing_kb != Some(kb) {
                        self.writing_kb = Some(kb);
                        out.push(ModelEvent::Working(if kb == 0 { "Writing a tool call".to_string() } else { format!("Writing a tool call · {kb} KB") }));
                    }
                    return;
                }
            };
            self.writing_kb = None;
            let call = if skip > 0 { parse_call(body.trim()) } else { parse_invoke(body) };
            self.shown += used;
            if let Some(after) = self.raw[self.shown..limit].trim_start().strip_prefix("</function_calls>") {
                self.shown = limit - after.len();
            }
            self.dispatch(call, out);
        }
    }

    fn dispatch(&mut self, call: ParsedCall, out: &mut Vec<ModelEvent>) {
        self.next_call += 1;
        let call_id = format!("x{}", self.next_call);
        match call {
            Ok((name, args)) if self.made < MAX_CALLS_PER_REPLY => {
                self.awaiting.push((call_id.clone(), name.clone()));
                out.push(ModelEvent::ToolCall { call_id, name, args });
                self.made += 1;
            }
            Ok((name, _)) => self.results.push((call_id, name, format!("[refused] more than {MAX_CALLS_PER_REPLY} calls in one reply"), Vec::new())),
            Err(e) => {
                makepad_platform::log!("{}: a <tool_call> block could not be read: {e}", self.kind.label());
                self.results.push((call_id, "?".into(), format!("[refused] a <tool_call> block could not be read: {e}"), Vec::new()))
            }
        }
    }

    /// The turn ended: the calls it made, or the end of the turn.
    fn finish(&mut self, text: String, out: &mut Vec<ModelEvent>) {
        self.in_turn = false;
        // Codex has no deltas: its whole reply arrives here.
        if self.raw.trim().is_empty() && !text.trim().is_empty() {
            self.take_delta(&text, out);
        }
        self.scan(true, out);
        let mut raw = std::mem::take(&mut self.raw);
        raw.truncate(self.shown.min(raw.len()));
        self.history.push(ChatMessage::new(ChatRole::Assistant, raw.trim().to_string()));
        let made = self.made;
        if made > 0 {
            out.push(ModelEvent::TurnDone { tool_calls: made });
        }
        if self.awaiting.is_empty() && !self.results.is_empty() {
            // Every call already answered (or only broken blocks): go on.
            self.begin_results();
        } else if made == 0 {
            self.turn_start = None;
            out.push(ModelEvent::TurnDone { tool_calls: 0 });
        }
    }

    fn begin_results(&mut self) {
        let mut text = String::new();
        let mut images = Vec::new();
        for (call_id, name, body, pictures) in self.results.drain(..) {
            images.extend(pictures);
            text.push_str(&format!("[tool result] {name} ({call_id})\n{body}\n\n"));
        }
        self.history.push(ChatMessage::new(ChatRole::Tool, text));
        self.begin(images);
    }
}

fn tool_image(image: ModelImage) -> ToolImage {
    ToolImage { label: image.label, png: image.png }
}

impl Model for CliModel {
    fn label(&self) -> String {
        match self.model.as_deref() {
            Some(model) => format!("{} · {}", self.kind.label(), super::models::claude_model_label(model)),
            None => self.kind.label().to_string(),
        }
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
        self.provider.cancel();
        self.turn_start = Some(self.history.len());
        self.history.push(ChatMessage::new(ChatRole::User, text));
        let images = std::mem::take(&mut self.input_images);
        self.begin(images);
    }

    fn send_tool_result(&mut self, call_id: &str, text: &str, is_error: bool) {
        let images = std::mem::take(&mut self.input_images);
        let Some(at) = self.awaiting.iter().position(|(id, _)| id == call_id) else { return };
        let (_, name) = self.awaiting.remove(at);
        let body = if is_error && !text.starts_with('[') { format!("[error] {text}") } else { text.to_string() };
        self.results.push((call_id.to_string(), name, body, images));
        // The next CLI turn starts once the reply is over and every call
        // it made has its answer.
        if self.awaiting.is_empty() && !self.in_turn {
            self.begin_results();
        }
    }

    fn cancel(&mut self) {
        self.rollback("the stop");
    }

    fn reset(&mut self) {
        self.turn_start = None;
        self.cancel();
        self.fresh_provider();
        self.history.clear();
        self.history_images.clear();
        self.replay_after = None;
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
                ProviderEvent::FunctionCall { .. } | ProviderEvent::FunctionCallPartial { .. } => {}
                ProviderEvent::Done { text } => self.finish(text, &mut out),
                ProviderEvent::Error(e) => {
                    self.rollback("the failed turn");
                    out.push(ModelEvent::Error(format!("{}: {e}", self.kind.label())));
                    break;
                }
            }
        }
        out.extend(std::mem::take(&mut self.queued));
        out
    }

    fn can_rebind_mid_turn(&self) -> bool {
        true
    }

    fn picture_room(&self) -> PictureRoom {
        PictureRoom::Unknown
    }

    fn admit_images(&self, images: &[ModelImage]) -> Result<(), String> {
        // Both CLI providers use this check. Return its refusal while the draft still owns the pictures.
        validate_tool_image_payloads(&images.iter().cloned().map(tool_image).collect::<Vec<_>>())
    }

    fn attach_images(&mut self, images: Vec<ModelImage>) -> Result<(), String> {
        self.input_images = images;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use makepad_ai_hub::{chat_wire::ProviderKind, providers::claude::render_prompt};

    #[derive(Default)]
    struct Sessions {
        prompts: Vec<Vec<String>>,
        turns: Vec<Received>,
        finish: Option<String>,
        error: Option<String>,
    }

    struct Received {
        session: usize,
        resume: Option<usize>,
        prompt: String,
        labels: Vec<String>,
        pictures: Vec<Arc<[u8]>>,
    }

    struct FakeCli {
        // Take attachments through the selected CLI's real provider. Only
        // the conversation itself runs in-process.
        provider: Box<dyn ChatProvider + Send>,
        sessions: Arc<Mutex<Sessions>>,
        resume: Option<usize>,
        active: Option<usize>,
        images: Vec<ToolImage>,
    }

    impl ChatProvider for FakeCli {
        fn kind(&self) -> ProviderKind { self.provider.kind() }
        fn availability(&mut self) -> ProviderAvailability { unreachable!() }
        fn begin_turn(&mut self, input: &TurnInput) -> Result<(), String> {
            let mut store = self.sessions.lock().unwrap();
            let session = self.resume.unwrap_or_else(|| {
                store.prompts.push(Vec::new());
                store.prompts.len() - 1
            });
            let prompt = render_prompt(input, self.resume.is_some());
            store.prompts[session].push(prompt.clone());
            store.turns.push(Received {
                session, resume: self.resume, prompt,
                labels: self.images.iter().map(|i| i.label.clone()).collect(),
                pictures: std::mem::take(&mut self.images).into_iter().map(|i| i.png).collect(),
            });
            self.active = Some(session);
            Ok(())
        }
        fn attach_tool_images(&mut self, images: Vec<ToolImage>) -> Result<(), String> {
            self.provider.attach_tool_images(images.clone())?;
            self.images = images;
            Ok(())
        }
        fn poll(&mut self) -> Vec<ProviderEvent> {
            let mut store = self.sessions.lock().unwrap();
            if let Some(error) = store.error.take() {
                return vec![ProviderEvent::Error(error)];
            }
            if let Some(text) = store.finish.take() {
                self.resume = Some(self.active.take().expect("only a running fake turn can finish"));
                vec![ProviderEvent::Done { text }]
            } else {
                Vec::new()
            }
        }
        fn cancel(&mut self) { self.active = None; }
    }

    fn fake_model() -> (CliModel, Arc<Mutex<Sessions>>) {
        fake_model_for(CliKind::ClaudeCode)
    }

    fn fake_model_for(kind: CliKind) -> (CliModel, Arc<Mutex<Sessions>>) {
        let sessions = Arc::new(Mutex::new(Sessions::default()));
        let shared = sessions.clone();
        let mut model = CliModel::new(kind);
        model.provider_factory = Some(Box::new(move || Box::new(FakeCli {
            provider: kind.provider(None), sessions: shared.clone(),
            resume: None, active: None, images: Vec::new(),
        })));
        model.fresh_provider();
        (model, sessions)
    }

    fn picture(label: &str) -> ModelImage {
        let png = makepad_ai_hub::testpattern::encode_png_rgb8(&[1, 2, 3], 1, 1).unwrap();
        ModelImage { label: label.into(), png: png.into() }
    }

    #[test]
    fn stop_restarts_the_cli_from_before_the_user_line() {
        let (mut model, sessions) = fake_model();
        let japan = "What is the capital of Japan?";
        let italy = "What is the capital of Italy?";
        model.attach_images(vec![picture("Japan")]).unwrap();
        model.send_user(japan, "");
        sessions.lock().unwrap().finish = Some("Tokyo".into());
        model.poll();
        model.send_user("Count from 1 to 300", "");
        model.cancel();
        model.send_user(italy, "");
        model.cancel();
        model.send_user(italy, "");
        let store = sessions.lock().unwrap();
        let last = store.turns.last().unwrap();
        assert_eq!(store.turns[1].resume, Some(store.turns[0].session), "the count contaminated the previously completed CLI session");
        assert_eq!(last.resume, None, "a stop must abandon the CLI resume id");
        assert!(last.prompt.contains(japan) && last.prompt.contains("Tokyo"), "both sides of the completed turn must be replayed");
        assert!(!last.prompt.contains("Count"), "the stopped line leaked into the rendered prompt");
        assert_eq!(last.prompt.matches(italy).count(), 1, "the retyped line must appear exactly once");
        assert!(store.prompts[last.session].iter().all(|p| !p.contains("Count")), "the CLI's own session must be clean");
        assert_eq!(last.labels, ["earlier: Japan"], "the completed turn's picture must be visible again");
        drop(store);

        sessions.lock().unwrap().finish = Some("Rome".into());
        model.poll();
        model.attach_images(vec![picture("failed picture")]).unwrap();
        model.send_user("refused CLI line", "");
        assert!(sessions.lock().unwrap().turns.last().unwrap().resume.is_some(),
            "the refusal must exercise a CLI session already containing committed conversation");
        sessions.lock().unwrap().error = Some("CLI disconnected".into());
        assert!(matches!(model.poll().as_slice(), [ModelEvent::Error(e)] if e.contains("CLI disconnected")),
            "the CLI failure must reach the engine");
        model.send_user("retry after failure", "");
        let store = sessions.lock().unwrap();
        let retry = store.turns.last().unwrap();
        assert_eq!(retry.resume, None, "a failed CLI session must never be resumed");
        assert!(retry.prompt.contains(japan) && retry.prompt.contains("Tokyo") && retry.prompt.contains(italy) && retry.prompt.contains("Rome") && !retry.prompt.contains("refused CLI line"),
            "the failed user line must be rolled back while the completed conversation survives");
        assert_eq!(retry.labels, ["earlier: Japan"], "the failed turn must preserve earlier pictures");
        assert!(store.prompts[retry.session].iter().all(|p| !p.contains("refused CLI line")), "the CLI's own next session must exclude the refused prompt");
    }

    #[test]
    fn stop_during_a_tool_continuation_rewinds_the_whole_cli_turn() {
        let (mut model, sessions) = fake_model();
        model.send_user("inspect the view", "");
        sessions.lock().unwrap().finish = Some(r#"<tool_call>{"name":"look","arguments":{}}</tool_call>"#.into());
        let call = model.poll().into_iter().find_map(|e| match e {
            ModelEvent::ToolCall { call_id, .. } => Some(call_id),
            _ => None,
        }).expect("the scripted reply must start a tool round");
        model.attach_images(vec![picture("view")]).unwrap();
        model.send_tool_result(&call, "view result", false);
        model.cancel();
        model.send_user("next line", "");
        let store = sessions.lock().unwrap();
        assert!(store.turns[1].prompt.contains("view result"), "the result must have started a CLI continuation before Stop");
        assert_eq!(store.turns[2].prompt, "[user]\nnext line\n", "the stopped user, tool call and result must all leave the prompt");
        assert!(store.turns[2].labels.is_empty(), "pictures from a stopped tool round must leave as well");
        assert_eq!(store.turns[2].resume, None, "the tool continuation's session must be abandoned");
    }

    #[test]
    fn picture_replay_keeps_the_newest_that_fit_and_reports_the_loss_once() {
        let (mut model, sessions) = fake_model();
        model.attach_images(["a", "b", "c", "d"].map(picture).to_vec()).unwrap();
        model.send_user("keep these", "");
        sessions.lock().unwrap().finish = Some("saved".into());
        model.poll();
        model.send_user("discard this", "");
        model.cancel();
        model.attach_images(vec![picture("new")]).unwrap();
        model.send_user("compare", "");
        assert_eq!(sessions.lock().unwrap().turns.last().unwrap().labels,
            ["new", "earlier: b", "earlier: c", "earlier: d"], "the new picture has priority, then the newest earlier pictures in chronological order");
        assert_eq!(model.poll(), [ModelEvent::Note("After the stop the model starts over from this chat: 1 earlier picture is no longer in its view.".into())], "the chat must explain the omitted picture on this line");
        assert!(model.poll().is_empty(), "the picture notice must be shown only once");
    }

    #[test]
    fn cli_offers_every_picture_to_the_provider_in_order() {
        use crate::engine::local_model::LocalModel;
        use makepad_ai_hub::providers::qwen::{FleetQwenChatProvider, HttpFleetTransport};
        use makepad_ai_hub::testpattern::encode_png_rgb8;

        let six: Vec<_> = (1..=6).map(|n| ModelImage {
            label: n.to_string(), png: encode_png_rgb8(&[n, 0, 0], 1, 1).unwrap().into(),
        }).collect();
        let expected: Vec<_> = six.iter().map(|image| image.png.clone()).collect();
        let wide = ModelImage {
            label: "wide".into(), png: encode_png_rgb8(&[0; 1025 * 3], 1025, 1).unwrap().into(),
        };
        // Four independently admissible PNGs, above the old byte limit without
        // crossing the fleet count limit. Share one encoded buffer.
        let mut noise = 1u32;
        let rgb: Vec<_> = (0..1024 * 700 * 3).map(|_| {
            noise ^= noise << 13;
            noise ^= noise >> 17;
            noise ^= noise << 5;
            noise as u8
        }).collect();
        let png: Arc<[u8]> = encode_png_rgb8(&rgb, 1024, 700).unwrap().into();
        let large_line: Vec<_> = (1..=4).map(|i| ModelImage { label: i.to_string(), png: png.clone() }).collect();
        assert!(png.len() * large_line.len() > 8 * 1024 * 1024, "the four-picture fixture must cross the old combined budget");
        for kind in [CliKind::ClaudeCode, CliKind::Codex] {
            let (mut model, sessions) = fake_model_for(kind);
            assert_eq!(model.picture_room(), PictureRoom::Unknown, "{kind:?} must report unknown picture room");
            assert_eq!(model.admit_images(&six), Ok(()), "{kind:?} composer must admit all six valid pictures");
            model.attach_images(six.clone()).unwrap();
            model.send_user("compare these six", "");
            {
                let store = sessions.lock().unwrap();
                let sent = store.turns.first().expect("the provider must start the six-picture turn");
                assert_eq!(sent.labels, ["1", "2", "3", "4", "5", "6"], "{kind:?} must receive every label in order");
                assert_eq!(sent.pictures, expected, "{kind:?} must receive the corresponding encoded pictures in order");
            }
            sessions.lock().unwrap().finish = Some("done".into());
            model.poll();
            model.send_user("discard this", "");
            model.cancel();
            model.send_user("look again", "");
            assert_eq!(sessions.lock().unwrap().turns.last().unwrap().labels,
                ["earlier: 3", "earlier: 4", "earlier: 5", "earlier: 6"],
                "{kind:?} replay must take only the four newest kept pictures, in chronological order");
            assert_eq!(model.poll(), [ModelEvent::Note("After the stop the model starts over from this chat: 2 earlier pictures are no longer in its view.".into())],
                "{kind:?} must explain both omitted pictures on the replayed turn");
            sessions.lock().unwrap().finish = Some("done".into());
            model.poll();

            assert_eq!(model.admit_images(&large_line), Ok(()), "{kind:?} must admit a valid line larger than 8 MiB");
            model.attach_images(large_line.clone()).unwrap();
            model.send_user("compare all four", "");
            assert_eq!(sessions.lock().unwrap().turns.last().unwrap().pictures, vec![png.clone(); 4],
                "{kind:?} must deliver every byte above the former limit");
            sessions.lock().unwrap().finish = Some("done".into());
            model.poll();

            assert!(model.admit_images(std::slice::from_ref(&wide)).is_err_and(|e| e.contains("1024 pixels")),
                "{kind:?} composer must explain the side limit while the draft still owns the picture");
            model.attach_images(vec![wide.clone()]).unwrap();
            model.send_user("inspect", "");
            assert!(matches!(model.poll().as_slice(), [ModelEvent::Error(e)] if e.contains("1024 pixels")),
                "{kind:?} provider's refusal at send must reach the engine with the pixel limit");
        }

        let local = LocalModel::new(None);
        let mut fleet = FleetQwenChatProvider::new(HttpFleetTransport, Vec::new());
        assert!(local.admit_images(&large_line).is_err_and(|e| e.contains("8 MiB")), "local admission must still enforce its combined byte budget");
        assert!(fleet.attach_tool_images(large_line.into_iter().map(tool_image).collect()).is_err_and(|e| e.contains("8 MiB")),
            "fleet admission must still enforce its combined byte budget");
        for n in [4, 5] {
            let pictures = &six[..n];
            assert_eq!(local.admit_images(pictures).is_ok(), n == 4, "local admission must still accept four and refuse five");
            assert_eq!(fleet.attach_tool_images(pictures.iter().cloned().map(tool_image).collect()).is_ok(), n == 4,
                "fleet admission must still accept four and refuse five");
        }
    }

    #[test]
    fn safe_end_never_cuts_inside_a_character() {
        // A streamed reply arrives in pieces that may end anywhere, and
        // the part already shown may end anywhere before that.
        for text in ["sun at 12°", "exposure −", "storm 🌩", "°", "−", "🌩", "a °<tool_c", "−<invoke name=", "🌩<function_calls"] {
            for len in (0..=text.len()).filter(|&i| text.is_char_boundary(i)) {
                let raw = &text[..len];
                for from in (0..=len).filter(|&i| raw.is_char_boundary(i)) {
                    let end = safe_end(raw, from);
                    assert!(from <= end && end <= len, "{raw:?} from {from}: {end}");
                    assert!(raw.is_char_boundary(end), "{raw:?} from {from}: {end} is inside a character");
                }
            }
        }
        // A marker's start after a multi-byte character is still held back.
        assert_eq!(safe_end("12°<tool_c", 0), "12°".len());
        assert_eq!(safe_end("−<inv", 0), "−".len());
        assert_eq!(safe_end("🌩<", 0), "🌩".len());
        assert_eq!(safe_end("12°", 0), "12°".len());
    }
}
