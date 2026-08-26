//! THE Metal store: shaders + metallib build, device/runtime, try_* shims,
//! and the host GpuTensor path. Compiled-graph execution lives in
//! makepad-ai-llm (the graph is that model).

pub mod affine;
pub mod backend_kind;
pub mod gpu_types;
#[cfg(target_os = "macos")]
pub mod gpu_tensor;
pub mod runtime;
pub mod shim;
pub mod rife;

pub use affine::*;
pub use backend_kind::*;
pub use gpu_types::{GpuLinearPart, GpuTensor};
pub use runtime::*;
pub use shim::*;
