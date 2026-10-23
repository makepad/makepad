//! The HTTP host's one road to the engine.
//!
//! Flow state lives in the engine (`runtime::engine`); this host owns no
//! store and no scheduler. One long-lived pump thread owns the
//! [`EngineHandle`]: it admits requests from a bounded queue, publishes the
//! engine's ordered journal into the [`EventHub`] and settles every request
//! exactly once, whether the engine answered, refused, stopped or died.
//! Connection threads and the watcher wait on their own request; nothing
//! here is ever called from a UI or audio thread.
//!
//! What stays a host service: the `<root>/flows` files (one lock orders
//! every write, watcher read and load, so an older snapshot can never land
//! on a newer accepted edit), the fleet snapshot, and the JSON journal.

use super::config::SharedConfig;
use super::events::EventHub;
use super::models::FleetSnapshot;
use super::state::publish_store_event;
use super::util::{atomic_write, log};
use super::ServerError;
use crate::document::closure::Closure;
use crate::engine::results::ResultStore;
use crate::engine::{self, RunId, Seams};
use crate::instance::{InstanceId, Owner};
use crate::runtime::store::SourceResult;
use crate::runtime::{
    Engine, EngineCommand, EngineConfig, EngineError, EngineHandle, EngineLimits, EngineQuery,
    HostServices, InputDtos, JournalPoll, Reply, ReplyPayload, RequestId, StdHostWork, Submit,
};
use crate::{
    graph, CreateBatchRequest, EvalError, FlowResponse, ModelsResponse, NodeTypeCatalog,
    ParallelismResponse, Value,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, RecvTimeoutError, SyncSender, TryRecvError, TrySendError};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

pub(crate) const MAX_SOURCE_BYTES: u64 = graph::MAX_SOURCE_BYTES as u64;

/// Run lanes of the HTTP host's engine and what each may hold. The planes
/// never refused a run for capacity before; this keeps that true for any
/// load the connection limits allow.
const RUN_LANES: usize = 4;
const LANE_RUN_CAPACITY: usize = 256;
/// Entries of the engine-to-host journal lane. The hub behind it is the
/// subscribers' bounded history; this only has to outlast one pump turn.
const JOURNAL_LANE: usize = 8_192;
/// Most entries taken from one lane per pump turn.
const TURN_BUDGET: usize = 1_024;
/// How long a run gets to notice its cancel flag at shutdown.
const STOP_GRACE: Duration = Duration::from_secs(5);
/// How long `stop` waits for the pump to end before it gives up on it: the
/// engine's grace, the pump's own admission turns, and margin.
const STOP_DEADLINE: Duration = Duration::from_secs(12);
/// Most run rows the planes' batches may retain at once (sixteen batches of
/// the largest size), refused before any run starts.
const MAX_BATCH_RUNS: usize = 16 * 256;

pub(crate) type EngineResult = Result<ReplyPayload, EngineError>;

struct Slot {
    result: Mutex<Option<EngineResult>>,
    ready: Condvar,
}

impl Slot {
    fn settle(&self, result: EngineResult) {
        let mut slot = self.result.lock().unwrap_or_else(|poison| poison.into_inner());
        if slot.is_none() {
            *slot = Some(result);
        }
        drop(slot);
        self.ready.notify_all();
    }
}

struct Request {
    command: EngineCommand,
    slot: Arc<Slot>,
}

pub(crate) struct Backend {
    requests: SyncSender<Request>,
    bell: SyncSender<()>,
    alive: Arc<AtomicBool>,
    /// Raised by `stop`. The pump submits the engine's `Stop` itself, ahead
    /// of everything queued, so a saturated request queue cannot keep it out.
    stopping: Arc<AtomicBool>,
    /// Orders every flow file write, watcher read and definition load. The
    /// value is the temp-file serial of `atomic_write`.
    files: Mutex<u64>,
    fleet: Mutex<FleetSnapshot>,
    catalog: Vec<NodeTypeCatalog>,
    config: SharedConfig,
    pub epoch: u64,
}

