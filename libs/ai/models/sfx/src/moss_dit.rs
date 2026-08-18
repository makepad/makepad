//! MOSS Wan-audio DiT (1.3B), CPU f32. One forward = one denoise velocity
//! prediction for a (128 x 1500) latent conditioned on a (512 x 2048) text
//! context and a scalar timestep.
//!
//! Reference: moss_soundeffect_v2/diffsynth/models/wan_audio_dit.py
//! (WanAudioModel, patch_size (1,)) + wan_video_dit.py (DiTBlock).

use crate::error::{DiffusionError, Result};
use makepad_ai_h3::h3::H3ShardedWeights;
use crate::moss::{
    moss_rope_apply_interleaved, moss_rope_tables, moss_sinusoid, MOSS_DIT_DIM, MOSS_DIT_EPS,
    MOSS_DIT_FFN, MOSS_DIT_FREQ_DIM, MOSS_DIT_HEADS, MOSS_DIT_HEAD_DIM, MOSS_DIT_LAYERS,
    MOSS_LATENT_DIM, MOSS_ROPE_THETA, MOSS_TEXT_TOKENS, MOSS_TE_HIDDEN,
};
use crate::sa3::{gelu_tanh, linear, par_rows, silu};
use crate::{emit_byte_progress, ProgressHook};

struct DitBlock {
    modulation: Vec<f32>, // (6, dim)
    self_q_w: Vec<f32>,
    self_q_b: Vec<f32>,
    self_k_w: Vec<f32>,
    self_k_b: Vec<f32>,
    self_v_w: Vec<f32>,
    self_v_b: Vec<f32>,
    self_o_w: Vec<f32>,
    self_o_b: Vec<f32>,
    self_norm_q: Vec<f32>,
    self_norm_k: Vec<f32>,
    cross_q_w: Vec<f32>,
    cross_q_b: Vec<f32>,
    cross_k_w: Vec<f32>,
    cross_k_b: Vec<f32>,
    cross_v_w: Vec<f32>,
    cross_v_b: Vec<f32>,
    cross_o_w: Vec<f32>,
    cross_o_b: Vec<f32>,
    cross_norm_q: Vec<f32>,
    cross_norm_k: Vec<f32>,
    norm3_w: Vec<f32>,
    norm3_b: Vec<f32>,
    ffn0_w: Vec<f32>,
    ffn0_b: Vec<f32>,
    ffn2_w: Vec<f32>,
    ffn2_b: Vec<f32>,
}

pub struct MossDit {
    patch_w: Vec<f32>, // (dim, latent_dim) — Conv1d k=1 squeezed
    patch_b: Vec<f32>,
    text0_w: Vec<f32>,
    text0_b: Vec<f32>,
    text2_w: Vec<f32>,
    text2_b: Vec<f32>,
    time0_w: Vec<f32>,
    time0_b: Vec<f32>,
    time2_w: Vec<f32>,
    time2_b: Vec<f32>,
    tproj_w: Vec<f32>, // (6*dim, dim)
    tproj_b: Vec<f32>,
    head_w: Vec<f32>, // (latent_dim, dim)
    head_b: Vec<f32>,
    head_mod: Vec<f32>, // (2, dim)
    blocks: Vec<DitBlock>,
    rope_cos: Vec<f32>,
    rope_sin: Vec<f32>,
    max_pos: usize,
}

/// Text context pre-projected through the DiT's text_embedding MLP —
/// timestep-independent, compute once per prompt.
pub struct MossDitContext {
    /// (MOSS_TEXT_TOKENS x dim)
    pub embedded: Vec<f32>,
}

impl MossDit {
    pub fn load(weights: &H3ShardedWeights) -> Result<Self> {
        Self::load_with_progress(weights, None)
    }

