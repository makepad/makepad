//! The shared flow store: definitions, instances, runs and values.
//!
//! This is the one implementation of flow state semantics. It is a plain
//! single-threaded structure with no I/O of its own: its owner (the engine's
//! worker) calls it, hands it a [`RunDriver`] when a call may dispatch a run,
//! and drains the typed [`StoreEvent`]s it produced. The engine numbers them
//! as one ordered stream; the HTTP host renders that stream as its JSON
//! journal, other hosts read it as transitions and observations.

use crate::digest::from_hex_32;
use crate::document::closure::Closure;
use crate::engine::{self, HttpLogEntry, RunEvent, RunId, RunInput};
use crate::instance::{InputEffect, Instance, InstanceId, Owner, RunDecision, Waiting};
use crate::values::{PortValue, Sequence, Value, ValueStore};
use crate::{
    EvalError, Graph, InputRefs, InputValueDto, InstanceRow, MAX_SEQUENCE, NodeRowDto, NodeState, PortSequenceRef, PortType, PortValueRef, RunRowDto, RunState, SequenceRef, ToolSchema, ValueRef, WaitingDto, graph,
};
use makepad_micro_serde::{JsonValue, SerJson};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

/// Every input change starts a run this many ms after the last one (§4.1).
const INPUT_DEBOUNCE_MS: u64 = 250;
/// A finished run's row is kept this long (§5.2).
const RUN_RETENTION_MS: u64 = 60 * 60 * 1000;
/// A cap on the accumulated `node.delta` text kept per node (§3 of the
/// brief: "text? (delta so far, ≤ 16 KB)"). Truncated from the front so the
/// most recent tokens stay visible.
const MAX_DELTA_TEXT: usize = 16 * 1024;

#[derive(Clone, Debug)]
pub struct Definition {
    pub name: String,
    /// The newest source, valid or not (what an editor shows).
    pub source: String,
    /// The source `graph` was evaluated from: what a run executes. It differs
    /// from `source` while the newest source is an invalid draft.
    pub valid_source: Option<String>,
    /// The module closure the newest source was evaluated with.
    pub closure: Arc<Closure>,
    /// The closure `graph` was evaluated with: what a run executes, paired
    /// with `valid_source`. An invalid newer closure never replaces it.
    pub valid_closure: Option<Arc<Closure>>,
    /// Owned by the session that installed it: no file of the root's.
    pub ephemeral: bool,
    pub revision: u64,
    pub graph: Option<Graph>,
    pub error: Option<EvalError>,
    pub canonical: bool,
    pub tools: ToolSchema,
    /// Every kept revision: its source and the closure it evaluated with.
    /// The current revision is always here, with up to `revision_ring`
    /// history entries; older revisions a pinned instance or a queued run
    /// still holds stay as well, so the length is bounded by the history
    /// count plus the held set, not by the history count alone.
    pub ring: VecDeque<(u64, String, Arc<Closure>)>,
    /// The executable triple of every revision the ring keeps, as it was
    /// accepted: shared and immutable, never re-evaluated. In lockstep with
    /// `ring` (an entry leaves when its revision does), so an instance at a
    /// kept revision can be handed exactly what it runs.
    pub accepted: Vec<Arc<AcceptedRevision>>,
}

/// One accepted revision of a definition: the source that evaluated, the
/// closure it evaluated with and the graph that came out, with the graph's
/// `revision` set. What a run at that revision executes.
#[derive(Debug)]
pub struct AcceptedRevision {
    pub revision: u64,
    pub source: Arc<str>,
    pub closure: Arc<Closure>,
    pub graph: Arc<Graph>,
}

/// What an instance executes: its definition's accepted revision at the
/// revision the instance is at (its pin, or the newest accepted one it
/// follows), shared with the store. Never a newer draft that failed to
/// evaluate, never the current graph in place of a pinned one.
#[derive(Clone, Debug)]
pub struct InstanceExecution {
    pub instance: InstanceId,
    pub definition: String,
    pub revision: u64,
    pub source: Arc<str>,
    pub closure: Arc<Closure>,
    pub graph: Arc<Graph>,
}

impl Definition {
    fn empty(name: &str) -> Self {
        Self {
            name: name.to_string(),
            source: String::new(),
            valid_source: None,
            closure: Arc::default(),
            valid_closure: None,
            ephemeral: false,
            revision: 0,
            graph: None,
            error: None,
            canonical: false,
            tools: ToolSchema::default(),
            ring: VecDeque::new(),
            accepted: Vec::new(),
        }
    }
}

/// One node's live status inside a [`RunRow`]; the wire projection is
/// `NodeRowDto`, which renames `delta_text` to `text`.
#[derive(Clone, Debug)]
pub struct NodeRow {
    pub state: NodeState,
    pub progress: Option<u16>,
    pub stage: Option<String>,
    pub delta_text: String,
    /// Every port the node completed with, in its shape.
    pub outputs: Vec<(String, PortValue)>,
    pub error: Option<String>,
}

impl Default for NodeRow {
    fn default() -> Self {
        Self {
            state: NodeState::Pending,
            progress: None,
            stage: None,
            delta_text: String::new(),
            outputs: Vec::new(),
            error: None,
        }
    }
}

/// How the store's owner reaches a run in flight. `answer` carries at most
/// one value per waiting Ask, because an instance parks on one Ask at a time.
#[derive(Clone)]
pub struct RunControl {
    pub cancel: Arc<AtomicBool>,
    pub answer: Sender<(String, Value)>,
}

/// A driver without room for another run. The input comes back untouched;
/// the store un-registers the run, so a refusal never strands it.
pub struct RunRefused(pub RunInput);

/// Starts runs for a store. The driver must deliver every [`RunEvent`] of a
/// run it started back to the store's owner, which applies it with
/// [`FlowStore::apply_run_event`], in order and without loss.
pub trait RunDriver {
    fn start(
        &mut self,
        input: RunInput,
        instance: &InstanceId,
        flow: &str,
    ) -> Result<RunControl, RunRefused>;
}

/// One run's live status; `handle` reaches the run while it is in flight and
/// is `None` once it finished or while it is still queued.
pub struct RunRow {
    pub instance: InstanceId,
    pub flow: String,
    pub batch: Option<String>,
    pub batch_index: Option<u64>,
    pub revision: u64,
    pub state: RunState,
    pub planned_nodes: Vec<String>,
    pub nodes: BTreeMap<String, NodeRow>,
    /// Every requested result that produced, in its shape.
    pub outputs: BTreeMap<String, PortValue>,
    pub http_log: Vec<HttpLogEntry>,
    pub started_ms: u64,
    pub finished_ms: Option<u64>,
    pub handle: Option<RunControl>,
    /// The `outputs` a queued run was requested with, remembered until it
    /// actually dispatches (§5.4's node-pruned "named tool entry point").
    requested_outputs: Option<Vec<String>>,
}

/// What a store call changed, in the order it happened.
#[derive(Clone, Debug)]
pub enum StoreEvent {
    FlowChanged { name: String, revision: u64, canonical: bool },
    FlowError { name: String, error: EvalError },
    FlowRemoved { name: String },
    InstanceCreated { instance: InstanceId, flow: String },
    InstanceRemoved { instance: InstanceId, flow: String },
    InstanceInputs { instance: InstanceId, flow: String },
    InstanceCleared { instance: InstanceId, flow: String },
    /// A run event after the store applied it. `NodeAnswered.by` already
    /// names the actor that really answered.
    Run { run_id: RunId, instance: InstanceId, flow: String, event: RunEvent },
    Log(String),
}

impl StoreEvent {
    /// Bytes this event keeps alive while queued (see [`RunEvent::payload_bytes`]).
    pub fn payload_bytes(&self) -> usize {
        match self {
            Self::FlowChanged { name, .. } | Self::FlowRemoved { name } => name.len(),
            Self::FlowError { name, error } => name.len() + error.file.len() + error.message.len(),
            Self::InstanceCreated { instance, flow }
            | Self::InstanceRemoved { instance, flow }
            | Self::InstanceInputs { instance, flow }
            | Self::InstanceCleared { instance, flow } => instance.0.len() + flow.len(),
            Self::Run { run_id, instance, flow, event } => {
                run_id.0.len() + instance.0.len() + flow.len() + event.payload_bytes()
            }
            Self::Log(message) => message.len(),
        }
    }
}

#[derive(Clone)]
pub struct SourceResult {
    pub revision: u64,
    pub graph: Option<Graph>,
    pub error: Option<EvalError>,
}

pub enum CreateInstanceOutcome {
    Created(InstanceId),
    FlowNotFound,
    FlowInvalid,
    Error(String),
}

pub enum SetInputsOutcome {
    Ok(InputRefs),
    InstanceNotFound,
    AskNotWaiting,
    Error(String),
}

pub enum StartRunOutcome {
    Started { run_id: String, queued: u64 },
    InstanceNotFound,
    FlowInvalid,
    /// The flow's concurrency is zero.
    Busy,
    /// The run driver has no room, or the instance's run queue is full.
    /// Nothing was registered.
    Refused,
}

pub enum ClearInstanceOutcome {
    Cleared,
    InstanceNotFound,
    Busy,
}

