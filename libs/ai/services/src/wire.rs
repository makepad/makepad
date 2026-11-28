//! The wire between a host's engine and an app's service.
//!
//! Everything here is a bounded JSON object (micro_serde), the same way the
//! window manager's own protocol is. A service is described once by its
//! [`ServiceManifest`]; from then on the engine sends [`ServiceDown`] messages
//! (calls, cancels) and the service answers with [`ServiceUp`] messages
//! (results, progress, context). The transport is not this module's
//! business: in-process it is a channel, hosted by the window manager it
//! rides the studio protocol's `Custom` frames under the `"wm_ai"` envelope
//! key.
//!
//! Addressing. An app id (`route`) says WHAT a service is; an
//! [`EndpointId`] says WHICH running instance it is. The host issues the
//! endpoint when a service registers and answers with
//! [`ServiceDown::Registered`]; from then on every up-frame is stamped with
//! its sender's endpoint by the host (never trusted from the sender) and
//! every down-frame names its target, so two instances of one app, or two
//! ports in one process, never see each other's calls. Where a service
//! lives — its parent, its tile, whether it is focused — is
//! [`InstanceMeta`], kept by the host beside the manifest.
//!
//! Names. A tool is known to the model by its canonical dotted name,
//! `<service>.<tool>` (`route.plan`, `files.list_dir`); native function
//! calling APIs forbid dots, so the same tool is `<service>__<tool>` there.
//! [`canonical_name`] / [`api_name`] / [`split_name`] are the one place that
//! mapping lives.
//!
//! Risk. Every tool declares how much it can break: reading, acting on the
//! app's own state, or destroying something outside the app's undo reach.
//! The ROUTER enforces the gate (a destructive call waits for the person to
//! confirm) and keeps its own floors per service, since a declaration is
//! self-reported; the app stays its own security boundary regardless — a
//! closed match over tool names, typed arguments, path jails, bounded output.
//!
//! Bounds are enforced where a frame ARRIVES: [`HostedUp::parse`] refuses a
//! frame over [`MAX_FRAME_BYTES`] before deserializing anything, and every
//! variant is checked by [`ServiceUp::validate`] / [`ServiceDown::validate`]
//! on receipt — the sender's own care is not relied on.

use makepad_micro_serde::*;

/// Bytes a service brief may occupy in the system prompt.
pub const MAX_BRIEF_BYTES: usize = 4 * 1024;
/// Tools one service may declare. Studio's F10 service alone carries its
/// tab, canvas, code, iteration and architecture tools; 128 leaves room for
/// the architecture view's later waves without folding tools into
/// dispatchers. The bound is a count, not a prompt budget: the model-facing
/// cost is bounded by [`MAX_MANIFEST_BYTES`] below.
pub const MAX_TOOLS: usize = 128;
/// Topics one service may publish.
pub const MAX_TOPICS: usize = 16;
/// Bytes of a subscription filter JSON object.
pub const MAX_FILTER_BYTES: usize = 1024;
/// Bytes of a message's model-facing text.
pub const MAX_MESSAGE_BYTES: usize = 4 * 1024;
/// Recommended text budget per page for paginating tools. The combined
/// wire ceiling for result text and data is [`MAX_RESULT_WIRE_BYTES`].
pub const MAX_RESULT_BYTES: usize = 16 * 1024;
/// Combined base64 bytes of a result's PNG or JPEG images, separate from its text.
pub const MAX_RESULT_IMAGE_BYTES: usize = 900 * 1024;
/// Recommended structured-data budget per page for [`ToolResult::data`],
/// which shares [`MAX_RESULT_WIRE_BYTES`] with the text. Hard cap for [`Message::data`].
pub const MAX_DATA_BYTES: usize = 16 * 1024;
/// Bytes of a result's transcript note.
pub const MAX_NOTE_BYTES: usize = 256;
/// Bytes of a task card's text together ([`TaskCard::bytes`]).
pub const MAX_TASK_BYTES: usize = 4 * 1024;
/// Bytes of a service's volatile per-turn context.
pub const MAX_CONTEXT_BYTES: usize = 2 * 1024;
/// Bytes of a call's argument JSON: the existing remote OPEN payload capacity
/// (`libs/remote/src/frame.rs`: 1 MiB minus chan/kind/flags). This is a sanity
/// check, not a buffer allocation. The remote sender also checks the actual
/// OPEN payload, including its variable service/routing header, before queueing.
/// Mirrored by ai-hub's chat_wire::MAX_TOOL_JSON_BYTES (neither data-only
/// crate depends on the remote transport).
pub const MAX_ARGS_BYTES: usize = (1 << 20) - 4;
/// Real wire ceiling for a result's `text` + `data` together, matching
/// [`MAX_ARGS_BYTES`]. [`ToolResult::bound`] also checks the hosted frame size.
pub const MAX_RESULT_WIRE_BYTES: usize = MAX_ARGS_BYTES;

// Printable text and valid JSON at most double when quoted. Other text can
// contain control bytes that take six bytes each, checked by ToolResult::bound.
const _: () = assert!(
    MAX_FRAME_BYTES >= 2 * MAX_RESULT_WIRE_BYTES + MAX_RESULT_IMAGE_BYTES + 256 * 1024,
    "MAX_FRAME_BYTES must hold maximal printable text or JSON at 2x, plus base64 images and envelope"
);

pub fn arguments_too_large(bytes: usize) -> String {
    format!("the encoded call is {bytes} bytes; one call carries {MAX_ARGS_BYTES} (including routing fields). Send a smaller first part, then append the rest with the service's edit tool.")
}

pub fn result_too_large(bytes: usize) -> String {
    format!("the tool reply is {bytes} bytes; one reply carries {MAX_RESULT_WIRE_BYTES}. Ask for a smaller range or page.")
}

pub fn result_frame_too_large(bytes: usize) -> String {
    format!("the JSON-encoded tool reply is {bytes} bytes; one hosted frame carries {MAX_FRAME_BYTES}. Ask for a smaller range or page.")
}

fn looks_like_json(text: &str) -> bool {
    let trimmed = text.trim();
    match trimmed.as_bytes().first() {
        Some(b'{' | b'[') => true,
        Some(b'"') => {
            trimmed.len() >= 2
                && trimmed.ends_with('"')
                && makepad_strict_json::parse_picked(
                    trimmed.as_bytes(),
                    0,
                    true,
                    &|_| makepad_strict_json::Pick::Skip,
                ).is_ok()
        }
        Some(b'-' | b'0'..=b'9' | b't' | b'f' | b'n') => {
            makepad_strict_json::parse_picked(
                trimmed.as_bytes(),
                0,
                true,
                &|_| makepad_strict_json::Pick::Skip,
            ).is_ok()
        }
        _ => false,
    }
}

/// Serialized byte length of a JSON string body (excluding quotes) under
/// `makepad_micro_serde::SerJson for str`: `\n`/`\r`/`\t`/`\`/`"` take 2
/// bytes and all other ASCII control bytes (`< 0x20`) take 6 (`\u00XX`).
fn json_escaped_len(s: &str) -> usize {
    let mut len = s.len();
    for b in s.bytes() {
        match b {
            b'\n' | b'\r' | b'\t' | b'\\' | b'"' => len += 1,
            0x00..=0x1f => len += 5,
            _ => {}
        }
    }
    len
}

/// Longest service id.
pub const MAX_SERVICE_ID: usize = 24;
/// Longest tool short name.
pub const MAX_TOOL_NAME: usize = 32;
/// Longest call id or endpoint id.
pub const MAX_ID_BYTES: usize = 64;
/// Bytes of a whole manifest's text fields together: brief, label and every
/// tool's and topic's name, description, schema and card layout. This is a
/// separate raw-text budget in addition to the encoded registration frame cap.
pub const MAX_MANIFEST_BYTES: usize = 1792 * 1024;
/// Bytes of one hosted frame. The receiver drops larger frames before decoding.
/// Printable text and valid JSON (including tab, newline and carriage return
/// whitespace) at most double when escaped. Other control bytes below 0x20
/// take six bytes. [`ServiceManifest::validate`], [`ServiceManifest::admit`]
/// and [`ToolResult::bound`] budget for them before the host stamps the frame.
pub const MAX_FRAME_BYTES: usize = 4 * 1024 * 1024;

/// Exact JSON byte overhead of an unstamped `HostedUp { from: None, msg: ServiceUp::Result(_) }`
/// (`micro_serde` omits `Option::None` struct fields).
const HOSTED_RESULT_WRAPPER_BYTES: usize = r#"{"wm_ai":{"msg":{"Result":[]}}}"#.len();
/// Exact worst-case byte growth when the WM stamps `from: Some(EndpointId(...))`
/// onto an unstamped [`HostedUp`] (adding `"from":["..."],` — 12 bytes plus up to
/// `2 * MAX_ID_BYTES` for a printable-ASCII opaque id escaped as JSON).
const MAX_HOSTED_STAMP_BYTES: usize = r#""from":[""],"#.len() + 2 * MAX_ID_BYTES;
/// Serialized byte ceiling for an unstamped [`HostedUp`] frame so both the WM
/// and `aichat` (after the WM stamps `from`) stay within [`MAX_FRAME_BYTES`].
pub(crate) const MAX_UNSTAMPED_FRAME_BYTES: usize = MAX_FRAME_BYTES - MAX_HOSTED_STAMP_BYTES;

/// Bytes the label may occupy (checked in [`ServiceManifest::validate`]).
const MAX_LABEL_BYTES: usize = 48;
/// Bytes of one tool's card layout text.
pub const MAX_LAYOUT_BYTES: usize = 2 * 1024;
const _: () = assert!(
    MAX_FRAME_BYTES >= 2 * MAX_MANIFEST_BYTES + 64 * 1024,
    "MAX_FRAME_BYTES must hold maximal printable manifest text or JSON at 2x plus its envelope"
);
/// Bytes of an instance's display name or location line.
pub const MAX_META_BYTES: usize = 128;

/// The envelope key hosted transports use inside a studio `Custom` frame,
/// distinct from the window manager's own `"wm"` key. A protocol
/// namespace, versioned with the wire — not a product string.
pub const HOSTED_KEY: &str = "wm_ai";

/// The reason returned to a call whose hosted frame could not be decoded.
pub const HOSTED_BUILD_MISMATCH: &str = "the app and its host come from different builds, so they cannot read each other's calls: rebuild them together";

/// How much a tool can break.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, SerJson, DeJson)]
pub enum Risk {
    /// Looks at something. Runs immediately.
    Read,
    /// Changes the app's own state — a route, a level, a selection. Runs
    /// immediately; the app can undo or redo it on its own terms.
    Act,
    /// Makes something: generate, render, export, new. The router parks the
    /// call as a settings card whose arguments the person can edit; the
    /// confirmed (possibly edited) arguments run it.
    Request,
    /// Deletes, sends, spends, installs, or otherwise reaches past the
    /// app. The router parks the call until the person confirms it.
    Destructive,
}

/// One tool, as the model is told about it.
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct ToolDef {
    /// Short name, unique within the service: `[a-z0-9_]{1,32}`.
    pub name: String,
    /// One or two sentences: what it does and when to use it.
    pub description: String,
    /// A JSON-schema object for the arguments, verbatim. Must describe an
    /// object (`"type":"object"`).
    pub parameters: String,
    pub risk: Risk,
    /// A successful call wants a live preview of the app under its card.
    pub preview: bool,
    /// How the settings card of a parked call is presented; absent = the
    /// defaults (title from the name, fields in schema order).
    pub layout: Option<CardLayout>,
}

/// How a tool's settings card is presented. Everything is optional in
/// effect: an empty title or confirm label takes the default.
#[derive(Clone, Debug, PartialEq, Default, SerJson, DeJson)]
pub struct CardLayout {
    /// The card's heading.
    pub title: String,
    /// The confirm button's label (`Make it` when empty).
    pub confirm: String,
    /// Arguments shown first, in this order, with their labels.
    pub order: Vec<FieldLabel>,
    /// Arguments the card does not show (passed through when prefilled).
    pub hide: Vec<String>,
}

/// One argument's place and label on a card.
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct FieldLabel {
    pub arg: String,
    pub label: String,
}