    /// [`Self::load`] with cumulative byte progress ("load moss dit
    /// 1.4/2.6GB") every ~256MB of disk read.
    pub fn load_with_progress(
        weights: &H3ShardedWeights,
        mut progress: Option<ProgressHook>,
    ) -> Result<Self> {
        let d = MOSS_DIT_DIM;
        let total_bytes = weights.total_disk_bytes() as usize;
        let mut done_bytes = 0usize;
        let mut last_emit = 0usize;
        emit_byte_progress(&mut progress, "load moss dit", 0, total_bytes)?;
        let mut get = |name: &str, len: usize| -> Result<Vec<f32>> {
            let v = weights.tensor_f32(name)?;
            if v.len() != len {
                return Err(DiffusionError::model(format!(
                    "moss dit tensor {name}: {} values, expected {len}",
                    v.len()
                )));
            }
            if progress.is_some() {
                done_bytes =
                    done_bytes.saturating_add(weights.tensor_disk_bytes(name)? as usize);
                if done_bytes - last_emit >= crate::BYTE_PROGRESS_STEP {
                    last_emit = done_bytes;
                    emit_byte_progress(&mut progress, "load moss dit", done_bytes, total_bytes)?;
                }
            }
            Ok(v)
        };
        let mut blocks = Vec::with_capacity(MOSS_DIT_LAYERS);
        for l in 0..MOSS_DIT_LAYERS {
            let p = format!("blocks.{l}");
            blocks.push(DitBlock {
                modulation: get(&format!("{p}.scale_shift_table"), 6 * d)?,
                self_q_w: get(&format!("{p}.attn1.to_q.weight"), d * d)?,
                self_q_b: get(&format!("{p}.attn1.to_q.bias"), d)?,
                self_k_w: get(&format!("{p}.attn1.to_k.weight"), d * d)?,
                self_k_b: get(&format!("{p}.attn1.to_k.bias"), d)?,
                self_v_w: get(&format!("{p}.attn1.to_v.weight"), d * d)?,
                self_v_b: get(&format!("{p}.attn1.to_v.bias"), d)?,
                self_o_w: get(&format!("{p}.attn1.to_out.0.weight"), d * d)?,
                self_o_b: get(&format!("{p}.attn1.to_out.0.bias"), d)?,
                self_norm_q: get(&format!("{p}.attn1.norm_q.weight"), d)?,
                self_norm_k: get(&format!("{p}.attn1.norm_k.weight"), d)?,
                cross_q_w: get(&format!("{p}.attn2.to_q.weight"), d * d)?,
                cross_q_b: get(&format!("{p}.attn2.to_q.bias"), d)?,
                cross_k_w: get(&format!("{p}.attn2.to_k.weight"), d * d)?,
                cross_k_b: get(&format!("{p}.attn2.to_k.bias"), d)?,
                cross_v_w: get(&format!("{p}.attn2.to_v.weight"), d * d)?,
                cross_v_b: get(&format!("{p}.attn2.to_v.bias"), d)?,
                cross_o_w: get(&format!("{p}.attn2.to_out.0.weight"), d * d)?,
                cross_o_b: get(&format!("{p}.attn2.to_out.0.bias"), d)?,
                cross_norm_q: get(&format!("{p}.attn2.norm_q.weight"), d)?,
                cross_norm_k: get(&format!("{p}.attn2.norm_k.weight"), d)?,
                norm3_w: get(&format!("{p}.norm2.weight"), d)?,
                norm3_b: get(&format!("{p}.norm2.bias"), d)?,
                ffn0_w: get(&format!("{p}.ffn.net.0.proj.weight"), MOSS_DIT_FFN * d)?,
                ffn0_b: get(&format!("{p}.ffn.net.0.proj.bias"), MOSS_DIT_FFN)?,
                ffn2_w: get(&format!("{p}.ffn.net.2.weight"), d * MOSS_DIT_FFN)?,
                ffn2_b: get(&format!("{p}.ffn.net.2.bias"), d)?,
            });
        }
        let max_pos = 4096;
        let (rope_cos, rope_sin) = moss_rope_tables(max_pos, MOSS_DIT_HEAD_DIM, MOSS_ROPE_THETA);
        Ok(Self {
            patch_w: get("patch_embedding.weight", d * MOSS_LATENT_DIM)?,
            patch_b: get("patch_embedding.bias", d)?,
            text0_w: get("condition_embedder.text_embedder.linear_1.weight", d * MOSS_TE_HIDDEN)?,
            text0_b: get("condition_embedder.text_embedder.linear_1.bias", d)?,
            text2_w: get("condition_embedder.text_embedder.linear_2.weight", d * d)?,
            text2_b: get("condition_embedder.text_embedder.linear_2.bias", d)?,
            time0_w: get("condition_embedder.time_embedder.linear_1.weight", d * MOSS_DIT_FREQ_DIM)?,
            time0_b: get("condition_embedder.time_embedder.linear_1.bias", d)?,
            time2_w: get("condition_embedder.time_embedder.linear_2.weight", d * d)?,
            time2_b: get("condition_embedder.time_embedder.linear_2.bias", d)?,
            tproj_w: get("condition_embedder.time_proj.weight", 6 * d * d)?,
            tproj_b: get("condition_embedder.time_proj.bias", 6 * d)?,
            head_w: get("proj_out.weight", MOSS_LATENT_DIM * d)?,
            head_b: get("proj_out.bias", MOSS_LATENT_DIM)?,
            head_mod: get("scale_shift_table", 2 * d)?,
            blocks,
            rope_cos,
            rope_sin,
            max_pos,
        })
    }

