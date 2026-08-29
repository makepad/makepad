//! MiniMax-H3 video family (bf16 / q4-gguf / nvfp4).
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

pub mod h3;
pub mod h3_audio_vae;
pub mod h3_image;
pub mod h3_pipeline;
pub mod h3_quant;
pub mod h3_quant_writer;
pub mod h3_text;
pub mod h3_tokenizer;
pub mod h3_transformer;
pub mod h3_vae;
