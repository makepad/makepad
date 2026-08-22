//! IndexTTS-2.5 s2mel stage, CPU f32: the InterpolateRegulator (semantic
//! features -> length-matched 512-d condition), the flow-matching estimator
//! (llama-style DiT with AdaptiveLayerNorm + WaveNet post-net) and the Euler
//! CFG solver that turns noise into an 80-bin mel at 22.05 kHz frame rate.
//!
//! References (ground truth, `local/indextts_ref/index-tts/indextts/s2mel/`):
//! `modules/length_regulator.py`, `modules/flow_matching.py` (BASECFM.
//! solve_euler), `modules/diffusion_transformer.py` (DiT), `modules/gpt_fast/
//! model.py` (Transformer/AdaptiveLayerNorm/rope), `modules/wavenet.py` (WN
//! over encodec `SConv1d` — REFLECT padding!), `modules/commons.py`.
//!
//! Weights: `s2mel.pth` via [`PthStateDict::load_nested`], keys under
//! `net.length_regulator.` / `net.cfm.estimator.`. Weight-norm pairs
//! (`weight_g`/`weight_v`) are folded at load: per OUT row over the remaining
//! dims, for Conv1d (out,in,k) and Linear (out,in) alike.
//!
//! Verified subtleties (all against the python sources):
//! - RoPE is the INTERLEAVED-pair complex form (`apply_rotary_emb` reshapes
//!   (..., -1, 2)), base 10000, head_dim 64 -> exactly
//!   [`moss_rope_tables`] + [`moss_rope_apply_interleaved`].
//! - AdaptiveLayerNorm is `weight * RMSNorm(x) + bias` (NOT 1+scale) with
//!   [weight|bias] = project_layer(t1) — t1 broadcast over time. The DiT
//!   FinalLayer DOES use the `x * (1 + scale) + shift` form, over a
//!   no-affine LayerNorm eps 1e-6, with [shift|scale] = Linear(SiLU(t1)).
//! - uvit skips: layers 0..=5 push their OUTPUT after running; layers 7..=12
//!   pop LIFO and merge through `skip_in_linear(cat[x, skip])` BEFORE the
//!   block body; layer 6 does neither (emit: i < 13//2, receive: i > 13//2).
//! - FFN intermediate dim is 1536 (find_multiple(int(2*2048/3), 256)); the
//!   w1/w3 checkpoint shapes confirm.
//! - The WN in_layers are encodec SConv1d: the `padding=` argument WN passes
//!   is dropped, and forward() reflect-pads (k-1)/2 on each side instead of
//!   zero padding.
//! - `x_embedder` exists in the checkpoint but DiT.forward never uses it
//!   (input goes through `cond_x_merge_linear`); it is not loaded.
//! - The solver zeroes `x[.., :prompt_len]` before the loop and after EVERY
//!   step (including the last), so the returned full mel has a zeroed prompt
//!   region; `vc_target` is the `prompt_len..` tail. `t` accumulates in f32
//!   (`t += dt`) exactly like the reference.

use std::path::Path;

use crate::error::{DiffusionError, Result};
use crate::indextts::{
    S2MEL_CFG_RATE, S2MEL_CFM_STEPS, S2MEL_DIT_DEPTH, S2MEL_DIT_DIM, S2MEL_DIT_HEADS, S2MEL_MELS,
    S2MEL_STYLE_DIM,
};
use makepad_ai_sfx::moss::{moss_rope_apply_interleaved, moss_rope_tables};
use makepad_ai_sfx::sa3::{attention, linear, par_rows, sigmoid, silu};
use crate::torch_pth::PthStateDict;
use makepad_ai_sfx::woosh::softplus;
use crate::{emit_progress, ProgressHook};

const DIM: usize = S2MEL_DIT_DIM; // 512
const HEADS: usize = S2MEL_DIT_HEADS; // 8
const HEAD_DIM: usize = DIM / HEADS; // 64
const DEPTH: usize = S2MEL_DIT_DEPTH; // 13
const MELS: usize = S2MEL_MELS; // 80
const STYLE: usize = S2MEL_STYLE_DIM; // 192
/// find_multiple(int(2 * 4*512 / 3), 256) — checkpoint w1/w3 are [1536, 512].
const FFN: usize = 1536;
const RMS_EPS: f32 = 1e-5;
const TIME_FREQS: usize = 128; // TimestepEmbedder frequency_embedding_size / 2
const REG_IN: usize = 1024; // length regulator in_channels
const REG_STAGES: usize = 4; // sampling_ratios [1,1,1,1]
const WN_LAYERS: usize = 8;
const WN_KERNEL: usize = 5;
const ROPE_BASE: f64 = 10_000.0;

// ---------------------------------------------------------------------------
// Shared helpers (single batch; planes are channel-major [ch, len], row
// matrices are time-major [len, ch]).
// ---------------------------------------------------------------------------

/// [rows, cols] -> [cols, rows].
fn transpose(x: &[f32], rows: usize, cols: usize) -> Vec<f32> {
    debug_assert_eq!(x.len(), rows * cols);
    let mut out = vec![0f32; x.len()];
    for r in 0..rows {
        for c in 0..cols {
            out[c * rows + r] = x[r * cols + c];
        }
    }
    out
}

/// torch weight_norm fold: w = g * v / ||v||, norm per row (dim 0) over `per`
/// trailing elements (Conv1d (out,in,k) and Linear (out,in) alike).
fn fold_weight_norm(g: &[f32], v: &[f32], rows: usize, per: usize) -> Vec<f32> {
    debug_assert_eq!(g.len(), rows);
    debug_assert_eq!(v.len(), rows * per);
    let mut w = vec![0f32; rows * per];
    for r in 0..rows {
        let src = &v[r * per..(r + 1) * per];
        let mut sum = 0f64;
        for &x in src {
            sum += x as f64 * x as f64;
        }
        let scale = g[r] as f64 / sum.sqrt().max(1e-30);
        for (dst, &x) in w[r * per..(r + 1) * per].iter_mut().zip(src) {
            *dst = (x as f64 * scale) as f32;
        }
    }
    w
}

/// torch mish: x * tanh(softplus(x)).
fn mish(x: f32) -> f32 {
    x * softplus(x).tanh()
}

/// Zero-padded Conv1d on a plane, weight (out, in, k) materialized,
/// length-preserving (2*padding == k-1).
struct Conv1d {
    w: Vec<f32>,
    b: Vec<f32>,
    out_ch: usize,
    in_ch: usize,
    k: usize,
    padding: usize,
}