    /// Projects the raw TE context (MOSS_TEXT_TOKENS x 2048) through
    /// text_embedding — do this once per prompt.
    pub fn embed_context(&self, context: &[f32]) -> Result<MossDitContext> {
        let d = MOSS_DIT_DIM;
        if context.len() != MOSS_TEXT_TOKENS * MOSS_TE_HIDDEN {
            return Err(DiffusionError::model(format!(
                "moss dit context: {} values, expected {}",
                context.len(),
                MOSS_TEXT_TOKENS * MOSS_TE_HIDDEN
            )));
        }
        let mut hidden = linear(
            context,
            &self.text0_w,
            Some(&self.text0_b),
            MOSS_TEXT_TOKENS,
            MOSS_TE_HIDDEN,
            d,
        );
        for v in hidden.iter_mut() {
            *v = gelu_tanh(*v);
        }
        let embedded = linear(&hidden, &self.text2_w, Some(&self.text2_b), MOSS_TEXT_TOKENS, d, d);
        Ok(MossDitContext { embedded })
    }

    /// One velocity prediction. `latents` is channel-major (128 x frames),
    /// `timestep` in 0..=1000. Returns (128 x frames).
    pub fn forward(
        &self,
        latents: &[f32],
        frames: usize,
        timestep: f32,
        context: &MossDitContext,
    ) -> Result<Vec<f32>> {
        let d = MOSS_DIT_DIM;
        let c = MOSS_LATENT_DIM;
        if latents.len() != c * frames {
            return Err(DiffusionError::model(format!(
                "moss dit latents: {} values, expected {}",
                latents.len(),
                c * frames
            )));
        }
        if frames > self.max_pos {
            return Err(DiffusionError::model(format!(
                "moss dit: {frames} frames exceeds rope table {}",
                self.max_pos
            )));
        }

        // Optional debug taps: MOSS_TAPS=<dir> writes raw LE f32 dumps.
        let taps_dir = std::env::var("MOSS_TAPS").ok();
        let tap = |name: &str, data: &[f32]| {
            if let Some(dir) = &taps_dir {
                let bytes: Vec<u8> = data.iter().flat_map(|v| v.to_le_bytes()).collect();
                let _ = std::fs::write(format!("{dir}/{name}.bin"), bytes);
            }
        };

        // t embeddings: sinusoid(256) -> Linear+SiLU+Linear -> t (dim);
        // t_mod = Linear(SiLU(t)) -> (6, dim)
        let sinus = moss_sinusoid(MOSS_DIT_FREQ_DIM, timestep as f64);
        let mut t_vec = linear(&sinus, &self.time0_w, Some(&self.time0_b), 1, MOSS_DIT_FREQ_DIM, d);
        for v in t_vec.iter_mut() {
            *v = silu(*v);
        }
        let t_vec = linear(&t_vec, &self.time2_w, Some(&self.time2_b), 1, d, d);
        let mut t_act = t_vec.clone();
        for v in t_act.iter_mut() {
            *v = silu(*v);
        }
        let t_mod = linear(&t_act, &self.tproj_w, Some(&self.tproj_b), 1, d, 6 * d);
        tap("t", &t_vec);
        tap("t_mod", &t_mod);

        // patchify: token per frame, x[f] = W @ latents[:, f] + b
        let mut x = vec![0f32; frames * d];
        par_rows(&mut x, d, &|frame, row| {
            for (j, out) in row.iter_mut().enumerate() {
                let w_row = &self.patch_w[j * c..(j + 1) * c];
                let mut acc = self.patch_b[j];
                for i in 0..c {
                    acc += w_row[i] * latents[i * frames + frame];
                }
                *out = acc;
            }
        });

        tap("patchify", &x);
        tap("ctx_emb", &context.embedded);

        let heads = MOSS_DIT_HEADS;
        let hd = MOSS_DIT_HEAD_DIM;
        let scale = 1.0 / (hd as f32).sqrt();

        for (block_index, block) in self.blocks.iter().enumerate() {
            // modulation rows: (modulation + t_mod) -> 6 x dim
            let mod_row = |idx: usize| -> Vec<f32> {
                let base = idx * d;
                (0..d)
                    .map(|i| block.modulation[base + i] + t_mod[base + i])
                    .collect()
            };
            let shift_msa = mod_row(0);
            let scale_msa = mod_row(1);
            let gate_msa = mod_row(2);
            let shift_mlp = mod_row(3);
            let scale_mlp = mod_row(4);
            let gate_mlp = mod_row(5);

            // --- self attention ---
            let mut normed = x.clone();
            layer_norm_rows(&mut normed, d, None, None);
            modulate_rows(&mut normed, d, &shift_msa, &scale_msa);
            let mut q = linear(&normed, &block.self_q_w, Some(&block.self_q_b), frames, d, d);
            let mut k = linear(&normed, &block.self_k_w, Some(&block.self_k_b), frames, d, d);
            let v = linear(&normed, &block.self_v_w, Some(&block.self_v_b), frames, d, d);
            rms_full_rows(&mut q, d, &block.self_norm_q);
            rms_full_rows(&mut k, d, &block.self_norm_k);
            for row in 0..frames {
                moss_rope_apply_interleaved(
                    &mut q[row * d..(row + 1) * d],
                    row,
                    heads,
                    hd,
                    &self.rope_cos,
                    &self.rope_sin,
                );
                moss_rope_apply_interleaved(
                    &mut k[row * d..(row + 1) * d],
                    row,
                    heads,
                    hd,
                    &self.rope_cos,
                    &self.rope_sin,
                );
            }
            let attn = attention_full(&q, &k, &v, frames, frames, heads, hd, scale);
            let proj = linear(&attn, &block.self_o_w, Some(&block.self_o_b), frames, d, d);
            for row in 0..frames {
                for i in 0..d {
                    x[row * d + i] += gate_msa[i] * proj[row * d + i];
                }
            }

            // --- cross attention (pre-norm affine norm3, ungated residual) ---
            let mut normed = x.clone();
            layer_norm_rows(&mut normed, d, Some(&block.norm3_w), Some(&block.norm3_b));
            let mut q = linear(&normed, &block.cross_q_w, Some(&block.cross_q_b), frames, d, d);
            let mut k = linear(
                &context.embedded,
                &block.cross_k_w,
                Some(&block.cross_k_b),
                MOSS_TEXT_TOKENS,
                d,
                d,
            );
            let v = linear(
                &context.embedded,
                &block.cross_v_w,
                Some(&block.cross_v_b),
                MOSS_TEXT_TOKENS,
                d,
                d,
            );
            rms_full_rows(&mut q, d, &block.cross_norm_q);
            rms_full_rows(&mut k, d, &block.cross_norm_k);
            let attn = attention_full(&q, &k, &v, frames, MOSS_TEXT_TOKENS, heads, hd, scale);
            let proj = linear(&attn, &block.cross_o_w, Some(&block.cross_o_b), frames, d, d);
            for (xv, pv) in x.iter_mut().zip(proj.iter()) {
                *xv += *pv;
            }

            // --- ffn ---
            let mut normed = x.clone();
            layer_norm_rows(&mut normed, d, None, None);
            modulate_rows(&mut normed, d, &shift_mlp, &scale_mlp);
            let mut hidden = linear(
                &normed,
                &block.ffn0_w,
                Some(&block.ffn0_b),
                frames,
                d,
                MOSS_DIT_FFN,
            );
            for v in hidden.iter_mut() {
                *v = gelu_tanh(*v);
            }
            let ffn = linear(&hidden, &block.ffn2_w, Some(&block.ffn2_b), frames, MOSS_DIT_FFN, d);
            for row in 0..frames {
                for i in 0..d {
                    x[row * d + i] += gate_mlp[i] * ffn[row * d + i];
                }
            }
            if matches!(block_index, 0 | 1 | 14 | 29) {
                tap(&format!("block{block_index:02}"), &x);
            }
        }

        // head: LayerNorm(no affine) * (1+scale) + shift with (head_mod + t)
        let mut shift = vec![0f32; d];
        let mut scale_h = vec![0f32; d];
        for i in 0..d {
            shift[i] = self.head_mod[i] + t_vec[i];
            scale_h[i] = self.head_mod[d + i] + t_vec[i];
        }
        // NOTE: reference Head does (modulation + t_mod).chunk(2) where t_mod
        // here is the SAME t vector added to both halves.
        let mut normed = x;
        layer_norm_rows(&mut normed, d, None, None);
        modulate_rows(&mut normed, d, &shift, &scale_h);
        let out_tokens = linear(&normed, &self.head_w, Some(&self.head_b), frames, d, c);
        tap("head", &out_tokens);

        // unpatchify: (frames x 128) -> (128 x frames)
        let mut out = vec![0f32; c * frames];
        for frame in 0..frames {
            for i in 0..c {
                out[i * frames + frame] = out_tokens[frame * c + i];
            }
        }
        tap("final", &out);
        Ok(out)
    }
}

