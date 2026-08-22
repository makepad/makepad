//! IndexTTS-2.5 GPT stage: text + conditioning -> semantic mel codes.
//!
//! Ports `UnifiedVoice` (spk_cond_mode="campplus") from the reference
//! `indextts/gpt/model_v2.py`:
//!
//! - a vanilla HF GPT-2 backbone (24 layers, d=1280, 20 heads, pre-norm,
//!   gelu_new MLP) whose built-in wte/wpe are unused; custom text/mel
//!   embeddings + learned absolute positions are added outside,
//! - the emotion conditioning path: wenet-style ConformerEncoder
//!   (1024 -> 512, 4 blocks, 4 heads, conv2d2 subsampling, rel-pos
//!   attention, conv module k15) followed by a 1-latent PerceiverResampler
//!   (dim 1024, GEGLU ff), then `emovec_layer` (1024->1280) and
//!   `emo_layer` (1280->1280),
//! - the 8-dim emotion-vector mixing over the feat1/feat2 matrices
//!   (`infer_v2_5.py` + `merge_emovec`),
//! - prefill assembly (`prepare_gpt_inputs`) and the KV-cached greedy /
//!   sampled decode loop of `GPT2InferenceModel`.
//!
//! Everything is CPU f32, deterministic. Weights load from `gpt.pth`
//! (flat dotted keys; HF Conv1D weights are stored `[in, out]` and are
//! transposed to `[out, in]` nn.Linear layout at load). Validation against
//! the oracle dumps lives in `src/bin/indextts_gpt_validate.rs`.

use crate::error::{DiffusionError, Result};
use crate::{emit_progress, ProgressHook};
use crate::indextts::{
    GPT_DIM, GPT_HEADS, GPT_LAYERS, GPT_MAX_MEL_TOKENS, GPT_MEL_VOCAB, GPT_START_MEL,
    GPT_START_TEXT, GPT_STOP_MEL, GPT_STOP_TEXT, GPT_TEXT_VOCAB,
};
use crate::indextts::EMOTION_CATEGORY_ROWS;
use makepad_ai_sfx::sa3::{gelu_tanh, linear, par_rows, sigmoid, silu};
use crate::torch_pth::PthStateDict;
use std::path::Path;

/// Conformer / perceiver dimensions (config.yaml `gpt.emo_condition_module`).
pub const EMO_CONFORMER_DIM: usize = 512;
pub const EMO_CONFORMER_BLOCKS: usize = 4;
pub const EMO_CONFORMER_HEADS: usize = 4;
pub const EMO_CONFORMER_FF: usize = 1024;
pub const EMO_CONFORMER_CONV_KERNEL: usize = 15;
pub const EMO_INPUT_DIM: usize = 1024;
pub const EMO_PERCEIVER_DIM: usize = 1024;
pub const EMO_PERCEIVER_HEADS: usize = 4;
pub const EMO_PERCEIVER_HEAD_DIM: usize = 64;
/// GEGLU inner dim: int(1024 * 2 (mult) * 2 / 3).
pub const EMO_PERCEIVER_FF_INNER: usize = 1365;

const LN_EPS: f32 = 1e-5;
const HEAD_DIM: usize = GPT_DIM / GPT_HEADS; // 64
const LANG_VOCAB: usize = 107;
const TEXT_POS_ROWS: usize = 602;
const MEL_POS_ROWS: usize = 1818;

// ---------------------------------------------------------------------------
// Small math helpers
// ---------------------------------------------------------------------------

/// erf via the all-positive-terms series erf(x) = (2x/sqrt(pi)) e^(-x^2)
/// sum_n (2x^2)^n / (1*3*...*(2n+1)); exact to f64 rounding, no cancellation.
fn erf64(x: f64) -> f64 {
    let ax = x.abs();
    if ax > 5.9 {
        // |erf| - 1 < 1e-16 out here.
        return 1.0f64.copysign(x);
    }
    let x2 = x * x;
    let mut term = 1.0f64;
    let mut sum = 1.0f64;
    let mut n = 1u32;
    while n < 400 {
        term *= 2.0 * x2 / (2 * n + 1) as f64;
        sum += term;
        if term < sum * 1e-18 {
            break;
        }
        n += 1;
    }
    (2.0 / std::f64::consts::PI.sqrt()) * x * (-x2).exp() * sum
}

/// Exact (erf-form) gelu, torch `F.gelu` default — used by the perceiver
/// GEGLU. (The GPT-2 MLP uses `gelu_new`, i.e. `sa3::gelu_tanh`.)
fn gelu_erf(x: f32) -> f32 {
    let x = x as f64;
    (0.5 * x * (1.0 + erf64(x * std::f64::consts::FRAC_1_SQRT_2))) as f32
}

fn softmax_row(row: &mut [f32]) {
    let mut max = f32::NEG_INFINITY;
    for v in row.iter() {
        if *v > max {
            max = *v;
        }
    }
    let mut sum = 0f32;
    for v in row.iter_mut() {
        *v = (*v - max).exp();
        sum += *v;
    }
    let inv = 1.0 / sum;
    for v in row.iter_mut() {
        *v *= inv;
    }
}

/// torch.argmax tie-break: first occurrence of the maximum.
pub fn argmax(logits: &[f32]) -> usize {
    let mut best = 0usize;
    let mut best_v = f32::NEG_INFINITY;
    for (i, &v) in logits.iter().enumerate() {
        if v > best_v {
            best_v = v;
            best = i;
        }
    }
    best
}

/// HF repetition penalty over the SET of previously generated tokens
/// (`seen[token]`): logits > 0 divided by `penalty`, logits < 0 multiplied.
pub fn apply_repetition_penalty(logits: &mut [f32], seen: &[bool], penalty: f32) {
    if penalty == 1.0 {
        return;
    }
    for (v, &s) in logits.iter_mut().zip(seen) {
        if s {
            if *v > 0.0 {
                *v /= penalty;
            } else {
                *v *= penalty;
            }
        }
    }
}

/// Single-row matmul with column-parallel threading (decode-step fast path).
fn linear_row1(a: &[f32], w: &[f32], bias: Option<&[f32]>, k: usize, n: usize) -> Vec<f32> {
    debug_assert_eq!(a.len(), k);
    debug_assert_eq!(w.len(), n * k);
    let mut out = vec![0f32; n];
    // Cap threads by useful work so tiny ops stay single-threaded.
    let threads = makepad_ai_sfx::sa3::sa3_threads().clamp(1, (n * k / 400_000).max(1)).min(n);
    let chunk = n.div_ceil(threads);
    std::thread::scope(|scope| {
        for (ci, out_chunk) in out.chunks_mut(chunk).enumerate() {
            let col0 = ci * chunk;
            scope.spawn(move || {
                for (j, o) in out_chunk.iter_mut().enumerate() {
                    let col = col0 + j;
                    let w_row = &w[col * k..(col + 1) * k];
                    let mut acc = 0f32;
                    for i in 0..k {
                        acc += a[i] * w_row[i];
                    }
                    *o = acc + bias.map_or(0.0, |b| b[col]);
                }
            });
        }
    });
    out
}

fn linear_any(a: &[f32], w: &[f32], bias: Option<&[f32]>, m: usize, k: usize, n: usize) -> Vec<f32> {
    if m == 1 {
        linear_row1(a, w, bias, k, n)
    } else {
        linear(a, w, bias, m, k, n)
    }
}

// ---------------------------------------------------------------------------
// Weight containers
// ---------------------------------------------------------------------------

struct Lin {
    w: Vec<f32>, // [out, in]
    b: Option<Vec<f32>>,
    i: usize,
    o: usize,
}

impl Lin {
    fn fwd(&self, x: &[f32], rows: usize) -> Vec<f32> {
        debug_assert_eq!(x.len(), rows * self.i);
        linear_any(x, &self.w, self.b.as_deref(), rows, self.i, self.o)
    }
}

struct Norm {
    w: Vec<f32>,
    b: Vec<f32>,
}

impl Norm {
    fn apply(&self, x: &mut [f32]) {
        let dim = self.w.len();
        for row in x.chunks_mut(dim) {
            let mut mean = 0f32;
            for v in row.iter() {
                mean += *v;
            }
            mean /= dim as f32;
            let mut var = 0f32;
            for v in row.iter() {
                let d = *v - mean;
                var += d * d;
            }
            var /= dim as f32;
            let inv = 1.0 / (var + LN_EPS).sqrt();
            for (v, (w, b)) in row.iter_mut().zip(self.w.iter().zip(&self.b)) {
                *v = (*v - mean) * inv * w + b;
            }
        }
    }

    fn fwd(&self, x: &[f32]) -> Vec<f32> {
        let mut out = x.to_vec();
        self.apply(&mut out);
        out
    }
}

fn load_lin(sd: &mut PthStateDict, prefix: &str, i: usize, o: usize, bias: bool) -> Result<Lin> {
    let w = sd.f32_shaped(&format!("{prefix}.weight"), &[o, i])?;
    let b = if bias {
        Some(sd.f32_shaped(&format!("{prefix}.bias"), &[o])?)
    } else {
        None
    };
    Ok(Lin { w, b, i, o })
}

