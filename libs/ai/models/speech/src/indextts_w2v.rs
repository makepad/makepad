//! IndexTTS-2.5 w2v-bert-2.0 audio front-end, CPU f32.
//!
//! Two pieces, validated against `local/indextts_ref/dumps/`:
//!
//! 1. [`extract_w2v_features`] — the SeamlessM4T feature extractor
//!    (`feature_extraction_seamless_m4t.py` @ the w2v-bert-2.0
//!    `preprocessor_config.json`): kaldi-style 80-bin log-mel fbank
//!    (25 ms povey window / 10 ms hop, waveform * 2^15, per-frame DC removal,
//!    preemphasis 0.97, kaldi mel scale 20..8000 Hz triangularized in mel
//!    space), per-mel-bin zero-mean/unit-var normalization (ddof=1, +1e-7),
//!    padding to an even frame count with `padding_value` **1.0**, then
//!    stride-2 stacking to 160-dim frames. The HF attention mask keeps the
//!    stacked frames whose *second* raw frame is real (odd raw indices), so
//!    an odd raw frame count yields a trailing masked frame
//!    (`valid_frames = raw_frames / 2`).
//!
//! 2. [`W2vBertEncoder`] — `Wav2Vec2BertModel` feature projection + the first
//!    17 conformer layers (`modeling_wav2vec2_bert.py`; the IndexTTS pipeline
//!    consumes `hidden_states[17]`, so layers 17..24 never run). Config:
//!    hidden 1024, 16 heads, intermediate 4096 swish, depthwise conv kernel
//!    31 (left-padded), `position_embeddings_type = "relative_key"` (per-layer
//!    73 x 64 distance embedding, distances clamped to [-64, +8]), LayerNorm
//!    eps 1e-5, no adapter, no spec-augment. Right-padded positions are
//!    zeroed post-projection, excluded as attention *keys*, and re-zeroed at
//!    each conv-module input — exactly the HF `attention_mask` semantics.
//!    [`W2vBertEncoder::encode`] then applies the pipeline's
//!    `(h - mean) / sqrt(var)` stats normalization (`wav2vec2bert_stats.pt`)
//!    to produce `spk_cond_emb`.
//!
//! The fbank front-end runs in f64 and casts to f32 like the numpy reference;
//! the encoder is plain f32 with `sa3::linear`/`par_rows`.

use crate::error::{DiffusionError, Result};
use crate::indextts::{W2V_DIM, W2V_HEADS, W2V_LAYERS_USED};
use makepad_ai_sfx::sa3::{linear, par_rows, sigmoid, silu, Sa3Tensors};
use crate::torch_pth::PthStateDict;
use std::path::Path;

/// Kaldi-style framing shared with the CAMPPlus fbank (25 ms / 10 ms @ 16 kHz).
pub(crate) const KALDI_FRAME: usize = 400;
pub(crate) const KALDI_HOP: usize = 160;
pub(crate) const KALDI_FFT: usize = 512;
/// One-sided spectrum bins (512/2 + 1). The kaldi mel bank has zero weight at
/// the Nyquist bin, so sharing 257 bins with the HF extractor is exact.
pub(crate) const KALDI_BINS: usize = KALDI_FFT / 2 + 1;

/// Stacked feature width consumed by the encoder (2 x 80 log-mels).
pub const W2V_FEATURE_DIM: usize = 160;
const MELS: usize = 80;
const HEAD_DIM: usize = W2V_DIM / W2V_HEADS; // 64
/// relative_key clamp range (config left/right_max_position_embeddings).
const LEFT_MAX_POSITIONS: i64 = 64;
const RIGHT_MAX_POSITIONS: i64 = 8;
const NUM_POSITIONS: usize = (LEFT_MAX_POSITIONS + RIGHT_MAX_POSITIONS) as usize + 1; // 73
const LN_EPS: f64 = 1e-5;
/// HF `mel_floor` constant (close to, but not exactly, f32 machine eps).
const HF_MEL_FLOOR: f64 = 1.192092955078125e-07;

// ---------------------------------------------------------------------------
// FFT (iterative radix-2 Cooley-Tukey, f64; module-local copy per crate
// convention).
// ---------------------------------------------------------------------------

