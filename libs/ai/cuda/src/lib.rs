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

pub use driver::*;
pub use launch::*;
