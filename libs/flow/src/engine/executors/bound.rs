//! A container boundary: the value that crosses it, republished under the
//! boundary node's own port so the crossing is observable. An ordered
//! boundary republishes the whole sequence, empty included, in the port's
//! shape. Never publishes to an asset store; that stays a root `Output`'s
//! business.

use super::{Executor, Poll};
use crate::{Node, PortValue};

#[derive(Default)]
pub struct BoundExecutor {
    result: Option<Poll>,
}

impl Executor for BoundExecutor {
    fn start(&mut self, node: &Node, inputs: &[(String, PortValue)]) -> Result<(), String> {
        let value = inputs
            .iter()
            .find_map(|(port, value)| (port == "value").then_some(value.clone()))
            .ok_or_else(|| format!("boundary `{}` has no value", node.id))?;
        let port = node
            .outputs
            .first()
            .map(|port| port.name.clone())
            .ok_or_else(|| format!("boundary `{}` has no output port", node.id))?;
        self.result = Some(Poll::Done(vec![(port, value)]));
        Ok(())
    }

    fn poll(&mut self) -> Poll {
        self.result.take().unwrap_or(Poll::Pending)
    }

    fn cancel(&mut self) {}
}