fn fft_radix2(re: &mut [f64], im: &mut [f64]) {
    let n = re.len();
    debug_assert_eq!(n, im.len());
    debug_assert!(n.is_power_of_two());
    // Bit-reversal permutation.
    let mut j = 0usize;
    for i in 0..n {
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
    }
    // Butterflies.
    let mut len = 2usize;
    while len <= n {
        let ang = -2.0 * std::f64::consts::PI / len as f64;
        let (w_re, w_im) = (ang.cos(), ang.sin());
        let mut start = 0usize;
        while start < n {
            let (mut cur_re, mut cur_im) = (1.0f64, 0.0f64);
            for k in start..start + len / 2 {
                let (ur, ui) = (re[k], im[k]);
                let (vr0, vi0) = (re[k + len / 2], im[k + len / 2]);
                let vr = vr0 * cur_re - vi0 * cur_im;
                let vi = vr0 * cur_im + vi0 * cur_re;
                re[k] = ur + vr;
                im[k] = ui + vi;
                re[k + len / 2] = ur - vr;
                im[k + len / 2] = ui - vi;
                let next_re = cur_re * w_re - cur_im * w_im;
                cur_im = cur_re * w_im + cur_im * w_re;
                cur_re = next_re;
            }
            start += len;
        }
        len <<= 1;
    }
}

// ---------------------------------------------------------------------------
// Shared kaldi fbank front-end (also used by indextts_campplus).
// ---------------------------------------------------------------------------

/// Power spectra of kaldi-framed audio: snip-edges frames of 400 samples every
/// 160, per-frame mean removal, preemphasis 0.97 (edge replicated), povey
/// window (symmetric hann^0.85), zero-pad to 512, |rfft|^2. `scale` is applied
/// in f32 first (the HF extractor multiplies by 2^15 in float32; kaldi uses
/// 1.0). Returns `frames * 257` row-major f64; empty when audio < 400 samples.
pub(crate) fn kaldi_power_spectra(audio: &[f32], scale: f32) -> Vec<f64> {
    if audio.len() < KALDI_FRAME {
        return Vec::new();
    }
    let frames = (audio.len() - KALDI_FRAME) / KALDI_HOP + 1;
    let window: Vec<f64> = (0..KALDI_FRAME)
        .map(|i| {
            let hann = 0.5
                - 0.5
                    * (2.0 * std::f64::consts::PI * i as f64 / (KALDI_FRAME as f64 - 1.0)).cos();
            hann.powf(0.85)
        })
        .collect();
    let mut out = vec![0f64; frames * KALDI_BINS];
    let mut re = vec![0f64; KALDI_FFT];
    let mut im = vec![0f64; KALDI_FFT];
    for frame in 0..frames {
        re.fill(0.0);
        im.fill(0.0);
        let src = &audio[frame * KALDI_HOP..frame * KALDI_HOP + KALDI_FRAME];
        for (dst, &v) in re[..KALDI_FRAME].iter_mut().zip(src) {
            *dst = (v * scale) as f64;
        }
        // Per-frame DC removal.
        let mean: f64 = re[..KALDI_FRAME].iter().sum::<f64>() / KALDI_FRAME as f64;
        for v in &mut re[..KALDI_FRAME] {
            *v -= mean;
        }
        // Preemphasis with the first sample replicated: x[0] *= 1 - p.
        for i in (1..KALDI_FRAME).rev() {
            re[i] -= 0.97 * re[i - 1];
        }
        re[0] *= 1.0 - 0.97;
        for (v, w) in re[..KALDI_FRAME].iter_mut().zip(&window) {
            *v *= w;
        }
        fft_radix2(&mut re, &mut im);
        let row = &mut out[frame * KALDI_BINS..(frame + 1) * KALDI_BINS];
        for (k, dst) in row.iter_mut().enumerate() {
            *dst = re[k] * re[k] + im[k] * im[k];
        }
    }
    out
}

/// 80-filter kaldi-mel triangular bank over the 257 rfft bins, `[mel][bin]`
/// row-major. Filters are triangularized in mel space (kaldi scale
/// `1127 ln(1 + f/700)`) between 20 Hz and 8 kHz; identical (in f64) to both
/// `transformers.audio_utils.mel_filter_bank(..., mel_scale="kaldi",
/// triangularize_in_mel_space=True)` and `torchaudio` kaldi `get_mel_banks`.
pub(crate) fn kaldi_mel_bank_80() -> Vec<f64> {
    let mel = |f: f64| 1127.0 * (1.0 + f / 700.0).ln();
    let mel_lo = mel(20.0);
    let mel_hi = mel(8000.0);
    let delta = (mel_hi - mel_lo) / (MELS as f64 + 1.0);
    let fft_bin_width = 16000.0 / KALDI_FFT as f64; // 31.25 Hz
    let mut bank = vec![0f64; MELS * KALDI_BINS];
    for m in 0..MELS {
        let left = mel_lo + m as f64 * delta;
        let center = left + delta;
        let right = center + delta;
        let row = &mut bank[m * KALDI_BINS..(m + 1) * KALDI_BINS];
        for (k, dst) in row.iter_mut().enumerate() {
            let mel_k = mel(fft_bin_width * k as f64);
            let up = (mel_k - left) / (center - left);
            let down = (right - mel_k) / (right - center);
            *dst = up.min(down).max(0.0);
        }
    }
    bank
}

