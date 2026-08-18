//! FLUX.2-dev text encoder over the Comfy fp8 single-file bundle
//! (`mistral_3_small_flux2_fp8.safetensors`): the Mistral-Small-3.2-24B
//! decoder pruned to the 30 layers the [10, 20, 30] hidden-state taps need,
//! tensor names `model.layers.N.*`, projections F8_E4M3 with per-tensor
//! `weight_scale`/`input_scale`, embeddings and norms BF16.
//!
//! This follows the COMFY reference semantics (the official 32GB-card
//! recipe), which differ from the diffusers path [`crate::flux2_text`]
//! targets: the prompt window is NOT padded to 512 tokens — the sequence is
//! exactly `<s>[SYSTEM_PROMPT]…[/SYSTEM_PROMPT][INST]{prompt}[/INST]` and
//! attention is plain causal. The DiT-side 512 window comes from
//! zero-LEFT-padding the conditioning rows (comfy `Flux2.extra_conds`),
//! which the pipeline does, not this encoder.
//!
//! Weight numerics: fp8 projections run through the F8-resident dense
//! linears (`gpu_linear_nt_cached_f8_mm`) — bf16 activations, f32
//! accumulate, per-tensor `weight_scale` applied as the f32 GEMM alpha, bf16
//! D — the same post-accumulate scale structure as the reference's
//! `_scaled_mm`. (The reference additionally quantizes ACTIVATIONS to fp8
//! with `input_scale`; keeping ours bf16 is the finer-precision side of the
//! oracle envelope.) Rotate-half RoPE at theta 1e9 with bf16 tables, RMSNorm
//! eps 1e-5, no qk-norm, GQA 32q/8kv.
//!
//! Every layer's weights are used exactly once per encode, so the cache is
//! evicted per layer right after its forward (streaming, ~1.1GB peak) —
//! encode works with the 25GB-resident dev DiT loaded.
//! `MAKEPAD_FLUX2_TE_RESIDENT=1` keeps layers cached for TE-only runs.

use crate::backend::{
    gpu_add_bf16, gpu_attention_packed_causal_bf16, gpu_bf16_round, gpu_concat_cols,
    gpu_download, gpu_linear_nt_cached_f8_mm, gpu_slice_cols, gpu_swiglu_value_gate, gpu_upload,
    gpu_weight_cache_ensure, gpu_weight_cache_evict_prefix, GpuLinearPart, GpuTensor,
};
use crate::flux2::{Flux2WeightFile, Mistral3TextConfig, FLUX2_HIDDEN_STATE_TAPS};
use crate::flux2_text::{
    flux2_conditioning_concat, mistral3_rope_inv_freq, mistral3_rope_table_values,
};
use crate::{DiffusionError, Result};
use makepad_ggml::quant::GGML_TYPE_F8_E4M3;

pub const FLUX2_DEV_TE_NAMESPACE: &str = "flux2-te-fp8::mistral3";
pub const FLUX2_DEV_EMBED_TOKENS: &str = "model.embed_tokens.weight";

pub fn flux2_dev_layer_prefix(layer: u32) -> String {
    format!("model.layers.{layer}.")
}

pub struct Flux2DevTextPrepared {
    pub config: Mistral3TextConfig,
    input_norm: Vec<Vec<f32>>,
    post_attention_norm: Vec<Vec<f32>>,
    /// Per-layer per-projection weight scales, keyed by tensor name.
    f8_scales: std::collections::HashMap<String, f32>,
    pub rope_inv_freq: Vec<f32>,
}

fn require_vec(weights: &Flux2WeightFile, name: &str, expected: usize) -> Result<Vec<f32>> {
    let values = weights.read_f32(name)?;
    if values.len() != expected {
        return Err(DiffusionError::model(format!(
            "flux2 dev te {name} has {} values, expected {expected}",
            values.len()
        )));
    }
    Ok(values)
}

