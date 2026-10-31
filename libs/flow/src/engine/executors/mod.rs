//! Node executors.
//!
//! `input`, `func`, `output` and `ask` are the core set and need nothing
//! outside the document model. The provider executors sit behind features:
//! `chat` and `gen` behind `ai`, `http` behind `http`, `publish` and
//! `archive` behind `media` (which implies `ai`). A graph that names a provider node in a build
//! without its feature fails that node with an explicit error.

pub mod ask;
pub mod bound;
#[cfg(feature = "media")]
pub mod archive;
#[cfg(feature = "ai")]
pub mod chat;
pub mod choose;
pub mod func;
#[cfg(feature = "ai")]
pub mod gen;
#[cfg(feature = "http")]
pub mod http;
pub mod input;
pub mod lineage;
pub mod map;
pub mod output;
#[cfg(feature = "media")]
pub mod publish;

use crate::{Node, PortValue, Value};

pub trait Executor {
    /// Start with the node's resolved inputs, each in its port's shape.
    fn start(&mut self, node: &Node, inputs: &[(String, PortValue)]) -> Result<(), String>;
    fn poll(&mut self) -> Poll;
    fn cancel(&mut self);
}

/// The value on `port` when it is a one port; `None` for a sequence.
#[cfg(feature = "media")]
pub(crate) fn one_input<'a>(inputs: &'a [(String, PortValue)], port: &str) -> Option<&'a Value> {
    inputs
        .iter()
        .find_map(|(name, value)| (name == port).then(|| value.one()).flatten())
}

/// Every input as one value: what a node whose ports are all one ports
/// reads. An ordered input here is refused outright, never read as its
/// first element.
#[cfg(any(feature = "ai", feature = "http"))]
pub(crate) fn one_inputs(node: &Node, inputs: &[(String, PortValue)]) -> Result<Vec<(String, Value)>, String> {
    inputs
        .iter()
        .map(|(port, value)| match value {
            PortValue::One(value) => Ok((port.clone(), value.clone())),
            PortValue::Seq(_) => Err(format!("`{}.{port}` is an ordered input, which this node takes one value on", node.id)),
        })
        .collect()
}

/// One value per port, for a node whose outputs are all one ports.
pub(crate) fn one_outputs(outputs: Vec<(String, Value)>) -> Vec<(String, PortValue)> {
    outputs.into_iter().map(|(port, value)| (port, PortValue::One(value))).collect()
}

#[derive(Clone, Debug)]
pub enum Poll {
    Pending,
    /// Progress of the whole node: `permille` is the executor's estimate of
    /// the complete operation and reaches 1000 only with `Done`.
    Progress { permille: u16, stage: String },
    /// The node entered or advanced a stage and its overall completion is
    /// unknown. `permille` is that stage's own fraction when the producer
    /// reports one; it restarts with every stage and is never node progress.
    Stage { stage: String, permille: Option<u16> },
    Delta { port: String, text: String },
    /// Finished: every port in its declared shape (an ordered port's whole
    /// sequence, possibly empty).
    Done(Vec<(String, PortValue)>),
    Failed(String),
}

pub(crate) fn param<'a>(node: &'a Node, name: &str) -> Option<&'a crate::Literal> {
    node.params
        .iter()
        .find_map(|(key, value)| (key == name).then_some(value))
}

#[cfg(any(feature = "ai", feature = "http"))]
pub(crate) fn string_param(node: &Node, name: &str) -> String {
    match param(node, name) {
        Some(crate::Literal::Str(value) | crate::Literal::Id(value)) => value.clone(),
        _ => String::new(),
    }
}
