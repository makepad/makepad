//! ACE-Step 1.5 text path: Qwen3-0.6B encoder (full last-hidden) plus the
//! embedding-only lyric lookup. Prompt / lyric strings are formatted by
//! [`crate::ace::ace_format_prompt`] and tokenized with the in-repo Qwen2 BPE
//! ([`makepad_ai_h3::h3_tokenizer::H3Tokenizer`]).

use crate::ace::{
    ace_gqa_attention, ace_open_shards, ace_rms_head, ace_swiglu, ace_tensor_any, ace_tensor_shaped,
    ACE_MAX_LYRIC_TOKENS, ACE_MAX_TEXT_TOKENS, ACE_RMS_EPS, ACE_TE_FFN, ACE_TE_HEAD_DIM,
    ACE_TE_PAD_ID,
    ACE_TE_HIDDEN, ACE_TE_KV_HEADS, ACE_TE_LAYERS, ACE_TE_Q_HEADS, ACE_TE_ROPE_THETA,
};
use crate::error::{DiffusionError, Result};
use makepad_ai_h3::h3::H3ShardedWeights;
use makepad_ai_h3::h3_tokenizer::H3Tokenizer;
use makepad_ai_sfx::moss::{moss_rope_apply_half, moss_rope_tables_half};
use makepad_ai_sfx::sa3::{linear, rms_norm_rows};
use crate::{emit_byte_progress, emit_progress, ProgressHook};
use std::path::Path;

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

pub struct AceTextEncoder {
    embed: Vec<f32>,
    layers: Vec<TeLayer>,
    final_norm: Vec<f32>,
    rope_cos: Vec<f32>,
    rope_sin: Vec<f32>,
}

impl AceTextEncoder {
    pub fn load(dir: impl AsRef<Path>) -> Result<Self> {
        Self::load_with_progress(dir, None)
    }

    pub fn load_with_progress(
        dir: impl AsRef<Path>,
        mut progress: Option<ProgressHook>,
    ) -> Result<Self> {
        let weights = ace_open_shards(dir)?;
        Self::from_weights(&weights, &mut progress)
    }

    pub fn from_weights(
        weights: &H3ShardedWeights,
        progress: &mut Option<ProgressHook>,
    ) -> Result<Self> {
        let h = ACE_TE_HIDDEN;
        let kv = ACE_TE_KV_HEADS * ACE_TE_HEAD_DIM;
        let total_bytes = weights.total_disk_bytes() as usize;
        let mut done_bytes = 0usize;
        let mut last_emit = 0usize;
        emit_byte_progress(progress, "load ace te", 0, total_bytes)?;
        let mut get = |name: &str, len: usize| -> Result<Vec<f32>> {
            let v = ace_tensor_shaped(weights, &[name], len)?;
            if progress.is_some() {
                done_bytes = done_bytes.saturating_add(weights.tensor_disk_bytes(name).unwrap_or(0) as usize);
                if done_bytes - last_emit >= crate::BYTE_PROGRESS_STEP {
                    last_emit = done_bytes;
                    emit_byte_progress(progress, "load ace te", done_bytes, total_bytes)?;
                }
            }
            Ok(v)
        };
        let prefix = if weights.has_tensor("model.layers.0.input_layernorm.weight") {
            "model.layers"
        } else {
            "layers"
        };
        let q = ACE_TE_Q_HEADS * ACE_TE_HEAD_DIM;
        let mut layers = Vec::with_capacity(ACE_TE_LAYERS);
        for l in 0..ACE_TE_LAYERS {
            let p = format!("{prefix}.{l}");
            layers.push(TeLayer {
                input_ln: get(&format!("{p}.input_layernorm.weight"), h)?,
                q_proj: get(&format!("{p}.self_attn.q_proj.weight"), q * h)?,
                k_proj: get(&format!("{p}.self_attn.k_proj.weight"), kv * h)?,
                v_proj: get(&format!("{p}.self_attn.v_proj.weight"), kv * h)?,
                o_proj: get(&format!("{p}.self_attn.o_proj.weight"), h * q)?,
                q_norm: get(&format!("{p}.self_attn.q_norm.weight"), ACE_TE_HEAD_DIM)?,
                k_norm: get(&format!("{p}.self_attn.k_norm.weight"), ACE_TE_HEAD_DIM)?,
                post_ln: get(&format!("{p}.post_attention_layernorm.weight"), h)?,
                gate: get(&format!("{p}.mlp.gate_proj.weight"), ACE_TE_FFN * h)?,
                up: get(&format!("{p}.mlp.up_proj.weight"), ACE_TE_FFN * h)?,
                down: get(&format!("{p}.mlp.down_proj.weight"), h * ACE_TE_FFN)?,
            });
        }
        let (rope_cos, rope_sin) =
            moss_rope_tables_half(ACE_MAX_LYRIC_TOKENS, ACE_TE_HEAD_DIM, ACE_TE_ROPE_THETA);
        let final_norm = ace_tensor_shaped(weights, &["norm.weight", "model.norm.weight"], h)?;
        emit_byte_progress(progress, "load ace te", done_bytes, total_bytes)?;
        let embed = ace_tensor_any(
            weights,
            &["embed_tokens.weight", "model.embed_tokens.weight"],
        )?;
        if embed.len() % h != 0 {
            return Err(DiffusionError::model(format!(
                "ace te embed {} not divisible by {h}",
                embed.len()
            )));
        }
        Ok(Self {
            embed,
            layers,
            final_norm,
            rope_cos,
            rope_sin,
        })
    }