/// LayerNorm over rows of width `d` (optionally affine), eps MOSS_DIT_EPS.
fn layer_norm_rows(x: &mut [f32], d: usize, weight: Option<&[f32]>, bias: Option<&[f32]>) {
    par_rows(x, d, &|_row, row| {
        let mut mean = 0f32;
        for v in row.iter() {
            mean += *v;
        }
        mean /= d as f32;
        let mut var = 0f32;
        for v in row.iter() {
            let dv = *v - mean;
            var += dv * dv;
        }
        var /= d as f32;
        let inv = 1.0 / (var + MOSS_DIT_EPS).sqrt();
        match (weight, bias) {
            (Some(w), Some(b)) => {
                for (i, v) in row.iter_mut().enumerate() {
                    *v = (*v - mean) * inv * w[i] + b[i];
                }
            }
            _ => {
                for v in row.iter_mut() {
                    *v = (*v - mean) * inv;
                }
            }
        }
    });
}

/// x = x * (1 + scale) + shift, per row.
fn modulate_rows(x: &mut [f32], d: usize, shift: &[f32], scale: &[f32]) {
    par_rows(x, d, &|_row, row| {
        for (i, v) in row.iter_mut().enumerate() {
            *v = *v * (1.0 + scale[i]) + shift[i];
        }
    });
}

