//! T5Gemma-B encoder (google/t5gemma-b-b-ul2) for the SA3 prompt conditioner —
//! CPU f32, mirrors transformers modeling_t5gemma.py exactly:
//! Gemma2-style bidirectional blocks, pre/post norms around both branches,
//! full-head-dim RoPE (theta 10000), attn logit softcapping 50, GeGLU MLP,
//! sqrt(hidden) embedding scale, (1+w) RMSNorm in f32, eps 1e-6.
//!
//! At SA3's 256-token max length the alternating sliding(4096)/full layers are
//! all effectively full attention, so no window logic is needed.
//!
//! The SA3 conditioner then replaces padding positions with the learned
//! `padding_embedding` from the main checkpoint (padding_mode "learned").

use crate::sa3::{gelu_tanh, linear, par_rows, Sa3Tensors};
use crate::{emit_progress, DiffusionError, ProgressHook, Result};
use std::path::Path;

pub const T5GEMMA_DIM: usize = 768;
pub const T5GEMMA_LAYERS: usize = 12;
pub const T5GEMMA_HEADS: usize = 12;
pub const T5GEMMA_HEAD_DIM: usize = 64;
pub const T5GEMMA_FFN: usize = 2048;
pub const T5GEMMA_EPS: f32 = 1e-6;
pub const T5GEMMA_SOFTCAP: f32 = 50.0;
pub const T5GEMMA_ROPE_THETA: f32 = 10_000.0;

struct Layer {
    pre_attn_norm: Vec<f32>,
    post_attn_norm: Vec<f32>,
    pre_ff_norm: Vec<f32>,
    post_ff_norm: Vec<f32>,
    q: Vec<f32>,
    k: Vec<f32>,
    v: Vec<f32>,
    o: Vec<f32>,
    gate: Vec<f32>,
    up: Vec<f32>,
    down: Vec<f32>,
}

pub struct T5GemmaEncoder {
    embed: Vec<f32>,
    layers: Vec<Layer>,
    final_norm: Vec<f32>,
}

/// Gemma RMSNorm: f32 x * rsqrt(mean(x^2)+eps) * (1 + w).
fn gemma_rms_norm(x: &[f32], w: &[f32]) -> Vec<f32> {
    let dim = w.len();
    let mut out = vec![0f32; x.len()];
    for (row_in, row_out) in x.chunks(dim).zip(out.chunks_mut(dim)) {
        let mut sum = 0f32;
        for v in row_in {
            sum += v * v;
        }
        let scale = 1.0 / (sum / dim as f32 + T5GEMMA_EPS).sqrt();
        for i in 0..dim {
            row_out[i] = row_in[i] * scale * (1.0 + w[i]);
        }
    }
    out
}

impl T5GemmaEncoder {
    /// Loads the encoder half of t5gemma-b-b-ul2/model.safetensors.
    pub fn load(path: impl AsRef<Path>) -> Result<Self> {
        let t = Sa3Tensors::load(path)?;
        let name = |suffix: &str| format!("model.encoder.{suffix}");
        let embed = t.f32_shaped(&name("embed_tokens.weight"), &[256_000, T5GEMMA_DIM])?;
        let mut layers = Vec::with_capacity(T5GEMMA_LAYERS);
        for i in 0..T5GEMMA_LAYERS {
            let l = |suffix: &str| name(&format!("layers.{i}.{suffix}"));
            layers.push(Layer {
                pre_attn_norm: t.f32_shaped(&l("pre_self_attn_layernorm.weight"), &[T5GEMMA_DIM])?,
                post_attn_norm: t.f32_shaped(&l("post_self_attn_layernorm.weight"), &[T5GEMMA_DIM])?,
                pre_ff_norm: t.f32_shaped(&l("pre_feedforward_layernorm.weight"), &[T5GEMMA_DIM])?,
                post_ff_norm: t.f32_shaped(&l("post_feedforward_layernorm.weight"), &[T5GEMMA_DIM])?,
                q: t.f32_shaped(&l("self_attn.q_proj.weight"), &[T5GEMMA_DIM, T5GEMMA_DIM])?,
                k: t.f32_shaped(&l("self_attn.k_proj.weight"), &[T5GEMMA_DIM, T5GEMMA_DIM])?,
                v: t.f32_shaped(&l("self_attn.v_proj.weight"), &[T5GEMMA_DIM, T5GEMMA_DIM])?,
                o: t.f32_shaped(&l("self_attn.o_proj.weight"), &[T5GEMMA_DIM, T5GEMMA_DIM])?,
                gate: t.f32_shaped(&l("mlp.gate_proj.weight"), &[T5GEMMA_FFN, T5GEMMA_DIM])?,
                up: t.f32_shaped(&l("mlp.up_proj.weight"), &[T5GEMMA_FFN, T5GEMMA_DIM])?,
                down: t.f32_shaped(&l("mlp.down_proj.weight"), &[T5GEMMA_DIM, T5GEMMA_FFN])?,
            });
        }
        let final_norm = t.f32_shaped(&name("norm.weight"), &[T5GEMMA_DIM])?;
        Ok(Self {
            embed,
            layers,
            final_norm,
        })
    }

