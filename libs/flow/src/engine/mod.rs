//! The run engine: run events, inputs, provider seams and the shared
//! [`scheduler::RunTask`], plus `spawn_run*`, which drives one task on a
//! thread of its own. No host schedules runs that way: the HTTP planes, the
//! Stage window and the headless host all use the engine's run lanes.

pub mod executors;
#[cfg(feature = "http")]
pub mod http_seam;
pub mod scheduler;
pub mod lower;
pub mod providers;
pub mod results;
#[cfg(feature = "media")]
pub mod thumbnail;

use crate::digest::Sha256;
use crate::document::closure::Closure;
use crate::{Graph, HttpLogEntryDto, Literal, PortType, PortValue, Value};
#[cfg(feature = "ai")]
use executors::chat::ChatSeam;
#[cfg(feature = "ai")]
use makepad_ai_hub::job::GenRouter;
#[cfg(feature = "http")]
use executors::http::HttpSeam;
use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{channel, Sender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;
#[cfg(any(not(target_arch = "wasm32"), feature = "http"))]
use std::time::{SystemTime, UNIX_EPOCH};

pub use crate::{NodeState, RunState};
#[cfg(feature = "ai")]
pub use executors::chat::{ChatEvent, ChatTurn, HubChat};
#[cfg(feature = "http")]
pub use http_seam::HubHttp;
pub use scheduler::{RunStep, RunTask};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RunId(pub String);

#[derive(Clone, Debug)]
pub enum RunEvent {
    RunStarted {
        run_id: RunId,
        instance: String,
        flow: String,
        revision: u64,
        planned_nodes: Vec<String>,
    },
    NodeStarted {
        node: String,
    },
    /// `permille` is the whole node's progress and is absent while that is
    /// unknown; `stage_permille` is the named stage's own fraction.
    NodeProgress {
        node: String,
        permille: Option<u16>,
        stage: String,
        stage_permille: Option<u16>,
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
    /// Every port the node completed with, in its shape (an ordered port's
    /// whole sequence, empty included).
    NodeDone {
        node: String,
        outputs: Vec<(String, PortValue)>,
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
        /// Every requested result that produced, in its shape.
        outputs: Vec<(String, PortValue)>,
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

/// Longest text one event may carry in a telemetry field (a delta, an
/// error, a stage, a warning). Longer text is cut and says so.
pub const EVENT_TEXT_MAX: usize = 64 * 1024;
/// Most records one event's telemetry list (http log, warnings) may carry.
pub const EVENT_LIST_MAX: usize = 256;
/// Most bytes one event's telemetry list may carry in total.
pub const EVENT_LIST_BYTES_MAX: usize = 256 * 1024;
/// Most metadata bytes (everything but value bytes) one event may carry
/// once its telemetry is cut. Semantic fields (a run's plan, an ask's
/// options) are never cut: they are bounded by the source size limit, and
/// an event past this limit is refused explicitly, not trimmed. Every
/// transport budget is floored here so one event can always travel.
pub const EVENT_META_MAX: usize =
    2 * crate::graph::MAX_SOURCE_BYTES + 2 * EVENT_LIST_BYTES_MAX + 2 * EVENT_TEXT_MAX;

/// Bytes a literal keeps alive: its text, or a word for a scalar.
fn literal_bytes(literal: &Literal) -> usize {
    match literal {
        Literal::Null | Literal::Bool(_) | Literal::Num(_) => 8,
        Literal::Str(text) | Literal::Id(text) => text.capacity(),
        Literal::Arr(items) => {
            items.capacity() * std::mem::size_of::<Literal>() + items.iter().map(literal_bytes).sum::<usize>()
        }
        Literal::Obj(fields) => {
            fields.capacity() * std::mem::size_of::<(String, Literal)>()
                + fields.iter().map(|(key, value)| key.capacity() + literal_bytes(value)).sum::<usize>()
        }
    }
}

/// Cut a text to the limit and release the backing allocation it no
/// longer needs: what a queue retains is capacity, not length.
fn bound_text(text: &mut String, lost: &mut usize) {
    if text.len() <= EVENT_TEXT_MAX {
        if text.capacity() > text.len() + EVENT_TEXT_MAX {
            text.shrink_to_fit();
        }
        return;
    }
    let mut cut = EVENT_TEXT_MAX;
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    *lost += text.len() - cut;
    text.truncate(cut);
    text.push_str("… [cut]");
    text.shrink_to_fit();
}

/// Keep a telemetry list within the record and byte caps, oldest first.
/// Returns how many records were dropped.
fn bound_records<T>(list: &mut Vec<T>, len_of: impl Fn(&T) -> usize, keep_tail: usize) -> usize {
    let max_len = EVENT_LIST_MAX.saturating_sub(keep_tail);
    let max_bytes = EVENT_LIST_BYTES_MAX.saturating_sub(keep_tail * EVENT_NOTE_MAX);
    let mut total = 0;
    let mut keep = 0;
    for item in list.iter() {
        let bytes = len_of(item);
        if keep >= max_len || total + bytes > max_bytes {
            break;
        }
        total += bytes;
        keep += 1;
    }
    let dropped = list.len() - keep;
    list.truncate(keep);
    if list.capacity() > list.len() * 2 + 8 {
        list.shrink_to_fit();
    }
    dropped
}

/// Longest note the engine itself appends to a terminal event.
const EVENT_NOTE_MAX: usize = 256;

fn http_entry_bytes(entry: &HttpLogEntry) -> usize {
    24 + entry.method.capacity() + entry.url.capacity()
}

impl RunEvent {
    /// Metadata bytes this event keeps alive while it waits in a queue:
    /// every string, list record and literal it carries, without values.
    /// Strings and lists count their allocation, not their length: that is
    /// what a queue retains.
    pub fn meta_bytes(&self) -> usize {
        let strings = |items: &Vec<String>| {
            items.capacity() * std::mem::size_of::<String>()
                + items.iter().map(String::capacity).sum::<usize>()
        };
        let ports = |items: &Vec<(String, PortValue)>| {
            items.capacity() * std::mem::size_of::<(String, PortValue)>()
                + items
                    .iter()
                    .map(|(port, value)| {
                        port.capacity()
                            + value.values().len() * std::mem::size_of::<Value>()
                            + value.values().iter().map(|value| value.content_type.len()).sum::<usize>()
                    })
                    .sum::<usize>()
        };
        match self {
            Self::RunStarted { run_id, instance, flow, planned_nodes, .. } => {
                run_id.0.capacity() + instance.capacity() + flow.capacity() + strings(planned_nodes)
            }
            Self::NodeStarted { node } => node.capacity(),
            Self::NodeProgress { node, stage, .. } => node.capacity() + stage.capacity(),
            Self::NodeDelta { node, port, text } => node.capacity() + port.capacity() + text.capacity(),
            Self::NodeWaiting { node, question, options, .. } => {
                node.capacity()
                    + question.capacity()
                    + options.capacity() * std::mem::size_of::<Literal>()
                    + options.iter().map(literal_bytes).sum::<usize>()
            }
            Self::NodeAnswered { node, by } => node.capacity() + by.capacity(),
            Self::NodeDone { node, outputs } => node.capacity() + ports(outputs),
            Self::NodeFailed { node, error } => node.capacity() + error.capacity(),
            Self::NodeSkipped { node, reason } => node.capacity() + reason.capacity(),
            Self::RunFinished { outputs, http_log, warnings, .. } => {
                ports(outputs)
                    + http_log.capacity() * std::mem::size_of::<HttpLogEntry>()
                    + http_log.iter().map(http_entry_bytes).sum::<usize>()
                    + strings(warnings)
            }
        }
    }

    /// Value bytes this event keeps alive. Values are shared (`Arc`) with
    /// the store, but a queued event keeps them alive past eviction, so
    /// they count in full against the media budget.
    pub fn media_bytes(&self) -> usize {
        match self {
            Self::NodeDone { outputs, .. } | Self::RunFinished { outputs, .. } => {
                outputs.iter().map(|(_, value)| value.bytes_len()).sum()
            }
            _ => 0,
        }
    }

    /// Everything this event keeps alive: metadata and values.
    pub fn payload_bytes(&self) -> usize {
        self.meta_bytes() + self.media_bytes()
    }

    /// Cut this event's telemetry (texts, the http log, warnings) to the
    /// per-event limits. Semantic fields are untouched. Returns how many
    /// bytes or records were cut.
    pub fn bound_telemetry(&mut self) -> usize {
        let mut lost = 0;
        match self {
            Self::NodeProgress { stage, .. } => bound_text(stage, &mut lost),
            Self::NodeDelta { text, .. } => bound_text(text, &mut lost),
            Self::NodeWaiting { question, .. } => bound_text(question, &mut lost),
            Self::NodeFailed { error, .. } => bound_text(error, &mut lost),
            Self::NodeSkipped { reason, .. } => bound_text(reason, &mut lost),
            Self::RunFinished { http_log, warnings, .. } => {
                lost += bound_records(http_log, http_entry_bytes, 0);
                for warning in warnings.iter_mut() {
                    bound_text(warning, &mut lost);
                }
                lost += bound_records(warnings, String::capacity, 0);
            }
            Self::RunStarted { .. } | Self::NodeStarted { .. } | Self::NodeAnswered { .. } | Self::NodeDone { .. } => {}
        }
        lost
    }

    /// Append the engine's own notes to a terminal event and keep the
    /// whole within the telemetry caps: the notes always fit, earlier
    /// warnings make room for them. A non-terminal event is left alone.
    pub fn finish_terminal(&mut self, notes: Vec<String>) {
        let Self::RunFinished { warnings, .. } = self else {
            return;
        };
        if notes.is_empty() {
            return;
        }
        let tail = notes.len().min(EVENT_LIST_MAX);
        bound_records(warnings, String::capacity, tail);
        for mut note in notes.into_iter().take(tail) {
            if note.len() > EVENT_NOTE_MAX {
                let mut cut = EVENT_NOTE_MAX;
                while !note.is_char_boundary(cut) {
                    cut -= 1;
                }
                note.truncate(cut);
                note.shrink_to_fit();
            }
            warnings.push(note);
        }
    }

    /// Whether this event is a run's lifecycle rather than progress or a
    /// text delta. Lifecycle is what the store needs to know a node's fate.
    pub fn is_lifecycle(&self) -> bool {
        !matches!(self, Self::NodeProgress { .. } | Self::NodeDelta { .. })
    }
    pub fn to_wire(&self) -> crate::RunEventPayload {
        use crate::RunEventPayload as Wire;
        match self {
            Self::RunStarted {
                run_id,
                instance,
                flow,
                revision,
                planned_nodes,
            } => Wire::RunStarted {
                run_id: run_id.0.clone(),
                instance: instance.clone(),
                flow: flow.clone(),
                revision: *revision,
                planned_nodes: planned_nodes.clone(),
            },
            Self::NodeStarted { node } => Wire::NodeStarted { node: node.clone() },
            Self::NodeProgress {
                node,
                permille,
                stage,
                stage_permille,
            } => Wire::NodeProgress {
                node: node.clone(),
                permille: *permille,
                stage: stage.clone(),
                stage_permille: *stage_permille,
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
                ty: ty.clone(),
                options: options.clone(),
            },
            Self::NodeAnswered { node, by } => Wire::NodeAnswered {
                node: node.clone(),
                by: by.clone(),
            },
            // The one place values split for the wire: one ports keep the
            // scalar `outputs` form, ordered ports go to `sequences`.
            Self::NodeDone { node, outputs } => {
                let mut ones = Vec::new();
                let mut sequences = Vec::new();
                for (port, value) in outputs {
                    match value {
                        PortValue::One(value) => ones.push(crate::PortValueRef { port: port.clone(), value: value.into() }),
                        PortValue::Seq(sequence) => sequences.push(crate::PortSequenceRef {
                            port: port.clone(),
                            ty: sequence.ty.clone(),
                            values: sequence.values.iter().map(Into::into).collect(),
                        }),
                    }
                }
                Wire::NodeDone {
                    node: node.clone(),
                    outputs: ones,
                    sequences: (!sequences.is_empty()).then_some(sequences),
                }
            }
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
            } => {
                let mut ones = Vec::new();
                let mut sequences = Vec::new();
                for (name, value) in outputs {
                    match value {
                        PortValue::One(value) => ones.push((name.clone(), value.into())),
                        PortValue::Seq(sequence) => sequences.push((name.clone(), crate::SequenceRef::from(sequence))),
                    }
                }
                Wire::RunFinished {
                    state: *state,
                    secs: *secs,
                    outputs: ones,
                    sequences: (!sequences.is_empty()).then_some(sequences),
                    http_log: http_log.iter().map(Into::into).collect(),
                    warnings: warnings.clone(),
                }
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct RunInput {
    pub run_id: RunId,
    pub instance: String,
    pub source: String,
    pub file_name: String,
    /// The module closure `source` evaluates with: the same immutable
    /// closure the definition's revision was evaluated with.
    pub closure: Arc<Closure>,
    /// Immutable document and executor snapshots shared by every engine revision.
    pub registry: Arc<crate::document::Registry>,
    pub executors: Arc<providers::ExecutorRegistry>,
    pub graph_revision: u64,
    /// The authored graph (containers as nodes); the run lowers it.
    pub graph: Graph,
    /// A caller's values for node ports, each in its port's shape.
    pub inputs: BTreeMap<String, BTreeMap<String, PortValue>>,
    pub outputs: Option<Vec<String>>,
    pub origin: (String, u64),
}

/// The provider seams a run may call. Which fields exist follows the
/// executor features, so a `document`/`runtime` build has none.
#[derive(Clone)]
pub struct Seams {
    #[cfg(feature = "ai")]
    pub chat: Arc<dyn ChatSeam>,
    #[cfg(feature = "ai")]
    pub gen: Arc<dyn GenRouter>,
    #[cfg(feature = "http")]
    pub http: Arc<dyn HttpSeam>,
}

impl Seams {
    #[cfg(all(feature = "ai", feature = "http"))]
    pub fn real() -> Self {
        Self {
            chat: Arc::new(HubChat::from_env()),
            gen: Arc::new(makepad_ai_hub::job::FleetRouter::default()),
            http: Arc::new(HubHttp),
        }
    }

    /// Seams for a host that configured no provider. Available in every
    /// feature set; a provider node that reaches one fails with an explicit
    /// error instead of silently doing nothing.
    pub fn unconfigured() -> Self {
        Self {
            #[cfg(feature = "ai")]
            chat: Arc::new(unconfigured::NoChat),
            #[cfg(feature = "ai")]
            gen: Arc::new(unconfigured::NoGen),
            #[cfg(feature = "http")]
            http: Arc::new(unconfigured::NoHttp),
        }
    }
}

#[cfg(any(feature = "ai", feature = "http"))]
mod unconfigured {
    #[cfg(feature = "ai")]
    pub(super) struct NoChat;
    #[cfg(feature = "ai")]
    impl super::ChatSeam for NoChat {
        fn start_turn(
            &self,
            _system: &str,
            _prompt: &str,
            _model: &str,
            _max_tokens: Option<u32>,
            _thinking: Option<bool>,
        ) -> Result<Box<dyn super::ChatTurn>, String> {
            Err("this host configured no chat provider".to_string())
        }
    }

    #[cfg(feature = "ai")]
    pub(super) struct NoGen;
    #[cfg(feature = "ai")]
    impl super::GenRouter for NoGen {
        fn route(
            &self,
            _domain: &str,
            _request: &makepad_ai_hub::protocol::GenerateRequestJson,
            _excluded: &[String],
        ) -> Result<makepad_ai_hub::job::GenPick, String> {
            Err("this host configured no generation provider".to_string())
        }
    }

    #[cfg(feature = "http")]
    pub(super) struct NoHttp;
    #[cfg(feature = "http")]
    impl super::HttpSeam for NoHttp {
        fn request(
            &self,
            _req: super::executors::http::HttpReq,
        ) -> Result<super::executors::http::HttpResp, String> {
            Err("this host configured no HTTP provider".to_string())
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
    spawn_run_with_policy_and_results(input, seams, events, policy, None)
}

/// Drive one run on a thread of its own, polling every 100 ms. For callers
/// that run a single graph by hand; hosts dispatch onto the engine's run
/// lanes (`runtime::lanes`), which step the same [`RunTask`].
pub fn spawn_run_with_policy_and_results(
    input: RunInput,
    seams: Seams,
    events: Sender<RunEvent>,
    policy: NetPolicy,
    results: Option<Arc<dyn results::ResultStore>>,
) -> RunHandle {
    let cancel = Arc::new(AtomicBool::new(false));
    let thread_cancel = cancel.clone();
    let (answer, answers) = channel();
    let join = thread::Builder::new()
        .name(format!("flow-{}", input.run_id.0))
        .spawn(move || {
            let mut sink = |event: RunEvent| {
                let _ = events.send(event);
            };
            let Some(mut task) =
                RunTask::start(input, seams, policy, thread_cancel, results, &mut sink)
            else {
                return;
            };
            loop {
                for (node, value) in answers.try_iter() {
                    task.answer(node, value);
                }
                if task.step(&mut sink) == RunStep::Finished {
                    return;
                }
                thread::sleep(Duration::from_millis(100));
            }
        })
        .expect("spawn flow run thread");
    RunHandle {
        cancel,
        answer,
        join,
    }
}

// Read by the native run store and the Http executor's log.
#[cfg(any(not(target_arch = "wasm32"), feature = "http"))]
pub(crate) fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

pub(crate) fn graph_digest(graph: &Graph) -> [u8; 32] {
    use makepad_micro_serde::SerJson;
    let mut sha = Sha256::new();
    sha.update(graph.serialize_json().as_bytes());
    sha.finish()
}