/// Names the evaluation file of a flow (`<root>/flows/<name>.splash` for the
/// HTTP host, the package module path for an embedding host).
pub type SourceLayout = Arc<dyn Fn(&str) -> String + Send + Sync>;

/// Which finished runs publish their outputs as the instance's live outputs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiveOutputs {
    /// Any finished run, whatever revision it ran (the HTTP host's inherited
    /// behaviour).
    AnyRevision,
    /// Only a run of the revision the instance is at. A run that outlived an
    /// edit keeps its result in its run row and does not overwrite the live
    /// instance.
    CurrentRevision,
}

pub struct FlowStoreConfig {
    pub registry: Arc<crate::document::Registry>,
    pub executors: Arc<engine::providers::ExecutorRegistry>,
    pub values: ValueStore,
    pub revision_ring: usize,
    pub instance_ttl: Duration,
    pub origin: (String, u64),
    pub layout: SourceLayout,
    pub live_outputs: LiveOutputs,
    /// Most runs one instance may hold queued behind its concurrency.
    pub max_queued_runs: Option<usize>,
    /// Most finished run rows kept; the oldest go first. `None` keeps them
    /// until the janitor's time-based retention.
    pub max_finished_runs: Option<usize>,
    /// Largest single input value, on every path that sets one.
    pub max_input_bytes: Option<usize>,
    /// Most run rows batches may own at once. A batch keeps its rows until
    /// it is cleared, so this is what bounds that retention.
    pub max_batch_runs: Option<usize>,
}

pub struct FlowStore {
    pub(crate) registry: Arc<crate::document::Registry>,
    executors: Arc<engine::providers::ExecutorRegistry>,
    pub definitions: BTreeMap<String, Definition>,
    pub instances: BTreeMap<InstanceId, Instance>,
    pub runs: BTreeMap<RunId, RunRow>,
    pub values: ValueStore,
    origin: (String, u64),
    layout: SourceLayout,
    /// Evaluation file names given explicitly with a source, by flow name.
    files: BTreeMap<String, String>,
    revision_ring: usize,
    instance_ttl: Duration,
    live_outputs: LiveOutputs,
    max_queued_runs: Option<usize>,
    max_finished_runs: Option<usize>,
    max_input_bytes: Option<usize>,
    pub(crate) max_batch_runs: Option<usize>,
    /// A queued run could not be promoted because the driver refused it;
    /// the owner retries with [`FlowStore::retry_queued`].
    stalled: bool,
    /// The owner is stopping: nothing new starts, nothing queued is promoted.
    shutting_down: bool,
    /// Instance -> debounce deadline (unix ms) for a pending `trigger:
    /// @input` run (§4.1); the owner's fast tick starts it once due.
    pending_input_runs: BTreeMap<InstanceId, u64>,
    /// (run, node) -> the actor that most recently answered that Ask,
    /// consumed the moment the matching `node.answered` event arrives so the
    /// published `by` reflects who really answered rather than the
    /// scheduler's generic "caller" (§3).
    pending_answer_actor: BTreeMap<(RunId, String), String>,
    /// Batches own the lifetime of their slices' instances and run rows
    /// (`runtime::batches`).
    pub(crate) batches: BTreeMap<String, super::batches::BatchRecord>,
    outbox: Vec<StoreEvent>,
}

impl FlowStore {
    pub fn new(config: FlowStoreConfig) -> Self {
        Self {
            registry: config.registry,
            executors: config.executors,
            definitions: BTreeMap::new(),
            instances: BTreeMap::new(),
            runs: BTreeMap::new(),
            values: config.values,
            origin: config.origin,
            layout: config.layout,
            files: BTreeMap::new(),
            revision_ring: config.revision_ring,
            instance_ttl: config.instance_ttl,
            live_outputs: config.live_outputs,
            max_queued_runs: config.max_queued_runs,
            max_finished_runs: config.max_finished_runs,
            max_input_bytes: config.max_input_bytes,
            max_batch_runs: config.max_batch_runs,
            stalled: false,
            shutting_down: false,
            pending_input_runs: BTreeMap::new(),
            pending_answer_actor: BTreeMap::new(),
            batches: BTreeMap::new(),
            outbox: Vec::new(),
        }
    }

    /// Take the events produced since the last drain, oldest first.
    pub fn drain_events(&mut self) -> Vec<StoreEvent> {
        std::mem::take(&mut self.outbox)
    }

    pub fn has_events(&self) -> bool {
        !self.outbox.is_empty()
    }

    fn emit(&mut self, event: StoreEvent) {
        self.outbox.push(event);
    }

    pub fn file_name(&self, flow: &str) -> String {
        self.files
            .get(flow)
            .cloned()
            .unwrap_or_else(|| (self.layout)(flow))
    }

    /// Whether `source` is already the definition's source (no revision
    /// bump, no event); returns the current result when it is. A closure,
    /// when given, must be the definition's current one as well.
    pub fn unchanged(&self, name: &str, source: &str) -> Option<SourceResult> {
        self.unchanged_with(name, source, None)
    }

    pub fn unchanged_with(&self, name: &str, source: &str, closure: Option<&Closure>) -> Option<SourceResult> {
        let existing = self.definitions.get(name)?;
        let same_closure = closure.is_none_or(|closure| *closure == *existing.closure);
        (existing.source == source && same_closure).then(|| SourceResult {
            revision: existing.revision,
            graph: existing.graph.clone(),
            error: existing.error.clone(),
        })
    }

    /// The closure a load of `name` evaluates with when the caller gives
    /// none: the definition's current one (a root edit keeps its imports).
    pub fn closure_for(&self, name: &str, given: Option<Arc<Closure>>) -> Arc<Closure> {
        given.unwrap_or_else(|| {
            self.definitions
                .get(name)
                .map(|definition| definition.closure.clone())
                .unwrap_or_default()
        })
    }

    /// Evaluate `source` and make it the definition `name`, keeping the
    /// definition's current closure.
    pub fn set_source(&mut self, name: String, source: String) -> SourceResult {
        self.set_source_with(name, source, None, None)
    }

    /// As [`Self::set_source`] with an explicit closure (`None` keeps the
    /// current one) and ownership (`None` keeps the current one).
    pub fn set_source_with(
        &mut self,
        name: String,
        source: String,
        closure: Option<Arc<Closure>>,
        ephemeral: Option<bool>,
    ) -> SourceResult {
        if let Some(result) = self.unchanged_with(&name, &source, closure.as_deref()) {
            if let (Some(ephemeral), Some(definition)) = (ephemeral, self.definitions.get_mut(&name)) {
                definition.ephemeral = ephemeral;
            }
            return result;
        }
        let closure = self.closure_for(&name, closure);
        let evaluated = graph::evaluate_with_registry(&source, &self.file_name(&name), &closure, &self.registry);
        self.apply_evaluated_with(name, source, closure, ephemeral, evaluated)
    }

    /// As [`Self::set_source`], evaluating under an explicit file name that
    /// later runs of this flow keep using.
    pub fn set_source_at(&mut self, name: String, source: String, file_name: String) -> SourceResult {
        self.files.insert(name.clone(), file_name);
        self.set_source(name, source)
    }

    /// Record the evaluation file name of a flow whose source is evaluated
    /// elsewhere (see [`Self::apply_evaluated`]).
    pub fn set_file_name(&mut self, name: &str, file_name: String) {
        self.files.insert(name.to_string(), file_name);
    }

    /// Install the result of evaluating `source`, which the caller may have
    /// computed on another thread. Semantics are those of `set_source`.
    pub fn apply_evaluated(
        &mut self,
        name: String,
        source: String,
        evaluated: Result<Graph, EvalError>,
    ) -> SourceResult {
        let closure = self.closure_for(&name, None);
        self.apply_evaluated_with(name, source, closure, None, evaluated)
    }

