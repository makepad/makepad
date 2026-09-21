//! THE CUDA store: driver/cuDNN FFI, every .cu kernel + nvcc build, the
//! dense `gpu_*` launch surface, and the llm `mkllm_*` ops.
//! Plan of record: /aiarch.md §1 + §4.

// Declared first, and with `#[macro_use]`, so `cuda_ffi!` is in scope for
// every module below: `macro_rules!` visibility is textual.
#[macro_use]
mod link_gate;

// The CPU quant kernels, the accel specs and the op profiler are backend-
// neutral and live in makepad-ai-loader; the launch surface reaches them as
// `crate::{quant, accel, prof}`. Nothing here re-exports them — consumers
// import from the loader, so a build without this crate loses nothing.
#[allow(unused_imports)] // launch.rs is an empty stub off linux/windows-with-kernels
use makepad_ai_loader::{accel, prof, quant};

pub mod cudnn;
pub mod cudnn_v8_bench;
pub mod driver;
pub mod launch;
pub mod llm_ops;
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
