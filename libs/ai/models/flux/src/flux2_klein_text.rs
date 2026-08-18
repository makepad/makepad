//! FLUX.2-klein-4B text encoder: stock Qwen3-4B, hidden-state taps [9,18,27].
//!
//! Prompt format (BFL `Qwen3Embedder.forward`): one user message, chat
//! template with `add_generation_prompt=True` and `enable_thinking=False`,
//! then pad/truncate to 512 with `<|endoftext|>` (id 151643).
//!
//! Operator family matches [`crate::flux2_text`]: BF16-operand / f32-accumulate
//! GEMMs, explicit BF16 round at module outputs, qk-norm (Qwen3), rotate-half
//! RoPE at theta 1e6.

use crate::backend::{
    gpu_add_bf16, gpu_attention_packed_causal_bf16, gpu_attention_packed_cross_bf16,
    gpu_bf16_round, gpu_concat_cols, gpu_concat_rows, gpu_download, gpu_linear_nt_cached_bf16_f32acc,
    gpu_rms_norm_mul_bf16, gpu_rope_half_bf16, gpu_slice_cols, gpu_slice_rows, gpu_swiglu_value_gate,
    gpu_upload, gpu_weight_cache_ensure, gpu_weight_cache_evict_prefix, GpuLinearPart, GpuTensor,
};
use crate::flux2::{
    Flux2WeightFile, Qwen3TextConfig, FLUX2_KLEIN_HIDDEN_STATE_TAPS, FLUX2_KLEIN_PAD_TOKEN,
};
use crate::flux2_tokenizer::FLUX2_MAX_SEQUENCE_LENGTH;
use makepad_ai_h3::h3_tokenizer::H3Tokenizer;
use crate::{DiffusionError, Result};
use makepad_ggml::quant::GGML_TYPE_BF16;
use std::path::Path;

pub const FLUX2_KLEIN_TE_NAMESPACE: &str = "flux2-te-bf16::qwen3-4b";
pub const FLUX2_KLEIN_EMBED_TOKENS: &str = "model.embed_tokens.weight";

pub fn flux2_klein_layer_prefix(layer: u32) -> String {
    format!("model.layers.{layer}.")
}

/// Render the Klein Qwen3 chat-template string (user + generation prompt,
/// thinking disabled).
pub fn render_flux2_klein_prompt(prompt: &str) -> String {
    format!(
        "<|im_start|>user\n{prompt}<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n"
    )
}

#[derive(Clone, Debug)]
pub struct Flux2KleinTokenizedPrompt {
    pub token_ids: Vec<u32>,
    pub real_len: usize,
}

pub fn flux2_klein_tokenize(
    tokenizer: &H3Tokenizer,
    prompt: &str,
) -> Result<Flux2KleinTokenizedPrompt> {
    let rendered = render_flux2_klein_prompt(prompt);
    let ids = tokenizer.encode(&rendered);
    let mut token_ids = ids;
    if token_ids.len() > FLUX2_MAX_SEQUENCE_LENGTH {
        token_ids.truncate(FLUX2_MAX_SEQUENCE_LENGTH);
    }
    let real_len = token_ids.len();
    if real_len == 0 {
        return Err(DiffusionError::workflow("flux2 klein te produced empty ids"));
    }
    token_ids.resize(FLUX2_MAX_SEQUENCE_LENGTH, FLUX2_KLEIN_PAD_TOKEN);
    Ok(Flux2KleinTokenizedPrompt {
        token_ids,
        real_len,
    })
}

pub struct Flux2KleinTextPrepared {
    pub config: Qwen3TextConfig,
    input_norm: Vec<Vec<f32>>,
    post_attention_norm: Vec<Vec<f32>>,
    q_norm: Vec<Vec<f32>>,
    k_norm: Vec<Vec<f32>>,
    pub rope_inv_freq: Vec<f32>,
}

