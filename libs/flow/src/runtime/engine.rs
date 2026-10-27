//! The embedding engine.
//!
//! [`Engine::start`] hands a [`FlowStore`] to one long-lived worker the host
//! admitted, plus run lanes that execute runs, and returns an
//! [`EngineHandle`] at once. Everything the handle does is nonblocking:
//!
//! - **submit** puts a command on a bounded queue. It is accepted only if a
//!   reply slot could be reserved for it, so its terminal [`Reply`] can never
//!   be dropped or stuck behind other traffic. A refused command comes back
//!   to the caller.
//! - **replies** carry exactly one terminal result per accepted command
//!   (`Ready` and `Stopped` included). That holds through a stop, a dropped
//!   evaluation job and a dead worker: the worker answers everything it
//!   still holds before it leaves, and the handle answers `Stopped` itself
//!   for whatever a vanished worker left outstanding.
//! - **transitions** carry lifecycle (flow, instance and run events) on their
//!   own bounded lane. When the consumer falls behind, the engine drops the
//!   oldest intermediate run transitions first and, if that is not enough,
//!   the oldest transitions of any kind. Every drop is visible: sequence
//!   numbers skip, and the snapshot names the highest dropped sequence so a
//!   trailing gap is seen too. Recovery is one step: `poll_transitions`
//!   hands back the snapshot to resynchronise from and moves its cursor to
//!   exactly the sequence that snapshot reflects. The worker publishes a
//!   snapshot before it sends the transitions that snapshot covers, so the
//!   recovery snapshot is never older than anything already delivered.
//! - **observations** carry progress and text deltas and are simply dropped
//!   when the consumer is slow.
//! - **journal** is the one ordered stream of everything the store did,
//!   progress and deltas included, for a host that republishes events (the
//!   HTTP planes). Every store event takes its sequence number before the
//!   lanes split, so the three lanes agree on order. The journal lane is
//!   bounded and never holds the engine back: an entry that does not fit is
//!   dropped, the drop is visible as a skipped sequence and in the snapshot's
//!   watermark, and `poll_journal` hands back the snapshot to recover from.
//!   It is off unless the host asks for it.
//!
//! A reply is sent only after the events its command caused were sent (or
//! counted as dropped) and after a snapshot that reflects the command was
//! published. A host that has seen a reply can rely on both.
//! - **snapshot** is the newest summary the worker published through a
//!   triple buffer.
//!
//! The handle takes no lock another thread can hold and never waits on a
//! channel, so a UI thread may own it. It is `Send` but not `Clone`: one
//! owner consumes the lanes. Every poll takes a budget, so a lane that
//! refills as fast as it drains cannot hold the caller.
//!
//! Memory is bounded by admission: definitions, instances, queued runs per
//! instance, finished run rows, input value size, in-flight runs (lanes ×
//! lane capacity), pending transitions and outstanding replies all have
//! limits in [`EngineConfig`]; a request past a limit is answered `Busy`.
//!
//! What this engine does not do yet is said explicitly: there is no control
//! partition, so the admitted control-endpoint table is empty and every
//! `SetControl` is refused with `UnknownEndpoint`; prepared revisions and
//! placement are later slices. The worker and its lanes exist on native
//! targets only.

use super::batches::CreateBatchOutcome;
use super::lanes::{lane_loop, LaneContext, LaneEvent, LaneStart, RunTag};
use super::store::{
    ClearInstanceOutcome, CreateInstanceOutcome, FlowStore, FlowStoreConfig, InstanceExecution, LiveOutputs,
    RunControl, RunDriver, RunRefused, SetInputsOutcome, SourceLayout, StartRunOutcome, StoreEvent,
};
use super::triple::{triple_buffer, Reader, Writer};
use super::work::{HostWork, JobLane, SpawnRefused};
use crate::document::closure::Closure;
use crate::document::ident::ParamAddr;
use crate::document::interface::FlowInterface;
use crate::engine::results::ResultStore;
use crate::engine::{self, NetPolicy, RunEvent, RunId, RunInput, RunState, Seams, EVENT_META_MAX};
use crate::instance::{InstanceId, Owner};
use crate::values::{PortValue, Value, ValueStore};
use crate::{
    graph, CreateBatchRequest, CreateBatchResponse, EvalError, FlowResponse, FlowSummary, Graph,
    InputRefs, InputValueDto, InstanceRow, RunRowDto,
};
use std::collections::{HashMap, HashSet, VecDeque};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, sync_channel, Receiver, SyncSender, TryRecvError, TrySendError};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub struct EngineConfig {
    /// Registered document types and job owners; fixed for this engine's lifetime.
    pub registry: Arc<crate::document::Registry>,
    pub executors: Arc<engine::providers::ExecutorRegistry>,
    /// Where values spill once the RAM budget is exceeded.
    pub values_dir: PathBuf,
    pub values_ram_budget: usize,
    pub value_ttl: Duration,
    pub revision_ring: usize,
    pub instance_ttl: Duration,
    /// Identity the engine stamps on provider leases: (host id, start epoch).
    pub origin: (String, u64),
    pub seams: Seams,
    pub net: NetPolicy,
    /// Commands queued but not yet taken by the worker.
    pub command_capacity: usize,
    /// Replies outstanding: accepted commands whose reply was not polled yet.
    pub reply_capacity: usize,
    pub transition_capacity: usize,
    pub observation_capacity: usize,
    /// Long-lived run lanes (at least one).
    pub run_lanes: usize,
    /// Runs one lane may hold at a time.
    pub lane_run_capacity: usize,
    /// How long `stop` waits for cancelled runs before it gives up on them.
    pub stop_grace: Duration,
    pub limits: EngineLimits,
    /// Which finished runs publish an instance's live outputs. This is the
    /// engine's policy, not a view's: `CurrentRevision` unless a host is
    /// explicitly configured as a legacy server.
    pub live_outputs: LiveOutputs,
    /// Entries the ordered journal lane holds. Zero leaves the lane off.
    pub journal_capacity: usize,
    /// Period of the janitor sweep (values, finished runs, idle instances).
    pub sweep_every: Duration,
    /// Most payload bytes each event lane (run lanes to worker, transitions,
    /// observations, journal) may hold at a time. Values in events are
    /// shared `Arc` bytes but count in full: a queued event keeps them alive.
    /// The budget bounds what queues behind an event, never whether one can
    /// travel: an empty lane takes any admitted event. An event that finds
    /// the lane past its budget is dropped and reported as any other drop
    /// (engine-side lanes) or held back until the lane drains (run lanes).
    pub lane_max_bytes: usize,
    /// Most value bytes one run event, and one run's queued events, may
    /// carry: the media admission budget. A node whose outputs exceed it
    /// fails explicitly; nothing is trimmed. A host scales this with the
    /// machine.
    pub run_media_max_bytes: usize,
    /// Whether snapshots carry the instance and run rows. A host that reads
    /// rows through queries turns this off: a snapshot is rebuilt on every
    /// change, and copying every row each time is what it then saves. The
    /// cursors and drop watermarks are always there.
    pub snapshot_rows: bool,
}

/// Admission limits. A request past one is answered `Busy` (or `Invalid`
/// for an oversized value); nothing is accepted and then dropped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EngineLimits {
    pub max_definitions: usize,
    pub max_instances: usize,
    /// Runs one instance may hold queued behind its concurrency.
    pub max_queued_runs: usize,
    /// Finished run rows kept for the snapshot; the oldest go first.
    pub max_finished_runs: usize,
    /// Largest single input value, on every path that sets one: a value the
    /// host built, a wire literal, or a stored digest.
    pub max_input_bytes: usize,
    /// Most run rows batches may own at once. A batch keeps its rows until it
    /// is cleared, so this bounds that retention; a batch past it is refused
    /// before any of its runs start.
    pub max_batch_runs: usize,
}

impl Default for EngineLimits {
    fn default() -> Self {
        Self {
            max_definitions: 256,
            max_instances: 1024,
            max_queued_runs: 16,
            max_finished_runs: 256,
            max_input_bytes: 64 * 1024 * 1024,
            max_batch_runs: 1024,
        }
    }
}

impl EngineConfig {
    pub fn new(values_dir: PathBuf) -> Self {
        Self {
            registry: Arc::default(),
            executors: Arc::default(),
            values_dir,
            values_ram_budget: 256 * 1024 * 1024,
            value_ttl: Duration::from_secs(60 * 60),
            revision_ring: 16,
            instance_ttl: Duration::from_secs(60 * 60),
            origin: ("embedded".to_string(), engine::unix_ms()),
            seams: Seams::unconfigured(),
            net: NetPolicy::default(),
            command_capacity: 64,
            reply_capacity: 64,
            transition_capacity: 1024,
            observation_capacity: 1024,
            run_lanes: 1,
            lane_run_capacity: 16,
            stop_grace: Duration::from_secs(5),
            limits: EngineLimits::default(),
            live_outputs: LiveOutputs::CurrentRevision,
            journal_capacity: 0,
            sweep_every: Duration::from_secs(30),
            lane_max_bytes: 64 * 1024 * 1024,
            run_media_max_bytes: 1024 * 1024 * 1024,
            snapshot_rows: true,
        }
    }
}

