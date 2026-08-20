//! The bounded chat/tool wire schema — the single coordination surface
//! between chat UIs (AI Content, sandbox) and a chat service. Everything
//! here encodes to / decodes from the asset client's strict JSON `Value`;
//! decoding is fail-closed (unknown tags, over-budget text and malformed ids
//! are refusals, never best-effort). No type in this module can represent a
//! credential — that is a schema-level guarantee the tests pin.

use makepad_asset_client::json::{self, Value};
use makepad_asset_data::AssetRevisionId;
use std::str::FromStr;

/// Bump on any incompatible schema change; consumers refuse unknown majors.
pub const WIRE_VERSION: u32 = 1;

// ---------------------------------------------------------------- bounds

/// One chat message's text.
pub const MAX_MESSAGE_BYTES: usize = 16 * 1024;
/// Messages retained per session; overflow refuses the send (start a new
/// session) rather than silently dropping context.
pub const MAX_MESSAGES: usize = 128;
/// Typed asset inputs bound per turn.
pub const MAX_ATTACHMENTS: usize = 8;
/// Exact input revisions per operation.create call (bounded input closure).
/// The name predates the operation tools and is pinned by the wire tests.
pub const MAX_TRANSFORM_INPUTS: usize = 4;
/// Serialized tool arguments / results.
pub const MAX_TOOL_JSON_BYTES: usize = 16 * 1024;
/// One streaming delta.
pub const MAX_DELTA_BYTES: usize = 4 * 1024;

/// Split `text` into UTF-8-safe chunks of at most `MAX_DELTA_BYTES`.
pub fn split_delta_text(text: &str) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut start = 0;
    while start < text.len() {
        let mut end = (start + MAX_DELTA_BYTES).min(text.len());
        while end > start && !text.is_char_boundary(end) {
            end -= 1;
        }
        if end == start {
            end = start + text[start..].chars().next().map(|c| c.len_utf8()).unwrap_or(1);
        }
        out.push(text[start..end].to_string());
        start = end;
    }
    out
}
/// Accumulated assistant text per turn.
pub const MAX_TURN_TEXT_BYTES: usize = 64 * 1024;
/// Tool progress note (mirrors the Asset Server heartbeat bound).
pub const MAX_NOTE_BYTES: usize = 200;
/// Tool rounds within one user turn.
/// Tool rounds per user turn. A level-building turn legitimately spends
/// schema + a few narrowing queries + get_source + set_source + a
/// correction pass — 8 was hit by real (non-looping) exploration the day
/// the catalog grew to ~3k models. Fail-closed as before; the session's
/// history/token budgets remain the real backstop.
pub const MAX_TOOL_ROUNDS: u32 = 16;
/// Native function `call_id` / session tool id.
pub const MAX_TOOL_CALL_ID: usize = 64;
/// Progress callbacks retained from one tool execution.
pub const MAX_PROGRESS_EVENTS: usize = 32;
/// Public error text on the chat wire / provider events.
pub const MAX_PUBLIC_ERROR_BYTES: usize = 256;

/// Cap and redact a transport/provider error before it crosses a public
/// boundary. Never forwards tokens, headers, or CR/LF.
pub fn sanitize_public_error(message: &str) -> String {
    let lower = message.to_ascii_lowercase();
    if lower.contains("bearer")
        || lower.contains("authorization")
        || lower.contains("sk-")
        || lower.contains("xai-")
        || lower.contains("api_key")
        || lower.contains("mpat_")
    {
        return "provider error".to_string();
    }
    let mut out = String::new();
    for c in message.chars() {
        if c == '\n' || c == '\r' || (c as u32) < 0x20 || c == '\u{7f}' {
            continue;
        }
        let n = c.len_utf8();
        if out.len().saturating_add(n) > MAX_PUBLIC_ERROR_BYTES {
            break;
        }
        out.push(c);
    }
    if out.is_empty() {
        "provider error".to_string()
    } else {
        out
    }
}

/// Lowercase identifier: `[a-z0-9_-]{1,32}` (attachment roles, tool names,
/// profile ids as they appear on this wire).
pub fn ident_ok(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 32
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

// ------------------------------------------------------------- providers

/// The three chat providers. Deliberately no `Auto` variant: provider
/// choice is user/state-explicit, and unavailability is surfaced, never
/// routed around.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderKind {
    /// Fleet or local Qwen chat backbone (Qwen3.8 when honestly ready).
    FleetQwen,
    /// OpenAI Responses API (`gpt-5.6` by default).
    OpenAi,
    /// xAI Grok Responses API (`grok-4.5` by default).
    Grok,
}