impl Conv1d {
    fn forward(&self, x: &[f32], len: usize) -> Vec<f32> {
        debug_assert_eq!(x.len(), self.in_ch * len);
        debug_assert_eq!(2 * self.padding, self.k - 1);
        let mut out = vec![0f32; self.out_ch * len];
        par_rows(&mut out, len, &|o, row| {
            row.fill(self.b[o]);
            for i in 0..self.in_ch {
                let x_row = &x[i * len..(i + 1) * len];
                let w_row = &self.w[(o * self.in_ch + i) * self.k..][..self.k];
                for (kk, &w) in w_row.iter().enumerate() {
                    let off = kk as isize - self.padding as isize;
                    let t0 = (-off).max(0) as usize;
                    let t1 = ((len as isize - off).min(len as isize)).max(0) as usize;
                    if t1 <= t0 {
                        continue;
                    }
                    let x_slice =
                        &x_row[(t0 as isize + off) as usize..(t1 as isize + off) as usize];
                    for (acc, &xv) in row[t0..t1].iter_mut().zip(x_slice) {
                        *acc += w * xv;
                    }
                }
            }
        });
        out
    }
}

/// REFLECT-padded conv (the encodec SConv1d convention the WN layers use):
/// pads (k-1)/2 on each side by single-bounce reflection, then runs a valid
/// correlation. k == 1 degenerates to a per-frame linear map.
struct ReflectConv1d {
    w: Vec<f32>, // (out, in, k) folded from weight norm
    b: Vec<f32>,
    out_ch: usize,
    in_ch: usize,
    k: usize,
}

impl ReflectConv1d {
    fn forward(&self, x: &[f32], len: usize) -> Vec<f32> {
        debug_assert_eq!(x.len(), self.in_ch * len);
        let pad = (self.k - 1) / 2;
        debug_assert!(pad < len, "reflect pad {pad} needs len > pad (len {len})");
        // Materialize the reflect-padded input once (shared by all out rows).
        let plen = len + 2 * pad;
        let mut padded = vec![0f32; self.in_ch * plen];
        for i in 0..self.in_ch {
            let src = &x[i * len..(i + 1) * len];
            let dst = &mut padded[i * plen..(i + 1) * plen];
            dst[pad..pad + len].copy_from_slice(src);
            for p in 0..pad {
                dst[pad - 1 - p] = src[1 + p]; // x[-1] -> x[1], x[-2] -> x[2]
                dst[pad + len + p] = src[len - 2 - p]; // x[len] -> x[len-2]
            }
        }
        let mut out = vec![0f32; self.out_ch * len];
        par_rows(&mut out, len, &|o, row| {
            row.fill(self.b[o]);
            for i in 0..self.in_ch {
                let x_row = &padded[i * plen..(i + 1) * plen];
                let w_row = &self.w[(o * self.in_ch + i) * self.k..][..self.k];
                for (kk, &w) in w_row.iter().enumerate() {
                    for (acc, &xv) in row.iter_mut().zip(&x_row[kk..kk + len]) {
                        *acc += w * xv;
                    }
                }
            }
        });
        out
    }
}

fn load_linear(
    sd: &mut PthStateDict,
    prefix: &str,
    out_dim: usize,
    in_dim: usize,
) -> Result<(Vec<f32>, Vec<f32>)> {
    Ok((
        sd.f32_shaped(&format!("{prefix}.weight"), &[out_dim, in_dim])?,
        sd.f32_shaped(&format!("{prefix}.bias"), &[out_dim])?,
    ))
}

/// Weight-normed Linear: fold weight_g [out,1] / weight_v [out,in] + bias.
fn load_wn_linear(
    sd: &mut PthStateDict,
    prefix: &str,
    out_dim: usize,
    in_dim: usize,
) -> Result<(Vec<f32>, Vec<f32>)> {
    let g = sd.f32_shaped(&format!("{prefix}.weight_g"), &[out_dim, 1])?;
    let v = sd.f32_shaped(&format!("{prefix}.weight_v"), &[out_dim, in_dim])?;
    Ok((
        fold_weight_norm(&g, &v, out_dim, in_dim),
        sd.f32_shaped(&format!("{prefix}.bias"), &[out_dim])?,
    ))
}

/// Weight-normed SConv1d (`<prefix>.conv.conv.{weight_g,weight_v,bias}`).
fn load_wn_conv(
    sd: &mut PthStateDict,
    prefix: &str,
    out_ch: usize,
    in_ch: usize,
    k: usize,
) -> Result<ReflectConv1d> {
    let g = sd.f32_shaped(&format!("{prefix}.conv.conv.weight_g"), &[out_ch, 1, 1])?;
    let v = sd.f32_shaped(&format!("{prefix}.conv.conv.weight_v"), &[out_ch, in_ch, k])?;
    Ok(ReflectConv1d {
        w: fold_weight_norm(&g, &v, out_ch, in_ch * k),
        b: sd.f32_shaped(&format!("{prefix}.conv.conv.bias"), &[out_ch])?,
        out_ch,
        in_ch,
        k,
    })
}

// ---------------------------------------------------------------------------
// Length regulator (continuous path: in_proj -> nearest interp -> 4x
// [conv k3 p1, GroupNorm(1, 512), Mish] -> conv 1x1).
// ---------------------------------------------------------------------------

struct RegStage {
    conv: Conv1d,
    gn_gamma: Vec<f32>,
    gn_beta: Vec<f32>,
}

pub struct LengthRegulator {
    in_proj_w: Vec<f32>, // (512, 1024)
    in_proj_b: Vec<f32>,
    stages: Vec<RegStage>,
    out_w: Vec<f32>, // (512, 512) from the 1x1 conv
    out_b: Vec<f32>,
}