pub struct HostServices {
    pub work: Arc<dyn HostWork>,
    pub results: Option<Arc<dyn ResultStore>>,
    pub log: Arc<dyn Fn(&str) + Send + Sync>,
    /// Called by the engine worker after it sent replies or events, so a
    /// host that parks can wake its poll. It must not block.
    pub wake: Option<Arc<dyn Fn() + Send + Sync>>,
}

/// Inputs in their wire form: literals, or digests of stored values.
pub type InputDtos = HashMap<String, HashMap<String, InputValueDto>>;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RequestId(pub u64);

/// The reply slot of the engine's own `Ready`.
pub const READY_REQUEST: RequestId = RequestId(0);

#[derive(Clone, Debug)]
pub enum EngineCommand {
    /// Evaluate `source` (on the host's job pool) and make it the definition
    /// `name`. `file_name` names the source in errors and in later runs.
    /// Evaluate `source` with `closure` (`None`: the definition's current
    /// closure, an ordinary root edit) and install it. `ephemeral` marks a
    /// session-owned definition that has no file of the host's.
    LoadDefinition {
        name: String,
        source: String,
        file_name: String,
        closure: Option<Arc<Closure>>,
        ephemeral: bool,
    },
    /// Record that `name`'s source could not be taken at all (a file past
    /// the size limit). The last valid graph stays executable.
    SetLoadError { name: String, error: EvalError },
    RemoveDefinition { name: String },
    /// Create the instance of every `autostart` flow.
    Autostart,
    /// `inputs` are applied with the creation, all or nothing. An embedding
    /// app uses `Owner::Application` for explicit lifetime: idle expiry
    /// cannot delete that instance. `pinned` only selects revision policy.
    Instantiate { flow: String, label: Option<String>, pinned: bool, owner: Owner, inputs: InputDtos },
    /// `(node, port, value)` triples. Answers a waiting Ask, or debounces a
    /// run on a `trigger: @input` flow, exactly as the HTTP plane does.
    SetInputs { instance: InstanceId, inputs: Vec<(String, String, PortValue)>, actor: String },
    /// `SetInputs` in wire form. Digests are resolved against the value
    /// store and the whole batch is validated before anything changes.
    SetInputDtos { instance: InstanceId, inputs: InputDtos, actor: String },
    /// Forget generated state, keep inputs. Refused while a run is in flight.
    ClearInstance { instance: InstanceId },
    StartRun { instance: InstanceId, outputs: Option<Vec<String>> },
    /// A run that belongs to a batch also retires its slice's instance.
    CancelRun { run: RunId },
    DeleteInstance { instance: InstanceId },
    /// Create `parallel` pinned instances, set their inputs and start their
    /// runs; any failure rolls the whole batch back.
    CreateBatch { flow: String, request: CreateBatchRequest },
    CancelBatch { batch: String },
    ClearBatch { batch: String },
    ReadValue { digest: [u8; 32] },
    /// Recover a graph value without changing its type when equal bytes
    /// are also stored under a different registered schema.
    ReadTypedValue { digest: [u8; 32], ty: crate::PortType },
    PutValue { value: Value },
    /// A read, answered by the worker in its turn like any command: it
    /// reflects every command the worker has *applied* before it. A
    /// `LoadDefinition` is applied when its evaluation completes, not when it
    /// is accepted, so a read that must see a load waits for that load's
    /// reply first.
    Query(EngineQuery),
    /// A control-rate parameter write. Only endpoints a prepared plan
    /// admitted can be written; this engine has no control partition yet, so
    /// the table is empty and the write is refused at `submit`.
    SetControl { addr: ParamAddr, value: f64 },
    Stop,
}

#[derive(Clone, Debug)]
pub enum EngineQuery {
    Definitions,
    Definition { name: String },
    /// The source `name` had at `revision`, while its revision ring holds it.
    DefinitionSource { name: String, revision: u64 },
    Instances { flow: Option<String>, waiting_only: bool },
    Instance { instance: InstanceId },
    /// What the instance executes: the accepted revision it is at (its pin,
    /// or the newest accepted one it follows), with the source, closure and
    /// graph shared as the store keeps them. `None` when the instance is
    /// unknown or its revision is no longer kept; never the current graph
    /// in its place.
    InstanceExecution { instance: InstanceId },
    Runs { instance: Option<InstanceId> },
    Run { run: RunId },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The control endpoint is not in the admitted table.
    UnknownEndpoint,
}

pub enum Submit {
    Accepted(RequestId),
    /// The command queue is full or no reply slot is free. Retry later.
    Full(EngineCommand),
    Refused(EngineCommand, Refusal),
    /// The engine stopped.
    Closed(EngineCommand),
}

#[derive(Clone, Debug, PartialEq)]
pub enum EngineError {
    /// A bounded resource was full; nothing happened.
    Busy(String),
    NotFound(String),
    Invalid(String),
    /// The request is well formed but the state it addresses cannot take it.
    Conflict(Conflict),
    /// A newer load of the same definition replaced this one.
    Superseded,
    /// The engine was stopping when it reached this command.
    Stopping,
    /// The engine is gone; the command was accepted but never answered.
    Stopped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Conflict {
    /// The flow has no valid graph (at the instance's revision).
    FlowInvalid,
    AskNotWaiting,
    ConcurrencyZero,
    /// The instance has a run in flight or queued.
    InstanceBusy,
}

impl std::fmt::Display for Conflict {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::FlowInvalid => "the flow has no valid graph",
            Self::AskNotWaiting => "that Ask node is not waiting",
            Self::ConcurrencyZero => "the flow's concurrency is 0",
            Self::InstanceBusy => "the instance has a run in flight",
        })
    }
}

impl std::fmt::Display for EngineError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Busy(what) => write!(formatter, "busy: {what}"),
            Self::NotFound(what) => write!(formatter, "not found: {what}"),
            Self::Invalid(what) => write!(formatter, "invalid: {what}"),
            Self::Conflict(what) => write!(formatter, "conflict: {what}"),
            Self::Superseded => write!(formatter, "superseded by a newer load"),
            Self::Stopping => write!(formatter, "the engine is stopping"),
            Self::Stopped => write!(formatter, "the engine stopped before answering"),
        }
    }
}

#[derive(Clone, Debug)]
pub enum ReplyPayload {
    Ready,
    /// The engine stopped. `abandoned_runs` is how many runs were still in
    /// flight when the stop grace ran out; zero means every run finished.
    Stopped { abandoned_runs: usize },
    /// The definition's revision after the load; `error` is the evaluation
    /// error when the source did not evaluate (the last valid graph stays
    /// executable). `interface` describes the executable graph, if any.
    Definition {
        name: String,
        revision: u64,
        error: Option<EvalError>,
        interface: Option<FlowInterface>,
        /// The executable graph after the load, if any.
        graph: Option<Graph>,
    },
    LoadErrorSet,
    Removed(bool),
    Autostarted,
    Instance(InstanceId),
    /// What each written port now holds, one and ordered ports apart.
    InputsSet(InputRefs),
    Cleared,
    RunStarted { run: RunId, queued: u64 },
    Cancelled(bool),
    Deleted(bool),
    Batch(CreateBatchResponse),
    /// How many slices the batch had, or `None` for an unknown batch.
    BatchRuns(Option<u64>),
    Value(Option<Value>),
    ValueStored([u8; 32]),
    Definitions(Vec<FlowSummary>),
    /// Shared and immutable: built once per definition revision.
    DefinitionDetail(Option<Arc<FlowResponse>>),
    /// A kept revision's source and the closure it evaluated with.
    DefinitionSource(Option<(String, Arc<Closure>)>),
    Instances(Vec<InstanceRow>),
    InstanceRow(Option<InstanceRow>),
    /// Shared and immutable: the instance's accepted revision, or `None`.
    InstanceExecution(Option<Arc<InstanceExecution>>),
    Runs(Vec<RunRowDto>),
    RunRow(Option<RunRowDto>),
}

#[derive(Clone, Debug)]
pub struct Reply {
    pub id: RequestId,
    pub result: Result<ReplyPayload, EngineError>,
}

#[derive(Clone, Debug)]
pub enum TransitionKind {
    FlowChanged { name: String, revision: u64, canonical: bool },
    FlowError { name: String, error: EvalError },
    FlowRemoved { name: String },
    InstanceCreated { instance: InstanceId, flow: String },
    InstanceRemoved { instance: InstanceId, flow: String },
    InstanceInputs { instance: InstanceId, flow: String },
    InstanceCleared { instance: InstanceId, flow: String },
    Run { run: RunId, instance: InstanceId, flow: String, event: RunEvent },
}

impl TransitionKind {
    /// Intermediate run transitions are the first to go under pressure.
    fn droppable(&self) -> bool {
        matches!(
            self,
            Self::Run { event, .. } if !matches!(event, RunEvent::RunFinished { .. })
        )
    }
}

#[derive(Clone, Debug)]
pub struct Transition {
    pub seq: u64,
    /// Position in the engine's one ordered event stream.
    pub event_seq: u64,
    /// Payload bytes, counted in the lane's byte budget until polled.
    pub bytes: usize,
    pub kind: TransitionKind,
}

/// Progress and text deltas: useful live, worthless late.
#[derive(Clone, Debug)]
pub struct Observation {
    pub seq: u64,
    /// Position in the engine's one ordered event stream.
    pub event_seq: u64,
    /// Payload bytes, counted in the lane's byte budget until polled.
    pub bytes: usize,
    pub run: RunId,
    pub instance: InstanceId,
    pub event: RunEvent,
}

