use super::{param, Executor, Poll};
use crate::{Literal, Node, PortType, Value};
use std::time::{Duration, Instant};

#[derive(Default)]
pub struct AskExecutor {
    node: String,
    ty: Option<PortType>,
    answer: Option<Value>,
    deadline: Option<Instant>,
    cancelled: bool,
}

impl AskExecutor {
    pub fn answer(&mut self, node: &str, value: Value) -> Result<bool, String> {
        if self.node != node {
            return Ok(false);
        }
        if self.answer.is_some() {
            return Ok(false);
        }
        let ty = self.ty.ok_or_else(|| "Ask executor is not started".to_string())?;
        if value.ty != ty {
            return Err(format!(
                "type mismatch for Ask `{node}`: expected {}, got {}",
                ty.as_str(),
                value.ty.as_str()
            ));
        }
        self.answer = Some(value);
        Ok(true)
    }
}

impl Executor for AskExecutor {
    fn start(&mut self, node: &Node, _inputs: &[(String, Value)]) -> Result<(), String> {
        let output = node
            .outputs
            .first()
            .ok_or_else(|| format!("Ask node `{}` has no output", node.id))?;
        self.node = node.id.clone();
        self.ty = Some(output.ty);
        self.deadline = match param(node, "timeout") {
            Some(Literal::Num(seconds)) if *seconds > 0.0 => {
                Some(Instant::now() + Duration::from_secs_f64(*seconds))
            }
            _ => None,
        };
        Ok(())
    }

    fn poll(&mut self) -> Poll {
        if self.cancelled {
            return Poll::Failed("cancelled".to_string());
        }
        if let Some(value) = self.answer.take() {
            let port = self.ty.unwrap().as_str().to_string();
            return Poll::Done(vec![(port, value)]);
        }
        if self.deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            return Poll::Failed("timeout".to_string());
        }
        Poll::Pending
    }

    fn cancel(&mut self) {
        self.cancelled = true;
    }
}
