pub mod ask;
pub mod archive;
pub mod chat;
pub mod func;
pub mod gen;
pub mod http;
pub mod input;
pub mod output;
pub mod publish;

use crate::{Node, Value};

pub trait Executor {
    fn start(&mut self, node: &Node, inputs: &[(String, Value)]) -> Result<(), String>;
    fn poll(&mut self) -> Poll;
    fn cancel(&mut self);
}

#[derive(Clone, Debug)]
pub enum Poll {
    Pending,
    Progress { permille: u16, stage: String },
    Delta { port: String, text: String },
    Done(Vec<(String, Value)>),
    Failed(String),
}

pub(crate) fn param<'a>(node: &'a Node, name: &str) -> Option<&'a crate::Literal> {
    node.params
        .iter()
        .find_map(|(key, value)| (key == name).then_some(value))
}

pub(crate) fn string_param(node: &Node, name: &str) -> String {
    match param(node, name) {
        Some(crate::Literal::Str(value) | crate::Literal::Id(value)) => value.clone(),
        _ => String::new(),
    }
}