    /// As [`Self::apply_evaluated`], with the closure `evaluated` was
    /// computed against. A valid result installs source and closure
    /// together; an invalid one records both as the newest draft and leaves
    /// the executable pair (`valid_source`, `valid_closure`) untouched.
    pub fn apply_evaluated_with(
        &mut self,
        name: String,
        source: String,
        closure: Arc<Closure>,
        ephemeral: Option<bool>,
        evaluated: Result<Graph, EvalError>,
    ) -> SourceResult {
        if let Some(result) = self.unchanged_with(&name, &source, Some(&closure)) {
            if let (Some(ephemeral), Some(definition)) = (ephemeral, self.definitions.get_mut(&name)) {
                definition.ephemeral = ephemeral;
            }
            return result;
        }
        let evaluated = evaluated.and_then(|graph| {
            graph::tool_schema_with_registry(&graph, &self.registry)
                .map(|tools| (graph, tools))
                .map_err(|message| EvalError { file: self.file_name(&name), line: 1, col: 1, message })
        });
        match evaluated {
            Ok((mut evaluated, tools)) => {
                let canonical = graph::is_canonical_with_registry(&source, &closure, &self.registry);
                let ring = self.revision_ring;
                let definition = self
                    .definitions
                    .entry(name.clone())
                    .or_insert_with(|| Definition::empty(&name));
                definition.revision = definition.revision.saturating_add(1);
                evaluated.revision = definition.revision;
                definition.source = source.clone();
                definition.valid_source = Some(source.clone());
                definition.closure = closure.clone();
                definition.valid_closure = Some(closure.clone());
                if let Some(ephemeral) = ephemeral {
                    definition.ephemeral = ephemeral;
                }
                definition.graph = Some(evaluated.clone());
                definition.error = None;
                definition.canonical = canonical;
                definition.tools = tools;
                definition.accepted.push(Arc::new(AcceptedRevision {
                    revision: definition.revision,
                    source: Arc::from(source.as_str()),
                    closure: closure.clone(),
                    graph: Arc::new(evaluated.clone()),
                }));
                definition.ring.push_back((definition.revision, source, closure));
                // The history is bounded, but the current revision and every
                // revision a pinned instance or a queued run still needs are
                // kept: the oldest entry nothing holds goes first, and the
                // entry just pushed is never the victim (a pin taken at it
                // later must find it).
                let retained: HashSet<u64> = self
                    .instances
                    .values()
                    .filter(|instance| instance.flow == name && instance.pinned)
                    .map(|instance| instance.revision)
                    .chain(
                        self.runs
                            .values()
                            .filter(|row| row.flow == name && row.state == RunState::Queued)
                            .map(|row| row.revision),
                    )
                    .chain(std::iter::once(definition.revision))
                    .collect();
                while definition.ring.len() > ring {
                    match definition.ring.iter().position(|entry| !retained.contains(&entry.0)) {
                        Some(victim) => {
                            definition.ring.remove(victim);
                        }
                        None => break,
                    }
                }
                definition
                    .accepted
                    .retain(|accepted| definition.ring.iter().any(|entry| entry.0 == accepted.revision));
                let revision = definition.revision;
                self.emit(StoreEvent::FlowChanged { name: name.clone(), revision, canonical });
                self.on_flow_changed(&name, &evaluated);
                SourceResult { revision, graph: Some(evaluated), error: None }
            }
            Err(error) => {
                let definition = self
                    .definitions
                    .entry(name.clone())
                    .or_insert_with(|| Definition::empty(&name));
                definition.source = source;
                definition.closure = closure;
                if let Some(ephemeral) = ephemeral {
                    definition.ephemeral = ephemeral;
                }
                definition.error = Some(error.clone());
                definition.canonical = false;
                let revision = definition.revision;
                let graph = definition.graph.clone();
                self.emit(StoreEvent::FlowError { name, error: error.clone() });
                SourceResult { revision, graph, error: Some(error) }
            }
        }
    }

    pub fn set_load_error(&mut self, name: String, error: EvalError) {
        let definition = self
            .definitions
            .entry(name.clone())
            .or_insert_with(|| Definition::empty(&name));
        definition.error = Some(error.clone());
        definition.canonical = false;
        self.emit(StoreEvent::FlowError { name, error });
    }

    pub fn remove_definition(&mut self, name: &str) -> bool {
        if self.definitions.remove(name).is_none() {
            return false;
        }
        self.files.remove(name);
        self.emit(StoreEvent::FlowRemoved { name: name.to_string() });
        self.drop_instances_of_flow(name);
        true
    }

    /// One instance per `autostart: true` definition, owner `Auto` (§5).
    /// Called once by the owner, right after its initial sources loaded.
    pub fn autostart_all(&mut self) {
        let now = engine::unix_ms();
        let starts: Vec<(String, Graph)> = self
            .definitions
            .values()
            .filter_map(|definition| {
                let graph = definition.graph.as_ref()?;
                graph.autostart.then(|| (definition.name.clone(), graph.clone()))
            })
            .collect();
        for (name, graph) in starts {
            match Instance::new(name.clone(), &graph, None, false, Owner::Auto, now) {
                Ok(instance) => {
                    let id = instance.id.clone();
                    store_instance_inputs(&instance, &mut self.values);
                    self.instances.insert(id.clone(), instance);
                    self.emit(StoreEvent::InstanceCreated { instance: id, flow: name });
                }
                Err(error) => self.emit(StoreEvent::Log(format!("autostart `{name}` failed: {error}"))),
            }
        }
    }