pub(crate) enum FileError {
    Io(ServerError),
    Engine(EngineError),
    /// The root's source is not the graph writer's form: a graph write
    /// would destroy code or text of its own.
    SourceOwned,
    /// The graph a client sent does not agree with itself or with the
    /// pinned components; nothing was written.
    Invalid(String),
}

impl From<EngineError> for FileError {
    fn from(error: EngineError) -> Self {
        Self::Engine(error)
    }
}

impl From<ServerError> for FileError {
    fn from(error: ServerError) -> Self {
        Self::Io(error)
    }
}

fn build_seams(config: &super::config::FlowServerConfig) -> Seams {
    if let Some(seams) = config.seams() {
        return seams;
    }
    Seams {
        chat: build_chat_seam(&config.chat_model),
        gen: Arc::new(makepad_ai_hub::job::FleetRouter::default()),
        http: Arc::new(engine::HubHttp),
    }
}

#[cfg(feature = "hub-chat")]
fn build_chat_seam(chat_model: &Option<PathBuf>) -> Arc<dyn crate::engine::executors::chat::ChatSeam> {
    Arc::new(engine::HubChat {
        model_path: chat_model.clone().unwrap_or_default(),
    })
}

#[cfg(not(feature = "hub-chat"))]
fn build_chat_seam(_chat_model: &Option<PathBuf>) -> Arc<dyn crate::engine::executors::chat::ChatSeam> {
    Arc::new(engine::HubChat::from_env())
}

impl Backend {
    /// Start the engine and its pump, then load `<root>/flows` in name order
    /// and create the autostart instances, as the planes always did at boot.
    pub(crate) fn start(
        config: SharedConfig,
        events: Arc<EventHub>,
        epoch: u64,
        origin: (String, u64),
        results: Arc<dyn ResultStore>,
    ) -> Result<(Arc<Self>, JoinHandle<()>), ServerError> {
        let catalog = graph::catalog_with_registry(&config.registry).map_err(ServerError::Prelude)?;
        // Every connection may hold one request, plus the watcher and boot.
        let admission = config.control_max_conns + config.data_max_conns + 4;
        let mut engine_config = EngineConfig::new(config.root.join("values"));
        engine_config.registry = config.registry.clone();
        engine_config.executors = config.executors.clone();
        engine_config.values_ram_budget = config.values_ram_budget;
        engine_config.value_ttl = Duration::from_secs(config.value_ttl_secs);
        engine_config.revision_ring = config.revision_ring;
        engine_config.instance_ttl = Duration::from_secs(config.instance_ttl_secs);
        engine_config.origin = origin;
        engine_config.seams = build_seams(&config);
        engine_config.net = config.net.clone();
        engine_config.command_capacity = admission;
        engine_config.reply_capacity = admission;
        engine_config.run_lanes = RUN_LANES;
        engine_config.lane_run_capacity = LANE_RUN_CAPACITY;
        engine_config.stop_grace = STOP_GRACE;
        engine_config.live_outputs = config.live_outputs;
        engine_config.journal_capacity = JOURNAL_LANE;
        engine_config.sweep_every = Duration::from_secs(config.janitor_sweep_secs.max(1));
        // The planes answer rows with linearized queries; nothing here reads
        // them from a snapshot, so the engine does not copy them into one.
        engine_config.snapshot_rows = false;
        engine_config.lane_max_bytes = 128 * 1024 * 1024;
        engine_config.run_media_max_bytes = config.media_max_bytes;
        // The planes had no admission limits of their own; these are far
        // above anything the connection limits let a client reach.
        engine_config.limits = EngineLimits {
            max_definitions: 4_096,
            max_instances: 65_536,
            max_queued_runs: 4_096,
            max_finished_runs: 65_536,
            max_input_bytes: 64 * 1024 * 1024,
            max_batch_runs: MAX_BATCH_RUNS,
        };

        let work = StdHostWork::new(2, 64, RUN_LANES + 1)
            .map_err(|error| ServerError::io("start flow work pool", error))?;
        let (bell_tx, bell_rx) = sync_channel::<()>(1);
        let wake_bell = bell_tx.clone();
        let log_config = config.clone();
        let handle = Engine::start(
            engine_config,
            HostServices {
                work,
                results: Some(results),
                log: Arc::new(move |line: &str| log(&log_config, line)),
                wake: Some(Arc::new(move || {
                    let _ = wake_bell.try_send(());
                })),
            },
        )
        .map_err(|_| ServerError::StateUnavailable)?;

        let (request_tx, request_rx) = sync_channel::<Request>(admission);
        let alive = Arc::new(AtomicBool::new(true));
        let stopping = Arc::new(AtomicBool::new(false));
        let pump = Pump {
            handle,
            requests: request_rx,
            bell: bell_rx,
            pending: HashMap::new(),
            retry: None,
            capacity: admission,
            events,
            config: config.clone(),
            stopping: stopping.clone(),
            stop_submitted: false,
            stop_since: None,
            stopped: false,
            orphaned: false,
        };
        let pump_alive = alive.clone();
        let join = std::thread::Builder::new()
            .name("flow-server-engine".to_string())
            .spawn(move || pump.run(pump_alive))
            .map_err(|error| ServerError::io("spawn engine pump thread", error))?;

        let backend = Arc::new(Self {
            requests: request_tx,
            bell: bell_tx,
            alive,
            stopping,
            files: Mutex::new(0),
            fleet: Mutex::new(FleetSnapshot::default()),
            catalog,
            config,
            epoch,
        });
        if let Err(error) = backend.boot() {
            backend.stop();
            let _ = join.join();
            return Err(error);
        }
        Ok((backend, join))
    }

