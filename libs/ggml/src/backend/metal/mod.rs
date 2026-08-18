#[allow(dead_code, non_camel_case_types)]
mod affine;
mod compat;
mod compiled;
// Host-backed Metal GpuTensor (SAM3 / Klein / RealESRGAN). Only valid on
// macOS: it constructs the stub GpuTensor { rows, cols, data, u32s }. The
// Windows/Linux CUDA GpuTensor is a different private device type, so this
// module must not compile there.
#[cfg(target_os = "macos")]
pub mod gpu_tensor;
mod qmm;
mod runtime;
mod selector;

pub use affine::*;
pub use compat::*;
pub use compiled::*;
pub use qmm::{affine_qmm_enabled, bench_steel_isolated, SteelBenchResult};
pub use runtime::*;
pub use selector::*;