impl ProviderKind {
    pub fn slug(&self) -> &'static str {
        match self {
            ProviderKind::FleetQwen => "fleet-qwen",
            ProviderKind::OpenAi => "openai",
            ProviderKind::Grok => "grok",
        }
    }

    pub fn from_slug(s: &str) -> Option<ProviderKind> {
        match s {
            "fleet-qwen" => Some(ProviderKind::FleetQwen),
            "openai" => Some(ProviderKind::OpenAi),
            "grok" => Some(ProviderKind::Grok),
            _ => None,
        }
    }

    /// Native function calling (OpenAI / Grok). Fleet Qwen keeps the
    /// textual `<<tool>>` marker contract.
    pub fn uses_native_tools(self) -> bool {
        matches!(self, ProviderKind::OpenAi | ProviderKind::Grok)
    }
}

/// Honest, probe-derived availability. `Unavailable.reason` is what the UI
/// shows beside a disabled provider row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProviderAvailability {
    Available {
        /// Model actually serving (e.g. `qwen3.8-27b`, `claude-code`).
        model: String,
        /// Where it lives (node base URL, CLI path) — diagnostics only.
        detail: String,
    },
    Unavailable { reason: String },
}

impl ProviderAvailability {
    pub fn is_available(&self) -> bool {
        matches!(self, ProviderAvailability::Available { .. })
    }

    pub fn encode(&self) -> Value {
        match self {
            ProviderAvailability::Available { model, detail } => json::obj(vec![
                ("state", json::s("available")),
                ("model", json::s(model.clone())),
                ("detail", json::s(detail.clone())),
            ]),
            ProviderAvailability::Unavailable { reason } => json::obj(vec![
                ("state", json::s("unavailable")),
                ("reason", json::s(reason.clone())),
            ]),
        }
    }

    pub fn decode(v: &Value) -> Result<Self, &'static str> {
        match v.get("state").and_then(Value::as_str) {
            Some("available") => Ok(ProviderAvailability::Available {
                model: str_field(v, "model", 128)?,
                detail: str_field(v, "detail", 512)?,
            }),
            Some("unavailable") => Ok(ProviderAvailability::Unavailable {
                reason: str_field(v, "reason", 512)?,
            }),
            _ => Err("availability state"),
        }
    }
}

// -------------------------------------------------------------- messages

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChatRole {
    User,
    Assistant,
    System,
    /// A tool result fed back into the conversation.
    Tool,
}

impl ChatRole {
    pub fn slug(&self) -> &'static str {
        match self {
            ChatRole::User => "user",
            ChatRole::Assistant => "assistant",
            ChatRole::System => "system",
            ChatRole::Tool => "tool",
        }
    }

    pub fn from_slug(s: &str) -> Option<ChatRole> {
        match s {
            "user" => Some(ChatRole::User),
            "assistant" => Some(ChatRole::Assistant),
            "system" => Some(ChatRole::System),
            "tool" => Some(ChatRole::Tool),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub text: String,
}

impl ChatMessage {
    pub fn new(role: ChatRole, text: impl Into<String>) -> ChatMessage {
        ChatMessage { role, text: text.into() }
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        if self.text.is_empty() {
            return Err("empty message");
        }
        if self.text.len() > MAX_MESSAGE_BYTES {
            return Err("message too large");
        }
        Ok(())
    }

    pub fn encode(&self) -> Value {
        json::obj(vec![
            ("role", json::s(self.role.slug())),
            ("text", json::s(self.text.clone())),
        ])
    }

    pub fn decode(v: &Value) -> Result<Self, &'static str> {
        let role = v
            .get("role")
            .and_then(Value::as_str)
            .and_then(ChatRole::from_slug)
            .ok_or("message role")?;
        let msg = ChatMessage { role, text: str_field(v, "text", MAX_MESSAGE_BYTES)? };
        msg.validate()?;
        Ok(msg)
    }
}

// ----------------------------------------------------------- attachments

/// One existing immutable asset revision bound into a turn as a typed
/// input (the UI's "attachment chip"). Order in the containing slice is the
/// input order. The client names ONLY the exact revision id — the backend
/// resolves and verifies bytes itself; paths never cross this wire.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AttachmentBinding {
    pub revision: AssetRevisionId,
    /// Semantic role, e.g. `source`, `reference`, `mask`.
    pub role: String,
}

impl AttachmentBinding {
    pub fn validate(&self) -> Result<(), &'static str> {
        if !ident_ok(&self.role) {
            return Err("attachment role");
        }
        Ok(())
    }

    pub fn encode(&self) -> Value {
        json::obj(vec![
            ("revision", json::s(self.revision.to_string())),
            ("role", json::s(self.role.clone())),
        ])
    }

    pub fn decode(v: &Value) -> Result<Self, &'static str> {
        let rev = v.get("revision").and_then(Value::as_str).ok_or("attachment revision")?;
        let revision = AssetRevisionId::from_str(rev).map_err(|_| "attachment revision")?;
        let binding = AttachmentBinding { revision, role: str_field(v, "role", 32)? };
        binding.validate()?;
        Ok(binding)
    }
}