/// RMSNorm over the FULL row width (all heads jointly) with learned weight —
/// torch.nn.RMSNorm(dim) semantics, eps MOSS_DIT_EPS.
fn rms_full_rows(x: &mut [f32], d: usize, gamma: &[f32]) {
    if crate::metal_accel::rms_norm_mul_inplace(x, gamma, d, MOSS_DIT_EPS) {
        return;
    }
    par_rows(x, d, &|_row, row| {
        let mut sum = 0f32;
        for v in row.iter() {
            sum += *v * *v;
        }
        let inv = 1.0 / (sum / d as f32 + MOSS_DIT_EPS).sqrt();
        for (i, v) in row.iter_mut().enumerate() {
            *v = *v * inv * gamma[i];
        }
    });
}

/// Plain multi-head attention: q (q_rows x heads*hd), k/v (kv_rows x heads*hd).
fn attention_full(
    q: &[f32],
    k: &[f32],
    v: &[f32],
    q_rows: usize,
    kv_rows: usize,
    heads: usize,
    hd: usize,
    scale: f32,
) -> Vec<f32> {
    let width = heads * hd;
    if let Some(out) =
        crate::metal_accel::flash_attn_packed(q, k, v, q_rows, kv_rows, heads, hd, scale)
    {
        return out;
    }
    let mut out = vec![0f32; q_rows * width];
    par_rows(&mut out, width, &|row, out_row| {
        let mut scores = vec![0f32; kv_rows];
        for head in 0..heads {
            let q_vec = &q[row * width + head * hd..][..hd];
            let mut max_s = f32::NEG_INFINITY;
            for (kr, score) in scores.iter_mut().enumerate() {
                let k_vec = &k[kr * width + head * hd..][..hd];
                let mut dot = 0f32;
                for i in 0..hd {
                    dot += q_vec[i] * k_vec[i];
                }
                *score = dot * scale;
                max_s = max_s.max(*score);
            }
            let mut denom = 0f32;
            for score in scores.iter_mut() {
                *score = (*score - max_s).exp();
                denom += *score;
            }
            let inv = 1.0 / denom;
            let out_vec = &mut out_row[head * hd..(head + 1) * hd];
            out_vec.fill(0.0);
            for (kr, &score) in scores.iter().enumerate() {
                let w = score * inv;
                let v_vec = &v[kr * width + head * hd..][..hd];
                for i in 0..hd {
                    out_vec[i] += w * v_vec[i];
                }
            }
        }
    });
    out
}

// ---------------------------------------------------------------------------
// CUDA device path (f16 cached weights, f32 activations — MOSS activations
// are tiny: x absmax ~4.4, out ~8.7, block hidden ramps to ~14k << f16 max,
// but we keep f32 activations for the first correctness pass).
// ---------------------------------------------------------------------------

use crate::sa3::{dev_err, F16Weight};
use makepad_ggml::backend::cuda::{
    gpu_add, gpu_attention_packed, gpu_attention_packed_cross, gpu_download,
    gpu_gated_residual_mod, gpu_gelu, gpu_layer_norm_mod, gpu_layer_norm_mul_add,
    gpu_linear_nt_cached, gpu_rms_norm_mul, gpu_rope_interleaved, gpu_slice_cols, gpu_upload,
    GpuTensor,
};

