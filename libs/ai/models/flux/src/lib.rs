//! FLUX.1 / FLUX.2 image family plus CLIP-L and T5 encoders.
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

mod assets;
pub mod clip;
pub mod clip_l;
pub mod comfy;
pub mod t5;
pub mod t5_encoder;
pub mod flux;
pub mod flux_fill_pipeline;
pub mod flux_gguf;
pub mod flux_lora;
pub mod flux_pipeline;
pub mod flux_schedule;
pub mod flux_text;
pub mod flux_transformer;
pub mod flux_vae;
pub mod flux2;
pub mod flux2_dev_text;
pub mod flux2_klein_text;
pub mod flux2_pipeline;
pub mod flux2_text;
pub mod flux2_tokenizer;
pub mod flux2_transformer;
pub mod flux2_vae;