impl Flux2KleinTextPrepared {
    pub fn prepare(weights: &Flux2WeightFile, config: Qwen3TextConfig) -> Result<Self> {
        let layers = config.layers_required_for_taps() as usize;
        let hidden = config.hidden_size as usize;
        let head_dim = config.head_dim as usize;
        let mut input_norm = Vec::with_capacity(layers);
        let mut post_attention_norm = Vec::with_capacity(layers);
        let mut q_norm = Vec::with_capacity(layers);
        let mut k_norm = Vec::with_capacity(layers);
        for layer in 0..layers {
            let prefix = flux2_klein_layer_prefix(layer as u32);
            input_norm.push(require_vec(
                weights,
                &format!("{prefix}input_layernorm.weight"),
                hidden,
            )?);
            post_attention_norm.push(require_vec(
                weights,
                &format!("{prefix}post_attention_layernorm.weight"),
                hidden,
            )?);
            q_norm.push(require_vec(
                weights,
                &format!("{prefix}self_attn.q_norm.weight"),
                head_dim,
            )?);
            k_norm.push(require_vec(
                weights,
                &format!("{prefix}self_attn.k_norm.weight"),
                head_dim,
            )?);
        }
        Ok(Self {
            config,
            input_norm,
            post_attention_norm,
            q_norm,
            k_norm,
            rope_inv_freq: qwen3_rope_inv_freq(&config),
        })
    }
}

fn require_vec(weights: &Flux2WeightFile, name: &str, expected: usize) -> Result<Vec<f32>> {
    let values = weights.read_f32(name)?;
    if values.len() != expected {
        return Err(DiffusionError::model(format!(
            "flux2 klein te {name} has {} values, expected {expected}",
            values.len()
        )));
    }
    Ok(values)
}

pub(crate) fn qwen3_rope_inv_freq(config: &Qwen3TextConfig) -> Vec<f32> {
    let head_dim = config.head_dim as usize;
    let theta = config.rope_theta as f32;
    (0..head_dim / 2)
        .map(|index| 1.0 / theta.powf(2.0 * index as f32 / head_dim as f32))
        .collect()
}

fn rope_tables(inv_freq: &[f32], seq: usize) -> (Vec<f32>, Vec<f32>) {
    let half = inv_freq.len();
    let mut cos = vec![0.0f32; seq * half];
    let mut sin = vec![0.0f32; seq * half];
    for pos in 0..seq {
        for (pair, freq) in inv_freq.iter().enumerate() {
            let angle = pos as f32 * *freq;
            cos[pos * half + pair] = angle.cos();
            sin[pos * half + pair] = angle.sin();
        }
    }
    // HF `cos.to(x.dtype)` / `sin.to(x.dtype)`: BF16 tables.
    for value in cos.iter_mut().chain(sin.iter_mut()) {
        *value = round_bf16(*value);
    }
    (cos, sin)
}

fn round_bf16(value: f32) -> f32 {
    let bits = value.to_bits();
    let rounding = (bits >> 16) & 1;
    let word = ((bits + 0x7fff + rounding) >> 16) as u16;
    f32::from_bits((word as u32) << 16)
}

fn embed_name(weights: &Flux2WeightFile) -> Result<&'static str> {
    if weights.has_tensor(FLUX2_KLEIN_EMBED_TOKENS) {
        Ok(FLUX2_KLEIN_EMBED_TOKENS)
    } else if weights.has_tensor("embed_tokens.weight") {
        Ok("embed_tokens.weight")
    } else {
        Err(DiffusionError::model(
            "flux2 klein te missing model.embed_tokens.weight",
        ))
    }
}

fn ensure_linear<'a>(
    weights: &'a Flux2WeightFile,
    name: &'a str,
    output_cols: usize,
    input_cols: usize,
) -> Result<GpuLinearPart<'a>> {
    let info = weights.tensor(name)?;
    let expected = [output_cols as u64, input_cols as u64];
    if info.shape != expected {
        return Err(DiffusionError::model(format!(
            "flux2 klein te {name} shape {:?} expected {:?}",
            info.shape, expected
        )));
    }
    gpu_weight_cache_ensure(
        FLUX2_KLEIN_TE_NAMESPACE,
        name,
        GGML_TYPE_BF16,
        output_cols,
        input_cols,
        false,
        || weights.read_bf16_bytes(name).map_err(|err| err.to_string()),
    )
    .map_err(DiffusionError::model)?;
    Ok(GpuLinearPart {
        bt_ggml_type: GGML_TYPE_BF16,
        n: output_cols,
        cache_key: name,
        bytes: &[],
    })
}

