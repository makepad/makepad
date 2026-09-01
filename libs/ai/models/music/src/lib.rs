//! MiniMax-Music3 + ACE-Step music family.
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

pub mod music3;
pub mod music3_ar;
pub mod music3_dit;
pub mod music3_gguf;
pub mod music3_gguf_gen;
pub mod music3_lm;
pub mod music3_pipeline;
pub mod music3_quant;
pub mod music3_reference;
pub mod music3_rvq;
pub mod music3_vocoder;
pub mod music3_weights;
pub mod ace;
pub mod ace_dit;
pub mod ace_pipeline;
pub mod ace_text;
pub mod ace_vae;
