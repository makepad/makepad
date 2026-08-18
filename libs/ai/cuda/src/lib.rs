//! makepad-ai-cuda: THE CUDA store. Driver FFI (absorbed from libs/cuda),
//! cuDNN bindings, and every .cu kernel + the nvcc build (absorbed from
//! libs/ggml/src/backend/cuda). Plan of record: /aiarch.md §1 + §4.
//!
//! The Rust launch surface (libs/ggml/src/backend/cuda/mod.rs) stays in
//! makepad-ggml; this crate only supplies the compiled kernel objects, the
//! driver/cudnn FFI, and the link-lib/cfg plumbing that mod.rs consumes via
//! `makepad_cuda::` (the libs/cuda facade) and `cfg(makepad_ggml_cuda_kernels)`.

pub mod cudnn;
pub mod cudnn_v8_bench;
pub mod driver;

pub use driver::*;
