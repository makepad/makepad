//! FLUX.2 text encoder: the Mistral-Small-3.2-24B-Instruct text decoder,
//! executed with PyTorch-BF16-exact operator boundaries.
//!
//! FLUX.2-dev conditions its DiT on features of the *frozen* Mistral3 text
//! model (the Pixtral vision tower is unused for text-to-image): the
//! reference pipeline runs the decoder over the 512-token padded
//! chat-template window with `output_hidden_states=True` and concatenates
//! hidden states 10/20/30 — the raw outputs of decoder layers 9/19/29, no
//! final norm — into one `(512, 15360)` conditioning tensor. Only layers
//! 0..30 of the 40-layer decoder ever execute.
//!
//! Operator contract: the frozen reference runs in `torch_dtype=bfloat16`,
//! so this port uses the same BF16-boundary operator family that made the
//! SkinTokens Qwen3 decoder reference-exact: BF16-operand/f32-accumulate
//! GEMMs with an explicit BF16 round at each module output
//! (`gpu_linear_nt_cached_bf16_f32acc` + `gpu_bf16_round`),
//! `gpu_rms_norm_mul_bf16`, `gpu_rope_half_bf16`, and the
//! `gpu_attention_packed_{causal,cross}_bf16` pair.
//!
//! Padding: the window is right-padded (`padding="max_length"`, 512 ids) and
//! HF's 4D mask only adds pad KEY columns on top of the causal triangle.
//! With right padding that decomposes exactly, no mask kernel needed:
//! - real query rows (positions `0..real_len`) see keys `<= pos`, which are
//!   all real -> plain causal attention over the leading real rows;
//! - pad query rows (positions `>= real_len`) see keys `<= pos` minus the
//!   pad columns = exactly the full real prefix -> cross attention onto the
//!   real K/V.
//! Pad rows are NOT discarded: their hidden states flow into the
//! conditioning tensor the DiT attends over, so they must match the
//! reference too. Position ids are `arange(512)` pads included — the
//! pipeline passes no `position_ids` and `Mistral3Model.forward` defaults to
//! `cache_position = arange(seq_len)`.
//!
//! Weights stay resident in the [`FLUX2_TE_NAMESPACE`] device cache between
//! prompts (~33.4GB for the 30 executed layers in BF16); call
//! [`flux2_text_release`] before loading the DiT. The `(131072, 5120)`
//! embedding matrix is never uploaded — rows stream from the shard file per
//! token like the H3 encoder does.

use crate::backend::{
    gpu_add_bf16, gpu_attention_packed_causal_bf16, gpu_attention_packed_cross_bf16,
    gpu_bf16_round, gpu_concat_cols, gpu_concat_rows, gpu_download,
    gpu_linear_nt_cached_bf16_f32acc, gpu_rms_norm_mul_bf16, gpu_rope_half_bf16, gpu_slice_cols,
    gpu_slice_rows, gpu_swiglu_value_gate, gpu_upload, gpu_weight_cache_ensure,
    gpu_weight_cache_evict_prefix, GpuLinearPart, GpuTensor,
};
use crate::flux2::{
    mistral3_layer_prefix, Flux2TextEncoderWeights, Mistral3TextConfig, FLUX2_HIDDEN_STATE_TAPS,
    MISTRAL3_EMBED_TOKENS,
};
use crate::flux2_tokenizer::{Flux2TokenizedPrompt, FLUX2_MAX_SEQUENCE_LENGTH};
use crate::{DiffusionError, Result};
use makepad_ggml::quant::GGML_TYPE_BF16;
use makepad_mlx::MlxDType;

/// Device weight-cache namespace shared by every FLUX.2 TE prompt; evicted as
/// one prefix by [`flux2_text_release`].
pub const FLUX2_TE_NAMESPACE: &str = "flux2-te-bf16::mistral3";

