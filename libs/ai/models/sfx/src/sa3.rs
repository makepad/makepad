//! Stable Audio 3 Small SFX shared foundation: single-file safetensors weights,
//! the LogSNR sampling schedule, size math and the CPU f32 tensor helpers used
//! by the text encoder / DiT / autoencoder modules.
//!
//! Every formula mirrors the Stability-AI/stable-audio-3 reference exactly
//! (model.py / inference/sampling.py / inference/distribution_shift.py /
//! models/transformer.py); the oracle dumps live in local/sa3_ref/dumps and
//! are compared stage-by-stage by `sa3-validate`.

use crate::{DiffusionError, Result};
use makepad_ai_loader::{MlxDType, MlxSafetensorsHeader};
use std::path::Path;

pub const SA3_SAMPLE_RATE: usize = 44_100;
pub const SA3_AUDIO_CHANNELS: usize = 2;
/// 120s max (model_config sample_size).
pub const SA3_MAX_SAMPLE_SIZE: usize = 5_292_032;
/// Latent channels (DiT io_channels, AE latent_dim).
pub const SA3_LATENT_DIM: usize = 256;
/// Audio samples per latent frame (patch 256 x AE stride 16).
pub const SA3_DOWNSAMPLE: usize = 4096;
/// generate() pads the requested duration by this many seconds.
pub const SA3_DURATION_PAD_SECONDS: f64 = 6.0;
/// Audio size alignment: downsample * (encoder chunk 32 / encoder stride 16).
pub const SA3_SIZE_ALIGN: usize = SA3_DOWNSAMPLE * 2;

pub const SA3_DIT_DIM: usize = 1024;
pub const SA3_DIT_DEPTH: usize = 20;
pub const SA3_DIT_HEADS: usize = 16;
pub const SA3_HEAD_DIM: usize = 64;
pub const SA3_COND_DIM: usize = 768;
pub const SA3_COND_TOKENS: usize = 257;
pub const SA3_TEXT_TOKENS: usize = 256;
pub const SA3_MEMORY_TOKENS: usize = 64;
pub const SA3_LOCAL_COND_DIM: usize = 257;
pub const SA3_TIMESTEP_FEATURES: usize = 256;
/// Partial rotary: first 32 of the 64 head dims are rotated.
pub const SA3_ROPE_DIM: usize = 32;
pub const SA3_ROPE_BASE: f32 = 10_000.0;
/// DiT block norms (RMSNorm) epsilon.
pub const SA3_NORM_EPS: f32 = 1e-5;
/// qk norms epsilon (DiT rms / AE DyT-unused).
pub const SA3_QK_NORM_EPS: f32 = 1e-6;
/// seconds_total conditioning range.
pub const SA3_SECONDS_MAX: f64 = 384.0;
/// ExpoFourierFeatures frequency range (timestep + seconds conditioners).
pub const SA3_EXPO_MIN_FREQ: f32 = 0.5;
pub const SA3_EXPO_MAX_FREQ: f32 = 10_000.0;
/// Sampling LogSNRShift defaults (rate=0 -> sequence-length invariant).
pub const SA3_LOGSNR_START: f32 = -6.2;
pub const SA3_LOGSNR_END: f32 = 2.0;

// AE decoder geometry.
pub const SA3_AE_DIM: usize = 768;
pub const SA3_AE_DEPTH: usize = 6;
pub const SA3_AE_STRIDE: usize = 16;
pub const SA3_AE_PATCH: usize = 256;
pub const SA3_AE_PATCH_CHANNELS: usize = 512;
/// Tokens per latent group in the decoder: 1 latent + stride new tokens.
pub const SA3_AE_GROUP: usize = SA3_AE_STRIDE + 1;
/// Attention chunk in tokens: chunk_size(32) + 32*1/stride = 34.
pub const SA3_AE_CHUNK_TOKENS: usize = 34;
pub const SA3_AE_FF_INNER: usize = 2304;

// ---------------------------------------------------------------------------
// Size math (mirrors StableAudioModel.generate + data/utils.py).
// ---------------------------------------------------------------------------