    fn boot(&self) -> Result<(), ServerError> {
        let flows_dir = self.config.root.join("flows");
        let mut files = Vec::new();
        for entry in std::fs::read_dir(&flows_dir)
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
        let _files = self.files.lock().unwrap_or_else(|poison| poison.into_inner());
        for (name, path) in files {
            let metadata = std::fs::metadata(&path)
                .map_err(|error| ServerError::io("stat flow source", error))?;
            let command = if metadata.len() > MAX_SOURCE_BYTES {
                EngineCommand::SetLoadError {
                    name,
                    error: graph::source_size_error(&path.display().to_string()),
                }
            } else {
                let source = std::fs::read_to_string(&path)
                    .map_err(|error| ServerError::io("read flow source", error))?;
                EngineCommand::LoadDefinition {
                    name,
                    source,
                    file_name: path.display().to_string(),
                    closure: None,
                    ephemeral: false,
                }
            };
            self.request(command).map_err(|_| ServerError::StateUnavailable)?;
        }
        self.request(EngineCommand::Autostart).map_err(|_| ServerError::StateUnavailable)?;
        Ok(())
    }

    /// Whether the engine can still answer. Routes that touch no state use
    /// it so a dead backend answers 503 everywhere.
    pub(crate) fn is_alive(&self) -> bool {
        self.alive.load(Ordering::SeqCst)
    }