    /// `flow.changed`: every live (unpinned) instance follows the file.
    fn on_flow_changed(&mut self, name: &str, graph: &Graph) {
        let ids: Vec<InstanceId> = self
            .instances
            .iter()
            .filter(|(_, instance)| instance.flow == name && !instance.pinned)
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            let mut stale_run = None;
            let mut failed = None;
            if let Some(instance) = self.instances.get_mut(&id) {
                if let Err(error) = instance.on_graph_changed(graph) {
                    failed = Some(error);
                }
                store_instance_inputs(instance, &mut self.values);
                if self.live_outputs == LiveOutputs::CurrentRevision {
                    // The run that was waiting belongs to the previous
                    // revision; it can no longer be answered through this
                    // instance and must not keep it parked.
                    stale_run = instance.waiting.take().map(|waiting| waiting.run);
                }
            }
            if let Some(error) = failed {
                self.emit(StoreEvent::Log(format!("instance {} on_graph_changed: {error}", id.0)));
            }
            if let Some(handle) = stale_run
                .and_then(|run| self.runs.get(&run))
                .and_then(|row| row.handle.as_ref())
            {
                handle.cancel.store(true, Ordering::SeqCst);
            }
        }
    }

    /// `flow.removed`: every instance of that flow is dropped, its runs
    /// cancelled (§4.4).
    fn drop_instances_of_flow(&mut self, flow: &str) {
        let doomed: Vec<InstanceId> = self
            .instances
            .iter()
            .filter(|(_, instance)| instance.flow == flow)
            .map(|(id, _)| id.clone())
            .collect();
        for id in doomed {
            self.cancel_instance_runs(&id);
            self.instances.remove(&id);
            self.emit(StoreEvent::InstanceRemoved { instance: id, flow: flow.to_string() });
        }
    }

    fn cancel_instance_runs(&mut self, id: &InstanceId) {
        let Some(instance) = self.instances.get(id) else {
            return;
        };
        let run_ids: Vec<RunId> = instance
            .active
            .iter()
            .cloned()
            .chain(instance.runs.iter().cloned())
            .collect();
        for run_id in run_ids {
            self.cancel_run_row(&run_id);
        }
    }

    /// Flip the cancel flag on a live run (its driver finishes it
    /// asynchronously and `apply_run_event` does the rest), or finish a
    /// still-queued run in place, since it never started.
    fn cancel_run_row(&mut self, run_id: &RunId) {
        let Some(row) = self.runs.get(run_id) else {
            return;
        };
        if let Some(handle) = &row.handle {
            handle.cancel.store(true, Ordering::SeqCst);
            return;
        }
        if matches!(row.state, RunState::Done | RunState::Failed | RunState::Cancelled) {
            return;
        }
        let instance_id = row.instance.clone();
        let flow = row.flow.clone();
        if let Some(row) = self.runs.get_mut(run_id) {
            row.state = RunState::Cancelled;
            row.finished_ms = Some(engine::unix_ms());
        }
        if let Some(instance) = self.instances.get_mut(&instance_id) {
            instance.runs.retain(|queued| queued != run_id);
            instance.active.retain(|active| active != run_id);
        }
        self.publish_run_event(
            run_id,
            &instance_id,
            &flow,
            RunEvent::RunFinished {
                state: RunState::Cancelled,
                secs: 0.0,
                outputs: Vec::new(),
                http_log: Vec::new(),
                warnings: Vec::new(),
            },
        );
        self.trim_finished_runs();
    }

    /// Emit one run event, substituting the real answering actor into a
    /// `node.answered` the scheduler stamped generically (§3).
    fn publish_run_event(
        &mut self,
        run_id: &RunId,
        instance_id: &InstanceId,
        flow: &str,
        mut event: RunEvent,
    ) {
        if let RunEvent::NodeAnswered { node, by } = &mut event {
            if let Some(actor) = self
                .pending_answer_actor
                .remove(&(run_id.clone(), node.clone()))
            {
                *by = actor;
            }
        }
        self.emit(StoreEvent::Run {
            run_id: run_id.clone(),
            instance: instance_id.clone(),
            flow: flow.to_string(),
            event,
        });
    }

    /// Apply one event of a run the driver started. A finished run may
    /// promote the instance's next queued run through `driver`.
    pub fn apply_run_event(
        &mut self,
        run_id: RunId,
        instance_id: InstanceId,
        flow: String,
        event: RunEvent,
        driver: &mut dyn RunDriver,
    ) {
        match &event {
            RunEvent::RunStarted { planned_nodes, .. } => {
                if let Some(row) = self.runs.get_mut(&run_id) {
                    row.planned_nodes = planned_nodes.clone();
                }
            }
            RunEvent::NodeStarted { node } => {
                if let Some(row) = self.runs.get_mut(&run_id) {
                    row.nodes.entry(node.clone()).or_default().state = NodeState::Running;
                }
            }
            RunEvent::NodeProgress { node, permille, stage, .. } => {
                if let Some(row) = self.runs.get_mut(&run_id) {
                    let entry = row.nodes.entry(node.clone()).or_default();
                    entry.progress = *permille;
                    entry.stage = Some(stage.clone());
                }
            }
            RunEvent::NodeDelta { node, text, .. } => {
                if let Some(row) = self.runs.get_mut(&run_id) {
                    let entry = row.nodes.entry(node.clone()).or_default();
                    entry.delta_text.push_str(text);
                    if entry.delta_text.len() > MAX_DELTA_TEXT {
                        let mut cut = entry.delta_text.len() - MAX_DELTA_TEXT;
                        while !entry.delta_text.is_char_boundary(cut) {
                            cut += 1;
                        }
                        entry.delta_text.drain(..cut);
                    }
                }
            }
            RunEvent::NodeWaiting { node, question, ty, options } => {
                if let Some(row) = self.runs.get_mut(&run_id) {
                    row.nodes.entry(node.clone()).or_default().state = NodeState::Waiting;
                    row.state = RunState::Waiting;
                }
                let run_revision = self.runs.get(&run_id).map(|row| row.revision);
                let mut stale = false;
                if let Some(instance) = self.instances.get_mut(&instance_id) {
                    let live = match self.live_outputs {
                        LiveOutputs::AnyRevision => true,
                        LiveOutputs::CurrentRevision => run_revision == Some(instance.revision),
                    };
                    if live {
                        instance.waiting = Some(Waiting {
                            run: run_id.clone(),
                            node: node.clone(),
                            question: question.clone(),
                            ty: ty.clone(),
                            options: options.clone(),
                        });
                    } else {
                        stale = true;
                    }
                }
                if stale {
                    // The instance moved on to another revision, so nobody
                    // can answer this Ask through it. Cancel the run instead
                    // of letting it park the edited instance or wait forever.
                    if let Some(handle) = self.runs.get(&run_id).and_then(|row| row.handle.as_ref()) {
                        handle.cancel.store(true, Ordering::SeqCst);
                    }
                }
            }
            RunEvent::NodeAnswered { node, .. } => {
                if let Some(instance) = self.instances.get_mut(&instance_id) {
                    if instance
                        .waiting
                        .as_ref()
                        .is_some_and(|waiting| &waiting.node == node && waiting.run == run_id)
                    {
                        instance.waiting = None;
                    }
                }
            }
            RunEvent::NodeDone { node, outputs } => {
                for (_, value) in outputs {
                    for element in value.values() {
                        self.values.put(element.clone());
                    }
                }
                if let Some(row) = self.runs.get_mut(&run_id) {
                    let entry = row.nodes.entry(node.clone()).or_default();
                    entry.state = NodeState::Done;
                    entry.outputs = outputs.clone();
                    entry.error = None;
                }
            }
            RunEvent::NodeFailed { node, error } => {
                if let Some(row) = self.runs.get_mut(&run_id) {
                    let entry = row.nodes.entry(node.clone()).or_default();
                    entry.state = NodeState::Failed;
                    entry.error = Some(error.clone());
                }
            }
            RunEvent::NodeSkipped { node, reason } => {
                if let Some(row) = self.runs.get_mut(&run_id) {
                    let entry = row.nodes.entry(node.clone()).or_default();
                    entry.state = NodeState::Skipped;
                    entry.error = Some(reason.clone());
                }
            }
            RunEvent::RunFinished { state, outputs, http_log, .. } => {
                let now = engine::unix_ms();
                let run_revision = self.runs.get(&run_id).map(|row| row.revision);
                if let Some(row) = self.runs.get_mut(&run_id) {
                    row.state = *state;
                    row.outputs = outputs.iter().cloned().collect();
                    row.http_log = http_log.clone();
                    row.finished_ms = Some(now);
                    row.handle = None;
                    // Output nodes emit no NodeDone of their own: the run's
                    // outputs are their values. A run that finished Done
                    // completes them here, so the stored row (what a later
                    // read or replay sees) agrees with what the live stream
                    // showed. Any other ending leaves them as they were:
                    // a failed or cancelled run has no done outputs.
                    if *state == RunState::Done {
                        for (node, value) in outputs {
                            let entry = row.nodes.entry(node.clone()).or_default();
                            if matches!(entry.state, NodeState::Pending | NodeState::Running | NodeState::Waiting) {
                                entry.state = NodeState::Done;
                                entry.progress = Some(1000);
                                entry.outputs = vec![("value".to_string(), value.clone())];
                                entry.error = None;
                            }
                        }
                    }
                    // A cancelled run's live nodes were cancelled with it;
                    // the row must not keep showing them mid-denoise.
                    if *state == RunState::Cancelled {
                        for node in row.nodes.values_mut() {
                            if matches!(
                                node.state,
                                NodeState::Pending | NodeState::Running | NodeState::Waiting
                            ) {
                                node.state = NodeState::Cancelled;
                            }
                        }
                    }
                }
                if let Some(instance) = self.instances.get_mut(&instance_id) {
                    instance.active.retain(|active| active != &run_id);
                    instance.last_activity_ms = now;
                    let live = match self.live_outputs {
                        LiveOutputs::AnyRevision => true,
                        LiveOutputs::CurrentRevision => run_revision == Some(instance.revision),
                    };
                    if *state == RunState::Done && live {
                        for (node, value) in outputs {
                            instance.outputs.insert(node.clone(), value.clone());
                        }
                    }
                    if instance.waiting.as_ref().is_some_and(|waiting| waiting.run == run_id) {
                        instance.waiting = None;
                    }
                }
                self.pending_answer_actor.retain(|(run, _), _| run != &run_id);
                self.dispatch_next_queued(&instance_id, driver);
                self.trim_finished_runs();
            }
        }
        self.publish_run_event(&run_id, &instance_id, &flow, event);
    }

    /// Keep at most `max_finished_runs` finished rows, dropping the oldest.
    fn trim_finished_runs(&mut self) {
        let Some(limit) = self.max_finished_runs else {
            return;
        };
        // A batch keeps its rows until the queue clears it.
        let mut finished: Vec<(u64, RunId)> = self
            .runs
            .iter()
            .filter(|(_, row)| row.batch.as_ref().is_none_or(|batch| !self.batches.contains_key(batch)))
            .filter_map(|(run_id, row)| row.finished_ms.map(|finished| (finished, run_id.clone())))
            .collect();
        if finished.len() <= limit {
            return;
        }
        finished.sort();
        let excess = finished.len() - limit;
        for (_, run_id) in finished.into_iter().take(excess) {
            self.runs.remove(&run_id);
        }
    }

    /// Whether a queued run is waiting for the driver to have room again.
    pub fn is_stalled(&self) -> bool {
        self.stalled
    }

    /// Promote queued runs the driver refused earlier. The owner calls this
    /// when its driver may have room again.
    pub fn retry_queued(&mut self, driver: &mut dyn RunDriver) {
        if !self.stalled || self.shutting_down {
            return;
        }
        self.stalled = false;
        let ready: Vec<InstanceId> = self
            .instances
            .iter()
            .filter(|(_, instance)| !instance.runs.is_empty() && instance.would_start())
            .map(|(id, _)| id.clone())
            .collect();
        for id in ready {
            self.dispatch_next_queued(&id, driver);
        }
    }

    fn dispatch_next_queued(&mut self, instance_id: &InstanceId, driver: &mut dyn RunDriver) {
        if self.shutting_down {
            return;
        }
        let Some(instance) = self.instances.get_mut(instance_id) else {
            return;
        };
        let Some(next_run_id) = instance.runs.pop_front() else {
            return;
        };
        instance.active.push(next_run_id.clone());
        let flow = instance.flow.clone();
        let Some(row) = self.runs.get(&next_run_id) else {
            return;
        };
        let revision = row.revision;
        let requested_outputs = row.requested_outputs.clone();
        match self.source_and_graph_for_revision(&flow, revision) {
            Some((source, closure, mut graph)) => {
                graph.revision = revision;
                let dispatched = self.dispatch_run(
                    instance_id.clone(),
                    next_run_id.clone(),
                    flow,
                    graph,
                    source,
                    closure,
                    requested_outputs,
                    driver,
                );
                if !dispatched {
                    // Back to the head of the queue, still `Queued`.
                    if let Some(instance) = self.instances.get_mut(instance_id) {
                        instance.active.retain(|active| active != &next_run_id);
                        instance.runs.push_front(next_run_id);
                    }
                    self.stalled = true;
                }
            }
            None => {
                self.fail_run_to_dispatch(
                    &next_run_id,
                    instance_id,
                    &flow,
                    "flow revision is no longer available",
                );
            }
        }
    }

    /// The executable triple of a kept revision: the graph as it was
    /// accepted, the source it came from and the closure it evaluated with.
    /// Never an invalid newer draft, and never an evaluation done again.
    fn source_and_graph_for_revision(&self, flow: &str, revision: u64) -> Option<(String, Arc<Closure>, Graph)> {
        let accepted = self.accepted_revision(flow, revision)?;
        Some((accepted.source.to_string(), accepted.closure.clone(), (*accepted.graph).clone()))
    }

    fn accepted_revision(&self, flow: &str, revision: u64) -> Option<&Arc<AcceptedRevision>> {
        self.definitions
            .get(flow)?
            .accepted
            .iter()
            .find(|accepted| accepted.revision == revision)
    }

    /// What `id` executes now: its definition's accepted revision at the
    /// instance's revision. `None` for an unknown instance or a revision the
    /// ring no longer keeps; the current graph is never substituted.
    pub fn instance_execution(&self, id: &InstanceId) -> Option<Arc<InstanceExecution>> {
        let instance = self.instances.get(id)?;
        let accepted = self.accepted_revision(&instance.flow, instance.revision)?;
        Some(Arc::new(InstanceExecution {
            instance: id.clone(),
            definition: instance.flow.clone(),
            revision: accepted.revision,
            source: accepted.source.clone(),
            closure: accepted.closure.clone(),
            graph: accepted.graph.clone(),
        }))
    }

    fn fail_run_to_dispatch(&mut self, run_id: &RunId, instance_id: &InstanceId, flow: &str, reason: &str) {
        let now = engine::unix_ms();
        if let Some(row) = self.runs.get_mut(run_id) {
            row.state = RunState::Failed;
            row.finished_ms = Some(now);
        }
        if let Some(instance) = self.instances.get_mut(instance_id) {
            instance.active.retain(|active| active != run_id);
        }
        self.publish_run_event(
            run_id,
            instance_id,
            flow,
            RunEvent::RunFinished {
                state: RunState::Failed,
                secs: 0.0,
                outputs: Vec::new(),
                http_log: Vec::new(),
                warnings: vec![reason.to_string()],
            },
        );
        self.trim_finished_runs();
    }

    /// Start (or, for a run already queued with a `RunRow`, promote) a run
    /// through the driver.
    #[allow(clippy::too_many_arguments)]
    fn dispatch_run(
        &mut self,
        instance_id: InstanceId,
        run_id: RunId,
        flow: String,
        graph: Graph,
        source: String,
        closure: Arc<Closure>,
        outputs: Option<Vec<String>>,
        driver: &mut dyn RunDriver,
    ) -> bool {
        let file_name = self.file_name(&flow);
        let revision = graph.revision;
        let inputs = self
            .instances
            .get(&instance_id)
            .map(|instance| instance.inputs.clone())
            .unwrap_or_default();
        let mut planned_nodes: Vec<String> =
            engine::scheduler::selected_nodes(&graph, outputs.as_deref())
                .into_iter()
                .collect();
        planned_nodes.sort();
        let input = RunInput {
            run_id: run_id.clone(),
            instance: instance_id.0.clone(),
            source,
            file_name,
            closure,
            registry: self.registry.clone(),
            executors: self.executors.clone(),
            graph_revision: revision,
            graph,
            inputs,
            outputs,
            origin: self.origin.clone(),
        };
        let Ok(handle) = driver.start(input, &instance_id, &flow) else {
            return false;
        };
        let now = engine::unix_ms();
        use std::collections::btree_map::Entry;
        match self.runs.entry(run_id) {
            Entry::Occupied(mut occupied) => {
                let row = occupied.get_mut();
                row.state = RunState::Running;
                row.started_ms = now;
                row.handle = Some(handle);
            }
            Entry::Vacant(vacant) => {
                vacant.insert(RunRow {
                    instance: instance_id,
                    flow,
                    batch: None,
                    batch_index: None,
                    revision,
                    state: RunState::Running,
                    planned_nodes,
                    nodes: BTreeMap::new(),
                    outputs: BTreeMap::new(),
                    http_log: Vec::new(),
                    started_ms: now,
                    finished_ms: None,
                    handle: Some(handle),
                    requested_outputs: None,
                });
            }
        }
        true
    }

    pub fn create_instance(
        &mut self,
        flow: &str,
        label: Option<String>,
        pin: bool,
        owner: Owner,
        inputs: HashMap<String, HashMap<String, InputValueDto>>,
    ) -> CreateInstanceOutcome {
        let Some(definition) = self.definitions.get(flow) else {
            return CreateInstanceOutcome::FlowNotFound;
        };
        let Some(graph) = definition.graph.clone() else {
            return CreateInstanceOutcome::FlowInvalid;
        };
        let now = engine::unix_ms();
        let mut instance = match Instance::new(flow.to_string(), &graph, label, pin, owner, now) {
            Ok(instance) => instance,
            Err(error) => return CreateInstanceOutcome::Error(error),
        };
        // Validate the entire request before resolving any media bytes.
        for (node_id, ports) in &inputs {
            let Some(node) = graph.nodes.iter().find(|node| &node.id == node_id) else {
                return CreateInstanceOutcome::Error(format!("node `{node_id}` is not declared"));
            };
            for (port, dto) in ports {
                if let Err(error) = check_input_dto(node, port, dto, &self.values, &self.registry, self.max_input_bytes) {
                    return CreateInstanceOutcome::Error(error);
                }
            }
        }
        for (node, ports) in &inputs {
            for (port, dto) in ports {
                let value = match port_value_from_dto(dto, &mut self.values) {
                    Ok(value) => value,
                    Err(error) => return CreateInstanceOutcome::Error(error),
                };
                if let Err(error) = self.check_input_size(node, port, &value) {
                    return CreateInstanceOutcome::Error(error);
                }
                if let Err(error) = instance.set_input(node, port, value, &graph) {
                    return CreateInstanceOutcome::Error(error);
                }
            }
        }
        let id = instance.id.clone();
        store_instance_inputs(&instance, &mut self.values);
        self.instances.insert(id.clone(), instance);
        self.emit(StoreEvent::InstanceCreated { instance: id.clone(), flow: flow.to_string() });
        CreateInstanceOutcome::Created(id)
    }

    /// The one input size rule, whatever path a value arrives by: a literal,
    /// a stored digest, or a value the embedding host built itself.
    fn check_input_size(&self, node: &str, port: &str, value: &PortValue) -> Result<(), String> {
        self.registry.types().require_artifact(&value.ty()).map_err(|error| error.to_string())?;
        match self.max_input_bytes {
            Some(limit) if value.bytes_len() > limit => {
                Err(format!("input `{node}.{port}` exceeds {limit} bytes"))
            }
            _ => Ok(()),
        }
    }

    /// Run rows the batches own right now.
    pub fn batch_runs(&self) -> usize {
        self.batches.values().map(|batch| batch.runs.len()).sum()
    }

    pub fn instance_row(&self, id: &InstanceId) -> Option<InstanceRow> {
        self.instances.get(id).map(instance_row_dto)
    }

    pub fn list_instance_rows(&self, flow: Option<&str>, waiting_only: bool) -> Vec<InstanceRow> {
        self.instances
            .values()
            .filter(|instance| flow.is_none_or(|flow| instance.flow == flow))
            .filter(|instance| !waiting_only || instance.waiting.is_some())
            .map(instance_row_dto)
            .collect()
    }

    pub fn delete_instance(&mut self, id: &InstanceId) -> bool {
        let Some(instance) = self.instances.get(id) else {
            return false;
        };
        let flow = instance.flow.clone();
        self.cancel_instance_runs(id);
        self.instances.remove(id);
        self.emit(StoreEvent::InstanceRemoved { instance: id.clone(), flow });
        true
    }

    /// Forget only state produced by runs. User-entered/default inputs and
    /// the flow definition remain intact.
    pub fn clear_instance(&mut self, id: &InstanceId) -> ClearInstanceOutcome {
        let Some(instance) = self.instances.get(id) else {
            return ClearInstanceOutcome::InstanceNotFound;
        };
        if !instance.active.is_empty() || !instance.runs.is_empty() {
            return ClearInstanceOutcome::Busy;
        }
        let flow = instance.flow.clone();
        let run_ids: HashSet<RunId> = self
            .runs
            .iter()
            .filter(|(_, row)| &row.instance == id)
            .map(|(run_id, _)| run_id.clone())
            .collect();
        self.runs.retain(|run_id, _| !run_ids.contains(run_id));
        self.pending_answer_actor
            .retain(|(run_id, _), _| !run_ids.contains(run_id));
        self.pending_input_runs.remove(id);
        if let Some(instance) = self.instances.get_mut(id) {
            instance.outputs.clear();
            instance.waiting = None;
            instance.last_activity_ms = engine::unix_ms();
        }
        self.emit(StoreEvent::InstanceCleared { instance: id.clone(), flow });
        ClearInstanceOutcome::Cleared
    }

    /// Set inputs from wire DTOs (see [`Self::set_instance_values`]).
    pub fn set_instance_inputs(
        &mut self,
        id: &InstanceId,
        raw: HashMap<String, HashMap<String, InputValueDto>>,
        actor: &str,
    ) -> SetInputsOutcome {
        let graph = match self.instance_graph(id) {
            Ok((_, graph)) => graph,
            Err(outcome) => return outcome,
        };
        // Check every target and DTO before reading media from the value
        // store. The instance remains untouched until the resolved batch
        // has also passed `set_instance_values`' checks.
        let mut declared = Vec::new();
        for (node_id, ports) in &raw {
            let Some(node_def) = graph.nodes.iter().find(|node| &node.id == node_id) else {
                return SetInputsOutcome::Error(format!("node `{node_id}` is not declared"));
            };
            declared.push((node_def, ports));
        }
        for &(node, ports) in &declared {
            for (port, dto) in ports {
                if node.kind == "ask" && !self.ask_is_waiting(id, &node.id) {
                    return SetInputsOutcome::AskNotWaiting;
                }
                if let Err(error) = check_input_dto(node, port, dto, &self.values, &self.registry, self.max_input_bytes) {
                    return SetInputsOutcome::Error(error);
                }
            }
        }
        let mut inputs = Vec::new();
        for (node, ports) in declared {
            for (port, dto) in ports {
                match port_value_from_dto(dto, &mut self.values) {
                    Ok(value) => inputs.push((node.id.clone(), port.clone(), value)),
                    Err(error) => return SetInputsOutcome::Error(error),
                }
            }
        }
        self.set_instance_values(id, inputs, actor)
    }

    /// The graph an instance's inputs are validated against: the revision the
    /// instance is at. For a live instance that is the definition's current
    /// valid graph; for a pinned one it is the pinned revision, not whatever
    /// the file has become since.
    fn instance_graph(&self, id: &InstanceId) -> Result<(String, Graph), SetInputsOutcome> {
        let Some(instance) = self.instances.get(id) else {
            return Err(SetInputsOutcome::InstanceNotFound);
        };
        let flow = instance.flow.clone();
        match self.source_and_graph_for_revision(&flow, instance.revision) {
            Some((_, _, graph)) => Ok((flow, graph)),
            None if instance.pinned => Err(SetInputsOutcome::Error(
                "the instance's pinned revision is no longer available".to_string(),
            )),
            None => Err(SetInputsOutcome::Error("flow has no valid graph".to_string())),
        }
    }

    fn ask_is_waiting(&self, id: &InstanceId, node_id: &str) -> bool {
        self.instances
            .get(id)
            .and_then(|instance| instance.waiting.as_ref())
            .is_some_and(|waiting| waiting.node == node_id)
    }

    /// Set `(node, port, value)` inputs as one batch. The whole batch is
    /// validated before any of it is applied, so a rejected batch changes
    /// nothing and cannot consume a waiting Ask. An Ask answer reaches its
    /// run, an `@input` flow debounces a run, and the instance reports
    /// `instance.inputs`.
    pub fn set_instance_values(
        &mut self,
        id: &InstanceId,
        inputs: Vec<(String, String, PortValue)>,
        actor: &str,
    ) -> SetInputsOutcome {
        let (flow, graph) = match self.instance_graph(id) {
            Ok(found) => found,
            Err(outcome) => return outcome,
        };
        let Some(instance) = self.instances.get(id) else {
            return SetInputsOutcome::InstanceNotFound;
        };

        // Validate everything first. One batch writes a port at most once:
        // two values for one port (two answers to one Ask, say) have no
        // defined winner, so the batch is rejected rather than half-honoured.
        for (index, (node_id, port, _)) in inputs.iter().enumerate() {
            if inputs[..index].iter().any(|(node, other, _)| node == node_id && other == port) {
                return SetInputsOutcome::Error(format!(
                    "input `{node_id}.{port}` is written more than once in one batch"
                ));
            }
        }
        for (node_id, port, value) in &inputs {
            let Some(node_def) = graph.nodes.iter().find(|node| &node.id == node_id) else {
                return SetInputsOutcome::Error(format!("node `{node_id}` is not declared"));
            };
            // A Choose's `pick` answers it while it waits and is an ordinary
            // input otherwise; an Ask takes values only while it waits.
            let answers_choice = node_def.kind == "choose"
                && instance.waiting.as_ref().is_some_and(|waiting| &waiting.node == node_id);
            if node_def.kind == "ask" || answers_choice {
                let Some(waiting) = instance.waiting.as_ref().filter(|waiting| &waiting.node == node_id) else {
                    return SetInputsOutcome::AskNotWaiting;
                };
                let reachable = self
                    .runs
                    .get(&waiting.run)
                    .is_some_and(|row| row.handle.is_some());
                if !reachable {
                    return SetInputsOutcome::Error(format!(
                        "the run waiting on `{node_id}` is no longer reachable"
                    ));
                }
            }
            if let Err(error) = self.check_input_size(node_id, port, value) {
                return SetInputsOutcome::Error(error);
            }
            if let Err(error) = instance.check_input(node_id, port, value, &graph) {
                return SetInputsOutcome::Error(error);
            }
        }

        // Apply. Nothing below can reject an entry the checks above passed.
        let had_inputs = !inputs.is_empty();
        let mut trigger_run = false;
        let mut answers: Vec<(RunId, String, Value)> = Vec::new();
        for (node_id, port, value) in inputs {
            for element in value.values() {
                self.values.put(element.clone());
            }
            let Some(instance) = self.instances.get_mut(id) else {
                return SetInputsOutcome::InstanceNotFound;
            };
            match instance.set_input(&node_id, &port, value.clone(), &graph) {
                Ok(InputEffect::None) => {}
                Ok(InputEffect::TriggerRun) => trigger_run = true,
                Ok(InputEffect::Answered(run_id)) => match value.one() {
                    // An Ask's port is a one port; its shape check above
                    // already refused anything else.
                    Some(answer) => answers.push((run_id, node_id, answer.clone())),
                    None => return SetInputsOutcome::Error(format!("the answer to `{node_id}` must be one value")),
                },
                Err(error) => return SetInputsOutcome::Error(error),
            }
        }

        let mut undelivered = None;
        for (run_id, node, value) in answers {
            self.pending_answer_actor.insert((run_id.clone(), node.clone()), actor.to_string());
            let delivered = self
                .runs
                .get(&run_id)
                .and_then(|row| row.handle.as_ref())
                .is_some_and(|handle| handle.answer.send((node.clone(), value)).is_ok());
            if !delivered {
                self.pending_answer_actor.remove(&(run_id, node.clone()));
                undelivered = Some(node);
            }
        }
        if trigger_run {
            let deadline = engine::unix_ms() + INPUT_DEBOUNCE_MS;
            self.pending_input_runs.insert(id.clone(), deadline);
        }
        if had_inputs {
            self.emit(StoreEvent::InstanceInputs { instance: id.clone(), flow });
        }
        if let Some(node) = undelivered {
            // The run ended between the check and the send; its RunFinished
            // settles the instance. The caller must know the answer was late.
            return SetInputsOutcome::Error(format!(
                "the run waiting on `{node}` ended before the answer reached it"
            ));
        }
        let refs = self.instances.get(id).map(|instance| input_refs(&instance.inputs)).unwrap_or_default();
        SetInputsOutcome::Ok(refs)
    }

    /// Request a run. A live instance runs the definition's current valid
    /// graph; a pinned instance runs the revision it was pinned at. Either
    /// way the run executes the source that graph was evaluated from, never
    /// a newer invalid draft.
    pub fn start_run(
        &mut self,
        id: &InstanceId,
        outputs: Option<Vec<String>>,
        driver: &mut dyn RunDriver,
    ) -> StartRunOutcome {
        if self.shutting_down {
            return StartRunOutcome::Refused;
        }
        let Some((flow, revision)) = self
            .instances
            .get(id)
            .map(|instance| (instance.flow.clone(), instance.revision))
        else {
            return StartRunOutcome::InstanceNotFound;
        };
        let Some((source, closure, mut graph)) = self.source_and_graph_for_revision(&flow, revision) else {
            return StartRunOutcome::FlowInvalid;
        };
        graph.revision = revision;
        let Some(instance) = self.instances.get_mut(id) else {
            return StartRunOutcome::InstanceNotFound;
        };
        if let Some(limit) = self.max_queued_runs {
            if instance.would_queue() && instance.runs.len() >= limit {
                return StartRunOutcome::Refused;
            }
        }
        match instance.request_run(outputs.clone()) {
            RunDecision::Busy => StartRunOutcome::Busy,
            RunDecision::Queued(queued) => {
                let run_id = instance.runs.back().expect("just queued").clone();
                let mut planned_nodes: Vec<String> =
                    engine::scheduler::selected_nodes(&graph, outputs.as_deref())
                        .into_iter()
                        .collect();
                planned_nodes.sort();
                self.runs.insert(
                    run_id.clone(),
                    RunRow {
                        instance: id.clone(),
                        flow,
                        batch: None,
                        batch_index: None,
                        revision: graph.revision,
                        state: RunState::Queued,
                        planned_nodes,
                        nodes: BTreeMap::new(),
                        outputs: BTreeMap::new(),
                        http_log: Vec::new(),
                        started_ms: engine::unix_ms(),
                        finished_ms: None,
                        handle: None,
                        requested_outputs: outputs,
                    },
                );
                StartRunOutcome::Started { run_id: run_id.0, queued: queued as u64 }
            }
            RunDecision::Start(run_id) => {
                if self.dispatch_run(id.clone(), run_id.clone(), flow, graph, source, closure, outputs, driver) {
                    StartRunOutcome::Started { run_id: run_id.0, queued: 0 }
                } else {
                    // The driver had no room: the run was never registered.
                    if let Some(instance) = self.instances.get_mut(id) {
                        instance.active.retain(|active| active != &run_id);
                    }
                    StartRunOutcome::Refused
                }
            }
        }
    }

    pub fn run_row(&self, run_id: &RunId) -> Option<RunRowDto> {
        self.runs.get(run_id).map(|row| run_row_dto(run_id, row))
    }

    pub fn list_run_rows(&self, instance: Option<&InstanceId>) -> Vec<RunRowDto> {
        self.runs
            .iter()
            .filter(|(_, row)| instance.is_none_or(|instance| &row.instance == instance))
            .map(|(run_id, row)| run_row_dto(run_id, row))
            .collect()
    }

    pub fn cancel_run(&mut self, run_id: &RunId) -> bool {
        if !self.runs.contains_key(run_id) {
            return false;
        }
        self.cancel_run_row(run_id);
        true
    }

    /// Begin shutting down: nothing starts from here on. Every run in flight
    /// gets its cancel flag (it still reports its own `RunFinished`), every
    /// queued run is finished `Cancelled` in place, and pending `@input`
    /// triggers are dropped. Returns the number of runs still in flight.
    pub fn begin_shutdown(&mut self) -> usize {
        self.shutting_down = true;
        self.stalled = false;
        self.pending_input_runs.clear();
        let mut in_flight = 0;
        let mut queued = Vec::new();
        for (run_id, row) in &self.runs {
            if let Some(handle) = &row.handle {
                handle.cancel.store(true, Ordering::SeqCst);
                in_flight += 1;
            } else if matches!(row.state, RunState::Queued) {
                queued.push(run_id.clone());
            }
        }
        for run_id in queued {
            self.cancel_run_row(&run_id);
        }
        in_flight
    }

    pub fn is_shutting_down(&self) -> bool {
        self.shutting_down
    }

    /// Finish, as `Cancelled`, every run the driver still holds when the
    /// owner gives up waiting for it. Returns how many were abandoned.
    pub fn abandon_in_flight(&mut self, reason: &str) -> usize {
        let abandoned: Vec<(RunId, InstanceId, String)> = self
            .runs
            .iter()
            .filter(|(_, row)| row.handle.is_some())
            .map(|(run_id, row)| (run_id.clone(), row.instance.clone(), row.flow.clone()))
            .collect();
        let count = abandoned.len();
        for (run_id, instance_id, flow) in abandoned {
            let now = engine::unix_ms();
            if let Some(row) = self.runs.get_mut(&run_id) {
                row.state = RunState::Cancelled;
                row.finished_ms = Some(now);
                row.handle = None;
                for node in row.nodes.values_mut() {
                    if matches!(node.state, NodeState::Pending | NodeState::Running | NodeState::Waiting) {
                        node.state = NodeState::Cancelled;
                    }
                }
            }
            if let Some(instance) = self.instances.get_mut(&instance_id) {
                instance.active.retain(|active| active != &run_id);
                if instance.waiting.as_ref().is_some_and(|waiting| waiting.run == run_id) {
                    instance.waiting = None;
                }
            }
            self.publish_run_event(
                &run_id,
                &instance_id,
                &flow,
                RunEvent::RunFinished {
                    state: RunState::Cancelled,
                    secs: 0.0,
                    outputs: Vec::new(),
                    http_log: Vec::new(),
                    warnings: vec![reason.to_string()],
                },
            );
        }
        self.trim_finished_runs();
        count
    }

    /// When the next debounced `@input` run is due, if any (unix ms).
    pub fn next_debounce_deadline(&self) -> Option<u64> {
        self.pending_input_runs.values().min().copied()
    }

    /// The fast tick: start any `trigger: @input` run whose debounce has
    /// elapsed. Cheap to call often.
    pub fn run_debounced_inputs(&mut self, now_ms: u64, driver: &mut dyn RunDriver) {
        if self.shutting_down {
            return;
        }
        let due: Vec<InstanceId> = self
            .pending_input_runs
            .iter()
            .filter(|(_, deadline)| **deadline <= now_ms)
            .map(|(id, _)| id.clone())
            .collect();
        for id in due {
            self.pending_input_runs.remove(&id);
            if self.instances.contains_key(&id) {
                if let StartRunOutcome::Refused = self.start_run(&id, None, driver) {
                    // No room right now: the trigger stays pending.
                    self.pending_input_runs.insert(id, now_ms + INPUT_DEBOUNCE_MS);
                }
            }
        }
    }

    /// The slow tick: expire values past TTL, finished runs past retention,
    /// idle instances past the instance TTL (§5.2). `keep_runs` and
    /// `keep_instances` are exempt, and so are application-owned and
    /// autostart instances and everything a batch owns.
    pub fn janitor_sweep(&mut self, keep_runs: &HashSet<RunId>, keep_instances: &HashSet<InstanceId>) {
        let now_ms = engine::unix_ms();
        let now = SystemTime::now();
        let mut keep_runs = keep_runs.clone();
        let mut keep_instances = keep_instances.clone();
        for run in self.batches.values().flat_map(|batch| &batch.runs) {
            keep_runs.insert(RunId(run.run_id.clone()));
            keep_instances.insert(InstanceId(run.instance.clone()));
        }

        let mut live_digests: HashSet<[u8; 32]> = HashSet::new();
        for instance in self.instances.values() {
            for ports in instance.inputs.values() {
                for value in ports.values() {
                    live_digests.extend(value.digests());
                }
            }
            for value in instance.outputs.values() {
                live_digests.extend(value.digests());
            }
        }
        for row in self.runs.values() {
            for value in row.outputs.values() {
                live_digests.extend(value.digests());
            }
            for node in row.nodes.values() {
                for (_, value) in &node.outputs {
                    live_digests.extend(value.digests());
                }
            }
        }
        self.values.expire(now, &live_digests);

        self.runs.retain(|run_id, row| {
            keep_runs.contains(run_id)
                || row
                    .finished_ms
                    .is_none_or(|finished| now_ms.saturating_sub(finished) < RUN_RETENTION_MS)
        });

        let ttl_ms = self.instance_ttl.as_millis() as u64;
        let doomed: Vec<(InstanceId, String)> = self
            .instances
            .iter()
            .filter(|(_, instance)| {
                !matches!(instance.owner, Owner::Auto | Owner::Application)
                    && !keep_instances.contains(&instance.id)
                    && instance.waiting.is_none()
                    && instance.active.is_empty()
                    && now_ms.saturating_sub(instance.last_activity_ms) >= ttl_ms
            })
            .map(|(id, instance)| (id.clone(), instance.flow.clone()))
            .collect();
        for (id, flow) in doomed {
            self.instances.remove(&id);
            self.emit(StoreEvent::InstanceRemoved { instance: id, flow });
        }
    }

    pub fn get_value(&mut self, digest: &[u8; 32]) -> Option<Value> {
        self.values.get(digest)
    }

    pub fn get_value_typed(&mut self, digest: &[u8; 32], ty: &PortType) -> Option<Value> {
        self.values.get_typed(digest, ty)
    }

    pub fn put_value(&mut self, value: Value) -> [u8; 32] {
        self.values.put(value)
    }
}