impl CardLayout {
    pub fn new(title: impl Into<String>, confirm: impl Into<String>) -> CardLayout {
        CardLayout { title: title.into(), confirm: confirm.into(), order: Vec::new(), hide: Vec::new() }
    }

    /// Show `arg` next, with `label`.
    pub fn field(mut self, arg: impl Into<String>, label: impl Into<String>) -> CardLayout {
        self.order.push(FieldLabel { arg: arg.into(), label: label.into() });
        self
    }

    pub fn hidden(mut self, arg: impl Into<String>) -> CardLayout {
        self.hide.push(arg.into());
        self
    }

    fn bytes(&self) -> usize {
        self.title.len()
            + self.confirm.len()
            + self.order.iter().map(|f| f.arg.len() + f.label.len()).sum::<usize>()
            + self.hide.iter().map(String::len).sum::<usize>()
    }

    fn encoded_bytes(&self) -> usize {
        48 + json_escaped_len(&self.title)
            + json_escaped_len(&self.confirm)
            + self.order.iter().map(|f| 24 + json_escaped_len(&f.arg) + json_escaped_len(&f.label)).sum::<usize>()
            + self.hide.iter().map(|h| 3 + json_escaped_len(h)).sum::<usize>()
    }
}

/// One named stream a service may publish.
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct TopicDef {
    /// Short name, unique within the service: `[a-z0-9_]{1,32}`.
    pub name: String,
    /// What arrives on this stream and when to subscribe.
    pub description: String,
    /// Optional JSON schema for the message's `data` field.
    pub schema: Option<String>,
}

impl TopicDef {
    pub fn new(name: impl Into<String>, description: impl Into<String>) -> Self {
        Self { name: name.into(), description: description.into(), schema: None }
    }

    pub fn with_schema(mut self, schema: impl Into<String>) -> Self {
        self.schema = Some(schema.into());
        self
    }

    fn bytes(&self) -> usize {
        self.name.len() + self.description.len() + self.schema.as_ref().map_or(0, String::len)
    }

    fn encoded_bytes(&self) -> usize {
        44 + json_escaped_len(&self.name)
            + json_escaped_len(&self.description)
            + self.schema.as_deref().map_or(0, json_escaped_len)
    }

    fn problem(&self, service: &str, earlier: &[TopicDef]) -> Option<String> {
        if !is_ident(&self.name, MAX_TOOL_NAME) {
            return Some(format!("service '{}' topic '{}' is not [a-z0-9_]{{1,{}}}", service, self.name, MAX_TOOL_NAME));
        }
        if earlier.iter().any(|topic| topic.name == self.name) {
            return Some(format!("service '{}' declares topic '{}' twice", service, self.name));
        }
        if self.description.trim().is_empty() {
            return Some(format!("topic '{}.{}' description is empty after trimming ({} bytes)", service, self.name, self.description.len()));
        }
        if let Some(schema) = &self.schema {
            if let Some(deep) = too_deep(schema) {
                return Some(format!("topic '{}.{}' schema {deep}", service, self.name));
            }
            if !matches!(parse_tool_json(schema), Ok(makepad_strict_json::Value::Obj(_))) {
                return Some(format!("topic '{}.{}' schema is not a JSON object", service, self.name));
            }
        }
        None
    }
}

impl ToolDef {
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: impl Into<String>,
        risk: Risk,
    ) -> ToolDef {
        ToolDef {
            name: name.into(),
            description: description.into(),
            parameters: parameters.into(),
            risk,
            preview: false,
            layout: None,
        }
    }

    /// The same tool, with a presentation for its settings card.
    pub fn with_layout(mut self, layout: CardLayout) -> ToolDef {
        self.layout = Some(layout);
        self
    }

    /// The same tool, asking for a preview card when it succeeds.
    pub fn with_preview(mut self) -> ToolDef {
        self.preview = true;
        self
    }

    fn bytes(&self) -> usize {
        self.name.len() + self.description.len() + self.parameters.len()
            + self.layout.as_ref().map_or(0, CardLayout::bytes)
    }

    fn encoded_bytes(&self) -> usize {
        104 + json_escaped_len(&self.name)
            + json_escaped_len(&self.description)
            + json_escaped_len(&self.parameters)
            + self.layout.as_ref().map_or(0, CardLayout::encoded_bytes)
    }

    fn problem(&self, service: &str, earlier: &[ToolDef]) -> Option<String> {
        if !is_ident(&self.name, MAX_TOOL_NAME) {
            return Some(format!("service '{}' tool '{}' is not [a-z0-9_]{{1,{}}}", service, self.name, MAX_TOOL_NAME));
        }
        if earlier.iter().any(|tool| tool.name == self.name) {
            return Some(format!("service '{}' declares '{}' twice", service, self.name));
        }
        if self.description.trim().is_empty() {
            return Some(format!("tool '{}.{}' description is empty after trimming ({} bytes)", service, self.name, self.description.len()));
        }
        if let Some(deep) = too_deep(&self.parameters) {
            return Some(format!("tool '{}.{}' schema {deep}", service, self.name));
        }
        match parse_tool_json(&self.parameters) {
            Ok(makepad_strict_json::Value::Obj(fields)) => {
                let is_object = fields
                    .iter()
                    .any(|(k, v)| k == "type" && v.as_str() == Some("object"));
                if !is_object {
                    return Some(format!("tool '{}.{}' schema must be an argument object (\"type\":\"object\")", service, self.name));
                }
            }
            Ok(_) => return Some(format!("tool '{}.{}' schema is not a JSON object", service, self.name)),
            Err(e) => return Some(format!("tool '{}.{}' schema does not parse: {e}", service, self.name)),
        }
        if let Some(layout) = &self.layout {
            if layout.bytes() > MAX_LAYOUT_BYTES {
                return Some(format!("tool '{}.{}' card layout is {} bytes; the cap is {}", service, self.name, layout.bytes(), MAX_LAYOUT_BYTES));
            }
        }
        None
    }
}

/// A tool schema (or a call's arguments) as admission, the model adapters and
/// the providers' tool-call streams read it: `TOOL_JSON_DEPTH` deep, not the
/// strict parser's default, which real schemas outgrow.
pub fn parse_tool_json(text: &str) -> Result<makepad_strict_json::Value, &'static str> {
    makepad_strict_json::parse_depth(text.as_bytes(), makepad_strict_json::TOOL_JSON_DEPTH)
}

/// Why a schema is refused for its depth, naming the depth and the limit.
fn too_deep(schema: &str) -> Option<String> {
    let depth = makepad_strict_json::nesting_depth(schema.as_bytes());
    (depth > makepad_strict_json::TOOL_JSON_DEPTH)
        .then(|| format!("nests {depth} levels deep; the limit is {}", makepad_strict_json::TOOL_JSON_DEPTH))
}

/// Who a service is and what it will do.
#[derive(Clone, Debug, PartialEq, Default, SerJson, DeJson)]
pub struct ServiceManifest {
    /// `[a-z0-9_]{1,24}`: `route`, `files`, `game`, `os`.
    pub id: String,
    /// Shown on the chip: `Route`, `Files`.
    pub label: String,
    /// The doctrine paragraph the model reads about this app.
    pub brief: String,
    pub tools: Vec<ToolDef>,
    pub topics: Vec<TopicDef>,
}

impl ServiceManifest {
    pub fn new(id: impl Into<String>, label: impl Into<String>, brief: impl Into<String>) -> Self {
        ServiceManifest {
            id: id.into(),
            label: label.into(),
            brief: brief.into(),
            tools: Vec::new(),
            topics: Vec::new(),
        }
    }

    pub fn with_tool(mut self, tool: ToolDef) -> Self {
        self.tools.push(tool);
        self
    }

    pub fn with_topic(mut self, topic: TopicDef) -> Self {
        self.topics.push(topic);
        self
    }

    pub fn topic(&self, name: &str) -> Option<&TopicDef> {
        self.topics.iter().find(|t| t.name == name)
    }

    pub fn tool(&self, name: &str) -> Option<&ToolDef> {
        self.tools.iter().find(|t| t.name == name)
    }

    /// Refuse anything the caps do not allow, naming the first problem.
    pub fn validate(&self) -> Result<(), String> {
        if !is_ident(&self.id, MAX_SERVICE_ID) {
            return Err(format!("service id '{}' is not [a-z0-9_]{{1,{}}}", self.id, MAX_SERVICE_ID));
        }
        if self.label.trim().is_empty() || self.label.len() > MAX_LABEL_BYTES {
            return Err(format!("service '{}' label must be 1..{} bytes", self.id, MAX_LABEL_BYTES));
        }
        if self.brief.len() > MAX_BRIEF_BYTES {
            return Err(format!("service '{}' brief is {} bytes; the cap is {}", self.id, self.brief.len(), MAX_BRIEF_BYTES));
        }
        if self.tools.len() > MAX_TOOLS {
            return Err(format!("service '{}' declares {} tools; the cap is {}", self.id, self.tools.len(), MAX_TOOLS));
        }
        if self.topics.len() > MAX_TOPICS {
            return Err(format!(
                "service '{}' declares {} topics; the cap is {}",
                self.id,
                self.topics.len(),
                MAX_TOPICS
            ));
        }
        for (i, tool) in self.tools.iter().enumerate() {
            if let Some(problem) = tool.problem(&self.id, &self.tools[..i]) {
                return Err(problem);
            }
        }
        for (i, topic) in self.topics.iter().enumerate() {
            if let Some(problem) = topic.problem(&self.id, &self.topics[..i]) {
                return Err(problem);
            }
        }
        let total = self.bytes();
        if total > MAX_MANIFEST_BYTES {
            return Err(format!("service '{}' manifest is {} bytes; the cap is {}", self.id, total, MAX_MANIFEST_BYTES));
        }
        let encoded = self.encoded_frame_bytes();
        if encoded > MAX_UNSTAMPED_FRAME_BYTES {
            return Err(format!("service '{}' manifest encodes to {} bytes; the frame cap is {}", self.id, encoded, MAX_FRAME_BYTES));
        }
        Ok(())
    }

    /// Drop entries with bad names, later duplicates, empty descriptions,
    /// invalid or too-deep schemas, or oversized layouts. Then drop tools
    /// and topics past their count caps, then the largest entries until the
    /// text fits both the manifest cap and the encoded hosted-frame cap. Ties
    /// drop the later entry, with topics after tools. Remaining entries keep
    /// their original order. Each removal returns a diagnostic. Only the id,
    /// label and brief can still fail `validate()`.
    pub fn admit(&mut self) -> Vec<String> {
        let mut problems = Vec::new();
        for tool in std::mem::take(&mut self.tools) {
            if let Some(problem) = tool.problem(&self.id, &self.tools) {
                problems.push(format!("ai service '{}': tool '{}' not served: {problem} (entry is {} bytes)", self.id, tool.name, tool.bytes()));
            } else {
                self.tools.push(tool);
            }
        }
        for topic in std::mem::take(&mut self.topics) {
            if let Some(problem) = topic.problem(&self.id, &self.topics) {
                problems.push(format!("ai service '{}': topic '{}' not served: {problem} (entry is {} bytes)", self.id, topic.name, topic.bytes()));
            } else {
                self.topics.push(topic);
            }
        }
        let tool_count = self.tools.len();
        if tool_count > MAX_TOOLS {
            for tool in self.tools.drain(MAX_TOOLS..) {
                problems.push(format!("ai service '{}': tool '{}' not served: {tool_count} tools; the cap is {MAX_TOOLS} (entry is {} bytes)", self.id, tool.name, tool.bytes()));
            }
        }
        let topic_count = self.topics.len();
        if topic_count > MAX_TOPICS {
            for topic in self.topics.drain(MAX_TOPICS..) {
                problems.push(format!("ai service '{}': topic '{}' not served: {topic_count} topics; the cap is {MAX_TOPICS} (entry is {} bytes)", self.id, topic.name, topic.bytes()));
            }
        }
        let mut total = self.bytes();
        while total > MAX_MANIFEST_BYTES {
            let largest = self.tools.iter().enumerate()
                .map(|(index, tool)| (tool.bytes(), false, index))
                .chain(self.topics.iter().enumerate().map(|(index, topic)| (topic.bytes(), true, index)))
                .max_by_key(|(bytes, _, _)| *bytes);
            let Some((bytes, is_topic, index)) = largest else { break };
            let (kind, name) = if is_topic {
                ("topic", self.topics.remove(index).name)
            } else {
                ("tool", self.tools.remove(index).name)
            };
            problems.push(format!("ai service '{}': {kind} '{name}' not served: manifest is {total} bytes; the cap is {MAX_MANIFEST_BYTES} (entry is {bytes} bytes)", self.id));
            total -= bytes;
        }
        let mut encoded = self.encoded_frame_bytes();
        while encoded > MAX_UNSTAMPED_FRAME_BYTES {
            let largest = self.tools.iter().enumerate()
                .map(|(index, tool)| (tool.encoded_bytes(), false, index))
                .chain(self.topics.iter().enumerate().map(|(index, topic)| (topic.encoded_bytes(), true, index)))
                .max_by_key(|(bytes, _, _)| *bytes);
            let Some((bytes, is_topic, index)) = largest else { break };
            let (kind, name) = if is_topic {
                ("topic", self.topics.remove(index).name)
            } else {
                ("tool", self.tools.remove(index).name)
            };
            problems.push(format!("ai service '{}': {kind} '{name}' not served: encoded manifest is {encoded} bytes; the frame cap is {MAX_FRAME_BYTES} (entry encodes to {bytes} bytes)", self.id));
            encoded -= bytes;
        }
        problems
    }