    /// Submit one command and wait for its one reply. Blocking: connection
    /// threads and host services only.
    pub(crate) fn request(&self, command: EngineCommand) -> EngineResult {
        if !self.is_alive() {
            return Err(EngineError::Stopped);
        }
        if self.stopping.load(Ordering::SeqCst) {
            return Err(EngineError::Stopping);
        }
        let slot = Arc::new(Slot { result: Mutex::new(None), ready: Condvar::new() });
        match self.requests.try_send(Request { command, slot: slot.clone() }) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                return Err(EngineError::Busy("the request queue is full".to_string()));
            }
            Err(TrySendError::Disconnected(_)) => return Err(EngineError::Stopped),
        }
        let _ = self.bell.try_send(());
        let mut result = slot.result.lock().unwrap_or_else(|poison| poison.into_inner());
        loop {
            if let Some(result) = result.take() {
                return result;
            }
            if !self.is_alive() {
                // The pump settles what it still holds before it clears the
                // flag; a request that slipped in afterwards ends here.
                return Err(EngineError::Stopped);
            }
            result = slot
                .ready
                .wait_timeout(result, Duration::from_millis(50))
                .unwrap_or_else(|poison| poison.into_inner())
                .0;
        }
    }

    pub(crate) fn query(&self, query: EngineQuery) -> EngineResult {
        self.request(EngineCommand::Query(query))
    }

    /// Cancel every run, wait out the stop grace and end the pump. The stop
    /// is a flag the pump reads every turn, not a queued request, so it
    /// cannot be kept out by a full queue; the wait is bounded by
    /// `STOP_DEADLINE`, after which the pump has given the engine up anyway.
    pub(crate) fn stop(&self) {
        self.stopping.store(true, Ordering::SeqCst);
        let _ = self.bell.try_send(());
        let deadline = std::time::Instant::now() + STOP_DEADLINE;
        while self.is_alive() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    // -- host services ------------------------------------------------------

    pub(crate) fn models_response(&self, domain: Option<&str>) -> ModelsResponse {
        self.fleet
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .response(&self.config.fleet_hint, domain)
    }

    pub(crate) fn registry_data(&self) -> crate::document::RegistryData {
        self.config.registry.data()
    }

    pub(crate) fn catalog_with_models(&self) -> Vec<NodeTypeCatalog> {
        self.fleet
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .catalog(&self.config.fleet_hint, &self.catalog)
    }

    pub(crate) fn definition(&self, name: &str) -> Result<Option<Arc<FlowResponse>>, EngineError> {
        match self.query(EngineQuery::Definition { name: name.to_string() })? {
            ReplyPayload::DefinitionDetail(detail) => Ok(detail),
            _ => Err(EngineError::Stopped),
        }
    }

    /// `Ok(None)`: no such flow. `Ok(Some(None))`: it has no valid graph.
    pub(crate) fn parallelism(&self, name: &str) -> Result<Option<Option<ParallelismResponse>>, EngineError> {
        let Some(detail) = self.definition(name)? else {
            return Ok(None);
        };
        let Some(graph) = detail.graph.as_ref() else {
            return Ok(Some(None));
        };
        let snapshot = self.models_response(None);
        Ok(Some(Some(super::parallelism::estimate_parallelism(graph, &snapshot))))
    }

    // -- flow files ---------------------------------------------------------

    fn flow_path(&self, name: &str) -> PathBuf {
        self.config.root.join("flows").join(format!("{name}.splash"))
    }

    /// Write, then load. The caller holds the files lock, so the engine sees
    /// loads in the order the files were written. A session-owned
    /// (ephemeral) definition has no file: it is loaded, never written.
    fn write_and_load(&self, serial: &mut u64, name: String, source: String) -> Result<SourceResult, FileError> {
        let ephemeral = self.is_ephemeral(&name);
        self.write_and_load_with(serial, name, source, None, ephemeral)
    }

    fn write_and_load_with(
        &self,
        serial: &mut u64,
        name: String,
        source: String,
        closure: Option<Arc<Closure>>,
        ephemeral: bool,
    ) -> Result<SourceResult, FileError> {
        *serial = serial.wrapping_add(1);
        let path = self.flow_path(&name);
        if !ephemeral {
            atomic_write(&path, source.as_bytes(), *serial)?;
        }
        self.load_with(name, source, path, closure, ephemeral)
    }

    fn load(&self, name: String, source: String, path: PathBuf) -> Result<SourceResult, FileError> {
        self.load_with(name, source, path, None, false)
    }

    fn load_with(
        &self,
        name: String,
        source: String,
        path: PathBuf,
        closure: Option<Arc<Closure>>,
        ephemeral: bool,
    ) -> Result<SourceResult, FileError> {
        let file_name = path.display().to_string();
        match self.request(EngineCommand::LoadDefinition { name, source, file_name, closure, ephemeral })? {
            ReplyPayload::Definition { revision, error, graph, .. } => Ok(SourceResult { revision, graph, error }),
            _ => Err(FileError::Engine(EngineError::Stopped)),
        }
    }


    /// Install `source` with an explicit module closure, optionally as a
    /// session-owned definition without a file. A definition that already
    /// is ephemeral stays file-less whatever the caller says.
    pub(crate) fn put_source_with(
        &self,
        name: String,
        source: String,
        closure: Option<Arc<Closure>>,
        ephemeral: bool,
    ) -> Result<SourceResult, FileError> {
        let mut serial = self.files.lock().unwrap_or_else(|poison| poison.into_inner());
        let ephemeral = ephemeral || self.is_ephemeral(&name);
        self.write_and_load_with(&mut serial, name, source, closure, ephemeral)
    }

    /// Rewrite a canonical root, or preserve source while patching root
    /// and pinned component layout. Source and closure are installed together.
    /// The source check and the write
    /// happen under the one files lock, so a source-owned root installed
    /// meanwhile is never rewritten from a stale graph.
    pub(crate) fn put_graph(&self, name: String, graph: &crate::Graph) -> Result<SourceResult, FileError> {
        let mut serial = self.files.lock().unwrap_or_else(|poison| poison.into_inner());
        let detail = self.definition(&name)?;
        let (canonical, ephemeral, components, modules) = match detail.as_ref() {
            Some(detail) => (
                detail.canonical,
                detail.ephemeral,
                detail.graph.as_ref().map(|graph| graph.components.clone()).unwrap_or_default(),
                detail.modules.clone(),
            ),
            None => (true, false, Vec::new(), Vec::new()),
        };
        let closure = Closure { modules };
        let component_edit = detail.as_ref().and_then(|detail| detail.graph.as_ref())
            .is_some_and(|before| before.components != graph.components);
        if !canonical || component_edit {
            let detail = detail.as_ref().expect("a source-preserving edit has a definition");
            if component_edit && graph.revision != detail.revision {
                return Err(FileError::Invalid("the graph revision changed; reload the current graph before editing component layout".into()));
            }
            let (source, next_closure) = crate::graph::patch_layout_closure_with_registry(
                &detail.source, graph, &closure, &self.config.registry,
            ).map_err(|error| match error {
                crate::graph::WriteError::NonLayout if !canonical => FileError::SourceOwned,
                error => FileError::Invalid(error.to_string()),
            })?;
            if source == detail.source && next_closure == closure {
                return Ok(SourceResult {
                    revision: detail.revision,
                    graph: detail.graph.clone(),
                    error: detail.error.clone(),
                });
            }
            return self.write_and_load_with(&mut serial, name, source, Some(Arc::new(next_closure)), ephemeral);
        }
        // The client's graph is checked against itself and against the
        // definition's own pinned components (never the metadata it sent)
        // before the writer sees it.
        crate::graph::validate_projection_with_registry(graph, &components, &self.config.registry).map_err(FileError::Invalid)?;
        let source = crate::graph::write_with_registry(graph, &self.config.registry)
            .map_err(|error| FileError::Invalid(error.to_string()))?;
        // Then the written source is evaluated here, with the definition's
        // own pinned closure, before a byte is written or installed: a
        // graph whose source does not evaluate, or evaluates to other
        // wiring than was sent, is refused with source and revision as
        // they were. It is never saved as an invalid draft.
        let file_name = self.flow_path(&name).to_string_lossy().to_string();
        let evaluated = crate::graph::evaluate_with_registry(&source, &file_name, &closure, &self.config.registry).map_err(|error| {
            FileError::Invalid(format!(
                "its source does not evaluate ({}:{}: {})",
                error.line, error.col, error.message
            ))
        })?;
        crate::graph::same_wiring(graph, &evaluated).map_err(FileError::Invalid)?;
        self.write_and_load_with(&mut serial, name, source, None, ephemeral)
    }

    /// Whether `name` is a session-owned definition (no file to write).
    pub(crate) fn is_ephemeral(&self, name: &str) -> bool {
        matches!(self.definition(name), Ok(Some(detail)) if detail.ephemeral)
    }

    /// `None`: the flow, or its file, already exists.
    pub(crate) fn create_source(&self, name: String, source: String) -> Result<Option<SourceResult>, FileError> {
        let mut serial = self.files.lock().unwrap_or_else(|poison| poison.into_inner());
        if self.definition(&name)?.is_some() || self.flow_path(&name).exists() {
            return Ok(None);
        }
        self.write_and_load(&mut serial, name, source).map(Some)
    }

    /// `None`: the revision ring no longer holds that revision.
    pub(crate) fn revert(&self, name: &str, revision: u64) -> Result<Option<SourceResult>, FileError> {
        let mut serial = self.files.lock().unwrap_or_else(|poison| poison.into_inner());
        let kept = match self.query(EngineQuery::DefinitionSource { name: name.to_string(), revision })? {
            ReplyPayload::DefinitionSource(kept) => kept,
            _ => return Err(FileError::Engine(EngineError::Stopped)),
        };
        match kept {
            // A revision is its source and its closure, restored together.
            Some((source, closure)) => {
                let ephemeral = self.is_ephemeral(name);
                self.write_and_load_with(&mut serial, name.to_string(), source, Some(closure), ephemeral).map(Some)
            }
            None => Ok(None),
        }
    }

    pub(crate) fn remove(&self, name: &str) -> Result<bool, FileError> {
        let _serial = self.files.lock().unwrap_or_else(|poison| poison.into_inner());
        match std::fs::remove_file(self.flow_path(name)) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(FileError::Io(ServerError::io("remove flow source", error))),
        }
        match self.request(EngineCommand::RemoveDefinition { name: name.to_string() })? {
            ReplyPayload::Removed(removed) => Ok(removed),
            _ => Err(FileError::Engine(EngineError::Stopped)),
        }
    }

    /// The watcher saw `name` change. What is loaded is what the file holds
    /// now, read under the files lock: a PUT that replaced the file since the
    /// watcher looked has already loaded those same bytes, and the engine
    /// answers an unchanged source without touching anything. `false` only
    /// when the engine is gone, so the watcher tries again.
    pub(crate) fn watched_changed(&self, name: &str) -> bool {
        let _serial = self.files.lock().unwrap_or_else(|poison| poison.into_inner());
        let path = self.flow_path(name);
        let Ok(metadata) = std::fs::metadata(&path) else {
            // Gone again; the removal pass handles it.
            return true;
        };
        if metadata.len() > MAX_SOURCE_BYTES {
            let error: EvalError = graph::source_size_error(&path.display().to_string());
            return self.request(EngineCommand::SetLoadError { name: name.to_string(), error }).is_ok();
        }
        match std::fs::read_to_string(&path) {
            Ok(source) => !matches!(
                self.load(name.to_string(), source, path),
                Err(FileError::Engine(EngineError::Stopped | EngineError::Stopping))
            ),
            Err(error) => {
                log(&self.config, &format!("watch read failed for {}: {error}", path.display()));
                true
            }
        }
    }

    pub(crate) fn watched_removed(&self, name: &str) -> bool {
        let _serial = self.files.lock().unwrap_or_else(|poison| poison.into_inner());
        if self.flow_path(name).exists() {
            return true;
        }
        self.request(EngineCommand::RemoveDefinition { name: name.to_string() }).is_ok()
    }

    // -- instances, runs, batches, values -----------------------------------

    pub(crate) fn create_instance(
        &self,
        flow: &str,
        label: Option<String>,
        pinned: bool,
        inputs: InputDtos,
    ) -> EngineResult {
        self.request(EngineCommand::Instantiate {
            flow: flow.to_string(),
            label,
            pinned,
            owner: Owner::Tab,
            inputs,
        })
    }

    pub(crate) fn set_instance_inputs(&self, instance: InstanceId, inputs: InputDtos, actor: String) -> EngineResult {
        self.request(EngineCommand::SetInputDtos { instance, inputs, actor })
    }

    pub(crate) fn start_run(&self, instance: InstanceId, outputs: Option<Vec<String>>) -> EngineResult {
        self.request(EngineCommand::StartRun { instance, outputs })
    }

    pub(crate) fn cancel_run(&self, run: RunId) -> EngineResult {
        self.request(EngineCommand::CancelRun { run })
    }

    pub(crate) fn create_batch(&self, flow: &str, request: CreateBatchRequest) -> EngineResult {
        self.request(EngineCommand::CreateBatch { flow: flow.to_string(), request })
    }

    pub(crate) fn put_value(&self, value: Value) -> EngineResult {
        self.request(EngineCommand::PutValue { value })
    }
}

