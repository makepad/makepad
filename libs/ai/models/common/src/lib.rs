//! Cross-family internals shared by the AI model family crates split out of
//! `libs/diffusion` (lanes T6a–T6, /aiarch.md §1).
//!
//! Shared because every family needs them, not because they are a runtime
//! abstraction: `DiffusionError`, the sharded-safetensors reader, the ggml
//! `backend` re-export surface, host `.pth` / Metal-GEMM helpers, and the
//! progress-hook types. Family-private internals stay in the family crates.

pub mod backend;
pub mod dtype;
pub mod error;
pub mod json;
pub mod raw_st;
pub mod metal_accel;
pub mod progress;
pub mod sharded;
pub mod torch_pth;

pub use dtype::f16_word_to_f32;
pub use error::{DiffusionError, Result};
pub use progress::{
    band_progress, emit_byte_progress, emit_progress, hook_ref, BoxedProgressHook,
    ProgressHook, BYTE_PROGRESS_STEP,
};