    pub fn embed_tokens(&self, ids: &[u32]) -> Result<Vec<f32>> {
        let h = ACE_TE_HIDDEN;
        let mut out = vec![0f32; ids.len() * h];
        for (row, &id) in ids.iter().enumerate() {
            let id = id as usize;
            if (id + 1) * h > self.embed.len() {
                return Err(DiffusionError::model(format!(
                    "ace te: token id {id} out of embedding range"
                )));
            }
            out[row * h..(row + 1) * h].copy_from_slice(&self.embed[id * h..(id + 1) * h]);
        }
        Ok(out)
    }

    /// Full causal Qwen3 encode of `ids` (no pad rows). Returns `[seq, hidden]`.
    pub fn encode(&self, ids: &[u32]) -> Result<Vec<f32>> {
        self.encode_with_progress(ids, None)
    }

    pub fn encode_with_progress(
        &self,
        ids: &[u32],
        mut progress: Option<ProgressHook>,
    ) -> Result<Vec<f32>> {
        let h = ACE_TE_HIDDEN;
        let seq = ids.len().min(ACE_MAX_TEXT_TOKENS);
        if seq == 0 {
            return Err(DiffusionError::model("ace te: empty token list"));
        }
        let mut x = self.embed_tokens(&ids[..seq])?;
        let q_heads = ACE_TE_Q_HEADS;
        let kv_heads = ACE_TE_KV_HEADS;
        let hd = ACE_TE_HEAD_DIM;

        for (layer_index, layer) in self.layers.iter().enumerate() {
            if progress.is_some() {
                emit_progress(
                    &mut progress,
                    &format!("text-encode {}/{}", layer_index + 1, ACE_TE_LAYERS),
                    layer_index as f64 / ACE_TE_LAYERS as f64,
                )?;
            }
            let mut normed = x.clone();
            rms_norm_rows(&mut normed, &layer.input_ln, h, ACE_RMS_EPS);
            let mut q = linear(&normed, &layer.q_proj, None, seq, h, q_heads * hd);
            let mut k = linear(&normed, &layer.k_proj, None, seq, h, kv_heads * hd);
            let v = linear(&normed, &layer.v_proj, None, seq, h, kv_heads * hd);
            for row in 0..seq {
                let qrow = &mut q[row * q_heads * hd..(row + 1) * q_heads * hd];
                for head in 0..q_heads {
                    ace_rms_head(&mut qrow[head * hd..(head + 1) * hd], &layer.q_norm);
                }
                moss_rope_apply_half(qrow, row, q_heads, hd, &self.rope_cos, &self.rope_sin);
                let krow = &mut k[row * kv_heads * hd..(row + 1) * kv_heads * hd];
                for head in 0..kv_heads {
                    ace_rms_head(&mut krow[head * hd..(head + 1) * hd], &layer.k_norm);
                }
                moss_rope_apply_half(krow, row, kv_heads, hd, &self.rope_cos, &self.rope_sin);
            }
            let attn = ace_gqa_attention(
                &q, &k, &v, q_heads, kv_heads, hd, seq, seq, None, true, None,
            );
            let proj = linear(&attn, &layer.o_proj, None, seq, q_heads * hd, h);
            crate::ace::ace_add_inplace(&mut x, &proj);

            let mut normed = x.clone();
            rms_norm_rows(&mut normed, &layer.post_ln, h, ACE_RMS_EPS);
            let gate = linear(&normed, &layer.gate, None, seq, h, ACE_TE_FFN);
            let up = linear(&normed, &layer.up, None, seq, h, ACE_TE_FFN);
            let act = ace_swiglu(&gate, &up);
            let down = linear(&act, &layer.down, None, seq, ACE_TE_FFN, h);
            crate::ace::ace_add_inplace(&mut x, &down);
        }
        rms_norm_rows(&mut x, &self.final_norm, h, ACE_RMS_EPS);
        Ok(x)
    }
}

