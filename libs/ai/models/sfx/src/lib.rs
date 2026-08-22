//! SA3 / MOSS / Woosh sound-effect family.
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

pub mod sa3;
pub mod sa3_ae;
pub mod sa3_pipeline;
pub mod sa3_text;
pub mod sa3_tokenizer;
pub mod sa3_transformer;
pub mod moss;
pub mod moss_dac;
pub mod moss_dit;
pub mod moss_pipeline;
pub mod moss_text;
pub mod woosh;
pub mod woosh_ae;
pub mod woosh_dit;
pub mod woosh_pipeline;
pub mod woosh_text;
pub mod woosh_tokenizer;