/// Small host-resident per-layer vectors plus the RoPE frequencies. The
/// large projection matrices remain on disk until their layer first
/// executes, then live in the device weight cache.
pub struct Flux2TextEncoderPrepared {
    pub config: Mistral3TextConfig,
    /// `input_layernorm.weight` for layers `0..30`, decoded to f32.
    input_norm: Vec<Vec<f32>>,
    /// `post_attention_layernorm.weight` for layers `0..30`.
    post_attention_norm: Vec<Vec<f32>>,
    /// Full-f32 inverse frequencies (see [`mistral3_rope_inv_freq`]);
    /// public so the parity validator can gate it against the oracle buffer.
    pub rope_inv_freq: Vec<f32>,
}

impl Flux2TextEncoderPrepared {
    pub fn prepare(weights: &Flux2TextEncoderWeights, config: Mistral3TextConfig) -> Result<Self> {
        weights.verify_shapes(&config)?;
        let layers = config.layers_required_for_taps() as usize;
        let hidden_size = config.hidden_size as usize;
        let mut input_norm = Vec::with_capacity(layers);
        let mut post_attention_norm = Vec::with_capacity(layers);
        for layer in 0..layers {
            let prefix = mistral3_layer_prefix(layer as u32);
            for (name, into) in [
                ("input_layernorm.weight", &mut input_norm),
                ("post_attention_layernorm.weight", &mut post_attention_norm),
            ] {
                let values = weights.shards.tensor_f32(&format!("{prefix}{name}"))?;
                if values.len() != hidden_size {
                    return Err(DiffusionError::model(format!(
                        "flux2 te layer {layer} {name} has {} values, expected {hidden_size}",
                        values.len()
                    )));
                }
                into.push(values);
            }
        }
        let rope_inv_freq = mistral3_rope_inv_freq(&config);
        Ok(Self {
            config,
            input_norm,
            post_attention_norm,
            rope_inv_freq,
        })
    }
}

/// HF `default_rope_init` computes `inv_freq = 1 / theta^(arange(0,hd,2)/hd)`
/// in f32 (the explicit `.float()` holds even under a BF16 default dtype),
/// and `from_pretrained(torch_dtype=bfloat16)` leaves the non-persistent
/// buffer f32 — unlike the SkinTokens reference, whose `model.to(bf16)` cast
/// it. So: full-f32 frequencies here; only the cos/sin *tables* get the BF16
/// round (`cos.to(x.dtype)` in the rotary forward). The oracle dump captures
/// the reference tables directly, so a wrong call on this shows up isolated.
pub(crate) fn mistral3_rope_inv_freq(config: &Mistral3TextConfig) -> Vec<f32> {
    let head_dim = config.head_dim as usize;
    let theta = config.rope_theta as f32;
    (0..head_dim / 2)
        .map(|index| 1.0 / theta.powf(2.0 * index as f32 / head_dim as f32))
        .collect()
}

/// Rotate-half tables for positions `0..rows`: f32 angle (f32 position x f32
/// frequency, matching HF's f32 outer product), f32 cos/sin, then one BF16
/// round matching the `cos.to(torch.bfloat16)` cast before rotation.
pub(crate) fn mistral3_rope_table_values(inv_freq: &[f32], rows: usize) -> (Vec<f32>, Vec<f32>) {
    let half = inv_freq.len();
    let mut cos = vec![0.0f32; rows * half];
    let mut sin = vec![0.0f32; rows * half];
    for position in 0..rows {
        for index in 0..half {
            let angle = position as f32 * inv_freq[index];
            cos[position * half + index] = round_to_bf16(angle.cos());
            sin[position * half + index] = round_to_bf16(angle.sin());
        }
    }
    (cos, sin)
}

pub fn round_to_bf16(value: f32) -> f32 {
    let bits = value.to_bits();
    let rounding_bias = 0x7fffu32 + ((bits >> 16) & 1);
    f32::from_bits(bits.wrapping_add(rounding_bias) & 0xffff_0000)
}

fn require_bf16_shape(
    weights: &Flux2TextEncoderWeights,
    name: &str,
    expected: &[u64],
) -> Result<()> {
    let (dtype, shape) = weights.shards.tensor_dtype_shape(name)?;
    if dtype != MlxDType::BF16 || shape != expected {
        return Err(DiffusionError::model(format!(
            "flux2 te tensor '{name}' is {dtype:?} {shape:?}, expected BF16 {expected:?}",
        )));
    }
    Ok(())
}