// ---------------------------------------------------------------------------
// SeamlessM4T feature extractor.
// ---------------------------------------------------------------------------

/// Stacked, normalized input features for the w2v-bert encoder.
pub struct W2vInputFeatures {
    /// `frames * 160` row-major stacked log-mels.
    pub data: Vec<f32>,
    /// Stacked frame count (raw fbank frames padded to even, / 2).
    pub frames: usize,
    /// Ones-count of the HF attention mask (`raw_frames / 2`); when the raw
    /// frame count is odd the last stacked frame is half padding (1.0s) and
    /// masked. `valid_frames` is `frames` or `frames - 1`.
    pub valid_frames: usize,
}

/// `SeamlessM4TFeatureExtractor.__call__` for one mono 16 kHz clip:
/// fbank -> per-mel-bin normalize -> pad to even with 1.0 -> stride-2 stack.
pub fn extract_w2v_features(audio_16k: &[f32]) -> Result<W2vInputFeatures> {
    if audio_16k.len() < KALDI_FRAME {
        return Err(DiffusionError::model(format!(
            "extract_w2v_features: need at least {KALDI_FRAME} samples, got {}",
            audio_16k.len()
        )));
    }
    let spec = kaldi_power_spectra(audio_16k, 32768.0);
    let raw_frames = spec.len() / KALDI_BINS;
    let bank = kaldi_mel_bank_80();
    // Log-mels, f64 pipeline cast to f32 like the numpy reference (`[t][m]`).
    let mut mel = vec![0f32; raw_frames * MELS];
    for t in 0..raw_frames {
        let power = &spec[t * KALDI_BINS..(t + 1) * KALDI_BINS];
        for m in 0..MELS {
            let filt = &bank[m * KALDI_BINS..(m + 1) * KALDI_BINS];
            let mut acc = 0f64;
            for (w, p) in filt.iter().zip(power) {
                acc += w * p;
            }
            mel[t * MELS + m] = acc.max(HF_MEL_FLOOR).ln() as f32;
        }
    }
    // Per-mel-bin zero-mean/unit-var (ddof=1, +1e-7 inside the sqrt).
    for m in 0..MELS {
        let mut sum = 0f64;
        for t in 0..raw_frames {
            sum += mel[t * MELS + m] as f64;
        }
        let mean = sum / raw_frames as f64;
        let mut sq = 0f64;
        for t in 0..raw_frames {
            let d = mel[t * MELS + m] as f64 - mean;
            sq += d * d;
        }
        let var = if raw_frames > 1 {
            sq / (raw_frames as f64 - 1.0)
        } else {
            0.0
        };
        let inv = 1.0 / (var + 1e-7).sqrt();
        for t in 0..raw_frames {
            let v = &mut mel[t * MELS + m];
            *v = ((*v as f64 - mean) * inv) as f32;
        }
    }
    // Pad to an even raw frame count with padding_value 1.0, stack stride 2.
    let padded = raw_frames + (raw_frames & 1);
    let frames = padded / 2;
    let mut data = vec![1.0f32; frames * W2V_FEATURE_DIM];
    data[..raw_frames * MELS].copy_from_slice(&mel);
    Ok(W2vInputFeatures {
        data,
        frames,
        valid_frames: raw_frames / 2,
    })
}

// ---------------------------------------------------------------------------
// Wav2Vec2BertModel encoder (feature projection + 17 conformer layers).
// ---------------------------------------------------------------------------

struct LayerNorm {
    w: Vec<f32>,
    b: Vec<f32>,
}

impl LayerNorm {
    /// torch LayerNorm (biased variance, eps 1e-5), f64 accumulation.
    fn forward(&self, x: &mut [f32]) {
        let width = self.w.len();
        debug_assert_eq!(x.len() % width, 0);
        for row in x.chunks_mut(width) {
            let mut sum = 0f64;
            for &v in row.iter() {
                sum += v as f64;
            }
            let mean = sum / width as f64;
            let mut sq = 0f64;
            for &v in row.iter() {
                let d = v as f64 - mean;
                sq += d * d;
            }
            let inv = 1.0 / (sq / width as f64 + LN_EPS).sqrt();
            for (v, (w, b)) in row.iter_mut().zip(self.w.iter().zip(&self.b)) {
                *v = ((*v as f64 - mean) * inv) as f32 * w + b;
            }
        }
    }
}