// ----------------------------------------------------------- tool outcome

/// The structured result of one tool execution. `Unavailable` is a
/// first-class, non-error answer ("this server honestly cannot do that");
/// `Denied` is an ACL refusal; `Refused` is a contract refusal (bad
/// arguments, unpinned input, content-type mismatch); `Failed` is an
/// operational error.
#[derive(Clone, Debug, PartialEq)]
pub enum ToolOutcome {
    Ok { value: Value },
    Unavailable { reason: String },
    Denied { what: String },
    Refused { what: String },
    Failed { message: String },
}

impl ToolOutcome {
    pub fn encode(&self) -> Value {
        match self {
            ToolOutcome::Ok { value } => {
                json::obj(vec![("outcome", json::s("ok")), ("value", value.clone())])
            }
            ToolOutcome::Unavailable { reason } => json::obj(vec![
                ("outcome", json::s("unavailable")),
                ("reason", json::s(reason.clone())),
            ]),
            ToolOutcome::Denied { what } => {
                json::obj(vec![("outcome", json::s("denied")), ("what", json::s(what.clone()))])
            }
            ToolOutcome::Refused { what } => {
                json::obj(vec![("outcome", json::s("refused")), ("what", json::s(what.clone()))])
            }
            ToolOutcome::Failed { message } => json::obj(vec![
                ("outcome", json::s("failed")),
                ("message", json::s(message.clone())),
            ]),
        }
    }

    /// Object-shaped `ok` values and an encoded size of at most
    /// [`MAX_TOOL_JSON_BYTES`]. Call before emit, store, or continuation.
    pub fn validate(&self) -> Result<(), &'static str> {
        if let ToolOutcome::Ok { value } = self {
            if !matches!(value, Value::Obj(_)) {
                return Err("outcome value");
            }
        }
        let field = match self {
            ToolOutcome::Unavailable { reason } => Some(reason.as_str()),
            ToolOutcome::Denied { what } | ToolOutcome::Refused { what } => Some(what.as_str()),
            ToolOutcome::Failed { message } => Some(message.as_str()),
            ToolOutcome::Ok { .. } => None,
        };
        if field.is_some_and(|s| s.len() > 512) {
            return Err("outcome field");
        }
        if self.encode().to_json().len() > MAX_TOOL_JSON_BYTES {
            return Err("outcome too large");
        }
        Ok(())
    }

    /// A copy that FITS the wire: message fields truncated to [`validate`]'s
    /// 512-byte cap (char-boundary safe, ellipsis appended), an oversized
    /// `Ok` value downgraded to the same honest `Failed` the broker's own
    /// bounding uses. Senders call this before posting — `encode` does not
    /// validate, so an unclamped long refusal reaches the receiving parser
    /// and is refused wholesale as malformed (play-session-1 entry 22: a
    /// 517-byte cramming refusal 400'd and the model was told the app
    /// never answered). Truncated guidance beats undeliverable guidance.
    pub fn clamped(self) -> Self {
        fn cap(s: String) -> String {
            const MAX: usize = 512;
            if s.len() <= MAX {
                return s;
            }
            let mut end = MAX - '…'.len_utf8();
            while end > 0 && !s.is_char_boundary(end) {
                end -= 1;
            }
            let mut out = s[..end].to_string();
            out.push('…');
            out
        }
        let out = match self {
            ToolOutcome::Ok { value } => ToolOutcome::Ok { value },
            ToolOutcome::Unavailable { reason } => {
                ToolOutcome::Unavailable { reason: cap(reason) }
            }
            ToolOutcome::Denied { what } => ToolOutcome::Denied { what: cap(what) },
            ToolOutcome::Refused { what } => ToolOutcome::Refused { what: cap(what) },
            ToolOutcome::Failed { message } => ToolOutcome::Failed { message: cap(message) },
        };
        if out.validate().is_err() {
            // Only the whole-outcome size cap can still fail here (an
            // oversized Ok value); mirror the broker's own bounding.
            return ToolOutcome::Failed { message: "tool result too large".to_string() };
        }
        out
    }

    pub fn decode(v: &Value) -> Result<Self, &'static str> {
        if v.to_json().len() > MAX_TOOL_JSON_BYTES {
            return Err("outcome too large");
        }
        let out = match v.get("outcome").and_then(Value::as_str) {
            Some("ok") => {
                let value = v.get("value").cloned().ok_or("outcome value")?;
                if !matches!(value, Value::Obj(_)) {
                    return Err("outcome value");
                }
                ToolOutcome::Ok { value }
            }
            Some("unavailable") => {
                ToolOutcome::Unavailable { reason: str_field(v, "reason", 512)? }
            }
            Some("denied") => ToolOutcome::Denied { what: str_field(v, "what", 512)? },
            Some("refused") => ToolOutcome::Refused { what: str_field(v, "what", 512)? },
            Some("failed") => ToolOutcome::Failed { message: str_field(v, "message", 512)? },
            _ => return Err("outcome tag"),
        };
        out.validate()?;
        Ok(out)
    }
}

