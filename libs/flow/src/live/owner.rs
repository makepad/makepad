//! What a live owner implements.
//!
//! Two halves. [`LiveOwner`] is an immutable factory: it turns its slice of
//! a plan into a prepared value on a worker thread, where it may allocate,
//! decode and compile. [`OwnerRuntime`] belongs to the host's thread and
//! hands prepared values to wherever the owner actually runs.
//!
//! Activation is two steps so that several owners change generation
//! coherently. `stage` is fallible and reserves everything the change needs
//! (queue space, retirement space); `commit` publishes what was staged and
//! cannot fail. The host stages every owner before it commits any, so a
//! refusal leaves every owner on its old generation. After a commit each
//! owner switches at its own next safe boundary and says so through
//! `active`: the generations agree, the instants do not, and the host
//! reports that skew instead of hiding it. If the execution context changes
//! between staging and admission, an owner retains its prior state and
//! reports `rejected`; the host prepares the newest revision again. This
//! eventual convergence is not synchronous rollback of other owners.

use super::plan::{Endpoint, LivePlanSpec, OwnerSlice, PlanError};
use super::value::{EndpointValue, ProbeValue, WriteRefusal};
use crate::document::ident::{Generation, InstanceId};
use std::any::Any;
use std::sync::Arc;

/// An owner-typed command for one admitted trigger endpoint: what a write
/// cannot say with a number, a toggle, a note or a pulse (a decoded track
/// for a deck, a grid for a world). The host validates the target exactly
/// as it validates a write and never looks inside `payload`; the owner
/// downcasts it with [`Self::take`] and refuses anything it does not know.
/// Every refusal hands the command back untouched, so ownership of a large
/// payload is never in doubt: the caller keeps it, retries the same target
/// on `Busy`, or drops it on its own thread.
pub struct OwnerCommand {
    /// The owner this command is for; a mismatch with the endpoint's owner
    /// is refused before any owner is called.
    pub owner: String,
    pub payload: Box<dyn Any + Send>,
}

impl OwnerCommand {
    pub fn new(owner: impl Into<String>, payload: impl Any + Send) -> Self {
        Self { owner: owner.into(), payload: Box::new(payload) }
    }

    /// The payload as `T`, or the whole command back when it is not one.
    pub fn take<T: Any + Send>(self) -> Result<Box<T>, Self> {
        let Self { owner, payload } = self;
        payload.downcast::<T>().map_err(|payload| Self { owner, payload })
    }

    pub fn is<T: Any>(&self) -> bool {
        self.payload.is::<T>()
    }
}

impl std::fmt::Debug for OwnerCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OwnerCommand").field("owner", &self.owner).finish_non_exhaustive()
    }
}

/// What an owner says about the resource behind one trigger endpoint, by
/// the tags the commands carried: what runs, what is on its way, the last
/// command that failed with why, and the last source command that was
/// accepted. Tags are the caller's; the host and the owner only carry them.
/// A newer pending tag is never cleared by an older acknowledgement and
/// `active` never regresses.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct ResourceState {
    /// Identity of the currently loaded resource.
    pub active: Option<u64>,
    pub pending: Option<u64>,
    pub error: Option<(u64, String)>,
    /// Last successfully accepted source command tag, including eject.
    pub accepted: Option<u64>,
}

/// One life of one instance in a host. An instance id that is stopped and
/// applied again gets a new incarnation, so nothing addressed to the old
/// life (a late prepare, a queued command) can reach the new one.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SessionKey {
    pub instance: InstanceId,
    pub incarnation: u64,
}

/// What an owner's running generation looks like to the next prepare: the
/// generation and an owner-defined layout (what can carry its state over).
#[derive(Clone)]
pub struct BaseInfo {
    pub generation: Generation,
    pub layout: Arc<dyn Any + Send + Sync>,
}

pub struct PrepareRequest<'a> {
    pub session: &'a SessionKey,
    pub generation: Generation,
    pub plan: &'a LivePlanSpec,
    pub slice: &'a OwnerSlice,
    /// What the owner runs now for this session, if anything. A prepared
    /// slice built against it is only valid while it is still what runs.
    pub base: Option<BaseInfo>,
    /// The owner runtime's context (a device format, a surface size).
    /// It must include an epoch/fingerprint when it can change during a
    /// prepare. The runtime rechecks this at staging and execution admission.
    pub context: Arc<dyn Any + Send + Sync>,
}

