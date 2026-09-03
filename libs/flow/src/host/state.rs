use super::config::{FlowServerConfig, SharedConfig};
use super::events::EventHub;
use super::models::FleetSnapshot;
use super::util::{atomic_write, log};
use super::ServerError;
use crate::engine::{self, HttpLogEntry, NetPolicy, RunEvent, RunHandle, RunId, RunInput, Seams};
use crate::instance::{InputEffect, Instance, InstanceId, Owner, RunDecision};
use crate::values::{Value, ValueStore};
use crate::{
    graph, EvalError, Graph, InputValueDto, InstanceRow, NodeRowDto, NodeState, NodeTypeCatalog,
    ModelsResponse, PortType, PortValueRef, RunEventPayload, RunRowDto, RunState, ToolSchema,
    ValueRef, WaitingDto,
};
use makepad_micro_serde::{DeJson, JsonValue, SerJson};
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{mpsc, Arc};
use std::time::{Duration, SystemTime};

pub(crate) const MAX_SOURCE_BYTES: u64 = graph::MAX_SOURCE_BYTES as u64;

#[derive(Clone, Debug)]
pub struct Definition {
    pub name: String,
    pub source: String,
    pub revision: u64,
    pub graph: Option<Graph>,
    pub error: Option<EvalError>,
    pub canonical: bool,
    pub tools: ToolSchema,
    pub ring: VecDeque<(u64, String)>,
}

/// One node's live status inside a `RunRow`; the wire projection is
/// `NodeRowDto` (`host/routes.rs`), which renames `delta_text` to `text`.
#[derive(Clone, Debug)]
pub struct NodeRow {
    pub state: NodeState,
    pub progress: Option<u16>,
    pub stage: Option<String>,
    pub delta_text: String,
    pub outputs: Vec<(String, Value)>,
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

/// A cap on the accumulated `node.delta` text kept per node (§3 of the
/// brief: "text? (delta so far, ≤ 16 KB)"). Truncated from the front so the
/// most recent tokens stay visible.
const MAX_DELTA_TEXT: usize = 16 * 1024;

/// One run's live status; `handle` is the live thread + cancel flag +
/// answer channel while the run is in flight, `None` once finished.
pub struct RunRow {
    pub instance: InstanceId,
    pub flow: String,
    pub revision: u64,
    pub state: RunState,
    pub planned_nodes: Vec<String>,
    pub nodes: BTreeMap<String, NodeRow>,
    pub outputs: BTreeMap<String, Value>,
    pub http_log: Vec<HttpLogEntry>,
    pub started_ms: u64,
    pub finished_ms: Option<u64>,
    pub handle: Option<RunHandle>,
    /// The `outputs` a queued run was requested with, remembered until it
    /// actually dispatches (§5.4's node-pruned "named tool entry point").
    requested_outputs: Option<Vec<String>>,
}

/// Handed from the state thread to the run-events thread (`host/server.rs`)
/// each time a run starts, so that one shared thread can forward every
/// run's events into `StateHandle::call` (§5.2's "one thread role added").
pub(crate) struct RunRegistration {
    pub run_id: RunId,
    pub instance: InstanceId,
    pub flow: String,
    pub receiver: mpsc::Receiver<RunEvent>,
}

pub struct FlowState {
    pub definitions: BTreeMap<String, Definition>,
    pub events: Arc<EventHub>,
    pub epoch: u64,
    pub(crate) catalog: Vec<NodeTypeCatalog>,
    pub(crate) fleet: FleetSnapshot,
    pub instances: BTreeMap<InstanceId, Instance>,
    pub runs: BTreeMap<RunId, RunRow>,
    pub values: ValueStore,
    pub(crate) seams: Seams,
    pub(crate) net: NetPolicy,
    pub(crate) origin: (String, u64),
    config: SharedConfig,
    root: PathBuf,
    revision_ring: usize,
    temp_serial: u64,
    instance_ttl: Duration,
    run_register_tx: mpsc::Sender<RunRegistration>,
    /// Instance -> debounce deadline (unix ms) for a pending `trigger:
    /// @input` run (§4.1); the janitor's fast tick starts it once due.
    pending_input_runs: BTreeMap<InstanceId, u64>,
    /// (run, node) -> the actor that most recently answered that Ask
    /// through `PUT .../inputs`, consumed the moment the matching
    /// `node.answered` event arrives so the published `by` reflects who
    /// really answered rather than the engine's generic "caller" (§3).
    pending_answer_actor: BTreeMap<(RunId, String), String>,
}

/// The real seams (`FleetGen` + `HubChat` + `HubHttp`), or whatever a test
/// injected via `FlowServerConfig::with_seams` — real by construction
/// unless a caller opts out.
fn build_seams(config: &FlowServerConfig) -> Seams {
    if let Some(seams) = config.seams() {
        return seams;
    }
    Seams {
        chat: build_chat_seam(&config.chat_model),
        gen: Arc::new(engine::FleetGen),
        http: Arc::new(engine::HubHttp),
    }
}

#[cfg(feature = "hub-chat")]
fn build_chat_seam(
    chat_model: &Option<PathBuf>,
) -> Arc<dyn crate::engine::executors::chat::ChatSeam> {
    Arc::new(engine::HubChat {
        model_path: chat_model.clone().unwrap_or_default(),
    })
}

#[cfg(not(feature = "hub-chat"))]
fn build_chat_seam(
    _chat_model: &Option<PathBuf>,
) -> Arc<dyn crate::engine::executors::chat::ChatSeam> {
    Arc::new(engine::HubChat::from_env())
}

#[derive(Clone)]
pub(crate) struct SourceResult {
    pub revision: u64,
    pub graph: Option<Graph>,
    pub error: Option<EvalError>,
}

impl FlowState {
    pub(crate) fn models_response(&mut self, domain: Option<&str>) -> ModelsResponse {
        self.fleet.response(&self.config.fleet_hint, domain)
    }

