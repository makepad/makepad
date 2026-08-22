//! Practical-RIFE v4.26 video frame interpolation (IFNet_HDv3).
//!
//! Re-exports the shared exec surface so `crate::backend` /
//! `crate::emit_progress` / `crate::error` resolve the same way they do in
//! the other `libs/ai/models/*` family crates.

pub use makepad_ai_common::backend;
pub use makepad_ai_common::error;
pub use makepad_ai_common::metal_accel;
pub use makepad_ai_common::torch_pth;
pub use makepad_ai_common::{
    band_progress, emit_byte_progress, emit_progress, f16_word_to_f32, hook_ref,
    BoxedProgressHook, DiffusionError, ProgressHook, Result, BYTE_PROGRESS_STEP,
};

pub mod rife;
pub mod rife_cpu;
mod rife_model;

pub use rife::{
    interpolation_timesteps, padded_extent, rife_device_available, unload_rife, Rife,
    RifeBackendKind, RifeCancel, RifeFlowField,
    RifeFramePair, RifeModelWeights, RifeScale, RifeWeights, RIFE_BLOCK_CHANNELS,
    RIFE_CACHE_NAMESPACE, RIFE_ENCODE_CHANNELS, RIFE_LRELU_SLOPE, RIFE_MODEL_PATH,
    RIFE_MODEL_SHA256, RIFE_MODEL_SIZE, RIFE_NUM_BLOCKS, RIFE_PAD_MULTIPLE, RIFE_REPO,
    RIFE_REVISION, RIFE_SCALE_LIST,
};