/// HF Conv1D weight (`[in, out]`) -> nn.Linear layout (`[out, in]`).
fn load_lin_conv1d(sd: &mut PthStateDict, prefix: &str, i: usize, o: usize) -> Result<Lin> {
    let w_io = sd.f32_shaped(&format!("{prefix}.weight"), &[i, o])?;
    let mut w = vec![0f32; o * i];
    par_rows(&mut w, i, &|out_row, slice| {
        for (in_row, v) in slice.iter_mut().enumerate() {
            *v = w_io[in_row * o + out_row];
        }
    });
    let b = sd.f32_shaped(&format!("{prefix}.bias"), &[o])?;
    Ok(Lin { w, b: Some(b), i, o })
}

/// Pointwise Conv1d weight `[out, in, 1]` used as a Linear.
fn load_lin_pw(sd: &mut PthStateDict, prefix: &str, i: usize, o: usize) -> Result<Lin> {
    let w = sd.f32_shaped(&format!("{prefix}.weight"), &[o, i, 1])?;
    let b = sd.f32_shaped(&format!("{prefix}.bias"), &[o])?;
    Ok(Lin { w, b: Some(b), i, o })
}

fn load_norm(sd: &mut PthStateDict, prefix: &str, dim: usize) -> Result<Norm> {
    Ok(Norm {
        w: sd.f32_shaped(&format!("{prefix}.weight"), &[dim])?,
        b: sd.f32_shaped(&format!("{prefix}.bias"), &[dim])?,
    })
}

// ---------------------------------------------------------------------------
// Emotion conformer encoder (wenet ConformerEncoder, macaron_style=False)
// ---------------------------------------------------------------------------

struct ConformerBlock {
    norm_mha: Norm,
    norm_conv: Norm,
    norm_ff: Norm,
    norm_final: Norm,
    linear_q: Lin,
    linear_k: Lin,
    linear_v: Lin,
    linear_out: Lin,
    linear_pos: Lin, // no bias
    pos_bias_u: Vec<f32>, // [heads * d_k]
    pos_bias_v: Vec<f32>,
    pw_conv1: Lin, // 512 -> 1024 (GLU halves)
    dw_conv_w: Vec<f32>, // [512, 15]
    dw_conv_b: Vec<f32>,
    conv_norm: Norm,
    pw_conv2: Lin,
    ffn_w1: Lin,
    ffn_w2: Lin,
}

struct EmoConformer {
    conv_w: Vec<f32>, // [512, 3, 3] (single input channel)
    conv_b: Vec<f32>,
    flatten: Lin, // 261632 -> 512
    pe: Vec<f32>, // [5000, 512] sinusoidal buffer from the checkpoint
    blocks: Vec<ConformerBlock>,
    after_norm: Norm,
}

/// Conv2dSubsampling2 output length for a `t`-frame input (k3 s2, no pad).
pub fn conformer_subsampled_len(t: usize) -> usize {
    (t - 3) / 2 + 1
}

impl EmoConformer {
    fn load(sd: &mut PthStateDict) -> Result<Self> {
        let p = "emo_conditioning_encoder";
        let dim = EMO_CONFORMER_DIM;
        let fp = (EMO_INPUT_DIM - 3) / 2 + 1; // 511
        let conv_w = sd.f32_shaped(&format!("{p}.embed.conv.0.weight"), &[dim, 1, 3, 3])?;
        let conv_b = sd.f32_shaped(&format!("{p}.embed.conv.0.bias"), &[dim])?;
        let flatten = load_lin(sd, &format!("{p}.embed.out.0"), dim * fp, dim, true)?;
        let pe = sd.f32_shaped(&format!("{p}.embed.pos_enc.pe"), &[1, 5000, dim])?;
        let after_norm = load_norm(sd, &format!("{p}.after_norm"), dim)?;
        let d_k = dim / EMO_CONFORMER_HEADS;
        let mut blocks = Vec::with_capacity(EMO_CONFORMER_BLOCKS);
        for i in 0..EMO_CONFORMER_BLOCKS {
            let b = format!("{p}.encoders.{i}");
            blocks.push(ConformerBlock {
                norm_mha: load_norm(sd, &format!("{b}.norm_mha"), dim)?,
                norm_conv: load_norm(sd, &format!("{b}.norm_conv"), dim)?,
                norm_ff: load_norm(sd, &format!("{b}.norm_ff"), dim)?,
                norm_final: load_norm(sd, &format!("{b}.norm_final"), dim)?,
                linear_q: load_lin(sd, &format!("{b}.self_attn.linear_q"), dim, dim, true)?,
                linear_k: load_lin(sd, &format!("{b}.self_attn.linear_k"), dim, dim, true)?,
                linear_v: load_lin(sd, &format!("{b}.self_attn.linear_v"), dim, dim, true)?,
                linear_out: load_lin(sd, &format!("{b}.self_attn.linear_out"), dim, dim, true)?,
                linear_pos: load_lin(sd, &format!("{b}.self_attn.linear_pos"), dim, dim, false)?,
                pos_bias_u: sd.f32_shaped(
                    &format!("{b}.self_attn.pos_bias_u"),
                    &[EMO_CONFORMER_HEADS, d_k],
                )?,
                pos_bias_v: sd.f32_shaped(
                    &format!("{b}.self_attn.pos_bias_v"),
                    &[EMO_CONFORMER_HEADS, d_k],
                )?,
                pw_conv1: load_lin_pw(sd, &format!("{b}.conv_module.pointwise_conv1"), dim, dim * 2)?,
                dw_conv_w: sd.f32_shaped(
                    &format!("{b}.conv_module.depthwise_conv.weight"),
                    &[dim, 1, EMO_CONFORMER_CONV_KERNEL],
                )?,
                dw_conv_b: sd.f32_shaped(&format!("{b}.conv_module.depthwise_conv.bias"), &[dim])?,
                conv_norm: load_norm(sd, &format!("{b}.conv_module.norm"), dim)?,
                pw_conv2: load_lin_pw(sd, &format!("{b}.conv_module.pointwise_conv2"), dim, dim)?,
                ffn_w1: load_lin(sd, &format!("{b}.feed_forward.w_1"), dim, EMO_CONFORMER_FF, true)?,
                ffn_w2: load_lin(sd, &format!("{b}.feed_forward.w_2"), EMO_CONFORMER_FF, dim, true)?,
            });
        }
        Ok(Self { conv_w, conv_b, flatten, pe, blocks, after_norm })
    }

    /// x: `[t, 1024]` (batch 1, unpadded — all-valid mask, which is what both
    /// the oracle and the batch-1 production path produce). Returns
    /// (`[t', 512]`, t').
    fn forward(&self, x: &[f32], t: usize) -> Result<(Vec<f32>, usize)> {
        let dim = EMO_CONFORMER_DIM;
        if t < 3 {
            return Err(DiffusionError::model(format!(
                "emo conformer needs >= 3 frames, got {t}"
            )));
        }
        if x.len() != t * EMO_INPUT_DIM {
            return Err(DiffusionError::model(format!(
                "emo conformer input len {} != {t} x {EMO_INPUT_DIM}",
                x.len()
            )));
        }
        let tp = conformer_subsampled_len(t);
        let fp = (EMO_INPUT_DIM - 3) / 2 + 1; // 511
        // Conv2d(1 -> 512, k3, s2) + ReLU, laid out [t'][c][f'] which is
        // exactly torch's (b, t', c*f') flatten order.
        let mut conv = vec![0f32; tp * dim * fp];
        par_rows(&mut conv, dim * fp, &|ti, slice| {
            for c in 0..dim {
                let wr = &self.conv_w[c * 9..c * 9 + 9];
                let bias = self.conv_b[c];
                let dst = &mut slice[c * fp..(c + 1) * fp];
                for (fi, out_v) in dst.iter_mut().enumerate() {
                    let mut acc = bias;
                    for ky in 0..3 {
                        let row = &x[(2 * ti + ky) * EMO_INPUT_DIM + 2 * fi..];
                        acc += wr[ky * 3] * row[0] + wr[ky * 3 + 1] * row[1] + wr[ky * 3 + 2] * row[2];
                    }
                    *out_v = acc.max(0.0);
                }
            }
        });
        let mut h = self.flatten.fwd(&conv, tp);
        drop(conv);
        // RelPositionalEncoding: x * sqrt(dim); pos_emb = pe[0..t'].
        let scale = (dim as f32).sqrt();
        for v in &mut h {
            *v *= scale;
        }
        let pos = &self.pe[..tp * dim];
        for block in &self.blocks {
            block.forward(&mut h, tp, pos);
        }
        self.after_norm.apply(&mut h);
        Ok((h, tp))
    }
}