struct W2vFeedForward {
    norm: LayerNorm,
    w_in: Vec<f32>,  // 4096 x 1024
    b_in: Vec<f32>,
    w_out: Vec<f32>, // 1024 x 4096
    b_out: Vec<f32>,
}

impl W2vFeedForward {
    /// LN -> Linear -> swish -> Linear (half-residual applied by the caller).
    fn forward(&self, x: &[f32], frames: usize) -> Vec<f32> {
        let mut y = x.to_vec();
        self.norm.forward(&mut y);
        let inter = self.w_in.len() / W2V_DIM;
        let mut h = linear(&y, &self.w_in, Some(&self.b_in), frames, W2V_DIM, inter);
        for v in &mut h {
            *v = silu(*v);
        }
        linear(&h, &self.w_out, Some(&self.b_out), frames, inter, W2V_DIM)
    }
}

struct W2vSelfAttention {
    norm: LayerNorm,
    wq: Vec<f32>,
    bq: Vec<f32>,
    wk: Vec<f32>,
    bk: Vec<f32>,
    wv: Vec<f32>,
    bv: Vec<f32>,
    wo: Vec<f32>,
    bo: Vec<f32>,
    /// relative_key distance embedding, 73 x 64 (index = clamp(r-l,-64,8)+64).
    distance: Vec<f32>,
}