    fn bytes(&self) -> usize {
        self.brief.len() + self.label.len()
            + self.tools.iter().map(ToolDef::bytes).sum::<usize>()
            + self.topics.iter().map(TopicDef::bytes).sum::<usize>()
    }

    // Includes the registration envelope and fixed JSON syntax for each entry.
    fn encoded_frame_bytes(&self) -> usize {
        160 + json_escaped_len(&self.id)
            + json_escaped_len(&self.label)
            + json_escaped_len(&self.brief)
            + self.tools.iter().map(ToolDef::encoded_bytes).sum::<usize>()
            + self.topics.iter().map(TopicDef::encoded_bytes).sum::<usize>()
    }
}

/// A subscription requested as part of a tool result.
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct SubscriptionRequest {
    pub topic: String,
    /// A JSON object the service may use to narrow the stream.
    pub filter: Option<String>,
}

impl SubscriptionRequest {
    pub fn new(topic: impl Into<String>) -> Self {
        Self { topic: topic.into(), filter: None }
    }

    pub fn with_filter(mut self, filter: impl Into<String>) -> Self {
        self.filter = Some(filter.into());
        self
    }

    fn validate(&self) -> Result<(), String> {
        if !is_ident(&self.topic, MAX_TOOL_NAME) {
            return Err("subscription: bad topic name".into());
        }
        validate_filter(self.filter.as_deref(), "subscription")
    }
}

/// One publication. The subscription id is supplied separately to
/// [`crate::port::AiServicePort::publish`] and carried beside this value on
/// the wire.
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct Message {
    pub topic: String,
    pub text: String,
    /// Optional JSON value for structured consumers.
    pub data: Option<String>,
    /// The service closes the subscription after this publication.
    #[rename(final)]
    pub final_: bool,
}

impl Message {
    pub fn new(topic: impl Into<String>, text: impl Into<String>) -> Self {
        Self { topic: topic.into(), text: text.into(), data: None, final_: false }
    }

    pub fn with_data(mut self, data: impl Into<String>) -> Self {
        self.data = Some(data.into());
        self
    }

    pub fn final_message(mut self) -> Self {
        self.final_ = true;
        self
    }

    pub fn bound(&mut self) {
        if self.text.len() > MAX_MESSAGE_BYTES {
            truncate_to_char_boundary(&mut self.text, MAX_MESSAGE_BYTES - 32);
            self.text.push_str("\n…[truncated by the router]");
        }
        if self.data.as_ref().is_some_and(|data| data.len() > MAX_DATA_BYTES) {
            self.data = None;
        }
    }
}

/// `[a-z0-9_]{1,max}`, starting with a letter.
pub fn is_ident(s: &str, max: usize) -> bool {
    !s.is_empty()
        && s.len() <= max
        && s.bytes().next().is_some_and(|b| b.is_ascii_lowercase())
        && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// An opaque id: printable ASCII, no whitespace, bounded.
pub fn is_opaque_id(s: &str) -> bool {
    !s.is_empty() && s.len() <= MAX_ID_BYTES && s.bytes().all(|b| b.is_ascii_graphic())
}

/// The name the model sees in a text tool protocol: `route.plan`.
pub fn canonical_name(service: &str, tool: &str) -> String {
    format!("{service}.{tool}")
}

/// The name a native function-calling API sees: `route__plan`.
pub fn api_name(service: &str, tool: &str) -> String {
    format!("{service}__{tool}")
}

/// `route.plan` or `route__plan` → `("route", "plan")`. A bare name has no
/// service and is returned as `("", name)` so the router can say so.
pub fn split_name(name: &str) -> (&str, &str) {
    if let Some((s, t)) = name.split_once("__") {
        return (s, t);
    }
    if let Some((s, t)) = name.split_once('.') {
        return (s, t);
    }
    ("", name)
}

/// Which running instance a service is. Issued by the host, opaque to
/// everyone else, unique for the host's lifetime (never reused).
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Default, SerJson, DeJson)]
pub struct EndpointId(pub String);

impl EndpointId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Where an instance lives, as the host knows it and the model is told.
#[derive(Clone, Debug, PartialEq, Default, SerJson, DeJson)]
pub struct InstanceMeta {
    /// The manifest's id (`route`).
    pub app_id: String,
    /// What the person calls it: `Route`, `Route (2)`, `Photos in Holiday Review`.
    pub display_name: String,
    /// The endpoint this instance is nested in, when it is an `AppView`
    /// inside a composed app.
    pub parent: Option<EndpointId>,
    /// One line: `workspace 1, left tile`, `inside Holiday Review`.
    pub location: String,
    /// Bumped by the host whenever this instance gains focus; the highest
    /// wins a tie when the model does not say which instance it means.
    pub focus_epoch: u64,
    /// The process the instance runs in, when a host serves it for another process (a window
    /// manager's hosted app): its own endpoint answers `ui` and `inspect` there. 0: this process.
    pub pid: u32,
}

/// Where a call came from: the conversation's engine (the model, or the person's console
/// line), or a request to the app from outside the conversation (`makepad-remote app <pid>
/// call`, an MCP client), with or without a `caller` token.
#[derive(Clone, Copy, Debug, PartialEq, Eq, SerJson, DeJson)]
pub enum CallOrigin {
    Conversation,
    Remote,
}

/// One call, as the service receives it.
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct ServiceCall {
    /// The engine's id for the call; the result must carry it back.
    pub call_id: String,
    /// The tool's SHORT name (`plan`, not `route.plan`).
    pub tool: String,
    /// The argument object as JSON text. The service parses it with its
    /// own typed reader and refuses what it does not expect.
    pub args: String,
    /// A capability the remote caller presented (`makepad-remote mcp --caller-env`): an app
    /// that scopes what its callers may do (Scope's per-lane token) checks it; others ignore it.
    /// `None` means no token was presented.
    pub caller: Option<String>,
    /// Where the call originated, regardless of whether a token was presented.
    pub origin: CallOrigin,
}

/// How a call ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, SerJson, DeJson)]
pub enum ToolOutcome {
    Ok,
    /// The tool ran and could not do it (no route found, file unreadable).
    Failed,
    /// The service would not do it: unknown tool, bad arguments, outside
    /// its jail.
    Refused,
    /// The person said no (a destructive call not confirmed), or the app's
    /// own policy denies AI control of this.
    Denied,
    /// Not right now: the app is busy, the model is loading, the service
    /// went away.
    Unavailable,
    /// The person or the router cancelled it before it finished.
    Cancelled,
    /// The service did not answer within the router's deadline.
    TimedOut,
}

impl ToolOutcome {
    pub fn is_ok(self) -> bool {
        matches!(self, ToolOutcome::Ok)
    }

    pub fn slug(self) -> &'static str {
        match self {
            ToolOutcome::Ok => "ok",
            ToolOutcome::Failed => "failed",
            ToolOutcome::Refused => "refused",
            ToolOutcome::Denied => "denied",
            ToolOutcome::Unavailable => "unavailable",
            ToolOutcome::Cancelled => "cancelled",
            ToolOutcome::TimedOut => "timed_out",
        }
    }
}

/// What the model's turn does after this result lands.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, SerJson, DeJson)]
pub enum Disposition {
    /// The usual: the model reads the result and goes on.
    #[default]
    Continue,
    /// The turn ends here with this result as its last word — the tool
    /// handed the person somewhere else (a new level, another app).
    EndTurn,
    /// The turn ends AND the conversation state is dropped: whatever the
    /// model believed about the world is no longer true.
    ResetConversation,
}

/// One call's answer.
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct ToolResult {
    pub call_id: String,
    pub outcome: ToolOutcome,
    /// What the model reads. Page at [`MAX_RESULT_BYTES`]. The combined wire
    /// cap for text and data is [`MAX_RESULT_WIRE_BYTES`].
    pub text: String,
    /// Structured JSON data for the model or panel, empty when absent. Page at
    /// [`MAX_DATA_BYTES`]. Shares the [`MAX_RESULT_WIRE_BYTES`] wire cap with `text`.
    pub data: String,
    /// One dim transcript line: "planned Dam → Utrecht, 41 min".
    pub note: String,
    /// PNG or JPEG images encoded as base64. The app sizes them before replying;
    /// their combined size is bounded at [`MAX_RESULT_IMAGE_BYTES`].
    pub images: Vec<String>,
    /// Show the service's live preview under this call's card.
    pub preview: bool,
    pub disposition: Disposition,
    /// Streams this successful result asks the conversation to join.
    pub subscribe: Vec<SubscriptionRequest>,
    /// The call's task card as it ended (what was made, its picture, what to do
    /// with it); `None` for a call that showed none.
    pub task: Option<TaskCard>,
}

/// A long task a tool runs (an AI hub job: a picture made into a part, a picture, a song),
/// as the chat draws it: one card per call with the input picture, a bar, the stage and
/// Stop, instead of status lines. The service sends it with its progress
/// ([`ServiceUp::Progress`]) and its result ([`ToolResult::task`]); the share and the
/// time left come from the progress itself. Bounded at [`MAX_TASK_BYTES`].
#[derive(Clone, Debug, PartialEq, Default, SerJson, DeJson)]
pub struct TaskCard {
    /// What is made, short: "Making a part".
    pub title: String,
    /// What it is made from (a file or picture name); the card shortens it, never widens.
    pub source: String,
    /// The input picture: the name of a picture sent in this chat, or a PNG file on this
    /// machine; empty for none.
    pub picture: String,
    /// The stage now, in plain words ("shape · 0/21", "meshing").
    pub stage: String,
    /// Where it runs, as a machine name ("a node"); never an address.
    pub host: String,
    /// The stages so far, oldest first, for the card's tooltip.
    pub stages: Vec<String>,
    /// Once done: one line on what came of it ("on the plate as figure").
    pub result: String,
    /// Once done: a PNG file of what was made, on this machine; empty for none.
    pub result_picture: String,
    /// Once done: what the person can do with it. A click comes back to the panel's host
    /// with the action's id.
    pub actions: Vec<TaskAction>,
}

/// A button on a finished task card.
#[derive(Clone, Debug, PartialEq, Default, SerJson, DeJson)]
pub struct TaskAction {
    /// The service's own id for it, handed back on a click.
    pub id: String,
    pub label: String,
}

impl TaskCard {
    /// Its text bytes together.
    pub fn bytes(&self) -> usize {
        [&self.title, &self.source, &self.picture, &self.stage, &self.host, &self.result, &self.result_picture].iter().map(|t| t.len()).sum::<usize>()
            + self.stages.iter().map(String::len).sum::<usize>()
            + self.actions.iter().map(|a| a.id.len() + a.label.len()).sum::<usize>()
    }

    fn escaped_bytes(&self) -> usize {
        [&self.title, &self.source, &self.picture, &self.stage, &self.host, &self.result, &self.result_picture]
            .iter().map(|t| json_escaped_len(t)).sum::<usize>()
            + self.stages.iter().map(|s| json_escaped_len(s) + 3).sum::<usize>()
            + self.actions.iter().map(|a| json_escaped_len(&a.id) + json_escaped_len(&a.label) + 24).sum::<usize>()
    }

