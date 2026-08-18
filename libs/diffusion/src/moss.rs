//! MOSS-SoundEffect v2.0 shared foundation.
//!
//! Text-to-audio SFX model from OpenMOSS (Apache 2.0), the second model of
//! the audio domain beside SA3 Small SFX — picked for output variety
//! (full-band 48 kHz, denser/'produced' event character vs SA3's dry
//! one-shots).
//!
//! Architecture (all dims pinned from the HF repo configs + oracle dumps):
//! - text encoder: Qwen3-1.7B causal LM, last hidden state POST final norm,
//!   prompt right-padded to 512 tokens, pad rows zeroed after encoding;
//! - DiT: 1.3B Wan-audio (30 blocks, dim 1536, 12 heads x 128, ffn 8960,
//!   AdaLN 6-way modulation + per-block modulation bias, self-attn with
//!   full-width RMS q/k norm + interleaved 1D RoPE theta 1e4, ungated
//!   cross-attn onto the 512-token context, GELU-tanh FFN), patch size 1
//!   (the Conv1d patch embed is a plain linear), in/out 128 channels;
//! - flow matching: 100 steps, sigmas linspace(1,0,101)[:100] shifted by
//!   sigma' = 5 sigma / (1 + 4 sigma), CFG scale 4 with empty negative
//!   prompt, x += v * (sigma_next - sigma);
//! - VAE: MOSS-trained continuous DAC decoder (post_quant_conv + 2048-dim
//!   decoder, rates [8,5,4,3,2] = hop 960 -> 50 latents/s), mono 48 kHz,
//!   weight-norm convs + Snake activations, final tanh;
//! - the pipeline always denoises the FULL 30 s latent (1500 frames) and
//!   crops the decoded waveform to the requested seconds. Duration is
//!   conditioned through the PROMPT TEXT: `"{prompt} duration: {sec:.1f}s"`.

use std::path::Path;

use crate::error::{DiffusionError, Result};
use crate::h3::H3ShardedWeights;

pub const MOSS_SAMPLE_RATE: usize = 48_000;
pub const MOSS_MAX_SECONDS: usize = 30;
/// DAC hop: product of the decoder rates [8,5,4,3,2].
pub const MOSS_HOP: usize = 960;
/// Latent frames for the full 30 s window: 48000*30/960.
pub const MOSS_LATENT_FRAMES: usize = MOSS_SAMPLE_RATE * MOSS_MAX_SECONDS / MOSS_HOP;
pub const MOSS_LATENT_DIM: usize = 128;

pub const MOSS_DIT_DIM: usize = 1536;
pub const MOSS_DIT_LAYERS: usize = 30;
pub const MOSS_DIT_HEADS: usize = 12;
pub const MOSS_DIT_HEAD_DIM: usize = 128;
pub const MOSS_DIT_FFN: usize = 8960;
pub const MOSS_DIT_FREQ_DIM: usize = 256;
pub const MOSS_DIT_EPS: f32 = 1e-6;
pub const MOSS_ROPE_THETA: f64 = 10_000.0;

pub const MOSS_TEXT_TOKENS: usize = 512;
pub const MOSS_TE_HIDDEN: usize = 2048;
pub const MOSS_TE_LAYERS: usize = 28;
pub const MOSS_TE_Q_HEADS: usize = 16;
pub const MOSS_TE_KV_HEADS: usize = 8;
pub const MOSS_TE_HEAD_DIM: usize = 128;
pub const MOSS_TE_FFN: usize = 6144;
pub const MOSS_TE_ROPE_THETA: f64 = 1_000_000.0;
pub const MOSS_TE_RMS_EPS: f32 = 1e-6;
pub const MOSS_TE_PAD_ID: u32 = 151_643;

pub const MOSS_DEFAULT_STEPS: usize = 100;
pub const MOSS_DEFAULT_CFG: f32 = 4.0;
pub const MOSS_DEFAULT_SHIFT: f32 = 5.0;
pub const MOSS_NUM_TRAIN_TIMESTEPS: f32 = 1000.0;

pub const MOSS_DAC_DECODER_DIM: usize = 2048;
pub const MOSS_DAC_RATES: [usize; 5] = [8, 5, 4, 3, 2];

/// Formats the model's duration-suffixed prompt exactly like the reference
/// (`f"{prompt.strip()} duration: {seconds:.1f}s"`).
pub fn moss_format_prompt(prompt: &str, seconds: f64) -> String {
    format!("{} duration: {:.1}s", prompt.trim(), seconds)
}

