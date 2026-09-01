//! The provider-neutral subset of the bounded chat wire schema.

use makepad_strict_json::{self as json, Value};

/// One chat message's text.
pub const MAX_MESSAGE_BYTES: usize = 16 * 1024;
/// Messages retained per session; overflow refuses the send (start a new
/// session) rather than silently dropping context.
pub const MAX_MESSAGES: usize = 128;
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
/// Native function `call_id` / session tool id.
pub const MAX_TOOL_CALL_ID: usize = 64;
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
    /// The `claude` CLI installed and logged in on the broker host, run
    /// headless ([`crate::providers::claude`]). No key passes through us:
    /// the host has the CLI or the provider is unavailable.
    ClaudeCli,
    /// The `codex` CLI on the broker host ([`crate::providers::codex_cli`]).
    CodexCli,
    /// The `grok` CLI on the broker host ([`crate::providers::grok_cli`]).
    GrokCli,
}

/// Where the model actually runs. `Local` = our own ai-hub fleet on the
/// LAN; `Cloud` = a frontier vendor, reached either by API key (OpenAi,
/// Grok) or through a logged-in CLI on the broker host. Frontends that
/// promise "local AI only" filter on this, and the wire carries it per
/// provider row so a client never has to know which kinds are which.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Locality {
    Local,
    Cloud,
}

impl Locality {
    pub fn slug(self) -> &'static str {
        match self {
            Locality::Local => "local",
            Locality::Cloud => "cloud",
        }
    }
}

impl ProviderKind {
    /// Every kind the broker can be asked for, in the order the providers
    /// route lists them.
    pub const ALL: [ProviderKind; 6] = [
        ProviderKind::FleetQwen,
        ProviderKind::OpenAi,
        ProviderKind::Grok,
        ProviderKind::ClaudeCli,
        ProviderKind::CodexCli,
        ProviderKind::GrokCli,
    ];

    pub fn slug(&self) -> &'static str {
        match self {
            ProviderKind::FleetQwen => "fleet-qwen",
            ProviderKind::OpenAi => "openai",
            ProviderKind::Grok => "grok",
            ProviderKind::ClaudeCli => "claude-cli",
            ProviderKind::CodexCli => "codex-cli",
            ProviderKind::GrokCli => "grok-cli",
        }
    }

    pub fn from_slug(s: &str) -> Option<ProviderKind> {
        match s {
            "fleet-qwen" => Some(ProviderKind::FleetQwen),
            "openai" => Some(ProviderKind::OpenAi),
            "grok" => Some(ProviderKind::Grok),
            "claude-cli" => Some(ProviderKind::ClaudeCli),
            "codex-cli" => Some(ProviderKind::CodexCli),
            "grok-cli" => Some(ProviderKind::GrokCli),
            _ => None,
        }
    }

    pub fn locality(self) -> Locality {
        match self {
            ProviderKind::FleetQwen => Locality::Local,
            _ => Locality::Cloud,
        }
    }

    /// A vendor CLI on the broker host (no key anywhere in our stack).
    pub fn is_cli(self) -> bool {
        matches!(self, ProviderKind::ClaudeCli | ProviderKind::CodexCli | ProviderKind::GrokCli)
    }

    /// Native function calling (OpenAI / Grok APIs). Fleet Qwen and the
    /// CLIs keep the textual `<<tool>>` marker contract.
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