impl W2vSelfAttention {
    /// Softmax attention with the relative_key positional score term:
    /// `score(l,r) = (q_l . k_r + q_l . D[clamp(r-l,-64,8)+64]) / sqrt(64)`.
    /// Keys are limited to `valid` (right-padded positions masked out); all
    /// query rows still produce output like the reference.
    fn forward(&self, x: &[f32], frames: usize, valid: usize) -> Vec<f32> {
        let mut y = x.to_vec();
        self.norm.forward(&mut y);
        let q = linear(&y, &self.wq, Some(&self.bq), frames, W2V_DIM, W2V_DIM);
        let k = linear(&y, &self.wk, Some(&self.bk), frames, W2V_DIM, W2V_DIM);
        let v = linear(&y, &self.wv, Some(&self.bv), frames, W2V_DIM, W2V_DIM);
        // Positional scores: q as (frames*heads, 64) @ D^T -> (frames*heads, 73).
        let pos = linear(
            &q,
            &self.distance,
            None,
            frames * W2V_HEADS,
            HEAD_DIM,
            NUM_POSITIONS,
        );
        let scale = 1.0 / (HEAD_DIM as f32).sqrt();
        let mut ctx = vec![0f32; frames * W2V_DIM];
        par_rows(&mut ctx, W2V_DIM, &|l, out_row| {
            let mut scores = vec![0f32; valid];
            for h in 0..W2V_HEADS {
                let q_vec = &q[(l * W2V_HEADS + h) * HEAD_DIM..(l * W2V_HEADS + h + 1) * HEAD_DIM];
                let pos_row =
                    &pos[(l * W2V_HEADS + h) * NUM_POSITIONS..(l * W2V_HEADS + h + 1) * NUM_POSITIONS];
                let mut max_score = f32::NEG_INFINITY;
                for (r, score) in scores.iter_mut().enumerate() {
                    let k_vec = &k[(r * W2V_HEADS + h) * HEAD_DIM..(r * W2V_HEADS + h + 1) * HEAD_DIM];
                    let mut acc = 0f32;
                    for i in 0..HEAD_DIM {
                        acc += q_vec[i] * k_vec[i];
                    }
                    let dist = (r as i64 - l as i64)
                        .clamp(-LEFT_MAX_POSITIONS, RIGHT_MAX_POSITIONS)
                        + LEFT_MAX_POSITIONS;
                    let s = (acc + pos_row[dist as usize]) * scale;
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
                let out_vec = &mut out_row[h * HEAD_DIM..(h + 1) * HEAD_DIM];
                out_vec.fill(0.0);
                for (r, &score) in scores.iter().enumerate() {
                    let w = score * inv;
                    let v_vec = &v[(r * W2V_HEADS + h) * HEAD_DIM..(r * W2V_HEADS + h + 1) * HEAD_DIM];
                    for i in 0..HEAD_DIM {
                        out_vec[i] += w * v_vec[i];
                    }
                }
            }
        });
        linear(&ctx, &self.wo, Some(&self.bo), frames, W2V_DIM, W2V_DIM)
    }
}

struct W2vConvModule {
    norm: LayerNorm,
    pw1: Vec<f32>, // 2048 x 1024 (no bias)
    dw: Vec<f32>,  // 1024 x 31 depthwise, left-padded by 30
    dw_norm: LayerNorm,
    pw2: Vec<f32>, // 1024 x 1024 (no bias)
}

const DW_KERNEL: usize = 31;

impl W2vConvModule {
    /// LN -> mask fill 0 -> pointwise conv (GLU) -> causal depthwise conv 31
    /// -> LN -> swish -> pointwise conv.
    fn forward(&self, x: &[f32], frames: usize, valid: usize) -> Vec<f32> {
        let mut y = x.to_vec();
        self.norm.forward(&mut y);
        y[valid * W2V_DIM..].fill(0.0);
        let g = linear(&y, &self.pw1, None, frames, W2V_DIM, 2 * W2V_DIM);
        let mut u = vec![0f32; frames * W2V_DIM];
        for t in 0..frames {
            let g_row = &g[t * 2 * W2V_DIM..(t + 1) * 2 * W2V_DIM];
            let u_row = &mut u[t * W2V_DIM..(t + 1) * W2V_DIM];
            for c in 0..W2V_DIM {
                u_row[c] = g_row[c] * sigmoid(g_row[W2V_DIM + c]);
            }
        }
        // Depthwise conv, all padding on the left (kernel-1 zeros).
        let mut d = vec![0f32; frames * W2V_DIM];
        par_rows(&mut d, W2V_DIM, &|t, out_row| {
            for tap in 0..DW_KERNEL {
                let src_t = t as isize + tap as isize - (DW_KERNEL as isize - 1);
                if src_t < 0 {
                    continue;
                }
                let src = &u[src_t as usize * W2V_DIM..(src_t as usize + 1) * W2V_DIM];
                for (c, s) in src.iter().enumerate() {
                    out_row[c] += self.dw[c * DW_KERNEL + tap] * s;
                }
            }
        });
        self.dw_norm.forward(&mut d);
        for v in &mut d {
            *v = silu(*v);
        }
        linear(&d, &self.pw2, None, frames, W2V_DIM, W2V_DIM)
    }
}

struct W2vLayer {
    ffn1: W2vFeedForward,
    attn: W2vSelfAttention,
    conv: W2vConvModule,
    ffn2: W2vFeedForward,
    final_norm: LayerNorm,
}

/// Feature projection + the first [`W2V_LAYERS_USED`] conformer layers, plus
/// the pipeline's hidden-state stats normalization.
pub struct W2vBertEncoder {
    proj_norm: LayerNorm,
    proj_w: Vec<f32>, // 1024 x 160
    proj_b: Vec<f32>,
    layers: Vec<W2vLayer>,
    stat_mean: Vec<f32>,
    stat_inv_std: Vec<f32>,
}

impl W2vBertEncoder {
    /// Loads from the IndexTTS checkpoints dir:
    /// `<dir>/hf_cache/w2v-bert-2.0/model.safetensors` (f32) and
    /// `<dir>/wav2vec2bert_stats.pt` (keys "mean"/"var").
    pub fn load(checkpoints_dir: impl AsRef<Path>) -> Result<Self> {
        let dir = checkpoints_dir.as_ref();
        Self::load_paths(
            dir.join("hf_cache/w2v-bert-2.0/model.safetensors"),
            dir.join("wav2vec2bert_stats.pt"),
        )
    }