fn linear_bf16(
    weights: &Flux2WeightFile,
    input: &GpuTensor,
    name: &str,
    output_cols: usize,
) -> Result<GpuTensor> {
    let part = ensure_linear(weights, name, output_cols, input.cols())?;
    let output = gpu_linear_nt_cached_bf16_f32acc(input, FLUX2_KLEIN_TE_NAMESPACE, &[part], &[])
        .map_err(DiffusionError::model)?;
    gpu_bf16_round(&output).map_err(DiffusionError::model)
}

fn repeat_kv(
    key: &GpuTensor,
    value: &GpuTensor,
    heads: usize,
    kv_heads: usize,
    head_dim: usize,
) -> Result<(GpuTensor, GpuTensor)> {
    let group = heads / kv_heads;
    let mut key_heads = Vec::with_capacity(heads);
    let mut value_heads = Vec::with_capacity(heads);
    for q_head in 0..heads {
        let kv_head = q_head / group;
        key_heads.push(
            gpu_slice_cols(key, kv_head * head_dim, head_dim).map_err(DiffusionError::model)?,
        );
        value_heads.push(
            gpu_slice_cols(value, kv_head * head_dim, head_dim).map_err(DiffusionError::model)?,
        );
    }
    let key_refs: Vec<&GpuTensor> = key_heads.iter().collect();
    let value_refs: Vec<&GpuTensor> = value_heads.iter().collect();
    Ok((
        gpu_concat_cols(&key_refs).map_err(DiffusionError::model)?,
        gpu_concat_cols(&value_refs).map_err(DiffusionError::model)?,
    ))
}

fn masked_causal(
    q: &GpuTensor,
    k: &GpuTensor,
    v: &GpuTensor,
    head_count: usize,
    scale: f32,
    real_len: usize,
) -> Result<GpuTensor> {
    let seq = q.rows();
    if real_len >= seq {
        return gpu_attention_packed_causal_bf16(q, k, v, head_count, scale)
            .map_err(DiffusionError::model);
    }
    // HF causal + padding mask (right-pad): real tokens attend only to earlier
    // real tokens; pad queries attend to the real prefix and cannot attend to
    // other pads.
    let q_real = gpu_slice_rows(q, 0, real_len).map_err(DiffusionError::model)?;
    let k_real = gpu_slice_rows(k, 0, real_len).map_err(DiffusionError::model)?;
    let v_real = gpu_slice_rows(v, 0, real_len).map_err(DiffusionError::model)?;
    let real = gpu_attention_packed_causal_bf16(&q_real, &k_real, &v_real, head_count, scale)
        .map_err(DiffusionError::model)?;
    let q_pad = gpu_slice_rows(q, real_len, seq - real_len).map_err(DiffusionError::model)?;
    let pad = gpu_attention_packed_cross_bf16(&q_pad, &k_real, &v_real, head_count, scale)
        .map_err(DiffusionError::model)?;
    gpu_concat_rows(&real, &pad).map_err(DiffusionError::model)
}

fn embed_rows(
    weights: &Flux2WeightFile,
    name: &str,
    ids: &[u32],
    hidden: usize,
    vocab: u32,
) -> Result<Vec<f32>> {
    let info = weights.tensor(name)?;
    let header = weights.header_for(name)?;
    let mut embeds = vec![0.0f32; ids.len() * hidden];
    for (row, id) in ids.iter().enumerate() {
        if *id >= vocab {
            return Err(DiffusionError::workflow(format!(
                "flux2 klein te token id {id} outside vocab {vocab}"
            )));
        }
        // Stream one BF16 row without loading the 151936 x 2560 matrix.
        let row_bytes = {
            use std::io::{Read, Seek, SeekFrom};
            let width = hidden * 2;
            let offset = header.file_offset(info) + (*id as u64) * width as u64;
            let mut file = std::fs::File::open(&header.path)
                .map_err(|err| DiffusionError::io(&header.path, err.to_string()))?;
            file.seek(SeekFrom::Start(offset))
                .map_err(|err| DiffusionError::io(&header.path, err.to_string()))?;
            let mut bytes = vec![0_u8; width];
            file.read_exact(&mut bytes)
                .map_err(|err| DiffusionError::io(&header.path, err.to_string()))?;
            bytes
        };
        for col in 0..hidden {
            let word = u16::from_le_bytes([row_bytes[col * 2], row_bytes[col * 2 + 1]]);
            embeds[row * hidden + col] = f32::from_bits((word as u32) << 16);
        }
    }
    Ok(embeds)
}