impl LengthRegulator {
    fn load_sd(sd: &mut PthStateDict, prefix: &str) -> Result<Self> {
        let (in_proj_w, in_proj_b) =
            load_linear(sd, &format!("{prefix}.content_in_proj"), DIM, REG_IN)?;
        let mut stages = Vec::with_capacity(REG_STAGES);
        for i in 0..REG_STAGES {
            let conv_idx = 3 * i;
            stages.push(RegStage {
                conv: Conv1d {
                    w: sd.f32_shaped(&format!("{prefix}.model.{conv_idx}.weight"), &[DIM, DIM, 3])?,
                    b: sd.f32_shaped(&format!("{prefix}.model.{conv_idx}.bias"), &[DIM])?,
                    out_ch: DIM,
                    in_ch: DIM,
                    k: 3,
                    padding: 1,
                },
                gn_gamma: sd.f32_shaped(&format!("{prefix}.model.{}.weight", conv_idx + 1), &[DIM])?,
                gn_beta: sd.f32_shaped(&format!("{prefix}.model.{}.bias", conv_idx + 1), &[DIM])?,
            });
        }
        let out_idx = 3 * REG_STAGES;
        let out_w = sd.f32_shaped(&format!("{prefix}.model.{out_idx}.weight"), &[DIM, DIM, 1])?;
        let out_b = sd.f32_shaped(&format!("{prefix}.model.{out_idx}.bias"), &[DIM])?;
        Ok(Self { in_proj_w, in_proj_b, stages, out_w, out_b })
    }

    /// x: time-major `[in_len, 1024]` -> `[out_len, 512]` (ylens full-length,
    /// mask all ones; discrete/f0/vq paths are not used by IndexTTS-2.5).
    pub fn forward(&self, x: &[f32], in_len: usize, out_len: usize) -> Result<Vec<f32>> {
        if x.len() != in_len * REG_IN || in_len == 0 || out_len == 0 {
            return Err(DiffusionError::model(format!(
                "length regulator: input {} != {in_len}x{REG_IN} (out_len {out_len})",
                x.len()
            )));
        }
        let rows = linear(x, &self.in_proj_w, Some(&self.in_proj_b), in_len, REG_IN, DIM);
        let plane_in = transpose(&rows, in_len, DIM); // (512, in_len)
        // F.interpolate(mode='nearest'): src = floor(dst * in_len / out_len).
        let mut plane = vec![0f32; DIM * out_len];
        for c in 0..DIM {
            let src = &plane_in[c * in_len..(c + 1) * in_len];
            let dst = &mut plane[c * out_len..(c + 1) * out_len];
            for (j, v) in dst.iter_mut().enumerate() {
                *v = src[j * in_len / out_len];
            }
        }
        for stage in &self.stages {
            let mut p = stage.conv.forward(&plane, out_len);
            group_norm_single(&mut p, out_len, &stage.gn_gamma, &stage.gn_beta);
            for v in p.iter_mut() {
                *v = mish(*v);
            }
            plane = p;
        }
        // Final 1x1 conv == per-frame linear on the transposed matrix.
        let rows = transpose(&plane, DIM, out_len); // (out_len, 512)
        Ok(linear(&rows, &self.out_w, Some(&self.out_b), out_len, DIM, DIM))
    }
}

/// GroupNorm(num_groups=1, C) on a plane [C, len]: one mean/var over the whole
/// (C, len) block (biased variance, eps 1e-5), per-channel affine.
fn group_norm_single(x: &mut [f32], len: usize, gamma: &[f32], beta: &[f32]) {
    let n = x.len() as f64;
    let mut mean = 0f64;
    for &v in x.iter() {
        mean += v as f64;
    }
    mean /= n;
    let mut var = 0f64;
    for &v in x.iter() {
        let d = v as f64 - mean;
        var += d * d;
    }
    var /= n;
    let inv = (1.0 / (var + 1e-5).sqrt()) as f32;
    let mean = mean as f32;
    for (c, row) in x.chunks_mut(len).enumerate() {
        let g = gamma[c] * inv;
        let b = beta[c];
        for v in row.iter_mut() {
            *v = (*v - mean) * g + b;
        }
    }
}

// ---------------------------------------------------------------------------
// DiT estimator.
// ---------------------------------------------------------------------------

/// glide-style TimestepEmbedder: [cos|sin](1000*t*freqs) -> MLP(SiLU).
struct TimestepEmbedder {
    freqs: Vec<f32>, // 128, exp(-ln(10000)*i/128) — loaded buffer
    w0: Vec<f32>,    // (512, 256)
    b0: Vec<f32>,
    w2: Vec<f32>, // (512, 512)
    b2: Vec<f32>,
}

impl TimestepEmbedder {
    fn load_sd(sd: &mut PthStateDict, prefix: &str) -> Result<Self> {
        let (w0, b0) = load_linear(sd, &format!("{prefix}.mlp.0"), DIM, 2 * TIME_FREQS)?;
        let (w2, b2) = load_linear(sd, &format!("{prefix}.mlp.2"), DIM, DIM)?;
        Ok(Self {
            freqs: sd.f32_shaped(&format!("{prefix}.freqs"), &[TIME_FREQS])?,
            w0,
            b0,
            w2,
            b2,
        })
    }

    fn embed(&self, t: f32) -> Vec<f32> {
        let mut emb = vec![0f32; 2 * TIME_FREQS];
        for (i, &f) in self.freqs.iter().enumerate() {
            let arg = 1000.0 * t * f;
            emb[i] = arg.cos();
            emb[TIME_FREQS + i] = arg.sin();
        }
        let mut h = linear(&emb, &self.w0, Some(&self.b0), 1, 2 * TIME_FREQS, DIM);
        for v in h.iter_mut() {
            *v = silu(*v);
        }
        linear(&h, &self.w2, Some(&self.b2), 1, DIM, DIM)
    }
}

/// AdaptiveLayerNorm: out = weight * RMSNorm(x) + bias with
/// [weight|bias] = project_layer(t1) (t1 shared across all rows).
struct AdaLayerNorm {
    proj_w: Vec<f32>, // (1024, 512)
    proj_b: Vec<f32>,
    gamma: Vec<f32>, // RMSNorm weight (512), eps 1e-5
}

impl AdaLayerNorm {
    fn load_sd(sd: &mut PthStateDict, prefix: &str) -> Result<Self> {
        let (proj_w, proj_b) = load_linear(sd, &format!("{prefix}.project_layer"), 2 * DIM, DIM)?;
        Ok(Self {
            proj_w,
            proj_b,
            gamma: sd.f32_shaped(&format!("{prefix}.norm.weight"), &[DIM])?,
        })
    }

    fn apply(&self, x: &[f32], t1: &[f32]) -> Vec<f32> {
        let wb = linear(t1, &self.proj_w, Some(&self.proj_b), 1, DIM, 2 * DIM);
        let (w, b) = wb.split_at(DIM);
        let mut out = x.to_vec();
        for row in out.chunks_mut(DIM) {
            let mut sum = 0f32;
            for v in row.iter() {
                sum += v * v;
            }
            let inv = 1.0 / (sum / DIM as f32 + RMS_EPS).sqrt();
            for i in 0..DIM {
                row[i] = row[i] * inv * self.gamma[i] * w[i] + b[i];
            }
        }
        out
    }
}