    /// Path-explicit load (the service cache lays the files out differently
    /// from the reference checkout; [`Self::load`] is the reference layout).
    pub fn load_paths(
        safetensors: impl AsRef<Path>,
        stats_pt: impl AsRef<Path>,
    ) -> Result<Self> {
        let st = Sa3Tensors::load(safetensors.as_ref())?;
        let ln = |name: &str| -> Result<LayerNorm> {
            Ok(LayerNorm {
                w: st.f32(&format!("{name}.weight"))?,
                b: st.f32(&format!("{name}.bias"))?,
            })
        };
        let ffn = |prefix: &str, norm: &str| -> Result<W2vFeedForward> {
            Ok(W2vFeedForward {
                norm: ln(norm)?,
                w_in: st.f32_shaped(
                    &format!("{prefix}.intermediate_dense.weight"),
                    &[4 * W2V_DIM, W2V_DIM],
                )?,
                b_in: st.f32(&format!("{prefix}.intermediate_dense.bias"))?,
                w_out: st.f32_shaped(
                    &format!("{prefix}.output_dense.weight"),
                    &[W2V_DIM, 4 * W2V_DIM],
                )?,
                b_out: st.f32(&format!("{prefix}.output_dense.bias"))?,
            })
        };
        let mut layers = Vec::with_capacity(W2V_LAYERS_USED);
        for i in 0..W2V_LAYERS_USED {
            let p = format!("encoder.layers.{i}");
            layers.push(W2vLayer {
                ffn1: ffn(&format!("{p}.ffn1"), &format!("{p}.ffn1_layer_norm"))?,
                attn: W2vSelfAttention {
                    norm: ln(&format!("{p}.self_attn_layer_norm"))?,
                    wq: st.f32_shaped(&format!("{p}.self_attn.linear_q.weight"), &[W2V_DIM, W2V_DIM])?,
                    bq: st.f32(&format!("{p}.self_attn.linear_q.bias"))?,
                    wk: st.f32_shaped(&format!("{p}.self_attn.linear_k.weight"), &[W2V_DIM, W2V_DIM])?,
                    bk: st.f32(&format!("{p}.self_attn.linear_k.bias"))?,
                    wv: st.f32_shaped(&format!("{p}.self_attn.linear_v.weight"), &[W2V_DIM, W2V_DIM])?,
                    bv: st.f32(&format!("{p}.self_attn.linear_v.bias"))?,
                    wo: st.f32_shaped(&format!("{p}.self_attn.linear_out.weight"), &[W2V_DIM, W2V_DIM])?,
                    bo: st.f32(&format!("{p}.self_attn.linear_out.bias"))?,
                    distance: st.f32_shaped(
                        &format!("{p}.self_attn.distance_embedding.weight"),
                        &[NUM_POSITIONS, HEAD_DIM],
                    )?,
                },
                conv: W2vConvModule {
                    norm: ln(&format!("{p}.conv_module.layer_norm"))?,
                    pw1: st.f32_shaped(
                        &format!("{p}.conv_module.pointwise_conv1.weight"),
                        &[2 * W2V_DIM, W2V_DIM, 1],
                    )?,
                    dw: st.f32_shaped(
                        &format!("{p}.conv_module.depthwise_conv.weight"),
                        &[W2V_DIM, 1, DW_KERNEL],
                    )?,
                    dw_norm: ln(&format!("{p}.conv_module.depthwise_layer_norm"))?,
                    pw2: st.f32_shaped(
                        &format!("{p}.conv_module.pointwise_conv2.weight"),
                        &[W2V_DIM, W2V_DIM, 1],
                    )?,
                },
                ffn2: ffn(&format!("{p}.ffn2"), &format!("{p}.ffn2_layer_norm"))?,
                final_norm: ln(&format!("{p}.final_layer_norm"))?,
            });
        }
        let proj_norm = ln("feature_projection.layer_norm")?;
        let proj_w = st.f32_shaped("feature_projection.projection.weight", &[W2V_DIM, W2V_FEATURE_DIM])?;
        let proj_b = st.f32("feature_projection.projection.bias")?;

        let mut stats = PthStateDict::load_nested(stats_pt.as_ref())?;
        let stat_mean = stats.f32_shaped("mean", &[W2V_DIM])?;
        let var = stats.f32_shaped("var", &[W2V_DIM])?;
        let stat_inv_std = var.iter().map(|&v| 1.0 / v.sqrt()).collect();
        Ok(Self {
            proj_norm,
            proj_w,
            proj_b,
            layers,
            stat_mean,
            stat_inv_std,
        })
    }

    pub fn num_layers(&self) -> usize {
        self.layers.len()
    }

    /// Feature projection (LN over 160 dims + Linear to 1024), with masked
    /// rows zeroed — this is HF `hidden_states[0]`.
    fn project(&self, feats: &W2vInputFeatures) -> Vec<f32> {
        debug_assert_eq!(feats.data.len(), feats.frames * W2V_FEATURE_DIM);
        let mut y = feats.data.clone();
        self.proj_norm.forward(&mut y);
        let mut h = linear(
            &y,
            &self.proj_w,
            Some(&self.proj_b),
            feats.frames,
            W2V_FEATURE_DIM,
            W2V_DIM,
        );
        h[feats.valid_frames * W2V_DIM..].fill(0.0);
        h
    }

