//! TRELLIS.2 mesh family.
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

pub mod trellis;
pub mod trellis_dino;
pub mod trellis_dit;
pub mod trellis_image;
pub mod trellis_mesh;
pub mod trellis_pipeline;
pub mod trellis_slat;
pub mod trellis_vae;