impl ConformerBlock {
    fn forward(&self, x: &mut Vec<f32>, t: usize, pos: &[f32]) {
        let dim = EMO_CONFORMER_DIM;
        let heads = EMO_CONFORMER_HEADS;
        let d_k = dim / heads;
        // --- rel-pos multi-head self-attention (pre-norm) ---
        let xn = self.norm_mha.fwd(x);
        let q = self.linear_q.fwd(&xn, t);
        let k = self.linear_k.fwd(&xn, t);
        let v = self.linear_v.fwd(&xn, t);
        let p = self.linear_pos.fwd(pos, t);
        let scale = 1.0 / (d_k as f32).sqrt();
        let mut ctx = vec![0f32; t * dim];
        par_rows(&mut ctx, dim, &|qi, out_row| {
            let mut scores = vec![0f32; t];
            for h in 0..heads {
                let q_row = &q[qi * dim + h * d_k..qi * dim + (h + 1) * d_k];
                let u = &self.pos_bias_u[h * d_k..(h + 1) * d_k];
                let vb = &self.pos_bias_v[h * d_k..(h + 1) * d_k];
                for (j, s) in scores.iter_mut().enumerate() {
                    let k_row = &k[j * dim + h * d_k..j * dim + (h + 1) * d_k];
                    let p_row = &p[j * dim + h * d_k..j * dim + (h + 1) * d_k];
                    let mut ac = 0f32;
                    let mut bd = 0f32;
                    for d in 0..d_k {
                        ac += (q_row[d] + u[d]) * k_row[d];
                        bd += (q_row[d] + vb[d]) * p_row[d];
                    }
                    *s = (ac + bd) * scale;
                }
                softmax_row(&mut scores);
                let o = &mut out_row[h * d_k..(h + 1) * d_k];
                for (j, &w) in scores.iter().enumerate() {
                    let v_row = &v[j * dim + h * d_k..j * dim + (h + 1) * d_k];
                    for d in 0..d_k {
                        o[d] += w * v_row[d];
                    }
                }
            }
        });
        let attn = self.linear_out.fwd(&ctx, t);
        for (a, b) in x.iter_mut().zip(&attn) {
            *a += b;
        }
        // --- convolution module (pre-norm) ---
        let xn = self.norm_conv.fwd(x);
        let pw1 = self.pw_conv1.fwd(&xn, t); // [t, 1024]
        // GLU over channel dim: a * sigmoid(b).
        let mut g = vec![0f32; t * dim];
        for ti in 0..t {
            let row = &pw1[ti * dim * 2..];
            let dst = &mut g[ti * dim..(ti + 1) * dim];
            for c in 0..dim {
                dst[c] = row[c] * sigmoid(row[dim + c]);
            }
        }
        // Depthwise conv k15 pad 7 over time (non-causal), zeros outside.
        let kernel = EMO_CONFORMER_CONV_KERNEL;
        let pad = (kernel - 1) / 2;
        let mut dw = vec![0f32; t * dim];
        par_rows(&mut dw, dim, &|ti, out_row| {
            for (c, out_v) in out_row.iter_mut().enumerate() {
                let wr = &self.dw_conv_w[c * kernel..(c + 1) * kernel];
                let mut acc = self.dw_conv_b[c];
                for (ki, &w) in wr.iter().enumerate() {
                    let src = ti as isize + ki as isize - pad as isize;
                    if src >= 0 && (src as usize) < t {
                        acc += w * g[src as usize * dim + c];
                    }
                }
                *out_v = acc;
            }
        });
        // LayerNorm over channels, SiLU, pointwise 2, residual.
        self.conv_norm.apply(&mut dw);
        for v in &mut dw {
            *v = silu(*v);
        }
        let conv_out = self.pw_conv2.fwd(&dw, t);
        for (a, b) in x.iter_mut().zip(&conv_out) {
            *a += b;
        }
        // --- feed forward (pre-norm, SiLU, ff_scale = 1.0: no macaron) ---
        let xn = self.norm_ff.fwd(x);
        let mut f = self.ffn_w1.fwd(&xn, t);
        for v in &mut f {
            *v = silu(*v);
        }
        let f = self.ffn_w2.fwd(&f, t);
        for (a, b) in x.iter_mut().zip(&f) {
            *a += b;
        }
        // --- final norm of the block ---
        self.norm_final.apply(x);
    }
}

// ---------------------------------------------------------------------------
// Emotion perceiver resampler (1 latent, depth 2, GEGLU ff)
// ---------------------------------------------------------------------------

struct PerceiverLayer {
    to_q: Lin,  // 1024 -> 256, no bias
    to_kv: Lin, // 1024 -> 512, no bias
    to_out: Lin, // 256 -> 1024, no bias
    ff0: Lin,   // 1024 -> 2730
    ff2: Lin,   // 1365 -> 1024
}

struct EmoPerceiver {
    latents: Vec<f32>, // [1, 1024]
    proj_context: Lin, // 512 -> 1024
    layers: Vec<PerceiverLayer>,
    norm_gamma: Vec<f32>,
}

impl EmoPerceiver {
    fn load(sd: &mut PthStateDict) -> Result<Self> {
        let p = "emo_perceiver_encoder";
        let dim = EMO_PERCEIVER_DIM;
        let inner = EMO_PERCEIVER_HEADS * EMO_PERCEIVER_HEAD_DIM; // 256
        let latents = sd.f32_shaped(&format!("{p}.latents"), &[1, dim])?;
        let proj_context = load_lin(sd, &format!("{p}.proj_context"), EMO_CONFORMER_DIM, dim, true)?;
        let mut layers = Vec::with_capacity(2);
        for i in 0..2 {
            layers.push(PerceiverLayer {
                to_q: load_lin(sd, &format!("{p}.layers.{i}.0.to_q"), dim, inner, false)?,
                to_kv: load_lin(sd, &format!("{p}.layers.{i}.0.to_kv"), dim, inner * 2, false)?,
                to_out: load_lin(sd, &format!("{p}.layers.{i}.0.to_out"), inner, dim, false)?,
                ff0: load_lin(sd, &format!("{p}.layers.{i}.1.0"), dim, EMO_PERCEIVER_FF_INNER * 2, true)?,
                ff2: load_lin(sd, &format!("{p}.layers.{i}.1.2"), EMO_PERCEIVER_FF_INNER, dim, true)?,
            });
        }
        let norm_gamma = sd.f32_shaped(&format!("{p}.norm.gamma"), &[dim])?;
        Ok(Self { latents, proj_context, layers, norm_gamma })
    }

    /// ctx: conformer output `[t, 512]` (all-valid mask). Returns `[1024]`.
    fn forward(&self, ctx: &[f32], t: usize) -> Vec<f32> {
        let dim = EMO_PERCEIVER_DIM;
        let heads = EMO_PERCEIVER_HEADS;
        let d_h = EMO_PERCEIVER_HEAD_DIM;
        let inner = heads * d_h;
        let ctx_p = self.proj_context.fwd(ctx, t); // [t, 1024]
        let mut lat = self.latents.clone(); // single latent row
        for layer in &self.layers {
            // cross_attn_include_queries: context = [latents; ctx].
            let n_ctx = 1 + t;
            let mut kv_in = Vec::with_capacity(n_ctx * dim);
            kv_in.extend_from_slice(&lat);
            kv_in.extend_from_slice(&ctx_p);
            let q = layer.to_q.fwd(&lat, 1); // [1, 256]
            let kv = layer.to_kv.fwd(&kv_in, n_ctx); // [n_ctx, 512] (k | v)
            let scale = 1.0 / (d_h as f32).sqrt();
            let mut ctx_out = vec![0f32; inner];
            for h in 0..heads {
                let q_row = &q[h * d_h..(h + 1) * d_h];
                let mut scores = vec![0f32; n_ctx];
                for (j, s) in scores.iter_mut().enumerate() {
                    let k_row = &kv[j * inner * 2 + h * d_h..];
                    let mut acc = 0f32;
                    for d in 0..d_h {
                        acc += q_row[d] * k_row[d];
                    }
                    *s = acc * scale;
                }
                softmax_row(&mut scores);
                let o = &mut ctx_out[h * d_h..(h + 1) * d_h];
                for (j, &w) in scores.iter().enumerate() {
                    let v_row = &kv[j * inner * 2 + inner + h * d_h..];
                    for d in 0..d_h {
                        o[d] += w * v_row[d];
                    }
                }
            }
            let attn = layer.to_out.fwd(&ctx_out, 1);
            for (a, b) in lat.iter_mut().zip(&attn) {
                *a += b;
            }
            // GEGLU feed forward: h = W0 x; (x_half, gate) = chunk(h);
            // out = W2 (gelu(gate) * x_half).
            let f = layer.ff0.fwd(&lat, 1); // [2730]
            let mut gated = vec![0f32; EMO_PERCEIVER_FF_INNER];
            for (i, g) in gated.iter_mut().enumerate() {
                *g = gelu_erf(f[EMO_PERCEIVER_FF_INNER + i]) * f[i];
            }
            let f = layer.ff2.fwd(&gated, 1);
            for (a, b) in lat.iter_mut().zip(&f) {
                *a += b;
            }
        }
        // RMSNorm F.normalize style: x / max(||x||, 1e-12) * sqrt(dim) * gamma.
        let mut norm = 0f32;
        for v in &lat {
            norm += v * v;
        }
        let norm = norm.sqrt().max(1e-12);
        let scale = (dim as f32).sqrt() / norm;
        for (v, g) in lat.iter_mut().zip(&self.norm_gamma) {
            *v = *v * scale * g;
        }
        lat
    }
}

// ---------------------------------------------------------------------------
// GPT-2 backbone
// ---------------------------------------------------------------------------

