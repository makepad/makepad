//! MOSS text conditioner: Qwen3-1.7B causal LM, last hidden state (post
//! final norm), CPU f32. The reference right-pads the tokenized prompt to
//! 512 and zeroes the hidden rows past the valid length; since the model is
//! causal and pads are on the right, valid rows never see pad rows — so we
//! only run the valid prefix and return zero rows for the tail.

use crate::error::{DiffusionError, Result};
use makepad_ai_h3::h3::H3ShardedWeights;
use crate::{emit_byte_progress, emit_progress, ProgressHook};
use crate::moss::{
    moss_rope_apply_half, moss_rope_tables_half, MOSS_TEXT_TOKENS, MOSS_TE_FFN, MOSS_TE_HEAD_DIM,
    MOSS_TE_HIDDEN, MOSS_TE_KV_HEADS, MOSS_TE_LAYERS, MOSS_TE_Q_HEADS, MOSS_TE_RMS_EPS,
    MOSS_TE_ROPE_THETA,
};
use crate::sa3::{linear, par_rows, rms_norm_rows, silu};

struct TeLayer {
    input_ln: Vec<f32>,
    q_proj: Vec<f32>,
    k_proj: Vec<f32>,
    v_proj: Vec<f32>,
    o_proj: Vec<f32>,
    q_norm: Vec<f32>,
    k_norm: Vec<f32>,
    post_ln: Vec<f32>,
    gate: Vec<f32>,
    up: Vec<f32>,
    down: Vec<f32>,
}

pub struct MossTextEncoder {
    embed: Vec<f32>,
    layers: Vec<TeLayer>,
    final_norm: Vec<f32>,
    rope_cos: Vec<f32>,
    rope_sin: Vec<f32>,
}

impl MossTextEncoder {
    pub fn load(weights: &H3ShardedWeights) -> Result<Self> {
        Self::load_with_progress(weights, None)
    }

    /// [`Self::load`] with cumulative byte progress ("load moss te
    /// 1.2/3.4GB") every ~256MB of disk read — the cold host read of the
    /// whole TE dir is one of the pipeline's long stalls.
    pub fn load_with_progress(
        weights: &H3ShardedWeights,
        mut progress: Option<ProgressHook>,
    ) -> Result<Self> {
        let h = MOSS_TE_HIDDEN;
        let kv = MOSS_TE_KV_HEADS * MOSS_TE_HEAD_DIM;
        let total_bytes = weights.total_disk_bytes() as usize;
        let mut done_bytes = 0usize;
        let mut last_emit = 0usize;
        emit_byte_progress(&mut progress, "load moss te", 0, total_bytes)?;
        let mut get = |name: &str, len: usize| -> Result<Vec<f32>> {
            let v = weights.tensor_f32(name)?;
            if v.len() != len {
                return Err(DiffusionError::model(format!(
                    "moss te tensor {name}: {} values, expected {len}",
                    v.len()
                )));
            }
            if progress.is_some() {
                done_bytes =
                    done_bytes.saturating_add(weights.tensor_disk_bytes(name)? as usize);
                if done_bytes - last_emit >= crate::BYTE_PROGRESS_STEP {
                    last_emit = done_bytes;
                    emit_byte_progress(&mut progress, "load moss te", done_bytes, total_bytes)?;
                }
            }
            Ok(v)
        };
        let mut layers = Vec::with_capacity(MOSS_TE_LAYERS);
        for l in 0..MOSS_TE_LAYERS {
            let p = format!("model.layers.{l}");
            layers.push(TeLayer {
                input_ln: get(&format!("{p}.input_layernorm.weight"), h)?,
                q_proj: get(&format!("{p}.self_attn.q_proj.weight"), h * h)?,
                k_proj: get(&format!("{p}.self_attn.k_proj.weight"), kv * h)?,
                v_proj: get(&format!("{p}.self_attn.v_proj.weight"), kv * h)?,
                o_proj: get(&format!("{p}.self_attn.o_proj.weight"), h * h)?,
                q_norm: get(&format!("{p}.self_attn.q_norm.weight"), MOSS_TE_HEAD_DIM)?,
                k_norm: get(&format!("{p}.self_attn.k_norm.weight"), MOSS_TE_HEAD_DIM)?,
                post_ln: get(&format!("{p}.post_attention_layernorm.weight"), h)?,
                gate: get(&format!("{p}.mlp.gate_proj.weight"), MOSS_TE_FFN * h)?,
                up: get(&format!("{p}.mlp.up_proj.weight"), MOSS_TE_FFN * h)?,
                down: get(&format!("{p}.mlp.down_proj.weight"), h * MOSS_TE_FFN)?,
            });
        }
        let (rope_cos, rope_sin) =
            moss_rope_tables_half(MOSS_TEXT_TOKENS, MOSS_TE_HEAD_DIM, MOSS_TE_ROPE_THETA);
        let final_norm = get("model.norm.weight", h)?;
        // The embedding matrix is the one remaining big read.
        emit_byte_progress(&mut progress, "load moss te", done_bytes, total_bytes)?;
        let embed = weights.tensor_f32("model.embed_tokens.weight")?;
        Ok(Self {
            embed,
            layers,
            final_norm,
            rope_cos,
            rope_sin,
        })
    }