// ---------------------------------------------------------------- events

/// One ordered event of a session's stream. `seq` is monotonic per session;
/// a consumer that observes a gap missed events and should resync.
#[derive(Clone, Debug, PartialEq)]
pub struct ChatEvent {
    pub seq: u64,
    pub body: ChatEventBody,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ChatEventBody {
    /// Streaming assistant text.
    Delta { text: String },
    /// The assistant invoked a tool; `args` is the raw argument object.
    ToolCall { id: String, name: String, args: Value },
    /// Bounded progress of a running tool (mirrors job heartbeats).
    ToolProgress { id: String, permille: u16, note: String },
    ToolResult { id: String, outcome: ToolOutcome },
    /// The turn finished normally.
    Done,
    Cancelled,
    Error { code: String, message: String },
}

impl ChatEvent {
    pub fn encode(&self) -> Value {
        let mut pairs: Vec<(&str, Value)> =
            vec![("seq", Value::Int(self.seq.min(i64::MAX as u64) as i64))];
        match &self.body {
            ChatEventBody::Delta { text } => {
                pairs.push(("type", json::s("delta")));
                pairs.push(("text", json::s(text.clone())));
            }
            ChatEventBody::ToolCall { id, name, args } => {
                pairs.push(("type", json::s("tool_call")));
                pairs.push(("id", json::s(id.clone())));
                pairs.push(("name", json::s(name.clone())));
                pairs.push(("args", args.clone()));
            }
            ChatEventBody::ToolProgress { id, permille, note } => {
                pairs.push(("type", json::s("tool_progress")));
                pairs.push(("id", json::s(id.clone())));
                pairs.push(("permille", Value::Int(*permille as i64)));
                pairs.push(("note", json::s(note.clone())));
            }
            ChatEventBody::ToolResult { id, outcome } => {
                pairs.push(("type", json::s("tool_result")));
                pairs.push(("id", json::s(id.clone())));
                pairs.push(("result", outcome.encode()));
            }
            ChatEventBody::Done => pairs.push(("type", json::s("done"))),
            ChatEventBody::Cancelled => pairs.push(("type", json::s("cancelled"))),
            ChatEventBody::Error { code, message } => {
                pairs.push(("type", json::s("error")));
                pairs.push(("code", json::s(code.clone())));
                pairs.push(("message", json::s(message.clone())));
            }
        }
        json::obj(pairs)
    }

    pub fn decode(v: &Value) -> Result<Self, &'static str> {
        let seq = v.get("seq").and_then(Value::as_u64).ok_or("event seq")?;
        let body = match v.get("type").and_then(Value::as_str) {
            Some("delta") => ChatEventBody::Delta { text: str_field(v, "text", MAX_DELTA_BYTES)? },
            Some("tool_call") => {
                let args = v.get("args").cloned().ok_or("tool args")?;
                if !matches!(args, Value::Obj(_)) {
                    return Err("tool args");
                }
                if args.to_json().len() > MAX_TOOL_JSON_BYTES {
                    return Err("tool args");
                }
                ChatEventBody::ToolCall {
                    id: str_field(v, "id", 64)?,
                    name: str_field(v, "name", 32)?,
                    args,
                }
            }
            Some("tool_progress") => {
                let permille = v.get("permille").and_then(Value::as_u64).ok_or("permille")?;
                if permille > 1000 {
                    return Err("permille range");
                }
                ChatEventBody::ToolProgress {
                    id: str_field(v, "id", 64)?,
                    permille: permille as u16,
                    note: str_field(v, "note", MAX_NOTE_BYTES)?,
                }
            }
            Some("tool_result") => ChatEventBody::ToolResult {
                id: str_field(v, "id", 64)?,
                outcome: ToolOutcome::decode(v.get("result").ok_or("tool result")?)?,
            },
            Some("done") => ChatEventBody::Done,
            Some("cancelled") => ChatEventBody::Cancelled,
            Some("error") => ChatEventBody::Error {
                code: str_field(v, "code", 64)?,
                message: str_field(v, "message", MAX_PUBLIC_ERROR_BYTES)?,
            },
            _ => return Err("event type"),
        };
        Ok(ChatEvent { seq, body })
    }
}

// ---------------------------------------------------------------- helpers

fn str_field(v: &Value, key: &'static str, max: usize) -> Result<String, &'static str> {
    let s = v.get(key).and_then(Value::as_str).ok_or(key)?;
    if s.len() > max {
        return Err(key);
    }
    Ok(s.to_string())
}