fn owner_str(owner: &Owner) -> &'static str {
    match owner {
        Owner::Tab => "tab",
        Owner::Chat { .. } => "chat",
        Owner::Service => "service",
        Owner::Auto => "auto",
        Owner::Application => "application",
    }
}

/// `waiting`, `running`, `queued` or `idle`.
pub fn instance_state_str(instance: &Instance) -> &'static str {
    if instance.waiting.is_some() {
        "waiting"
    } else if !instance.active.is_empty() {
        "running"
    } else if !instance.runs.is_empty() {
        "queued"
    } else {
        "idle"
    }
}

/// The wire split of grouped port values: one ports in the scalar maps as
/// ever, ordered ports beside them.
pub fn input_refs(inputs: &BTreeMap<String, BTreeMap<String, PortValue>>) -> InputRefs {
    let mut refs = InputRefs::default();
    for (node, ports) in inputs {
        for (port, value) in ports {
            match value {
                PortValue::One(value) => {
                    refs.inputs.entry(node.clone()).or_default().insert(port.clone(), ValueRef::from(value));
                }
                PortValue::Seq(sequence) => {
                    refs.ordered_inputs
                        .entry(node.clone())
                        .or_default()
                        .insert(port.clone(), SequenceRef::from(sequence));
                }
            }
        }
    }
    refs
}