    /// Encodes `ids` (valid tokens only, no pads) and returns the padded
    /// (MOSS_TEXT_TOKENS x hidden) context with zero rows past `ids.len()`.
    pub fn encode(&self, ids: &[u32]) -> Result<Vec<f32>> {
        self.encode_with_progress(ids, None)
    }

    /// [`Self::encode`] ticking "text-encode k/28" per decoder layer.
    pub fn encode_with_progress(
        &self,
        ids: &[u32],
        mut progress: Option<ProgressHook>,
    ) -> Result<Vec<f32>> {
        let h = MOSS_TE_HIDDEN;
        let seq = ids.len().min(MOSS_TEXT_TOKENS);
        if seq == 0 {
            return Err(DiffusionError::model("moss te: empty token list"));
        }
        let mut x = vec![0f32; seq * h];
        for (row, &id) in ids[..seq].iter().enumerate() {
            let id = id as usize;
            if (id + 1) * h > self.embed.len() {
                return Err(DiffusionError::model(format!(
                    "moss te: token id {id} out of embedding range"
                )));
            }
            x[row * h..(row + 1) * h].copy_from_slice(&self.embed[id * h..(id + 1) * h]);
        }

        let q_heads = MOSS_TE_Q_HEADS;
        let kv_heads = MOSS_TE_KV_HEADS;
        let hd = MOSS_TE_HEAD_DIM;
        let group = q_heads / kv_heads;
        let scale = 1.0 / (hd as f32).sqrt();

        for (layer_index, layer) in self.layers.iter().enumerate() {
            if progress.is_some() {
                emit_progress(
                    &mut progress,
                    &format!("text-encode {}/{}", layer_index + 1, MOSS_TE_LAYERS),
                    layer_index as f64 / MOSS_TE_LAYERS as f64,
                )?;
            }
            // --- self attention ---
            let mut normed = x.clone();
            rms_norm_rows(&mut normed, &layer.input_ln, h, MOSS_TE_RMS_EPS);
            let mut q = linear(&normed, &layer.q_proj, None, seq, h, q_heads * hd);
            let mut k = linear(&normed, &layer.k_proj, None, seq, h, kv_heads * hd);
            let v = linear(&normed, &layer.v_proj, None, seq, h, kv_heads * hd);
            // per-head q/k RMS norm, then rope
            for row in 0..seq {
                let qrow = &mut q[row * q_heads * hd..(row + 1) * q_heads * hd];
                for head in 0..q_heads {
                    rms_head(&mut qrow[head * hd..(head + 1) * hd], &layer.q_norm);
                }
                moss_rope_apply_half(qrow, row, q_heads, hd, &self.rope_cos, &self.rope_sin);
                let krow = &mut k[row * kv_heads * hd..(row + 1) * kv_heads * hd];
                for head in 0..kv_heads {
                    rms_head(&mut krow[head * hd..(head + 1) * hd], &layer.k_norm);
                }
                moss_rope_apply_half(krow, row, kv_heads, hd, &self.rope_cos, &self.rope_sin);
            }
            // causal GQA attention
            let mut attn_out = vec![0f32; seq * q_heads * hd];
            par_rows(&mut attn_out, q_heads * hd, &|row, out_row| {
                let mut scores = vec![0f32; row + 1];
                for head in 0..q_heads {
                    let kv_head = head / group;
                    let q_vec = &q[row * q_heads * hd + head * hd..][..hd];
                    let mut max_s = f32::NEG_INFINITY;
                    for (key_row, score) in scores.iter_mut().enumerate() {
                        let k_vec = &k[key_row * kv_heads * hd + kv_head * hd..][..hd];
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
                    for (key_row, &score) in scores.iter().enumerate() {
                        let w = score * inv;
                        let v_vec = &v[key_row * kv_heads * hd + kv_head * hd..][..hd];
                        for i in 0..hd {
                            out_vec[i] += w * v_vec[i];
                        }
                    }
                }
            });
            let proj = linear(&attn_out, &layer.o_proj, None, seq, q_heads * hd, h);
            for (xv, pv) in x.iter_mut().zip(proj.iter()) {
                *xv += *pv;
            }

            // --- mlp ---
            let mut normed = x.clone();
            rms_norm_rows(&mut normed, &layer.post_ln, h, MOSS_TE_RMS_EPS);
            let gate = linear(&normed, &layer.gate, None, seq, h, MOSS_TE_FFN);
            let up = linear(&normed, &layer.up, None, seq, h, MOSS_TE_FFN);
            let mut act = vec![0f32; seq * MOSS_TE_FFN];
            par_rows(&mut act, MOSS_TE_FFN, &|row, slice| {
                let g = &gate[row * MOSS_TE_FFN..(row + 1) * MOSS_TE_FFN];
                let u = &up[row * MOSS_TE_FFN..(row + 1) * MOSS_TE_FFN];
                for (i, value) in slice.iter_mut().enumerate() {
                    *value = silu(g[i]) * u[i];
                }
            });
            let down = linear(&act, &layer.down, None, seq, MOSS_TE_FFN, h);
            for (xv, dv) in x.iter_mut().zip(down.iter()) {
                *xv += *dv;
            }
        }

        rms_norm_rows(&mut x, &self.final_norm, h, MOSS_TE_RMS_EPS);

        // Right-pad with zero rows to the fixed context length.
        let mut out = vec![0f32; MOSS_TEXT_TOKENS * h];
        out[..seq * h].copy_from_slice(&x);
        Ok(out)
    }
}

fn rms_head(head: &mut [f32], gamma: &[f32]) {
    let mut sum = 0f32;
    for v in head.iter() {
        sum += v * v;
    }
    let inv = 1.0 / (sum / head.len() as f32 + MOSS_TE_RMS_EPS).sqrt();
    for (v, g) in head.iter_mut().zip(gamma.iter()) {
        *v = *v * inv * *g;
    }
}

// ---------------------------------------------------------------------------
// CUDA device path (f16 cached weights, f32 activations). The prompt is
// short (<=512 tokens, typically ~20) so every gemm is tiny; the win over
// CPU is the 28-layer sequential latency (~2s -> ~0.1s).
// ---------------------------------------------------------------------------

use crate::sa3::{dev_err, F16Weight};
use makepad_ai_common::backend::cuda::{
    gpu_attention_packed_causal, gpu_concat_cols, gpu_download, gpu_linear_nt_cached,
    gpu_rms_norm_mul, gpu_rope_half, gpu_upload, GpuTensor,
};

struct TeDeviceLayer {
    /// Packed q/k/v ((q+2kv) x hidden).
    qkv: F16Weight,
    o: F16Weight,
    /// Packed gate/up (2*ffn x hidden).
    gate_up: F16Weight,
    down: F16Weight,
}

pub struct MossTeDevice {
    layers: Vec<TeDeviceLayer>,
    rope_cos_full: Vec<f32>,
    rope_sin_full: Vec<f32>,
}

impl MossTextEncoder {
    pub fn prepare_device(&self) -> MossTeDevice {
        let h = MOSS_TE_HIDDEN;
        let q = MOSS_TE_Q_HEADS * MOSS_TE_HEAD_DIM;
        let kv = MOSS_TE_KV_HEADS * MOSS_TE_HEAD_DIM;
        let layers = self
            .layers
            .iter()
            .enumerate()
            .map(|(i, layer)| {
                let mut qkv = Vec::with_capacity((q + 2 * kv) * h);
                qkv.extend_from_slice(&layer.q_proj);
                qkv.extend_from_slice(&layer.k_proj);
                qkv.extend_from_slice(&layer.v_proj);
                // gpu_swiglu_value_gate is VALUE-first: out = first * silu(second)
                // -> pack [up (value); gate].
                let mut gate_up = Vec::with_capacity(2 * MOSS_TE_FFN * h);
                gate_up.extend_from_slice(&layer.up);
                gate_up.extend_from_slice(&layer.gate);
                TeDeviceLayer {
                    qkv: F16Weight::new(format!("mosste.{i}.qkv"), &qkv, q + 2 * kv, h),
                    o: F16Weight::new(format!("mosste.{i}.o"), &layer.o_proj, h, q),
                    gate_up: F16Weight::new(format!("mosste.{i}.gu"), &gate_up, 2 * MOSS_TE_FFN, h),
                    down: F16Weight::new(format!("mosste.{i}.dn"), &layer.down, h, MOSS_TE_FFN),
                }
            })
            .collect();
        MossTeDevice {
            layers,
            rope_cos_full: self.rope_cos.clone(),
            rope_sin_full: self.rope_sin.clone(),
        }
    }

