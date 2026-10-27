use super::{param, Executor, Poll};
use crate::{Literal, Node, PortValue, Sequence, Value};

#[derive(Default)]
pub struct InputExecutor {
    result: Option<Poll>,
}

impl Executor for InputExecutor {
    /// The caller's value for the port, else the design default: for a one
    /// port its literal; for an ordered port its literal array element by
    /// element, or the typed empty sequence when nothing is authored.
    fn start(&mut self, node: &Node, inputs: &[(String, PortValue)]) -> Result<(), String> {
        let output = node
            .outputs
            .first()
            .ok_or_else(|| format!("Input node `{}` has no output", node.id))?;
        let supplied = inputs
            .iter()
            .find_map(|(port, value)| (port == &output.name).then_some(value.clone()));
        let default = param(node, "value").or_else(|| param(node, "default"));
        let value = match (supplied, output.many) {
            (Some(value), _) => value,
            (None, Some(_)) => PortValue::Seq(match default {
                Some(Literal::Arr(items)) => Sequence {
                    ty: output.ty.clone(),
                    values: items
                        .iter()
                        .map(|item| Value::from_literal(output.ty.clone(), item))
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(|error| format!("Input node `{}` default: {error}", node.id))?,
                },
                _ => Sequence::empty(output.ty.clone()),
            }),
            (None, None) => PortValue::One(
                default
                    .filter(|default| !matches!(default, Literal::Null))
                    .map(|default| Value::from_literal(output.ty.clone(), default))
                    .transpose()
                    .ok()
                    .flatten()
                    .ok_or_else(|| format!("Input node `{}` has no value", node.id))?,
            ),
        };
        value
            .matches(output.shape())
            .map_err(|error| format!("Input node `{}`: {error}", node.id))?;
        self.result = Some(Poll::Done(vec![(output.name.clone(), value)]));
        Ok(())
    }

    fn poll(&mut self) -> Poll {
        self.result.take().unwrap_or(Poll::Pending)
    }

    fn cancel(&mut self) {}
}