struct DitBlock {
    attn_norm: AdaLayerNorm,
    wqkv: Vec<f32>, // (1536, 512), no bias
    wo: Vec<f32>,   // (512, 512), no bias
    ffn_norm: AdaLayerNorm,
    w1: Vec<f32>, // (1536, 512)
    w3: Vec<f32>, // (1536, 512)
    w2: Vec<f32>, // (512, 1536)
    skip_w: Vec<f32>, // (512, 1024) — used only by uvit receive layers
    skip_b: Vec<f32>,
}

impl DitBlock {
    fn load_sd(sd: &mut PthStateDict, prefix: &str) -> Result<Self> {
        Ok(Self {
            attn_norm: AdaLayerNorm::load_sd(sd, &format!("{prefix}.attention_norm"))?,
            wqkv: sd.f32_shaped(&format!("{prefix}.attention.wqkv.weight"), &[3 * DIM, DIM])?,
            wo: sd.f32_shaped(&format!("{prefix}.attention.wo.weight"), &[DIM, DIM])?,
            ffn_norm: AdaLayerNorm::load_sd(sd, &format!("{prefix}.ffn_norm"))?,
            w1: sd.f32_shaped(&format!("{prefix}.feed_forward.w1.weight"), &[FFN, DIM])?,
            w3: sd.f32_shaped(&format!("{prefix}.feed_forward.w3.weight"), &[FFN, DIM])?,
            w2: sd.f32_shaped(&format!("{prefix}.feed_forward.w2.weight"), &[DIM, FFN])?,
            skip_w: sd.f32_shaped(&format!("{prefix}.skip_in_linear.weight"), &[DIM, 2 * DIM])?,
            skip_b: sd.f32_shaped(&format!("{prefix}.skip_in_linear.bias"), &[DIM])?,
        })
    }

    fn forward(&self, x: Vec<f32>, t_len: usize, t1: &[f32], cos: &[f32], sin: &[f32]) -> Vec<f32> {
        // Attention with pre-norm AdaLN.
        let normed = self.attn_norm.apply(&x, t1);
        let qkv = linear(&normed, &self.wqkv, None, t_len, DIM, 3 * DIM);
        let mut q = vec![0f32; t_len * DIM];
        let mut k = vec![0f32; t_len * DIM];
        let mut v = vec![0f32; t_len * DIM];
        for tt in 0..t_len {
            let src = &qkv[tt * 3 * DIM..(tt + 1) * 3 * DIM];
            q[tt * DIM..(tt + 1) * DIM].copy_from_slice(&src[..DIM]);
            k[tt * DIM..(tt + 1) * DIM].copy_from_slice(&src[DIM..2 * DIM]);
            v[tt * DIM..(tt + 1) * DIM].copy_from_slice(&src[2 * DIM..]);
            moss_rope_apply_interleaved(&mut q[tt * DIM..(tt + 1) * DIM], tt, HEADS, HEAD_DIM, cos, sin);
            moss_rope_apply_interleaved(&mut k[tt * DIM..(tt + 1) * DIM], tt, HEADS, HEAD_DIM, cos, sin);
        }
        let scale = 1.0 / (HEAD_DIM as f32).sqrt();
        let attn = attention(&q, &k, &v, t_len, t_len, HEADS, None, scale);
        let attn = linear(&attn, &self.wo, None, t_len, DIM, DIM);
        let mut h = x;
        for (a, &b) in h.iter_mut().zip(&attn) {
            *a += b;
        }
        // SwiGLU FFN with pre-norm AdaLN.
        let normed = self.ffn_norm.apply(&h, t1);
        let f1 = linear(&normed, &self.w1, None, t_len, DIM, FFN);
        let mut f3 = linear(&normed, &self.w3, None, t_len, DIM, FFN);
        for (g, &a) in f3.iter_mut().zip(&f1) {
            *g *= silu(a);
        }
        let ff = linear(&f3, &self.w2, None, t_len, FFN, DIM);
        for (a, &b) in h.iter_mut().zip(&ff) {
            *a += b;
        }
        h
    }
}

pub struct CfmEstimator {
    t_embedder: TimestepEmbedder,
    t_embedder2: TimestepEmbedder,
    cond_proj_w: Vec<f32>, // (512, 512)
    cond_proj_b: Vec<f32>,
    merge_w: Vec<f32>, // (512, 864)
    merge_b: Vec<f32>,
    blocks: Vec<DitBlock>,
    final_norm: AdaLayerNorm, // transformer.norm
    skip_linear_w: Vec<f32>,  // (512, 592) long skip
    skip_linear_b: Vec<f32>,
    conv1_w: Vec<f32>, // Linear (512, 512)
    conv1_b: Vec<f32>,
    wn_cond_w: Vec<f32>, // (8192, 512) folded 1x1 conv
    wn_cond_b: Vec<f32>,
    wn_in: Vec<ReflectConv1d>,       // 8x k5 512 -> 1024
    wn_res_skip: Vec<ReflectConv1d>, // 7x 1x1 512 -> 1024, last 512 -> 512
    res_proj_w: Vec<f32>,            // (512, 512)
    res_proj_b: Vec<f32>,
    fl_adaln_w: Vec<f32>, // final_layer.adaLN_modulation.1 (1024, 512)
    fl_adaln_b: Vec<f32>,
    fl_linear_w: Vec<f32>, // final_layer.linear folded (512, 512)
    fl_linear_b: Vec<f32>,
    conv2_w: Vec<f32>, // (80, 512) from (80, 512, 1)
    conv2_b: Vec<f32>,
}

