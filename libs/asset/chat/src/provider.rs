//! The explicit-provider seam. A provider owns exactly one conversation
//! lane (fleet Qwen, OpenAI Responses, or xAI Grok Responses) and exposes
//! the same tiny, headless, poll-driven surface — no UI Cx, no async
//! runtime, no callbacks. The session engine above is provider-agnostic.
//! Fleet Qwen still streams text and uses the textual tool marker;
//! OpenAI/Grok emit a native [`ProviderEvent::FunctionCall`] and resume
//! through [`ChatProvider::continue_function`].
//!
//! Contract:
//! - One in-flight turn per provider. `begin_turn` while active is an error.
//! - `poll` is non-blocking and may return nothing.
//! - `cancel` is best-effort immediate; after it, the provider is idle and
//!   late events are ignored. The worker is never joined.
//! - `availability` is a live, honest probe — it is what the UI shows, and
//!   what the session engine checks before every send. There is no
//!   fallback path anywhere below this trait.

use crate::wire::{ChatMessage, ProviderAvailability, ProviderKind};

/// Everything a provider needs for one turn. `system` carries the tool
/// protocol, live capabilities and attachment bindings; `messages` is the
/// bounded conversation so far (user/assistant/tool roles).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TurnInput {
    pub system: String,
    pub messages: Vec<ChatMessage>,
    /// When false, native providers omit the function list. Used by the
    /// broker's `llm.consult` one-shot so the external model cannot nest
    /// another tool-capable session.
    pub tools_enabled: bool,
}

impl TurnInput {
    pub fn new(system: impl Into<String>, messages: Vec<ChatMessage>) -> TurnInput {
        TurnInput { system: system.into(), messages, tools_enabled: true }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProviderEvent {
    /// Streaming assistant text, in order. Native Responses providers emit
    /// one `Delta` with the full visible text before `FunctionCall` or
    /// `Done`.
    Delta(String),
    /// Job/load progress (prefill, token count, queued). Not assistant text.
    Status { note: String, permille: u16 },
    /// A native function call. `arguments` is the raw JSON string; the
    /// session parses it to an object and remains the security boundary.
    FunctionCall { call_id: String, name: String, arguments: String },
    /// The turn finished; `text` is the full assistant reply (providers
    /// that only streamed deltas pass the accumulated text).
    Done { text: String },
    /// The turn failed. The provider is idle afterwards.
    Error(String),
}

pub trait ChatProvider {
    fn kind(&self) -> ProviderKind;

    /// Live probe. Called before every send and whenever the UI wants a
    /// status row; must be cheap enough for that.
    fn availability(&mut self) -> ProviderAvailability;

    /// Start one turn. Err is a bounded, user-showable reason.
    fn begin_turn(&mut self, input: &TurnInput) -> Result<(), String>;

    /// Drain pending events. Non-blocking.
    fn poll(&mut self) -> Vec<ProviderEvent>;

    /// Abort the in-flight turn, if any.
    fn cancel(&mut self);

    /// Resume after a native function call with the exact provider
    /// `call_id` and the encoded tool outcome. Providers that do not
    /// implement native tools leave the default (an error) in place.
    fn continue_function(&mut self, call_id: &str, output: &str) -> Result<(), String> {
        let _ = (call_id, output);
        Err("native tool continuation is not supported by this provider".to_string())
    }
}