fn split_results(values: &BTreeMap<String, PortValue>) -> (HashMap<String, ValueRef>, HashMap<String, SequenceRef>) {
    let mut ones = HashMap::new();
    let mut sequences = HashMap::new();
    for (node, value) in values {
        match value {
            PortValue::One(value) => {
                ones.insert(node.clone(), ValueRef::from(value));
            }
            PortValue::Seq(sequence) => {
                sequences.insert(node.clone(), SequenceRef::from(sequence));
            }
        }
    }
    (ones, sequences)
}

pub fn instance_row_dto(instance: &Instance) -> InstanceRow {
    let InputRefs { inputs, ordered_inputs } = input_refs(&instance.inputs);
    let (outputs, ordered_outputs) = split_results(&instance.outputs);
    InstanceRow {
        instance: instance.id.0.clone(),
        flow: instance.flow.clone(),
        label: instance.label.clone(),
        revision: instance.revision,
        live: !instance.pinned,
        state: instance_state_str(instance).to_string(),
        run: instance.active.first().map(|run| run.0.clone()),
        inputs,
        outputs,
        ordered_inputs: Some(ordered_inputs),
        ordered_outputs: Some(ordered_outputs),
        waiting: instance.waiting.as_ref().map(|waiting| WaitingDto {
            node: waiting.node.clone(),
            question: waiting.question.clone(),
            ty: waiting.ty.clone(),
            options: waiting.options.clone(),
        }),
        owner: owner_str(&instance.owner).to_string(),
        created_ms: instance.created_ms,
        last_activity_ms: instance.last_activity_ms,
        subscribers: 0,
    }
}