    pub(crate) fn catalog_with_models(&mut self) -> Vec<NodeTypeCatalog> {
        self.fleet.catalog(&self.config.fleet_hint, &self.catalog)
    }

    fn build(
        config: &SharedConfig,
        events: Arc<EventHub>,
        epoch: u64,
        origin: (String, u64),
        run_register_tx: mpsc::Sender<RunRegistration>,
    ) -> Result<Self, ServerError> {
        let catalog = graph::prelude_catalog().map_err(ServerError::Prelude)?;
        let mut values = ValueStore::new(config.root.join("values"));
        values.ram_budget = config.values_ram_budget;
        values.ttl = Duration::from_secs(config.value_ttl_secs);
        let mut state = Self {
            definitions: BTreeMap::new(),
            events,
            epoch,
            catalog,
            fleet: FleetSnapshot::default(),
            instances: BTreeMap::new(),
            runs: BTreeMap::new(),
            values,
            seams: build_seams(config),
            net: config.net.clone(),
            origin,
            config: config.clone(),
            root: config.root.clone(),
            revision_ring: config.revision_ring,
            temp_serial: 0,
            instance_ttl: Duration::from_secs(config.instance_ttl_secs),
            run_register_tx,
            pending_input_runs: BTreeMap::new(),
            pending_answer_actor: BTreeMap::new(),
        };
        let mut files = Vec::new();
        for entry in std::fs::read_dir(config.root.join("flows"))
            .map_err(|error| ServerError::io("read flows directory", error))?
        {
            let entry = entry.map_err(|error| ServerError::io("read flow entry", error))?;
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("splash") {
                continue;
            }
            let Some(name) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            if super::routes::valid_name(name) {
                files.push((name.to_string(), path));
            }
        }
        files.sort_by(|a, b| a.0.cmp(&b.0));
        for (name, path) in files {
            let metadata = std::fs::metadata(&path)
                .map_err(|error| ServerError::io("stat flow source", error))?;
            if metadata.len() > MAX_SOURCE_BYTES {
                state.set_load_error(name, graph::source_size_error(&path.display().to_string()));
                continue;
            }
            let source = std::fs::read_to_string(&path)
                .map_err(|error| ServerError::io("read flow source", error))?;
            state.set_source(name, source);
        }
        state.autostart_all();
        Ok(state)
    }