struct DitDeviceBlock {
    /// Packed self q/k/v (3d x d) + bias (3d).
    self_qkv: F16Weight,
    self_qkv_b: Vec<f32>,
    self_out: F16Weight,
    cross_q: F16Weight,
    /// Packed cross k/v (2d x d) + bias (2d).
    cross_kv: F16Weight,
    cross_kv_b: Vec<f32>,
    cross_out: F16Weight,
    ffn0: F16Weight,
    ffn2: F16Weight,
}

pub struct MossDitDevice {
    patch: F16Weight,
    head: F16Weight,
    blocks: Vec<DitDeviceBlock>,
}

/// Per-context device residents (cross K/V are timestep-independent).
pub struct MossDitDeviceRun {
    /// Per block: rms-normed + biased cross K, raw cross V (512 x d each).
    cross_kv: Vec<(GpuTensor, GpuTensor)>,
}

/// Per-generation rope tables (frames x half_dim).
pub struct MossDitDeviceRope {
    cos: GpuTensor,
    sin: GpuTensor,
    frames: usize,
}

impl MossDit {
    pub fn prepare_device(&self) -> MossDitDevice {
        let d = MOSS_DIT_DIM;
        let c = MOSS_LATENT_DIM;
        let blocks = self
            .blocks
            .iter()
            .enumerate()
            .map(|(i, block)| {
                let mut qkv = Vec::with_capacity(3 * d * d);
                qkv.extend_from_slice(&block.self_q_w);
                qkv.extend_from_slice(&block.self_k_w);
                qkv.extend_from_slice(&block.self_v_w);
                let mut qkv_b = Vec::with_capacity(3 * d);
                qkv_b.extend_from_slice(&block.self_q_b);
                qkv_b.extend_from_slice(&block.self_k_b);
                qkv_b.extend_from_slice(&block.self_v_b);
                let mut ckv = Vec::with_capacity(2 * d * d);
                ckv.extend_from_slice(&block.cross_k_w);
                ckv.extend_from_slice(&block.cross_v_w);
                let mut ckv_b = Vec::with_capacity(2 * d);
                ckv_b.extend_from_slice(&block.cross_k_b);
                ckv_b.extend_from_slice(&block.cross_v_b);
                DitDeviceBlock {
                    self_qkv: F16Weight::new(format!("mossdit.{i}.qkv"), &qkv, 3 * d, d),
                    self_qkv_b: qkv_b,
                    self_out: F16Weight::new(format!("mossdit.{i}.so"), &block.self_o_w, d, d),
                    cross_q: F16Weight::new(format!("mossdit.{i}.cq"), &block.cross_q_w, d, d),
                    cross_kv: F16Weight::new(format!("mossdit.{i}.ckv"), &ckv, 2 * d, d),
                    cross_kv_b: ckv_b,
                    cross_out: F16Weight::new(format!("mossdit.{i}.co"), &block.cross_o_w, d, d),
                    ffn0: F16Weight::new(format!("mossdit.{i}.f0"), &block.ffn0_w, MOSS_DIT_FFN, d),
                    ffn2: F16Weight::new(format!("mossdit.{i}.f2"), &block.ffn2_w, d, MOSS_DIT_FFN),
                }
            })
            .collect();
        MossDitDevice {
            patch: F16Weight::new("mossdit.patch", &self.patch_w, d, c),
            head: F16Weight::new("mossdit.head", &self.head_w, c, d),
            blocks,
        }
    }

    /// Uploads one context's cross K/V residents (call per prompt; the empty
    /// negative context yields constant tensors reusable across prompts).
    pub fn begin_device_run(
        &self,
        device: &MossDitDevice,
        context: &MossDitContext,
    ) -> Result<MossDitDeviceRun> {
        let d = MOSS_DIT_DIM;
        let ctx = gpu_upload(&context.embedded, MOSS_TEXT_TOKENS, d)
            .map_err(|e| dev_err("moss dit ctx upload", e))?;
        let mut cross_kv = Vec::with_capacity(self.blocks.len());
        for (i, block) in self.blocks.iter().enumerate() {
            let kv = gpu_linear_nt_cached(
                &ctx,
                "mossdit",
                &[device.blocks[i].cross_kv.part()],
                &device.blocks[i].cross_kv_b,
            )
            .map_err(|e| dev_err("moss dit cross kv", e))?;
            let k = gpu_slice_cols(&kv, 0, d).map_err(|e| dev_err("moss dit ck slice", e))?;
            let v = gpu_slice_cols(&kv, d, d).map_err(|e| dev_err("moss dit cv slice", e))?;
            let k = gpu_rms_norm_mul(
                &k,
                d,
                "mossdit",
                &format!("b{i}.ckn"),
                &block.cross_norm_k,
                MOSS_DIT_EPS,
            )
            .map_err(|e| dev_err("moss dit ck norm", e))?;
            cross_kv.push((k, v));
        }
        Ok(MossDitDeviceRun { cross_kv })
    }