    /// Encodes padded token ids (with attention mask, 1 = valid) into the
    /// final hidden states [tokens, 768]. Pad keys are masked out of
    /// attention; pad rows still flow through (the conditioner replaces them
    /// with the learned padding embedding afterwards).
    pub fn encode(&self, token_ids: &[u32], mask: &[bool]) -> Result<Vec<f32>> {
        self.encode_with_progress(token_ids, mask, None)
    }

    /// [`Self::encode`] ticking "text-encode k/12" per encoder layer.
    pub fn encode_with_progress(
        &self,
        token_ids: &[u32],
        mask: &[bool],
        mut progress: Option<ProgressHook>,
    ) -> Result<Vec<f32>> {
        let tokens = token_ids.len();
        if mask.len() != tokens {
            return Err(DiffusionError::model("t5gemma mask/ids length mismatch"));
        }
        let dim = T5GEMMA_DIM;
        let heads = T5GEMMA_HEADS;
        let hd = T5GEMMA_HEAD_DIM;

        // Embedding lookup * sqrt(hidden).
        let normalizer = (dim as f32).sqrt();
        let mut x = vec![0f32; tokens * dim];
        for (t_ix, &id) in token_ids.iter().enumerate() {
            let id = id as usize;
            if id >= 256_000 {
                return Err(DiffusionError::model(format!("t5gemma token id {id} out of range")));
            }
            let src = &self.embed[id * dim..(id + 1) * dim];
            let dst = &mut x[t_ix * dim..(t_ix + 1) * dim];
            for i in 0..dim {
                dst[i] = src[i] * normalizer;
            }
        }

        // Full-head-dim rope tables (positions 0..tokens).
        let half = hd / 2;
        let mut cos = vec![0f32; tokens * hd];
        let mut sin = vec![0f32; tokens * hd];
        for pos in 0..tokens {
            for i in 0..half {
                let inv = 1.0 / T5GEMMA_ROPE_THETA.powf(2.0 * i as f32 / hd as f32);
                let (s, c) = ((pos as f32) * inv).sin_cos();
                cos[pos * hd + i] = c;
                cos[pos * hd + half + i] = c;
                sin[pos * hd + i] = s;
                sin[pos * hd + half + i] = s;
            }
        }
        let apply_rope_full = |t_buf: &mut [f32]| {
            for tok in 0..tokens {
                for h in 0..heads {
                    let base = (tok * heads + h) * hd;
                    for i in 0..half {
                        let a = t_buf[base + i];
                        let b = t_buf[base + half + i];
                        t_buf[base + i] = a * cos[tok * hd + i] - b * sin[tok * hd + i];
                        t_buf[base + half + i] =
                            b * cos[tok * hd + half + i] + a * sin[tok * hd + half + i];
                    }
                }
            }
        };

        let key_mask: Vec<f32> = mask
            .iter()
            .map(|&m| if m { 0.0 } else { f32::NEG_INFINITY })
            .collect();
        let scale = 1.0 / (hd as f32).sqrt();

        for (layer_index, layer) in self.layers.iter().enumerate() {
            if progress.is_some() {
                emit_progress(
                    &mut progress,
                    &format!("text-encode {}/{}", layer_index + 1, T5GEMMA_LAYERS),
                    layer_index as f64 / T5GEMMA_LAYERS as f64,
                )?;
            }
            // Self-attention branch.
            let normed = gemma_rms_norm(&x, &layer.pre_attn_norm);
            let mut q = linear(&normed, &layer.q, None, tokens, dim, dim);
            let mut k = linear(&normed, &layer.k, None, tokens, dim, dim);
            let v = linear(&normed, &layer.v, None, tokens, dim, dim);
            apply_rope_full(&mut q);
            apply_rope_full(&mut k);

            // Softcapped attention: softmax(50*tanh(qk*scale/50) + mask) @ v.
            let mut attn_out = vec![0f32; tokens * dim];
            par_rows(&mut attn_out, dim, &|qt, out_row| {
                let mut scores = vec![0f32; tokens];
                for h in 0..heads {
                    let q_vec = &q[(qt * heads + h) * hd..(qt * heads + h + 1) * hd];
                    let mut max_score = f32::NEG_INFINITY;
                    for (kt, score) in scores.iter_mut().enumerate() {
                        let k_vec = &k[(kt * heads + h) * hd..(kt * heads + h + 1) * hd];
                        let mut acc = 0f32;
                        for i in 0..hd {
                            acc += q_vec[i] * k_vec[i];
                        }
                        let capped =
                            T5GEMMA_SOFTCAP * ((acc * scale) / T5GEMMA_SOFTCAP).tanh();
                        let s = capped + key_mask[kt];
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
                    let out_vec = &mut out_row[h * hd..(h + 1) * hd];
                    out_vec.fill(0.0);
                    for (kt, &score) in scores.iter().enumerate() {
                        let w = score * inv;
                        let v_vec = &v[(kt * heads + h) * hd..(kt * heads + h + 1) * hd];
                        for i in 0..hd {
                            out_vec[i] += w * v_vec[i];
                        }
                    }
                }
            });
            let projected = linear(&attn_out, &layer.o, None, tokens, dim, dim);
            let post = gemma_rms_norm(&projected, &layer.post_attn_norm);
            for i in 0..x.len() {
                x[i] += post[i];
            }

            // Feedforward branch (GeGLU).
            let normed = gemma_rms_norm(&x, &layer.pre_ff_norm);
            let gate = linear(&normed, &layer.gate, None, tokens, dim, T5GEMMA_FFN);
            let up = linear(&normed, &layer.up, None, tokens, dim, T5GEMMA_FFN);
            let mut inner = vec![0f32; tokens * T5GEMMA_FFN];
            for i in 0..inner.len() {
                inner[i] = gelu_tanh(gate[i]) * up[i];
            }
            let down = linear(&inner, &layer.down, None, tokens, T5GEMMA_FFN, dim);
            let post = gemma_rms_norm(&down, &layer.post_ff_norm);
            for i in 0..x.len() {
                x[i] += post[i];
            }
        }

        Ok(gemma_rms_norm(&x, &self.final_norm))
    }
}

/// Applies SA3's "learned" padding mode: pad positions replaced with the
/// checkpoint's `conditioner.conditioners.prompt.padding_embedding`.
pub fn apply_learned_padding(hidden: &mut [f32], mask: &[bool], padding_embedding: &[f32]) {
    let dim = padding_embedding.len();
    for (row, &valid) in hidden.chunks_mut(dim).zip(mask) {
        if !valid {
            row.copy_from_slice(padding_embedding);
        }
    }
}

// ---------------------------------------------------------------------------
// CUDA device path (f16 cached weights, f32 activations).
// ---------------------------------------------------------------------------

use crate::sa3::{dev_err, F16Weight};
use makepad_ai_common::backend::cuda::{
    gpu_add, gpu_attention_packed_softcap, gpu_download, gpu_geglu_tanh_value_gate,
    gpu_linear_nt_cached, gpu_rms_norm_mul, gpu_rope_half, gpu_upload,
};

struct DeviceLayer {
    /// (1 + w) folded norm gammas.
    pre_attn_norm: Vec<f32>,
    post_attn_norm: Vec<f32>,
    pre_ff_norm: Vec<f32>,
    post_ff_norm: Vec<f32>,
    q: F16Weight,
    k: F16Weight,
    v: F16Weight,
    o: F16Weight,
    /// GeGLU packed as [up (value), gate] output columns.
    up: F16Weight,
    gate: F16Weight,
    down: F16Weight,
}

/// Prepared device weights for the T5Gemma encoder.
pub struct T5GemmaDevice {
    layers: Vec<DeviceLayer>,
    final_norm: Vec<f32>,
}

fn fold_gemma_norm(w: &[f32]) -> Vec<f32> {
    w.iter().map(|v| 1.0 + v).collect()
}

impl T5GemmaEncoder {
    /// Converts the encoder weights for the CUDA path (f16 linears).
    pub fn prepare_device(&self) -> T5GemmaDevice {
        let d = T5GEMMA_DIM;
        let layers = self
            .layers
            .iter()
            .enumerate()
            .map(|(i, layer)| DeviceLayer {
                pre_attn_norm: fold_gemma_norm(&layer.pre_attn_norm),
                post_attn_norm: fold_gemma_norm(&layer.post_attn_norm),
                pre_ff_norm: fold_gemma_norm(&layer.pre_ff_norm),
                post_ff_norm: fold_gemma_norm(&layer.post_ff_norm),
                q: F16Weight::new(format!("sa3te.{i}.q"), &layer.q, d, d),
                k: F16Weight::new(format!("sa3te.{i}.k"), &layer.k, d, d),
                v: F16Weight::new(format!("sa3te.{i}.v"), &layer.v, d, d),
                o: F16Weight::new(format!("sa3te.{i}.o"), &layer.o, d, d),
                up: F16Weight::new(format!("sa3te.{i}.up"), &layer.up, T5GEMMA_FFN, d),
                gate: F16Weight::new(format!("sa3te.{i}.gate"), &layer.gate, T5GEMMA_FFN, d),
                down: F16Weight::new(format!("sa3te.{i}.down"), &layer.down, d, T5GEMMA_FFN),
            })
            .collect();
        T5GemmaDevice {
            layers,
            final_norm: fold_gemma_norm(&self.final_norm),
        }
    }