    /// Keep it within [`MAX_TASK_BYTES`]: the oldest stages go first, then the actions.
    pub fn bound(&mut self) {
        while self.bytes() > MAX_TASK_BYTES && !self.stages.is_empty() {
            self.stages.remove(0);
        }
        if self.bytes() > MAX_TASK_BYTES {
            self.actions.clear();
        }
        for text in [&mut self.title, &mut self.source, &mut self.picture, &mut self.stage, &mut self.host, &mut self.result, &mut self.result_picture] {
            if text.len() > MAX_NOTE_BYTES * 2 {
                truncate_to_char_boundary(text, MAX_NOTE_BYTES * 2 - 3);
                text.push('…');
            }
        }
    }
}

impl ToolResult {
    fn make(call_id: impl Into<String>, outcome: ToolOutcome, text: String, note: String) -> Self {
        ToolResult {
            call_id: call_id.into(),
            outcome,
            text,
            data: String::new(),
            note,
            images: Vec::new(),
            preview: false,
            disposition: Disposition::Continue,
            subscribe: Vec::new(),
            task: None,
        }
    }

    pub fn ok(call_id: impl Into<String>, text: impl Into<String>, note: impl Into<String>) -> Self {
        Self::make(call_id, ToolOutcome::Ok, text.into(), note.into())
    }

    pub fn failed(call_id: impl Into<String>, message: impl Into<String>) -> Self {
        let message = message.into();
        Self::make(call_id, ToolOutcome::Failed, message.clone(), message)
    }

    pub fn refused(call_id: impl Into<String>, what: impl Into<String>) -> Self {
        let what = what.into();
        Self::make(call_id, ToolOutcome::Refused, what.clone(), what)
    }

    pub fn denied(call_id: impl Into<String>, what: impl Into<String>) -> Self {
        let what = what.into();
        Self::make(call_id, ToolOutcome::Denied, what.clone(), what)
    }

    pub fn unavailable(call_id: impl Into<String>, reason: impl Into<String>) -> Self {
        let reason = reason.into();
        Self::make(call_id, ToolOutcome::Unavailable, reason.clone(), reason)
    }

    pub fn cancelled(call_id: impl Into<String>) -> Self {
        Self::make(call_id, ToolOutcome::Cancelled, "cancelled".into(), "cancelled".into())
    }

    pub fn timed_out(call_id: impl Into<String>, what: impl Into<String>) -> Self {
        let what = what.into();
        Self::make(call_id, ToolOutcome::TimedOut, what.clone(), what)
    }

    pub fn with_preview(mut self) -> Self {
        self.preview = true;
        self
    }

    pub fn with_data(mut self, json: impl Into<String>) -> Self {
        self.data = json.into();
        self
    }

    pub fn with_disposition(mut self, disposition: Disposition) -> Self {
        self.disposition = disposition;
        self
    }

    pub fn with_subscription(mut self, subscription: SubscriptionRequest) -> Self {
        self.subscribe.push(subscription);
        self
    }

    fn bound_note(note: &mut String) {
        if note.len() > MAX_NOTE_BYTES {
            truncate_to_char_boundary(note, MAX_NOTE_BYTES - 3);
            note.push('…');
        }
    }

    fn append_images_truncated_note(note: &mut String) {
        const MARKER: &str = "[images truncated by the router]";
        if note.len() <= MAX_NOTE_BYTES && note.ends_with(MARKER) {
            return;
        }
        if note.is_empty() {
            note.push_str(MARKER);
            return;
        }
        if note.len().saturating_add(1 + MARKER.len()) > MAX_NOTE_BYTES {
            truncate_to_char_boundary(note, MAX_NOTE_BYTES - MARKER.len() - "… ".len());
            if !note.is_empty() {
                note.push_str("… ");
            }
        } else {
            note.push(' ');
        }
        note.push_str(MARKER);
    }

    fn may_exceed_frame_cap(&self) -> bool {
        // Lengths alone usually suffice even at six encoded bytes per input byte.
        let images_raw: usize = self.images.iter().map(String::len).sum();
        let subs_raw: usize = self.subscribe.iter()
            .map(|s| s.topic.len() + s.filter.as_ref().map_or(0, String::len))
            .sum();
        let task_raw = self.task.as_ref().map_or(0, TaskCard::bytes);
        let raw = self.call_id.len() + self.text.len() + self.data.len() + self.note.len() + images_raw + task_raw + subs_raw;
        let elems = self.images.len().saturating_mul(3)
            .saturating_add(self.subscribe.len().saturating_mul(64))
            .saturating_add(self.task.as_ref().map_or(0, |t| {
                t.stages.len().saturating_mul(3).saturating_add(t.actions.len().saturating_mul(32))
            }));
        if raw.saturating_mul(6).saturating_add(elems).saturating_add(512) <= MAX_UNSTAMPED_FRAME_BYTES {
            return false;
        }
        // Count actual escapes before allocating a serialized frame. The 512
        // bytes cover the fixed wrapper, result and task fields in both estimates.
        let escaped = json_escaped_len(&self.call_id)
            .saturating_add(json_escaped_len(&self.text))
            .saturating_add(json_escaped_len(&self.data))
            .saturating_add(json_escaped_len(&self.note))
            .saturating_add(self.images.iter().map(|i| json_escaped_len(i).saturating_add(3)).sum::<usize>())
            .saturating_add(self.task.as_ref().map_or(0, TaskCard::escaped_bytes))
            .saturating_add(self.subscribe.iter().map(|s| {
                json_escaped_len(&s.topic)
                    .saturating_add(s.filter.as_deref().map_or(0, json_escaped_len))
                    .saturating_add(64)
            }).sum::<usize>())
            .saturating_add(512);
        escaped > MAX_UNSTAMPED_FRAME_BYTES
    }

    fn hosted_frame_bytes(&self) -> usize {
        self.serialize_json().len().saturating_add(HOSTED_RESULT_WRAPPER_BYTES)
    }

    /// Enforce the wire and hosted-frame caps in place without truncating
    /// `text` mid-payload. If `text` exceeds [`MAX_RESULT_WIRE_BYTES`], the
    /// result becomes [`ToolOutcome::Failed`] carrying [`result_too_large`].
    /// Otherwise excess images are dropped (recording the marker in `note`
    /// when `text` looks like JSON or is within the marker length of the wire
    /// cap), `data` pushing `text + data` past [`MAX_RESULT_WIRE_BYTES`] is
    /// dropped whole, `note` is bounded at [`MAX_NOTE_BYTES`], and any `task`
    /// is bounded in place. Finally, if JSON escaping (up to 6x for control
    /// bytes `< 0x20`) would push the unstamped [`HostedUp`] frame past
    /// `MAX_UNSTAMPED_FRAME_BYTES`, `data` and then trailing `images` are
    /// shed; if still over, the result becomes [`ToolOutcome::Failed`]
    /// carrying [`result_frame_too_large`].
    pub fn bound(&mut self) {
        if self.text.len() > MAX_RESULT_WIRE_BYTES {
            let bytes = self.text.len();
            let call_id = std::mem::take(&mut self.call_id);
            *self = ToolResult::failed(call_id, result_too_large(bytes));
            return;
        }
        let count = self.images.len();
        let mut remaining = MAX_RESULT_IMAGE_BYTES;
        self.images.retain(|image| {
            if image.len() > remaining {
                return false;
            }
            remaining -= image.len();
            true
        });
        let images_dropped = self.images.len() != count;
        if images_dropped {
            const MARKER: &str = "\n…[images truncated by the router]";
            if looks_like_json(&self.text)
                || self.text.len().saturating_add(MARKER.len()) > MAX_RESULT_WIRE_BYTES
            {
                Self::append_images_truncated_note(&mut self.note);
            } else {
                self.text.push_str(MARKER);
            }
        }
        if self.text.len().saturating_add(self.data.len()) > MAX_RESULT_WIRE_BYTES {
            self.data.clear();
        }
        Self::bound_note(&mut self.note);
        if let Some(task) = &mut self.task {
            task.bound();
        }
        if self.may_exceed_frame_cap() {
            let mut encoded = self.hosted_frame_bytes();
            if encoded > MAX_UNSTAMPED_FRAME_BYTES && !self.data.is_empty() {
                self.data.clear();
                encoded = self.hosted_frame_bytes();
            }
            if encoded > MAX_UNSTAMPED_FRAME_BYTES && !self.images.is_empty() {
                let mut marked = images_dropped;
                while encoded > MAX_UNSTAMPED_FRAME_BYTES {
                    let Some(img) = self.images.last() else { break };
                    let saved = json_escaped_len(img) + 2 + usize::from(self.images.len() > 1);
                    self.images.pop();
                    encoded -= saved;
                    if !marked {
                        let before_note = json_escaped_len(&self.note);
                        Self::append_images_truncated_note(&mut self.note);
                        encoded = encoded - before_note + json_escaped_len(&self.note);
                        marked = true;
                    }
                }
                encoded = self.hosted_frame_bytes();
            }
            if encoded > MAX_UNSTAMPED_FRAME_BYTES {
                let call_id = std::mem::take(&mut self.call_id);
                *self = ToolResult::failed(call_id, result_frame_too_large(encoded));
            }
        }
    }
}

/// A service's volatile state for the next turn: "[route] gps=52.37,4.90
/// map=Utrecht z13 trip=Dam→Utrecht 41min".
#[derive(Clone, Debug, PartialEq, Default, SerJson, DeJson)]
pub struct ServiceContext {
    pub text: String,
}

/// Service → engine.
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub enum ServiceUp {
    /// Here I am. `port_tag` is the port's own nonce, so the host's
    /// [`ServiceDown::Registered`] reaches the right port when a process
    /// has several. Sending it again replaces the manifest (a re-register
    /// after a reload is fine).
    Register { manifest: ServiceManifest, port_tag: u32 },
    Result(ToolResult),
    /// A long call is still alive. Resets the router's deadline. `task`: the
    /// call's task card now (the chat draws it in place of the note). It is not
    /// the variant's last field: the JSON writer leaves a comma after a last
    /// absent Option of an enum variant.
    Progress { call_id: String, task: Option<TaskCard>, note: String, permille: u16 },
    Context(ServiceContext),
    /// A publication for a subscription the engine created.
    Message {
        sub_id: String,
        topic: String,
        text: String,
        data: Option<String>,
        #[rename(final)]
        final_: bool,
    },
    /// Going away on purpose (the host also forgets a service whose
    /// transport dies).
    Unregister,
    ConfirmAnswer { call_id: String, allow: bool },
}

/// Engine → service.
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub enum ServiceDown {
    /// The host's answer to `Register`: this is who you are now. Every
    /// later frame is addressed by this endpoint.
    Registered { port_tag: u32, endpoint: EndpointId },
    Call(ServiceCall),
    /// The person or the router gave up on this call; the service should
    /// stop if it can and need not reply.
    Cancel { call_id: String },
    Subscribe { sub_id: String, filter: Option<String>, topic: String },
    Unsubscribe { sub_id: String },
    /// The host's chat pane is showing (or not). Informational: an
    /// embedded panel may hide itself while the desktop one is up.
    ChatOpen { open: bool },
    Confirm { call_id: String, tool: String, args: String },
}