impl CfmEstimator {
    fn load_sd(sd: &mut PthStateDict, prefix: &str) -> Result<Self> {
        let mut blocks = Vec::with_capacity(DEPTH);
        for i in 0..DEPTH {
            blocks.push(DitBlock::load_sd(sd, &format!("{prefix}.transformer.layers.{i}"))?);
        }
        let (cond_proj_w, cond_proj_b) = load_linear(sd, &format!("{prefix}.cond_projection"), DIM, DIM)?;
        let merge_in = 2 * MELS + DIM + STYLE; // 864
        let (merge_w, merge_b) =
            load_linear(sd, &format!("{prefix}.cond_x_merge_linear"), DIM, merge_in)?;
        let (skip_linear_w, skip_linear_b) =
            load_linear(sd, &format!("{prefix}.skip_linear"), DIM, DIM + MELS)?;
        let (conv1_w, conv1_b) = load_linear(sd, &format!("{prefix}.conv1"), DIM, DIM)?;
        let (res_proj_w, res_proj_b) = load_linear(sd, &format!("{prefix}.res_projection"), DIM, DIM)?;
        let (fl_adaln_w, fl_adaln_b) =
            load_linear(sd, &format!("{prefix}.final_layer.adaLN_modulation.1"), 2 * DIM, DIM)?;
        let (fl_linear_w, fl_linear_b) =
            load_wn_linear(sd, &format!("{prefix}.final_layer.linear"), DIM, DIM)?;
        let wn_cond = load_wn_conv(
            sd,
            &format!("{prefix}.wavenet.cond_layer"),
            2 * DIM * WN_LAYERS,
            DIM,
            1,
        )?;
        let mut wn_in = Vec::with_capacity(WN_LAYERS);
        let mut wn_res_skip = Vec::with_capacity(WN_LAYERS);
        for i in 0..WN_LAYERS {
            wn_in.push(load_wn_conv(
                sd,
                &format!("{prefix}.wavenet.in_layers.{i}"),
                2 * DIM,
                DIM,
                WN_KERNEL,
            )?);
            let out_ch = if i < WN_LAYERS - 1 { 2 * DIM } else { DIM };
            wn_res_skip.push(load_wn_conv(
                sd,
                &format!("{prefix}.wavenet.res_skip_layers.{i}"),
                out_ch,
                DIM,
                1,
            )?);
        }
        let conv2_w = sd.f32_shaped(&format!("{prefix}.conv2.weight"), &[MELS, DIM, 1])?;
        let conv2_b = sd.f32_shaped(&format!("{prefix}.conv2.bias"), &[MELS])?;
        Ok(Self {
            t_embedder: TimestepEmbedder::load_sd(sd, &format!("{prefix}.t_embedder"))?,
            t_embedder2: TimestepEmbedder::load_sd(sd, &format!("{prefix}.t_embedder2"))?,
            cond_proj_w,
            cond_proj_b,
            merge_w,
            merge_b,
            blocks,
            final_norm: AdaLayerNorm::load_sd(sd, &format!("{prefix}.transformer.norm"))?,
            skip_linear_w,
            skip_linear_b,
            conv1_w,
            conv1_b,
            wn_cond_w: wn_cond.w,
            wn_cond_b: wn_cond.b,
            wn_in,
            wn_res_skip,
            res_proj_w,
            res_proj_b,
            fl_adaln_w,
            fl_adaln_b,
            fl_linear_w,
            fl_linear_b,
            conv2_w,
            conv2_b,
        })
    }

    /// One estimator pass (reference `DiT.forward`, single batch item).
    ///
    /// - `x`: noisy mel, channel-major `[80, t_len]`
    /// - `prompt_x`: reference mel or zeros, channel-major `[80, t_len]`
    /// - `t`: scalar flow time in [0, 1]
    /// - `style`: `[192]` (zeros for the CFG null pass)
    /// - `mu`: condition, time-major `[t_len, 512]` (zeros for the null pass)
    ///
    /// Returns the velocity, channel-major `[80, t_len]`.
    pub fn forward(
        &self,
        x: &[f32],
        prompt_x: &[f32],
        t: f32,
        style: &[f32],
        mu: &[f32],
        t_len: usize,
    ) -> Result<Vec<f32>> {
        if x.len() != MELS * t_len
            || prompt_x.len() != MELS * t_len
            || style.len() != STYLE
            || mu.len() != t_len * DIM
            || t_len == 0
        {
            return Err(DiffusionError::model(format!(
                "cfm estimator: bad input sizes (t_len {t_len}, x {}, prompt {}, style {}, mu {})",
                x.len(),
                prompt_x.len(),
                style.len(),
                mu.len()
            )));
        }
        let t1 = self.t_embedder.embed(t);
        let cond = linear(mu, &self.cond_proj_w, Some(&self.cond_proj_b), t_len, DIM, DIM);

        // x_in = cat([x^T, prompt_x^T, cond, style]) -> merge linear.
        let merge_in = 2 * MELS + DIM + STYLE;
        let mut x_in = vec![0f32; t_len * merge_in];
        for tt in 0..t_len {
            let row = &mut x_in[tt * merge_in..(tt + 1) * merge_in];
            for c in 0..MELS {
                row[c] = x[c * t_len + tt];
                row[MELS + c] = prompt_x[c * t_len + tt];
            }
            row[2 * MELS..2 * MELS + DIM].copy_from_slice(&cond[tt * DIM..(tt + 1) * DIM]);
            row[2 * MELS + DIM..].copy_from_slice(style);
        }
        let mut h = linear(&x_in, &self.merge_w, Some(&self.merge_b), t_len, merge_in, DIM);

        // Transformer with uvit skips (emit i < 6, receive i > 6, LIFO).
        let (cos, sin) = moss_rope_tables(t_len, HEAD_DIM, ROPE_BASE);
        let mut skips: Vec<Vec<f32>> = Vec::new();
        for (i, block) in self.blocks.iter().enumerate() {
            if i > DEPTH / 2 {
                let skip = skips
                    .pop()
                    .ok_or_else(|| DiffusionError::model("cfm estimator: uvit skip underflow"))?;
                let mut cat = vec![0f32; t_len * 2 * DIM];
                for tt in 0..t_len {
                    cat[tt * 2 * DIM..tt * 2 * DIM + DIM]
                        .copy_from_slice(&h[tt * DIM..(tt + 1) * DIM]);
                    cat[tt * 2 * DIM + DIM..(tt + 1) * 2 * DIM]
                        .copy_from_slice(&skip[tt * DIM..(tt + 1) * DIM]);
                }
                h = linear(&cat, &block.skip_w, Some(&block.skip_b), t_len, 2 * DIM, DIM);
            }
            h = block.forward(h, t_len, &t1, &cos, &sin);
            if i < DEPTH / 2 {
                skips.push(h.clone());
            }
        }
        let x_res = self.final_norm.apply(&h, &t1);

        // Long skip: skip_linear(cat[x_res, x^T]).
        let cat_w = DIM + MELS;
        let mut cat = vec![0f32; t_len * cat_w];
        for tt in 0..t_len {
            cat[tt * cat_w..tt * cat_w + DIM].copy_from_slice(&x_res[tt * DIM..(tt + 1) * DIM]);
            for c in 0..MELS {
                cat[tt * cat_w + DIM + c] = x[c * t_len + tt];
            }
        }
        let x_res = linear(&cat, &self.skip_linear_w, Some(&self.skip_linear_b), t_len, cat_w, DIM);

        // WaveNet head over conv1(x_res), conditioned on t_embedder2(t).
        let rows = linear(&x_res, &self.conv1_w, Some(&self.conv1_b), t_len, DIM, DIM);
        let plane = transpose(&rows, t_len, DIM); // (512, t_len)
        let t2 = self.t_embedder2.embed(t);
        let g = linear(&t2, &self.wn_cond_w, Some(&self.wn_cond_b), 1, DIM, 2 * DIM * WN_LAYERS);
        let wn_out = self.wavenet(plane, t_len, &g);

        // x = wavenet^T + res_projection(x_res); FinalLayer; conv2.
        let mut rows = transpose(&wn_out, DIM, t_len); // (t_len, 512)
        let res = linear(&x_res, &self.res_proj_w, Some(&self.res_proj_b), t_len, DIM, DIM);
        for (a, &b) in rows.iter_mut().zip(&res) {
            *a += b;
        }
        let rows = self.final_layer(&rows, &t1);
        let out_rows = linear(&rows, &self.conv2_w, Some(&self.conv2_b), t_len, DIM, MELS);
        Ok(transpose(&out_rows, t_len, MELS)) // (80, t_len)
    }