pub fn ace_tokenize(tokenizer: &H3Tokenizer, text: &str, max_len: usize) -> Vec<u32> {
    let mut ids = tokenizer.encode(text);
    // Official AceStepPipeline uses tokenizer(..., add_special_tokens=True).
    // Qwen2TokenizerFast appends an extra `<|endoftext|>` (151643).
    ids.push(ACE_TE_PAD_ID);
    if ids.len() > max_len {
        ids.truncate(max_len);
    }
    ids
}

// ---------------------------------------------------------------------------
// CUDA device path: official Qwen3 is bf16 Linear + RMS + causal math-SDPA.
// ---------------------------------------------------------------------------

use makepad_ai_sfx::sa3::dev_err;
use makepad_ggml::backend::cuda::{
    gpu_add, gpu_attention_packed_causal_bf16, gpu_bf16_round, gpu_concat_cols, gpu_download,
    gpu_linear_nt_cached_bf16_mm, gpu_mul, gpu_rms_norm_mul, gpu_rope_half, gpu_silu,
    gpu_slice_cols, gpu_upload, GpuTensor,
};

struct TeBf16Weight {
    n: usize,
    key: String,
    bytes: Vec<u8>,
}

fn te_f32_to_bf16_word(v: f32) -> u16 {
    let bits = v.to_bits();
    let rounding_bias = 0x7FFFu32 + ((bits >> 16) & 1);
    ((bits + rounding_bias) >> 16) as u16
}

impl TeBf16Weight {
    fn new(key: impl Into<String>, w: &[f32], n: usize, k: usize) -> Self {
        debug_assert_eq!(w.len(), n * k);
        let mut bytes = Vec::with_capacity(w.len() * 2);
        for &v in w {
            bytes.extend_from_slice(&te_f32_to_bf16_word(v).to_le_bytes());
        }
        Self {
            n,
            key: key.into(),
            bytes,
        }
    }

    fn part(&self) -> makepad_ggml::backend::cuda::GpuLinearPart<'_> {
        makepad_ggml::backend::cuda::GpuLinearPart {
            bt_ggml_type: makepad_ggml::quant::GGML_TYPE_BF16,
            n: self.n,
            cache_key: &self.key,
            bytes: &self.bytes,
        }
    }
}

struct TeDeviceLayer {
    q: TeBf16Weight,
    k: TeBf16Weight,
    v: TeBf16Weight,
    o: TeBf16Weight,
    gate: TeBf16Weight,
    up: TeBf16Weight,
    down: TeBf16Weight,
}

pub struct AceTeDevice {
    layers: Vec<TeDeviceLayer>,
    rope_cos_full: Vec<f32>,
    rope_sin_full: Vec<f32>,
}

impl AceTextEncoder {
    pub fn prepare_device(&self) -> AceTeDevice {
        let h = ACE_TE_HIDDEN;
        let q = ACE_TE_Q_HEADS * ACE_TE_HEAD_DIM;
        let kv = ACE_TE_KV_HEADS * ACE_TE_HEAD_DIM;
        let layers = self
            .layers
            .iter()
            .enumerate()
            .map(|(i, layer)| TeDeviceLayer {
                q: TeBf16Weight::new(format!("acete.{i}.q"), &layer.q_proj, q, h),
                k: TeBf16Weight::new(format!("acete.{i}.k"), &layer.k_proj, kv, h),
                v: TeBf16Weight::new(format!("acete.{i}.v"), &layer.v_proj, kv, h),
                o: TeBf16Weight::new(format!("acete.{i}.o"), &layer.o_proj, h, q),
                gate: TeBf16Weight::new(format!("acete.{i}.gate"), &layer.gate, ACE_TE_FFN, h),
                up: TeBf16Weight::new(format!("acete.{i}.up"), &layer.up, ACE_TE_FFN, h),
                down: TeBf16Weight::new(format!("acete.{i}.dn"), &layer.down, h, ACE_TE_FFN),
            })
            .collect();
        AceTeDevice {
            layers,
            rope_cos_full: self.rope_cos.clone(),
            rope_sin_full: self.rope_sin.clone(),
        }
    }

