//! IndexTTS-2.5 speech family (kokoro joins in T6d).
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

pub mod indextts;
pub mod indextts_bigvgan;
pub mod indextts_campplus;
pub mod indextts_codec;
pub mod indextts_gpt;
pub mod indextts_mel;
pub mod indextts_pipeline;
pub mod indextts_s2mel;
pub mod indextts_tokenizer;
pub mod indextts_w2v;