/// What one `poll_transitions` call found.
#[derive(Clone, Debug)]
pub enum TransitionPoll {
    /// Every transition since the last poll is in `out`, in order.
    InOrder,
    /// Transitions were dropped. `snapshot` is the state to resynchronise
    /// from; the cursor now stands at `snapshot.transition_seq`, nothing
    /// that snapshot covers was (or will be) handed to the caller, and what
    /// arrives afterwards continues from it.
    Resync { dropped: u64, snapshot: Arc<EngineSnapshot> },
}

/// One entry of the ordered journal: a store event with its position in the
/// engine's event stream. Sequences are dense; a skipped one was dropped.
#[derive(Clone, Debug)]
pub struct JournalEntry {
    pub seq: u64,
    /// Payload bytes, counted in the lane's byte budget until polled.
    pub bytes: usize,
    pub event: StoreEvent,
}

/// What one `poll_journal` call found.
#[derive(Clone, Debug)]
pub enum JournalPoll {
    /// Every entry since the last poll is in `out`, in order.
    InOrder,
    /// Entries were dropped, so the stream is resynchronised in one step,
    /// exactly as the transition lane does: `snapshot` is the state to
    /// recover from, `cursor` is its sequence, and nothing at or before
    /// `cursor` is (or will be) handed to the caller, this call's entries
    /// included. What arrives afterwards continues from it. `dropped` counts
    /// the sequences lost in transport.
    Gap { dropped: u64, cursor: u64, snapshot: Arc<EngineSnapshot> },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaneGap {
    None,
    /// This many entries were dropped before the ones just returned. Read
    /// the snapshot for the current state.
    Dropped(u64),
}

#[derive(Clone, Debug)]
pub struct DefinitionSummary {
    pub name: String,
    pub revision: u64,
    pub valid: bool,
    pub error: Option<EvalError>,
}

#[derive(Clone, Debug, Default)]
pub struct EngineSnapshot {
    /// Increases with every published snapshot.
    pub serial: u64,
    pub stopping: bool,
    pub definitions: Vec<DefinitionSummary>,
    /// Empty when the host turned `snapshot_rows` off and reads rows by query.
    pub instances: Vec<InstanceRow>,
    pub runs: Vec<RunRowDto>,
    /// Control endpoints a prepared plan admitted. Empty in this engine.
    pub control_endpoints: Vec<ParamAddr>,
    /// Sequence of the newest transition this snapshot already reflects.
    pub transition_seq: u64,
    pub transitions_dropped: u64,
    /// Highest transition sequence that was dropped; a consumer that has not
    /// seen it yet never will.
    pub transition_dropped_through: u64,
    pub observations_dropped: u64,
    /// Sequence of the newest store event this snapshot already reflects.
    pub journal_seq: u64,
    pub journal_dropped: u64,
    /// Highest journal sequence that was dropped.
    pub journal_dropped_through: u64,
}

#[derive(Debug)]
pub enum EngineStartError {
    /// The host's loop budget cannot hold the worker and its run lanes.
    LoopBudget { needed: usize, free: usize },
    Spawn(String),
}

impl std::fmt::Display for EngineStartError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LoopBudget { needed, free } => write!(
                formatter,
                "the host admits {free} more long-lived loops; the engine needs {needed}"
            ),
            Self::Spawn(error) => write!(formatter, "the host could not start an engine loop: {error}"),
        }
    }
}

impl std::error::Error for EngineStartError {}

pub struct Engine;

impl Engine {
    /// Start the engine and return at once. `Ready` arrives as the reply to
    /// [`READY_REQUEST`]; `Stopped` as the reply to [`EngineHandle::stop`].
    pub fn start(config: EngineConfig, host: HostServices) -> Result<EngineHandle, EngineStartError> {
        let lanes = config.run_lanes.max(1);
        let needed = lanes + 1;
        let free = host.work.budget().loops_free;
        if free < needed {
            return Err(EngineStartError::LoopBudget { needed, free });
        }

        let command_capacity = config.command_capacity.max(1);
        // One slot beyond the configured capacity is the engine's own Ready.
        let reply_capacity = config.reply_capacity.max(1) + 1;
        let (command_tx, command_rx) = sync_channel::<Envelope>(command_capacity);
        let (reply_tx, reply_rx) = sync_channel::<Reply>(reply_capacity);
        let (transition_tx, transition_rx) = sync_channel::<Transition>(config.transition_capacity.max(1));
        let (observation_tx, observation_rx) = sync_channel::<Observation>(config.observation_capacity.max(1));
        let (journal_tx, journal_rx) = if config.journal_capacity > 0 {
            let (tx, rx) = sync_channel::<JournalEntry>(config.journal_capacity);
            (Some(tx), Some(rx))
        } else {
            (None, None)
        };
        let (doorbell_tx, doorbell_rx) = sync_channel::<()>(1);
        let (snapshot_writer, snapshot_reader) = triple_buffer(Arc::new(EngineSnapshot::default()));
        // The Ready reply holds one credit until it is polled.
        let reply_credits = Arc::new(AtomicUsize::new(reply_capacity - 1));

        let lane_run_capacity = config.lane_run_capacity.max(1);
        let lane_event_capacity = config.transition_capacity.max(64);
        // Floored at one event's metadata limit: with the lone-event rule a
        // budget never blocks progress, this keeps it meaningful.
        let lane_max_bytes = config.lane_max_bytes.max(EVENT_META_MAX);
        let media_max_bytes = config.run_media_max_bytes.max(EVENT_META_MAX);
        let lane_bytes = Arc::new(AtomicUsize::new(0));
        let transition_bytes = Arc::new(AtomicUsize::new(0));
        let observation_bytes = Arc::new(AtomicUsize::new(0));
        let journal_bytes = Arc::new(AtomicUsize::new(0));
        let (lane_event_tx, lane_event_rx) = sync_channel::<LaneEvent>(lane_event_capacity);
        let mut lane_handles = Vec::with_capacity(lanes);
        for index in 0..lanes {
            let (lane_tx, lane_rx) = sync_channel::<LaneStart>(lane_run_capacity);
            let exited = Arc::new(AtomicBool::new(false));
            let context = LaneContext {
                exited: exited.clone(),
                commands: lane_rx,
                events: lane_event_tx.clone(),
                lane_bytes: lane_bytes.clone(),
                lane_max_bytes,
                media_max_bytes,
                doorbell: doorbell_tx.clone(),
                seams: config.seams.clone(),
                net: config.net.clone(),
                results: host.results.clone(),
            };
            host.work
                .spawn_loop(&format!("flow-lane-{index}"), Box::new(move || lane_loop(context)))
                .map_err(spawn_error)?;
            lane_handles.push(Lane { commands: Some(lane_tx), exited, runs: HashMap::new() });
        }
        drop(lane_event_tx);

        let mut values = ValueStore::new(config.values_dir.clone());
        values.ram_budget = config.values_ram_budget;
        values.ttl = config.value_ttl;
        let layout: SourceLayout = Arc::new(|flow: &str| format!("{flow}.splash"));
        let store = FlowStore::new(FlowStoreConfig {
            registry: config.registry.clone(),
            executors: config.executors.clone(),
            values,
            revision_ring: config.revision_ring.max(1),
            instance_ttl: config.instance_ttl,
            origin: config.origin.clone(),
            layout,
            live_outputs: config.live_outputs,
            max_queued_runs: Some(config.limits.max_queued_runs),
            max_finished_runs: Some(config.limits.max_finished_runs),
            max_input_bytes: Some(config.limits.max_input_bytes),
            max_batch_runs: Some(config.limits.max_batch_runs),
        });
        let (eval_tx, eval_rx) = sync_channel::<EvalDone>(reply_capacity);
        let worker = Worker {
            store,
            driver: LaneDriver { lanes: lane_handles, lane_run_capacity },
            work: host.work.clone(),
            log: host.log.clone(),
            limits: config.limits,
            commands: command_rx,
            command_capacity,
            replies: reply_tx,
            pending_replies: Vec::new(),
            transitions: transition_tx,
            observations: observation_tx,
            journal: journal_tx,
            pending_journal: VecDeque::new(),
            journal_capacity: config.journal_capacity,
            next_event: 1,
            journal_dropped: 0,
            journal_dropped_through: 0,
            details: HashMap::new(),
            wake: host.wake.clone(),
            sent: false,
            doorbell: doorbell_rx,
            doorbell_tx: doorbell_tx.clone(),
            lane_events: lane_event_rx,
            lane_event_capacity,
            lane_bytes: lane_bytes.clone(),
            lane_max_bytes,
            transition_bytes: transition_bytes.clone(),
            observation_bytes: observation_bytes.clone(),
            journal_bytes: journal_bytes.clone(),
            eval_tx,
            eval_rx,
            eval_serial: HashMap::new(),
            pending_evals: HashMap::new(),
            next_eval: 0,
            snapshot: snapshot_writer,
            snapshot_serial: 0,
            dirty: true,
            retry_queued: false,
            pending_transitions: VecDeque::new(),
            transition_capacity: config.transition_capacity.max(1),
            next_transition: 1,
            transitions_dropped: 0,
            transition_dropped_through: 0,
            next_observation: 1,
            observations_dropped: 0,
            stopping: None,
            stop_grace: config.stop_grace,
            sweep_every: config.sweep_every.max(Duration::from_millis(1)),
            snapshot_rows: config.snapshot_rows,
            last_sweep: Instant::now(),
        };
        host.work
            .spawn_loop("flow-engine", Box::new(move || worker.run()))
            .map_err(spawn_error)?;

        Ok(EngineHandle {
            commands: command_tx,
            doorbell: doorbell_tx,
            replies: reply_rx,
            transitions: transition_rx,
            observations: observation_rx,
            journal: journal_rx,
            transition_bytes,
            observation_bytes,
            journal_bytes,
            snapshot: snapshot_reader,
            reply_credits,
            next_request: AtomicU64::new(1),
            outstanding: vec![READY_REQUEST],
            closed: false,
            last_transition: 0,
            last_observation: 0,
            last_journal: 0,
        })
    }
}