    fn forward_layer(&self, layer: &W2vLayer, x: &mut [f32], frames: usize, valid: usize) {
        // 1. half-residual feed-forward.
        let f1 = layer.ffn1.forward(x, frames);
        for (v, d) in x.iter_mut().zip(&f1) {
            *v += 0.5 * d;
        }
        // 2. self-attention.
        let a = layer.attn.forward(x, frames, valid);
        for (v, d) in x.iter_mut().zip(&a) {
            *v += d;
        }
        // 3. conformer convolution.
        let c = layer.conv.forward(x, frames, valid);
        for (v, d) in x.iter_mut().zip(&c) {
            *v += d;
        }
        // 4. half-residual feed-forward + final LN.
        let f2 = layer.ffn2.forward(x, frames);
        for (v, d) in x.iter_mut().zip(&f2) {
            *v += 0.5 * d;
        }
        layer.final_norm.forward(x);
    }

    /// Runs conformer layers `range` on an existing hidden state in place
    /// (`hidden_states[range.start]` -> `hidden_states[range.end]`). Used by
    /// the validate bin to measure single-layer error from oracle inputs.
    pub fn forward_hidden(
        &self,
        hidden: &mut [f32],
        frames: usize,
        valid_frames: usize,
        range: std::ops::Range<usize>,
    ) {
        assert_eq!(hidden.len(), frames * W2V_DIM);
        assert!(range.end <= self.layers.len());
        for layer in &self.layers[range] {
            self.forward_layer(layer, hidden, frames, valid_frames);
        }
    }

    /// Runs the encoder and returns the requested HF `hidden_states[i]`
    /// snapshots (unnormalized): `hidden_states[0]` is the post-projection
    /// input of layer 0, `hidden_states[i]` the output of layer `i-1`.
    /// `capture` must be ascending, each entry <= [`Self::num_layers`].
    pub fn encode_layers(&self, feats: &W2vInputFeatures, capture: &[usize]) -> Vec<Vec<f32>> {
        assert!(capture.windows(2).all(|w| w[0] < w[1]), "capture must ascend");
        let last = capture.last().copied().unwrap_or(0);
        assert!(last <= self.layers.len(), "capture beyond loaded layers");
        let mut x = self.project(feats);
        let mut out = Vec::with_capacity(capture.len());
        if capture.contains(&0) {
            out.push(x.clone());
        }
        for (i, layer) in self.layers[..last].iter().enumerate() {
            self.forward_layer(layer, &mut x, feats.frames, feats.valid_frames);
            if capture.contains(&(i + 1)) {
                out.push(x.clone());
            }
        }
        out
    }

    /// `(h - mean) / sqrt(var)` with the `wav2vec2bert_stats.pt` statistics.
    pub fn normalize(&self, hidden: &[f32]) -> Vec<f32> {
        debug_assert_eq!(hidden.len() % W2V_DIM, 0);
        let mut out = hidden.to_vec();
        for row in out.chunks_mut(W2V_DIM) {
            for ((v, m), s) in row.iter_mut().zip(&self.stat_mean).zip(&self.stat_inv_std) {
                *v = (*v - m) * s;
            }
        }
        out
    }