fn ensure_linear<'a>(
    weights: &Flux2TextEncoderWeights,
    name: &'a str,
    output_cols: usize,
    input_cols: usize,
) -> Result<GpuLinearPart<'a>> {
    require_bf16_shape(weights, name, &[output_cols as u64, input_cols as u64])?;
    gpu_weight_cache_ensure(
        FLUX2_TE_NAMESPACE,
        name,
        GGML_TYPE_BF16,
        output_cols,
        input_cols,
        false,
        || {
            weights
                .shards
                .tensor_bytes(name)
                .map_err(|error| error.to_string())
        },
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
    weights: &Flux2TextEncoderWeights,
    input: &GpuTensor,
    name: &str,
    output_cols: usize,
) -> Result<GpuTensor> {
    let part = ensure_linear(weights, name, output_cols, input.cols())?;
    let output = gpu_linear_nt_cached_bf16_f32acc(input, FLUX2_TE_NAMESPACE, &[part], &[])
        .map_err(DiffusionError::model)?;
    gpu_bf16_round(&output).map_err(DiffusionError::model)
}

fn norm_bf16(
    input: &GpuTensor,
    group_cols: usize,
    cache_key: &str,
    scale: &[f32],
    eps: f32,
) -> Result<GpuTensor> {
    gpu_rms_norm_mul_bf16(input, group_cols, FLUX2_TE_NAMESPACE, cache_key, scale, eps)
        .map_err(DiffusionError::model)
}