    pub fn encode_device(
        &self,
        device: &AceTeDevice,
        ids: &[u32],
        mut progress: Option<ProgressHook>,
    ) -> Result<Vec<f32>> {
        let h = ACE_TE_HIDDEN;
        let seq = ids.len().min(ACE_MAX_TEXT_TOKENS);
        if seq == 0 {
            return Err(DiffusionError::model("ace te: empty token list"));
        }
        let q_heads = ACE_TE_Q_HEADS;
        let kv_heads = ACE_TE_KV_HEADS;
        let hd = ACE_TE_HEAD_DIM;
        let group = q_heads / kv_heads;
        let scale = 1.0 / (hd as f32).sqrt();
        let half = hd / 2;

        let x_host = self.embed_tokens(&ids[..seq])?;
        te_dump_npy("native_te_embed.npy", &x_host, &[1, seq, h]);
        let mut x = gpu_upload(&x_host, seq, h).map_err(|e| dev_err("ace te x", e))?;
        x = gpu_bf16_round(&x).map_err(|e| dev_err("ace te x rnd", e))?;
        let rope_cos = gpu_upload(&device.rope_cos_full[..seq * half], seq, half)
            .map_err(|e| dev_err("ace te rope cos", e))?;
        let rope_sin = gpu_upload(&device.rope_sin_full[..seq * half], seq, half)
            .map_err(|e| dev_err("ace te rope sin", e))?;
        let lin = |x: &GpuTensor, w: &TeBf16Weight| gpu_linear_nt_cached_bf16_mm(x, "acete", &[w.part()]);
        let rms = |x: &GpuTensor, group: usize, key: &str, w: &[f32]| -> Result<GpuTensor> {
            let n = gpu_rms_norm_mul(x, group, "acete", key, w, ACE_RMS_EPS)
                .map_err(|e| dev_err("ace te rms", e))?;
            gpu_bf16_round(&n).map_err(|e| dev_err("ace te rms rnd", e))
        };

        for (i, (layer, dev)) in self.layers.iter().zip(&device.layers).enumerate() {
            if progress.is_some() {
                emit_progress(
                    &mut progress,
                    &format!("text-encode {}/{}", i + 1, ACE_TE_LAYERS),
                    i as f64 / ACE_TE_LAYERS as f64,
                )?;
            }
            let normed = rms(&x, h, &format!("l{i}.in"), &layer.input_ln)?;
            let q = lin(&normed, &dev.q).map_err(|e| dev_err("ace te q", e))?;
            let k = lin(&normed, &dev.k).map_err(|e| dev_err("ace te k", e))?;
            let v = lin(&normed, &dev.v).map_err(|e| dev_err("ace te v", e))?;
            let q = rms(&q, hd, &format!("l{i}.qn"), &layer.q_norm)?;
            let k = rms(&k, hd, &format!("l{i}.kn"), &layer.k_norm)?;
            let q = gpu_rope_half(&q, q_heads, half, &rope_cos, &rope_sin)
                .map_err(|e| dev_err("ace te rope q", e))?;
            let k = gpu_rope_half(&k, kv_heads, half, &rope_cos, &rope_sin)
                .map_err(|e| dev_err("ace te rope k", e))?;
            let q = gpu_bf16_round(&q).map_err(|e| dev_err("ace te rq rnd", e))?;
            let k = gpu_bf16_round(&k).map_err(|e| dev_err("ace te rk rnd", e))?;
            let (k, v) = expand_gqa(k, v, q_heads, kv_heads, hd, group)?;
            let attn = gpu_attention_packed_causal_bf16(&q, &k, &v, q_heads, scale)
                .map_err(|e| dev_err("ace te attention", e))?;
            let attn = gpu_bf16_round(&attn).map_err(|e| dev_err("ace te attn rnd", e))?;
            let proj = lin(&attn, &dev.o).map_err(|e| dev_err("ace te o", e))?;
            x = gpu_add(&x, &proj).map_err(|e| dev_err("ace te attn residual", e))?;
            x = gpu_bf16_round(&x).map_err(|e| dev_err("ace te attn add rnd", e))?;

            let normed = rms(&x, h, &format!("l{i}.pn"), &layer.post_ln)?;
            let gate = lin(&normed, &dev.gate).map_err(|e| dev_err("ace te gate", e))?;
            let up = lin(&normed, &dev.up).map_err(|e| dev_err("ace te up", e))?;
            let act = gpu_silu(&gate).map_err(|e| dev_err("ace te silu", e))?;
            let act = gpu_bf16_round(&act).map_err(|e| dev_err("ace te silu rnd", e))?;
            let act = gpu_mul(&act, &up).map_err(|e| dev_err("ace te swiglu mul", e))?;
            let act = gpu_bf16_round(&act).map_err(|e| dev_err("ace te swiglu mul rnd", e))?;
            let down = lin(&act, &dev.down).map_err(|e| dev_err("ace te down", e))?;
            x = gpu_add(&x, &down).map_err(|e| dev_err("ace te mlp residual", e))?;
            x = gpu_bf16_round(&x).map_err(|e| dev_err("ace te mlp add rnd", e))?;
            if i == 0 {
                if let Ok(h0) = gpu_download(&x) {
                    te_dump_npy("native_te_l0.npy", &h0, &[1, seq, h]);
                }
            }
        }
        let x = rms(&x, h, "final", &self.final_norm)?;
        let host = gpu_download(&x).map_err(|e| dev_err("ace te download", e))?;
        te_dump_npy("native_text_hidden.npy", &host, &[1, seq, h]);
        Ok(host)
    }
}