struct GptBlock {
    ln_1: Norm,
    ln_2: Norm,
    c_attn: Lin,    // 1280 -> 3840 (q|k|v)
    attn_proj: Lin, // 1280 -> 1280
    c_fc: Lin,      // 1280 -> 5120
    mlp_proj: Lin,  // 5120 -> 1280
}

pub struct IndexTtsGpt {
    text_embedding: Vec<f32>, // [60510, 1280]
    mel_embedding: Vec<f32>,  // [8194, 1280]
    text_pos: Vec<f32>,       // [602, 1280]
    mel_pos: Vec<f32>,        // [1818, 1280]
    lang_embedding: Vec<f32>, // [107, 1280]
    spk_emb_proj: Lin,        // 192 -> 1280
    emovec_lin: Lin,          // 1024 -> 1280
    emo_lin: Lin,             // 1280 -> 1280
    final_norm: Norm,
    mel_head: Lin, // 1280 -> 8194
    ln_f: Norm,
    blocks: Vec<GptBlock>,
    emo_conformer: EmoConformer,
    emo_perceiver: EmoPerceiver,
}

/// Per-layer KV cache, position-major rows of 1280 (`[pos][head*64]`).
struct KvCache {
    k: Vec<Vec<f32>>,
    v: Vec<Vec<f32>>,
}

impl KvCache {
    fn new(layers: usize) -> Self {
        Self { k: vec![Vec::new(); layers], v: vec![Vec::new(); layers] }
    }
}

/// Output of `prepare_gpt_inputs`: `rows` embedding rows of 1280 with
/// `n_pad` zero rows (attention-masked) at the front. Does NOT include the
/// start-mel token row.
pub struct PrefillEmbeds {
    pub embeds: Vec<f32>,
    pub rows: usize,
    pub n_pad: usize,
}

impl IndexTtsGpt {
    /// Loads `gpt.pth` from the checkpoints dir (~3.2 GB f32; tensors are
    /// read lazily but all needed ones are materialized here; the unused
    /// `text_head` is skipped).
    pub fn load(dir: &Path) -> Result<Self> {
        let mut sd = PthStateDict::load_nested(dir.join("gpt.pth"))?;
        let d = GPT_DIM;
        let mut blocks = Vec::with_capacity(GPT_LAYERS);
        for i in 0..GPT_LAYERS {
            let p = format!("gpt.h.{i}");
            blocks.push(GptBlock {
                ln_1: load_norm(&mut sd, &format!("{p}.ln_1"), d)?,
                ln_2: load_norm(&mut sd, &format!("{p}.ln_2"), d)?,
                c_attn: load_lin_conv1d(&mut sd, &format!("{p}.attn.c_attn"), d, 3 * d)?,
                attn_proj: load_lin_conv1d(&mut sd, &format!("{p}.attn.c_proj"), d, d)?,
                c_fc: load_lin_conv1d(&mut sd, &format!("{p}.mlp.c_fc"), d, 4 * d)?,
                mlp_proj: load_lin_conv1d(&mut sd, &format!("{p}.mlp.c_proj"), 4 * d, d)?,
            });
        }
        Ok(Self {
            text_embedding: sd.f32_shaped("text_embedding.weight", &[GPT_TEXT_VOCAB, d])?,
            mel_embedding: sd.f32_shaped("mel_embedding.weight", &[GPT_MEL_VOCAB, d])?,
            text_pos: sd.f32_shaped("text_pos_embedding.emb.weight", &[TEXT_POS_ROWS, d])?,
            mel_pos: sd.f32_shaped("mel_pos_embedding.emb.weight", &[MEL_POS_ROWS, d])?,
            lang_embedding: sd.f32_shaped("lang_embedding.weight", &[LANG_VOCAB, d])?,
            spk_emb_proj: load_lin(&mut sd, "spk_emb_proj", 192, d, true)?,
            emovec_lin: load_lin(&mut sd, "emovec_layer", EMO_PERCEIVER_DIM, d, true)?,
            emo_lin: load_lin(&mut sd, "emo_layer", d, d, true)?,
            final_norm: load_norm(&mut sd, "final_norm", d)?,
            mel_head: load_lin(&mut sd, "mel_head", d, GPT_MEL_VOCAB, true)?,
            ln_f: load_norm(&mut sd, "gpt.ln_f", d)?,
            blocks,
            emo_conformer: EmoConformer::load(&mut sd)?,
            emo_perceiver: EmoPerceiver::load(&mut sd)?,
        })
    }

    // -- emotion conditioning path ------------------------------------------

    /// Conformer encoder over w2v-bert features `[frames, 1024]` ->
    /// (`[frames', 512]`, frames').
    pub fn emo_conformer(&self, cond: &[f32], frames: usize) -> Result<(Vec<f32>, usize)> {
        self.emo_conformer.forward(cond, frames)
    }

    /// Perceiver resampler over the conformer output -> `[1024]`.
    pub fn emo_perceiver(&self, conformer_out: &[f32], frames: usize) -> Vec<f32> {
        self.emo_perceiver.forward(conformer_out, frames)
    }

    pub fn apply_emovec_layer(&self, x: &[f32]) -> Vec<f32> {
        self.emovec_lin.fwd(x, 1)
    }

    pub fn apply_emo_layer(&self, x: &[f32]) -> Vec<f32> {
        self.emo_lin.fwd(x, 1)
    }

    /// `get_emovec`: full emotion path, `[frames, 1024]` -> `[1280]`.
    pub fn emovec(&self, cond: &[f32], frames: usize) -> Result<Vec<f32>> {
        let (conf, tp) = self.emo_conformer(cond, frames)?;
        let perc = self.emo_perceiver(&conf, tp);
        Ok(self.apply_emo_layer(&self.apply_emovec_layer(&perc)))
    }

    /// `merge_emovec`: base + alpha * (emo - base) over the two paths.
    pub fn merge_emovec(
        &self,
        spk_cond: &[f32],
        spk_frames: usize,
        emo_cond: &[f32],
        emo_frames: usize,
        alpha: f32,
    ) -> Result<Vec<f32>> {
        let emo = self.emovec(emo_cond, emo_frames)?;
        let base = self.emovec(spk_cond, spk_frames)?;
        Ok(base
            .iter()
            .zip(&emo)
            .map(|(b, e)| b + alpha * (e - b))
            .collect())
    }

    // -- prefill assembly -----------------------------------------------------

    /// `spk_emb_proj(campplus_style)` -> `[1280]`.
    pub fn spk_latent(&self, style: &[f32]) -> Vec<f32> {
        debug_assert_eq!(style.len(), 192);
        self.spk_emb_proj.fwd(style, 1)
    }

    /// conds_latent `[3, 1280]` = [spk_latent + emovec, zeros, zeros].
    pub fn conds_latent(&self, style: &[f32], emovec: &[f32]) -> Vec<f32> {
        let mut out = vec![0f32; 3 * GPT_DIM];
        let spk = self.spk_latent(style);
        for i in 0..GPT_DIM {
            out[i] = spk[i] + emovec[i];
        }
        out
    }

    /// `prepare_gpt_inputs` for batch 1: strips any start/stop tokens from
    /// `text_tokens`, re-frames with start=0 / stop=1, adds text positional
    /// rows 0.. and the language embedding to every text row, prepends the
    /// conds rows, and left-pads with `n_pad` zero (attention-masked) rows so
    /// the total is `conds_rows + text_tokens.len() + 2`.
    pub fn prefill_embeds(
        &self,
        conds_latent: &[f32],
        text_tokens: &[u32],
        lang: u32,
    ) -> Result<PrefillEmbeds> {
        let d = GPT_DIM;
        if conds_latent.len() % d != 0 {
            return Err(DiffusionError::model("conds_latent not a multiple of 1280"));
        }
        let conds_rows = conds_latent.len() / d;
        let framed = frame_text_tokens(text_tokens);
        if framed.len() > TEXT_POS_ROWS {
            return Err(DiffusionError::model(format!(
                "text too long: {} framed tokens > {TEXT_POS_ROWS}",
                framed.len()
            )));
        }
        let lang = lang as usize;
        if lang >= LANG_VOCAB {
            return Err(DiffusionError::model(format!("lang token {lang} out of range")));
        }
        let rows = conds_rows + text_tokens.len() + 2;
        let n_pad = rows - conds_rows - framed.len();
        let mut embeds = vec![0f32; rows * d];
        embeds[n_pad * d..(n_pad + conds_rows) * d].copy_from_slice(conds_latent);
        let lang_row = &self.lang_embedding[lang * d..(lang + 1) * d];
        for (pos, &tok) in framed.iter().enumerate() {
            let tok = tok as usize;
            if tok >= GPT_TEXT_VOCAB {
                return Err(DiffusionError::model(format!("text token {tok} out of range")));
            }
            let dst = &mut embeds[(n_pad + conds_rows + pos) * d..(n_pad + conds_rows + pos + 1) * d];
            let emb = &self.text_embedding[tok * d..(tok + 1) * d];
            let pos_row = &self.text_pos[pos * d..(pos + 1) * d];
            for i in 0..d {
                dst[i] = emb[i] + pos_row[i] + lang_row[i];
            }
        }
        Ok(PrefillEmbeds { embeds, rows, n_pad })
    }