    /// One instance per `autostart: true` definition, owner `Auto` (§5).
    /// Called once, right after every `<root>/flows/*.splash` file has
    /// loaded, so it never fires again on a later `flow.changed`.
    fn autostart_all(&mut self) {
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
                    self.instances.insert(id.clone(), instance);
                    self.publish_instance_lifecycle("instance.created", &id, &name);
                }
                Err(error) => log(&self.config, &format!("autostart `{name}` failed: {error}")),
            }
        }
    }

    pub(crate) fn put_source(&mut self, name: String, source: String) -> Result<SourceResult, ServerError> {
        self.temp_serial = self.temp_serial.wrapping_add(1);
        let path = self.root.join("flows").join(format!("{name}.splash"));
        atomic_write(&path, source.as_bytes(), self.temp_serial)?;
        Ok(self.set_source(name, source))
    }

    pub(crate) fn create_source(
        &mut self,
        name: String,
        source: String,
    ) -> Result<Option<SourceResult>, ServerError> {
        let path = self.root.join("flows").join(format!("{name}.splash"));
        if self.definitions.contains_key(&name) || path.exists() {
            return Ok(None);
        }
        self.put_source(name, source).map(Some)
    }

    pub(crate) fn revert(&mut self, name: &str, revision: u64) -> Result<Option<SourceResult>, ServerError> {
        let source = self
            .definitions
            .get(name)
            .and_then(|definition| definition.ring.iter().find(|entry| entry.0 == revision))
            .map(|entry| entry.1.clone());
        match source {
            Some(source) => self.put_source(name.to_string(), source).map(Some),
            None => Ok(None),
        }
    }

    pub(crate) fn remove(&mut self, name: &str) -> Result<bool, ServerError> {
        let path = self.root.join("flows").join(format!("{name}.splash"));
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(ServerError::io("remove flow source", error)),
        }
        Ok(self.remove_definition(name))
    }

    pub(crate) fn set_source(&mut self, name: String, source: String) -> SourceResult {
        if let Some(existing) = self.definitions.get(&name) {
            if existing.source == source {
                return SourceResult {
                    revision: existing.revision,
                    graph: existing.graph.clone(),
                    error: existing.error.clone(),
                };
            }
        }
        let file = self.root.join("flows").join(format!("{name}.splash"));
        match graph::evaluate(&source, &file.display().to_string()) {
            Ok(mut evaluated) => {
                let canonical = graph::is_canonical(&source);
                let definition = self.definitions.entry(name.clone()).or_insert_with(|| Definition {
                    name: name.clone(),
                    source: String::new(),
                    revision: 0,
                    graph: None,
                    error: None,
                    canonical: false,
                    tools: ToolSchema::default(),
                    ring: VecDeque::new(),
                });
                definition.revision = definition.revision.saturating_add(1);
                evaluated.revision = definition.revision;
                let tools = graph::tool_schema(&evaluated);
                definition.source = source.clone();
                definition.graph = Some(evaluated.clone());
                definition.error = None;
                definition.canonical = canonical;
                definition.tools = tools;
                definition.ring.push_back((definition.revision, source));
                while definition.ring.len() > self.revision_ring {
                    definition.ring.pop_front();
                }
                let revision = definition.revision;
                self.events.publish(
                    "flows",
                    "flow.changed",
                    JsonValue::Object(HashMap::from([
                        ("name".to_string(), JsonValue::String(name.clone())),
                        ("revision".to_string(), JsonValue::U64(revision)),
                        ("canonical".to_string(), JsonValue::Bool(canonical)),
                    ])),
                );
                self.on_flow_changed(&name, &evaluated);
                SourceResult { revision, graph: Some(evaluated), error: None }
            }
            Err(error) => {
                let definition = self.definitions.entry(name.clone()).or_insert_with(|| Definition {
                    name: name.clone(),
                    source: String::new(),
                    revision: 0,
                    graph: None,
                    error: None,
                    canonical: false,
                    tools: ToolSchema::default(),
                    ring: VecDeque::new(),
                });
                definition.source = source;
                definition.error = Some(error.clone());
                definition.canonical = false;
                let revision = definition.revision;
                self.events.publish(
                    "flows",
                    "flow.error",
                    JsonValue::Object(HashMap::from([
                        ("name".to_string(), JsonValue::String(name)),
                        ("error".to_string(), json_value(&error)),
                    ])),
                );
                SourceResult { revision, graph: definition.graph.clone(), error: Some(error) }
            }
        }
    }

    /// Apply a watcher snapshot only while it is still the bytes on disk.
    /// A PUT may have replaced the file after the watcher read it but before
    /// this serialized closure runs; that stale snapshot must never win.
    pub(crate) fn set_watched_source(&mut self, name: String, source: String) {
        let path = self.root.join("flows").join(format!("{name}.splash"));
        if std::fs::read_to_string(path).ok().as_deref() == Some(source.as_str()) {
            self.set_source(name, source);
        }
    }

    pub(crate) fn set_watched_oversize(&mut self, name: String, _error: EvalError) {
        let path = self.root.join("flows").join(format!("{name}.splash"));
        if std::fs::metadata(&path).is_ok_and(|metadata| metadata.len() > MAX_SOURCE_BYTES) {
            self.set_load_error(name, graph::source_size_error(&path.display().to_string()));
        }
    }

    pub(crate) fn remove_watched(&mut self, name: &str) {
        let path = self.root.join("flows").join(format!("{name}.splash"));
        if !path.exists() {
            self.remove_definition(name);
        }
    }

    pub(crate) fn set_load_error(&mut self, name: String, error: EvalError) {
        let definition = self.definitions.entry(name.clone()).or_insert_with(|| Definition {
            name: name.clone(),
            source: String::new(),
            revision: 0,
            graph: None,
            error: None,
            canonical: false,
            tools: ToolSchema::default(),
            ring: VecDeque::new(),
        });
        definition.error = Some(error.clone());
        definition.canonical = false;
        self.events.publish(
            "flows",
            "flow.error",
            JsonValue::Object(HashMap::from([
                ("name".to_string(), JsonValue::String(name)),
                ("error".to_string(), json_value(&error)),
            ])),
        );
    }

    pub(crate) fn remove_definition(&mut self, name: &str) -> bool {
        if self.definitions.remove(name).is_none() {
            return false;
        }
        self.events.publish(
            "flows",
            "flow.removed",
            JsonValue::Object(HashMap::from([(
                "name".to_string(),
                JsonValue::String(name.to_string()),
            )])),
        );
        self.drop_instances_of_flow(name);
        true
    }
}