    /// Device encode; same contract as [`MossTextEncoder::encode`].
    pub fn encode_device(&self, device: &MossTeDevice, ids: &[u32]) -> Result<Vec<f32>> {
        self.encode_device_with_progress(device, ids, None)
    }

    /// [`Self::encode_device`] ticking "text-encode k/28" per decoder layer —
    /// each layer's f16 weights stream into the device cache on first touch,
    /// so cold encodes move visibly.
    pub fn encode_device_with_progress(
        &self,
        device: &MossTeDevice,
        ids: &[u32],
        mut progress: Option<ProgressHook>,
    ) -> Result<Vec<f32>> {
        let h = MOSS_TE_HIDDEN;
        let seq = ids.len().min(MOSS_TEXT_TOKENS);
        if seq == 0 {
            return Err(DiffusionError::model("moss te: empty token list"));
        }
        let q_heads = MOSS_TE_Q_HEADS;
        let kv_heads = MOSS_TE_KV_HEADS;
        let hd = MOSS_TE_HEAD_DIM;
        let group = q_heads / kv_heads;
        let scale = 1.0 / (hd as f32).sqrt();
        let half = hd / 2;

        // Host embedding lookup (single gather), then device layers.
        let mut x_host = vec![0f32; seq * h];
        for (row, &id) in ids[..seq].iter().enumerate() {
            let id = id as usize;
            if (id + 1) * h > self.embed.len() {
                return Err(DiffusionError::model(format!(
                    "moss te: token id {id} out of embedding range"
                )));
            }
            x_host[row * h..(row + 1) * h].copy_from_slice(&self.embed[id * h..(id + 1) * h]);
        }
        let mut x = gpu_upload(&x_host, seq, h).map_err(|e| dev_err("moss te x", e))?;
        let rope_cos = gpu_upload(&device.rope_cos_full[..seq * half], seq, half)
            .map_err(|e| dev_err("moss te rope cos", e))?;
        let rope_sin = gpu_upload(&device.rope_sin_full[..seq * half], seq, half)
            .map_err(|e| dev_err("moss te rope sin", e))?;

        for (i, (layer, dev)) in self.layers.iter().zip(&device.layers).enumerate() {
            if progress.is_some() {
                emit_progress(
                    &mut progress,
                    &format!("text-encode {}/{}", i + 1, MOSS_TE_LAYERS),
                    i as f64 / MOSS_TE_LAYERS as f64,
                )?;
            }
            let normed = gpu_rms_norm_mul(
                &x, h, "mosste", &format!("l{i}.in"), &layer.input_ln, MOSS_TE_RMS_EPS,
            )
            .map_err(|e| dev_err("moss te input norm", e))?;
            let qkv = gpu_linear_nt_cached(&normed, "mosste", &[dev.qkv.part()], &[])
                .map_err(|e| dev_err("moss te qkv", e))?;
            let q = makepad_ai_common::backend::cuda::gpu_slice_cols(&qkv, 0, q_heads * hd)
                .map_err(|e| dev_err("moss te q slice", e))?;
            let k = makepad_ai_common::backend::cuda::gpu_slice_cols(&qkv, q_heads * hd, kv_heads * hd)
                .map_err(|e| dev_err("moss te k slice", e))?;
            let v = makepad_ai_common::backend::cuda::gpu_slice_cols(
                &qkv,
                (q_heads + kv_heads) * hd,
                kv_heads * hd,
            )
            .map_err(|e| dev_err("moss te v slice", e))?;
            // Qwen3 q/k norms share ONE head_dim weight across heads: group
            // RMS with group_cols = head_dim applies exactly that.
            let q = gpu_rms_norm_mul(
                &q, hd, "mosste", &format!("l{i}.qn"), &layer.q_norm, MOSS_TE_RMS_EPS,
            )
            .map_err(|e| dev_err("moss te q norm", e))?;
            let k = gpu_rms_norm_mul(
                &k, hd, "mosste", &format!("l{i}.kn"), &layer.k_norm, MOSS_TE_RMS_EPS,
            )
            .map_err(|e| dev_err("moss te k norm", e))?;
            let q = gpu_rope_half(&q, q_heads, half, &rope_cos, &rope_sin)
                .map_err(|e| dev_err("moss te rope q", e))?;
            let k = gpu_rope_half(&k, kv_heads, half, &rope_cos, &rope_sin)
                .map_err(|e| dev_err("moss te rope k", e))?;
            // GQA: expand kv heads to q heads by column duplication.
            let (k, v) = if group > 1 {
                let mut k_parts = Vec::with_capacity(q_heads);
                let mut v_parts = Vec::with_capacity(q_heads);
                for head in 0..q_heads {
                    let kv_head = head / group;
                    k_parts.push(
                        makepad_ai_common::backend::cuda::gpu_slice_cols(&k, kv_head * hd, hd)
                            .map_err(|e| dev_err("moss te k expand", e))?,
                    );
                    v_parts.push(
                        makepad_ai_common::backend::cuda::gpu_slice_cols(&v, kv_head * hd, hd)
                            .map_err(|e| dev_err("moss te v expand", e))?,
                    );
                }
                let k_refs: Vec<&GpuTensor> = k_parts.iter().collect();
                let v_refs: Vec<&GpuTensor> = v_parts.iter().collect();
                let k_full =
                    gpu_concat_cols(&k_refs).map_err(|e| dev_err("moss te k concat", e))?;
                let v_full =
                    gpu_concat_cols(&v_refs).map_err(|e| dev_err("moss te v concat", e))?;
                (k_full, v_full)
            } else {
                (k, v)
            };
            let attn = gpu_attention_packed_causal(&q, &k, &v, q_heads, scale)
                .map_err(|e| dev_err("moss te attention", e))?;
            let proj = gpu_linear_nt_cached(&attn, "mosste", &[dev.o.part()], &[])
                .map_err(|e| dev_err("moss te o", e))?;
            x = makepad_ai_common::backend::cuda::gpu_add(&x, &proj)
                .map_err(|e| dev_err("moss te attn residual", e))?;

            let normed = gpu_rms_norm_mul(
                &x, h, "mosste", &format!("l{i}.pn"), &layer.post_ln, MOSS_TE_RMS_EPS,
            )
            .map_err(|e| dev_err("moss te post norm", e))?;
            let gate_up = gpu_linear_nt_cached(&normed, "mosste", &[dev.gate_up.part()], &[])
                .map_err(|e| dev_err("moss te gate up", e))?;
            let act = makepad_ai_common::backend::cuda::gpu_swiglu_value_gate(&gate_up)
                .map_err(|e| dev_err("moss te swiglu", e))?;
            let down = gpu_linear_nt_cached(&act, "mosste", &[dev.down.part()], &[])
                .map_err(|e| dev_err("moss te down", e))?;
            x = makepad_ai_common::backend::cuda::gpu_add(&x, &down)
                .map_err(|e| dev_err("moss te mlp residual", e))?;
        }
        let x = gpu_rms_norm_mul(&x, h, "mosste", "final", &self.final_norm, MOSS_TE_RMS_EPS)
            .map_err(|e| dev_err("moss te final norm", e))?;
        let host = gpu_download(&x).map_err(|e| dev_err("moss te download", e))?;
        let mut out = vec![0f32; MOSS_TEXT_TOKENS * h];
        out[..seq * h].copy_from_slice(&host);
        Ok(out)
    }
}
