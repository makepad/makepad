use crate::graph::NodeId;
use crate::tensor::BufferUsage;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BufferSlice {
    pub usage: BufferUsage,
    pub offset_bytes: usize,
    pub size_bytes: usize,
}

#[derive(Clone, Debug, Default)]
pub struct ExecutionPlan {
    pub allocations: Vec<BufferSlice>,
    pub execution_order: Vec<NodeId>,
    pub reusable: bool,
}

impl ExecutionPlan {
    pub fn push_allocation(&mut self, slice: BufferSlice) {
        self.allocations.push(slice);
    }

    pub fn push_node(&mut self, id: NodeId) {
        self.execution_order.push(id);
    }
}