/// Every input change starts a run this many ms after the last one (§4.1).
const INPUT_DEBOUNCE_MS: u64 = 250;
/// A finished run's row is kept this long for `GET /v1/runs/{id}` (§5.2).
const RUN_RETENTION_MS: u64 = 60 * 60 * 1000;

pub(crate) enum CreateInstanceOutcome {
    Created(InstanceId),
    FlowNotFound,
    FlowInvalid,
    Error(String),
}

pub(crate) enum SetInputsOutcome {
    Ok(HashMap<String, HashMap<String, ValueRef>>),
    InstanceNotFound,
    AskNotWaiting,
    Error(String),
}

pub(crate) enum StartRunOutcome {
    Started { run_id: String, queued: u64 },
    InstanceNotFound,
    FlowInvalid,
    Busy,
}

/// Instances, runs, values: the F2 lane's slice of `FlowState`.
impl FlowState {
    fn file_path(&self, flow: &str) -> PathBuf {
        self.root.join("flows").join(format!("{flow}.splash"))
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
            if let Some(instance) = self.instances.get_mut(&id) {
                if let Err(error) = instance.on_graph_changed(graph) {
                    log(&self.config, &format!("instance {} on_graph_changed: {error}", id.0));
                }
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
            self.publish_instance_lifecycle("instance.removed", &id, flow);
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

    /// Flip the cancel flag on a live run (the run thread finishes it
    /// asynchronously and `apply_run_event` does the rest), or finish a
    /// still-queued run in place — it never had a thread to cancel.
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
            &RunEvent::RunFinished {
                state: RunState::Cancelled,
                secs: 0.0,
                outputs: Vec::new(),
                http_log: Vec::new(),
                warnings: Vec::new(),
            },
        );
    }

    fn publish_instance_lifecycle(&self, kind: &str, id: &InstanceId, flow: &str) {
        self.events.publish(
            "flows",
            kind,
            JsonValue::Object(HashMap::from([
                ("instance".to_string(), JsonValue::String(id.0.clone())),
                ("flow".to_string(), JsonValue::String(flow.to_string())),
            ])),
        );
    }

    /// Publish one run event to `run` (always) and, at the granularity
    /// §4.3 names, to `instance` too; substitutes the real answering actor
    /// into a `node.answered` the engine stamped generically (§3).
    fn publish_run_event(
        &mut self,
        run_id: &RunId,
        instance_id: &InstanceId,
        flow: &str,
        event: &RunEvent,
    ) {
        let mut payload = event.to_wire();
        if let RunEventPayload::NodeAnswered { node, by } = &mut payload {
            if let Some(actor) = self
                .pending_answer_actor
                .remove(&(run_id.clone(), node.clone()))
            {
                *by = actor;
            }
        }
        let kind = run_event_kind(&payload);
        let json = run_event_json(run_id, instance_id, flow, &payload);
        self.events.publish("run", kind, json.clone());
        if matches!(
            payload,
            RunEventPayload::NodeDone { .. }
                | RunEventPayload::NodeWaiting { .. }
                | RunEventPayload::NodeAnswered { .. }
                | RunEventPayload::RunStarted { .. }
                | RunEventPayload::RunFinished { .. }
        ) {
            self.events.publish("instance", kind, json);
        }
    }

