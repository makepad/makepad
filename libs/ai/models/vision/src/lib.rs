//! SAM3 / BiRefNet / DA3 / Real-ESRGAN vision family.
//! Re-exports the shared exec surface so existing `crate::backend` /
//! `crate::emit_progress` / `crate::error` paths inside moved modules
//! keep compiling unchanged.

pub use makepad_ai_common::backend;
pub use makepad_ai_common::error;
pub use makepad_ai_common::metal_accel;
pub use makepad_ai_common::torch_pth;
pub use makepad_ai_common::{
    band_progress, emit_byte_progress, emit_progress, f16_word_to_f32, hook_ref,
    BoxedProgressHook, DiffusionError, ProgressHook, Result, BYTE_PROGRESS_STEP,
};

pub mod birefnet;
mod birefnet_model;
pub mod da3;
pub mod realesrgan;
mod realesrgan_model;
pub mod sam3;
mod sam3_model;
