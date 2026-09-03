use super::config::SharedConfig;
use super::events::EventHub;
use super::util::{atomic_write, log};
use super::ServerError;
use crate::{graph, EvalError, Graph, NodeTypeCatalog, ToolSchema};
use makepad_micro_serde::{DeJson, JsonValue, SerJson};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{mpsc, Arc};

pub(crate) const MAX_SOURCE_BYTES: u64 = 1024 * 1024;

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

pub struct FlowState {
    pub definitions: BTreeMap<String, Definition>,
    pub events: Arc<EventHub>,
    pub epoch: u64,
    pub(crate) catalog: Vec<NodeTypeCatalog>,
    root: PathBuf,
    revision_ring: usize,
    temp_serial: u64,
}

#[derive(Clone)]
pub(crate) struct SourceResult {
    pub revision: u64,
    pub graph: Option<Graph>,
    pub error: Option<EvalError>,
}

impl FlowState {
    fn build(config: &SharedConfig, events: Arc<EventHub>, epoch: u64) -> Result<Self, ServerError> {
        let catalog = graph::prelude_catalog().map_err(ServerError::Prelude)?;
        let mut state = Self {
            definitions: BTreeMap::new(),
            events,
            epoch,
            catalog,
            root: config.root.clone(),
            revision_ring: config.revision_ring,
            temp_serial: 0,
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
                state.set_load_error(
                    name,
                    EvalError {
                        file: path.display().to_string(),
                        line: 1,
                        col: 1,
                        message: "flow source exceeds 1 MiB".to_string(),
                    },
                );
                continue;
            }
            let source = std::fs::read_to_string(&path)
                .map_err(|error| ServerError::io("read flow source", error))?;
            state.set_source(name, source);
        }
        Ok(state)
    }

    pub(crate) fn put_source(&mut self, name: String, source: String) -> Result<SourceResult, ServerError> {
        self.temp_serial = self.temp_serial.wrapping_add(1);
        let path = self.root.join("flows").join(format!("{name}.splash"));
        atomic_write(&path, source.as_bytes(), self.temp_serial)?;
        Ok(self.set_source(name, source))
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
                        ("name".to_string(), JsonValue::String(name)),
                        ("revision".to_string(), JsonValue::U64(revision)),
                        ("canonical".to_string(), JsonValue::Bool(canonical)),
                    ])),
                );
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

    pub(crate) fn set_watched_oversize(&mut self, name: String, error: EvalError) {
        let path = self.root.join("flows").join(format!("{name}.splash"));
        if std::fs::metadata(path).is_ok_and(|metadata| metadata.len() > MAX_SOURCE_BYTES) {
            self.set_load_error(name, error);
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
        true
    }
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
) -> Result<(StateHandle, std::thread::JoinHandle<()>), ServerError> {
    let (tx, rx) = mpsc::channel::<Task>();
    let (ready_tx, ready_rx) = mpsc::channel();
    let thread_config = config.clone();
    let thread_events = events.clone();
    let join = std::thread::Builder::new()
        .name("flow-server-state".to_string())
        .spawn(move || {
            let mut state = match FlowState::build(&thread_config, thread_events.clone(), epoch) {
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
                    match FlowState::build(&thread_config, thread_events.clone(), epoch) {
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
