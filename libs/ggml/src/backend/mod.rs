mod accel;
pub mod prof;

pub use accel::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum BackendKind {
    Metal,
    Vulkan,
    Cuda,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BackendCapabilities {
    pub bf16: bool,
    pub tensor_cores: bool,
    pub max_buffer_size: Option<usize>,
    pub max_threadgroup_memory: Option<usize>,
    pub subgroup_width: Option<usize>,
    pub asynchronous: bool,
    pub host_buffer: bool,
    pub buffer_from_host_ptr: bool,
    pub events: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BackendInfo {
    pub kind: BackendKind,
    pub name: String,
    pub description: String,
    pub capabilities: BackendCapabilities,
}

pub mod cuda;
pub mod llm_ops;
pub mod metal;