    /// mel_embedding[token] + mel_pos_embedding[pos] — the decode-step (and
    /// start-mel) input row.
    pub fn mel_input_row(&self, token: u32, pos: usize) -> Vec<f32> {
        let d = GPT_DIM;
        let tok = token as usize;
        debug_assert!(tok < GPT_MEL_VOCAB && pos < MEL_POS_ROWS);
        let emb = &self.mel_embedding[tok * d..(tok + 1) * d];
        let pos_row = &self.mel_pos[pos * d..(pos + 1) * d];
        emb.iter().zip(pos_row).map(|(a, b)| a + b).collect()
    }

    // -- transformer ----------------------------------------------------------

    /// One-shot transformer forward (no session): `rows` embedding rows in,
    /// hidden states after `gpt.ln_f` out. `masked_prefix` key positions at
    /// the front are attention-masked for all queries at or beyond it
    /// (HF additive-mask semantics); pass 0 for a fully unmasked pass (this
    /// is what the `gpt_prefill_hidden` oracle dump used).
    pub fn forward_hidden(&self, embeds: &[f32], rows: usize, masked_prefix: usize) -> Vec<f32> {
        let mut cache = KvCache::new(GPT_LAYERS);
        self.gpt_forward(embeds, rows, &mut cache, masked_prefix)
    }

    /// final_norm + mel_head over one hidden row -> logits `[8194]`.
    pub fn lm_head(&self, hidden_row: &[f32]) -> Vec<f32> {
        let normed = self.final_norm.fwd(hidden_row);
        self.mel_head.fwd(&normed, 1)
    }

    fn gpt_forward(
        &self,
        embeds: &[f32],
        n: usize,
        cache: &mut KvCache,
        masked_prefix: usize,
    ) -> Vec<f32> {
        let d = GPT_DIM;
        debug_assert_eq!(embeds.len(), n * d);
        let past = cache.k[0].len() / d;
        let mut h = embeds.to_vec();
        let scale = 1.0 / (HEAD_DIM as f32).sqrt();
        for (li, blk) in self.blocks.iter().enumerate() {
            let xn = blk.ln_1.fwd(&h);
            let qkv = blk.c_attn.fwd(&xn, n); // [n, 3*d] = q | k | v
            for r in 0..n {
                cache.k[li].extend_from_slice(&qkv[r * 3 * d + d..r * 3 * d + 2 * d]);
                cache.v[li].extend_from_slice(&qkv[r * 3 * d + 2 * d..r * 3 * d + 3 * d]);
            }
            let k_cache = &cache.k[li];
            let v_cache = &cache.v[li];
            let mut ctx = vec![0f32; n * d];
            par_rows(&mut ctx, d, &|qi, out_row| {
                let abs = past + qi;
                // Queries inside the masked prefix see uniformly-shifted
                // scores (softmax invariant); queries beyond it exclude the
                // masked keys exactly (their HF attention weight underflows
                // to 0.0 in f32).
                let start = if abs >= masked_prefix { masked_prefix } else { 0 };
                let n_keys = abs + 1 - start;
                let mut scores = vec![0f32; n_keys];
                for head in 0..GPT_HEADS {
                    let q_row = &qkv[qi * 3 * d + head * HEAD_DIM..qi * 3 * d + (head + 1) * HEAD_DIM];
                    for (jj, s) in scores.iter_mut().enumerate() {
                        let j = start + jj;
                        let k_row = &k_cache[j * d + head * HEAD_DIM..];
                        let mut acc = 0f32;
                        for dd in 0..HEAD_DIM {
                            acc += q_row[dd] * k_row[dd];
                        }
                        *s = acc * scale;
                    }
                    softmax_row(&mut scores);
                    let o = &mut out_row[head * HEAD_DIM..(head + 1) * HEAD_DIM];
                    for (jj, &w) in scores.iter().enumerate() {
                        let j = start + jj;
                        let v_row = &v_cache[j * d + head * HEAD_DIM..];
                        for dd in 0..HEAD_DIM {
                            o[dd] += w * v_row[dd];
                        }
                    }
                }
            });
            let proj = blk.attn_proj.fwd(&ctx, n);
            for (a, b) in h.iter_mut().zip(&proj) {
                *a += b;
            }
            let x2 = blk.ln_2.fwd(&h);
            let mut fc = blk.c_fc.fwd(&x2, n);
            for v in &mut fc {
                *v = gelu_tanh(*v);
            }
            let mp = blk.mlp_proj.fwd(&fc, n);
            for (a, b) in h.iter_mut().zip(&mp) {
                *a += b;
            }
        }
        self.ln_f.apply(&mut h);
        h
    }

    // -- decode ---------------------------------------------------------------

    /// Runs the prefill (conds + text + start-mel token) through the
    /// transformer with the pad-row attention mask and returns the session
    /// plus the first-step logits (`gpt_logits_step0` in the oracle).
    pub fn prefill(
        &self,
        conds_latent: &[f32],
        text_tokens: &[u32],
        lang: u32,
    ) -> Result<(GptSession<'_>, Vec<f32>)> {
        let prefill = self.prefill_embeds(conds_latent, text_tokens, lang)?;
        let mut embeds = prefill.embeds;
        embeds.extend_from_slice(&self.mel_input_row(GPT_START_MEL, 0));
        let rows = prefill.rows + 1;
        let mut session = GptSession {
            gpt: self,
            cache: KvCache::new(GPT_LAYERS),
            seq_len: rows,
            masked_prefix: prefill.n_pad,
            prefill_embed_len: prefill.rows,
        };
        let hidden = self.gpt_forward(&embeds, rows, &mut session.cache, prefill.n_pad);
        let logits = self.lm_head(&hidden[(rows - 1) * GPT_DIM..]);
        Ok((session, logits))
    }

    /// Full AR generation. Returns the generated mel codes, including the
    /// trailing stop token (8193) when it was emitted before `max_tokens`.
    pub fn generate(
        &self,
        conds_latent: &[f32],
        text_tokens: &[u32],
        lang: u32,
        cfg: &GptSamplingConfig,
    ) -> Result<Vec<u32>> {
        self.generate_observed(conds_latent, text_tokens, lang, cfg, None)
    }

    /// [`Self::generate`] with a progress hook: emits `gpt <n>` every few
    /// tokens (fraction against a text-length-based length estimate — AR
    /// length is open-ended) and aborts the loop when the hook returns Err
    /// (the crate-wide cancel path).
    pub fn generate_observed(
        &self,
        conds_latent: &[f32],
        text_tokens: &[u32],
        lang: u32,
        cfg: &GptSamplingConfig,
        mut progress: Option<ProgressHook>,
    ) -> Result<Vec<u32>> {
        let (mut session, mut logits) = self.prefill(conds_latent, text_tokens, lang)?;
        let mut out = Vec::new();
        let mut seen = vec![false; GPT_MEL_VOCAB];
        let mut rng = SplitMix64::new(cfg.seed);
        let max_tokens = cfg.max_tokens.min(GPT_MAX_MEL_TOKENS);
        // Observed mel/text ratio is ~7-8 codes per BPE token; the estimate
        // only shapes the progress fraction, never the loop bound.
        let estimate = (text_tokens.len() * 10).clamp(50, max_tokens);
        for _ in 0..max_tokens {
            if out.len() % 8 == 0 {
                let fraction = (out.len() as f64 / estimate as f64).min(0.98);
                emit_progress(&mut progress, &format!("gpt {}", out.len()), fraction)?;
            }
            apply_repetition_penalty(&mut logits, &seen, cfg.repetition_penalty);
            let tok = if cfg.greedy {
                argmax(&logits) as u32
            } else {
                sample_logits(&mut logits, cfg, &mut rng) as u32
            };
            out.push(tok);
            seen[tok as usize] = true;
            if tok == GPT_STOP_MEL {
                break;
            }
            logits = session.step(tok);
        }
        emit_progress(&mut progress, &format!("gpt {}", out.len()), 1.0)?;
        Ok(out)
    }
}

/// A KV-cached decode session over one prefill.
pub struct GptSession<'a> {
    gpt: &'a IndexTtsGpt,
    cache: KvCache,
    seq_len: usize,
    masked_prefix: usize,
    prefill_embed_len: usize,
}

impl GptSession<'_> {
    /// Feeds one generated mel token, returns the next-step logits `[8194]`.
    ///
    /// Positional row: the reference computes `attention_mask.len - mel_len`
    /// AFTER the mask has grown, so the k-th generated token (0-based) gets
    /// mel_pos row k+2 (row 0 went to the start-mel token, row 1 is skipped —
    /// faithful to the shipped model).
    pub fn step(&mut self, token: u32) -> Vec<f32> {
        self.seq_len += 1;
        let pos = self.seq_len - self.prefill_embed_len;
        let emb = self.gpt.mel_input_row(token, pos);
        let hidden = self
            .gpt
            .gpt_forward(&emb, 1, &mut self.cache, self.masked_prefix);
        self.gpt.lm_head(&hidden)
    }

    /// Sequence length so far (prefill rows + start-mel + generated tokens).
    pub fn seq_len(&self) -> usize {
        self.seq_len
    }
}