fn spawn_error(refused: SpawnRefused) -> EngineStartError {
    EngineStartError::Spawn(format!("{refused:?}"))
}

struct Envelope {
    id: RequestId,
    command: EngineCommand,
}

pub struct EngineHandle {
    commands: SyncSender<Envelope>,
    doorbell: SyncSender<()>,
    replies: Receiver<Reply>,
    transitions: Receiver<Transition>,
    observations: Receiver<Observation>,
    journal: Option<Receiver<JournalEntry>>,
    /// Payload bytes in flight on each lane; released here as entries are
    /// taken, so the worker's budget check sees what the consumer holds.
    transition_bytes: Arc<AtomicUsize>,
    observation_bytes: Arc<AtomicUsize>,
    journal_bytes: Arc<AtomicUsize>,
    snapshot: Reader<Arc<EngineSnapshot>>,
    reply_credits: Arc<AtomicUsize>,
    next_request: AtomicU64,
    /// Accepted requests whose reply was not polled yet. Bounded by the
    /// reply capacity.
    outstanding: Vec<RequestId>,
    /// The worker is gone (its reply lane closed).
    closed: bool,
    last_transition: u64,
    last_observation: u64,
    last_journal: u64,
}

impl EngineHandle {
    /// Whether the engine worker still exists. It turns false once a poll
    /// found the reply lane closed: the worker stopped, or died.
    pub fn is_alive(&self) -> bool {
        !self.closed
    }

    /// Queue a command. Never blocks. A command that is not accepted comes
    /// back to the caller untouched.
    pub fn submit(&mut self, command: EngineCommand) -> Submit {
        if let EngineCommand::SetControl { addr, .. } = &command {
            if !self.snapshot.latest().control_endpoints.contains(addr) {
                return Submit::Refused(command, Refusal::UnknownEndpoint);
            }
        }
        // Reserve the reply slot first: an accepted command always has room
        // for its terminal reply.
        if self
            .reply_credits
            .try_update(Ordering::AcqRel, Ordering::Acquire, |credits| credits.checked_sub(1))
            .is_err()
        {
            return Submit::Full(command);
        }
        let id = RequestId(self.next_request.fetch_add(1, Ordering::Relaxed));
        match self.commands.try_send(Envelope { id, command }) {
            Ok(()) => {
                self.outstanding.push(id);
                let _ = self.doorbell.try_send(());
                Submit::Accepted(id)
            }
            Err(TrySendError::Full(envelope)) => {
                self.reply_credits.fetch_add(1, Ordering::AcqRel);
                Submit::Full(envelope.command)
            }
            Err(TrySendError::Disconnected(envelope)) => {
                self.reply_credits.fetch_add(1, Ordering::AcqRel);
                Submit::Closed(envelope.command)
            }
        }
    }

    /// Ask the engine to cancel its runs and stop. `Stopped` is the reply.
    pub fn stop(&mut self) -> Submit {
        self.submit(EngineCommand::Stop)
    }

    /// Take up to `max` terminal replies. Each one frees its slot. When the
    /// engine is gone, every request it left unanswered is answered here with
    /// [`EngineError::Stopped`], so an accepted command always gets exactly
    /// one reply.
    pub fn poll_replies(&mut self, out: &mut Vec<Reply>, max: usize) {
        for _ in 0..max {
            match self.replies.try_recv() {
                Ok(reply) => {
                    self.outstanding.retain(|id| *id != reply.id);
                    self.reply_credits.fetch_add(1, Ordering::AcqRel);
                    out.push(reply);
                }
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => {
                    self.closed = true;
                    let Some(id) = self.outstanding.pop() else {
                        return;
                    };
                    self.reply_credits.fetch_add(1, Ordering::AcqRel);
                    out.push(Reply { id, result: Err(EngineError::Stopped) });
                }
            }
        }
    }

    /// Take up to `max` lifecycle transitions, oldest first.
    ///
    /// On a gap the result is [`TransitionPoll::Resync`]: it carries the one
    /// snapshot to resynchronise from, the cursor moves to exactly that
    /// snapshot's sequence, and every transition of this call that the
    /// snapshot already covers is removed from `out` before the caller can
    /// act on it. An older transition that still arrives later is discarded
    /// here, so a late delivery can never roll the caller's state back.
    pub fn poll_transitions(&mut self, out: &mut Vec<Transition>, max: usize) -> TransitionPoll {
        let first = out.len();
        let mut dropped = 0;
        let mut drained = false;
        for _ in 0..max {
            match self.transitions.try_recv() {
                Ok(transition) => {
                    self.transition_bytes.fetch_sub(transition.bytes, Ordering::AcqRel);
                    if transition.seq <= self.last_transition {
                        // Already covered by an earlier resync.
                        continue;
                    }
                    dropped += transition.seq - (self.last_transition + 1);
                    self.last_transition = transition.seq;
                    out.push(transition);
                }
                Err(_) => {
                    drained = true;
                    break;
                }
            }
        }
        if drained {
            // A trailing gap: nothing newer is queued, yet the engine says it
            // dropped sequences this consumer has not reached.
            let through = self.snapshot.latest().transition_dropped_through;
            if through > self.last_transition {
                dropped += through - self.last_transition;
            }
        }
        if dropped == 0 {
            return TransitionPoll::InOrder;
        }
        // The worker publishes a snapshot before sending what it covers, so
        // this snapshot reflects at least every sequence received above.
        let snapshot = self.snapshot.latest().clone();
        self.last_transition = self.last_transition.max(snapshot.transition_seq);
        let cursor = self.last_transition;
        let mut index = first;
        while index < out.len() {
            if out[index].seq <= cursor {
                out.remove(index);
            } else {
                index += 1;
            }
        }
        TransitionPoll::Resync { dropped, snapshot }
    }

    /// Take up to `max` observations, oldest first.
    pub fn poll_observations(&mut self, out: &mut Vec<Observation>, max: usize) -> LaneGap {
        let mut dropped = 0;
        for _ in 0..max {
            let Ok(observation) = self.observations.try_recv() else {
                break;
            };
            self.observation_bytes.fetch_sub(observation.bytes, Ordering::AcqRel);
            dropped += observation.seq.saturating_sub(self.last_observation + 1);
            self.last_observation = observation.seq;
            out.push(observation);
        }
        if dropped == 0 { LaneGap::None } else { LaneGap::Dropped(dropped) }
    }

    /// Take up to `max` journal entries, oldest first. Empty and `InOrder`
    /// when the lane is off.
    ///
    /// The cursor only ever moves forward. An entry at or behind it is
    /// discarded here, whether it was queued before a recovery or arrives
    /// late after one, so a consumer's view can never roll back.
    pub fn poll_journal(&mut self, out: &mut Vec<JournalEntry>, max: usize) -> JournalPoll {
        let Some(journal) = &self.journal else {
            return JournalPoll::InOrder;
        };
        let first = out.len();
        let mut dropped = 0;
        let mut drained = false;
        for _ in 0..max {
            match journal.try_recv() {
                Ok(entry) => {
                    self.journal_bytes.fetch_sub(entry.bytes, Ordering::AcqRel);
                    if entry.seq <= self.last_journal {
                        // Already covered by an earlier recovery.
                        continue;
                    }
                    dropped += entry.seq - (self.last_journal + 1);
                    self.last_journal = entry.seq;
                    out.push(entry);
                }
                Err(_) => {
                    drained = true;
                    break;
                }
            }
        }
        if drained {
            // A trailing loss: nothing newer is queued to show it. Only read
            // once the lane is empty, so a watermark the worker published
            // after sending entries that are still queued cannot be taken
            // for a loss ahead of them.
            let through = self.snapshot.latest().journal_dropped_through;
            if through > self.last_journal {
                dropped += through - self.last_journal;
            }
        }
        if dropped == 0 {
            return JournalPoll::InOrder;
        }
        // The worker publishes a snapshot before sending what it covers, so
        // this snapshot reflects at least every sequence received above and
        // every loss counted above.
        let snapshot = self.snapshot.latest().clone();
        self.last_journal = self.last_journal.max(snapshot.journal_seq);
        let cursor = self.last_journal;
        let mut index = first;
        while index < out.len() {
            if out[index].seq <= cursor {
                out.remove(index);
            } else {
                index += 1;
            }
        }
        JournalPoll::Gap { dropped, cursor, snapshot }
    }

    /// The newest summary the engine published.
    pub fn snapshot(&mut self) -> Arc<EngineSnapshot> {
        self.snapshot.latest().clone()
    }
}

