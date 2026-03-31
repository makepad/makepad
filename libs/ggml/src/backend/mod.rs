use crate::core::Status;
use crate::graph::Graph;
use crate::tensor::Tensor;

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

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum BackendBufferUsage {
    Any,
    Weights,
    Compute,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum BackendDeviceType {
    Cpu,
    Gpu,
    Igpu,
    Accel,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BackendDeviceCaps {
    pub asynchronous: bool,
    pub host_buffer: bool,
    pub buffer_from_host_ptr: bool,
    pub events: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BackendDeviceProps {
    pub name: String,
    pub description: String,
    pub memory_free: usize,
    pub memory_total: usize,
    pub device_type: Option<BackendDeviceType>,
    pub device_id: Option<String>,
    pub caps: BackendDeviceCaps,
}

pub trait BackendBufferType {
    fn name(&self) -> &str;
    fn alignment(&self) -> usize;
    fn max_size(&self) -> Option<usize> {
        None
    }
    fn is_host(&self) -> bool {
        false
    }
    fn alloc_size(&self, tensor: &Tensor) -> usize {
        tensor.nbytes()
    }
}

pub trait BackendBuffer {
    fn name(&self) -> &str;
    fn size(&self) -> usize;
    fn alignment(&self) -> usize;
    fn max_size(&self) -> Option<usize> {
        None
    }
    fn is_host(&self) -> bool {
        false
    }
    fn usage(&self) -> BackendBufferUsage {
        BackendBufferUsage::Any
    }
    fn set_usage(&mut self, _usage: BackendBufferUsage) {}
    fn clear(&mut self, _value: u8) {}
}

pub trait BackendGraphPlan {}

pub trait BackendEvent {
    fn synchronize(&self) -> Result<(), String>;
}

pub trait Backend {
    type BufferType: BackendBufferType;
    type Buffer: BackendBuffer;
    type Event: BackendEvent;
    type GraphPlan: BackendGraphPlan;

    fn info(&self) -> &BackendInfo;
    fn default_buffer_type(&self) -> &Self::BufferType;
    fn alloc_buffer(&self, size: usize) -> Result<Self::Buffer, String>;
    fn alignment(&self) -> usize;
    fn max_size(&self) -> Option<usize>;
    fn synchronize(&self) -> Result<(), String>;
    fn graph_plan_create(&self, _graph: &Graph) -> Result<Self::GraphPlan, String> {
        Err("graph planning not implemented".to_string())
    }
    fn graph_plan_compute(&self, _plan: &Self::GraphPlan) -> Status {
        Status::Failed
    }
    fn graph_compute(&self, _graph: &Graph) -> Status {
        Status::Failed
    }
}

pub mod metal;
