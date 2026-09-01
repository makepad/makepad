//! SkinTokens rig family.
//! Re-exports the shared exec surface so existing `crate::backend` /
//! `crate::emit_progress` / `crate::error` paths inside moved modules
//! keep compiling unchanged.

#![allow(dead_code)] // a lane crate keeps reference and debug paths beside the shipping ones
pub use makepad_ai_common::backend;
pub use makepad_ai_common::error;
pub use makepad_ai_common::metal_accel;
pub use makepad_ai_common::torch_pth;
pub use makepad_ai_common::{
    band_progress, emit_byte_progress, emit_progress, f16_word_to_f32, hook_ref,
    BoxedProgressHook, DiffusionError, ProgressHook, Result, BYTE_PROGRESS_STEP,
};

pub mod skin_tokens;
pub mod skin_tokens_condition;
pub mod skin_tokens_convert;
pub mod skin_tokens_decode;
pub mod skin_tokens_mesh;
pub mod skin_tokens_neural;
pub mod skin_tokens_output;
pub mod skin_tokens_pipeline;
pub mod skin_tokens_qwen;
pub mod skin_tokens_tokenizer;