pub fn flux2_klein_text_encode(
    weights: &Flux2WeightFile,
    prepared: &Flux2KleinTextPrepared,
    prompt: &Flux2KleinTokenizedPrompt,
) -> Result<Vec<f32>> {
    let config = &prepared.config;
    let seq = prompt.token_ids.len();
    let hidden = config.hidden_size as usize;
    if seq != FLUX2_MAX_SEQUENCE_LENGTH {
        return Err(DiffusionError::workflow(format!(
            "flux2 klein te expects {FLUX2_MAX_SEQUENCE_LENGTH} ids, got {seq}"
        )));
    }
    let embed = embed_name(weights)?;
    let embeds = embed_rows(weights, embed, &prompt.token_ids, hidden, config.vocab_size)?;
    let mut hidden_t = gpu_upload(&embeds, seq, hidden).map_err(DiffusionError::model)?;
    let (cos, sin) = rope_tables(&prepared.rope_inv_freq, seq);
    let half = prepared.rope_inv_freq.len();
    let rope_cos = gpu_upload(&cos, seq, half).map_err(DiffusionError::model)?;
    let rope_sin = gpu_upload(&sin, seq, half).map_err(DiffusionError::model)?;

    let layers = config.layers_required_for_taps() as usize;
    let mut taps: Vec<(usize, Vec<f32>)> = Vec::new();
    for layer in 0..layers {
        hidden_t = klein_layer(
            weights,
            prepared,
            layer,
            hidden_t,
            &rope_cos,
            &rope_sin,
            prompt.real_len,
        )?;
        let hidden_state_index = layer + 1;
        if FLUX2_KLEIN_HIDDEN_STATE_TAPS.contains(&hidden_state_index) {
            taps.push((
                hidden_state_index,
                gpu_download(&hidden_t).map_err(DiffusionError::model)?,
            ));
        }
    }
    taps.sort_by_key(|(index, _)| *index);
    let mut conditioning = vec![0.0f32; seq * hidden * 3];
    for (tap_i, (_, values)) in taps.iter().enumerate() {
        for token in 0..seq {
            let dest = token * hidden * 3 + tap_i * hidden;
            let src = token * hidden;
            conditioning[dest..dest + hidden].copy_from_slice(&values[src..src + hidden]);
        }
    }
    Ok(conditioning)
}