fn te_dump_npy(name: &str, data: &[f32], shape: &[usize]) {
    let Ok(dir) = std::env::var("ACE_HOOK_DUMP") else {
        return;
    };
    if dir.is_empty() {
        return;
    }
    let path = std::path::Path::new(&dir).join(name);
    let shape_txt = if shape.len() == 1 {
        format!("({},)", shape[0])
    } else {
        format!(
            "({})",
            shape
                .iter()
                .map(|d| d.to_string())
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    let header = format!(
        "{{'descr': '<f4', 'fortran_order': False, 'shape': {shape_txt}, }}"
    );
    let mut bytes = b"\x93NUMPY\x01\x00".to_vec();
    let mut hdr = header.into_bytes();
    hdr.push(b'\n');
    while (10 + hdr.len()) % 16 != 0 {
        hdr.insert(hdr.len() - 1, b' ');
    }
    let hlen = hdr.len() as u16;
    bytes.extend_from_slice(&hlen.to_le_bytes());
    bytes.extend_from_slice(&hdr);
    for &v in data {
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    let _ = std::fs::write(path, bytes);
}

fn expand_gqa(
    k: GpuTensor,
    v: GpuTensor,
    q_heads: usize,
    kv_heads: usize,
    hd: usize,
    group: usize,
) -> Result<(GpuTensor, GpuTensor)> {
    if group <= 1 {
        return Ok((k, v));
    }
    let mut k_parts = Vec::with_capacity(q_heads);
    let mut v_parts = Vec::with_capacity(q_heads);
    for head in 0..q_heads {
        let kv_head = head / group;
        k_parts.push(
            gpu_slice_cols(&k, kv_head * hd, hd).map_err(|e| dev_err("ace te k expand", e))?,
        );
        v_parts.push(
            gpu_slice_cols(&v, kv_head * hd, hd).map_err(|e| dev_err("ace te v expand", e))?,
        );
    }
    let k_refs: Vec<&GpuTensor> = k_parts.iter().collect();
    let v_refs: Vec<&GpuTensor> = v_parts.iter().collect();
    let k_full = gpu_concat_cols(&k_refs).map_err(|e| dev_err("ace te k concat", e))?;
    let v_full = gpu_concat_cols(&v_refs).map_err(|e| dev_err("ace te v concat", e))?;
    let _ = kv_heads;
    Ok((k_full, v_full))
}
