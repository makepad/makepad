//! The embedding runtime.
//!
//! [`store`] is the one implementation of flow state semantics, shared with
//! the HTTP host. [`engine`] wraps it in the bounded nonblocking contract a
//! host embeds; [`work`] is how the host supplies threads; [`lanes`] executes
//! runs; [`triple`] publishes snapshots; [`batches`] is the store's batch
//! bookkeeping.

pub mod batches;
#[cfg(feature = "live")]
pub mod controller;
pub mod engine;
pub(crate) mod lanes;
pub mod store;
pub use crate::triple;
pub mod work;

pub use engine::{
    Conflict, DefinitionSummary, Engine, EngineCommand, EngineConfig, EngineError, EngineHandle,
    EngineLimits, EngineQuery, EngineSnapshot, EngineStartError, HostServices, InputDtos, JournalEntry,
    JournalPoll, LaneGap, Observation, Refusal, Reply, ReplyPayload, RequestId, Submit, Transition,
    TransitionKind, TransitionPoll, READY_REQUEST,
};
#[cfg(feature = "live")]
pub use controller::{ControllerEvent, FlowController, Pump};
pub use store::{AcceptedRevision, InstanceExecution, LiveOutputs, StoreEvent};
pub use work::{HostWork, Job, JobLane, JobRefused, SpawnRefused, StdHostWork, WorkBudget};