    /// `spk_cond_emb`: normalized `hidden_states[17]`, `frames * 1024`.
    pub fn encode(&self, feats: &W2vInputFeatures) -> Vec<f32> {
        let hidden = self
            .encode_layers(feats, &[self.layers.len()])
            .pop()
            .expect("hidden state");
        self.normalize(&hidden)
    }
}

// ---------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fft_matches_naive_dft() {
        let n = 64;
        let signal: Vec<f64> = (0..n)
            .map(|i| (i as f64 * 0.37).sin() + 0.3 * (i as f64 * 1.7).cos())
            .collect();
        let mut re = signal.clone();
        let mut im = vec![0f64; n];
        fft_radix2(&mut re, &mut im);
        for k in 0..n {
            let (mut dr, mut di) = (0f64, 0f64);
            for (i, &x) in signal.iter().enumerate() {
                let ang = -2.0 * std::f64::consts::PI * (k * i) as f64 / n as f64;
                dr += x * ang.cos();
                di += x * ang.sin();
            }
            assert!((re[k] - dr).abs() < 1e-9, "bin {k} re {} vs {dr}", re[k]);
            assert!((im[k] - di).abs() < 1e-9, "bin {k} im {} vs {di}", im[k]);
        }
    }

    #[test]
    fn mel_bank_shape_and_nyquist() {
        let bank = kaldi_mel_bank_80();
        assert_eq!(bank.len(), 80 * KALDI_BINS);
        for m in 0..80 {
            let row = &bank[m * KALDI_BINS..(m + 1) * KALDI_BINS];
            let sum: f64 = row.iter().sum();
            assert!(sum > 0.0, "mel filter {m} all zero");
            assert!(row.iter().all(|&v| (0.0..=1.0).contains(&v)));
            // Nyquist bin (8 kHz) sits exactly on the last filter's right
            // edge -> zero weight everywhere (kaldi zeroes this bin).
            assert_eq!(row[KALDI_BINS - 1], 0.0, "filter {m} nyquist");
        }
    }

    #[test]
    fn extractor_framing_padding_and_mask() {
        // 48401 samples (the oracle clip length): 301 raw frames -> padded to
        // 302 -> 151 stacked frames, 150 valid, padding tail exactly 1.0.
        let audio: Vec<f32> = (0..48401)
            .map(|i| (i as f32 * 0.01).sin() * 0.1)
            .collect();
        let feats = extract_w2v_features(&audio).unwrap();
        assert_eq!(feats.frames, 151);
        assert_eq!(feats.valid_frames, 150);
        assert_eq!(feats.data.len(), 151 * W2V_FEATURE_DIM);
        let last = &feats.data[150 * W2V_FEATURE_DIM..];
        assert!(last[80..].iter().all(|&v| v == 1.0), "padding tail not 1.0");
        assert!(last[..80].iter().any(|&v| v != 1.0), "real half looks padded");
        // Per-bin normalization: each mel bin ~zero mean over the raw frames.
        for m in 0..3 {
            let mut sum = 0f64;
            for t in 0..301 {
                let (row, col) = (t / 2, (t % 2) * 80 + m);
                sum += feats.data[row * W2V_FEATURE_DIM + col] as f64;
            }
            assert!((sum / 301.0).abs() < 1e-4, "bin {m} mean {}", sum / 301.0);
        }
        // Even raw frame count (300) -> no padding, all frames valid.
        let feats_even = extract_w2v_features(&audio[..48401 - 160]).unwrap();
        assert_eq!(feats_even.frames, 150);
        assert_eq!(feats_even.valid_frames, 150);
    }

    #[test]
    fn layer_norm_matches_reference_formula() {
        let ln = LayerNorm {
            w: vec![2.0, 2.0, 2.0, 2.0],
            b: vec![0.5, 0.5, 0.5, 0.5],
        };
        let mut x = vec![1.0f32, 2.0, 3.0, 4.0];
        ln.forward(&mut x);
        // mean 2.5, var 1.25 -> normalized [-1.3416, -0.4472, 0.4472, 1.3416]
        let expect = [-2.1832f32, -0.3944, 1.3944, 3.1832];
        for (a, e) in x.iter().zip(expect) {
            assert!((a - e).abs() < 1e-3, "{a} vs {e}");
        }
    }

    #[test]
    fn distance_index_clamps_like_reference() {
        let idx = |l: i64, r: i64| ((r - l).clamp(-64, 8) + 64) as usize;
        assert_eq!(idx(0, 0), 64);
        assert_eq!(idx(100, 0), 0); // key far left -> clamp at -64
        assert_eq!(idx(0, 100), 72); // key far right -> clamp at +8
        assert_eq!(idx(10, 12), 66);
    }

    /// Oracle-backed: extractor vs `w2v_input_features.npy` (skips when the
    /// reference checkout is absent). The encoder oracle runs in the
    /// `indextts-w2v-validate` bin (1.6 GB weight load; too heavy here).
    #[test]
    fn extractor_matches_oracle_dump() {
        let dir = crate::indextts::reference_dumps_dir();
        let audio_path = dir.join("audio_16k.npy");
        if !audio_path.is_file() {
            eprintln!("skipping extractor_matches_oracle_dump: {audio_path:?} missing");
            return;
        }
        let audio = read_npy_f32(&audio_path);
        let reference = read_npy_f32(&dir.join("w2v_input_features.npy"));
        let feats = extract_w2v_features(&audio).unwrap();
        assert_eq!(feats.data.len(), reference.len());
        let mut max_abs = 0f32;
        for (a, b) in feats.data.iter().zip(&reference) {
            max_abs = max_abs.max((a - b).abs());
        }
        assert!(max_abs < 1e-4, "extractor max abs diff {max_abs}");
    }

    /// Minimal f32 .npy reader for the oracle test (little-endian '<f4').
    fn read_npy_f32(path: &std::path::Path) -> Vec<f32> {
        let bytes = std::fs::read(path).unwrap();
        assert_eq!(&bytes[..6], b"\x93NUMPY");
        let header_len = u16::from_le_bytes([bytes[8], bytes[9]]) as usize;
        let header = String::from_utf8_lossy(&bytes[10..10 + header_len]).to_string();
        assert!(header.contains("<f4"), "expected f32 npy: {header}");
        assert!(!header.contains("'fortran_order': True"));
        bytes[10 + header_len..]
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect()
    }
}