    /// WN: 8 gated layers (tanh/sigmoid split), reflect-padded k5 in_layers,
    /// per-layer slice of the (time-constant) cond projection `g`.
    fn wavenet(&self, mut x: Vec<f32>, t_len: usize, g: &[f32]) -> Vec<f32> {
        let mut output = vec![0f32; DIM * t_len];
        for i in 0..WN_LAYERS {
            let x_in = self.wn_in[i].forward(&x, t_len); // (1024, t_len)
            let g_l = &g[i * 2 * DIM..(i + 1) * 2 * DIM];
            let mut acts = vec![0f32; DIM * t_len];
            par_rows(&mut acts, t_len, &|c, row| {
                let ta = &x_in[c * t_len..(c + 1) * t_len];
                let sa = &x_in[(DIM + c) * t_len..(DIM + c + 1) * t_len];
                let ga = g_l[c];
                let gb = g_l[DIM + c];
                for (tt, v) in row.iter_mut().enumerate() {
                    *v = (ta[tt] + ga).tanh() * sigmoid(sa[tt] + gb);
                }
            });
            let rs = self.wn_res_skip[i].forward(&acts, t_len);
            if i < WN_LAYERS - 1 {
                for (xv, &r) in x.iter_mut().zip(&rs[..DIM * t_len]) {
                    *xv += r;
                }
                for (o, &s) in output.iter_mut().zip(&rs[DIM * t_len..]) {
                    *o += s;
                }
            } else {
                for (o, &s) in output.iter_mut().zip(&rs) {
                    *o += s;
                }
            }
        }
        output
    }

    /// DiT FinalLayer: no-affine LayerNorm (eps 1e-6), modulate with
    /// x*(1+scale)+shift from Linear(SiLU(t1)), then the weight-normed linear.
    fn final_layer(&self, x: &[f32], t1: &[f32]) -> Vec<f32> {
        let mut c = t1.to_vec();
        for v in c.iter_mut() {
            *v = silu(*v);
        }
        let sb = linear(&c, &self.fl_adaln_w, Some(&self.fl_adaln_b), 1, DIM, 2 * DIM);
        let (shift, scale) = sb.split_at(DIM);
        let mut out = x.to_vec();
        for row in out.chunks_mut(DIM) {
            let mut mean = 0f32;
            for v in row.iter() {
                mean += v;
            }
            mean /= DIM as f32;
            let mut var = 0f32;
            for v in row.iter() {
                let d = v - mean;
                var += d * d;
            }
            var /= DIM as f32;
            let inv = 1.0 / (var + 1e-6).sqrt();
            for i in 0..DIM {
                row[i] = (row[i] - mean) * inv * (1.0 + scale[i]) + shift[i];
            }
        }
        linear(&out, &self.fl_linear_w, Some(&self.fl_linear_b), x.len() / DIM, DIM, DIM)
    }

    /// Euler CFG solver (reference `BASECFM.solve_euler`, t_span
    /// linspace(0, 1, n_steps+1), batched CFG collapsed to two passes).
    ///
    /// - `mu`: time-major `[t_len, 512]` (`cat_condition`)
    /// - `prompt_mel`: channel-major `[80, prompt_len]` reference mel
    /// - `noise`: initial x (draw index 0, `80 * t_len` values)
    ///
    /// The prompt region of x is zeroed before the loop and after every step
    /// (reference semantics — the final mel keeps it zeroed).
    #[allow(clippy::too_many_arguments)]
    pub fn solve_euler(
        &self,
        mu: &[f32],
        t_len: usize,
        prompt_mel: &[f32],
        prompt_len: usize,
        style: &[f32],
        n_steps: usize,
        cfg_rate: f32,
        noise: &mut dyn S2melNoiseSource,
        progress: Option<ProgressHook>,
    ) -> Result<CfmMel> {
        self.solve_euler_observed(
            mu, t_len, prompt_mel, prompt_len, style, n_steps, cfg_rate, noise, None, progress,
        )
    }