    /// Uploads the per-position interleaved rope tables for `frames` rows.
    pub fn device_rope(&self, frames: usize) -> Result<MossDitDeviceRope> {
        let half = MOSS_DIT_HEAD_DIM / 2;
        let cos = gpu_upload(&self.rope_cos[..frames * half], frames, half)
            .map_err(|e| dev_err("moss rope cos", e))?;
        let sin = gpu_upload(&self.rope_sin[..frames * half], frames, half)
            .map_err(|e| dev_err("moss rope sin", e))?;
        Ok(MossDitDeviceRope { cos, sin, frames })
    }

    /// One device velocity prediction. `x_tokens` is FRAME-major
    /// (frames x 128); returns FRAME-major (frames x 128).
    pub fn forward_device(
        &self,
        device: &MossDitDevice,
        run: &MossDitDeviceRun,
        rope: &MossDitDeviceRope,
        x_tokens: &[f32],
        timestep: f32,
    ) -> Result<Vec<f32>> {
        let d = MOSS_DIT_DIM;
        let c = MOSS_LATENT_DIM;
        let frames = rope.frames;
        let heads = MOSS_DIT_HEADS;
        let scale = 1.0 / (MOSS_DIT_HEAD_DIM as f32).sqrt();
        if x_tokens.len() != frames * c {
            return Err(DiffusionError::model(format!(
                "moss dit device: {} x values, expected {}",
                x_tokens.len(),
                frames * c
            )));
        }

        // Host: t vectors + full AdaLN table (30 x 6d), raw values (gates raw).
        let sinus = moss_sinusoid(MOSS_DIT_FREQ_DIM, timestep as f64);
        let mut t_vec = linear(&sinus, &self.time0_w, Some(&self.time0_b), 1, MOSS_DIT_FREQ_DIM, d);
        for v in t_vec.iter_mut() {
            *v = silu(*v);
        }
        let t_vec = linear(&t_vec, &self.time2_w, Some(&self.time2_b), 1, d, d);
        let mut t_act = t_vec.clone();
        for v in t_act.iter_mut() {
            *v = silu(*v);
        }
        let t_mod = linear(&t_act, &self.tproj_w, Some(&self.tproj_b), 1, d, 6 * d);
        let depth = self.blocks.len();
        let mut table = vec![0f32; depth * 6 * d];
        for (b, block) in self.blocks.iter().enumerate() {
            let row = &mut table[b * 6 * d..(b + 1) * 6 * d];
            for i in 0..6 * d {
                row[i] = block.modulation[i] + t_mod[i];
            }
        }
        let mods = gpu_upload(&table, depth, 6 * d).map_err(|e| dev_err("moss dit mods", e))?;

        // patchify (k=1 conv == linear over channels), frame-major input.
        let x_dev = gpu_upload(x_tokens, frames, c).map_err(|e| dev_err("moss dit x upload", e))?;
        let mut h = gpu_linear_nt_cached(&x_dev, "mossdit", &[device.patch.part()], &self.patch_b)
            .map_err(|e| dev_err("moss dit patch", e))?;

        for (b, block) in self.blocks.iter().enumerate() {
            let dev = &device.blocks[b];
            let base = b * 6 * d;

            // --- self attention: LN(no affine) + (1+scale)+shift, qkv, full
            //     RMS norms, interleaved rope, gated residual ---
            let a = gpu_layer_norm_mod(&h, &mods, base + d, base, MOSS_DIT_EPS)
                .map_err(|e| dev_err("moss dit norm1", e))?;
            let qkv = gpu_linear_nt_cached(&a, "mossdit", &[dev.self_qkv.part()], &dev.self_qkv_b)
                .map_err(|e| dev_err("moss dit qkv", e))?;
            let q = gpu_slice_cols(&qkv, 0, d).map_err(|e| dev_err("moss dit q slice", e))?;
            let k = gpu_slice_cols(&qkv, d, d).map_err(|e| dev_err("moss dit k slice", e))?;
            let v = gpu_slice_cols(&qkv, 2 * d, d).map_err(|e| dev_err("moss dit v slice", e))?;
            let q = gpu_rms_norm_mul(&q, d, "mossdit", &format!("b{b}.qn"), &block.self_norm_q, MOSS_DIT_EPS)
                .map_err(|e| dev_err("moss dit q norm", e))?;
            let k = gpu_rms_norm_mul(&k, d, "mossdit", &format!("b{b}.kn"), &block.self_norm_k, MOSS_DIT_EPS)
                .map_err(|e| dev_err("moss dit k norm", e))?;
            let q = gpu_rope_interleaved(&q, heads, &rope.cos, &rope.sin)
                .map_err(|e| dev_err("moss dit rope q", e))?;
            let k = gpu_rope_interleaved(&k, heads, &rope.cos, &rope.sin)
                .map_err(|e| dev_err("moss dit rope k", e))?;
            let attn = gpu_attention_packed(&q, &k, &v, heads, scale)
                .map_err(|e| dev_err("moss dit self attention", e))?;
            let out = gpu_linear_nt_cached(&attn, "mossdit", &[dev.self_out.part()], &block.self_o_b)
                .map_err(|e| dev_err("moss dit self out", e))?;
            h = gpu_gated_residual_mod(&h, &out, &mods, base + 2 * d)
                .map_err(|e| dev_err("moss dit self gate", e))?;

            // --- cross attention: affine LN, plain residual, no rope ---
            // (gpu_layer_norm_mul_add applies LN * mul + add with NO +1 on mul)
            let cn = gpu_layer_norm_mul_add(&h, &block.norm3_w, &block.norm3_b, MOSS_DIT_EPS)
                .map_err(|e| dev_err("moss dit norm3", e))?;
            let cq = gpu_linear_nt_cached(&cn, "mossdit", &[dev.cross_q.part()], &block.cross_q_b)
                .map_err(|e| dev_err("moss dit cross q", e))?;
            let cq = gpu_rms_norm_mul(&cq, d, "mossdit", &format!("b{b}.cqn"), &block.cross_norm_q, MOSS_DIT_EPS)
                .map_err(|e| dev_err("moss dit cq norm", e))?;
            let (ck, cv) = &run.cross_kv[b];
            let cattn = gpu_attention_packed_cross(&cq, ck, cv, heads, scale)
                .map_err(|e| dev_err("moss dit cross attention", e))?;
            let cout = gpu_linear_nt_cached(&cattn, "mossdit", &[dev.cross_out.part()], &block.cross_o_b)
                .map_err(|e| dev_err("moss dit cross out", e))?;
            h = gpu_add(&h, &cout).map_err(|e| dev_err("moss dit cross residual", e))?;

            // --- ffn: LN mod, gelu-tanh, gated residual ---
            let f = gpu_layer_norm_mod(&h, &mods, base + 4 * d, base + 3 * d, MOSS_DIT_EPS)
                .map_err(|e| dev_err("moss dit norm2", e))?;
            let hidden = gpu_linear_nt_cached(&f, "mossdit", &[dev.ffn0.part()], &block.ffn0_b)
                .map_err(|e| dev_err("moss dit ffn0", e))?;
            let hidden = gpu_gelu(&hidden).map_err(|e| dev_err("moss dit gelu", e))?;
            let out = gpu_linear_nt_cached(&hidden, "mossdit", &[dev.ffn2.part()], &block.ffn2_b)
                .map_err(|e| dev_err("moss dit ffn2", e))?;
            h = gpu_gated_residual_mod(&h, &out, &mods, base + 5 * d)
                .map_err(|e| dev_err("moss dit ffn gate", e))?;
        }

        // head: LN(no affine) with shift/scale = head_mod + t, then linear.
        let mut head_table = vec![0f32; 2 * d];
        for i in 0..d {
            head_table[i] = self.head_mod[i] + t_vec[i]; // shift
            head_table[d + i] = self.head_mod[d + i] + t_vec[i]; // scale
        }
        let head_mods = gpu_upload(&head_table, 2, d).map_err(|e| dev_err("moss dit head mods", e))?;
        let hn = gpu_layer_norm_mod(&h, &head_mods, d, 0, MOSS_DIT_EPS)
            .map_err(|e| dev_err("moss dit head norm", e))?;
        let out = gpu_linear_nt_cached(&hn, "mossdit", &[device.head.part()], &self.head_b)
            .map_err(|e| dev_err("moss dit head", e))?;
        gpu_download(&out).map_err(|e| dev_err("moss dit download", e))
    }
}
