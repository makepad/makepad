pub mod executors;
pub mod http_seam;
pub mod scheduler;

use crate::graph::FlowVm;
use crate::{Graph, HttpLogEntryDto, Literal, PortType, Value};
use executors::chat::ChatSeam;
use executors::gen::GenSeam;
use executors::http::HttpSeam;
use makepad_ai_hub::sha256::Sha256;
use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{channel, Sender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{SystemTime, UNIX_EPOCH};

pub use crate::{NodeState, RunState};
pub use executors::chat::{ChatEvent, ChatTurn, HubChat};
pub use executors::gen::{FixedGen, FleetGen};
pub use http_seam::HubHttp;

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RunId(pub String);

#[derive(Clone, Debug)]
pub enum RunEvent {
    RunStarted {
        run_id: RunId,
        instance: String,
        flow: String,
        revision: u64,
    },
    NodeStarted {
        node: String,
    },
    NodeProgress {
        node: String,
        permille: u16,
        stage: String,
    },
    NodeDelta {
        node: String,
        port: String,
        text: String,
    },
    NodeWaiting {
        node: String,
        question: String,
        ty: PortType,
        options: Vec<Literal>,
    },
    NodeAnswered {
        node: String,
        by: String,
    },
    NodeDone {
        node: String,
        outputs: Vec<(String, Value)>,
    },
    NodeFailed {
        node: String,
        error: String,
    },
    NodeSkipped {
        node: String,
        reason: String,
    },
    RunFinished {
        state: RunState,
        secs: f64,
        outputs: Vec<(String, Value)>,
        http_log: Vec<HttpLogEntry>,
        warnings: Vec<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HttpLogEntry {
    pub ms: u64,
    pub method: String,
    pub url: String,
    pub status: Option<u16>,
}

impl From<&HttpLogEntry> for HttpLogEntryDto {
    fn from(value: &HttpLogEntry) -> Self {
        Self {
            ms: value.ms,
            method: value.method.clone(),
            url: value.url.clone(),
            status: value.status,
        }
    }
}

impl RunEvent {
    pub fn to_wire(&self) -> crate::RunEventPayload {
        use crate::RunEventPayload as Wire;
        match self {
            Self::RunStarted {
                run_id,
                instance,
                flow,
                revision,
            } => Wire::RunStarted {
                run_id: run_id.0.clone(),
                instance: instance.clone(),
                flow: flow.clone(),
                revision: *revision,
            },
            Self::NodeStarted { node } => Wire::NodeStarted { node: node.clone() },
            Self::NodeProgress {
                node,
                permille,
                stage,
            } => Wire::NodeProgress {
                node: node.clone(),
                permille: *permille,
                stage: stage.clone(),
            },
            Self::NodeDelta { node, port, text } => Wire::NodeDelta {
                node: node.clone(),
                port: port.clone(),
                text: text.clone(),
            },
            Self::NodeWaiting {
                node,
                question,
                ty,
                options,
            } => Wire::NodeWaiting {
                node: node.clone(),
                question: question.clone(),
                ty: *ty,
                options: options.clone(),
            },
            Self::NodeAnswered { node, by } => Wire::NodeAnswered {
                node: node.clone(),
                by: by.clone(),
            },
            Self::NodeDone { node, outputs } => Wire::NodeDone {
                node: node.clone(),
                outputs: outputs
                    .iter()
                    .map(|(port, value)| crate::PortValueRef {
                        port: port.clone(),
                        value: value.into(),
                    })
                    .collect(),
            },
            Self::NodeFailed { node, error } => Wire::NodeFailed {
                node: node.clone(),
                error: error.clone(),
            },
            Self::NodeSkipped { node, reason } => Wire::NodeSkipped {
                node: node.clone(),
                reason: reason.clone(),
            },
            Self::RunFinished {
                state,
                secs,
                outputs,
                http_log,
                warnings,
            } => Wire::RunFinished {
                state: *state,
                secs: *secs,
                outputs: outputs
                    .iter()
                    .map(|(name, value)| (name.clone(), value.into()))
                    .collect(),
                http_log: http_log.iter().map(Into::into).collect(),
                warnings: warnings.clone(),
            },
        }
    }
}

#[derive(Clone, Debug)]
pub struct RunInput {
    pub run_id: RunId,
    pub instance: String,
    pub source: String,
    pub file_name: String,
    pub graph_revision: u64,
    pub graph: Graph,
    pub inputs: BTreeMap<String, BTreeMap<String, Value>>,
    pub outputs: Option<Vec<String>>,
    pub origin: (String, u64),
}

#[derive(Clone)]
pub struct Seams {
    pub chat: Arc<dyn ChatSeam>,
    pub gen: Arc<dyn GenSeam>,
    pub http: Arc<dyn HttpSeam>,
}

impl Seams {
    pub fn real() -> Self {
        Self {
            chat: Arc::new(HubChat::from_env()),
            gen: Arc::new(FleetGen),
            http: Arc::new(HubHttp),
        }
    }
}

pub struct RunHandle {
    pub cancel: Arc<AtomicBool>,
    pub answer: Sender<(String, Value)>,
    pub join: JoinHandle<()>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetPolicy {
    pub allow: Vec<String>,
    pub deny_private: bool,
}

impl Default for NetPolicy {
    fn default() -> Self {
        Self {
            allow: vec!["*".to_string()],
            deny_private: false,
        }
    }
}

pub fn spawn_run(input: RunInput, seams: Seams, events: Sender<RunEvent>) -> RunHandle {
    spawn_run_with_policy(input, seams, events, NetPolicy::default())
}

pub fn spawn_run_with_policy(
    input: RunInput,
    seams: Seams,
    events: Sender<RunEvent>,
    policy: NetPolicy,
) -> RunHandle {
    let cancel = Arc::new(AtomicBool::new(false));
    let thread_cancel = cancel.clone();
    let (answer, answers) = channel();
    let join = thread::Builder::new()
        .name(format!("flow-{}", input.run_id.0))
        .spawn(move || {
            let started = std::time::Instant::now();
            let flow = std::path::Path::new(&input.file_name)
                .file_stem()
                .and_then(|name| name.to_str())
                .unwrap_or(&input.file_name)
                .to_string();
            let _ = events.send(RunEvent::RunStarted {
                run_id: input.run_id.clone(),
                instance: input.instance.clone(),
                flow,
                revision: input.graph_revision,
            });
            let loaded = FlowVm::load(&input.source, &input.file_name);
            let (mut vm, mut run_graph) = match loaded {
                Ok(loaded) => loaded,
                Err(error) => {
                    let _ = events.send(RunEvent::RunFinished {
                        state: RunState::Failed,
                        secs: started.elapsed().as_secs_f64(),
                        outputs: Vec::new(),
                        http_log: Vec::new(),
                        warnings: vec![format!("run source evaluation failed: {error}")],
                    });
                    return;
                }
            };
            run_graph.revision = input.graph.revision;
            if graph_digest(&run_graph) != graph_digest(&input.graph) {
                let _ = events.send(RunEvent::RunFinished {
                    state: RunState::Failed,
                    secs: started.elapsed().as_secs_f64(),
                    outputs: Vec::new(),
                    http_log: Vec::new(),
                    warnings: vec!["run source graph does not match requested graph".to_string()],
                });
                return;
            }
            scheduler::run(
                input,
                &mut vm,
                seams,
                policy,
                events,
                answers,
                thread_cancel,
            );
        })
        .expect("spawn flow run thread");
    RunHandle {
        cancel,
        answer,
        join,
    }
}

pub(crate) fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn graph_digest(graph: &Graph) -> [u8; 32] {
    use makepad_micro_serde::SerJson;
    let mut sha = Sha256::new();
    sha.update(graph.serialize_json().as_bytes());
    sha.finish()
}