/// FlowMatchScheduler sigmas (shift form), with the terminal 0 appended:
/// `sigmas[i]` for i in 0..steps, `sigmas[steps] = 0`.
/// Reference: linspace(1, 0, steps+1)[:-1], then s*sig/(1+(s-1)*sig).
pub fn moss_sigmas(steps: usize, shift: f32) -> Vec<f32> {
    let mut sigmas = Vec::with_capacity(steps + 1);
    for i in 0..steps {
        // torch.linspace(1, 0, steps+1)[i]
        let sigma = 1.0 - i as f32 / steps as f32;
        sigmas.push(shift * sigma / (1.0 + (shift - 1.0) * sigma));
    }
    sigmas.push(0.0);
    sigmas
}

/// Timesteps fed to the DiT: sigma * 1000 for the first `steps` sigmas.
pub fn moss_timesteps(sigmas: &[f32]) -> Vec<f32> {
    sigmas[..sigmas.len() - 1]
        .iter()
        .map(|s| s * MOSS_NUM_TRAIN_TIMESTEPS)
        .collect()
}

/// `sinusoidal_embedding_1d(dim, t)` — COS half first, then SIN half.
/// sinusoid_i = t * 10000^(-i/(dim/2)) for i in 0..dim/2.
pub fn moss_sinusoid(dim: usize, t: f64) -> Vec<f32> {
    let half = dim / 2;
    let mut out = vec![0f32; dim];
    for i in 0..half {
        let freq = 10_000f64.powf(-(i as f64) / half as f64);
        let arg = t * freq;
        out[i] = arg.cos() as f32;
        out[half + i] = arg.sin() as f32;
    }
    out
}

/// Interleaved-rope cos/sin tables for `positions` x `dim/2` frequencies:
/// freq_j = theta^(-2j/dim). Applied to (even, odd) element PAIRS.
pub fn moss_rope_tables(positions: usize, dim: usize, theta: f64) -> (Vec<f32>, Vec<f32>) {
    let half = dim / 2;
    let mut cos = vec![0f32; positions * half];
    let mut sin = vec![0f32; positions * half];
    for pos in 0..positions {
        for j in 0..half {
            let freq = theta.powf(-((2 * j) as f64) / dim as f64);
            let arg = pos as f64 * freq;
            cos[pos * half + j] = arg.cos() as f32;
            sin[pos * half + j] = arg.sin() as f32;
        }
    }
    (cos, sin)
}

/// Applies INTERLEAVED rope in place to one row laid out `[head0 d, head1 d, ..]`
/// with per-head dim `dim`; pairs are (2j, 2j+1) inside each head.
pub fn moss_rope_apply_interleaved(
    row: &mut [f32],
    pos: usize,
    heads: usize,
    dim: usize,
    cos: &[f32],
    sin: &[f32],
) {
    let half = dim / 2;
    let c = &cos[pos * half..(pos + 1) * half];
    let s = &sin[pos * half..(pos + 1) * half];
    for h in 0..heads {
        let base = h * dim;
        for j in 0..half {
            let e = row[base + 2 * j];
            let o = row[base + 2 * j + 1];
            row[base + 2 * j] = e * c[j] - o * s[j];
            row[base + 2 * j + 1] = e * s[j] + o * c[j];
        }
    }
}

/// Applies HALF-ROTATION (NeoX/llama) rope in place, pairs (j, j+half) inside
/// each head — the Qwen3 text-encoder convention.
pub fn moss_rope_apply_half(
    row: &mut [f32],
    pos: usize,
    heads: usize,
    dim: usize,
    cos: &[f32],
    sin: &[f32],
) {
    let half = dim / 2;
    let c = &cos[pos * half..(pos + 1) * half];
    let s = &sin[pos * half..(pos + 1) * half];
    for h in 0..heads {
        let base = h * dim;
        for j in 0..half {
            let a = row[base + j];
            let b = row[base + half + j];
            row[base + j] = a * c[j] - b * s[j];
            row[base + half + j] = a * s[j] + b * c[j];
        }
    }
}

/// Half-rotation rope tables (freq_j = theta^(-2j/dim), j in 0..dim/2) — the
/// same frequency ladder as interleaved; only the pairing differs.
pub fn moss_rope_tables_half(positions: usize, dim: usize, theta: f64) -> (Vec<f32>, Vec<f32>) {
    moss_rope_tables(positions, dim, theta)
}

/// Opens a weights dir of safetensors shards (works for both the 2-shard
/// text encoder dir and the single-file transformer dir).
pub fn moss_open_shards(dir: impl AsRef<Path>) -> Result<H3ShardedWeights> {
    H3ShardedWeights::load(dir.as_ref()).map_err(|e| {
        DiffusionError::model(format!(
            "moss weights {}: {e:?}",
            dir.as_ref().display()
        ))
    })
}

/// True when the CUDA device path should be used (device present and not
/// disabled with MOSS_DEVICE=0).
pub fn moss_device_enabled() -> bool {
    if std::env::var("MOSS_DEVICE").map(|v| v == "0").unwrap_or(false) {
        return false;
    }
    makepad_ggml::backend::cuda::gpu_device_available()
}
