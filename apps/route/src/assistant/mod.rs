//! The assistant seam: what the panel's prompt line talks to, chosen by
//! feature so native and every other profile compile the same widget
//! tree. The UI is deliberately outside this module.
//!
//! `native` links the brain (assistant/native.rs): the local Qwen
//! dispatcher, the cloud escalation agent, the voice gate and the mic,
//! spoken replies, image search. It reports what happened as
//! [`AssistantEvent`]s and the view keeps the transcript, runs the tools
//! and answers back — the brain never reaches into the map or the trip.
//!
//! Without it (assistant/unavailable.rs) nothing is linked: the prompt
//! line says so, `/tool` commands still run against the local registry,
//! and a host's assistant reaches the same tools over the bus.

use makepad_widgets::{Cx, WidgetRef};

#[cfg(feature = "native")]
mod native;
#[cfg(not(feature = "native"))]
mod unavailable;

#[cfg(feature = "native")]
pub use native::AssistantService;
#[cfg(not(feature = "native"))]
pub use unavailable::AssistantService;

pub const UNAVAILABLE_REPLY: &str = "assistant is not available in this build";

pub trait AssistantController {
    fn configure_ui(&self, cx: &mut Cx, ui: &WidgetRef);
    fn unavailable_reply(&self, prompt: &str) -> Option<&'static str>;
}

/// What the brain reports to the view, in the order it happened.
#[derive(Debug, PartialEq)]
pub enum AssistantEvent {
    /// A line for the transcript: warnings, gate verdicts, cloud previews.
    Info(String),
    /// Streamed reply text for the pending assistant line.
    Text(String),
    /// The dispatcher asked for a tool. The view runs it (after
    /// [`AssistantService::begin_tool`] lets it) and answers with
    /// `send_tool_result`.
    ToolRequest {
        tool_use_id: String,
        name: String,
        input: String,
    },
    /// The turn ended; the status line to show.
    TurnComplete { status: String },
    /// The mic endpointed an utterance; shown raw, then judged by the gate
    /// once the view hands it back with the recent dialog.
    Transcript(String),
    /// The voice gate forwarded an utterance: a prompt to send.
    Directed(String),
    /// A thumbnail for card `slot` of the image strip.
    Thumb { slot: usize, data: Vec<u8> },
    /// Image search finished; the digest goes in the transcript. Any
    /// pending tool result was already answered.
    SearchDone { digest: String, is_error: bool },
    /// The status line changed (models loading, voice on or off).
    Status(String),
}

/// What sending a prompt did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptOutcome {
    Sent,
    /// No dispatcher in this build (or it was switched off).
    NoAgent,
}

/// The brain's answer to a tool request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BeginTool {
    /// Run it against the registry and answer with `send_tool_result`.
    Execute,
    /// The brain answered itself (a loop breaker, an asynchronous tool it
    /// owns): nothing to run.
    Handled,
}