struct Lane {
    /// `None` once the lane is known to be gone.
    commands: Option<SyncSender<LaneStart>>,
    /// Raised by the lane when its loop ends; read every worker turn.
    exited: Arc<AtomicBool>,
    /// Runs dispatched to the lane and not finished yet.
    runs: HashMap<RunId, Arc<RunTag>>,
}

/// Dispatches a run onto the least loaded live lane, or refuses it. There is
/// no overflow queue: a refused run stays with the store, which keeps it
/// queued (or reports `Refused`) and retries when a run finishes.
struct LaneDriver {
    lanes: Vec<Lane>,
    lane_run_capacity: usize,
}

impl LaneDriver {
    fn active_runs(&self) -> usize {
        self.lanes.iter().map(|lane| lane.runs.len()).sum()
    }

    fn run_finished(&mut self, lane: usize, run: &RunId) {
        if let Some(lane) = self.lanes.get_mut(lane) {
            lane.runs.remove(run);
        }
    }

    /// Lanes whose loop ended and that were not retired yet.
    fn exited_lanes(&self) -> Vec<usize> {
        self.lanes
            .iter()
            .enumerate()
            .filter(|(_, lane)| lane.commands.is_some() && lane.exited.load(Ordering::SeqCst))
            .map(|(index, _)| index)
            .collect()
    }

    /// Mark a lane gone and hand back the runs it was holding.
    fn lane_exited(&mut self, lane: usize) -> Vec<Arc<RunTag>> {
        let Some(lane) = self.lanes.get_mut(lane) else {
            return Vec::new();
        };
        lane.commands = None;
        lane.runs.drain().map(|(_, tag)| tag).collect()
    }
}

impl RunDriver for LaneDriver {
    fn start(
        &mut self,
        input: RunInput,
        instance: &InstanceId,
        flow: &str,
    ) -> Result<RunControl, RunRefused> {
        let capacity = self.lane_run_capacity;
        let Some(index) = self
            .lanes
            .iter()
            .enumerate()
            .filter(|(_, lane)| lane.commands.is_some() && lane.runs.len() < capacity)
            .min_by_key(|(_, lane)| lane.runs.len())
            .map(|(index, _)| index)
        else {
            return Err(RunRefused(input));
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let (answer, answers) = channel();
        let tag = Arc::new(RunTag {
            lane: index,
            run_id: input.run_id.clone(),
            instance: instance.clone(),
            flow: flow.to_string(),
        });
        let start = LaneStart { tag: tag.clone(), input, cancel: cancel.clone(), answers };
        let lane = &mut self.lanes[index];
        let sent = lane.commands.as_ref().map(|commands| commands.try_send(start));
        match sent {
            Some(Ok(())) => {
                lane.runs.insert(tag.run_id.clone(), tag);
                Ok(RunControl { cancel, answer })
            }
            Some(Err(TrySendError::Full(start))) => Err(RunRefused(start.input)),
            Some(Err(TrySendError::Disconnected(start))) => {
                lane.commands = None;
                Err(RunRefused(start.input))
            }
            None => unreachable!("the lane was filtered on `commands.is_some()`"),
        }
    }
}

struct EvalDone {
    id: RequestId,
    serial: u64,
    name: String,
    source: String,
    file_name: String,
    closure: Arc<Closure>,
    ephemeral: bool,
    evaluated: Result<Graph, EvalError>,
}

/// Reports an evaluation that never completed: its job panicked, or the
/// host dropped it unrun. Either way the request still gets its reply.
struct EvalGuard {
    done: SyncSender<EvalDone>,
    doorbell: SyncSender<()>,
    /// Cleared by the worker when the host refused the job: that request was
    /// already answered `Busy`, so the guard must not report anything and
    /// must not take a completion slot it holds no reply credit for.
    armed: Arc<AtomicBool>,
    pending: Option<(RequestId, u64, String, String, String, Arc<Closure>, bool)>,
}

impl EvalGuard {
    fn finish(&mut self, evaluated: Result<Graph, EvalError>) {
        let Some((id, serial, name, source, file_name, closure, ephemeral)) = self.pending.take() else {
            return;
        };
        // Room is guaranteed: one pending evaluation per reserved reply.
        let _ = self.done.try_send(EvalDone { id, serial, name, source, file_name, closure, ephemeral, evaluated });
        let _ = self.doorbell.try_send(());
    }
}

impl Drop for EvalGuard {
    fn drop(&mut self) {
        if !self.armed.load(Ordering::SeqCst) {
            return;
        }
        if let Some((_, _, _, _, file_name, _, _)) = &self.pending {
            let error = EvalError {
                file: file_name.clone(),
                line: 1,
                col: 1,
                message: "the evaluation job did not complete".to_string(),
            };
            self.finish(Err(error));
        }
    }
}

struct Worker {
    store: FlowStore,
    driver: LaneDriver,
    work: Arc<dyn HostWork>,
    log: Arc<dyn Fn(&str) + Send + Sync>,
    limits: EngineLimits,
    commands: Receiver<Envelope>,
    /// Capacity of the command channel: the bound on the final drain.
    command_capacity: usize,
    replies: SyncSender<Reply>,
    /// Replies of this turn, sent after the turn's events and snapshot.
    pending_replies: Vec<Reply>,
    transitions: SyncSender<Transition>,
    observations: SyncSender<Observation>,
    /// The ordered journal lane, when the host asked for one.
    journal: Option<SyncSender<JournalEntry>>,
    /// This turn's journal entries, sent after the snapshot that covers them.
    /// Holds at most `journal_capacity`: what the lane could take if it were
    /// empty. Anything past that is dropped here, counted the same way.
    pending_journal: VecDeque<JournalEntry>,
    journal_capacity: usize,
    /// Sequence of the next store event: one stream, before the lanes split.
    next_event: u64,
    journal_dropped: u64,
    journal_dropped_through: u64,
    /// Definition detail, built once per revision and shared with readers.
    details: HashMap<String, Arc<FlowResponse>>,
    wake: Option<Arc<dyn Fn() + Send + Sync>>,
    /// Something was sent this turn; the host is woken once at its end.
    sent: bool,
    doorbell: Receiver<()>,
    doorbell_tx: SyncSender<()>,
    lane_events: Receiver<LaneEvent>,
    /// Capacity of the lane event channel: the bound on a dead lane's drain.
    lane_event_capacity: usize,
    /// Payload bytes the run lanes have in the channel; released on receipt.
    lane_bytes: Arc<AtomicUsize>,
    lane_max_bytes: usize,
    /// Payload bytes each outbound lane holds (pending plus in flight).
    transition_bytes: Arc<AtomicUsize>,
    observation_bytes: Arc<AtomicUsize>,
    journal_bytes: Arc<AtomicUsize>,
    eval_tx: SyncSender<EvalDone>,
    eval_rx: Receiver<EvalDone>,
    /// Newest load issued per definition; a result with an older serial is
    /// stale and is answered `Superseded`.
    eval_serial: HashMap<String, u64>,
    /// Evaluations in flight, by request, with the definition they load.
    pending_evals: HashMap<RequestId, String>,
    next_eval: u64,
    snapshot: Writer<Arc<EngineSnapshot>>,
    snapshot_serial: u64,
    dirty: bool,
    /// A run finished or a lane went away: queued runs may fit now.
    retry_queued: bool,
    pending_transitions: VecDeque<Transition>,
    transition_capacity: usize,
    next_transition: u64,
    transitions_dropped: u64,
    transition_dropped_through: u64,
    next_observation: u64,
    observations_dropped: u64,
    /// The stop request (if any) and when stopping began.
    stopping: Option<(Option<RequestId>, Instant)>,
    stop_grace: Duration,
    sweep_every: Duration,
    snapshot_rows: bool,
    last_sweep: Instant,
}

/// Longest the worker parks when nothing is scheduled.
const IDLE_PARK: Duration = Duration::from_millis(250);
/// How soon the worker retries a transition lane that was full.
const RETRY_PARK: Duration = Duration::from_millis(5);
/// Most entries taken from one channel per turn, so no source can starve
/// the others.
const TURN_BUDGET: usize = 256;

impl Worker {
    fn run(mut self) {
        self.reply(READY_REQUEST, Ok(ReplyPayload::Ready));
        self.flush_replies();
        let mut host_gone = false;
        loop {
            let _ = self.doorbell.recv_timeout(self.park_for());

            for _ in 0..TURN_BUDGET {
                match self.lane_events.try_recv() {
                    Ok(event) => self.on_lane_event(event),
                    Err(_) => break,
                }
            }
            self.retire_exited_lanes();
            for _ in 0..TURN_BUDGET {
                match self.eval_rx.try_recv() {
                    Ok(done) => self.on_evaluated(done),
                    Err(_) => break,
                }
            }
            for _ in 0..TURN_BUDGET {
                match self.commands.try_recv() {
                    Ok(envelope) => self.on_command(envelope),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        host_gone = true;
                        break;
                    }
                }
            }

            if self.stopping.is_none() {
                if self.retry_queued {
                    self.retry_queued = false;
                    self.store.retry_queued(&mut self.driver);
                }
                self.store.run_debounced_inputs(engine::unix_ms(), &mut self.driver);
                if self.last_sweep.elapsed() >= self.sweep_every {
                    self.last_sweep = Instant::now();
                    self.store.janitor_sweep(&HashSet::new(), &HashSet::new());
                    self.dirty = true;
                }
            }
            self.route_store_events();
            // Snapshot first: a consumer that sees a gap resynchronises from
            // the published snapshot, which must cover everything sent.
            if self.dirty {
                self.publish_snapshot();
            }
            self.flush_transitions();
            self.flush_journal();
            self.flush_replies();

            if host_gone && self.stopping.is_none() {
                // The handle was dropped: stop without a requester.
                self.begin_stop(None);
            }
            if let Some((request, since)) = self.stopping {
                let drained = self.driver.active_runs() == 0;
                if drained || since.elapsed() >= self.stop_grace {
                    let abandoned = if drained {
                        0
                    } else {
                        // The grace period is over. The runs still in flight
                        // are recorded as cancelled-by-abandonment, and the
                        // reply says how many there were.
                        let abandoned = self.store.abandon_in_flight(
                            "abandoned: the engine stopped before the run finished",
                        );
                        (self.log)(&format!(
                            "flow engine: stop grace elapsed; {abandoned} runs abandoned in flight"
                        ));
                        abandoned
                    };
                    self.finish(request, abandoned);
                    return;
                }
            }
        }
    }