pub fn run_row_dto(run_id: &RunId, row: &RunRow) -> RunRowDto {
    RunRowDto {
        run_id: run_id.0.clone(),
        instance: row.instance.0.clone(),
        flow: row.flow.clone(),
        batch: row.batch.clone(),
        batch_index: row.batch_index,
        revision: row.revision,
        state: row.state,
        planned_nodes: row.planned_nodes.clone(),
        nodes: row
            .nodes
            .iter()
            .map(|(id, node)| {
                (
                    id.clone(),
                    NodeRowDto {
                        state: node.state,
                        progress: node.progress,
                        stage: node.stage.clone(),
                        outputs: node
                            .outputs
                            .iter()
                            .filter_map(|(port, value)| {
                                value.one().map(|value| PortValueRef { port: port.clone(), value: value.into() })
                            })
                            .collect(),
                        sequences: {
                            let sequences: Vec<PortSequenceRef> = node
                                .outputs
                                .iter()
                                .filter_map(|(port, value)| match value {
                                    PortValue::Seq(sequence) => Some(PortSequenceRef {
                                        port: port.clone(),
                                        ty: sequence.ty.clone(),
                                        values: sequence.values.iter().map(Into::into).collect(),
                                    }),
                                    PortValue::One(_) => None,
                                })
                                .collect();
                            (!sequences.is_empty()).then_some(sequences)
                        },
                        error: node.error.clone(),
                        text: (!node.delta_text.is_empty()).then(|| node.delta_text.clone()),
                    },
                )
            })
            .collect(),
        outputs: split_results(&row.outputs).0,
        sequences: {
            let sequences = split_results(&row.outputs).1;
            (!sequences.is_empty()).then_some(sequences)
        },
        http_log: row.http_log.iter().map(Into::into).collect(),
        started_ms: row.started_ms,
        finished_ms: row.finished_ms,
    }
}