/// Strips any start(0)/stop(1) tokens then re-frames as [start, ..., stop].
fn frame_text_tokens(tokens: &[u32]) -> Vec<u32> {
    let mut framed = Vec::with_capacity(tokens.len() + 2);
    framed.push(GPT_START_TEXT);
    framed.extend(
        tokens
            .iter()
            .copied()
            .filter(|&t| t != GPT_START_TEXT && t != GPT_STOP_TEXT),
    );
    framed.push(GPT_STOP_TEXT);
    framed
}

// ---------------------------------------------------------------------------
// Sampling
// ---------------------------------------------------------------------------

/// Sampling configuration. `Default` mirrors the reference production call
/// (`infer_v2_5.py`: do_sample, temperature 0.8, top_p 0.8, top_k 30,
/// repetition_penalty 10). Repetition penalty follows the ORACLE loop
/// semantics: applied over the set of previously generated mel tokens only.
#[derive(Clone, Debug)]
pub struct GptSamplingConfig {
    /// Argmax decode (validation) instead of sampling.
    pub greedy: bool,
    pub temperature: f32,
    /// 0 disables top-k.
    pub top_k: usize,
    /// >= 1.0 disables top-p.
    pub top_p: f32,
    pub repetition_penalty: f32,
    pub max_tokens: usize,
    pub seed: u64,
}

impl Default for GptSamplingConfig {
    fn default() -> Self {
        Self {
            greedy: false,
            temperature: 0.8,
            top_k: 30,
            top_p: 0.8,
            repetition_penalty: 10.0,
            max_tokens: GPT_MAX_MEL_TOKENS,
            seed: 0,
        }
    }
}

impl GptSamplingConfig {
    /// Greedy decode with the given repetition penalty (the oracle setup).
    pub fn greedy(repetition_penalty: f32) -> Self {
        Self {
            greedy: true,
            temperature: 1.0,
            top_k: 0,
            top_p: 1.0,
            repetition_penalty,
            max_tokens: GPT_MAX_MEL_TOKENS,
            seed: 0,
        }
    }
}

