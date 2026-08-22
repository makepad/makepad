//! THE CUDA store: driver/cuDNN FFI, every .cu kernel + nvcc build, the
//! dense `gpu_*` launch surface, quant compute, and llm `mkllm_*` ops.
//! Plan of record: /aiarch.md §1 + §4.

pub mod accel;
pub mod cudnn;
pub mod cudnn_v8_bench;
pub mod driver;
pub mod launch;
pub mod llm_ops;
pub mod prof;
pub mod quant;
pub mod quant_iq;
pub mod quant_iq_tables;
pub mod roformer_ops;

pub use driver::*;
// `launch` is an empty stub off linux/windows-with-kernels (e.g. this
// macOS build), so the glob has nothing to re-export there.
#[allow(unused_imports)]
pub use launch::*;
// Both modules export `is_available`. Prefer the driver probe (device
// count) at the crate root so `crate::is_available()` is unambiguous on
// linux/windows where launch.rs is not an empty stub.
pub use driver::is_available;
