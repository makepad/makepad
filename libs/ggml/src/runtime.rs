use crate::backend::BackendInfo;
use crate::graph::Graph;
use crate::plan::ExecutionPlan;

#[derive(Clone, Debug, Default)]
pub struct RuntimeConfig;

#[derive(Clone, Debug)]
pub struct CompiledGraph {
    pub graph: Graph,
    pub plan: ExecutionPlan,
    pub backend: BackendInfo,
}

impl CompiledGraph {
    pub fn new(graph: Graph, plan: ExecutionPlan, backend: BackendInfo) -> Self {
        Self {
            graph,
            plan,
            backend,
        }
    }
}