/// Padded/aligned audio sample count for a requested duration.
pub fn sa3_audio_sample_size(seconds: f64) -> usize {
    let target = ((seconds + SA3_DURATION_PAD_SECONDS) * SA3_SAMPLE_RATE as f64) as usize;
    let target = target.div_ceil(SA3_DOWNSAMPLE) * SA3_DOWNSAMPLE;
    let target = target.div_ceil(SA3_SIZE_ALIGN) * SA3_SIZE_ALIGN;
    target.min(SA3_MAX_SAMPLE_SIZE)
}

pub fn sa3_latent_len(seconds: f64) -> usize {
    sa3_audio_sample_size(seconds) / SA3_DOWNSAMPLE
}

/// Effective (valid-content) latent length: ceil(int(sec*sr)/downsample).
pub fn sa3_effective_seq_len(seconds: f64) -> usize {
    let audio_samples = (seconds * SA3_SAMPLE_RATE as f64) as usize;
    audio_samples.div_ceil(SA3_DOWNSAMPLE)
}

/// Valid latent length for the attention padding mask
/// (effective + 6s headroom, clamped to the latent length).
pub fn sa3_valid_len(seconds: f64) -> usize {
    let headroom = (SA3_DURATION_PAD_SECONDS * SA3_SAMPLE_RATE as f64
        / SA3_DOWNSAMPLE as f64) as usize;
    (sa3_effective_seq_len(seconds) + headroom).min(sa3_latent_len(seconds))
}