/// Expand grouped K/V to one column block per query head (`q_head / group`
/// selects the kv head, exactly HF `repeat_kv`'s interleave order).
fn repeat_kv_heads(
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

/// Causal attention under right-padding (see the module doc for why the
/// masked softmax splits exactly into causal-over-real + pad-onto-real
/// cross). K/V must already be head-expanded to `q`'s width.
fn masked_causal_attention_bf16(
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

/// Layer-0 operator boundaries for the parity validator, flattened token-major
/// exactly like HF module hooks. Isolates projection vs rope vs attention vs
/// mlp drift without conflating them across 30 layers.
#[derive(Clone, Debug, Default)]
pub struct Flux2TextLayer0Taps {
    pub input_norm: Vec<f32>,
    pub query_projected: Vec<f32>,
    pub key_projected: Vec<f32>,
    pub value_projected: Vec<f32>,
    pub query_rope: Vec<f32>,
    pub key_rope: Vec<f32>,
    pub attention_raw: Vec<f32>,
    pub attention_projected: Vec<f32>,
    pub attention_residual: Vec<f32>,
    pub post_attention_norm: Vec<f32>,
    /// Fused `[up | gate]` projection output, `(512, 2*32768)`.
    pub up_gate: Vec<f32>,
    pub activated: Vec<f32>,
    pub down_projected: Vec<f32>,
    pub hidden: Vec<f32>,
}

/// Validation-only capture of every boundary the oracle dump also records.
#[derive(Clone, Debug, Default)]
pub struct Flux2TextTaps {
    /// `inputs_embeds` = hidden state 0, `(512, 5120)`.
    pub embed: Vec<f32>,
    /// BF16-rounded rotate-half tables, `(512, 64)` each.
    pub rope_cos: Vec<f32>,
    pub rope_sin: Vec<f32>,
    pub layer0: Flux2TextLayer0Taps,
    /// `(hidden_state_index, (512, 5120) values)` for indices 10/20/30.
    pub hidden_states: Vec<(usize, Vec<f32>)>,
}

/// Encode one padded prompt window into the `(512, 15360)` row-major
/// conditioning tensor (feature order `[h10 | h20 | h30]` per row).
pub fn flux2_text_encode(
    weights: &Flux2TextEncoderWeights,
    prepared: &Flux2TextEncoderPrepared,
    prompt: &Flux2TokenizedPrompt,
) -> Result<Vec<f32>> {
    flux2_text_encode_progress(weights, prepared, prompt, None)
}

/// [`flux2_text_encode`] with a per-layer `(done, total)` progress callback —
/// the first prompt streams ~33GB of weights from disk, so this is what makes
/// the TE phase visibly move.
pub fn flux2_text_encode_progress(
    weights: &Flux2TextEncoderWeights,
    prepared: &Flux2TextEncoderPrepared,
    prompt: &Flux2TokenizedPrompt,
    on_layer: Option<&mut dyn FnMut(usize, usize)>,
) -> Result<Vec<f32>> {
    Ok(flux2_text_encode_inner(weights, prepared, prompt, on_layer, false)?.0)
}

/// [`flux2_text_encode`] plus every validator boundary.
pub fn flux2_text_encode_tapped(
    weights: &Flux2TextEncoderWeights,
    prepared: &Flux2TextEncoderPrepared,
    prompt: &Flux2TokenizedPrompt,
) -> Result<(Vec<f32>, Flux2TextTaps)> {
    let (conditioning, taps) = flux2_text_encode_inner(weights, prepared, prompt, None, true)?;
    Ok((conditioning, taps.expect("taps requested")))
}

fn flux2_text_encode_inner(
    weights: &Flux2TextEncoderWeights,
    prepared: &Flux2TextEncoderPrepared,
    prompt: &Flux2TokenizedPrompt,
    mut on_layer: Option<&mut dyn FnMut(usize, usize)>,
    want_taps: bool,
) -> Result<(Vec<f32>, Option<Flux2TextTaps>)> {
    let config = &prepared.config;
    let seq = prompt.token_ids.len();
    let hidden_size = config.hidden_size as usize;
    if seq != FLUX2_MAX_SEQUENCE_LENGTH {
        return Err(DiffusionError::workflow(format!(
            "flux2 te expects the fixed {FLUX2_MAX_SEQUENCE_LENGTH}-token window, got {seq} ids"
        )));
    }
    if prompt.real_len == 0 || prompt.real_len > seq {
        return Err(DiffusionError::workflow(format!(
            "flux2 te real_len {} outside 1..={seq}",
            prompt.real_len
        )));
    }

    // Embedding rows straight off the shard file: BF16 rows decode exactly
    // into f32, so no extra rounding is needed to match `inputs_embeds`.
    let mut embeds = vec![0.0f32; seq * hidden_size];
    for (row, id) in prompt.token_ids.iter().enumerate() {
        if *id >= config.vocab_size {
            return Err(DiffusionError::workflow(format!(
                "flux2 te token id {id} outside vocab {}",
                config.vocab_size
            )));
        }
        let values = weights.shards.tensor_row_f32(MISTRAL3_EMBED_TOKENS, *id as u64)?;
        if values.len() != hidden_size {
            return Err(DiffusionError::model(format!(
                "flux2 te embed row {id} width {}",
                values.len()
            )));
        }
        embeds[row * hidden_size..(row + 1) * hidden_size].copy_from_slice(&values);
    }
    let mut hidden = gpu_upload(&embeds, seq, hidden_size).map_err(DiffusionError::model)?;

    let (cos, sin) = mistral3_rope_table_values(&prepared.rope_inv_freq, seq);
    let half = prepared.rope_inv_freq.len();
    let rope_cos = gpu_upload(&cos, seq, half).map_err(DiffusionError::model)?;
    let rope_sin = gpu_upload(&sin, seq, half).map_err(DiffusionError::model)?;

    let mut taps = want_taps.then(|| Flux2TextTaps {
        embed: embeds,
        rope_cos: cos,
        rope_sin: sin,
        ..Flux2TextTaps::default()
    });

    let layers = config.layers_required_for_taps() as usize;
    let mut tap_hidden: Vec<(usize, Vec<f32>)> = Vec::with_capacity(FLUX2_HIDDEN_STATE_TAPS.len());
    for layer in 0..layers {
        if let Some(on_layer) = on_layer.as_deref_mut() {
            on_layer(layer + 1, layers);
        }
        let layer0_taps = match taps.as_mut() {
            Some(taps) if layer == 0 => Some(&mut taps.layer0),
            _ => None,
        };
        hidden = mistral3_layer_forward(
            weights,
            prepared,
            layer,
            hidden,
            &rope_cos,
            &rope_sin,
            prompt.real_len,
            layer0_taps,
        )?;
        let hidden_state_index = layer + 1;
        // Tapped runs keep EVERY layer so one validator pass bisects drift to
        // a single layer; production runs download only the conditioning taps.
        if FLUX2_HIDDEN_STATE_TAPS.contains(&hidden_state_index) || want_taps {
            tap_hidden.push((
                hidden_state_index,
                gpu_download(&hidden).map_err(DiffusionError::model)?,
            ));
        }
    }

    let tap_slices: Vec<&[f32]> = tap_hidden
        .iter()
        .filter(|(index, _)| FLUX2_HIDDEN_STATE_TAPS.contains(index))
        .map(|(_, v)| v.as_slice())
        .collect();
    let conditioning = flux2_conditioning_concat(&tap_slices, seq, hidden_size);
    debug_assert_eq!(conditioning.len(), seq * config.conditioning_dim() as usize);
    if let Some(taps) = taps.as_mut() {
        taps.hidden_states = tap_hidden;
    }
    Ok((conditioning, taps))
}

/// Row-major feature concat: `out[row] = [parts[0][row] | parts[1][row] | ...]`.
pub(crate) fn flux2_conditioning_concat(
    parts: &[&[f32]],
    rows: usize,
    hidden_size: usize,
) -> Vec<f32> {
    let width = parts.len() * hidden_size;
    let mut out = vec![0.0f32; rows * width];
    for (slot, part) in parts.iter().enumerate() {
        debug_assert_eq!(part.len(), rows * hidden_size);
        for row in 0..rows {
            let src = &part[row * hidden_size..(row + 1) * hidden_size];
            let dst = row * width + slot * hidden_size;
            out[dst..dst + hidden_size].copy_from_slice(src);
        }
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn mistral3_layer_forward(
    weights: &Flux2TextEncoderWeights,
    prepared: &Flux2TextEncoderPrepared,
    layer: usize,
    hidden: GpuTensor,
    rope_cos: &GpuTensor,
    rope_sin: &GpuTensor,
    real_len: usize,
    mut taps: Option<&mut Flux2TextLayer0Taps>,
) -> Result<GpuTensor> {
    let config = &prepared.config;
    let hidden_size = config.hidden_size as usize;
    let head_dim = config.head_dim as usize;
    let heads = config.num_attention_heads as usize;
    let kv_heads = config.num_key_value_heads as usize;
    let q_width = heads * head_dim;
    let kv_width = kv_heads * head_dim;
    let ffn = config.intermediate_size as usize;
    // HF computes `head_dim ** -0.5` in f64 and hands SDPA the f32 cast.
    let scale = (1.0 / (head_dim as f64).sqrt()) as f32;
    let prefix = mistral3_layer_prefix(layer as u32);

    let normed = norm_bf16(
        &hidden,
        hidden_size,
        &format!("{prefix}input_layernorm.weight"),
        &prepared.input_norm[layer],
        config.rms_norm_eps,
    )?;
    if let Some(taps) = taps.as_deref_mut() {
        taps.input_norm = gpu_download(&normed).map_err(DiffusionError::model)?;
    }

    let q_name = format!("{prefix}self_attn.q_proj.weight");
    let k_name = format!("{prefix}self_attn.k_proj.weight");
    let v_name = format!("{prefix}self_attn.v_proj.weight");
    let parts = [
        ensure_linear(weights, &q_name, q_width, hidden_size)?,
        ensure_linear(weights, &k_name, kv_width, hidden_size)?,
        ensure_linear(weights, &v_name, kv_width, hidden_size)?,
    ];
    let qkv = gpu_linear_nt_cached_bf16_f32acc(&normed, FLUX2_TE_NAMESPACE, &parts, &[])
        .map_err(DiffusionError::model)?;
    let qkv = gpu_bf16_round(&qkv).map_err(DiffusionError::model)?;
    let q = gpu_slice_cols(&qkv, 0, q_width).map_err(DiffusionError::model)?;
    let key = gpu_slice_cols(&qkv, q_width, kv_width).map_err(DiffusionError::model)?;
    let value = gpu_slice_cols(&qkv, q_width + kv_width, kv_width).map_err(DiffusionError::model)?;
    if let Some(taps) = taps.as_deref_mut() {
        taps.query_projected = gpu_download(&q).map_err(DiffusionError::model)?;
        taps.key_projected = gpu_download(&key).map_err(DiffusionError::model)?;
        taps.value_projected = gpu_download(&value).map_err(DiffusionError::model)?;
    }

    // Mistral3 has no q/k norms — projections go straight to rope.
    let q = gpu_rope_half_bf16(&q, heads, head_dim / 2, rope_cos, rope_sin)
        .map_err(DiffusionError::model)?;
    let key = gpu_rope_half_bf16(&key, kv_heads, head_dim / 2, rope_cos, rope_sin)
        .map_err(DiffusionError::model)?;
    if let Some(taps) = taps.as_deref_mut() {
        taps.query_rope = gpu_download(&q).map_err(DiffusionError::model)?;
        taps.key_rope = gpu_download(&key).map_err(DiffusionError::model)?;
    }

    let (key_full, value_full) = repeat_kv_heads(&key, &value, heads, kv_heads, head_dim)?;
    let attention = masked_causal_attention_bf16(&q, &key_full, &value_full, heads, scale, real_len)?;
    let attention = gpu_bf16_round(&attention).map_err(DiffusionError::model)?;
    if let Some(taps) = taps.as_deref_mut() {
        taps.attention_raw = gpu_download(&attention).map_err(DiffusionError::model)?;
    }
    let attention = linear_bf16(
        weights,
        &attention,
        &format!("{prefix}self_attn.o_proj.weight"),
        hidden_size,
    )?;
    if let Some(taps) = taps.as_deref_mut() {
        taps.attention_projected = gpu_download(&attention).map_err(DiffusionError::model)?;
    }
    let residual = gpu_add_bf16(&hidden, &attention).map_err(DiffusionError::model)?;
    if let Some(taps) = taps.as_deref_mut() {
        taps.attention_residual = gpu_download(&residual).map_err(DiffusionError::model)?;
    }

    let normed = norm_bf16(
        &residual,
        hidden_size,
        &format!("{prefix}post_attention_layernorm.weight"),
        &prepared.post_attention_norm[layer],
        config.rms_norm_eps,
    )?;
    if let Some(taps) = taps.as_deref_mut() {
        taps.post_attention_norm = gpu_download(&normed).map_err(DiffusionError::model)?;
    }
    // `gpu_swiglu_value_gate` computes `value * silu(gate)` with the value in
    // the LEFT half, so the fused projection order is [up | gate].
    let up_name = format!("{prefix}mlp.up_proj.weight");
    let gate_name = format!("{prefix}mlp.gate_proj.weight");
    let parts = [
        ensure_linear(weights, &up_name, ffn, hidden_size)?,
        ensure_linear(weights, &gate_name, ffn, hidden_size)?,
    ];
    let up_gate = gpu_linear_nt_cached_bf16_f32acc(&normed, FLUX2_TE_NAMESPACE, &parts, &[])
        .map_err(DiffusionError::model)?;
    let up_gate = gpu_bf16_round(&up_gate).map_err(DiffusionError::model)?;
    if let Some(taps) = taps.as_deref_mut() {
        taps.up_gate = gpu_download(&up_gate).map_err(DiffusionError::model)?;
    }
    let activated = gpu_swiglu_value_gate(&up_gate).map_err(DiffusionError::model)?;
    let activated = gpu_bf16_round(&activated).map_err(DiffusionError::model)?;
    if let Some(taps) = taps.as_deref_mut() {
        taps.activated = gpu_download(&activated).map_err(DiffusionError::model)?;
    }
    let update = linear_bf16(
        weights,
        &activated,
        &format!("{prefix}mlp.down_proj.weight"),
        hidden_size,
    )?;
    if let Some(taps) = taps.as_deref_mut() {
        taps.down_projected = gpu_download(&update).map_err(DiffusionError::model)?;
    }
    let out = gpu_add_bf16(&residual, &update).map_err(DiffusionError::model)?;
    if let Some(taps) = taps.as_deref_mut() {
        taps.hidden = gpu_download(&out).map_err(DiffusionError::model)?;
    }
    Ok(out)
}

/// Evict every cached TE weight buffer (~33.4GB) — call before loading the
/// DiT. Returns the number of buffers freed.
pub fn flux2_text_release() -> Result<usize> {
    gpu_weight_cache_evict_prefix(&format!("{FLUX2_TE_NAMESPACE}::")).map_err(DiffusionError::model)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bf16_round_is_nearest_even() {
        // bf16 keeps 7 explicit mantissa bits: ulp at 1.0 is 2^-7.
        let tie_low = f32::from_bits(0x3f80_8000); // 1.0 + 2^-8, even mantissa below
        assert_eq!(round_to_bf16(tie_low), 1.0);
        let above_tie = f32::from_bits(0x3f80_8100);
        assert_eq!(round_to_bf16(above_tie), f32::from_bits(0x3f81_0000));
        let tie_odd = f32::from_bits(0x3f81_8000); // odd mantissa rounds up to even
        assert_eq!(round_to_bf16(tie_odd), f32::from_bits(0x3f82_0000));
        assert_eq!(round_to_bf16(-1.0), -1.0);
        assert_eq!(round_to_bf16(3.140625), 3.140625); // bf16-exact survives
        assert_eq!(round_to_bf16(std::f32::consts::PI), 3.140625);
    }

    #[test]
    fn rope_inv_freq_matches_f64_reference() {
        let config = Mistral3TextConfig::flux2_dev();
        let inv_freq = mistral3_rope_inv_freq(&config);
        assert_eq!(inv_freq.len(), 64);
        assert_eq!(inv_freq[0], 1.0);
        for (index, value) in inv_freq.iter().enumerate() {
            let expected = 1.0 / 1.0e9f64.powf(2.0 * index as f64 / 128.0);
            let rel = ((*value as f64 - expected) / expected).abs();
            assert!(rel < 1e-6, "inv_freq[{index}] {value} vs {expected}");
        }
        assert!(inv_freq.windows(2).all(|pair| pair[1] < pair[0]));
    }

    #[test]
    fn rope_tables_are_bf16_rounded() {
        let config = Mistral3TextConfig::flux2_dev();
        let inv_freq = mistral3_rope_inv_freq(&config);
        let (cos, sin) = mistral3_rope_table_values(&inv_freq, 3);
        assert_eq!(cos.len(), 3 * 64);
        // Position 0: angle 0 for every frequency.
        assert!(cos[..64].iter().all(|value| *value == 1.0));
        assert!(sin[..64].iter().all(|value| *value == 0.0));
        // Position 1, frequency 0: bf16-rounded cos(1)/sin(1).
        assert_eq!(cos[64], round_to_bf16(1.0f32.cos()));
        assert_eq!(sin[64], round_to_bf16(1.0f32.sin()));
        // Every emitted value must already be a bf16 fixed point.
        for value in cos.iter().chain(sin.iter()) {
            assert_eq!(*value, round_to_bf16(*value));
        }
    }

    #[test]
    fn conditioning_concat_is_row_major_feature_blocks() {
        let h10 = [1.0f32, 2.0, 3.0, 10.0, 20.0, 30.0];
        let h20 = [4.0f32, 5.0, 6.0, 40.0, 50.0, 60.0];
        let h30 = [7.0f32, 8.0, 9.0, 70.0, 80.0, 90.0];
        let out = flux2_conditioning_concat(&[&h10, &h20, &h30], 2, 3);
        assert_eq!(
            out,
            vec![
                1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, //
                10.0, 20.0, 30.0, 40.0, 50.0, 60.0, 70.0, 80.0, 90.0,
            ]
        );
    }
}
