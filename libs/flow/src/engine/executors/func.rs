use super::{Executor, Poll};
use crate::graph::FlowVm;
use crate::{Node, Value};

#[derive(Default)]
pub struct FuncExecutor {
    result: Option<Poll>,
}

impl FuncExecutor {
    pub fn start_with_vm(
        &mut self,
        vm: &mut FlowVm,
        node: &Node,
        inputs: &[(String, Value)],
    ) {
        self.result = Some(match vm.call_fn(&node.id, inputs) {
            Ok(outputs) => Poll::Done(outputs),
            Err(error) => Poll::Failed(error),
        });
    }
}

impl Executor for FuncExecutor {
    fn start(&mut self, _node: &Node, _inputs: &[(String, Value)]) -> Result<(), String> {
        Err("Fn executor must be started with its run VM".to_string())
    }

    fn poll(&mut self) -> Poll {
        self.result.take().unwrap_or(Poll::Pending)
    }

    fn cancel(&mut self) {}
}