    /// Device forward: same contract as `encode`. The embedding lookup stays
    /// on the host (one 256x768 gather), everything else runs on the GPU.
    pub fn encode_device(
        &self,
        device: &T5GemmaDevice,
        token_ids: &[u32],
        mask: &[bool],
    ) -> Result<Vec<f32>> {
        self.encode_device_with_progress(device, token_ids, mask, None)
    }

    /// [`Self::encode_device`] ticking "text-encode k/12" per layer — each
    /// layer's f16 weights stream into the device cache on first touch.
    pub fn encode_device_with_progress(
        &self,
        device: &T5GemmaDevice,
        token_ids: &[u32],
        mask: &[bool],
        mut progress: Option<ProgressHook>,
    ) -> Result<Vec<f32>> {
        let tokens = token_ids.len();
        let d = T5GEMMA_DIM;

        // Host embedding lookup * sqrt(hidden).
        let normalizer = (d as f32).sqrt();
        let mut x_host = vec![0f32; tokens * d];
        for (t_ix, &id) in token_ids.iter().enumerate() {
            let id = id as usize;
            if id >= 256_000 {
                return Err(DiffusionError::model("t5gemma token id out of range"));
            }
            let src = &self.embed[id * d..(id + 1) * d];
            let dst = &mut x_host[t_ix * d..(t_ix + 1) * d];
            for i in 0..d {
                dst[i] = src[i] * normalizer;
            }
        }
        let mut x = gpu_upload(&x_host, tokens, d).map_err(|e| dev_err("te upload", e))?;

        // Full-head-dim rope tables: unique half = 32 entries per position.
        let half = T5GEMMA_HEAD_DIM / 2;
        let mut cos = vec![0f32; tokens * half];
        let mut sin = vec![0f32; tokens * half];
        for pos in 0..tokens {
            for i in 0..half {
                let inv =
                    1.0 / T5GEMMA_ROPE_THETA.powf(2.0 * i as f32 / T5GEMMA_HEAD_DIM as f32);
                let (s, c) = ((pos as f32) * inv).sin_cos();
                cos[pos * half + i] = c;
                sin[pos * half + i] = s;
            }
        }
        let cos = gpu_upload(&cos, tokens, half).map_err(|e| dev_err("te rope cos", e))?;
        let sin = gpu_upload(&sin, tokens, half).map_err(|e| dev_err("te rope sin", e))?;

        let mask_host: Vec<f32> = mask
            .iter()
            .map(|&m| if m { 0.0 } else { f32::NEG_INFINITY })
            .collect();
        let key_mask = gpu_upload(&mask_host, 1, tokens).map_err(|e| dev_err("te mask", e))?;
        let scale = 1.0 / (T5GEMMA_HEAD_DIM as f32).sqrt();

        for (i, layer) in device.layers.iter().enumerate() {
            if progress.is_some() {
                emit_progress(
                    &mut progress,
                    &format!("text-encode {}/{}", i + 1, T5GEMMA_LAYERS),
                    i as f64 / T5GEMMA_LAYERS as f64,
                )?;
            }
            let norm_key = |what: &str| format!("l{i}.{what}");
            let normed = gpu_rms_norm_mul(
                &x, d, "sa3te", &norm_key("pre_attn"), &layer.pre_attn_norm, T5GEMMA_EPS,
            )
            .map_err(|e| dev_err("te pre_attn norm", e))?;
            let q = gpu_linear_nt_cached(&normed, "sa3te", &[layer.q.part()], &[])
                .map_err(|e| dev_err("te q", e))?;
            let k = gpu_linear_nt_cached(&normed, "sa3te", &[layer.k.part()], &[])
                .map_err(|e| dev_err("te k", e))?;
            let v = gpu_linear_nt_cached(&normed, "sa3te", &[layer.v.part()], &[])
                .map_err(|e| dev_err("te v", e))?;
            let q = gpu_rope_half(&q, T5GEMMA_HEADS, half, &cos, &sin).map_err(|e| dev_err("te rope q", e))?;
            let k = gpu_rope_half(&k, T5GEMMA_HEADS, half, &cos, &sin).map_err(|e| dev_err("te rope k", e))?;
            let attn = gpu_attention_packed_softcap(
                &q,
                &k,
                &v,
                T5GEMMA_HEADS,
                scale,
                T5GEMMA_SOFTCAP,
                Some(&key_mask),
            )
            .map_err(|e| dev_err("te attention", e))?;
            let projected = gpu_linear_nt_cached(&attn, "sa3te", &[layer.o.part()], &[])
                .map_err(|e| dev_err("te o", e))?;
            let post = gpu_rms_norm_mul(
                &projected, d, "sa3te", &norm_key("post_attn"), &layer.post_attn_norm,
                T5GEMMA_EPS,
            )
            .map_err(|e| dev_err("te post_attn norm", e))?;
            x = gpu_add(&x, &post).map_err(|e| dev_err("te attn residual", e))?;

            let normed = gpu_rms_norm_mul(
                &x, d, "sa3te", &norm_key("pre_ff"), &layer.pre_ff_norm, T5GEMMA_EPS,
            )
            .map_err(|e| dev_err("te pre_ff norm", e))?;
            // [up (value), gate] packed output for the geglu kernel.
            let packed = gpu_linear_nt_cached(
                &normed,
                "sa3te",
                &[layer.up.part(), layer.gate.part()],
                &[],
            )
            .map_err(|e| dev_err("te up/gate", e))?;
            let inner = gpu_geglu_tanh_value_gate(&packed).map_err(|e| dev_err("te geglu", e))?;
            let down = gpu_linear_nt_cached(&inner, "sa3te", &[layer.down.part()], &[])
                .map_err(|e| dev_err("te down", e))?;
            let post = gpu_rms_norm_mul(
                &down, d, "sa3te", &norm_key("post_ff"), &layer.post_ff_norm, T5GEMMA_EPS,
            )
            .map_err(|e| dev_err("te post_ff norm", e))?;
            x = gpu_add(&x, &post).map_err(|e| dev_err("te ff residual", e))?;
        }

        let out = gpu_rms_norm_mul(&x, d, "sa3te", "final", &device.final_norm, T5GEMMA_EPS)
            .map_err(|e| dev_err("te final norm", e))?;
        gpu_download(&out).map_err(|e| dev_err("te download", e))
    }
}