    /// Called by the run-events thread (`host/server.rs`) via
    /// `StateHandle::call` for every event a run thread sent.
    pub(crate) fn apply_run_event(
        &mut self,
        run_id: RunId,
        instance_id: InstanceId,
        flow: String,
        event: RunEvent,
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
            RunEvent::NodeProgress { node, permille, stage } => {
                if let Some(row) = self.runs.get_mut(&run_id) {
                    let entry = row.nodes.entry(node.clone()).or_default();
                    entry.progress = Some(*permille);
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
                if let Some(instance) = self.instances.get_mut(&instance_id) {
                    instance.waiting = Some(crate::instance::Waiting {
                        run: run_id.clone(),
                        node: node.clone(),
                        question: question.clone(),
                        ty: *ty,
                        options: options.clone(),
                    });
                }
            }
            RunEvent::NodeAnswered { node, .. } => {
                if let Some(instance) = self.instances.get_mut(&instance_id) {
                    if instance.waiting.as_ref().is_some_and(|waiting| &waiting.node == node) {
                        instance.waiting = None;
                    }
                }
            }
            RunEvent::NodeDone { node, outputs } => {
                for (_, value) in outputs {
                    self.values.put(value.clone());
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
                if let Some(row) = self.runs.get_mut(&run_id) {
                    row.state = *state;
                    row.outputs = outputs.iter().cloned().collect();
                    row.http_log = http_log.clone();
                    row.finished_ms = Some(now);
                    row.handle = None;
                }
                if let Some(instance) = self.instances.get_mut(&instance_id) {
                    instance.active.retain(|active| active != &run_id);
                    instance.last_activity_ms = now;
                    if *state == RunState::Done {
                        for (node, value) in outputs {
                            instance.outputs.insert(node.clone(), value.clone());
                        }
                    }
                    if instance.waiting.as_ref().is_some_and(|waiting| waiting.run == run_id) {
                        instance.waiting = None;
                    }
                }
                self.pending_answer_actor.retain(|(run, _), _| run != &run_id);
                self.dispatch_next_queued(&instance_id);
            }
        }
        self.publish_run_event(&run_id, &instance_id, &flow, &event);
    }

    fn dispatch_next_queued(&mut self, instance_id: &InstanceId) {
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
            Some((source, mut graph)) => {
                graph.revision = revision;
                self.dispatch_run(instance_id.clone(), next_run_id, flow, graph, source, requested_outputs);
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

    fn source_and_graph_for_revision(&self, flow: &str, revision: u64) -> Option<(String, Graph)> {
        let definition = self.definitions.get(flow)?;
        if definition.revision == revision {
            return Some((definition.source.clone(), definition.graph.clone()?));
        }
        let source = definition
            .ring
            .iter()
            .find(|entry| entry.0 == revision)?
            .1
            .clone();
        let file = self.file_path(flow);
        let graph = graph::evaluate(&source, &file.display().to_string()).ok()?;
        Some((source, graph))
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
            &RunEvent::RunFinished {
                state: RunState::Failed,
                secs: 0.0,
                outputs: Vec::new(),
                http_log: Vec::new(),
                warnings: vec![reason.to_string()],
            },
        );
    }

    /// Spawn (or, for a run already queued with a `RunRow`, promote) a run
    /// and register its event receiver with the run-events thread.
    fn dispatch_run(
        &mut self,
        instance_id: InstanceId,
        run_id: RunId,
        flow: String,
        graph: Graph,
        source: String,
        outputs: Option<Vec<String>>,
    ) {
        let file_name = self.file_path(&flow).display().to_string();
        let revision = graph.revision;
        let (tx, rx) = mpsc::channel::<RunEvent>();
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
            graph_revision: revision,
            graph,
            inputs,
            outputs,
            origin: self.origin.clone(),
        };
        let handle = engine::spawn_run_with_policy(input, self.seams.clone(), tx, self.net.clone());
        let now = engine::unix_ms();
        use std::collections::btree_map::Entry;
        match self.runs.entry(run_id.clone()) {
            Entry::Occupied(mut occupied) => {
                let row = occupied.get_mut();
                row.state = RunState::Running;
                row.started_ms = now;
                row.handle = Some(handle);
            }
            Entry::Vacant(vacant) => {
                vacant.insert(RunRow {
                    instance: instance_id.clone(),
                    flow: flow.clone(),
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
        let _ = self.run_register_tx.send(RunRegistration {
            run_id,
            instance: instance_id,
            flow,
            receiver: rx,
        });
    }

    pub(crate) fn create_instance(
        &mut self,
        flow: &str,
        label: Option<String>,
        pin: bool,
        inputs: HashMap<String, HashMap<String, InputValueDto>>,
    ) -> CreateInstanceOutcome {
        let Some(definition) = self.definitions.get(flow) else {
            return CreateInstanceOutcome::FlowNotFound;
        };
        let Some(graph) = definition.graph.clone() else {
            return CreateInstanceOutcome::FlowInvalid;
        };
        let now = engine::unix_ms();
        let mut instance = match Instance::new(flow.to_string(), &graph, label, pin, Owner::Tab, now) {
            Ok(instance) => instance,
            Err(error) => return CreateInstanceOutcome::Error(error),
        };
        for (node, ports) in &inputs {
            for (port, dto) in ports {
                let value = match value_from_dto(dto, &mut self.values) {
                    Ok(value) => value,
                    Err(error) => return CreateInstanceOutcome::Error(error),
                };
                if let Err(error) = instance.set_input(node, port, value, &graph) {
                    return CreateInstanceOutcome::Error(error);
                }
            }
        }
        let id = instance.id.clone();
        self.instances.insert(id.clone(), instance);
        self.publish_instance_lifecycle("instance.created", &id, flow);
        CreateInstanceOutcome::Created(id)
    }

    pub(crate) fn instance_row(&self, id: &InstanceId) -> Option<InstanceRow> {
        self.instances.get(id).map(instance_row_dto)
    }

    pub(crate) fn list_instance_rows(&self, flow: Option<&str>, waiting_only: bool) -> Vec<InstanceRow> {
        self.instances
            .values()
            .filter(|instance| flow.is_none_or(|flow| instance.flow == flow))
            .filter(|instance| !waiting_only || instance.waiting.is_some())
            .map(instance_row_dto)
            .collect()
    }

    pub(crate) fn delete_instance(&mut self, id: &InstanceId) -> bool {
        let Some(instance) = self.instances.get(id) else {
            return false;
        };
        let flow = instance.flow.clone();
        self.cancel_instance_runs(id);
        self.instances.remove(id);
        self.publish_instance_lifecycle("instance.removed", id, &flow);
        true
    }

    pub(crate) fn set_instance_inputs(
        &mut self,
        id: &InstanceId,
        raw: HashMap<String, HashMap<String, InputValueDto>>,
        actor: &str,
    ) -> SetInputsOutcome {
        let Some(flow) = self.instances.get(id).map(|instance| instance.flow.clone()) else {
            return SetInputsOutcome::InstanceNotFound;
        };
        let Some(graph) = self.definitions.get(&flow).and_then(|definition| definition.graph.clone()) else {
            return SetInputsOutcome::Error("flow has no valid graph".to_string());
        };
        let mut trigger_run = false;
        let mut answers: Vec<(RunId, String, Value)> = Vec::new();
        for (node_id, ports) in &raw {
            let Some(node_def) = graph.nodes.iter().find(|node| &node.id == node_id) else {
                return SetInputsOutcome::Error(format!("node `{node_id}` is not declared"));
            };
            let is_ask = node_def.kind == "ask";
            for (port, dto) in ports {
                if is_ask {
                    let waiting_here = self
                        .instances
                        .get(id)
                        .and_then(|instance| instance.waiting.as_ref())
                        .is_some_and(|waiting| &waiting.node == node_id);
                    if !waiting_here {
                        return SetInputsOutcome::AskNotWaiting;
                    }
                }
                let value = match value_from_dto(dto, &mut self.values) {
                    Ok(value) => value,
                    Err(error) => return SetInputsOutcome::Error(error),
                };
                let Some(instance) = self.instances.get_mut(id) else {
                    return SetInputsOutcome::InstanceNotFound;
                };
                match instance.set_input(node_id, port, value.clone(), &graph) {
                    Ok(InputEffect::None) => {}
                    Ok(InputEffect::TriggerRun) => trigger_run = true,
                    Ok(InputEffect::Answered(run_id)) => answers.push((run_id, node_id.clone(), value)),
                    Err(error) => return SetInputsOutcome::Error(error),
                }
            }
        }
        for (run_id, node, value) in answers {
            self.pending_answer_actor.insert((run_id.clone(), node.clone()), actor.to_string());
            if let Some(row) = self.runs.get(&run_id) {
                if let Some(handle) = &row.handle {
                    let _ = handle.answer.send((node, value));
                }
            }
        }
        if trigger_run {
            let deadline = engine::unix_ms() + INPUT_DEBOUNCE_MS;
            self.pending_input_runs.insert(id.clone(), deadline);
        }
        if !raw.is_empty() {
            self.publish_instance_lifecycle("instance.inputs", id, &flow);
        }
        let inputs = self
            .instances
            .get(id)
            .map(|instance| {
                instance
                    .inputs
                    .iter()
                    .map(|(node, ports)| {
                        (
                            node.clone(),
                            ports
                                .iter()
                                .map(|(port, value)| (port.clone(), ValueRef::from(value)))
                                .collect(),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        SetInputsOutcome::Ok(inputs)
    }

    pub(crate) fn start_run(&mut self, id: &InstanceId, outputs: Option<Vec<String>>) -> StartRunOutcome {
        let Some(flow) = self.instances.get(id).map(|instance| instance.flow.clone()) else {
            return StartRunOutcome::InstanceNotFound;
        };
        let Some(graph) = self.definitions.get(&flow).and_then(|definition| definition.graph.clone()) else {
            return StartRunOutcome::FlowInvalid;
        };
        let Some(source) = self.definitions.get(&flow).map(|definition| definition.source.clone()) else {
            return StartRunOutcome::FlowInvalid;
        };
        let Some(instance) = self.instances.get_mut(id) else {
            return StartRunOutcome::InstanceNotFound;
        };
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
                self.dispatch_run(id.clone(), run_id.clone(), flow, graph, source, outputs);
                StartRunOutcome::Started { run_id: run_id.0, queued: 0 }
            }
        }
    }

    pub(crate) fn run_row(&self, run_id: &RunId) -> Option<RunRowDto> {
        self.runs.get(run_id).map(|row| run_row_dto(run_id, row))
    }

    pub(crate) fn list_run_rows(&self, instance: Option<&InstanceId>) -> Vec<RunRowDto> {
        self.runs
            .iter()
            .filter(|(_, row)| instance.is_none_or(|instance| &row.instance == instance))
            .map(|(run_id, row)| run_row_dto(run_id, row))
            .collect()
    }

    pub(crate) fn cancel_run(&mut self, run_id: &RunId) -> bool {
        if !self.runs.contains_key(run_id) {
            return false;
        }
        self.cancel_run_row(run_id);
        true
    }

    /// Every in-flight run's handle, drained for a bounded shutdown join.
    pub(crate) fn take_all_run_handles(&mut self) -> Vec<RunHandle> {
        self.runs.values_mut().filter_map(|row| row.handle.take()).collect()
    }

    /// The fast tick: start any `trigger: @input` run whose debounce has
    /// elapsed. Cheap to call often (`host/server.rs`'s janitor thread).
    pub(crate) fn run_debounced_inputs(&mut self, now_ms: u64) {
        let due: Vec<InstanceId> = self
            .pending_input_runs
            .iter()
            .filter(|(_, deadline)| **deadline <= now_ms)
            .map(|(id, _)| id.clone())
            .collect();
        for id in due {
            self.pending_input_runs.remove(&id);
            if self.instances.contains_key(&id) {
                let _ = self.start_run(&id, None);
            }
        }
    }

    /// The slow (30 s) tick: expire values past TTL, finished runs past
    /// retention, idle instances past `instance_ttl` (§5.2).
    pub(crate) fn janitor_sweep(&mut self) {
        let now_ms = engine::unix_ms();
        let now = SystemTime::now();

        let mut live_digests: HashSet<[u8; 32]> = HashSet::new();
        for instance in self.instances.values() {
            for ports in instance.inputs.values() {
                for value in ports.values() {
                    live_digests.insert(value.digest);
                }
            }
            for value in instance.outputs.values() {
                live_digests.insert(value.digest);
            }
        }
        for row in self.runs.values() {
            for value in row.outputs.values() {
                live_digests.insert(value.digest);
            }
            for node in row.nodes.values() {
                for (_, value) in &node.outputs {
                    live_digests.insert(value.digest);
                }
            }
        }
        self.values.expire(now, &live_digests);

        self.runs.retain(|_, row| {
            row.finished_ms
                .is_none_or(|finished| now_ms.saturating_sub(finished) < RUN_RETENTION_MS)
        });

        let ttl_ms = self.instance_ttl.as_millis() as u64;
        let doomed: Vec<(InstanceId, String)> = self
            .instances
            .iter()
            .filter(|(_, instance)| {
                !matches!(instance.owner, Owner::Auto)
                    && instance.waiting.is_none()
                    && instance.active.is_empty()
                    && now_ms.saturating_sub(instance.last_activity_ms) >= ttl_ms
            })
            .map(|(id, instance)| (id.clone(), instance.flow.clone()))
            .collect();
        for (id, flow) in doomed {
            self.instances.remove(&id);
            self.publish_instance_lifecycle("instance.removed", &id, &flow);
        }
    }

    pub(crate) fn get_value(&mut self, digest: &[u8; 32]) -> Option<Value> {
        self.values.get(digest)
    }

    pub(crate) fn put_value(&mut self, value: Value) -> [u8; 32] {
        self.values.put(value)
    }
}

fn owner_str(owner: &Owner) -> &'static str {
    match owner {
        Owner::Tab => "tab",
        Owner::Chat { .. } => "chat",
        Owner::Service => "service",
        Owner::Auto => "auto",
    }
}

fn instance_state_str(instance: &Instance) -> &'static str {
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

fn instance_row_dto(instance: &Instance) -> InstanceRow {
    let inputs = instance
        .inputs
        .iter()
        .map(|(node, ports)| {
            (
                node.clone(),
                ports
                    .iter()
                    .map(|(port, value)| (port.clone(), ValueRef::from(value)))
                    .collect(),
            )
        })
        .collect();
    let outputs = instance
        .outputs
        .iter()
        .map(|(node, value)| (node.clone(), ValueRef::from(value)))
        .collect();
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
        waiting: instance.waiting.as_ref().map(|waiting| WaitingDto {
            node: waiting.node.clone(),
            question: waiting.question.clone(),
            ty: waiting.ty,
            options: waiting.options.clone(),
        }),
        owner: owner_str(&instance.owner).to_string(),
        created_ms: instance.created_ms,
        last_activity_ms: instance.last_activity_ms,
        subscribers: 0,
    }
}

fn run_row_dto(run_id: &RunId, row: &RunRow) -> RunRowDto {
    RunRowDto {
        run_id: run_id.0.clone(),
        instance: row.instance.0.clone(),
        flow: row.flow.clone(),
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
                            .map(|(port, value)| PortValueRef { port: port.clone(), value: value.into() })
                            .collect(),
                        error: node.error.clone(),
                        text: (!node.delta_text.is_empty()).then(|| node.delta_text.clone()),
                    },
                )
            })
            .collect(),
        outputs: row.outputs.iter().map(|(id, value)| (id.clone(), value.into())).collect(),
        http_log: row.http_log.iter().map(Into::into).collect(),
        started_ms: row.started_ms,
        finished_ms: row.finished_ms,
    }
}

fn value_from_dto(dto: &InputValueDto, values: &mut ValueStore) -> Result<Value, String> {
    match dto.ty {
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
        ty if ty.is_media() => {
            let digest_text = dto.digest.as_deref().ok_or_else(|| "media value requires `digest`".to_string())?;
            let digest = parse_prefixed_digest(digest_text).ok_or_else(|| "malformed digest".to_string())?;
            let value = values.get(&digest).ok_or_else(|| "unknown value digest".to_string())?;
            if value.ty != ty {
                return Err(format!("digest is a {} value, not {}", value.ty.as_str(), ty.as_str()));
            }
            Ok(value)
        }
        ty => Err(format!("unsupported input type `{}`", ty.as_str())),
    }
}

/// Accepts both the bare hex digest and the `sha256:<hex>` form the design
/// prose uses for a media input reference.
fn parse_prefixed_digest(text: &str) -> Option<[u8; 32]> {
    super::util::from_hex_32(text.strip_prefix("sha256:").unwrap_or(text))
}

fn run_event_kind(payload: &RunEventPayload) -> &'static str {
    match payload {
        RunEventPayload::RunStarted { .. } => "run.started",
        RunEventPayload::NodeStarted { .. } => "node.started",
        RunEventPayload::NodeProgress { .. } => "node.progress",
        RunEventPayload::NodeDelta { .. } => "node.delta",
        RunEventPayload::NodeWaiting { .. } => "node.waiting",
        RunEventPayload::NodeAnswered { .. } => "node.answered",
        RunEventPayload::NodeDone { .. } => "node.done",
        RunEventPayload::NodeFailed { .. } => "node.failed",
        RunEventPayload::NodeSkipped { .. } => "node.skipped",
        RunEventPayload::RunFinished { .. } => "run.finished",
    }
}

/// Flatten `{"<Variant>": {fields...}}` (the derive's enum shape) into a
/// bare fields object stamped with `run_id`/`instance`/`flow`, matching
/// DESIGN.md §5.4's event payload shape.
fn run_event_json(run_id: &RunId, instance_id: &InstanceId, flow: &str, payload: &RunEventPayload) -> JsonValue {
    let wrapped = json_value(payload);
    let mut fields = match wrapped {
        JsonValue::Object(mut outer) => outer
            .drain()
            .next()
            .map(|(_, inner)| match inner {
                JsonValue::Object(fields) => fields,
                _ => HashMap::new(),
            })
            .unwrap_or_default(),
        _ => HashMap::new(),
    };
    fields.insert("run_id".to_string(), JsonValue::String(run_id.0.clone()));
    fields.insert("instance".to_string(), JsonValue::String(instance_id.0.clone()));
    fields.insert("flow".to_string(), JsonValue::String(flow.to_string()));
    JsonValue::Object(fields)
}

pub(crate) fn json_value<T: SerJson>(value: &T) -> JsonValue {
    JsonValue::deserialize_json(&value.serialize_json()).unwrap_or(JsonValue::Null)
}

type Task = Box<dyn FnOnce(&mut FlowState) + Send>;

#[derive(Clone)]
pub struct StateHandle {
    tx: mpsc::Sender<Task>,
}

impl StateHandle {
    pub fn call<R: Send + 'static>(
        &self,
        closure: impl FnOnce(&mut FlowState) -> R + Send + 'static,
    ) -> Option<R> {
        let (reply_tx, reply_rx) = mpsc::channel();
        let task: Task = Box::new(move |state| {
            let result = closure(state);
            let _ = reply_tx.send(result);
        });
        self.tx.send(task).ok()?;
        reply_rx.recv().ok()
    }
}