    /// [`Self::solve_euler`] with an optional per-step observer that sees the
    /// CFG-combined velocity (`step` is 1-based, matching the oracle dumps).
    #[allow(clippy::too_many_arguments)]
    pub fn solve_euler_observed(
        &self,
        mu: &[f32],
        t_len: usize,
        prompt_mel: &[f32],
        prompt_len: usize,
        style: &[f32],
        n_steps: usize,
        cfg_rate: f32,
        noise: &mut dyn S2melNoiseSource,
        mut on_velocity: Option<&mut dyn FnMut(usize, &[f32])>,
        mut progress: Option<ProgressHook>,
    ) -> Result<CfmMel> {
        if mu.len() != t_len * DIM
            || prompt_mel.len() != MELS * prompt_len
            || prompt_len > t_len
            || n_steps == 0
        {
            return Err(DiffusionError::model(format!(
                "cfm solve: bad sizes (t_len {t_len}, prompt_len {prompt_len}, mu {}, prompt {}, steps {n_steps})",
                mu.len(),
                prompt_mel.len()
            )));
        }
        let mut x = noise.draw(0, MELS * t_len);
        if x.len() != MELS * t_len {
            return Err(DiffusionError::model("cfm solve: noise source returned wrong length"));
        }
        let mut prompt_x = vec![0f32; MELS * t_len];
        for c in 0..MELS {
            prompt_x[c * t_len..c * t_len + prompt_len]
                .copy_from_slice(&prompt_mel[c * prompt_len..(c + 1) * prompt_len]);
            x[c * t_len..c * t_len + prompt_len].fill(0.0);
        }
        let zero_prompt = vec![0f32; MELS * t_len];
        let zero_style = vec![0f32; STYLE];
        let zero_mu = vec![0f32; t_len * DIM];
        let t_span: Vec<f32> = (0..=n_steps).map(|i| (i as f64 / n_steps as f64) as f32).collect();
        let mut t = t_span[0];
        for step in 1..=n_steps {
            emit_progress(
                &mut progress,
                &format!("s2mel-cfm {step}/{n_steps}"),
                (step - 1) as f64 / n_steps as f64,
            )?;
            let dt = t_span[step] - t_span[step - 1];
            let v_cond = self.forward(&x, &prompt_x, t, style, mu, t_len)?;
            let mut v = self.forward(&x, &zero_prompt, t, &zero_style, &zero_mu, t_len)?;
            for (u, &c) in v.iter_mut().zip(&v_cond) {
                *u = (1.0 + cfg_rate) * c - cfg_rate * *u;
            }
            if let Some(observe) = on_velocity.as_mut() {
                observe(step, &v);
            }
            for (xv, &dv) in x.iter_mut().zip(&v) {
                *xv += dt * dv;
            }
            t += dt;
            for c in 0..MELS {
                x[c * t_len..c * t_len + prompt_len].fill(0.0);
            }
        }
        emit_progress(&mut progress, &format!("s2mel-cfm {n_steps}/{n_steps}"), 1.0)?;
        Ok(CfmMel { mel: x, frames: t_len, prompt_frames: prompt_len })
    }
}

/// Solver output: the full mel (prompt region zeroed, channel-major
/// `[80, frames]`) plus the prompt split point.
pub struct CfmMel {
    pub mel: Vec<f32>,
    pub frames: usize,
    pub prompt_frames: usize,
}

impl CfmMel {
    /// The generated tail `mel[.., prompt_frames..]`, channel-major
    /// `[80, frames - prompt_frames]`.
    pub fn vc_target(&self) -> Vec<f32> {
        let gen = self.frames - self.prompt_frames;
        let mut out = vec![0f32; MELS * gen];
        for c in 0..MELS {
            out[c * gen..(c + 1) * gen]
                .copy_from_slice(&self.mel[c * self.frames + self.prompt_frames..(c + 1) * self.frames]);
        }
        out
    }
}

// ---------------------------------------------------------------------------
// Noise sources (the Sa3NoiseSource pattern: oracle replay for validation,
// seeded splitmix64 gaussians for production).
// ---------------------------------------------------------------------------

/// Noise provider for [`CfmEstimator::solve_euler`]; index 0 is the initial
/// latent (the Euler solver draws exactly once).
pub trait S2melNoiseSource {
    fn draw(&mut self, index: usize, len: usize) -> Vec<f32>;
}

/// Deterministic gaussian source (splitmix64 + Box-Muller). Same-seed output
/// is stable across runs but NOT torch-compatible bit-for-bit.
pub struct S2melSeededNoise {
    state: u64,
    spare: Option<f32>,
}

impl S2melSeededNoise {
    pub fn new(seed: u64) -> Self {
        Self { state: seed ^ 0x9E37_79B9_7F4A_7C15, spare: None }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn next_f32(&mut self) -> f32 {
        ((self.next_u64() >> 40) as f32 + 0.5) / 16_777_216.0
    }

    fn next_gaussian(&mut self) -> f32 {
        if let Some(v) = self.spare.take() {
            return v;
        }
        let u1 = self.next_f32();
        let u2 = self.next_f32();
        let r = (-2.0 * u1.ln()).sqrt();
        let theta = 2.0 * std::f32::consts::PI * u2;
        self.spare = Some(r * theta.sin());
        r * theta.cos()
    }
}

impl S2melNoiseSource for S2melSeededNoise {
    fn draw(&mut self, _index: usize, len: usize) -> Vec<f32> {
        (0..len).map(|_| self.next_gaussian()).collect()
    }
}

// ---------------------------------------------------------------------------
// Bundle loader.
// ---------------------------------------------------------------------------

/// Both s2mel stages loaded from `s2mel.pth`.
pub struct S2mel {
    pub length_regulator: LengthRegulator,
    pub estimator: CfmEstimator,
    /// Production-step device solver session, built at load whenever the
    /// CUDA backend is available. A build failure fails the load loudly —
    /// there is no silent CPU fallback on CUDA machines.
    cuda_session: Option<cuda::S2melCudaSession>,
}

impl S2mel {
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let mut sd = PthStateDict::load_nested(path)?;
        let length_regulator = LengthRegulator::load_sd(&mut sd, "net.length_regulator")?;
        let estimator = CfmEstimator::load_sd(&mut sd, "net.cfm.estimator")?;
        let cuda_session = if cuda::s2mel_cuda_available() {
            Some(cuda::S2melCudaSession::new(&estimator, S2MEL_CFM_STEPS)?)
        } else {
            None
        };
        Ok(Self { length_regulator, estimator, cuda_session })
    }

    /// The device solver session when this build/machine runs the CUDA path.
    pub fn cuda_session(&self) -> Option<&cuda::S2melCudaSession> {
        self.cuda_session.as_ref()
    }

    /// Convenience: run the flow-matching stage with the production defaults
    /// (25 steps, CFG 0.7) on the device path when present, else the CPU
    /// reference implementation.
    #[allow(clippy::too_many_arguments)]
    pub fn generate_mel(
        &self,
        mu: &[f32],
        t_len: usize,
        prompt_mel: &[f32],
        prompt_len: usize,
        style: &[f32],
        noise: &mut dyn S2melNoiseSource,
        progress: Option<ProgressHook>,
    ) -> Result<CfmMel> {
        if let Some(session) = &self.cuda_session {
            return session.solve_euler_observed(
                &self.estimator,
                mu,
                t_len,
                prompt_mel,
                prompt_len,
                style,
                S2MEL_CFG_RATE,
                noise,
                None,
                progress,
            );
        }
        self.estimator.solve_euler(
            mu,
            t_len,
            prompt_mel,
            prompt_len,
            style,
            S2MEL_CFM_STEPS,
            S2MEL_CFG_RATE,
            noise,
            progress,
        )
    }
}

