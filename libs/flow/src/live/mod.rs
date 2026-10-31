//! The live partition of a flow revision.
//!
//! A revision is one graph. Its nodes belong to owners: `core.jobs` and the
//! registered job owners run as finite jobs on the scheduler; the nodes of a
//! *live* owner (audio, frames, a simulated world) run continuously, each in
//! the place its owner runs (a device callback, the draw thread, a worker).
//! Nothing here knows what those places are.
//!
//! - [`plan`] partitions a lowered revision by owner, resolves wires through
//!   component boundaries, refuses the crossings no owner can carry and lists
//!   the endpoints a running plan admits.
//! - [`owner`] is what an owner implements: prepare a slice off every hot
//!   thread, stage it (reserve, fallibly), commit it (publish, infallibly).
//! - [`host`] drives instances through prepare, stage, commit and retire,
//!   keeps the last good generation when a revision fails, and routes typed
//!   endpoint writes. It never blocks its thread.
//! - [`spsc`] is the bounded single-producer single-consumer ring owners use
//!   to reach a realtime thread, with each end owned by one side.
//!
//! Continuous media never enters the value store and never crosses HTTP.

pub mod owner;
pub mod plan;
pub mod spsc;
pub mod value;

#[cfg(not(target_arch = "wasm32"))]
pub mod host;

pub use owner::{
    BaseInfo, LiveOwner, NotReady, OwnerCommand, OwnerRuntime, PrepareRequest, PreparedSlice, ResourceState, SessionKey,
    StageRefusal,
};
pub use plan::{partition, ConnectionHop, Endpoint, EndpointId, EndpointKind, LiveInput, LiveNode, LivePlanSpec, LiveSource, OwnerSlice, PlanError};
pub use value::{AudioProbe, EndpointValue, ProbeValue, WriteRefusal, AUDIO_PROBE_BINS};

/// Whether changing between two evaluated root graphs only changes editor
/// layout or source locations. This is deliberately conservative: all other
/// graph fields, authored code, component graphs and module pins must agree.
///
/// Live owners consume evaluated node parameters and connections, not the
/// root's layout fields. This does not claim that arbitrary job/Splash code
/// which explicitly reads a captured node's layout cannot observe the edit.
pub fn same_root_layout_execution(before: &crate::Graph, after: &crate::Graph) -> bool {
    if before.nodes.len() != after.nodes.len() {
        return false;
    }
    // Compare the complete schema so a newly added field stays significant
    // by default. Only these known editor fields are substituted.
    let mut projection = before.clone();
    projection.revision = after.revision;
    projection.warnings.clone_from(&after.warnings);
    for (node, layout) in projection.nodes.iter_mut().zip(&after.nodes) {
        node.at = layout.at;
        node.size = layout.size;
        node.flip = layout.flip;
        node.loc.clone_from(&layout.loc);
    }
    projection == *after
}

#[cfg(not(target_arch = "wasm32"))]
pub use host::{ApplyRevision, LiveEvent, LiveHost, LiveLimits, LiveStatus, WriteTarget};