/// PRESENTATION-ONLY serving facts observed while a turn streams, ADDITIVE
/// on the `delta` event: a client that predates this field ignores it and
/// behaves exactly as before, which is why this rides on `delta` rather
/// than arriving as a new event tag (unknown tags are refusals here).
///
/// `gen_tokens` counts tokens the SERVING box has generated in the current
/// provider round — it restarts at 0 every round (each tool round is a new
/// job) and a consumer must treat a decrease as a restart, not a gap. It is
/// what makes an honest tok/s readout possible at all: deltas are
/// `partial_text` diffs at the broker's poll cadence, so their count and
/// their byte length say nothing about how many TOKENS were produced.
///
/// The lane pair is the serving box's decode-lane contention as its
/// `/health` advertised it at probe time (see `LanesJson` in asset-ai):
/// stale by construction, a preference/context signal, never a reservation.
/// Absent when the box advertises no lanes — which, per that protocol,
/// means one lane; a consumer shows nothing rather than inventing "1/1".
///
/// Nothing here is a security boundary or a budget: decoding CLAMPS
/// implausible values and drops malformed ones instead of failing the page,
/// because a cosmetic counter must never be able to kill a live turn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ServingFacts {
    /// Cumulative tokens generated in the current provider round.
    pub gen_tokens: u32,
    /// Lanes generating on the serving box at probe time.
    pub lanes_active: Option<u32>,
    /// Lanes the serving box is configured for.
    pub slots_total: Option<u32>,
    /// Tokens generated inside the model's think block — reasoning the user
    /// never sees. Rises while the block is open.
    ///
    /// Without it a client cannot tell a slow box from a thinking one: the
    /// meter reads a stalled rate while the box decodes flat out, and the
    /// wait before ANY text appears is the whole block. With it the client
    /// can say "thinking · N" and, once `visible_tokens` arrives, quote the
    /// rate the person actually perceived alongside the rate the box hit.
    pub think_tokens: Option<u32>,
    /// Tokens generated after the think block closed. ABSENT while it is
    /// still open, which is exactly how a client knows it still is.
    pub visible_tokens: Option<u32>,
    /// Tokens this turn had to ingest at prefill. A warm conversation ingests
    /// a handful; a cold one ingests its whole history, and the difference is
    /// seconds the user feels and cannot otherwise attribute.
    pub prefix_ingested: Option<u32>,
    /// True when the serving box kept this conversation's cache and appended
    /// to it instead of re-reading the whole thing.
    pub prefix_resumed: Option<bool>,
}

/// Sanity ceilings for the presentation counters (clamped, never refused).
const MAX_GEN_TOKENS: u64 = 10_000_000;
const MAX_LANES: u64 = 1024;

impl ServingFacts {
    pub fn encode(&self) -> Value {
        let mut pairs: Vec<(&str, Value)> =
            vec![("gen_tokens", Value::Int(self.gen_tokens as i64))];
        if let Some(active) = self.lanes_active {
            pairs.push(("lanes_active", Value::Int(active as i64)));
        }
        if let Some(total) = self.slots_total {
            pairs.push(("slots_total", Value::Int(total as i64)));
        }
        if let Some(think) = self.think_tokens {
            pairs.push(("think_tokens", Value::Int(think as i64)));
        }
        if let Some(visible) = self.visible_tokens {
            pairs.push(("visible_tokens", Value::Int(visible as i64)));
        }
        if let Some(ingested) = self.prefix_ingested {
            pairs.push(("prefix_ingested", Value::Int(ingested as i64)));
        }
        if let Some(resumed) = self.prefix_resumed {
            pairs.push(("prefix_resumed", Value::Bool(resumed)));
        }
        json::obj(pairs)
    }

    /// Lenient by design (see the type doc): anything unreadable decodes to
    /// `None` and the stream carries on without a rate readout.
    pub fn decode(v: &Value) -> Option<ServingFacts> {
        if !matches!(v, Value::Obj(_)) {
            return None;
        }
        let lane = |key: &str| {
            v.get(key)
                .and_then(Value::as_u64)
                .map(|n| n.min(MAX_LANES) as u32)
        };
        // Same clamping law as the counters above: a cosmetic number must
        // never be able to kill a live turn, so implausible values are pinned
        // and unreadable ones simply go missing.
        let count = |key: &str| {
            v.get(key)
                .and_then(Value::as_u64)
                .map(|n| n.min(MAX_GEN_TOKENS) as u32)
        };
        Some(ServingFacts {
            gen_tokens: v.get("gen_tokens").and_then(Value::as_u64)?.min(MAX_GEN_TOKENS) as u32,
            lanes_active: lane("lanes_active"),
            slots_total: lane("slots_total"),
            think_tokens: count("think_tokens"),
            visible_tokens: count("visible_tokens"),
            prefix_ingested: count("prefix_ingested"),
            prefix_resumed: v.get("prefix_resumed").and_then(|b| match b {
                Value::Bool(value) => Some(*value),
                _ => None,
            }),
        })
    }
}

fn str_field(v: &Value, key: &'static str, max: usize) -> Result<String, &'static str> {
    let s = v.get(key).and_then(Value::as_str).ok_or(key)?;
    if s.len() > max {
        return Err(key);
    }
    Ok(s.to_string())
}
