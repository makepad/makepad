use super::{param, Executor, Poll};
use crate::{Literal, Node, Value};

#[derive(Default)]
pub struct InputExecutor {
    result: Option<Poll>,
}

impl Executor for InputExecutor {
    fn start(&mut self, node: &Node, inputs: &[(String, Value)]) -> Result<(), String> {
        let output = node
            .outputs
            .first()
            .ok_or_else(|| format!("Input node `{}` has no output", node.id))?;
        let value = inputs
            .iter()
            .find_map(|(port, value)| (port == &output.name).then_some(value.clone()))
            .or_else(|| {
                let default = param(node, "value").or_else(|| param(node, "default"))?;
                (!matches!(default, Literal::Null))
                    .then(|| Value::from_literal(output.ty, default))
                    .transpose()
                    .ok()
                    .flatten()
            })
            .ok_or_else(|| format!("Input node `{}` has no value", node.id))?;
        self.result = Some(Poll::Done(vec![(output.name.clone(), value)]));
        Ok(())
    }

    fn poll(&mut self) -> Poll {
        self.result.take().unwrap_or(Poll::Pending)
    }

    fn cancel(&mut self) {}
}