/// Every published input reference is resolvable before its first run,
/// including authored defaults and newly seeded inputs after an edit.
fn store_instance_inputs(instance: &Instance, values: &mut ValueStore) {
    for ports in instance.inputs.values() {
        for value in ports.values() {
            for element in value.values() {
                values.put(element.clone());
            }
        }
    }
}

/// Admit a DTO using only its structure and stored value metadata. In
/// particular, a rejected sequence never expands its media digests first.
fn check_input_dto(
    node: &crate::Node,
    port: &str,
    dto: &InputValueDto,
    values: &ValueStore,
    registry: &crate::document::Registry,
    max_bytes: Option<usize>,
) -> Result<(), String> {
    let shape = crate::instance::input_shape(node, port)
        .ok_or_else(|| format!("node `{}` does not declare input port `{port}`", node.id))?;
    registry.types().require_artifact(&dto.ty).map_err(|error| error.to_string())?;
    let mismatch = |error| format!("type mismatch for `{}.{port}`: {error}", node.id);
    match (dto.values.as_ref(), shape.many) {
        (None, Some(_)) => {
            return Err(mismatch(format!("expected an ordered {} sequence, got one value", shape.ty.as_str())));
        }
        (Some(_), None) => {
            return Err(mismatch(format!("expected one {} value, got a sequence", shape.ty.as_str())));
        }
        (Some(elements), Some(bound)) => {
            if dto.text.is_some() || dto.json.is_some() || dto.digest.is_some() {
                return Err("an ordered value carries `values` only, not `text`, `json` or `digest`".to_string());
            }
            if elements.len() > MAX_SEQUENCE as usize {
                return Err(format!("{} values, over the {MAX_SEQUENCE} limit", elements.len()));
            }
            if elements.len() > bound as usize {
                return Err(mismatch(format!("{} values, over the bound of {bound}", elements.len())));
            }
            for (index, element) in elements.iter().enumerate() {
                if element.values.is_some() {
                    return Err(format!("element {} of an ordered value cannot itself be ordered", index + 1));
                }
                if element.ty != dto.ty {
                    return Err(format!(
                        "element {} is {}, in a sequence of {}",
                        index + 1,
                        element.ty.as_str(),
                        dto.ty.as_str()
                    ));
                }
            }
        }
        (None, None) => {}
    }
    if dto.ty != shape.ty {
        return Err(mismatch(format!("expected {}, got {}", shape.ty.as_str(), dto.ty.as_str())));
    }
    let elements = dto.values.as_deref().unwrap_or_else(|| std::slice::from_ref(dto));
    let mut remaining = max_bytes;
    for (index, element) in elements.iter().enumerate() {
        let bytes = input_value_size(element, values).map_err(|error| {
            if dto.values.is_some() { format!("element {}: {error}", index + 1) } else { error }
        })?;
        if let Some(available) = remaining.as_mut() {
            let Some(rest) = available.checked_sub(bytes) else {
                return Err(format!("input `{}.{port}` exceeds {} bytes", node.id, max_bytes.unwrap()));
            };
            *available = rest;
        }
    }
    Ok(())
}

fn input_value_size(dto: &InputValueDto, values: &ValueStore) -> Result<usize, String> {
    let digest_only = dto.digest.is_some() && dto.text.is_none() && dto.json.is_none() && dto.values.is_none();
    match &dto.ty {
        ty if ty.is_media() || digest_only => {
            let digest_text = dto.digest.as_deref().ok_or_else(|| "media value requires `digest`".to_string())?;
            let digest = parse_prefixed_digest(digest_text).ok_or_else(|| "malformed digest".to_string())?;
            let (stored_ty, bytes) = values.type_and_size_typed(&digest, ty)
                .ok_or_else(|| format!("unknown {} value digest", ty.as_str()))?;
            if stored_ty != *ty {
                return Err(format!("digest is a {} value, not {}", stored_ty.as_str(), ty.as_str()));
            }
            Ok(bytes)
        }
        PortType::Text => dto.text.as_ref().map(String::len).ok_or_else(|| "text value requires `text`".to_string()),
        PortType::Json | PortType::List => {
            let json = dto.json.as_ref().ok_or_else(|| format!("{} value requires `json`", dto.ty.as_str()))?;
            if dto.ty == PortType::List && !matches!(json, JsonValue::Array(_)) {
                return Err("list value requires a json array".to_string());
            }
            Ok(json.serialize_json().len())
        }
        ty => Err(format!("unsupported input type `{}`", ty.as_str())),
    }
}

/// Resolve a DTO after the whole request passed `check_input_dto`.
fn port_value_from_dto(dto: &InputValueDto, values: &mut ValueStore) -> Result<PortValue, String> {
    let Some(elements) = dto.values.as_ref() else {
        return value_from_dto(dto, values).map(PortValue::One);
    };
    let mut sequence = Sequence::empty(dto.ty.clone());
    for (index, element) in elements.iter().enumerate() {
        sequence.values.push(value_from_dto(element, values).map_err(|error| format!("element {}: {error}", index + 1))?);
    }
    Ok(PortValue::Seq(sequence))
}

fn value_from_dto(dto: &InputValueDto, values: &mut ValueStore) -> Result<Value, String> {
    // A complete digest reference can carry any type. Literal text/json
    // keep their existing routes when those payload fields are present.
    let digest_only = dto.digest.is_some() && dto.text.is_none() && dto.json.is_none() && dto.values.is_none();
    match &dto.ty {
        ty if ty.is_media() || digest_only => {
            let digest_text = dto.digest.as_deref().ok_or_else(|| "media value requires `digest`".to_string())?;
            let digest = parse_prefixed_digest(digest_text).ok_or_else(|| "malformed digest".to_string())?;
            let value = values.get_typed(&digest, ty)
                .ok_or_else(|| format!("unknown {} value digest", ty.as_str()))?;
            if value.ty != *ty {
                return Err(format!("digest is a {} value, not {}", value.ty.as_str(), ty.as_str()));
            }
            Ok(value)
        }
        PortType::Text => {
            let text = dto.text.as_deref().ok_or_else(|| "text value requires `text`".to_string())?;
            Ok(Value::text(text))
        }
        PortType::Json => {
            let json = dto.json.as_ref().ok_or_else(|| "json value requires `json`".to_string())?;
            Ok(Value::json(json.serialize_json()))
        }
        PortType::List => {
            let json = dto.json.as_ref().ok_or_else(|| "list value requires `json`".to_string())?;
            if !matches!(json, JsonValue::Array(_)) {
                return Err("list value requires a json array".to_string());
            }
            Ok(Value::list(json.serialize_json()))
        }
        ty => Err(format!("unsupported input type `{}`", ty.as_str())),
    }
}

/// Accepts both the bare hex digest and the `sha256:<hex>` form the design
/// prose uses for a stored input reference.
fn parse_prefixed_digest(text: &str) -> Option<[u8; 32]> {
    from_hex_32(text.strip_prefix("sha256:").unwrap_or(text))
}