/// Deterministic RNG (splitmix64, same pattern as `Sa3SeededNoise`).
struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    fn new(seed: u64) -> Self {
        Self { state: seed ^ 0x9E37_79B9_7F4A_7C15 }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in [0, 1).
    fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// HF warper order: temperature, then top-k, then top-p (descending
/// cumulative mass, keeping the token that crosses the threshold), then a
/// multinomial draw from the renormalized survivors. Assumes the repetition
/// penalty has already been applied.
fn sample_logits(logits: &mut [f32], cfg: &GptSamplingConfig, rng: &mut SplitMix64) -> usize {
    if cfg.temperature != 1.0 && cfg.temperature > 0.0 {
        for v in logits.iter_mut() {
            *v /= cfg.temperature;
        }
    }
    let mut order: Vec<usize> = (0..logits.len()).collect();
    order.sort_by(|&a, &b| logits[b].partial_cmp(&logits[a]).unwrap_or(std::cmp::Ordering::Equal));
    let mut keep = order.len();
    if cfg.top_k > 0 {
        keep = keep.min(cfg.top_k);
    }
    if cfg.top_p < 1.0 {
        // Softmax over the top-k survivors in descending order.
        let max = logits[order[0]];
        let mut sum = 0f64;
        let probs: Vec<f64> = order[..keep]
            .iter()
            .map(|&i| {
                let p = ((logits[i] - max) as f64).exp();
                sum += p;
                p
            })
            .collect();
        let mut cum = 0f64;
        let mut cut = keep;
        for (rank, p) in probs.iter().enumerate() {
            cum += p / sum;
            if cum >= cfg.top_p as f64 {
                cut = rank + 1;
                break;
            }
        }
        keep = cut.max(1);
    }
    // Final softmax over survivors + CDF walk.
    let max = logits[order[0]];
    let mut sum = 0f64;
    let probs: Vec<f64> = order[..keep]
        .iter()
        .map(|&i| {
            let p = ((logits[i] - max) as f64).exp();
            sum += p;
            p
        })
        .collect();
    let draw = rng.next_f64() * sum;
    let mut cum = 0f64;
    for (rank, p) in probs.iter().enumerate() {
        cum += p;
        if draw < cum {
            return order[rank];
        }
    }
    order[keep - 1]
}

// ---------------------------------------------------------------------------
// 8-dim emotion-vector mixing (feat1/feat2 matrices)
// ---------------------------------------------------------------------------

/// The feat1 (`[73, 192]` speaker) / feat2 (`[73, 1280]` emotion) matrices,
/// split into the 8 emotion categories by `EMOTION_CATEGORY_ROWS`.
pub struct EmotionMatrices {
    spk: Vec<f32>,
    emo: Vec<f32>,
}

/// Result of [`EmotionMatrices::mix`].
pub struct EmotionMix {
    /// Per-category picked row (index WITHIN the category, cosine-most-similar
    /// to the campplus style) — `emo_random_index` in the oracle meta.
    pub picks: [usize; 8],
    /// sum_i weight_i * feat2[pick_i] — `emovec_mat`.
    pub emovec_mat: Vec<f32>,
    /// emovec_mat + (1 - sum(weights)) * emovec_ref — the final emovec.
    pub emovec: Vec<f32>,
}

impl EmotionMatrices {
    pub fn load(dir: &Path) -> Result<Self> {
        let rows: usize = EMOTION_CATEGORY_ROWS.iter().sum();
        let (shape, spk) = PthStateDict::load_single_tensor(dir.join("feat1.pt"))?;
        if shape != [rows, 192] {
            return Err(DiffusionError::model(format!("feat1.pt shape {shape:?}")));
        }
        let (shape, emo) = PthStateDict::load_single_tensor(dir.join("feat2.pt"))?;
        if shape != [rows, GPT_DIM] {
            return Err(DiffusionError::model(format!("feat2.pt shape {shape:?}")));
        }
        Ok(Self { spk, emo })
    }

    /// Cosine-most-similar feat1 row per category vs the campplus style
    /// (`[192]`); ties resolve to the first row like `torch.argmax`.
    pub fn pick_rows(&self, style: &[f32]) -> [usize; 8] {
        debug_assert_eq!(style.len(), 192);
        let mut style_norm = 0f32;
        for v in style {
            style_norm += v * v;
        }
        let style_norm = style_norm.sqrt().max(1e-8);
        let mut picks = [0usize; 8];
        let mut offset = 0usize;
        for (cat, &count) in EMOTION_CATEGORY_ROWS.iter().enumerate() {
            let mut best = 0usize;
            let mut best_v = f32::NEG_INFINITY;
            for row in 0..count {
                let r = &self.spk[(offset + row) * 192..(offset + row + 1) * 192];
                let mut dot = 0f32;
                let mut norm = 0f32;
                for (a, b) in r.iter().zip(style) {
                    dot += a * b;
                    norm += a * a;
                }
                let sim = dot / (norm.sqrt().max(1e-8) * style_norm);
                if sim > best_v {
                    best_v = sim;
                    best = row;
                }
            }
            picks[cat] = best;
            offset += count;
        }
        picks
    }

    /// The `infer_v2_5.py` mixing: per category pick the cosine-most-similar
    /// feat1 row, take the same feat2 row, weight-sum with the (normalized)
    /// 8-dim emotion vector, then blend with the reference emovec.
    pub fn mix(&self, style: &[f32], weights: &[f32; 8], emovec_ref: &[f32]) -> EmotionMix {
        let picks = self.pick_rows(style);
        let mut emovec_mat = vec![0f32; GPT_DIM];
        let mut offset = 0usize;
        for (cat, &count) in EMOTION_CATEGORY_ROWS.iter().enumerate() {
            let row = &self.emo[(offset + picks[cat]) * GPT_DIM..(offset + picks[cat] + 1) * GPT_DIM];
            let w = weights[cat];
            for (o, r) in emovec_mat.iter_mut().zip(row) {
                *o += w * r;
            }
            offset += count;
        }
        let rest = 1.0 - weights.iter().sum::<f32>();
        let emovec = emovec_mat
            .iter()
            .zip(emovec_ref)
            .map(|(m, r)| m + rest * r)
            .collect();
        EmotionMix { picks, emovec_mat, emovec }
    }
}

// ---------------------------------------------------------------------------
// CUDA decode path
// ---------------------------------------------------------------------------
//
// Same computation as `gpt_forward` with torch-autocast-bf16 numerics (the
// precision the official deployment and the frozen 5090 reference bar run
// at): weights/GEMMs bf16 with f32 accumulation, LayerNorm in f32, gelu_new
// elementwise in bf16, attention with bf16 score/probability operands.
//
// The pad rows are OMITTED here. Proof of equivalence with the CPU
// masked-prefix forward: pad rows sit at absolute positions < masked_prefix,
// so (a) every query at abs >= masked_prefix starts its key window at
// masked_prefix — it never reads a pad key/value; (b) queries INSIDE the pad
// prefix only produce hidden states at pad positions, whose k/v are again
// only read by pad queries (their kv never enters any non-pad softmax).
// Dropping the pad rows and running standard causal attention over
// [conds | text | start-mel] therefore yields bit-identical non-pad rows.
// Mel/text position indices are unaffected (they are content-relative, not
// absolute-row-relative).

use crate::backend::{
    gpu_add_bf16, gpu_attention_gqa_decode_bf16, gpu_attention_packed_causal_bf16,
    gpu_beam_cache_reorder_append, gpu_bf16_round, gpu_device_available, gpu_download, gpu_gelu,
    gpu_layer_norm_pytorch, gpu_linear_nt_cached_bf16_bias_epilogue, gpu_slice_cols,
    gpu_slice_rows, gpu_upload, gpu_weight_cache_ensure, GpuLinearPart, GpuTensor,
};
use makepad_ai_common::quant::GGML_TYPE_BF16;

const GPT_GPU_NS: &str = "indextts_gpt";

/// True when a CUDA device is present (Windows/Linux builds with kernels).
pub fn gpt_cuda_available() -> bool {
    gpu_device_available()
}

/// f32 -> bf16 bits, round-to-nearest-even (torch `.to(bfloat16)`); the same
/// conversion the bias-epilogue kernel applies to its cached bias.
fn f32_to_bf16_bytes(values: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(values.len() * 2);
    for &value in values {
        let bits = value.to_bits();
        let rounded = bits.wrapping_add(0x7fff + ((bits >> 16) & 1));
        out.extend_from_slice(&((rounded >> 16) as u16).to_le_bytes());
    }
    out
}

fn gm(error: String) -> DiffusionError {
    DiffusionError::model(format!("gpt cuda: {error}"))
}

/// Weight-cached bf16 linear with the torch addmm bias epilogue. The weight
/// converts f32 -> bf16 once (round-to-nearest-even) into the device cache.
fn gpt_linear_gpu(x: &GpuTensor, key: &str, lin: &Lin) -> Result<GpuTensor> {
    gpu_weight_cache_ensure(GPT_GPU_NS, key, GGML_TYPE_BF16, lin.o, lin.i, false, || {
        Ok(f32_to_bf16_bytes(&lin.w))
    })
    .map_err(gm)?;
    let part = GpuLinearPart {
        bt_ggml_type: GGML_TYPE_BF16,
        n: lin.o,
        cache_key: key,
        bytes: &[],
    };
    let bias = lin
        .b
        .as_deref()
        .ok_or_else(|| DiffusionError::model(format!("gpt cuda: linear {key} has no bias")))?;
    gpu_linear_nt_cached_bf16_bias_epilogue(x, GPT_GPU_NS, &[part], bias).map_err(gm)
}

fn gpt_norm_gpu(x: &GpuTensor, norm: &Norm) -> Result<GpuTensor> {
    gpu_layer_norm_pytorch(x, &norm.w, &norm.b, LN_EPS).map_err(gm)
}

impl IndexTtsGpt {
    /// ln_f -> final_norm -> mel_head over device rows; downloads the logits.
    fn lm_head_gpu(&self, hidden: &GpuTensor) -> Result<Vec<f32>> {
        let normed = gpt_norm_gpu(hidden, &self.ln_f)?;
        let normed = gpt_norm_gpu(&normed, &self.final_norm)?;
        let logits = gpt_linear_gpu(&normed, "mel_head", &self.mel_head)?;
        gpu_download(&logits).map_err(gm)
    }

    /// CUDA prefill over the pad-free rows; returns the session and step-0
    /// logits (the CUDA counterpart of [`Self::prefill`]).
    pub fn prefill_cuda(
        &self,
        conds_latent: &[f32],
        text_tokens: &[u32],
        lang: u32,
    ) -> Result<(GptCudaSession<'_>, Vec<f32>)> {
        let d = GPT_DIM;
        let prefill = self.prefill_embeds(conds_latent, text_tokens, lang)?;
        // Strip the inert pad rows (see module proof) and add the start-mel row.
        let mut embeds = prefill.embeds[prefill.n_pad * d..].to_vec();
        embeds.extend_from_slice(&self.mel_input_row(GPT_START_MEL, 0));
        let rows = prefill.rows - prefill.n_pad + 1;
        let scale = 1.0 / (HEAD_DIM as f32).sqrt();

        let mut h = gpu_upload(&embeds, rows, d).map_err(gm)?;
        let mut k_cache = Vec::with_capacity(GPT_LAYERS);
        let mut v_cache = Vec::with_capacity(GPT_LAYERS);
        for (li, blk) in self.blocks.iter().enumerate() {
            let xn = gpt_norm_gpu(&h, &blk.ln_1)?;
            let qkv = gpt_linear_gpu(&xn, &format!("h{li}.c_attn"), &blk.c_attn)?;
            let q = gpu_slice_cols(&qkv, 0, d).map_err(gm)?;
            let k = gpu_slice_cols(&qkv, d, d).map_err(gm)?;
            let v = gpu_slice_cols(&qkv, 2 * d, d).map_err(gm)?;
            let ctx = gpu_attention_packed_causal_bf16(&q, &k, &v, GPT_HEADS, scale).map_err(gm)?;
            let ctx = gpu_bf16_round(&ctx).map_err(gm)?;
            let proj = gpt_linear_gpu(&ctx, &format!("h{li}.attn_proj"), &blk.attn_proj)?;
            h = gpu_add_bf16(&h, &proj).map_err(gm)?;
            let x2 = gpt_norm_gpu(&h, &blk.ln_2)?;
            let fc = gpt_linear_gpu(&x2, &format!("h{li}.c_fc"), &blk.c_fc)?;
            let fc = gpu_gelu(&fc).map_err(gm)?;
            let mp = gpt_linear_gpu(&fc, &format!("h{li}.mlp_proj"), &blk.mlp_proj)?;
            h = gpu_add_bf16(&h, &mp).map_err(gm)?;
            k_cache.push(k);
            v_cache.push(v);
        }
        let last = gpu_slice_rows(&h, rows - 1, 1).map_err(gm)?;
        let logits = self.lm_head_gpu(&last)?;
        let session = GptCudaSession { gpt: self, k_cache, v_cache, seq: rows, fed_count: 0 };
        Ok((session, logits))
    }

    /// [`Self::generate_observed`] on the CUDA session: same sampling loop,
    /// penalties, progress and cancel semantics; device transformer.
    pub fn generate_cuda_observed(
        &self,
        conds_latent: &[f32],
        text_tokens: &[u32],
        lang: u32,
        cfg: &GptSamplingConfig,
        mut progress: Option<ProgressHook>,
    ) -> Result<Vec<u32>> {
        let (mut session, mut logits) = self.prefill_cuda(conds_latent, text_tokens, lang)?;
        let mut out = Vec::new();
        let mut seen = vec![false; GPT_MEL_VOCAB];
        let mut rng = SplitMix64::new(cfg.seed);
        let max_tokens = cfg.max_tokens.min(GPT_MAX_MEL_TOKENS);
        let estimate = (text_tokens.len() * 10).clamp(50, max_tokens);
        for _ in 0..max_tokens {
            if out.len() % 8 == 0 {
                let fraction = (out.len() as f64 / estimate as f64).min(0.98);
                emit_progress(&mut progress, &format!("gpt {}", out.len()), fraction)?;
            }
            apply_repetition_penalty(&mut logits, &seen, cfg.repetition_penalty);
            let tok = if cfg.greedy {
                argmax(&logits) as u32
            } else {
                sample_logits(&mut logits, cfg, &mut rng) as u32
            };
            out.push(tok);
            seen[tok as usize] = true;
            if tok == GPT_STOP_MEL {
                break;
            }
            logits = session.step(tok)?;
        }
        emit_progress(&mut progress, &format!("gpt {}", out.len()), 1.0)?;
        Ok(out)
    }
}

/// Device-resident KV-cached decode session (beam width 1).
pub struct GptCudaSession<'a> {
    gpt: &'a IndexTtsGpt,
    k_cache: Vec<GpuTensor>, // per layer [seq, 1280]
    v_cache: Vec<GpuTensor>,
    seq: usize,
    fed_count: usize,
}