fn klein_layer(
    weights: &Flux2WeightFile,
    prepared: &Flux2KleinTextPrepared,
    layer: usize,
    input: GpuTensor,
    rope_cos: &GpuTensor,
    rope_sin: &GpuTensor,
    real_len: usize,
) -> Result<GpuTensor> {
    let config = &prepared.config;
    let hidden = config.hidden_size as usize;
    let heads = config.num_attention_heads as usize;
    let kv_heads = config.num_key_value_heads as usize;
    let head_dim = config.head_dim as usize;
    let ffn = config.intermediate_size as usize;
    let prefix = flux2_klein_layer_prefix(layer as u32);
    let scale = (head_dim as f64).sqrt().recip() as f32;

    let normed = gpu_rms_norm_mul_bf16(
        &input,
        hidden,
        FLUX2_KLEIN_TE_NAMESPACE,
        &format!("{prefix}input_layernorm.weight"),
        &prepared.input_norm[layer],
        config.rms_norm_eps,
    )
    .map_err(DiffusionError::model)?;
    let q = linear_bf16(
        weights,
        &normed,
        &format!("{prefix}self_attn.q_proj.weight"),
        heads * head_dim,
    )?;
    let k = linear_bf16(
        weights,
        &normed,
        &format!("{prefix}self_attn.k_proj.weight"),
        kv_heads * head_dim,
    )?;
    let v = linear_bf16(
        weights,
        &normed,
        &format!("{prefix}self_attn.v_proj.weight"),
        kv_heads * head_dim,
    )?;
    let q = gpu_rms_norm_mul_bf16(
        &q,
        head_dim,
        FLUX2_KLEIN_TE_NAMESPACE,
        &format!("{prefix}self_attn.q_norm.weight"),
        &prepared.q_norm[layer],
        config.rms_norm_eps,
    )
    .map_err(DiffusionError::model)?;
    let k = gpu_rms_norm_mul_bf16(
        &k,
        head_dim,
        FLUX2_KLEIN_TE_NAMESPACE,
        &format!("{prefix}self_attn.k_norm.weight"),
        &prepared.k_norm[layer],
        config.rms_norm_eps,
    )
    .map_err(DiffusionError::model)?;
    let q = gpu_rope_half_bf16(&q, heads, head_dim / 2, rope_cos, rope_sin)
        .map_err(DiffusionError::model)?;
    let k = gpu_rope_half_bf16(&k, kv_heads, head_dim / 2, rope_cos, rope_sin)
        .map_err(DiffusionError::model)?;
    let (k, v) = repeat_kv(&k, &v, heads, kv_heads, head_dim)?;
    let attn = masked_causal(&q, &k, &v, heads, scale, real_len)?;
    let attn = linear_bf16(
        weights,
        &attn,
        &format!("{prefix}self_attn.o_proj.weight"),
        hidden,
    )?;
    let residual = gpu_add_bf16(&input, &attn).map_err(DiffusionError::model)?;

    let normed = gpu_rms_norm_mul_bf16(
        &residual,
        hidden,
        FLUX2_KLEIN_TE_NAMESPACE,
        &format!("{prefix}post_attention_layernorm.weight"),
        &prepared.post_attention_norm[layer],
        config.rms_norm_eps,
    )
    .map_err(DiffusionError::model)?;
    let gate = linear_bf16(
        weights,
        &normed,
        &format!("{prefix}mlp.gate_proj.weight"),
        ffn,
    )?;
    let up = linear_bf16(weights, &normed, &format!("{prefix}mlp.up_proj.weight"), ffn)?;
    // TE fused order [up, gate] so value=left, gate=right.
    let fused = gpu_concat_cols(&[&up, &gate]).map_err(DiffusionError::model)?;
    let swiglu = gpu_swiglu_value_gate(&fused).map_err(DiffusionError::model)?;
    let swiglu = gpu_bf16_round(&swiglu).map_err(DiffusionError::model)?;
    let down = linear_bf16(
        weights,
        &swiglu,
        &format!("{prefix}mlp.down_proj.weight"),
        hidden,
    )?;
    gpu_add_bf16(&residual, &down).map_err(DiffusionError::model)
}

pub fn flux2_klein_text_release() -> Result<usize> {
    gpu_weight_cache_evict_prefix(&format!("{FLUX2_KLEIN_TE_NAMESPACE}::"))
        .map_err(DiffusionError::model)
}

pub fn flux2_klein_tokenizer_load(dir: impl AsRef<Path>) -> Result<H3Tokenizer> {
    H3Tokenizer::load(dir.as_ref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn klein_template_thinking_disabled() {
        let rendered = render_flux2_klein_prompt("make it night");
        assert!(rendered.starts_with("<|im_start|>user\nmake it night<|im_end|>\n"));
        assert!(rendered.contains("<|im_start|>assistant\n<think>\n\n</think>\n\n"));
    }

    #[test]
    fn klein_tokenizer_loads_when_present() {
        let dir = std::path::PathBuf::from("/tmp/flux2-klein-tok");
        if !dir.join("tokenizer.json").is_file() {
            eprintln!("klein tokenizer missing, skip");
            return;
        }
        let tok = H3Tokenizer::load(&dir).expect("load qwen3 tokenizer");
        let prompt = flux2_klein_tokenize(&tok, "change the sky to sunset orange").expect("tok");
        assert_eq!(prompt.token_ids.len(), 512);
        assert!(prompt.real_len > 8 && prompt.real_len < 64);
        assert_eq!(prompt.token_ids[prompt.real_len], FLUX2_KLEIN_PAD_TOKEN);
    }
}