impl Flux2DevTextPrepared {
    pub fn prepare(weights: &Flux2WeightFile, config: Mistral3TextConfig) -> Result<Self> {
        let layers = config.layers_required_for_taps() as usize;
        let hidden = config.hidden_size as usize;
        let mut input_norm = Vec::with_capacity(layers);
        let mut post_attention_norm = Vec::with_capacity(layers);
        let mut f8_scales = std::collections::HashMap::new();
        for layer in 0..layers {
            let prefix = flux2_dev_layer_prefix(layer as u32);
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
            for proj in [
                "self_attn.q_proj",
                "self_attn.k_proj",
                "self_attn.v_proj",
                "self_attn.o_proj",
                "mlp.gate_proj",
                "mlp.up_proj",
                "mlp.down_proj",
            ] {
                let name = format!("{prefix}{proj}.weight");
                let info = weights.tensor(&name)?;
                if info.dtype != "F8_E4M3" {
                    return Err(DiffusionError::model(format!(
                        "flux2 dev te {name} is {}, expected F8_E4M3",
                        info.dtype
                    )));
                }
                let scale_name = format!("{name}_scale");
                let scale = if weights.has_tensor(&scale_name) {
                    weights.read_f32(&scale_name)?.first().copied().unwrap_or(1.0)
                } else {
                    1.0
                };
                f8_scales.insert(name, scale);
            }
        }
        // Embeddings must be BF16 for the row-streaming reader.
        let embed = weights.tensor(FLUX2_DEV_EMBED_TOKENS)?;
        if embed.dtype != "BF16"
            || embed.shape != [config.vocab_size as u64, config.hidden_size as u64]
        {
            return Err(DiffusionError::model(format!(
                "flux2 dev te embed_tokens is {} {:?}",
                embed.dtype, embed.shape
            )));
        }
        Ok(Self {
            config,
            input_norm,
            post_attention_norm,
            f8_scales,
            rope_inv_freq: mistral3_rope_inv_freq(&config),
        })
    }
}

fn te_resident() -> bool {
    std::env::var("MAKEPAD_FLUX2_TE_RESIDENT").as_deref() == Ok("1")
}

fn ensure_f8_linear<'a>(
    weights: &'a Flux2WeightFile,
    prepared: &Flux2DevTextPrepared,
    name: &'a str,
    output_cols: usize,
    input_cols: usize,
) -> Result<(GpuLinearPart<'a>, f32)> {
    let info = weights.tensor(name)?;
    let expected = [output_cols as u64, input_cols as u64];
    if info.shape != expected {
        return Err(DiffusionError::model(format!(
            "flux2 dev te {name} shape {:?} expected {:?}",
            info.shape, expected
        )));
    }
    gpu_weight_cache_ensure(
        FLUX2_DEV_TE_NAMESPACE,
        name,
        GGML_TYPE_F8_E4M3,
        output_cols,
        input_cols,
        false,
        || weights.read_bytes(name).map_err(|err| err.to_string()),
    )
    .map_err(DiffusionError::model)?;
    let scale = prepared.f8_scales.get(name).copied().unwrap_or(1.0);
    Ok((
        GpuLinearPart {
            bt_ggml_type: GGML_TYPE_F8_E4M3,
            n: output_cols,
            cache_key: name,
            bytes: &[],
        },
        scale,
    ))
}

fn f8_linear(
    weights: &Flux2WeightFile,
    prepared: &Flux2DevTextPrepared,
    input: &GpuTensor,
    name: &str,
    output_cols: usize,
) -> Result<GpuTensor> {
    let (part, scale) = ensure_f8_linear(weights, prepared, name, output_cols, input.cols())?;
    gpu_linear_nt_cached_f8_mm(input, FLUX2_DEV_TE_NAMESPACE, &[part], scale)
        .map_err(DiffusionError::model)
}