/// CUDA device path (child module so it can reuse the private oracle-
/// validated structs above).
#[path = "indextts_s2mel_cuda.rs"]
pub mod cuda;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mish_matches_torch_reference() {
        // torch.nn.functional.mish reference values.
        assert!((mish(1.0) - 0.865_098_4).abs() < 1e-6);
        assert!((mish(-1.0) - -0.303_401_46).abs() < 1e-6);
        assert!(mish(0.0).abs() < 1e-9);
        // softplus threshold branch: mish(25) ~= 25.
        assert!((mish(25.0) - 25.0).abs() < 1e-5);
    }

    #[test]
    fn nearest_interpolate_matches_hand_computed() {
        // F.interpolate(size=5, mode='nearest') on len 3: floor(j*3/5).
        let src = [10.0f32, 20.0, 30.0];
        let out: Vec<f32> = (0..5).map(|j| src[j * 3 / 5]).collect();
        assert_eq!(out, vec![10.0, 10.0, 20.0, 20.0, 30.0]);
        // 151 -> 260 stays in range and is monotone.
        let idx: Vec<usize> = (0..260).map(|j| j * 151 / 260).collect();
        assert_eq!(idx[0], 0);
        assert_eq!(*idx.last().unwrap(), 150);
        assert!(idx.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn rope_interleaved_matches_complex_reference() {
        // gpt_fast apply_rotary_emb: pairs (2j, 2j+1) rotated by
        // theta^(-2j/dim) * pos as a complex product.
        let heads = 2;
        let dim = 8;
        let pos = 3usize;
        let (cos, sin) = moss_rope_tables(8, dim, ROPE_BASE);
        let mut row: Vec<f32> = (0..heads * dim).map(|i| (i as f32 * 0.37).sin()).collect();
        let expect: Vec<f32> = {
            let mut out = vec![0f32; heads * dim];
            for h in 0..heads {
                for j in 0..dim / 2 {
                    let freq = (ROPE_BASE as f64).powf(-((2 * j) as f64) / dim as f64);
                    let ang = pos as f64 * freq;
                    let (s, c) = ang.sin_cos();
                    let e = row[h * dim + 2 * j] as f64;
                    let o = row[h * dim + 2 * j + 1] as f64;
                    out[h * dim + 2 * j] = (e * c - o * s) as f32;
                    out[h * dim + 2 * j + 1] = (o * c + e * s) as f32;
                }
            }
            out
        };
        moss_rope_apply_interleaved(&mut row, pos, heads, dim, &cos, &sin);
        for (a, b) in row.iter().zip(&expect) {
            assert!((a - b).abs() < 1e-5, "{a} vs {b}");
        }
    }

    #[test]
    fn group_norm_single_group_normalizes_jointly() {
        // GroupNorm(1, C) normalizes over channels AND time together.
        let mut x = vec![1.0f32, 2.0, 3.0, 4.0, 5.0, 6.0]; // (2 ch, 3 len)
        group_norm_single(&mut x, 3, &[1.0, 1.0], &[0.0, 0.0]);
        let mean: f32 = x.iter().sum::<f32>() / 6.0;
        let var: f32 = x.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / 6.0;
        assert!(mean.abs() < 1e-6);
        assert!((var - 1.0).abs() < 1e-4);
        // Channel affine is per channel.
        let mut y = vec![1.0f32, 2.0, 3.0, 4.0, 5.0, 6.0];
        group_norm_single(&mut y, 3, &[1.0, 0.0], &[0.0, 7.0]);
        assert!(y[3..].iter().all(|&v| (v - 7.0).abs() < 1e-6));
    }

    #[test]
    fn reflect_conv_pads_by_reflection() {
        // 1 channel, k3: padded row is [x1, x0, x1, x2, x1] for len 3.
        let conv = ReflectConv1d {
            w: vec![1.0, 0.0, 0.0], // picks the left-padded element
            b: vec![0.0],
            out_ch: 1,
            in_ch: 1,
            k: 3,
        };
        let out = conv.forward(&[1.0, 2.0, 3.0], 3);
        assert_eq!(out, vec![2.0, 1.0, 2.0]); // x[-1] = x[1] = 2
    }

    #[test]
    fn seeded_noise_is_deterministic_with_sane_moments() {
        let mut a = S2melSeededNoise::new(7);
        let mut b = S2melSeededNoise::new(7);
        let va = a.draw(0, 4096);
        let vb = b.draw(0, 4096);
        assert_eq!(va, vb);
        let mean: f32 = va.iter().sum::<f32>() / va.len() as f32;
        let var: f32 = va.iter().map(|v| v * v).sum::<f32>() / va.len() as f32;
        assert!(mean.abs() < 0.1);
        assert!((var - 1.0).abs() < 0.1);
    }

    #[test]
    fn euler_t_span_accumulates_like_reference() {
        // t += dt over f32 span differences, 25 steps: final t ~ 1.
        let n = 25usize;
        let span: Vec<f32> = (0..=n).map(|i| (i as f64 / n as f64) as f32).collect();
        let mut t = span[0];
        for step in 1..=n {
            t += span[step] - span[step - 1];
        }
        assert!((t - 1.0).abs() < 1e-5);
    }

    /// Regulator smoke on the reference checkpoint (skipped when absent):
    /// loads only the `net.length_regulator.` keys, runs a tiny forward.
    #[test]
    fn length_regulator_smoke_on_reference_checkpoint() {
        let path = crate::indextts::reference_checkpoints_dir().join("s2mel.pth");
        if !path.is_file() {
            eprintln!("skipping length_regulator_smoke_on_reference_checkpoint: {path:?} missing");
            return;
        }
        let mut sd = PthStateDict::load_nested(&path).unwrap();
        let reg = LengthRegulator::load_sd(&mut sd, "net.length_regulator").unwrap();
        let x = vec![0.25f32; 5 * REG_IN];
        let out = reg.forward(&x, 5, 9).unwrap();
        assert_eq!(out.len(), 9 * DIM);
        assert!(out.iter().all(|v| v.is_finite()));
    }
}
