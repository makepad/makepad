//! The AI runtime's Vulkan compute backend (libs/ai/vulkan/ONDEVICE.md).
//!
//! Raw Vulkan through our generated bindings (`makepad-vulkan`): one compute
//! device, buffers reached by device address, and kernels written as Splash
//! compute functions (`kernels/*.splash`) that our own shader compiler turns
//! into SPIR-V. The model runtimes (`makepad-ai-llm`, the speech models)
//! execute their graphs and seams on it the way they do on Metal and CUDA.

pub mod device;
pub mod gemm;
pub mod gemm_int8;
pub mod gemm_quant;
pub mod kernel;
pub mod library;
pub mod memory;
pub mod stream;

pub use device::{CooperativeMatrixShape, Device, DeviceFeatures, Result};
pub use gemm::{ActivationType, Gemm, GemmCandidate, GemmMode, GEMM_CANDIDATES};
pub use gemm_int8::{Int8Gemm, Int8GemmMode, Int8Weights};
pub use gemm_quant::{BlockQuant, QuantizedMatmul};
pub use kernel::{Args, Kernel};
pub use library::{Library, KERNEL_SOURCES};
pub use memory::{Buffer, BufferKind};
pub use stream::Stream;
