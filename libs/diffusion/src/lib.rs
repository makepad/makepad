mod error;

mod assets;
mod metal_accel;

pub mod backend;
pub mod birefnet;
pub mod realesrgan;
mod realesrgan_model;
mod birefnet_model;
pub mod sam3;
mod sam3_model;
pub mod clip;
pub mod clip_l;
pub mod comfy;
pub mod da3;
pub mod flux;
pub mod flux_gguf;
pub mod flux_pipeline;
pub mod flux_schedule;
pub mod flux_text;
pub mod flux_transformer;
pub mod flux_vae;
pub mod flux2;
pub mod flux2_klein_text;
pub mod flux2_pipeline;
pub mod flux2_text;
pub mod flux2_tokenizer;
pub mod flux2_transformer;
pub mod flux2_vae;
pub mod h3;
pub mod h3_audio_vae;
pub mod h3_image;
pub mod h3_pipeline;
pub mod h3_quant;
pub mod h3_text;
pub mod h3_tokenizer;
pub mod h3_transformer;
pub mod h3_vae;
pub mod hy_motion;
pub mod hy_motion_clip;
pub mod hy_motion_decode;
pub mod hy_motion_pipeline;
pub mod hy_motion_text;
pub mod hy_motion_transformer;
pub mod hy_motion_weights;
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
pub mod moss;
pub mod moss_dac;
pub mod music3;
pub mod music3_ar;
pub mod music3_dit;
pub mod music3_gguf;
pub mod music3_gguf_gen;
pub mod music3_lm;
pub mod music3_pipeline;
pub mod music3_quant;
pub mod music3_rvq;
pub mod music3_vocoder;
pub mod music3_weights;
pub mod moss_dit;
pub mod moss_pipeline;
pub mod moss_text;
pub mod sa3;
pub mod sa3_ae;
pub mod sa3_pipeline;
pub mod sa3_text;
pub mod sa3_tokenizer;
pub mod sa3_transformer;
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
pub mod torch_pth;
pub mod t5;
pub mod t5_encoder;
pub mod woosh;
pub mod woosh_ae;
pub mod woosh_dit;
pub mod woosh_pipeline;
pub mod woosh_text;
pub mod woosh_tokenizer;
pub mod ace;
pub mod ace_dit;
pub mod ace_pipeline;
pub mod ace_text;
pub mod ace_vae;
pub mod trellis;
pub mod trellis_dino;
pub mod trellis_dit;
pub mod trellis_image;
pub mod trellis_mesh;
pub mod trellis_pipeline;
pub mod trellis_slat;
pub mod trellis_vae;

pub use error::{DiffusionError, Result};

/// Fine-grained progress hook threaded through long pipeline phases:
/// `(label, phase-local fraction 0..=1)`. Returning `Err` (typically
/// [`DiffusionError::Cancelled`]) unwinds the run at the next emission
/// boundary. Emission sites throttle themselves (per layer/block, or per
/// 256MB of weight bytes) so the hook never sits on a hot path.
pub type ProgressHook<'a> = &'a mut dyn FnMut(&str, f64) -> Result<()>;

/// Emits through an optional [`ProgressHook`]; no-op when absent.
pub fn emit_progress(
    progress: &mut Option<ProgressHook>,
    label: &str,
    fraction: f64,
) -> Result<()> {
    match progress {
        Some(hook) => hook(label, fraction),
        None => Ok(()),
    }
}

/// Owned form of [`ProgressHook`] for band-remapping wrappers.
pub type BoxedProgressHook<'a> = Box<dyn FnMut(&str, f64) -> Result<()> + 'a>;

/// Remaps a callee's phase-local 0..=1 fraction onto the `[base, base + span]`
/// band of the caller's own progress scale, forwarding labels as-is. Returns
/// None (a free no-op for the callee) when the hook is absent.
pub fn band_progress<'a>(
    progress: &'a mut Option<ProgressHook<'_>>,
    base: f64,
    span: f64,
) -> Option<BoxedProgressHook<'a>> {
    progress.as_mut().map(|hook| -> BoxedProgressHook<'a> {
        Box::new(move |label: &str, fraction: f64| {
            hook(label, base + fraction.clamp(0.0, 1.0) * span)
        })
    })
}

/// Reborrows a boxed band hook as the `Option<ProgressHook>` callees take.
pub fn hook_ref<'a>(sub: &'a mut Option<BoxedProgressHook<'_>>) -> Option<ProgressHook<'a>> {
    sub.as_mut().map(|hook| &mut **hook as ProgressHook)
}

/// Weight-streaming loops emit at most once per this many cumulative bytes.
pub(crate) const BYTE_PROGRESS_STEP: usize = 256 * 1024 * 1024;

/// Emits "`phase` 3.2/9.5GB" with fraction `done/total`; no-op when the hook
/// is absent (the label is not even formatted).
pub(crate) fn emit_byte_progress(
    progress: &mut Option<ProgressHook>,
    phase: &str,
    done: usize,
    total: usize,
) -> Result<()> {
    if progress.is_none() {
        return Ok(());
    }
    const GB: f64 = 1024.0 * 1024.0 * 1024.0;
    let fraction = if total == 0 {
        1.0
    } else {
        done as f64 / total as f64
    };
    emit_progress(
        progress,
        &format!("{phase} {:.1}/{:.1}GB", done as f64 / GB, total as f64 / GB),
        fraction,
    )
}