pub(crate) fn spawn_state(
    config: SharedConfig,
    events: Arc<EventHub>,
    epoch: u64,
    origin: (String, u64),
    run_register_tx: mpsc::Sender<RunRegistration>,
) -> Result<(StateHandle, std::thread::JoinHandle<()>), ServerError> {
    let (tx, rx) = mpsc::channel::<Task>();
    let (ready_tx, ready_rx) = mpsc::channel();
    let thread_config = config.clone();
    let thread_events = events.clone();
    let join = std::thread::Builder::new()
        .name("flow-server-state".to_string())
        .spawn(move || {
            let mut state = match FlowState::build(
                &thread_config,
                thread_events.clone(),
                epoch,
                origin.clone(),
                run_register_tx.clone(),
            ) {
                Ok(state) => {
                    let _ = ready_tx.send(Ok(()));
                    state
                }
                Err(error) => {
                    let _ = ready_tx.send(Err(error));
                    return;
                }
            };
            let mut poisoned = false;
            while let Ok(task) = rx.recv() {
                if poisoned {
                    continue;
                }
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| task(&mut state)));
                if outcome.is_err() {
                    log(&thread_config, "state closure panicked; rebuilding flow state");
                    match FlowState::build(
                        &thread_config,
                        thread_events.clone(),
                        epoch,
                        origin.clone(),
                        run_register_tx.clone(),
                    ) {
                        Ok(rebuilt) => state = rebuilt,
                        Err(error) => {
                            log(&thread_config, &format!("state rebuild failed ({error}); poisoned"));
                            poisoned = true;
                        }
                    }
                }
            }
        })
        .map_err(|error| ServerError::io("spawn state thread", error))?;
    ready_rx
        .recv()
        .map_err(|_| ServerError::StateUnavailable)??;
    Ok((StateHandle { tx }, join))
}