impl ServiceUp {
    /// Receiver-side check of one frame, whatever transport it came by.
    /// The manifest is validated again here even though the sending app
    /// validated it: the host does not rely on the app.
    pub fn validate(&self) -> Result<(), String> {
        match self {
            ServiceUp::Register { manifest, .. } => manifest.validate(),
            ServiceUp::Result(r) => {
                if !is_opaque_id(&r.call_id) {
                    return Err("result: bad call id".into());
                }
                if r.text.len().saturating_add(r.data.len()) > MAX_RESULT_WIRE_BYTES {
                    return Err("result: payload over wire cap".into());
                }
                if r.images.iter().fold(0usize, |bytes, image| bytes.saturating_add(image.len())) > MAX_RESULT_IMAGE_BYTES {
                    return Err("result: images over cap".into());
                }
                if r.note.len() > MAX_NOTE_BYTES + 4 {
                    return Err("result: note over cap".into());
                }
                if r.task.as_ref().is_some_and(|t| t.bytes() > MAX_TASK_BYTES + 64) {
                    return Err("result: task over cap".into());
                }
                if r.subscribe.len() > MAX_TOPICS {
                    return Err("result: subscriptions over cap".into());
                }
                for subscription in &r.subscribe {
                    subscription.validate()?;
                }
                Ok(())
            }
            ServiceUp::Progress { call_id, note, permille, task } => {
                if !is_opaque_id(call_id) {
                    return Err("progress: bad call id".into());
                }
                if note.len() > MAX_NOTE_BYTES + 4 || *permille > 1000 || task.as_ref().is_some_and(|t| t.bytes() > MAX_TASK_BYTES + 64) {
                    return Err("progress: over cap".into());
                }
                Ok(())
            }
            ServiceUp::Context(c) => {
                if c.text.len() > MAX_CONTEXT_BYTES + 4 {
                    return Err("context: over cap".into());
                }
                Ok(())
            }
            ServiceUp::Message { sub_id, topic, text, data, .. } => {
                if !is_opaque_id(sub_id) {
                    return Err("message: bad subscription id".into());
                }
                if !is_ident(topic, MAX_TOOL_NAME) {
                    return Err("message: bad topic name".into());
                }
                if text.len() > MAX_MESSAGE_BYTES {
                    return Err("message: text over cap".into());
                }
                if let Some(data) = data {
                    if data.len() > MAX_DATA_BYTES {
                        return Err("message: data over cap".into());
                    }
                    if makepad_strict_json::parse(data.as_bytes()).is_err() {
                        return Err("message: data is not JSON".into());
                    }
                }
                Ok(())
            }
            ServiceUp::ConfirmAnswer { call_id, .. } if !is_opaque_id(call_id) => Err("confirm answer: bad call id".into()),
            ServiceUp::ConfirmAnswer { .. } | ServiceUp::Unregister => Ok(()),
        }
    }
}

impl ServiceDown {
    /// Receiver-side check of one frame.
    pub fn validate(&self) -> Result<(), String> {
        match self {
            ServiceDown::Registered { endpoint, .. } => {
                if !is_opaque_id(endpoint.as_str()) {
                    return Err("registered: bad endpoint".into());
                }
                Ok(())
            }
            ServiceDown::Call(ServiceCall { call_id, tool, args, .. }) | ServiceDown::Confirm { call_id, tool, args } => {
                if !is_opaque_id(call_id) {
                    return Err("call: bad call id".into());
                }
                if !is_ident(tool, MAX_TOOL_NAME) {
                    return Err("call: bad tool name".into());
                }
                if args.len() > MAX_ARGS_BYTES {
                    return Err(arguments_too_large(args.len()));
                }
                Ok(())
            }
            ServiceDown::Cancel { call_id } => {
                if !is_opaque_id(call_id) {
                    return Err("cancel: bad call id".into());
                }
                Ok(())
            }
            ServiceDown::Subscribe { sub_id, topic, filter } => {
                if !is_opaque_id(sub_id) {
                    return Err("subscribe: bad subscription id".into());
                }
                if !is_ident(topic, MAX_TOOL_NAME) {
                    return Err("subscribe: bad topic name".into());
                }
                validate_filter(filter.as_deref(), "subscribe")
            }
            ServiceDown::Unsubscribe { sub_id } => {
                if !is_opaque_id(sub_id) {
                    return Err("unsubscribe: bad subscription id".into());
                }
                Ok(())
            }
            ServiceDown::ChatOpen { .. } => Ok(()),
        }
    }
}

fn validate_filter(filter: Option<&str>, frame: &str) -> Result<(), String> {
    let Some(filter) = filter else { return Ok(()) };
    if filter.len() > MAX_FILTER_BYTES {
        return Err(format!("{frame}: filter over cap"));
    }
    if !matches!(makepad_strict_json::parse(filter.as_bytes()), Ok(makepad_strict_json::Value::Obj(_))) {
        return Err(format!("{frame}: filter is not a JSON object"));
    }
    Ok(())
}

/// A hosted up-frame: the message and, once the host has stamped it, the
/// sender. An app leaves `from` empty; the host overwrites it with the
/// endpoint it issued to that client — a sender's own claim is never used.
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct HostedUp {
    pub from: Option<EndpointId>,
    pub msg: ServiceUp,
}

/// A hosted down-frame: the message and the endpoint it is for. Only
/// `Registered` may travel without a target (the port has no endpoint yet
/// and matches on its `port_tag` instead).
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub struct HostedDown {
    pub to: Option<EndpointId>,
    pub msg: ServiceDown,
}

#[derive(SerJson, DeJson)]
struct UpEnvelope {
    wm_ai: HostedUp,
}

#[derive(SerJson, DeJson)]
struct DownEnvelope {
    wm_ai: HostedDown,
}

/// Which frame shape a [`RejectedFrame`] carried, read only to decide whether
/// a caller is waiting for an answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RejectedKind {
    Call,
    Result,
    Progress,
    Other,
}

/// A refused hosted frame. Only a typed decode failure is a wire mismatch.
#[derive(Debug, PartialEq, Eq)]
pub struct RejectedFrame {
    pub error: String,
    pub mismatch: bool,
    pub kind: RejectedKind,
    pub endpoint: Option<String>,
    pub call_id: Option<String>,
}

enum Direction {
    Up,
    Down,
}

fn parse_hosted<T>(
    json: &str,
    direction: Direction,
    decode: impl FnOnce() -> Result<T, DeJsonErr>,
    validate: impl FnOnce(&T) -> Result<(), String>,
) -> Option<Result<T, RejectedFrame>> {
    if !json.contains("\"wm_ai\"") {
        return None;
    }
    if json.len() > MAX_FRAME_BYTES {
        return Some(Err(RejectedFrame {
            error: format!("hosted frame is {} bytes; the cap is {MAX_FRAME_BYTES}", json.len()),
            mismatch: false,
            kind: RejectedKind::Other,
            endpoint: None,
            call_id: None,
        }));
    }
    let (error, mismatch) = match decode() {
        Ok(frame) => match validate(&frame) {
            Ok(()) => return Some(Ok(frame)),
            Err(error) => (error, false),
        },
        Err(error) => (error.to_string(), true),
    };

    // Only the address, frame kind and call id can be used from a rejected frame.
    let root = makepad_strict_json::parse(json.as_bytes()).ok();
    let envelope = root.as_ref().and_then(|root| root.get(HOSTED_KEY));
    let msg = envelope.and_then(|frame| frame.get("msg"));
    let tuple_arg = |variant: &str| msg?.get(variant)?.as_arr()?.first();
    let (address, kind, payload) = match direction {
        Direction::Down => match tuple_arg("Call") {
            Some(payload) => ("to", RejectedKind::Call, Some(payload)),
            None => ("to", RejectedKind::Other, None),
        },
        Direction::Up => match tuple_arg("Result") {
            Some(payload) => ("from", RejectedKind::Result, Some(payload)),
            None => match msg.and_then(|msg| msg.get("Progress")) {
                Some(payload) => ("from", RejectedKind::Progress, Some(payload)),
                None => ("from", RejectedKind::Other, None),
            },
        },
    };
    let endpoint = envelope.and_then(|frame| frame.get(address)?.as_arr()?.first()?.as_str());
    let call_id = payload.and_then(|payload| payload.get("call_id")?.as_str());
    Some(Err(RejectedFrame {
        error,
        mismatch,
        kind,
        endpoint: endpoint.map(str::to_owned),
        call_id: call_id.map(str::to_owned),
    }))
}

impl HostedUp {
    /// The hosted frame: `{"wm_ai": {"from": …, "msg": …}}`.
    pub fn to_json(&self) -> String {
        UpEnvelope { wm_ai: self.clone() }.serialize_json()
    }

    /// `None` for frames that are not ours (the window manager's own
    /// `"wm"` envelope, anything else), over the size cap, or invalid.
    pub fn parse(json: &str) -> Option<HostedUp> {
        Self::parse_checked(json)?.ok()
    }

    /// `None` for a foreign frame, otherwise the accepted frame or its refusal.
    pub fn parse_checked(json: &str) -> Option<Result<HostedUp, RejectedFrame>> {
        parse_hosted(
            json,
            Direction::Up,
            || UpEnvelope::deserialize_json(json).map(|envelope| envelope.wm_ai),
            |up| up.msg.validate(),
        )
    }
}

impl HostedDown {
    pub fn to_json(&self) -> String {
        DownEnvelope { wm_ai: self.clone() }.serialize_json()
    }

    pub fn parse(json: &str) -> Option<HostedDown> {
        Self::parse_checked(json)?.ok()
    }

    /// `None` for a foreign frame, otherwise the accepted frame or its refusal.
    pub fn parse_checked(json: &str) -> Option<Result<HostedDown, RejectedFrame>> {
        parse_hosted(
            json,
            Direction::Down,
            || DownEnvelope::deserialize_json(json).map(|envelope| envelope.wm_ai),
            |down| down.msg.validate(),
        )
    }
}

/// One answer line of the process's `app` service (makepad-remote `app/1`, served by
/// `app_service`), as JSON. A channel is one request: its OPEN arguments are a first line
/// `list`, `context` or `call <service>.<tool> [<instance>]`, then for a call the argument object
/// as JSON. `list` answers an `Instance` line per running instance and a `Launchable` line per
/// app the host can start; `context` one `Context` line; a call `Progress` lines and one
/// `Result`. Closing the channel cancels a call.
#[derive(Clone, Debug, PartialEq, SerJson, DeJson)]
pub enum AppLine {
    Instance { endpoint: EndpointId, meta: InstanceMeta, manifest: ServiceManifest },
    Launchable { id: String, label: String },
    Context { text: String },
    Progress { note: String, permille: u16 },
    Result(ToolResult),
}

impl AppLine {
    pub fn to_json(&self) -> String {
        self.serialize_json()
    }

    pub fn parse(json: &str) -> Option<AppLine> {
        AppLine::deserialize_json(json).ok()
    }
}

/// A parsed `app` request (the OPEN arguments).
#[derive(Clone, Debug, PartialEq)]
pub enum AppRequest {
    List,
    Context,
    Call { service: String, tool: String, instance: Option<String>, args: String, caller: Option<String> },
}

impl AppRequest {
    pub fn parse(text: &str) -> Result<AppRequest, String> {
        let (head, body) = text.split_once('\n').unwrap_or((text, ""));
        let mut words = head.split_whitespace();
        match words.next() {
            Some("list") => Ok(AppRequest::List),
            Some("context") => Ok(AppRequest::Context),
            Some("call") => {
                let name = words.next().ok_or("call: which tool (`service.tool`)?")?;
                let (service, tool) = name.split_once('.').ok_or("call: the tool is `service.tool`")?;
                let mut caller = None;
                let instance = words
                    .filter(|w| match w.strip_prefix("caller=") {
                        Some(c) => {
                            caller = Some(c.to_string());
                            false
                        }
                        None => true,
                    })
                    .collect::<Vec<_>>()
                    .join(" ");
                let args = if body.trim().is_empty() { "{}".to_string() } else { body.trim().to_string() };
                if args.len() > MAX_ARGS_BYTES {
                    return Err(arguments_too_large(args.len()));
                }
                Ok(AppRequest::Call {
                    service: service.to_string(),
                    tool: tool.to_string(),
                    instance: if instance.is_empty() { None } else { Some(instance) },
                    args,
                    caller,
                })
            }
            Some(other) => Err(format!("unknown app request `{other}` (list, context, call)")),
            None => Err("empty app request (list, context, call)".into()),
        }
    }

    pub fn to_args(&self) -> String {
        match self {
            AppRequest::List => "list".into(),
            AppRequest::Context => "context".into(),
            AppRequest::Call { service, tool, instance, args, caller } => {
                let mut head = format!("call {service}.{tool}");
                if let Some(i) = instance {
                    head.push_str(&format!(" {i}"));
                }
                if let Some(c) = caller {
                    head.push_str(&format!(" caller={c}"));
                }
                format!("{head}\n{args}")
            }
        }
    }
}