struct Pump {
    handle: EngineHandle,
    requests: Receiver<Request>,
    bell: Receiver<()>,
    /// Accepted by the engine, not answered yet. Bounded by the engine's
    /// reply capacity.
    pending: HashMap<RequestId, Arc<Slot>>,
    /// The one request the engine had no room for; nothing else is taken
    /// from the queue until it is in, so the backlog stays in the bounded
    /// queue.
    retry: Option<Request>,
    capacity: usize,
    events: Arc<EventHub>,
    config: SharedConfig,
    stopping: Arc<AtomicBool>,
    /// The engine accepted the `Stop`; its reply ends the pump.
    stop_submitted: bool,
    /// When the stop began; past `STOP_DEADLINE` the pump leaves without it.
    stop_since: Option<std::time::Instant>,
    stopped: bool,
    /// Every sender of the request queue is gone: the server dropped the
    /// backend without stopping it.
    orphaned: bool,
}

impl Pump {
    fn run(mut self, alive: Arc<AtomicBool>) {
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.pump()));
        if outcome.is_err() {
            log(&self.config, "engine pump panicked; every request is answered unavailable");
        }
        // Nothing accepted may stay unanswered. The flag drops first: a
        // request queued after the drain below sees it and gives up itself.
        alive.store(false, Ordering::SeqCst);
        for (_, slot) in self.pending.drain() {
            slot.settle(Err(EngineError::Stopped));
        }
        if let Some(request) = self.retry.take() {
            request.slot.settle(Err(EngineError::Stopped));
        }
        // What is queued fits in the queue's capacity; that is the budget.
        for _ in 0..self.capacity {
            match self.requests.try_recv() {
                Ok(request) => request.slot.settle(Err(EngineError::Stopped)),
                Err(_) => break,
            }
        }
        self.events.shutdown();
    }

    fn pump(&mut self) {
        let mut replies: Vec<Reply> = Vec::new();
        loop {
            let park = if self.retry.is_some() { Duration::from_millis(2) } else { Duration::from_millis(50) };
            if let Err(RecvTimeoutError::Disconnected) = self.bell.recv_timeout(park) {
                self.orphaned = true;
            }

            if self.stopping.load(Ordering::SeqCst) {
                self.stop_engine();
            } else {
                self.admit();
            }

            // Replies first, then the journal: the engine sends a command's
            // events before its reply, so everything a reply taken here
            // caused is already on the journal lane and reaches the hub
            // before the waiting connection is released.
            self.handle.poll_replies(&mut replies, TURN_BUDGET);
            self.publish_journal();
            for reply in replies.drain(..) {
                if matches!(reply.result, Ok(ReplyPayload::Stopped { .. })) {
                    self.stopped = true;
                }
                if let Some(slot) = self.pending.remove(&reply.id) {
                    slot.settle(reply.result);
                }
            }

            if self.stopped || !self.handle.is_alive() {
                return;
            }
            if self.stop_since.is_some_and(|since| since.elapsed() >= STOP_DEADLINE) {
                // The engine never answered the stop. Leaving drops the
                // handle, and a worker that is still alive stops on that.
                log(&self.config, "engine pump: stop deadline passed without `Stopped`");
                return;
            }
            if self.orphaned && self.pending.is_empty() && self.retry.is_none() {
                // Dropping the handle stops the engine: it cancels its runs
                // and waits out the grace on its own.
                return;
            }
        }
    }

    /// Submit the engine's `Stop` ahead of everything queued, and answer the
    /// queued requests `Stopping` so their connections are released now
    /// rather than at the end. Retried every turn until the engine takes it:
    /// each reply the engine sends frees a slot, so a full engine admits it
    /// within its own capacity of replies.
    fn stop_engine(&mut self) {
        if self.stop_since.is_none() {
            self.stop_since = Some(std::time::Instant::now());
        }
        if let Some(request) = self.retry.take() {
            request.slot.settle(Err(EngineError::Stopping));
        }
        for _ in 0..self.capacity {
            match self.requests.try_recv() {
                Ok(request) => request.slot.settle(Err(EngineError::Stopping)),
                Err(_) => break,
            }
        }
        if self.stop_submitted {
            return;
        }
        match self.handle.stop() {
            Submit::Accepted(_) => self.stop_submitted = true,
            Submit::Full(_) => {}
            Submit::Refused(_, _) | Submit::Closed(_) => self.stopped = true,
        }
    }

    fn admit(&mut self) {
        loop {
            if self.pending.len() >= self.capacity {
                return;
            }
            let request = match self.retry.take() {
                Some(request) => request,
                None => match self.requests.try_recv() {
                    Ok(request) => request,
                    Err(TryRecvError::Empty) => return,
                    Err(TryRecvError::Disconnected) => {
                        self.orphaned = true;
                        return;
                    }
                },
            };
            let Request { command, slot } = request;
            match self.handle.submit(command) {
                Submit::Accepted(id) => {
                    self.pending.insert(id, slot);
                }
                Submit::Full(command) => {
                    self.retry = Some(Request { command, slot });
                    return;
                }
                Submit::Refused(_, _) => {
                    slot.settle(Err(EngineError::NotFound("control endpoint".to_string())));
                }
                Submit::Closed(_) => slot.settle(Err(EngineError::Stopped)),
            }
        }
    }

    /// The handle owns the one cursor. On a gap it has already moved that
    /// cursor to the recovery snapshot and kept back everything the snapshot
    /// covers, so what reaches the hub is in order and never behind a gap
    /// it published: subscribers behind the gap resynchronise from the
    /// planes' linearized reads, exactly as after journal overflow.
    fn publish_journal(&mut self) {
        let mut entries = Vec::new();
        // A few full budgets at most per turn, so a busy journal cannot keep
        // the replies behind it waiting.
        for _ in 0..8 {
            let poll = self.handle.poll_journal(&mut entries, TURN_BUDGET);
            let full = entries.len() >= TURN_BUDGET;
            if let JournalPoll::Gap { dropped, .. } = &poll {
                self.events.publish_gap((*dropped).max(1));
            }
            for entry in entries.drain(..) {
                publish_store_event(&self.events, &self.config, entry.event);
            }
            if !full {
                return;
            }
        }
    }
}
