//! The engine: one conversation that drives every connected service.
//!
//! Three parts, all free of `Cx` so they are tested with a scripted model
//! and no window:
//!
//! - [`registry`] — the connected services: issues endpoints, keeps the
//!   manifests and where each instance lives, pumps their up-frames;
//! - [`core`] — the conversation: builds the system prompt from the
//!   registry, runs the model, routes each tool call to the instance that
//!   owns it (risk gate, deadlines, dispositions), and keeps the
//!   [`crate::state::EngineState`] a panel draws;
//! - [`Model`] — the seam to whatever answers: the local engine through
//!   the hub, a cloud provider, or a scripted model in tests. Real models
//!   live behind the `engine` cargo feature (`models`); the core does not
//!   need them.
//!
//! A host (the aichat app, the Window overlay) wraps the core in a thin
//! `Cx` adapter: it calls `pump` on every event and redraws when the
//! state's generation moved.

pub mod core;
pub mod no_model;
pub mod registry;

#[cfg(feature = "engine")]
pub mod models;

pub use core::{
    EngineCore, EngineEvent, DOCTRINE, MAX_SUBSCRIPTIONS, MAX_SUBSCRIPTION_QUEUE,
    WAKE_INTERVAL_SECS,
};
pub use no_model::{NoModel, NoModelWithReason};
pub use registry::{RegistryUp, ServiceRegistry, MAX_INSTANCES};

/// One tool as the model is told about it: the canonical dotted name, a
/// description, and the argument schema as JSON text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub parameters: String,
}

/// What a model reports back, in order.
#[derive(Clone, Debug, PartialEq)]
pub enum ModelEvent {
    /// Weights streaming, prefix prefilling.
    Loading { phase: String, fraction: f64 },
    /// Ready to take a turn.
    Ready,
    /// A piece of the visible answer.
    Delta(String),
    /// A piece of the model's reasoning, when the provider shows it.
    Thinking(String),
    /// The model asks for a tool. `call_id` is the model's own id for the
    /// call; the engine answers with exactly one result per call. `args`
    /// is a JSON object as text.
    ToolCall { call_id: String, name: String, args: String },
    /// Tokens per second for the answer so far, when known.
    Rate(f32),
    /// The turn ended. `tool_calls` says how many results are now owed.
    TurnDone { tool_calls: usize },
    /// The turn (or the load) failed. The model is idle afterwards.
    Error(String),
}

/// The seam to whatever answers. Every method is non-blocking; work
/// happens on the model's own threads and surfaces through `poll`.
pub trait Model {
    /// A label for the chip: `Local · Qwen3.5 9B`, `Claude`.
    fn label(&self) -> String;

    /// Bind the system prompt and the tool table. Called before the first
    /// turn and again whenever the registry changes. A model that cannot
    /// rebind mid-conversation must say so with `Err` — the core then
    /// restarts the conversation rather than letting the table drift.
    fn configure(&mut self, system: &str, tools: &[ToolDefinition]) -> Result<(), String>;

    /// Start a turn. `dynamic_context` is the volatile per-turn state of
    /// the services (bounded); a stateless provider folds it into the
    /// system, a KV-holding one renders it inside the turn.
    fn send_user(&mut self, text: &str, dynamic_context: &str);

    /// Answer one tool call the model asked for.
    fn send_tool_result(&mut self, call_id: &str, text: &str, is_error: bool);

    /// Abort the in-flight turn, if any. The model stays usable.
    fn cancel(&mut self);

    /// Drop the conversation. The next `send_user` starts fresh.
    fn reset(&mut self);

    fn poll(&mut self) -> Vec<ModelEvent>;

    /// Whether `configure` may be called while a turn is in flight without
    /// losing it — a template-prompted model folds a new tool table into
    /// its next message; a native-tool API rebuilds its session. The
    /// engine only rebinds mid-turn when this is true.
    fn can_rebind_mid_turn(&self) -> bool {
        false
    }
}