    /// Leave nothing unanswered: the commands queued when the stop completes
    /// and every evaluation still in flight get their reply, then `Stopped`.
    fn finish(&mut self, request: Option<RequestId>, abandoned_runs: usize) {
        // Whatever was queued at this moment sits in the first
        // `command_capacity` entries. A live host that polls replies and keeps
        // submitting cannot hold the worker here past the stop deadline: what
        // it sends beyond this prefix is answered `Stopped` by the handle.
        for _ in 0..self.command_capacity {
            match self.commands.try_recv() {
                Ok(envelope) => self.reply(envelope.id, Err(EngineError::Stopping)),
                Err(_) => break,
            }
        }
        let pending: Vec<RequestId> = self.pending_evals.drain().map(|(id, _)| id).collect();
        for id in pending {
            self.reply(id, Err(EngineError::Stopping));
        }
        self.route_store_events();
        self.publish_snapshot();
        self.flush_transitions();
        self.flush_journal();
        if let Some(request) = request {
            self.reply(request, Ok(ReplyPayload::Stopped { abandoned_runs }));
        }
        self.flush_replies();
        // Dropping the worker closes every lane's command channel; a command
        // still queued or accepted after the drain above is answered
        // `Stopped` by the handle when it sees the reply lane closed.
    }

    fn park_for(&self) -> Duration {
        if !self.pending_transitions.is_empty() || self.stopping.is_some() {
            return RETRY_PARK;
        }
        let mut park = IDLE_PARK.min(self.sweep_every.saturating_sub(self.last_sweep.elapsed()).max(Duration::from_millis(1)));
        if let Some(deadline) = self.store.next_debounce_deadline() {
            let wait = Duration::from_millis(deadline.saturating_sub(engine::unix_ms()));
            park = park.min(wait.max(Duration::from_millis(1)));
        }
        park
    }

    /// Replies wait for the end of the turn: by then the events the command
    /// caused are on their lanes and the snapshot reflects it.
    fn reply(&mut self, id: RequestId, result: Result<ReplyPayload, EngineError>) {
        self.pending_replies.push(Reply { id, result });
    }

    fn flush_replies(&mut self) {
        for reply in std::mem::take(&mut self.pending_replies) {
            // Cannot be full: the slot was reserved when the command was accepted.
            if self.replies.try_send(reply).is_err() {
                (self.log)("flow engine: reply lane refused a reserved reply (host gone?)");
            }
            self.sent = true;
        }
        if std::mem::take(&mut self.sent) {
            if let Some(wake) = &self.wake {
                wake();
            }
        }
    }

    /// Entries that do not fit are dropped, never kept: the lane is a
    /// transport, and its consumer recovers from the snapshot.
    fn flush_journal(&mut self) {
        let Some(journal) = &self.journal else {
            let pending: usize = self.pending_journal.iter().map(|entry| entry.bytes).sum();
            self.journal_bytes.fetch_sub(pending, Ordering::AcqRel);
            self.pending_journal.clear();
            return;
        };
        let mut lost = false;
        while let Some(entry) = self.pending_journal.pop_front() {
            let seq = entry.seq;
            let bytes = entry.bytes;
            match journal.try_send(entry) {
                Ok(()) => self.sent = true,
                Err(TrySendError::Full(_)) => {
                    self.journal_bytes.fetch_sub(bytes, Ordering::AcqRel);
                    self.journal_dropped += 1;
                    self.journal_dropped_through = self.journal_dropped_through.max(seq);
                    lost = true;
                }
                Err(TrySendError::Disconnected(_)) => {
                    let pending: usize = self.pending_journal.iter().map(|entry| entry.bytes).sum::<usize>() + bytes;
                    self.journal_bytes.fetch_sub(pending, Ordering::AcqRel);
                    self.pending_journal.clear();
                    self.journal = None;
                    return;
                }
            }
        }
        if lost {
            // The watermark travels in the snapshot.
            self.publish_snapshot();
        }
    }

    fn begin_stop(&mut self, request: Option<RequestId>) {
        // Nothing starts from here on: queued runs are settled as cancelled,
        // pending triggers dropped, runs in flight flagged.
        self.store.begin_shutdown();
        self.stopping = Some((request, Instant::now()));
        self.dirty = true;
    }

    fn on_lane_event(&mut self, lane_event: LaneEvent) {
        let LaneEvent { tag, event, bytes } = lane_event;
        self.lane_bytes.fetch_sub(bytes, Ordering::AcqRel);
        if matches!(event, RunEvent::RunFinished { .. }) {
            self.driver.run_finished(tag.lane, &tag.run_id);
            self.retry_queued = true;
        }
        self.store.apply_run_event(
            tag.run_id.clone(),
            tag.instance.clone(),
            tag.flow.clone(),
            event,
            &mut self.driver,
        );
        self.dirty = true;
    }

    /// A lane whose loop ended raises a flag it cannot fail to raise. Its
    /// last events may still sit in the channel, so those are applied first
    /// (the channel is bounded, and a dead lane adds nothing more); whatever
    /// runs it still held after that are finished `Failed`.
    fn retire_exited_lanes(&mut self) {
        let exited = self.driver.exited_lanes();
        if exited.is_empty() {
            return;
        }
        // Everything the dead lane managed to enqueue is within the first
        // `lane_event_capacity` entries of the channel right now. Live lanes
        // may keep refilling it, so the drain is bounded by that capacity
        // rather than by emptiness.
        for _ in 0..self.lane_event_capacity {
            match self.lane_events.try_recv() {
                Ok(event) => self.on_lane_event(event),
                Err(_) => break,
            }
        }
        for lane in exited {
            let orphaned = self.driver.lane_exited(lane);
            (self.log)(&format!(
                "flow engine: run lane {lane} ended with {} runs in flight",
                orphaned.len()
            ));
            for tag in orphaned {
                self.store.apply_run_event(
                    tag.run_id.clone(),
                    tag.instance.clone(),
                    tag.flow.clone(),
                    RunEvent::RunFinished {
                        state: RunState::Failed,
                        secs: 0.0,
                        outputs: Vec::new(),
                        http_log: Vec::new(),
                        warnings: vec!["the run lane terminated".to_string()],
                    },
                    &mut self.driver,
                );
            }
        }
        self.retry_queued = true;
        self.dirty = true;
    }

    fn on_evaluated(&mut self, done: EvalDone) {
        if self.pending_evals.remove(&done.id).is_none() {
            // Already answered (the engine was stopping).
            return;
        }
        if self.eval_serial.get(&done.name) != Some(&done.serial) {
            self.reply(done.id, Err(EngineError::Superseded));
            return;
        }
        self.eval_serial.remove(&done.name);
        let name = done.name.clone();
        // The file name belongs to this evaluation and is installed with it.
        self.store.set_file_name(&name, done.file_name);
        let result = self.store.apply_evaluated_with(done.name, done.source, done.closure, Some(done.ephemeral), done.evaluated);
        self.details.remove(&name);
        self.dirty = true;
        let interface = result.graph.as_ref().map(FlowInterface::of);
        self.reply(
            done.id,
            Ok(ReplyPayload::Definition {
                name,
                revision: result.revision,
                error: result.error,
                interface,
                graph: result.graph,
            }),
        );
    }

