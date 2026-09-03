use super::{Executor, Poll};
use crate::{Node, Value};

#[derive(Default)]
pub struct OutputExecutor {
    result: Option<Poll>,
}

impl Executor for OutputExecutor {
    fn start(&mut self, node: &Node, inputs: &[(String, Value)]) -> Result<(), String> {
        let value = inputs
            .iter()
            .find_map(|(port, value)| (port == "value").then_some(value.clone()))
            .ok_or_else(|| format!("Output node `{}` has no value", node.id))?;
        self.result = Some(Poll::Done(vec![("value".to_string(), value)]));
        Ok(())
    }

    fn poll(&mut self) -> Poll {
        self.result.take().unwrap_or(Poll::Pending)
    }

    fn cancel(&mut self) {}
}