/// Sampling sigmas, steps+1 entries from 1.0 down to 0.0: linspace warped by
/// LogSNRShift(rate=0, anchor_logsnr=-6.2, logsnr_end=2.0) with endpoints
/// preserved exactly (build_schedule + LogSNRShift.shift).
pub fn sa3_sigmas(steps: usize) -> Vec<f32> {
    let mut out = Vec::with_capacity(steps + 1);
    for i in 0..=steps {
        // torch.linspace(1, 0, steps+1)
        let t = 1.0 - i as f32 / steps as f32;
        if i == 0 {
            out.push(1.0);
        } else if i == steps {
            out.push(0.0);
        } else {
            let logsnr = SA3_LOGSNR_END - t * (SA3_LOGSNR_END - SA3_LOGSNR_START);
            out.push(1.0 / (1.0 + logsnr.exp()));
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Weights.
// ---------------------------------------------------------------------------

/// One safetensors file with f32 (or bf16, converted) tensor reads by name.
pub struct Sa3Tensors {
    header: MlxSafetensorsHeader,
}

impl Sa3Tensors {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let header = MlxSafetensorsHeader::load(path.as_ref()).map_err(|err| {
            DiffusionError::model(format!("sa3 weights {}: {err:?}", path.as_ref().display()))
        })?;
        Ok(Self { header })
    }

    pub fn shape(&self, name: &str) -> Result<Vec<usize>> {
        let entry = self
            .header
            .tensors
            .get(name)
            .ok_or_else(|| DiffusionError::model(format!("sa3 tensor missing: {name}")))?;
        Ok(entry.shape.iter().map(|&v| v as usize).collect())
    }

    pub fn has(&self, name: &str) -> bool {
        self.header.tensors.contains_key(name)
    }

    /// Reads a tensor as f32 (F32 directly, BF16 converted).
    pub fn f32(&self, name: &str) -> Result<Vec<f32>> {
        let entry = self
            .header
            .tensors
            .get(name)
            .ok_or_else(|| DiffusionError::model(format!("sa3 tensor missing: {name}")))?;
        let dtype = entry.dtype;
        let bytes = self
            .header
            .read_tensor_bytes(name)
            .map_err(|err| DiffusionError::model(format!("sa3 read {name}: {err:?}")))?;
        match dtype {
            MlxDType::F32 => Ok(bytes
                .chunks_exact(4)
                .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect()),
            MlxDType::BF16 => Ok(bytes
                .chunks_exact(2)
                .map(|c| f32::from_bits(u32::from(u16::from_le_bytes([c[0], c[1]])) << 16))
                .collect()),
            other => Err(DiffusionError::model(format!(
                "sa3 tensor {name}: unsupported dtype {other:?}"
            ))),
        }
    }

    /// f32 tensor with an exact-shape check.
    pub fn f32_shaped(&self, name: &str, shape: &[usize]) -> Result<Vec<f32>> {
        let actual = self.shape(name)?;
        if actual != shape {
            return Err(DiffusionError::model(format!(
                "sa3 tensor {name}: shape {actual:?}, expected {shape:?}"
            )));
        }
        self.f32(name)
    }
}

// ---------------------------------------------------------------------------
// CPU f32 helpers (shared by sa3_text / sa3_transformer / sa3_ae).
// ---------------------------------------------------------------------------

pub fn sa3_threads() -> usize {
    std::thread::available_parallelism()
        .map(|v| v.get())
        .unwrap_or(4)
}

/// Splits `out` into per-thread contiguous row chunks and runs `f(row, slice)`.
pub fn par_rows<F: Fn(usize, &mut [f32]) + Sync>(out: &mut [f32], row_len: usize, f: &F) {
    debug_assert_eq!(out.len() % row_len.max(1), 0);
    if row_len == 0 || out.is_empty() {
        return;
    }
    let rows = out.len() / row_len;
    let threads = sa3_threads().clamp(1, rows);
    if threads <= 1 {
        for (row, slice) in out.chunks_mut(row_len).enumerate() {
            f(row, slice);
        }
        return;
    }
    let rows_per = rows.div_ceil(threads);
    std::thread::scope(|scope| {
        let mut rest = out;
        let mut first = 0usize;
        while !rest.is_empty() {
            let take = (rows_per * row_len).min(rest.len());
            let (chunk, tail) = rest.split_at_mut(take);
            rest = tail;
            let start = first;
            first += take / row_len;
            scope.spawn(move || {
                for (offset, slice) in chunk.chunks_mut(row_len).enumerate() {
                    f(start + offset, slice);
                }
            });
        }
    });
}

/// out[m, n] = a[m, k] @ w[n, k]^T (+ bias[n]); torch nn.Linear layout.
pub fn linear(a: &[f32], w: &[f32], bias: Option<&[f32]>, m: usize, k: usize, n: usize) -> Vec<f32> {
    debug_assert_eq!(a.len(), m * k);
    debug_assert_eq!(w.len(), n * k);
    if let Some(out) = crate::metal_accel::linear_nt(a, w, bias, m, k, n) {
        return out;
    }
    let mut out = vec![0f32; m * n];
    par_rows(&mut out, n, &|row, slice| {
        let a_row = &a[row * k..(row + 1) * k];
        for (col, out_v) in slice.iter_mut().enumerate() {
            let w_row = &w[col * k..(col + 1) * k];
            let mut acc = 0f32;
            for i in 0..k {
                acc += a_row[i] * w_row[i];
            }
            *out_v = acc + bias.map_or(0.0, |b| b[col]);
        }
    });
    out
}

pub fn silu(x: f32) -> f32 {
    x / (1.0 + (-x).exp())
}

pub fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

/// tanh-approximated gelu (gelu_pytorch_tanh), used by the T5Gemma MLP.
pub fn gelu_tanh(x: f32) -> f32 {
    0.5 * x * (1.0 + (0.797_884_56_f32 * (x + 0.044_715 * x * x * x)).tanh())
}

/// RMSNorm x * rsqrt(mean(x^2)+eps) * gamma, computed in f32 (force_fp32).
pub fn rms_norm_rows(x: &mut [f32], gamma: &[f32], width: usize, eps: f32) {
    let dim = gamma.len();
    debug_assert_eq!(width, dim);
    if crate::metal_accel::rms_norm_mul_inplace(x, gamma, width, eps) {
        return;
    }
    for row in x.chunks_mut(width) {
        let mut sum = 0f32;
        for v in row.iter() {
            sum += v * v;
        }
        let scale = 1.0 / (sum / dim as f32 + eps).sqrt();
        for (v, g) in row.iter_mut().zip(gamma) {
            *v = *v * scale * g;
        }
    }
}

/// DynamicTanh: tanh(alpha*x) * gamma + beta (AE norms + AE qk norms).
pub(crate) fn dyt_rows(x: &mut [f32], alpha: f32, gamma: &[f32], beta: &[f32], width: usize) {
    debug_assert_eq!(width, gamma.len());
    for row in x.chunks_mut(width) {
        for i in 0..width {
            row[i] = (alpha * row[i]).tanh() * gamma[i] + beta[i];
        }
    }
}

/// ExpoFourierFeatures(dim, 0.5, 10000): [cos(t*f*2pi), sin(t*f*2pi)] with
/// f = exp(linspace(0,1,dim/2) * (ln(max)-ln(min)) + ln(min)).
pub(crate) fn expo_fourier(t: f32, dim: usize) -> Vec<f32> {
    let half = dim / 2;
    let log_min = SA3_EXPO_MIN_FREQ.ln();
    let log_max = SA3_EXPO_MAX_FREQ.ln();
    let mut out = vec![0f32; dim];
    for i in 0..half {
        let ramp = if half > 1 { i as f32 / (half - 1) as f32 } else { 0.0 };
        let freq = (ramp * (log_max - log_min) + log_min).exp();
        let arg = t * freq * 2.0 * std::f32::consts::PI;
        out[i] = arg.cos();
        out[half + i] = arg.sin();
    }
    out
}

/// Rotary tables for the shared partial-rotary scheme (DiT + AE blocks):
/// inv_freq[i] = base^(-2i/rope_dim) for i in 0..16; freqs duplicated to 32.
/// Returns (cos, sin), each positions x SA3_ROPE_DIM.
pub(crate) fn rope_tables(positions: usize) -> (Vec<f32>, Vec<f32>) {
    let half = SA3_ROPE_DIM / 2; // 16
    let mut cos = vec![0f32; positions * SA3_ROPE_DIM];
    let mut sin = vec![0f32; positions * SA3_ROPE_DIM];
    for pos in 0..positions {
        for i in 0..half {
            let inv = 1.0 / SA3_ROPE_BASE.powf(2.0 * i as f32 / SA3_ROPE_DIM as f32);
            let angle = pos as f32 * inv;
            let (s, c) = angle.sin_cos();
            cos[pos * SA3_ROPE_DIM + i] = c;
            cos[pos * SA3_ROPE_DIM + half + i] = c;
            sin[pos * SA3_ROPE_DIM + i] = s;
            sin[pos * SA3_ROPE_DIM + half + i] = s;
        }
    }
    (cos, sin)
}

/// Applies partial rotary to head vectors laid out as [tokens, heads, 64]:
/// first 32 dims rotated non-interleaved (rotate_half over 16+16), rest kept.
pub(crate) fn apply_rope(
    x: &mut [f32],
    cos: &[f32],
    sin: &[f32],
    tokens: usize,
    heads: usize,
) {
    let half = SA3_ROPE_DIM / 2;
    debug_assert_eq!(x.len(), tokens * heads * SA3_HEAD_DIM);
    for t in 0..tokens {
        let cos_row = &cos[t * SA3_ROPE_DIM..(t + 1) * SA3_ROPE_DIM];
        let sin_row = &sin[t * SA3_ROPE_DIM..(t + 1) * SA3_ROPE_DIM];
        for h in 0..heads {
            let base = (t * heads + h) * SA3_HEAD_DIM;
            for i in 0..half {
                let a = x[base + i];
                let b = x[base + half + i];
                // rotate_half([a-block, b-block]) = [-b, a]
                x[base + i] = a * cos_row[i] - b * sin_row[i];
                x[base + half + i] = b * cos_row[half + i] + a * sin_row[half + i];
            }
        }
    }
}

/// Plain softmax attention over one head-major layout:
/// q,k,v as [tokens, heads, 64] (kv_tokens for k/v), additive `key_mask`
/// (0 or -inf per key) optional. Returns [tokens, heads, 64].
pub fn attention(
    q: &[f32],
    k: &[f32],
    v: &[f32],
    q_tokens: usize,
    kv_tokens: usize,
    heads: usize,
    key_mask: Option<&[f32]>,
    scale: f32,
) -> Vec<f32> {
    let d = SA3_HEAD_DIM;
    if key_mask.is_none() {
        if let Some(out) =
            crate::metal_accel::flash_attn_packed(q, k, v, q_tokens, kv_tokens, heads, d, scale)
        {
            return out;
        }
    }
    let mut out = vec![0f32; q_tokens * heads * d];
    par_rows(&mut out, heads * d, &|qt, out_row| {
        let mut scores = vec![0f32; kv_tokens];
        for h in 0..heads {
            let q_vec = &q[(qt * heads + h) * d..(qt * heads + h + 1) * d];
            let mut max_score = f32::NEG_INFINITY;
            for (kt, score) in scores.iter_mut().enumerate() {
                let k_vec = &k[(kt * heads + h) * d..(kt * heads + h + 1) * d];
                let mut acc = 0f32;
                for i in 0..d {
                    acc += q_vec[i] * k_vec[i];
                }
                let mut s = acc * scale;
                if let Some(mask) = key_mask {
                    s += mask[kt];
                }
                *score = s;
                if s > max_score {
                    max_score = s;
                }
            }
            let mut denom = 0f32;
            for score in scores.iter_mut() {
                *score = (*score - max_score).exp();
                denom += *score;
            }
            let inv = 1.0 / denom;
            let out_vec = &mut out_row[h * d..(h + 1) * d];
            out_vec.fill(0.0);
            for (kt, &score) in scores.iter().enumerate() {
                let w = score * inv;
                let v_vec = &v[(kt * heads + h) * d..(kt * heads + h + 1) * d];
                for i in 0..d {
                    out_vec[i] += w * v_vec[i];
                }
            }
        }
    });
    out
}

// ---------------------------------------------------------------------------
// Device-path helpers (CUDA; stubbed elsewhere).
// ---------------------------------------------------------------------------

/// One f16-converted weight for the cached device linear path
/// (`gpu_linear_nt_cached`). The f16 conversion matches the reference's
/// shipping CUDA config (model_half=True); activations stay f32.
pub struct F16Weight {
    pub n: usize,
    pub key: String,
    pub bytes: Vec<u8>,
}

impl F16Weight {
    pub fn new(key: impl Into<String>, w: &[f32], n: usize, k: usize) -> Self {
        debug_assert_eq!(w.len(), n * k);
        let mut bytes = Vec::with_capacity(w.len() * 2);
        for &v in w {
            bytes.extend_from_slice(&makepad_ggml::quant::f32_to_f16(v).to_le_bytes());
        }
        Self {
            n,
            key: key.into(),
            bytes,
        }
    }

    pub fn part(&self) -> makepad_ggml::backend::cuda::GpuLinearPart<'_> {
        makepad_ggml::backend::cuda::GpuLinearPart {
            bt_ggml_type: makepad_ggml::quant::GGML_TYPE_F16,
            n: self.n,
            cache_key: &self.key,
            bytes: &self.bytes,
        }
    }
}

/// True when the CUDA device path should be used (device present and not
/// disabled with SA3_DEVICE=0).
pub fn sa3_device_enabled() -> bool {
    if std::env::var("SA3_DEVICE").map(|v| v == "0").unwrap_or(false) {
        return false;
    }
    makepad_ggml::backend::cuda::gpu_device_available()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sigma_schedule_matches_reference() {
        // Values from local/sa3_ref/dumps/sword_clash/sigmas.npy.
        let expected = [
            1.0, 0.9943756, 0.9844802, 0.95791227, 0.8909032, 0.7455466, 0.51249737,
            0.273885, 0.0,
        ];
        let got = sa3_sigmas(8);
        assert_eq!(got.len(), expected.len());
        for (g, e) in got.iter().zip(expected) {
            assert!((g - e).abs() < 3e-6, "sigma {g} vs {e}");
        }
    }

    #[test]
    fn size_math_matches_reference() {
        // sword_clash: 4s -> 442368 samples, 108 latents, valid 108 (all true).
        assert_eq!(sa3_audio_sample_size(4.0), 442_368);
        assert_eq!(sa3_latent_len(4.0), 108);
        assert_eq!(sa3_effective_seq_len(4.0), 44);
        assert_eq!(sa3_valid_len(4.0), 108);
        // coin_pickup: 2s -> 360448 samples, 88 latents, valid 86.
        assert_eq!(sa3_audio_sample_size(2.0), 360_448);
        assert_eq!(sa3_latent_len(2.0), 88);
        assert_eq!(sa3_valid_len(2.0), 86);
    }
}

/// Formats a device-path error with its stage tag.
pub fn dev_err(what: &str, err: String) -> DiffusionError {
    DiffusionError::model(format!("sa3 device {what}: {err}"))
}