    fn on_command(&mut self, envelope: Envelope) {
        let Envelope { id, command } = envelope;
        if self.stopping.is_some() {
            self.reply(id, Err(EngineError::Stopping));
            return;
        }
        let result = match command {
            EngineCommand::LoadDefinition { name, source, file_name, closure, ephemeral } => {
                match self.load_definition(id, name, source, file_name, closure, ephemeral) {
                    // The reply follows the evaluation.
                    None => return,
                    Some(result) => result,
                }
            }
            EngineCommand::SetLoadError { name, error } => match self.definition_room(&name) {
                Err(error) => Err(error),
                Ok(()) => {
                    // A load still in flight for this name is now stale.
                    self.eval_serial.remove(&name);
                    self.details.remove(&name);
                    self.store.set_load_error(name, error);
                    Ok(ReplyPayload::LoadErrorSet)
                }
            },
            EngineCommand::RemoveDefinition { name } => {
                // A load still in flight for this name is now stale.
                self.eval_serial.remove(&name);
                self.details.remove(&name);
                Ok(ReplyPayload::Removed(self.store.remove_definition(&name)))
            }
            EngineCommand::Autostart => {
                self.store.autostart_all();
                Ok(ReplyPayload::Autostarted)
            }
            EngineCommand::Instantiate { flow, label, pinned, owner, inputs } => {
                if self.store.instances.len() >= self.limits.max_instances {
                    Err(EngineError::Busy(format!("the engine holds {} instances", self.limits.max_instances)))
                } else {
                    match self.store.create_instance(&flow, label, pinned, owner, inputs) {
                        CreateInstanceOutcome::Created(instance) => Ok(ReplyPayload::Instance(instance)),
                        CreateInstanceOutcome::FlowNotFound => Err(EngineError::NotFound(format!("flow `{flow}`"))),
                        CreateInstanceOutcome::FlowInvalid => Err(EngineError::Conflict(Conflict::FlowInvalid)),
                        CreateInstanceOutcome::Error(error) => Err(EngineError::Invalid(error)),
                    }
                }
            }
            EngineCommand::SetInputs { instance, inputs, actor } => {
                // The size bound is the store's, so every path shares it.
                let outcome = self.store.set_instance_values(&instance, inputs, &actor);
                inputs_reply(&instance, outcome)
            }
            EngineCommand::SetInputDtos { instance, inputs, actor } => {
                let outcome = self.store.set_instance_inputs(&instance, inputs, &actor);
                inputs_reply(&instance, outcome)
            }
            EngineCommand::ClearInstance { instance } => match self.store.clear_instance(&instance) {
                ClearInstanceOutcome::Cleared => Ok(ReplyPayload::Cleared),
                ClearInstanceOutcome::InstanceNotFound => {
                    Err(EngineError::NotFound(format!("instance `{}`", instance.0)))
                }
                ClearInstanceOutcome::Busy => Err(EngineError::Conflict(Conflict::InstanceBusy)),
            },
            EngineCommand::StartRun { instance, outputs } => {
                match self.store.start_run(&instance, outputs, &mut self.driver) {
                    StartRunOutcome::Started { run_id, queued } => {
                        Ok(ReplyPayload::RunStarted { run: RunId(run_id), queued })
                    }
                    StartRunOutcome::InstanceNotFound => {
                        Err(EngineError::NotFound(format!("instance `{}`", instance.0)))
                    }
                    StartRunOutcome::FlowInvalid => Err(EngineError::Conflict(Conflict::FlowInvalid)),
                    StartRunOutcome::Busy => Err(EngineError::Conflict(Conflict::ConcurrencyZero)),
                    StartRunOutcome::Refused => Err(EngineError::Busy(
                        "every run lane is full, or the instance's run queue is".to_string(),
                    )),
                }
            }
            EngineCommand::CancelRun { run } => {
                Ok(ReplyPayload::Cancelled(self.store.cancel_run_and_retire_batch_instance(&run)))
            }
            EngineCommand::DeleteInstance { instance } => {
                Ok(ReplyPayload::Deleted(self.store.delete_instance(&instance)))
            }
            EngineCommand::CreateBatch { flow, request } => {
                let room = self.limits.max_instances.saturating_sub(self.store.instances.len());
                if request.parallel as usize > room && request.parallel <= super::batches::MAX_BATCH_PARALLEL {
                    Err(EngineError::Busy(format!("the engine holds {} instances", self.limits.max_instances)))
                } else {
                    match self.store.create_batch(&flow, request, &mut self.driver) {
                        CreateBatchOutcome::Created(response) => Ok(ReplyPayload::Batch(response)),
                        CreateBatchOutcome::FlowNotFound => Err(EngineError::NotFound(format!("flow `{flow}`"))),
                        CreateBatchOutcome::FlowInvalid => Err(EngineError::Conflict(Conflict::FlowInvalid)),
                        CreateBatchOutcome::InvalidParallel => {
                            Err(EngineError::Invalid("parallel must be between 1 and 256".to_string()))
                        }
                        CreateBatchOutcome::Busy { retained, limit } => Err(EngineError::Busy(format!(
                            "batches hold {retained} of {limit} retained runs; clear a batch first"
                        ))),
                        CreateBatchOutcome::Error(error) => Err(EngineError::Invalid(error)),
                    }
                }
            }
            EngineCommand::CancelBatch { batch } => Ok(ReplyPayload::BatchRuns(self.store.cancel_batch(&batch))),
            EngineCommand::ClearBatch { batch } => Ok(ReplyPayload::BatchRuns(self.store.clear_batch(&batch))),
            EngineCommand::ReadValue { digest } => Ok(ReplyPayload::Value(self.store.get_value(&digest))),
            EngineCommand::ReadTypedValue { digest, ty } => {
                match self.store.registry.types().require_artifact(&ty) {
                    Err(error) => Err(EngineError::Invalid(error.to_string())),
                    Ok(_) => Ok(ReplyPayload::Value(self.store.get_value_typed(&digest, &ty))),
                }
            }
            EngineCommand::PutValue { value } => {
                let limit = self.limits.max_input_bytes;
                if let Err(error) = self.store.registry.types().require_artifact(&value.ty) {
                    Err(EngineError::Invalid(error.to_string()))
                } else if value.bytes.len() > limit {
                    Err(EngineError::Invalid(format!("the value exceeds {limit} bytes")))
                } else {
                    Ok(ReplyPayload::ValueStored(self.store.put_value(value)))
                }
            }
            EngineCommand::Query(query) => Ok(self.answer(query)),
            EngineCommand::SetControl { addr, .. } => Err(EngineError::NotFound(format!(
                "control endpoint `{}.{}` is not admitted",
                addr.def.node.node.0, addr.def.param
            ))),
            EngineCommand::Stop => {
                self.begin_stop(Some(id));
                return;
            }
        };
        self.dirty = true;
        self.reply(id, result);
    }

    /// Returns the immediate result, or `None` when the evaluation was
    /// handed to the host's pool and the reply follows it. Every load takes
    /// a new serial, so it supersedes any load of the same name still in
    /// flight, whether or not it needs an evaluation itself.
    /// Whether a definition named `name` may exist: it already does (loaded
    /// or loading), or there is a slot left once every load in flight for a
    /// new name has taken its own.
    fn definition_room(&self, name: &str) -> Result<(), EngineError> {
        if self.store.definitions.contains_key(name) || self.eval_serial.contains_key(name) {
            return Ok(());
        }
        let loading = self
            .eval_serial
            .keys()
            .filter(|name| !self.store.definitions.contains_key(*name))
            .count();
        if self.store.definitions.len() + loading >= self.limits.max_definitions {
            return Err(EngineError::Busy(format!(
                "the engine holds {} definitions",
                self.limits.max_definitions
            )));
        }
        Ok(())
    }

    fn load_definition(
        &mut self,
        id: RequestId,
        name: String,
        source: String,
        file_name: String,
        closure: Option<Arc<Closure>>,
        ephemeral: bool,
    ) -> Option<Result<ReplyPayload, EngineError>> {
        if let Err(error) = self.definition_room(&name) {
            return Some(Err(error));
        }
        self.next_eval += 1;
        let serial = self.next_eval;
        if let Some(unchanged) = self.store.unchanged_with(&name, &source, closure.as_deref()) {
            // Nothing to evaluate, but an older load in flight must not land
            // on top of this one.
            self.eval_serial.remove(&name);
            self.store.set_file_name(&name, file_name);
            return Some(Ok(ReplyPayload::Definition {
                name,
                revision: unchanged.revision,
                error: unchanged.error,
                interface: unchanged.graph.as_ref().map(FlowInterface::of),
                graph: unchanged.graph,
            }));
        }
        // The closure is fixed here, with the request: what the job
        // evaluates against is what gets installed, whatever else lands on
        // the definition meanwhile.
        let closure = self.store.closure_for(&name, closure);
        let armed = Arc::new(AtomicBool::new(true));
        let mut guard = EvalGuard {
            done: self.eval_tx.clone(),
            doorbell: self.doorbell_tx.clone(),
            armed: armed.clone(),
            pending: Some((id, serial, name.clone(), source, file_name, closure, ephemeral)),
        };
        let registry = self.store.registry.clone();
        let job = Box::new(move || {
            let Some((_, _, _, source, file_name, closure, _)) = guard.pending.as_ref() else {
                return;
            };
            let evaluated = catch_unwind(AssertUnwindSafe(|| graph::evaluate_with_registry(source, file_name, closure, &registry)));
            match evaluated {
                Ok(evaluated) => guard.finish(evaluated),
                // The guard's drop reports the failure.
                Err(_) => drop(guard),
            }
        });
        // Registered before submission: a refused job drops its guard, whose
        // report must find the request pending.
        self.pending_evals.insert(id, name.clone());
        let previous = self.eval_serial.insert(name.clone(), serial);
        match self.work.submit_job(JobLane::Evaluate, job) {
            Ok(()) => None,
            Err(refused) => {
                // Disarm the guard before the job drops: this request gets
                // its one reply (`Busy`) right here, and a disarmed guard
                // sends nothing, so refusals can never fill the completion
                // channel that admitted evaluations rely on.
                armed.store(false, Ordering::SeqCst);
                self.pending_evals.remove(&id);
                match previous {
                    Some(previous) => self.eval_serial.insert(name, previous),
                    None => self.eval_serial.remove(&name),
                };
                drop(refused);
                Some(Err(EngineError::Busy("the host's evaluation lane is full".to_string())))
            }
        }
    }

