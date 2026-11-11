//! TripoSplat and Idol run through one native Rust/CUDA pipeline: one PNG
//! becomes a 3D Gaussian splat PLY using either release's checkpoints.
//!
//! Layering follows the other `libs/ai/models/*` family crates: this module
//! re-exports the shared exec surface, [`splat`] owns the pinned manifest and
//! the sampler/embedder math, [`splat_ops`] is the one tensor layer the model
//! code is written against (portable CPU reference + CUDA device path), and
//! [`splat_pipeline`] wires the stages together.
//!
//! Background removal is deliberately NOT in this crate: the service already
//! owns a native BiRefNet lane, so the backend mattes on the same worker and
//! hands this crate an RGBA image with a meaningful alpha.

pub use makepad_ai_common::backend;
pub use makepad_ai_common::error;
pub use makepad_ai_common::{
    band_progress, emit_byte_progress, emit_progress, f16_word_to_f32, hook_ref,
    BoxedProgressHook, DiffusionError, ProgressHook, Result, BYTE_PROGRESS_STEP,
};

pub mod splat;
pub mod splat_decoder;
pub mod splat_dino;
pub mod splat_flow;
pub mod splat_image;
pub mod splat_ops;
pub mod splat_pipeline;
pub mod splat_ply;
pub mod splat_rand;

pub use splat::{
    resolve_num_gaussians, SplatCancel, SplatModel, SplatWeights,
    GAUSSIANS_MIN, SPLAT_CANVAS, TRIPOSPLAT_DECODER_PATH,
    TRIPOSPLAT_DECODER_SHA256, TRIPOSPLAT_DECODER_SIZE, TRIPOSPLAT_DINO_PATH,
    TRIPOSPLAT_DINO_SHA256, TRIPOSPLAT_DINO_SIZE, TRIPOSPLAT_DIT_PATH, TRIPOSPLAT_DIT_SHA256,
    TRIPOSPLAT_DIT_SIZE, TRIPOSPLAT_REPO, TRIPOSPLAT_REVISION, TRIPOSPLAT_RMBG_PATH,
    TRIPOSPLAT_RMBG_SHA256, TRIPOSPLAT_RMBG_SIZE, TRIPOSPLAT_VAE_PATH, TRIPOSPLAT_VAE_SHA256,
    TRIPOSPLAT_VAE_SIZE,
};
pub use splat_image::SplatImage;
pub use splat_ops::Device;
pub use splat_pipeline::{SplatCondition, SplatParams, SplatPipeline};
pub use splat_ply::{write_ply, PlySplat};

/// True when this machine can actually run the model. There is no CPU
/// production path: the reference forward is ~1.9B parameters of f32 and the
/// CPU tensor layer exists for the unit tests and tiny-config forwards, not
/// for serving.
pub fn splat_device_available() -> bool {
    backend::gpu_device_available()
}
