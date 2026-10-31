use crate::engine::RunId;
use crate::{Graph, Literal, Node, PortType, PortValue, Sequence, Shape, Value};
use crate::digest::Sha256;
use std::collections::{BTreeMap, VecDeque};
use std::time::{SystemTime, UNIX_EPOCH};

pub use crate::document::ident::InstanceId;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Owner {
    Tab,
    Chat { lease: String },
    Service,
    Auto,
    /// Owned explicitly by an embedding application. Idle sweeps never
    /// remove it; the application deletes it or stops its engine. This is
    /// independent of whether its definition revision is pinned.
    Application,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Waiting {
    pub run: RunId,
    pub node: String,
    pub question: String,
    pub ty: PortType,
    pub options: Vec<Literal>,
}

#[derive(Clone, Debug)]
pub struct Instance {
    pub id: InstanceId,
    pub flow: String,
    pub label: Option<String>,
    pub revision: u64,
    pub pinned: bool,
    /// Every port a caller set, in the port's shape.
    pub inputs: BTreeMap<String, BTreeMap<String, PortValue>>,
    pub outputs: BTreeMap<String, PortValue>,
    pub runs: VecDeque<RunId>,
    pub active: Vec<RunId>,
    pub waiting: Option<Waiting>,
    pub created_ms: u64,
    pub last_activity_ms: u64,
    pub owner: Owner,
    concurrency: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InputEffect {
    None,
    TriggerRun,
    Answered(RunId),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunDecision {
    Start(RunId),
    Queued(usize),
    Busy,
}

impl Instance {
    pub fn new(
        flow: impl Into<String>,
        graph: &Graph,
        label: Option<String>,
        pinned: bool,
        owner: Owner,
        now_ms: u64,
    ) -> Result<Self, String> {
        let flow = flow.into();
        let mut instance = Self {
            id: InstanceId(make_id("inst", &[flow.as_bytes(), &now_ms.to_le_bytes()])),
            flow,
            label,
            revision: graph.revision,
            pinned,
            inputs: BTreeMap::new(),
            outputs: BTreeMap::new(),
            runs: VecDeque::new(),
            active: Vec::new(),
            waiting: None,
            created_ms: now_ms,
            last_activity_ms: now_ms,
            owner,
            concurrency: graph.concurrency,
        };
        instance.seed_input_defaults(graph)?;
        Ok(instance)
    }

    pub fn set_input(
        &mut self,
        node_id: &str,
        port: &str,
        value: PortValue,
        graph: &Graph,
    ) -> Result<InputEffect, String> {
        self.check_input(node_id, port, &value, graph)?;
        let answering = self.waiting.as_ref().is_some_and(|waiting| waiting.node == node_id);
        // A Choose's answer is this run's choice: kept, it would pick for
        // every later run without asking. A `pick` set while nothing waits
        // is an ordinary input and is kept.
        let choice = answering && graph.nodes.iter().any(|node| node.id == node_id && node.kind == "choose");
        if !choice {
            self.inputs
                .entry(node_id.to_string())
                .or_default()
                .insert(port.to_string(), value);
        }
        self.last_activity_ms = unix_ms();
        if answering {
            let run = self.waiting.take().unwrap().run;
            return Ok(InputEffect::Answered(run));
        }
        Ok(if graph.trigger == "input" {
            InputEffect::TriggerRun
        } else {
            InputEffect::None
        })
    }

    /// What `set_input` would do, without doing it: the type check and the
    /// Ask gate, so a batch can be validated before any of it is applied.
    /// The one admission rule for a value on any node port: the node and
    /// port exist and the value fits the port's shape (element type, one
    /// value or an ordered sequence within its bound). A one value never
    /// stands in for a sequence, nor a sequence for one value.
    pub fn check_input(
        &self,
        node_id: &str,
        port: &str,
        value: &PortValue,
        graph: &Graph,
    ) -> Result<(), String> {
        let node = graph
            .nodes
            .iter()
            .find(|node| node.id == node_id)
            .ok_or_else(|| format!("node `{node_id}` is not declared"))?;
        let shape = input_shape(node, port)
            .ok_or_else(|| format!("node `{node_id}` does not declare input port `{port}`"))?;
        value
            .matches(shape)
            .map_err(|error| format!("type mismatch for `{node_id}.{port}`: {error}"))
    }

    /// Whether a run requested now would queue behind the concurrency limit.
    pub fn would_queue(&self) -> bool {
        self.concurrency > 0 && self.active.len() >= self.concurrency as usize
    }

    /// Whether a run requested now would start rather than queue.
    pub fn would_start(&self) -> bool {
        self.concurrency > 0 && self.active.len() < self.concurrency as usize
    }

    pub fn on_graph_changed(&mut self, graph: &Graph) -> Result<(), String> {
        if self.pinned {
            return Ok(());
        }
        self.revision = graph.revision;
        self.concurrency = graph.concurrency;
        // An override outlives an edit only while it still fits the port:
        // a port that became ordered, or changed its element type, drops
        // what was set for its old shape.
        self.inputs.retain(|node_id, ports| {
            let Some(node) = graph.nodes.iter().find(|node| &node.id == node_id) else {
                return false;
            };
            ports.retain(|port, value| input_shape(node, port).is_some_and(|shape| value.matches(shape).is_ok()));
            !ports.is_empty()
        });
        self.seed_input_defaults(graph)
    }

    pub fn request_run(&mut self, outputs: Option<Vec<String>>) -> RunDecision {
        let revision = self.revision.to_le_bytes();
        let activity = self.last_activity_ms.to_le_bytes();
        let mut parts: Vec<&[u8]> = vec![
            self.id.0.as_bytes(),
            &revision,
            &activity,
        ];
        let output_text = outputs.unwrap_or_default().join("\0");
        parts.push(output_text.as_bytes());
        let ordinal = (self.active.len() + self.runs.len()) as u64;
        let ordinal = ordinal.to_le_bytes();
        parts.push(&ordinal);
        let run = RunId(make_id("run", &parts));
        self.last_activity_ms = unix_ms();
        if self.concurrency == 0 {
            return RunDecision::Busy;
        }
        if self.active.len() < self.concurrency as usize {
            self.active.push(run.clone());
            RunDecision::Start(run)
        } else {
            self.runs.push_back(run);
            RunDecision::Queued(self.runs.len())
        }
    }

    fn seed_input_defaults(&mut self, graph: &Graph) -> Result<(), String> {
        for node in graph.nodes.iter().filter(|node| node.kind == "input") {
            let Some(port) = node.outputs.first() else {
                continue;
            };
            if self
                .inputs
                .get(&node.id)
                .is_some_and(|ports| ports.contains_key(&port.name))
            {
                continue;
            }
            let default = param(node, "value").or_else(|| param(node, "default"));
            let ty = port.ty.clone();
            let value = match port.many {
                // An ordered Input starts as its authored elements, or as
                // the typed empty sequence: never as a one value.
                Some(_) => PortValue::Seq(match default {
                    Some(Literal::Arr(items)) => Sequence {
                        ty: ty.clone(),
                        values: items
                            .iter()
                            .map(|item| Value::from_literal(ty.clone(), item))
                            .collect::<Result<Vec<_>, _>>()?,
                    },
                    _ => Sequence::empty(ty),
                }),
                None => {
                    let Some(default) = default else {
                        continue;
                    };
                    if matches!(default, Literal::Null) {
                        continue;
                    }
                    PortValue::One(Value::from_literal(ty, default)?)
                }
            };
            self.inputs
                .entry(node.id.clone())
                .or_default()
                .insert(port.name.clone(), value);
        }
        Ok(())
    }
}

/// The shape a caller's value for `port` of `node` must fit: an Input's or
/// Ask's output port, any other node's input port.
pub fn input_shape(node: &Node, port: &str) -> Option<Shape> {
    if node.kind == "input" || node.kind == "ask" {
        node.outputs
            .iter()
            .find_map(|output| (output.name == port).then(|| output.shape()))
    } else {
        node.inputs
            .iter()
            .find_map(|input| (input.port == port).then(|| input.shape()))
    }
}

fn param<'a>(node: &'a Node, name: &str) -> Option<&'a Literal> {
    node.params
        .iter()
        .find_map(|(key, value)| (key == name).then_some(value))
}

fn make_id(prefix: &str, parts: &[&[u8]]) -> String {
    let mut sha = Sha256::new();
    for part in parts {
        sha.update(part);
    }
    sha.update(&unix_nanos().to_le_bytes());
    let digest = sha.finish();
    let mut suffix = String::with_capacity(16);
    for byte in &digest[..8] {
        suffix.push_str(&format!("{byte:02x}"));
    }
    format!("{prefix}_{suffix}")
}

fn unix_ms() -> u64 {
    unix_nanos().saturating_div(1_000_000) as u64
}

fn unix_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}