/// Cut `s` to at most `max` bytes without splitting a character.
pub fn truncate_to_char_boundary(s: &mut String, max: usize) {
    if s.len() <= max {
        return;
    }
    let mut cut = max;
    while cut > 0 && !s.is_char_boundary(cut) {
        cut -= 1;
    }
    s.truncate(cut);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route() -> ServiceManifest {
        ServiceManifest::new("route", "Route", "The trip planner.")
            .with_tool(
                ToolDef::new(
                    "plan",
                    "Plan a trip.",
                    r#"{"type":"object","properties":{"to":{"type":"string"}},"required":["to"]}"#,
                    Risk::Act,
                )
                .with_preview(),
            )
            .with_tool(ToolDef::new("status", "The trip so far.", r#"{"type":"object","properties":{}}"#, Risk::Read))
            .with_topic(
                TopicDef::new("trip", "Trip progress and arrival.")
                    .with_schema(r#"{"type":"object","properties":{"minutes":{"type":"number"}}}"#),
            )
    }

    fn ep(s: &str) -> EndpointId {
        EndpointId(s.to_string())
    }

    #[test]
    fn a_registration_round_trips_and_the_host_stamps_the_sender() {
        let up = HostedUp { from: None, msg: ServiceUp::Register { manifest: route(), port_tag: 7 } };
        let json = up.to_json();
        assert!(json.contains("\"wm_ai\""));
        let parsed = HostedUp::parse(&json).unwrap();
        assert_eq!(parsed, up);
        let stamped = HostedUp { from: Some(ep("e1")), ..parsed };
        assert_eq!(HostedUp::parse(&stamped.to_json()), Some(stamped));
    }

    #[test]
    fn every_up_and_down_variant_round_trips() {
        let ups = vec![
            ServiceUp::Register { manifest: route(), port_tag: 1 },
            ServiceUp::Result(
                ToolResult::ok("c1", "41 min", "planned")
                    .with_preview()
                    .with_data(r#"{"minutes":41}"#)
                    .with_subscription(SubscriptionRequest::new("trip").with_filter(r#"{"trip":"t1"}"#)),
            ),
            ServiceUp::Result(ToolResult::failed("c2", "no route")),
            ServiceUp::Result(ToolResult::refused("c3", "not a tool")),
            ServiceUp::Result(ToolResult::denied("c4", "no")),
            ServiceUp::Result(ToolResult::unavailable("c5", "loading")),
            ServiceUp::Result(ToolResult::cancelled("c6")),
            ServiceUp::Result(ToolResult::timed_out("c7", "60 s").with_disposition(Disposition::EndTurn)),
            ServiceUp::Progress { call_id: "c1".into(), note: "routing".into(), permille: 500, task: None },
            ServiceUp::Context(ServiceContext { text: "[route] gps=…".into() }),
            ServiceUp::Message {
                sub_id: "s1".into(),
                topic: "trip".into(),
                text: "arrived".into(),
                data: Some(r#"{"minutes":41}"#.into()),
                final_: true,
            },
            ServiceUp::Message {
                sub_id: "s2".into(),
                topic: "trip".into(),
                text: "departed".into(),
                data: None,
                final_: false,
            },
            ServiceUp::Unregister,
        ];
        for msg in ups {
            let f = HostedUp { from: Some(ep("e9")), msg };
            let json = f.to_json();
            if matches!(&f.msg, ServiceUp::Message { final_: true, .. }) {
                assert!(json.contains(r#""final":true"#) && !json.contains("final_"), "{json}");
            }
            makepad_strict_json::parse(json.as_bytes()).unwrap_or_else(|error| panic!("invalid JSON: {error}: {json}"));
            assert_eq!(HostedUp::parse(&json).as_ref(), Some(&f), "{json}");
        }
        let downs = vec![
            ServiceDown::Registered { port_tag: 1, endpoint: ep("e9") },
            ServiceDown::Call(ServiceCall { call_id: "c1".into(), tool: "plan".into(), args: r#"{"to":"Utrecht"}"#.into(), caller: None, origin: CallOrigin::Conversation }),
            ServiceDown::Cancel { call_id: "c1".into() },
            ServiceDown::Subscribe {
                sub_id: "s1".into(),
                topic: "trip".into(),
                filter: Some(r#"{"trip":"t1"}"#.into()),
            },
            ServiceDown::Subscribe { sub_id: "s2".into(), topic: "trip".into(), filter: None },
            ServiceDown::Unsubscribe { sub_id: "s1".into() },
            ServiceDown::ChatOpen { open: true },
        ];
        for msg in downs {
            let f = HostedDown { to: Some(ep("e9")), msg };
            let json = f.to_json();
            makepad_strict_json::parse(json.as_bytes()).unwrap_or_else(|error| panic!("invalid JSON: {error}: {json}"));
            assert_eq!(HostedDown::parse(&json).as_ref(), Some(&f), "{json}");
        }
    }

    #[test]
    fn foreign_oversized_and_invalid_frames_are_refused_before_use() {
        assert_eq!(HostedUp::parse(r#"{"wm":{"Close":{}}}"#), None);
        assert_eq!(HostedDown::parse(r#"{"wm":{"Adopted":{}}}"#), None);
        assert!(HostedUp::parse_checked(r#"{"wm":{"Close":{}}}"#).is_none());
        assert!(HostedDown::parse_checked(r#"{"wm":{"Adopted":{}}}"#).is_none());
        assert_eq!(HostedUp::parse("not json"), None);
        let huge = format!(
            "{{\"wm_ai\":{{\"from\":null,\"msg\":{{\"Unregister\":{{}}}}}},\"pad\":\"{}\"}}",
            "x".repeat(MAX_FRAME_BYTES)
        );
        assert_eq!(HostedUp::parse(&huge), None);
        assert!(!HostedUp::parse_checked(&huge).unwrap().unwrap_err().mismatch);
        let call = ServiceCall {
            call_id: "c1".into(), tool: "plan".into(), args: "{}".into(),
            caller: None, origin: CallOrigin::Conversation,
        };
        let down = |call| HostedDown { to: Some(ep("e1")), msg: ServiceDown::Call(call) };
        let missing_origin = down(call).to_json().replace(r#","origin":{"Conversation":[]}"#, "");
        let rejected = HostedDown::parse_checked(&missing_origin).unwrap().unwrap_err();
        assert!(rejected.mismatch && rejected.error.contains("origin"), "{rejected:?}");
        assert_eq!(rejected.endpoint.as_deref(), Some("e1"));
        assert_eq!(rejected.call_id.as_deref(), Some("c1"));
        let bad = HostedDown { to: None, msg: ServiceDown::Call(ServiceCall { call_id: "c 1".into(), tool: "Plan".into(), args: "{}".into(), caller: None, origin: CallOrigin::Conversation }) };
        assert_eq!(HostedDown::parse(&bad.to_json()), None);
    }

    #[test]
    fn validate_accepts_results_up_to_wire_cap_and_classifies_rejected_kinds() {
        let json_string = |bytes: usize| format!(r#""{}""#, "x".repeat(bytes - 2));
        let up = |msg| HostedUp { from: Some(ep("e1")), msg };
        let result = ToolResult::ok("c1", json_string(64 * 1024), "").with_data(json_string(32 * 1024));
        let frame = up(ServiceUp::Result(result));
        assert_eq!(HostedUp::parse(&frame.to_json()), Some(frame), "large JSON text and data round-trip intact");

        let result = ToolResult::ok("c1", "x".repeat(MAX_RESULT_WIRE_BYTES - 2), "").with_data("{}");
        let mut frame = up(ServiceUp::Result(result));
        assert_eq!(frame.msg.validate(), Ok(()), "the combined payload can fill the wire cap exactly");
        let ServiceUp::Result(result) = &mut frame.msg else { unreachable!() };
        result.text.push('x');
        let rejected = HostedUp::parse_checked(&frame.to_json()).unwrap().unwrap_err();
        assert!(!rejected.mismatch, "oversized results are validation failures");
        assert_eq!(rejected.kind, RejectedKind::Result, "oversized results remain identifiable");
        assert_eq!(rejected.call_id.as_deref(), Some("c1"), "oversized results retain the call id");
        assert_eq!(rejected.error, "result: payload over wire cap", "the refusal names the combined cap");

        let progress = up(ServiceUp::Progress { call_id: "c1".into(), note: String::new(), permille: 1001, task: None });
        let rejected = HostedUp::parse_checked(&progress.to_json()).unwrap().unwrap_err();
        assert!(!rejected.mismatch, "invalid progress is a validation failure");
        assert_eq!(rejected.kind, RejectedKind::Progress, "invalid progress remains non-terminal");
        assert_eq!(rejected.call_id.as_deref(), Some("c1"), "invalid progress retains the call id");

        let call = ServiceCall {
            call_id: "c1".into(), tool: "plan".into(), args: "x".repeat(MAX_ARGS_BYTES + 1),
            caller: None, origin: CallOrigin::Conversation,
        };
        let oversized = HostedDown { to: Some(ep("e1")), msg: ServiceDown::Call(call) }.to_json();
        assert_eq!(HostedDown::parse(&oversized), None, "oversized arguments are refused in full");
        let rejected = HostedDown::parse_checked(&oversized).unwrap().unwrap_err();
        assert!(!rejected.mismatch, "oversized calls are validation failures");
        assert_eq!(rejected.kind, RejectedKind::Call, "oversized calls remain identifiable");
        assert_eq!(rejected.call_id.as_deref(), Some("c1"), "oversized calls retain the call id");
        assert_eq!(rejected.error, arguments_too_large(MAX_ARGS_BYTES + 1), "the refusal gives the encoded argument size");
    }

    #[test]
    fn names_map_both_ways() {
        assert_eq!(canonical_name("route", "plan"), "route.plan");
        assert_eq!(api_name("route", "plan"), "route__plan");
        assert_eq!(split_name("route.plan"), ("route", "plan"));
        assert_eq!(split_name("route__plan"), ("route", "plan"));
        assert_eq!(split_name("plan"), ("", "plan"));
    }

    #[test]
    fn validation_names_the_first_problem() {
        assert!(route().validate().is_ok());
        let mut bad = route();
        bad.id = "Route".into();
        assert!(bad.validate().unwrap_err().contains("service id"));
        let mut dup = route();
        dup.tools.push(dup.tools[0].clone());
        assert!(dup.validate().unwrap_err().contains("twice"));
        assert_eq!(dup.admit().len(), 1);
        assert_eq!(dup, route(), "the first tool survives a duplicate");
        dup.topics.push(dup.topics[0].clone());
        assert_eq!(dup.admit().len(), 1);
        assert_eq!(dup, route(), "the first topic survives a duplicate");
        let mut schema = route();
        schema.tools[0].parameters = "[]".into();
        assert!(schema.validate().unwrap_err().contains("not a JSON object"));
        let mut not_object = route();
        not_object.tools[0].parameters = r#"{"type":"string"}"#.into();
        assert!(not_object.validate().unwrap_err().contains("argument object"));
        // Schemas nest past the strict parser's default: a dozen levels are served as declared;
        // past the tool depth the tool is refused, saying how deep it went and the limit.
        let nest = |levels: usize| format!(r#"{{"type":"object","properties":{}{{"type":"string"}}{}}}"#,
            r#"{"a":{"type":"object","properties":"#.repeat(levels), "}}".repeat(levels));
        let mut deep = route();
        deep.tools[0].parameters = nest(6);
        assert_eq!(makepad_strict_json::nesting_depth(deep.tools[0].parameters.as_bytes()), 14);
        deep.validate().expect("a 14-level schema is served");
        assert!(deep.clone().admit().is_empty());
        let mut too_deep = route();
        too_deep.tools[0].parameters = nest(35);
        let depth = makepad_strict_json::nesting_depth(too_deep.tools[0].parameters.as_bytes());
        let why = format!("schema nests {depth} levels deep; the limit is {}", makepad_strict_json::TOOL_JSON_DEPTH);
        assert!(too_deep.validate().unwrap_err().contains(&why), "{why}");
        let problems = too_deep.admit();
        assert!(problems.len() == 1 && problems[0].contains(&why) && problems[0].contains("not served"), "{problems:?}");
        let mut brief = route();
        brief.brief = "x".repeat(MAX_BRIEF_BYTES + 1);
        assert!(brief.validate().unwrap_err().contains("brief"));
        let mut topics = route();
        topics.topics = (0..17)
            .map(|index| TopicDef::new(format!("topic_{index}"), "Updates."))
            .collect();
        assert!(topics.validate().unwrap_err().contains("17 topics"));
        let problems = topics.admit();
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("topic 'topic_16' not served: 17 topics; the cap is 16"));
        assert_eq!(topics.topics.len(), MAX_TOPICS);
        topics.validate().unwrap();
        let mut topic_schema = route();
        topic_schema.topics[0].schema = Some("[]".into());
        assert!(topic_schema.validate().unwrap_err().contains("schema is not a JSON object"));

        let big_tool = sized_tool(0, 16 * 1024);
        let big_topic = sized_topic(0, 16 * 1024);
        let mut admitted = route()
            .with_tool(ToolDef::new("verbose", "d".repeat(4 * 1024), r#"{"type":"object"}"#, Risk::Read))
            .with_tool(big_tool)
            .with_topic(big_topic);
        admitted.topics[0].description = "d".repeat(4 * 1024);
        admitted.validate().expect("4 KiB descriptions and 16 KiB schemas are valid for tools and topics");
        let good = admitted.clone();
        assert!(admitted.admit().is_empty(), "admission keeps large valid schemas");
        assert_eq!(admitted, good, "good tools, long descriptions and topics stay unchanged");

        let mut empty = route();
        empty.tools[0].description = " \t\n".into();
        empty.topics[0].description.clear();
        assert!(empty.validate().unwrap_err().contains("description is empty after trimming"));
        let mut names = route();
        names.tools[0].name = "Bad".into();
        names.topics[0].name = "long".repeat(MAX_TOOL_NAME);
        let mut layout = route();
        layout.tools[0].layout = Some(CardLayout::new("x".repeat(MAX_LAYOUT_BYTES + 1), ""));
        let mut topic_deep = route();
        topic_deep.topics[0].schema = Some(nest(35));
        for mut invalid in [schema, not_object, topic_schema, empty, names, layout, topic_deep] {
            assert!(!invalid.admit().is_empty());
            invalid.validate().unwrap();
            assert!(invalid.admit().is_empty(), "admission is idempotent");
        }
        let mut label = route();
        label.label.clear();
        for mut invalid in [bad, brief, label] {
            let original = invalid.clone();
            assert!(invalid.admit().is_empty(), "service fields are not entries");
            assert_eq!(invalid, original, "admission leaves service fields alone");
            assert!(invalid.validate().is_err());
        }
    }

    /// A tool at the name and layout caps with a valid argument object of `schema_bytes`.
    fn sized_tool(index: usize, schema_bytes: usize) -> ToolDef {
        let name = format!("{:_<width$}", format!("t{index}"), width = MAX_TOOL_NAME);
        let head = r#"{"type":"object","properties":{"p":{"type":"string","description":""#;
        let tail = r#""}},"additionalProperties":false}"#;
        let schema = format!("{head}{}{tail}", "s".repeat(schema_bytes - head.len() - tail.len()));
        assert_eq!(schema.len(), schema_bytes);
        ToolDef::new(name, "d", &schema, Risk::Act)
            .with_layout(CardLayout::new("c".repeat(MAX_LAYOUT_BYTES), ""))
    }

    fn sized_topic(index: usize, schema_bytes: usize) -> TopicDef {
        let name = format!("{:_<width$}", format!("p{index}"), width = MAX_TOOL_NAME);
        let head = r#"{"type":"object","properties":{"v":{"type":"string","description":""#;
        let tail = r#""}}}"#;
        let schema = format!("{head}{}{tail}", "s".repeat(schema_bytes - head.len() - tail.len()));
        assert_eq!(schema.len(), schema_bytes);
        TopicDef::new(name, "d").with_schema(schema)
    }

    #[test]
    fn the_tool_cap_admits_128_and_names_the_cap_at_129() {
        let mut m = ServiceManifest::new("studio", "Studio", "The IDE.");
        for i in 0..MAX_TOOLS {
            m = m.with_tool(ToolDef::new(format!("tool_{i}"), "Does one thing.", r#"{"type":"object","properties":{}}"#, Risk::Read));
        }
        assert_eq!(m.tools.len(), 128);
        assert!(m.validate().is_ok(), "{:?}", m.validate());
        let mut over = m.clone().with_tool(ToolDef::new("tool_129", "One too many.", r#"{"type":"object","properties":{}}"#, Risk::Read));
        let err = over.validate().unwrap_err();
        assert!(err.contains("129 tools; the cap is 128"), "{err}");
        let problems = over.admit();
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains("tool 'tool_129' not served: 129 tools; the cap is 128"));
        assert_eq!(over, m, "the earlier tools stay in order");
        over.validate().unwrap();
    }

    #[test]
    fn a_manifest_at_the_text_cap_validates_and_transports() {
        let mut m = ServiceManifest::new(
            &"s".repeat(MAX_SERVICE_ID),
            &"l".repeat(MAX_LABEL_BYTES),
            &"b".repeat(MAX_BRIEF_BYTES),
        );
        for i in 0..MAX_TOOLS {
            m = m.with_tool(sized_tool(i, 10 * 1024));
        }
        for i in 0..MAX_TOPICS {
            m = m.with_topic(sized_topic(i, 10 * 1024));
        }
        let padding = MAX_MANIFEST_BYTES - m.bytes();
        m.tools[0].description.push_str(&"\"".repeat(padding));
        assert_eq!(m.bytes(), MAX_MANIFEST_BYTES);
        assert!(m.validate().is_ok(), "{:?}", m.validate());
        // The registration frame of that manifest must survive the frame cap
        // with every quote escaped.
        let up = HostedUp { from: None, msg: ServiceUp::Register { manifest: m.clone(), port_tag: 1 } };
        let json = up.to_json();
        assert!(json.len() <= MAX_FRAME_BYTES, "{} > {MAX_FRAME_BYTES}", json.len());
        let parsed = HostedUp::parse(&json).expect("a maximal manifest transports");
        assert_eq!(parsed.msg, up.msg);
        // One more byte of text and the byte cap, not the tool cap, says no.
        let mut fat = m;
        fat.tools[0].description.push('d');
        assert!(fat.validate().unwrap_err().contains("manifest"));
        let largest = fat.tools[0].clone();
        let problems = fat.admit();
        assert_eq!(problems.len(), 1);
        assert!(problems[0].contains(&format!("tool '{}' not served: manifest is {} bytes", largest.name, MAX_MANIFEST_BYTES + 1)));
        assert!(problems[0].contains(&format!("entry is {} bytes", largest.bytes())));
        assert!(fat.tool(&largest.name).is_none());
        assert_eq!((fat.tools.len(), fat.topics.len()), (MAX_TOOLS - 1, MAX_TOPICS));
        fat.validate().unwrap();

        // Equal-size entries are removed from the end, across both kinds,
        // until the total fits. Names, schemas and layouts all count.
        let mut tied = ServiceManifest::new("service", "Service", "")
            .with_tool(ToolDef::new("first", "d", r#"{"type":"object"}"#, Risk::Read))
            .with_tool(ToolDef::new("later", "d", r#"{"type":"object"}"#, Risk::Read)
                .with_layout(CardLayout::new("Card", "Run")))
            .with_topic(TopicDef::new("topic", "d").with_schema("{}"));
        for tool in &mut tied.tools {
            tool.description = "d".repeat(MAX_MANIFEST_BYTES / 2 - tool.bytes() + 1);
        }
        let topic = &mut tied.topics[0];
        topic.description = "d".repeat(MAX_MANIFEST_BYTES / 2 - topic.bytes() + 1);
        let problems = tied.admit();
        assert_eq!(problems.len(), 2, "{problems:?}");
        assert!(problems[0].contains("topic 'topic' not served:"));
        assert!(problems[1].contains("tool 'later' not served:"));
        assert_eq!(tied.tools.len(), 1);
        assert_eq!(tied.tools[0].name, "first");
        assert!(tied.topics.is_empty());
        tied.validate().unwrap();

        let mut service_only = ServiceManifest::new("service", "Service", "b".repeat(MAX_MANIFEST_BYTES));
        assert!(service_only.admit().is_empty());
        assert_eq!(service_only.brief.len(), MAX_MANIFEST_BYTES);
        assert!(service_only.validate().unwrap_err().contains("brief"));
        // The frame cap still refuses anything larger than it.
        let huge = format!(
            "{{\"wm_ai\":{{\"from\":null,\"msg\":{{\"Unregister\":{{}}}}}},\"pad\":\"{}\"}}",
            "x".repeat(MAX_FRAME_BYTES)
        );
        assert_eq!(HostedUp::parse(&huge), None);
    }

    fn json_array(bytes: usize) -> String {
        format!(r#"["{}"]"#, "x".repeat(bytes - 4))
    }

    #[test]
    fn a_large_json_result_passes_intact_up_to_wire_cap() {
        let text = json_array(64 * 1024);
        let data = json_array(32 * 1024);
        let mut r = ToolResult::ok("c1", &text, "é".repeat(MAX_NOTE_BYTES)).with_data(&data);
        r.bound();
        assert_eq!(r.outcome, ToolOutcome::Ok, "a large JSON reply succeeds");
        assert!(r.text == text && r.data == data, "both JSON payloads retain every byte");
        assert!(parse_tool_json(&r.text).is_ok(), "the model receives valid JSON text");
        assert!(parse_tool_json(&r.data).is_ok(), "the panel receives valid JSON data");
        assert!(r.note.len() <= MAX_NOTE_BYTES, "the transcript note fits its byte cap");
        assert!(r.note.ends_with('…'), "the shortened UTF-8 note ends with an ellipsis");
        let frame = HostedUp { from: None, msg: ServiceUp::Result(r) };
        assert!(HostedUp::parse(&frame.to_json()) == Some(frame), "the large JSON reply round-trips");
    }

    #[test]
    fn a_result_over_wire_cap_fails_cleanly_without_truncating_json() {
        let oversized = MAX_RESULT_WIRE_BYTES + 1;
        let large_image = || "A".repeat(MAX_RESULT_IMAGE_BYTES + 1);
        let mut r = ToolResult::ok("c1", json_array(oversized), "done")
            .with_data("{}")
            .with_preview()
            .with_disposition(Disposition::EndTurn)
            .with_subscription(SubscriptionRequest::new("trip"));
        r.images.push(large_image());
        r.task = Some(TaskCard { title: "t".into(), ..Default::default() });
        r.bound();
        assert_eq!(r.outcome, ToolOutcome::Failed, "oversized text fails the call");
        assert_eq!(r.call_id, "c1", "the failure still identifies the call");
        assert_eq!(r.text, result_too_large(oversized), "the failure reports the original text size");
        assert_eq!(r.note, r.text, "the transcript shows the same failure as the model");
        assert!(r.data.is_empty(), "the failure discards structured data");
        assert!(r.images.is_empty(), "the failure discards images");
        assert!(!r.preview, "the failure clears the preview request");
        assert_eq!(r.disposition, Disposition::Continue, "the failure lets the turn continue");
        assert!(r.subscribe.is_empty(), "the failure requests no subscriptions");
        assert!(r.task.is_none(), "the failure clears the task card");
        assert!(ServiceUp::Result(r).validate().is_ok(), "the receiver accepts the failure");

        let text = json_array(1024);
        let mut r = ToolResult::ok("c2", &text, "").with_data(json_array(MAX_RESULT_WIRE_BYTES));
        r.bound();
        assert_eq!(r.outcome, ToolOutcome::Ok, "excess data leaves the call successful");
        assert_eq!(r.text, text, "shedding data preserves the JSON text");
        assert!(r.data.is_empty(), "data over the combined budget is dropped whole");

        r.images.push(large_image());
        r.bound();
        assert!(r.images.is_empty(), "the oversized image is removed");
        assert_eq!(r.text, text, "the image marker leaves the JSON array intact");
        assert!(parse_tool_json(&r.text).is_ok(), "the JSON array still parses after shedding images");
        assert!(r.note.contains("[images truncated by the router]"), "the note records the dropped image");

        let text = "x".repeat(MAX_RESULT_WIRE_BYTES - 8);
        let mut r = ToolResult::ok("c3", &text, "");
        r.images.push(large_image());
        r.bound();
        assert_eq!(r.outcome, ToolOutcome::Ok, "near-cap text succeeds after shedding images");
        assert!(r.text == text, "near-cap text retains every byte");
        assert!(r.note.contains("[images truncated by the router]"), "the marker fits in the note");
        assert!(ServiceUp::Result(r).validate().is_ok(), "the receiver accepts near-cap text");

        // A note already at or past MAX_NOTE_BYTES trims its prefix so the marker survives.
        for long_note in ["n".repeat(MAX_NOTE_BYTES), format!("a{}", "é".repeat(MAX_NOTE_BYTES))] {
            let mut r = ToolResult::ok("c4", &text, long_note);
            r.images.push(large_image());
            r.bound();
            assert!(r.note.len() <= MAX_NOTE_BYTES, "a full note stays within MAX_NOTE_BYTES");
            assert!(
                r.note.ends_with("… [images truncated by the router]"),
                "a full note keeps the image-truncation marker after trimming its prefix",
            );
            assert!(ServiceUp::Result(r.clone()).validate().is_ok(), "the receiver accepts the trimmed note");
            // Re-bounding after another dropped image does not duplicate the marker.
            r.images.push(large_image());
            r.bound();
            assert_eq!(
                r.note.matches("[images truncated by the router]").count(),
                1,
                "re-bounding does not duplicate the image-truncation marker",
            );
        }

        // Scalar JSON replies keep valid JSON text and move the image marker to note,
        // while plain prose starting with numbers, quotes or keywords keeps the text marker.
        for scalar in [r#""text""#, "42", "-3.5", "true", "false", "null"] {
            let mut r = ToolResult::ok("c5", scalar, "");
            r.images.push(large_image());
            r.bound();
            assert_eq!(r.text, scalar, "shedding images preserves scalar JSON reply {scalar}");
            assert!(parse_tool_json(&r.text).is_ok(), "scalar JSON reply {scalar} still parses");
            assert!(r.note.contains("[images truncated by the router]"), "scalar JSON reply {scalar} notes the dropped image");
        }
        for plain in ["41 min", "true: saved", r#""hi" there"#, r#""a" and "b""#] {
            let mut r = ToolResult::ok("c6", plain, "");
            r.images.push(large_image());
            r.bound();
            assert!(
                r.text.ends_with("\n…[images truncated by the router]"),
                "plain text {plain:?} still receives the model-visible image marker",
            );
            assert!(r.note.is_empty(), "plain text {plain:?} does not move the marker into note");
        }
    }

    #[test]
    fn control_char_heavy_results_and_manifests_stay_within_frame_cap() {
        for c in (0u8..=127).map(char::from).chain(['é', '—', '🛠']) {
            let s = c.to_string();
            assert_eq!(json_escaped_len(&s) + 2, s.serialize_json().len(), "encoded length for {c:?}");
        }
        let sample = ToolResult::ok("c1", "ok", "");
        let unstamped = HostedUp { from: None, msg: ServiceUp::Result(sample.clone()) }.to_json().len();
        assert_eq!(sample.hosted_frame_bytes(), unstamped, "the wrapper matches real unstamped frames");
        let stamp = EndpointId("\"".repeat(MAX_ID_BYTES));
        let stamped = HostedUp { from: Some(stamp.clone()), msg: ServiceUp::Result(sample) }.to_json().len();
        assert_eq!(stamped - unstamped, MAX_HOSTED_STAMP_BYTES, "the reserve matches worst-case stamp growth");

        let check_transport = |msg, case| {
            let frame = HostedUp { from: Some(stamp.clone()), msg };
            let json = frame.to_json();
            assert!(json.len() <= MAX_FRAME_BYTES, "{case} fits the stamped frame cap");
            assert!(HostedUp::parse(&json) == Some(frame), "{case} round-trips after stamping");
        };
        let mut r = ToolResult::ok("c1", "\x00".repeat(700 * 1024), "");
        r.bound();
        assert_eq!(r.outcome, ToolOutcome::Failed, "oversized encoded text fails cleanly");
        assert_eq!(r.call_id, "c1", "the frame-size failure identifies the original call");
        assert!(r.text.contains(&MAX_FRAME_BYTES.to_string()), "the failure names the hosted frame cap");
        let msg = ServiceUp::Result(r);
        assert!(msg.validate().is_ok(), "the receiver accepts the frame-size failure");
        check_transport(msg, "the frame-size failure");

        let text = "\x1b".repeat(500 * 1024);
        let mut r = ToolResult::ok("c2", &text, "").with_data("\x1b".repeat(250 * 1024));
        r.bound();
        assert_eq!(r.outcome, ToolOutcome::Ok, "shedding encoded data preserves success");
        assert!(r.text == text, "shedding encoded data preserves every text byte");
        assert!(r.data.is_empty(), "excess encoded data is dropped whole");
        check_transport(ServiceUp::Result(r), "the text without excess data");

        let text = "\x1b".repeat(550 * 1024);
        let mut r = ToolResult::ok("c3", &text, "");
        let first_image = "A".repeat(450 * 1024);
        r.images = vec![first_image.clone(), "B".repeat(450 * 1024)];
        r.bound();
        assert_eq!(r.outcome, ToolOutcome::Ok, "shedding trailing images preserves success");
        assert!(r.text == text, "shedding trailing images preserves every text byte");
        assert!(r.images == [first_image], "the first image survives while the trailing image is shed");
        assert!(r.note.contains("[images truncated by the router]"), "the note records the shed image");
        check_transport(ServiceUp::Result(r), "the reply with its first image");

        // A NUL-filled note trims 180 encoded bytes when making room for the
        // image-truncation marker; that shrinkage must not suppress shedding
        // the over-cap image itself, while still avoiding over-shedding once an
        // image has actually been dropped.
        let text = "\x00".repeat(698_978 - MAX_NOTE_BYTES);
        let note = "\x00".repeat(MAX_NOTE_BYTES);
        let mut r = ToolResult::ok(stamp.as_str(), &text, &note);
        r.images.push("AAAA".into());
        assert_eq!(
            r.hosted_frame_bytes(),
            MAX_UNSTAMPED_FRAME_BYTES + 6,
            "one AAAA image puts the NUL-note exact-ceiling frame 6 bytes over",
        );
        r.bound();
        assert_eq!(r.outcome, ToolOutcome::Ok, "shedding the single over-cap image preserves success");
        assert!(r.text == text, "shedding the single over-cap image preserves every text byte");
        assert!(r.images.is_empty(), "the over-cap image is actually shed before marking the note");
        assert!(
            r.note.ends_with("… [images truncated by the router]"),
            "the note records the shed image",
        );
        assert_eq!(
            r.hosted_frame_bytes(),
            MAX_UNSTAMPED_FRAME_BYTES - 180,
            "shedding AAAA and marking the NUL note saves 6 + 180 encoded bytes",
        );
        assert!(ServiceUp::Result(r.clone()).validate().is_ok(), "the receiver accepts the shed reply");
        check_transport(ServiceUp::Result(r), "the NUL-note reply after shedding AAAA");

        let mut r = ToolResult::ok(stamp.as_str(), &text, &note);
        r.images = vec!["AAAA".into(), "BBBB".into()];
        assert_eq!(
            r.hosted_frame_bytes(),
            MAX_UNSTAMPED_FRAME_BYTES + 13,
            "two 4-byte images put the NUL-note exact-ceiling frame 13 bytes over",
        );
        r.bound();
        assert_eq!(r.outcome, ToolOutcome::Ok, "shedding the trailing image preserves success");
        assert_eq!(r.images, ["AAAA"], "the exact note-marker savings keep the first image");
        assert!(
            r.note.ends_with("… [images truncated by the router]"),
            "the note records the shed trailing image",
        );
        assert_eq!(
            r.hosted_frame_bytes(),
            MAX_UNSTAMPED_FRAME_BYTES - 174,
            "shedding BBBB and marking the NUL note leaves the frame 174 bytes under the cap",
        );
        assert!(ServiceUp::Result(r).validate().is_ok(), "the receiver accepts the reply keeping AAAA");

        // Conversely, when shedding the first image alone drops below the cap
        // but appending the marker to an empty note (+32) pushes back over, the
        // loop keeps shedding until the marked frame fits.
        let text = "\x00".repeat(698_972);
        let mut r = ToolResult::ok(stamp.as_str(), &text, "");
        r.images = vec!["A".repeat(30), "B".repeat(20)];
        assert_eq!(
            r.hosted_frame_bytes(),
            MAX_UNSTAMPED_FRAME_BYTES + 19,
            "two images put the 36-bytes-under frame 19 bytes over",
        );
        r.bound();
        assert_eq!(r.outcome, ToolOutcome::Ok, "continuing to shed after adding the marker preserves success");
        assert!(r.images.is_empty(), "the second image is also shed once the +32-byte marker pushes back over");
        assert_eq!(r.note, "[images truncated by the router]", "the empty note receives the truncation marker");
        assert_eq!(
            r.hosted_frame_bytes(),
            MAX_UNSTAMPED_FRAME_BYTES - 4,
            "shedding both images (55 bytes) and adding the marker (+32) lands 4 bytes under the cap",
        );
        assert!(ServiceUp::Result(r).validate().is_ok(), "the receiver accepts the multi-shed marked reply");

        let tool = |name| ToolDef::new(name, "\x00".repeat(400 * 1024), r#"{"type":"object"}"#, Risk::Read);
        let mut manifest = ServiceManifest::new("files", "Files", "").with_tool(tool("first")).with_tool(tool("later"));
        let err = manifest.validate().expect_err("the control-heavy manifest exceeds the frame cap");
        assert!(err.contains("frame cap"), "the refusal names the encoded frame cap");
        let problems = manifest.admit();
        assert_eq!(problems.len(), 1, "one entry removal brings the manifest within budget");
        assert!(problems[0].contains("tool 'later' not served"), "the diagnostic identifies the later tied tool");
        assert_eq!(manifest.tools.len(), 1, "admission keeps one tool");
        assert_eq!(manifest.tools[0].name, "first", "the earlier tied tool survives");
        assert!(manifest.validate().is_ok(), "the admitted manifest passes wire validation");
        check_transport(ServiceUp::Register { manifest, port_tag: u32::MAX }, "the admitted manifest");
    }

    #[test]
    fn opaque_ids_are_bounded_printable_and_whitespace_free() {
        assert!(is_opaque_id("c-12_ab"));
        assert!(!is_opaque_id(""));
        assert!(!is_opaque_id("c 1"));
        assert!(!is_opaque_id(&"x".repeat(MAX_ID_BYTES + 1)));
    }

    #[test]
    fn subscription_and_message_caps_are_enforced_on_arrival() {
        let oversized_filter = HostedDown {
            to: Some(ep("e1")),
            msg: ServiceDown::Subscribe {
                sub_id: "s1".into(),
                topic: "trip".into(),
                filter: Some(format!(r#"{{"value":"{}"}}"#, "x".repeat(MAX_FILTER_BYTES))),
            },
        };
        assert_eq!(HostedDown::parse(&oversized_filter.to_json()), None);
        let non_object_filter = HostedDown {
            to: Some(ep("e1")),
            msg: ServiceDown::Subscribe {
                sub_id: "s1".into(),
                topic: "trip".into(),
                filter: Some("[]".into()),
            },
        };
        assert_eq!(HostedDown::parse(&non_object_filter.to_json()), None);
        let unsubscribed = HostedDown {
            to: Some(ep("e1")),
            msg: ServiceDown::Unsubscribe { sub_id: "x".repeat(MAX_ID_BYTES + 1) },
        };
        assert_eq!(HostedDown::parse(&unsubscribed.to_json()), None);
        for (text, data) in [
            ("x".repeat(MAX_MESSAGE_BYTES + 1), None),
            (String::new(), Some(format!(r#""{}""#, "x".repeat(MAX_DATA_BYTES)))),
            (String::new(), Some("not-json".into())),
        ] {
            let up = HostedUp {
                from: Some(ep("e1")),
                msg: ServiceUp::Message {
                    sub_id: "s1".into(),
                    topic: "trip".into(),
                    text,
                    data,
                    final_: false,
                },
            };
            assert_eq!(HostedUp::parse(&up.to_json()), None);
        }
        let too_many = HostedUp {
            from: Some(ep("e1")),
            msg: ServiceUp::Result(ToolResult {
                subscribe: (0..17).map(|_| SubscriptionRequest::new("trip")).collect(),
                ..ToolResult::ok("c1", "started", "")
            }),
        };
        assert_eq!(HostedUp::parse(&too_many.to_json()), None);
    }
}