pub trait LiveOwner: Send + Sync {
    fn key(&self) -> &str;
    /// Worker thread. No device, no GPU, no UI. Validate supported type rates,
    /// carriers and domain lowerings before allocating execution state; the
    /// generic partition only validates connectivity and owner boundaries.
    fn prepare(&self, request: PrepareRequest<'_>) -> Result<Box<dyn PreparedSlice>, PlanError>;
}

pub trait PreparedSlice: Send {
    /// The generation this slice was prepared against.
    fn base(&self) -> Option<Generation>;
    fn into_any(self: Box<Self>) -> Box<dyn Any + Send>;
}

/// The runtime cannot say yet what a prepare needs (a device that has not
/// reported its format). The host asks again on a later pump.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NotReady;

#[derive(Clone, Debug, PartialEq)]
pub enum StageRefusal {
    /// No capacity right now; the slice comes back untouched and the host
    /// retries on a later pump.
    Busy,
    /// Another generation became the base since this was prepared.
    StaleBase,
    Refused(String),
}

pub trait OwnerRuntime {
    fn key(&self) -> &str;
    fn context(&mut self) -> Result<Arc<dyn Any + Send + Sync>, NotReady>;
    fn base(&self, session: &SessionKey) -> Option<BaseInfo>;
    /// Reserve and hold. Nothing becomes audible, visible or simulated.
    fn stage(
        &mut self,
        session: &SessionKey,
        generation: Generation,
        prepared: Box<dyn PreparedSlice>,
    ) -> Result<(), (StageRefusal, Box<dyn PreparedSlice>)>;
    /// Publish the staged generation. Infallible: `stage` reserved for it.
    fn commit(&mut self, session: &SessionKey, generation: Generation);
    /// Forget a staged generation that will not be committed.
    fn abort(&mut self, session: &SessionKey, generation: Generation);
    /// The generation that is acknowledged as running where the owner runs.
    fn active(&self, session: &SessionKey) -> Option<Generation>;
    /// A committed change refused at the execution boundary (for example a
    /// device format changed after staging). Keep the prior active state and
    /// report once; the host prepares its newest request again.
    fn rejected(&mut self, _session: &SessionKey) -> Option<(Generation, String)> {
        None
    }
    /// Reserve retirement before any owner commits a revision that omits
    /// this owner. Implementations with live state must reserve a reliable
    /// stop command; the default only admits an already stopped owner.
    fn stage_stop(&mut self, session: &SessionKey, _generation: Generation) -> Result<(), StageRefusal> {
        if self.stopped(session) { Ok(()) } else { Err(StageRefusal::Busy) }
    }
    fn commit_stop(&mut self, session: &SessionKey, _generation: Generation) {
        self.stop(session);
    }
    fn abort_stop(&mut self, _session: &SessionKey, _generation: Generation) {}
    /// `generation` is the plan the endpoint was resolved in. The applied
    /// value (after clamping and snapping) comes back for a number.
    fn write(
        &mut self,
        session: &SessionKey,
        generation: Generation,
        endpoint: &Endpoint,
        value: EndpointValue,
    ) -> Result<Option<f64>, WriteRefusal>;
    fn probe(&self, session: &SessionKey, generation: Generation, endpoint: &Endpoint) -> Option<ProbeValue>;
    /// Optional owner-defined observation of a node in this exact plan. This
    /// does not create a graph port, connection, or writable endpoint.
    fn probe_node(&self, _session: &SessionKey, _generation: Generation, _node: &str, _name: &str) -> Option<ProbeValue> {
        None
    }
    /// An owner-typed command for a trigger endpoint the host admitted for
    /// `generation`. Admission is bounded and non-blocking: refuse `Busy`
    /// when the owner's queue is full and hand the command back untouched,
    /// so the caller retries the exact same target on a later pump. Any
    /// payload the owner does not know is refused with `WrongKind`. A
    /// payload the owner sends to a realtime thread must come back to this
    /// thread before its last reference is dropped; a large payload never
    /// makes its final drop on a realtime callback. The default refuses.
    fn command(
        &mut self,
        _session: &SessionKey,
        _generation: Generation,
        _endpoint: &Endpoint,
        command: OwnerCommand,
    ) -> Result<(), (WriteRefusal, OwnerCommand)> {
        Err((WriteRefusal::WrongKind { expected: "nothing: this owner takes no command" }, command))
    }
    /// The resource behind a trigger endpoint, if the owner keeps one.
    fn resource(&self, _session: &SessionKey, _generation: Generation, _endpoint: &Endpoint) -> Option<ResourceState> {
        None
    }
    /// The target a control currently has (what a control shows on mount).
    fn control(&self, session: &SessionKey, generation: Generation, endpoint: &Endpoint) -> Option<f64>;
    /// Begin retiring everything of this session. Acknowledged by `stopped`.
    fn stop(&mut self, session: &SessionKey);
    fn stopped(&self, session: &SessionKey) -> bool;
    /// Once per host pump: take what came back from where the owner runs
    /// and drop it here. Returns whether the context changed (a device
    /// format), in which case the host prepares the running sessions again.
    fn pump(&mut self) -> bool;
}