impl GptCudaSession<'_> {
    /// Feeds one generated mel token, returns next-step logits `[8194]`.
    /// Position math matches [`GptSession::step`]: the k-th fed token
    /// (1-based) gets mel_pos row k+1.
    pub fn step(&mut self, token: u32) -> Result<Vec<f32>> {
        let d = GPT_DIM;
        self.fed_count += 1;
        let pos = self.fed_count + 1;
        let emb = self.gpt.mel_input_row(token, pos);
        let mut h = gpu_upload(&emb, 1, d).map_err(gm)?;
        let scale = 1.0 / (HEAD_DIM as f32).sqrt();
        for (li, blk) in self.gpt.blocks.iter().enumerate() {
            let xn = gpt_norm_gpu(&h, &blk.ln_1)?;
            let qkv = gpt_linear_gpu(&xn, &format!("h{li}.c_attn"), &blk.c_attn)?;
            let q = gpu_slice_cols(&qkv, 0, d).map_err(gm)?;
            let k_step = gpu_slice_cols(&qkv, d, d).map_err(gm)?;
            let v_step = gpu_slice_cols(&qkv, 2 * d, d).map_err(gm)?;
            let k = gpu_beam_cache_reorder_append(&self.k_cache[li], &k_step, &[0], 1, self.seq)
                .map_err(gm)?;
            let v = gpu_beam_cache_reorder_append(&self.v_cache[li], &v_step, &[0], 1, self.seq)
                .map_err(gm)?;
            let ctx =
                gpu_attention_gqa_decode_bf16(&q, &k, &v, GPT_HEADS, GPT_HEADS, scale).map_err(gm)?;
            let ctx = gpu_bf16_round(&ctx).map_err(gm)?;
            let proj = gpt_linear_gpu(&ctx, &format!("h{li}.attn_proj"), &blk.attn_proj)?;
            h = gpu_add_bf16(&h, &proj).map_err(gm)?;
            let x2 = gpt_norm_gpu(&h, &blk.ln_2)?;
            let fc = gpt_linear_gpu(&x2, &format!("h{li}.c_fc"), &blk.c_fc)?;
            let fc = gpu_gelu(&fc).map_err(gm)?;
            let mp = gpt_linear_gpu(&fc, &format!("h{li}.mlp_proj"), &blk.mlp_proj)?;
            h = gpu_add_bf16(&h, &mp).map_err(gm)?;
            self.k_cache[li] = k;
            self.v_cache[li] = v;
        }
        self.seq += 1;
        self.gpt.lm_head_gpu(&h)
    }

    /// Device rows cached so far (pad-free prefill + start-mel + fed tokens).
    pub fn seq_len(&self) -> usize {
        self.seq
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indextts::{normalize_emotion_vector, reference_checkpoints_dir, reference_dumps_dir};

    #[test]
    fn erf_and_gelu_match_torch() {
        // torch reference values.
        assert!((erf64(0.5) - 0.5204998778130465).abs() < 1e-14);
        assert!((erf64(1.5) - 0.9661051464753107).abs() < 1e-14);
        assert!((erf64(-2.2) + 0.9981371537020182).abs() < 1e-14);
        assert!((erf64(6.5) - 1.0).abs() < 1e-15);
        // F.gelu (erf form).
        assert!((gelu_erf(1.0) - 0.841_344_7).abs() < 1e-6);
        assert!((gelu_erf(-0.5) + 0.154_268_77).abs() < 1e-6);
        // gelu_new (tanh form) for the GPT-2 MLP.
        assert!((gelu_tanh(1.0) - 0.841_192).abs() < 1e-5);
    }

    #[test]
    fn text_framing_strips_and_reframes() {
        // The oracle text_tokens end with stop=1; net effect: prepend start.
        assert_eq!(frame_text_tokens(&[5, 6, 1]), vec![0, 5, 6, 1]);
        // Already-framed input is stripped then re-framed (one extra pad row).
        assert_eq!(frame_text_tokens(&[0, 5, 6, 1]), vec![0, 5, 6, 1]);
        // Interior 0/1 are stripped too (reference valid_mask).
        assert_eq!(frame_text_tokens(&[5, 0, 6, 1, 7]), vec![0, 5, 6, 7, 1]);
    }

    #[test]
    fn repetition_penalty_matches_hf_semantics() {
        let mut logits = vec![2.0, -3.0, 1.0];
        let seen = vec![true, true, false];
        apply_repetition_penalty(&mut logits, &seen, 2.0);
        assert_eq!(logits, vec![1.0, -6.0, 1.0]);
        // penalty 1.0 is a no-op.
        let mut logits = vec![2.0, -3.0];
        apply_repetition_penalty(&mut logits, &[true, true], 1.0);
        assert_eq!(logits, vec![2.0, -3.0]);
    }

    #[test]
    fn argmax_first_max_wins() {
        assert_eq!(argmax(&[1.0, 3.0, 3.0, 2.0]), 1);
    }

    #[test]
    fn sampling_is_deterministic_and_respects_topk() {
        let cfg = GptSamplingConfig {
            greedy: false,
            temperature: 0.8,
            top_k: 1,
            top_p: 1.0,
            repetition_penalty: 1.0,
            max_tokens: 10,
            seed: 42,
        };
        let mut rng = SplitMix64::new(cfg.seed);
        let mut logits = vec![0.1, 2.0, 0.3, 1.9];
        assert_eq!(sample_logits(&mut logits, &cfg, &mut rng), 1);
        // Same seed, wider top-k: identical draws across runs.
        let cfg = GptSamplingConfig { top_k: 3, top_p: 0.9, ..cfg };
        let mut a = SplitMix64::new(7);
        let mut b = SplitMix64::new(7);
        for _ in 0..20 {
            let mut la = vec![0.5, 1.5, -0.2, 0.9, 3.0];
            let mut lb = la.clone();
            assert_eq!(
                sample_logits(&mut la, &cfg, &mut a),
                sample_logits(&mut lb, &cfg, &mut b)
            );
        }
    }

    #[test]
    fn conformer_subsampling_length() {
        assert_eq!(conformer_subsampled_len(151), 75);
        assert_eq!(conformer_subsampled_len(3), 1);
        assert_eq!(conformer_subsampled_len(4), 1);
        assert_eq!(conformer_subsampled_len(5), 2);
    }

    /// Minimal f32 npy reader for oracle fixtures (test-only).
    fn load_npy_f32(path: &std::path::Path) -> Option<Vec<f32>> {
        let bytes = std::fs::read(path).ok()?;
        if bytes.len() < 10 || &bytes[..6] != b"\x93NUMPY" {
            return None;
        }
        let (len, start) = if bytes[6] == 1 {
            (u16::from_le_bytes([bytes[8], bytes[9]]) as usize, 10)
        } else {
            (u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize, 12)
        };
        let header = String::from_utf8_lossy(&bytes[start..start + len]).to_string();
        if !header.contains("<f4") {
            return None;
        }
        Some(
            bytes[start + len..]
                .chunks_exact(4)
                .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect(),
        )
    }

    /// Oracle: the 8-dim emotion mixing (feat1/feat2 + campplus style) must
    /// reproduce emovec_mat_sad / emovec_sad. Small files only; skips when
    /// the reference checkout is absent.
    #[test]
    fn emotion_mixing_matches_oracle() {
        let ckpt = reference_checkpoints_dir();
        let dumps = reference_dumps_dir();
        if !ckpt.join("feat1.pt").is_file() || !dumps.join("campplus_style.npy").is_file() {
            eprintln!("skipping emotion_mixing_matches_oracle: reference files missing");
            return;
        }
        let mats = EmotionMatrices::load(&ckpt).unwrap();
        let style = load_npy_f32(&dumps.join("campplus_style.npy")).unwrap();
        let emovec_ref = load_npy_f32(&dumps.join("emovec_ref.npy")).unwrap();
        let want_mat = load_npy_f32(&dumps.join("emovec_mat_sad.npy")).unwrap();
        let want_vec = load_npy_f32(&dumps.join("emovec_sad.npy")).unwrap();
        let weights = normalize_emotion_vector(&[0.0, 0.0, 0.8, 0.0, 0.0, 0.0, 0.0, 0.0]);
        let mix = mats.mix(&style, &weights, &emovec_ref);
        // Oracle meta emo_random_index for this style.
        assert_eq!(mix.picks, [2, 6, 1, 2, 1, 3, 0, 9]);
        for (a, b) in mix.emovec_mat.iter().zip(&want_mat) {
            assert!((a - b).abs() <= 2e-3, "emovec_mat mismatch: {a} vs {b}");
        }
        for (a, b) in mix.emovec.iter().zip(&want_vec) {
            assert!((a - b).abs() <= 2e-3, "emovec mismatch: {a} vs {b}");
        }
    }

    /// Full-model oracle smoke (loads ~3.2 GB): opt-in via
    /// `INDEXTTS_GPT_ORACLE=1` so `cargo test` stays fast; the validate bin
    /// is the real gate.
    #[test]
    fn emo_path_matches_oracle_when_opted_in() {
        if std::env::var("INDEXTTS_GPT_ORACLE").as_deref() != Ok("1") {
            eprintln!("skipping emo_path_matches_oracle_when_opted_in: set INDEXTTS_GPT_ORACLE=1");
            return;
        }
        let ckpt = reference_checkpoints_dir();
        let dumps = reference_dumps_dir();
        if !ckpt.join("gpt.pth").is_file() || !dumps.join("spk_cond_emb.npy").is_file() {
            eprintln!("skipping emo_path_matches_oracle_when_opted_in: reference files missing");
            return;
        }
        let gpt = IndexTtsGpt::load(&ckpt).unwrap();
        let cond = load_npy_f32(&dumps.join("spk_cond_emb.npy")).unwrap();
        let frames = cond.len() / EMO_INPUT_DIM;
        let (conf, tp) = gpt.emo_conformer(&cond, frames).unwrap();
        let want = load_npy_f32(&dumps.join("emo_conformer_out.npy")).unwrap();
        assert_eq!(conf.len(), want.len());
        assert_eq!(tp, conformer_subsampled_len(frames));
        let max = conf
            .iter()
            .zip(&want)
            .map(|(a, b)| (a - b).abs())
            .fold(0f32, f32::max);
        assert!(max <= 2e-3, "emo conformer max abs {max}");
    }
}