fn norm_bf16(
    input: &GpuTensor,
    group_cols: usize,
    cache_key: &str,
    scale: &[f32],
    eps: f32,
) -> Result<GpuTensor> {
    crate::backend::gpu_rms_norm_mul_bf16(
        input,
        group_cols,
        FLUX2_DEV_TE_NAMESPACE,
        cache_key,
        scale,
        eps,
    )
    .map_err(DiffusionError::model)
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

fn embed_rows(
    weights: &Flux2WeightFile,
    ids: &[u32],
    hidden: usize,
    vocab: u32,
) -> Result<Vec<f32>> {
    use std::io::{Read, Seek, SeekFrom};
    let info = weights.tensor(FLUX2_DEV_EMBED_TOKENS)?;
    let header = weights.header_for(FLUX2_DEV_EMBED_TOKENS)?;
    let width = hidden * 2;
    let mut file = std::fs::File::open(&header.path)
        .map_err(|err| DiffusionError::io(&header.path, err.to_string()))?;
    let mut embeds = vec![0.0f32; ids.len() * hidden];
    let mut bytes = vec![0_u8; width];
    for (row, id) in ids.iter().enumerate() {
        if *id >= vocab {
            return Err(DiffusionError::workflow(format!(
                "flux2 dev te token id {id} outside vocab {vocab}"
            )));
        }
        let offset = header.file_offset(info) + (*id as u64) * width as u64;
        file.seek(SeekFrom::Start(offset))
            .map_err(|err| DiffusionError::io(&header.path, err.to_string()))?;
        file.read_exact(&mut bytes)
            .map_err(|err| DiffusionError::io(&header.path, err.to_string()))?;
        for (col, chunk) in bytes.chunks_exact(2).enumerate() {
            let word = u16::from_le_bytes([chunk[0], chunk[1]]);
            embeds[row * hidden + col] = f32::from_bits((word as u32) << 16);
        }
    }
    Ok(embeds)
}

/// Validation taps mirroring the oracle capture points.
#[derive(Clone, Debug, Default)]
pub struct Flux2DevTextTaps {
    pub embed: Vec<f32>,
    /// `(hidden_state_index, (seq, 5120))` for indices 10/20/30.
    pub hidden_states: Vec<(usize, Vec<f32>)>,
}

/// Encode one UNPADDED id window into the `(seq, 15360)` conditioning tensor
/// (feature order `[h10 | h20 | h30]` per row). The pipeline left-pads the
/// result to the DiT's 512-row window with zero rows.
pub fn flux2_dev_text_encode(
    weights: &Flux2WeightFile,
    prepared: &Flux2DevTextPrepared,
    token_ids: &[u32],
    mut on_layer: Option<&mut dyn FnMut(usize, usize)>,
    want_taps: bool,
) -> Result<(Vec<f32>, Option<Flux2DevTextTaps>)> {
    let config = &prepared.config;
    let seq = token_ids.len();
    if seq == 0 {
        return Err(DiffusionError::workflow("flux2 dev te got no token ids"));
    }
    let hidden_size = config.hidden_size as usize;
    let head_dim = config.head_dim as usize;
    let heads = config.num_attention_heads as usize;
    let kv_heads = config.num_key_value_heads as usize;
    let q_width = heads * head_dim;
    let kv_width = kv_heads * head_dim;
    let ffn = config.intermediate_size as usize;
    let scale = (1.0 / (head_dim as f64).sqrt()) as f32;

    let embeds = embed_rows(weights, token_ids, hidden_size, config.vocab_size)?;
    let mut hidden = gpu_upload(&embeds, seq, hidden_size).map_err(DiffusionError::model)?;
    let mut taps = want_taps.then(|| Flux2DevTextTaps {
        embed: embeds,
        ..Flux2DevTextTaps::default()
    });

    let (cos, sin) = mistral3_rope_table_values(&prepared.rope_inv_freq, seq);
    let half = prepared.rope_inv_freq.len();
    let rope_cos = gpu_upload(&cos, seq, half).map_err(DiffusionError::model)?;
    let rope_sin = gpu_upload(&sin, seq, half).map_err(DiffusionError::model)?;

    let layers = config.layers_required_for_taps() as usize;
    let mut tap_hidden: Vec<(usize, Vec<f32>)> = Vec::new();
    for layer in 0..layers {
        if let Some(on_layer) = on_layer.as_deref_mut() {
            on_layer(layer + 1, layers);
        }
        let prefix = flux2_dev_layer_prefix(layer as u32);

        let normed = norm_bf16(
            &hidden,
            hidden_size,
            &format!("{prefix}input_layernorm.weight"),
            &prepared.input_norm[layer],
            config.rms_norm_eps,
        )?;
        let q = f8_linear(
            weights,
            prepared,
            &normed,
            &format!("{prefix}self_attn.q_proj.weight"),
            q_width,
        )?;
        let key = f8_linear(
            weights,
            prepared,
            &normed,
            &format!("{prefix}self_attn.k_proj.weight"),
            kv_width,
        )?;
        let value = f8_linear(
            weights,
            prepared,
            &normed,
            &format!("{prefix}self_attn.v_proj.weight"),
            kv_width,
        )?;
        // No qk-norm in Mistral3 — straight to rotate-half rope.
        let q = crate::backend::gpu_rope_half_bf16(&q, heads, half, &rope_cos, &rope_sin)
            .map_err(DiffusionError::model)?;
        let key = crate::backend::gpu_rope_half_bf16(&key, kv_heads, half, &rope_cos, &rope_sin)
            .map_err(DiffusionError::model)?;
        let (key_full, value_full) = repeat_kv(&key, &value, heads, kv_heads, head_dim)?;
        let attention =
            gpu_attention_packed_causal_bf16(&q, &key_full, &value_full, heads, scale)
                .map_err(DiffusionError::model)?;
        let attention = gpu_bf16_round(&attention).map_err(DiffusionError::model)?;
        let attention = f8_linear(
            weights,
            prepared,
            &attention,
            &format!("{prefix}self_attn.o_proj.weight"),
            hidden_size,
        )?;
        let residual = gpu_add_bf16(&hidden, &attention).map_err(DiffusionError::model)?;

        let normed = norm_bf16(
            &residual,
            hidden_size,
            &format!("{prefix}post_attention_layernorm.weight"),
            &prepared.post_attention_norm[layer],
            config.rms_norm_eps,
        )?;
        // gpu_swiglu_value_gate takes [up | gate].
        let up = f8_linear(
            weights,
            prepared,
            &normed,
            &format!("{prefix}mlp.up_proj.weight"),
            ffn,
        )?;
        let gate = f8_linear(
            weights,
            prepared,
            &normed,
            &format!("{prefix}mlp.gate_proj.weight"),
            ffn,
        )?;
        let up_gate = crate::backend::gpu_concat_cols(&[&up, &gate]).map_err(DiffusionError::model)?;
        let activated = gpu_swiglu_value_gate(&up_gate).map_err(DiffusionError::model)?;
        let activated = gpu_bf16_round(&activated).map_err(DiffusionError::model)?;
        let update = f8_linear(
            weights,
            prepared,
            &activated,
            &format!("{prefix}mlp.down_proj.weight"),
            hidden_size,
        )?;
        hidden = gpu_add_bf16(&residual, &update).map_err(DiffusionError::model)?;

        let hidden_state_index = layer + 1;
        if FLUX2_HIDDEN_STATE_TAPS.contains(&hidden_state_index) || want_taps {
            tap_hidden.push((
                hidden_state_index,
                gpu_download(&hidden).map_err(DiffusionError::model)?,
            ));
        }
        // Layer weights are single-use per encode: evict immediately so the
        // encoder streams beside the resident DiT.
        if !te_resident() {
            let _ = gpu_weight_cache_evict_prefix(&format!(
                "{FLUX2_DEV_TE_NAMESPACE}::{prefix}"
            ));
        }
    }

    let tap_slices: Vec<&[f32]> = tap_hidden
        .iter()
        .filter(|(index, _)| FLUX2_HIDDEN_STATE_TAPS.contains(index))
        .map(|(_, v)| v.as_slice())
        .collect();
    let conditioning = flux2_conditioning_concat(&tap_slices, seq, hidden_size);
    if let Some(taps) = taps.as_mut() {
        taps.hidden_states = tap_hidden;
    }
    Ok((conditioning, taps))
}

/// Evict everything this encoder cached (norm vectors etc.).
pub fn flux2_dev_text_release() -> Result<usize> {
    gpu_weight_cache_evict_prefix(&format!("{FLUX2_DEV_TE_NAMESPACE}::"))
        .map_err(DiffusionError::model)
}