    fn route_store_events(&mut self) {
        for event in self.store.drain_events() {
            self.dirty = true;
            if let StoreEvent::Log(message) = &event {
                (self.log)(message);
                continue;
            }
            // One ordered stream first; the lanes split below.
            let event_seq = self.next_event;
            self.next_event += 1;
            let bytes = event.payload_bytes();
            match &event {
                StoreEvent::FlowChanged { name, .. }
                | StoreEvent::FlowError { name, .. }
                | StoreEvent::FlowRemoved { name } => {
                    self.details.remove(name);
                }
                _ => {}
            }
            if self.journal.is_some() {
                if self.pending_journal.len() < self.journal_capacity && self.reserve(&self.journal_bytes.clone(), bytes) {
                    self.pending_journal.push_back(JournalEntry { seq: event_seq, bytes, event: event.clone() });
                } else {
                    self.journal_dropped += 1;
                    self.journal_dropped_through = self.journal_dropped_through.max(event_seq);
                }
            }
            let kind = match event {
                StoreEvent::FlowChanged { name, revision, canonical } => {
                    TransitionKind::FlowChanged { name, revision, canonical }
                }
                StoreEvent::FlowError { name, error } => TransitionKind::FlowError { name, error },
                StoreEvent::FlowRemoved { name } => TransitionKind::FlowRemoved { name },
                StoreEvent::InstanceCreated { instance, flow } => TransitionKind::InstanceCreated { instance, flow },
                StoreEvent::InstanceRemoved { instance, flow } => TransitionKind::InstanceRemoved { instance, flow },
                StoreEvent::InstanceInputs { instance, flow } => TransitionKind::InstanceInputs { instance, flow },
                StoreEvent::InstanceCleared { instance, flow } => TransitionKind::InstanceCleared { instance, flow },
                StoreEvent::Run { run_id, instance, event, .. }
                    if matches!(event, RunEvent::NodeProgress { .. } | RunEvent::NodeDelta { .. }) =>
                {
                    let seq = self.next_observation;
                    self.next_observation += 1;
                    if !self.reserve(&self.observation_bytes.clone(), bytes) {
                        self.observations_dropped += 1;
                        continue;
                    }
                    if self
                        .observations
                        .try_send(Observation { seq, event_seq, bytes, run: run_id, instance, event })
                        .is_err()
                    {
                        self.observation_bytes.fetch_sub(bytes, Ordering::AcqRel);
                        self.observations_dropped += 1;
                    } else {
                        self.sent = true;
                    }
                    continue;
                }
                StoreEvent::Run { run_id, instance, flow, event } => {
                    TransitionKind::Run { run: run_id, instance, flow, event }
                }
                StoreEvent::Log(_) => continue,
            };
            let seq = self.next_transition;
            self.next_transition += 1;
            self.transition_bytes.fetch_add(bytes, Ordering::AcqRel);
            self.pending_transitions.push_back(Transition { seq, event_seq, bytes, kind });
        }
        // Over capacity (count, or payload bytes pending plus in flight):
        // the oldest intermediate run transition goes first; with none left,
        // the oldest transition of any kind. Each drop is recorded so the
        // consumer can see it and resynchronise.
        while !self.pending_transitions.is_empty()
            && (self.pending_transitions.len() > self.transition_capacity
                || (self.pending_transitions.len() > 1
                    && self.transition_bytes.load(Ordering::Acquire) > self.lane_max_bytes))
        {
            let victim = self
                .pending_transitions
                .iter()
                .position(|transition| transition.kind.droppable())
                .unwrap_or(0);
            if let Some(dropped) = self.pending_transitions.remove(victim) {
                self.transition_bytes.fetch_sub(dropped.bytes, Ordering::AcqRel);
                self.transitions_dropped += 1;
                self.transition_dropped_through = self.transition_dropped_through.max(dropped.seq);
            }
        }
    }

    /// Take `bytes` of a lane's budget, or refuse.
    /// Take `bytes` of a lane's budget, or refuse. An empty lane takes any
    /// event: the budget bounds what queues behind one, never whether one
    /// can travel.
    fn reserve(&self, counter: &AtomicUsize, bytes: usize) -> bool {
        counter
            .try_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                (current == 0 || current + bytes <= self.lane_max_bytes).then_some(current + bytes)
            })
            .is_ok()
    }

    fn flush_transitions(&mut self) {
        while let Some(transition) = self.pending_transitions.pop_front() {
            match self.transitions.try_send(transition) {
                Ok(()) => self.sent = true,
                Err(TrySendError::Full(transition)) => {
                    self.pending_transitions.push_front(transition);
                    return;
                }
                Err(TrySendError::Disconnected(_)) => {
                    let pending: usize = self.pending_transitions.iter().map(|t| t.bytes).sum();
                    self.transition_bytes.fetch_sub(pending, Ordering::AcqRel);
                    self.pending_transitions.clear();
                    return;
                }
            }
        }
    }

    fn publish_snapshot(&mut self) {
        self.dirty = false;
        self.snapshot_serial += 1;
        let snapshot = EngineSnapshot {
            serial: self.snapshot_serial,
            stopping: self.stopping.is_some(),
            definitions: self
                .store
                .definitions
                .values()
                .map(|definition| DefinitionSummary {
                    name: definition.name.clone(),
                    revision: definition.revision,
                    valid: definition.graph.is_some() && definition.error.is_none(),
                    error: definition.error.clone(),
                })
                .collect(),
            instances: if self.snapshot_rows { self.store.list_instance_rows(None, false) } else { Vec::new() },
            runs: if self.snapshot_rows { self.store.list_run_rows(None) } else { Vec::new() },
            control_endpoints: Vec::new(),
            transition_seq: self.next_transition - 1,
            transitions_dropped: self.transitions_dropped,
            transition_dropped_through: self.transition_dropped_through,
            observations_dropped: self.observations_dropped,
            journal_seq: self.next_event - 1,
            journal_dropped: self.journal_dropped,
            journal_dropped_through: self.journal_dropped_through,
        };
        self.snapshot.publish(Arc::new(snapshot));
    }

    fn answer(&mut self, query: EngineQuery) -> ReplyPayload {
        match query {
            EngineQuery::Definitions => ReplyPayload::Definitions(
                self.store
                    .definitions
                    .values()
                    .map(|definition| {
                        let graph = definition.graph.as_ref();
                        FlowSummary {
                            name: definition.name.clone(),
                            label: graph
                                .map(|graph| graph.label.clone())
                                .unwrap_or_else(|| definition.name.clone()),
                            revision: definition.revision,
                            state: if definition.error.is_some() { "error" } else { "ok" }.to_string(),
                            error: definition.error.clone(),
                            canonical: definition.canonical,
                            instances: self
                                .store
                                .instances
                                .values()
                                .filter(|instance| instance.flow == definition.name)
                                .count() as u64,
                            autostart: graph.is_some_and(|graph| graph.autostart),
                        }
                    })
                    .collect(),
            ),
            EngineQuery::Definition { name } => {
                if !self.details.contains_key(&name) {
                    if let Some(definition) = self.store.definitions.get(&name) {
                        let detail = FlowResponse {
                            source: definition.source.clone(),
                            revision: definition.revision,
                            graph: definition.graph.clone(),
                            tools: definition.tools.clone(),
                            error: definition.error.clone(),
                            modules: definition
                                .valid_closure
                                .as_ref()
                                .map(|closure| closure.modules.clone())
                                .unwrap_or_default(),
                            valid_source: definition.valid_source.clone(),
                            canonical: definition.canonical,
                            ephemeral: definition.ephemeral,
                        };
                        self.details.insert(name.clone(), Arc::new(detail));
                    }
                }
                ReplyPayload::DefinitionDetail(self.details.get(&name).cloned())
            }
            EngineQuery::DefinitionSource { name, revision } => ReplyPayload::DefinitionSource(
                self.store
                    .definitions
                    .get(&name)
                    .and_then(|definition| definition.ring.iter().find(|entry| entry.0 == revision))
                    .map(|entry| (entry.1.clone(), entry.2.clone())),
            ),
            EngineQuery::Instances { flow, waiting_only } => {
                ReplyPayload::Instances(self.store.list_instance_rows(flow.as_deref(), waiting_only))
            }
            EngineQuery::Instance { instance } => ReplyPayload::InstanceRow(self.store.instance_row(&instance)),
            EngineQuery::InstanceExecution { instance } => {
                ReplyPayload::InstanceExecution(self.store.instance_execution(&instance))
            }
            EngineQuery::Runs { instance } => ReplyPayload::Runs(self.store.list_run_rows(instance.as_ref())),
            EngineQuery::Run { run } => ReplyPayload::RunRow(self.store.run_row(&run)),
        }
    }
}

fn inputs_reply(instance: &InstanceId, outcome: SetInputsOutcome) -> Result<ReplyPayload, EngineError> {
    match outcome {
        SetInputsOutcome::Ok(inputs) => Ok(ReplyPayload::InputsSet(inputs)),
        SetInputsOutcome::InstanceNotFound => Err(EngineError::NotFound(format!("instance `{}`", instance.0))),
        SetInputsOutcome::AskNotWaiting => Err(EngineError::Conflict(Conflict::AskNotWaiting)),
        SetInputsOutcome::Error(error) => Err(EngineError::Invalid(error)),
    }
}
