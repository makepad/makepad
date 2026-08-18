//! Native SkinTokens Qwen3 decoder.
//!
//! TokenRig does not feed token ids for its initial forward.  The 512-row
//! Michelangelo prefix is concatenated with the two skeleton start-token
//! embeddings and passed as `inputs_embeds` to a custom-width Qwen3 decoder.
//! This module preserves that contract, including BF16 operator boundaries,
//! grouped-query attention, and the un-repeated per-layer K/V cache needed by
//! autoregressive rig generation.

use crate::backend::{
    gpu_add_bf16, gpu_attention_gqa_decode_bf16, gpu_attention_packed_causal_bf16,
    gpu_attention_packed_cross_bf16, gpu_beam_cache_reorder_append, gpu_bf16_round,
    gpu_concat_cols, gpu_concat_rows, gpu_download,
    gpu_linear_nt_cached_bf16_f32acc, gpu_rms_norm_mul_bf16, gpu_rope_half_bf16,
    gpu_slice_cols, gpu_slice_rows, gpu_swiglu_value_gate, gpu_upload, gpu_weight_cache_ensure,
    gpu_weight_cache_evict_prefix, GpuLinearPart, GpuTensor,
};
use crate::skin_tokens::{
    SkinTokensWeights, SKIN_TOKENS_QWEN_CONTEXT, SKIN_TOKENS_QWEN_FFN,
    SKIN_TOKENS_QWEN_HEADS, SKIN_TOKENS_QWEN_HEAD_DIM, SKIN_TOKENS_QWEN_KV_HEADS,
    SKIN_TOKENS_QWEN_LAYERS, SKIN_TOKENS_QWEN_WIDTH, SKIN_TOKENS_VOCAB,
};
use crate::skin_tokens_tokenizer::{
    SkinTokensGenerationPhase, SkinTokensGrammar, SKIN_TOKENS_FSQ_OFFSET,
    SKIN_TOKENS_TOKEN_BOS, SKIN_TOKENS_TOKEN_CLASS_ARTICULATION_XL,
    SKIN_TOKENS_TOKEN_GLOBAL_EOS,
    SKIN_TOKENS_TOKEN_SKELETON_EOS,
};
use crate::{DiffusionError, Result};
use makepad_ggml::quant::GGML_TYPE_BF16;
use makepad_ai_loader::MlxDType;

/// Shared with the continuous-prefix encoders so one service unload evicts
/// every streamed TokenRig matrix, rather than leaving the decoder resident.
pub const SKIN_TOKENS_QWEN_NAMESPACE: &str = "skin-tokens-tokenrig-bf16::qwen";
pub const SKIN_TOKENS_QWEN_ROPE_THETA: f32 = 1_000_000.0;
pub const SKIN_TOKENS_QWEN_RMS_EPS: f32 = 1.0e-6;

/// Generation grammar policy. Production is deliberately strict: the
/// released Python processor emits global EOS one step early and its decoder
/// silently interprets that EOS as FSQ index 32768, which modulo-wraps to code
/// zero. Compatibility exists only to reproduce upstream validator IDs.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SkinTokensGenerationGrammar {
    #[default]
    Strict,
    OfficialOffByOneCompatibility,
}

#[derive(Clone, Debug)]
pub struct SkinTokensGenerationParams {
    pub seed: u64,
    pub do_sample: bool,
    pub max_length: usize,
    pub top_k: usize,
    pub top_p: f32,
    pub temperature: f32,
    pub repetition_penalty: f32,
    pub num_beams: usize,
    pub grammar: SkinTokensGenerationGrammar,
}

#[derive(Clone, Debug)]
pub struct SkinTokensGenerationProgress {
    pub generated: usize,
    pub max_length: usize,
    pub active_beams: usize,
    pub phase: SkinTokensGenerationPhase,
}

#[derive(Clone, Debug)]
pub struct SkinTokensGeneration {
    /// IDs returned by HF `generate(inputs_embeds=...)`, excluding the two
    /// explicit TokenRig start IDs.
    pub generated_ids: Vec<u32>,
    /// `[BOS, class] + generated_ids`.
    pub full_ids: Vec<u32>,
    /// Skeleton IDs including skeleton EOS 258.
    pub skeleton_ids: Vec<u32>,
    /// Zero-based FSQ codebook indices, exactly four per decoded joint in
    /// strict mode.
    pub fsq_indices: Vec<[u32; 4]>,
    pub score: f32,
    pub grammar: SkinTokensGenerationGrammar,
}

/// Validation-only view of one sampled-beam selection. Masked zero-probability
/// fillers are omitted; the official trace can filter its `-inf` scores before
/// comparing these arrays.
#[derive(Clone, Debug)]
pub struct SkinTokensBeamSelectionTrace {
    pub sampled_flat_indices: Vec<u32>,
    pub sampled_scores: Vec<f32>,
    pub running_parent_ids: Vec<u32>,
    pub running_token_ids: Vec<u32>,
}

impl Default for SkinTokensGenerationParams {
    fn default() -> Self {
        Self {
            seed: 0,
            do_sample: true,
            max_length: 2_048,
            top_k: 5,
            top_p: 0.95,
            temperature: 1.0,
            repetition_penalty: 2.0,
            num_beams: 10,
            grammar: SkinTokensGenerationGrammar::Strict,
        }
    }
}

fn layer_prefix(layer: usize) -> String {
    format!("transformer.model.layers.{layer}")
}

fn require_bf16_shape(
    weights: &SkinTokensWeights,
    name: &str,
    expected: &[u64],
) -> Result<()> {
    let (dtype, shape) = weights.tensor_dtype_shape(name)?;
    if dtype != MlxDType::BF16 || shape != expected {
        return Err(DiffusionError::model(format!(
            "SkinTokens Qwen tensor '{name}' is {dtype:?} {shape:?}, expected BF16 {expected:?}",
        )));
    }
    Ok(())
}

fn ensure_linear<'a>(
    weights: &SkinTokensWeights,
    name: &'a str,
    output_cols: usize,
    input_cols: usize,
) -> Result<GpuLinearPart<'a>> {
    require_bf16_shape(
        weights,
        name,
        &[output_cols as u64, input_cols as u64],
    )?;
    gpu_weight_cache_ensure(
        SKIN_TOKENS_QWEN_NAMESPACE,
        name,
        GGML_TYPE_BF16,
        output_cols,
        input_cols,
        false,
        || weights.tensor_bytes(name).map_err(|error| error.to_string()),
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
    weights: &SkinTokensWeights,
    input: &GpuTensor,
    name: &str,
    output_cols: usize,
) -> Result<GpuTensor> {
    let part = ensure_linear(weights, name, output_cols, input.cols())?;
    let output = gpu_linear_nt_cached_bf16_f32acc(
        input,
        SKIN_TOKENS_QWEN_NAMESPACE,
        &[part],
        &[],
    )
    .map_err(DiffusionError::model)?;
    gpu_bf16_round(&output).map_err(DiffusionError::model)
}

fn qkv_bf16(
    weights: &SkinTokensWeights,
    input: &GpuTensor,
    prefix: &str,
) -> Result<(GpuTensor, GpuTensor, GpuTensor)> {
    let q_width = SKIN_TOKENS_QWEN_HEADS * SKIN_TOKENS_QWEN_HEAD_DIM;
    let kv_width = SKIN_TOKENS_QWEN_KV_HEADS * SKIN_TOKENS_QWEN_HEAD_DIM;
    let q_name = format!("{prefix}.self_attn.q_proj.weight");
    let k_name = format!("{prefix}.self_attn.k_proj.weight");
    let v_name = format!("{prefix}.self_attn.v_proj.weight");
    let parts = [
        ensure_linear(weights, &q_name, q_width, input.cols())?,
        ensure_linear(weights, &k_name, kv_width, input.cols())?,
        ensure_linear(weights, &v_name, kv_width, input.cols())?,
    ];
    let projected = gpu_linear_nt_cached_bf16_f32acc(
        input,
        SKIN_TOKENS_QWEN_NAMESPACE,
        &parts,
        &[],
    )
    .map_err(DiffusionError::model)?;
    let projected = gpu_bf16_round(&projected).map_err(DiffusionError::model)?;
    let q = gpu_slice_cols(&projected, 0, q_width).map_err(DiffusionError::model)?;
    let key = gpu_slice_cols(&projected, q_width, kv_width).map_err(DiffusionError::model)?;
    let value = gpu_slice_cols(&projected, q_width + kv_width, kv_width)
        .map_err(DiffusionError::model)?;
    Ok((q, key, value))
}

fn up_gate_bf16(
    weights: &SkinTokensWeights,
    input: &GpuTensor,
    prefix: &str,
) -> Result<GpuTensor> {
    let up_name = format!("{prefix}.mlp.up_proj.weight");
    let gate_name = format!("{prefix}.mlp.gate_proj.weight");
    let parts = [
        ensure_linear(weights, &up_name, SKIN_TOKENS_QWEN_FFN, input.cols())?,
        ensure_linear(weights, &gate_name, SKIN_TOKENS_QWEN_FFN, input.cols())?,
    ];
    let output = gpu_linear_nt_cached_bf16_f32acc(
        input,
        SKIN_TOKENS_QWEN_NAMESPACE,
        &parts,
        &[],
    )
    .map_err(DiffusionError::model)?;
    gpu_bf16_round(&output).map_err(DiffusionError::model)
}

fn norm_bf16(
    input: &GpuTensor,
    group_cols: usize,
    cache_key: &str,
    scale: &[f32],
) -> Result<GpuTensor> {
    gpu_rms_norm_mul_bf16(
        input,
        group_cols,
        SKIN_TOKENS_QWEN_NAMESPACE,
        cache_key,
        scale,
        SKIN_TOKENS_QWEN_RMS_EPS,
    )
    .map_err(DiffusionError::model)
}

/// Small host-resident vectors and RoPE frequencies reused by every rig.
/// Large matrices remain on disk until their layer first executes.
pub struct SkinTokensQwenPrepared {
    embeddings_bf16: Vec<u16>,
    input_norm: Vec<Vec<f32>>,
    post_attention_norm: Vec<Vec<f32>>,
    q_norm: Vec<Vec<f32>>,
    k_norm: Vec<Vec<f32>>,
    final_norm: Vec<f32>,
    rope_inv_freq: Vec<f32>,
}

impl SkinTokensQwenPrepared {
    pub fn prepare(weights: &SkinTokensWeights) -> Result<Self> {
        require_bf16_shape(
            weights,
            "transformer.model.embed_tokens.weight",
            &[SKIN_TOKENS_VOCAB as u64, SKIN_TOKENS_QWEN_WIDTH as u64],
        )?;
        require_bf16_shape(
            weights,
            "transformer.model.norm.weight",
            &[SKIN_TOKENS_QWEN_WIDTH as u64],
        )?;
        require_bf16_shape(
            weights,
            "transformer.lm_head.weight",
            &[SKIN_TOKENS_VOCAB as u64, SKIN_TOKENS_QWEN_WIDTH as u64],
        )?;

        let embedding_bytes = weights.tensor_bytes("transformer.model.embed_tokens.weight")?;
        let embeddings_bf16: Vec<u16> = embedding_bytes
            .chunks_exact(2)
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .collect();
        if embeddings_bf16.len() != SKIN_TOKENS_VOCAB * SKIN_TOKENS_QWEN_WIDTH {
            return Err(DiffusionError::model(format!(
                "SkinTokens Qwen embedding table has {} values, expected {}",
                embeddings_bf16.len(),
                SKIN_TOKENS_VOCAB * SKIN_TOKENS_QWEN_WIDTH,
            )));
        }
        let q_width = SKIN_TOKENS_QWEN_HEADS * SKIN_TOKENS_QWEN_HEAD_DIM;
        let kv_width = SKIN_TOKENS_QWEN_KV_HEADS * SKIN_TOKENS_QWEN_HEAD_DIM;
        let mut input_norm = Vec::with_capacity(SKIN_TOKENS_QWEN_LAYERS);
        let mut post_attention_norm = Vec::with_capacity(SKIN_TOKENS_QWEN_LAYERS);
        let mut q_norm = Vec::with_capacity(SKIN_TOKENS_QWEN_LAYERS);
        let mut k_norm = Vec::with_capacity(SKIN_TOKENS_QWEN_LAYERS);
        for layer in 0..SKIN_TOKENS_QWEN_LAYERS {
            let prefix = layer_prefix(layer);
            for (suffix, shape) in [
                (
                    "input_layernorm.weight",
                    vec![SKIN_TOKENS_QWEN_WIDTH as u64],
                ),
                (
                    "post_attention_layernorm.weight",
                    vec![SKIN_TOKENS_QWEN_WIDTH as u64],
                ),
                (
                    "self_attn.q_norm.weight",
                    vec![SKIN_TOKENS_QWEN_HEAD_DIM as u64],
                ),
                (
                    "self_attn.k_norm.weight",
                    vec![SKIN_TOKENS_QWEN_HEAD_DIM as u64],
                ),
                (
                    "self_attn.q_proj.weight",
                    vec![q_width as u64, SKIN_TOKENS_QWEN_WIDTH as u64],
                ),
                (
                    "self_attn.k_proj.weight",
                    vec![kv_width as u64, SKIN_TOKENS_QWEN_WIDTH as u64],
                ),
                (
                    "self_attn.v_proj.weight",
                    vec![kv_width as u64, SKIN_TOKENS_QWEN_WIDTH as u64],
                ),
                (
                    "self_attn.o_proj.weight",
                    vec![SKIN_TOKENS_QWEN_WIDTH as u64, q_width as u64],
                ),
                (
                    "mlp.gate_proj.weight",
                    vec![SKIN_TOKENS_QWEN_FFN as u64, SKIN_TOKENS_QWEN_WIDTH as u64],
                ),
                (
                    "mlp.up_proj.weight",
                    vec![SKIN_TOKENS_QWEN_FFN as u64, SKIN_TOKENS_QWEN_WIDTH as u64],
                ),
                (
                    "mlp.down_proj.weight",
                    vec![SKIN_TOKENS_QWEN_WIDTH as u64, SKIN_TOKENS_QWEN_FFN as u64],
                ),
            ] {
                require_bf16_shape(weights, &format!("{prefix}.{suffix}"), &shape)?;
            }
            input_norm.push(
                weights.tensor_f32(&format!("{prefix}.input_layernorm.weight"))?,
            );
            post_attention_norm.push(weights.tensor_f32(&format!(
                "{prefix}.post_attention_layernorm.weight"
            ))?);
            q_norm.push(weights.tensor_f32(&format!(
                "{prefix}.self_attn.q_norm.weight"
            ))?);
            k_norm.push(weights.tensor_f32(&format!(
                "{prefix}.self_attn.k_norm.weight"
            ))?);
        }
        let final_norm = weights.tensor_f32("transformer.model.norm.weight")?;
        let rope_inv_freq = (0..SKIN_TOKENS_QWEN_HEAD_DIM / 2)
            .map(|index| {
                // `model.to(torch.bfloat16)` also casts Qwen's non-persistent
                // inverse-frequency buffer. The rotary forward promotes that
                // already-rounded value back to f32 before multiplying by the
                // position. Keeping full-f32 frequencies looks preferable but
                // accumulates a large, reference-incompatible phase error at
                // the 512-token Michelangelo prefix.
                round_to_bf16(
                    1.0 / SKIN_TOKENS_QWEN_ROPE_THETA.powf(
                        2.0 * index as f32 / SKIN_TOKENS_QWEN_HEAD_DIM as f32,
                    ),
                )
            })
            .collect();
        Ok(Self {
            embeddings_bf16,
            input_norm,
            post_attention_norm,
            q_norm,
            k_norm,
            final_norm,
            rope_inv_freq,
        })
    }

    fn embedding(&self, token_id: u32) -> Vec<f32> {
        let start = token_id as usize * SKIN_TOKENS_QWEN_WIDTH;
        self.embeddings_bf16[start..start + SKIN_TOKENS_QWEN_WIDTH]
            .iter()
            .map(|&word| f32::from_bits((word as u32) << 16))
            .collect()
    }
}

/// Un-repeated, RoPE-applied K and projected V for one decoder layer.  Both
/// tensors are token-major `[sequence, 8 * 128]`; this is efficient for native
/// append while validation taps transpose it to HF `[8, sequence, 128]`.
pub struct SkinTokensQwenLayerCache {
    pub key: GpuTensor,
    pub value: GpuTensor,
}

pub struct SkinTokensQwenCache {
    pub sequence: usize,
    pub layers: Vec<SkinTokensQwenLayerCache>,
}

/// Beam-major cache: each tensor flattens `[beams, sequence, 8 * 128]` into
/// rows. Parent reorder is fused with appending the current token.
pub struct SkinTokensQwenBeamCache {
    pub beams: usize,
    pub sequence: usize,
    pub layers: Vec<SkinTokensQwenLayerCache>,
}

impl SkinTokensQwenCache {
    /// Device-to-device cache copy used when the initial beam fans out. GPU
    /// tensors are intentionally not `Clone`; making this fallible and
    /// explicit prevents accidental multi-hundred-MB copies in ordinary code.
    pub fn try_clone_device(&self) -> Result<Self> {
        let mut layers = Vec::with_capacity(self.layers.len());
        for layer in &self.layers {
            layers.push(SkinTokensQwenLayerCache {
                key: gpu_slice_rows(&layer.key, 0, layer.key.rows())
                    .map_err(DiffusionError::model)?,
                value: gpu_slice_rows(&layer.value, 0, layer.value.rows())
                    .map_err(DiffusionError::model)?,
            });
        }
        Ok(Self {
            sequence: self.sequence,
            layers,
        })
    }


    /// Fan a one-sequence prefill cache out to the initial live beams. Later
    /// steps use fused parent reorder rather than cloning whole cache objects.
    pub fn expand_beams(&self, beams: usize) -> Result<SkinTokensQwenBeamCache> {
        if beams == 0 {
            return Err(DiffusionError::workflow(
                "SkinTokens Qwen needs at least one beam",
            ));
        }
        let mut layers = Vec::with_capacity(self.layers.len());
        for layer in &self.layers {
            let mut keys = Vec::with_capacity(beams);
            let mut values = Vec::with_capacity(beams);
            for _ in 0..beams {
                keys.push(
                    gpu_slice_rows(&layer.key, 0, layer.key.rows())
                        .map_err(DiffusionError::model)?,
                );
                values.push(
                    gpu_slice_rows(&layer.value, 0, layer.value.rows())
                        .map_err(DiffusionError::model)?,
                );
            }
            let mut key = keys.remove(0);
            let mut value = values.remove(0);
            for next in keys {
                key = gpu_concat_rows(&key, &next).map_err(DiffusionError::model)?;
            }
            for next in values {
                value = gpu_concat_rows(&value, &next).map_err(DiffusionError::model)?;
            }
            layers.push(SkinTokensQwenLayerCache { key, value });
        }
        Ok(SkinTokensQwenBeamCache {
            beams,
            sequence: self.sequence,
            layers,
        })
    }
}

pub struct SkinTokensQwenPrefill {
    /// BF16-rounded logits for the final input row, one value per TokenRig
    /// vocabulary entry.
    pub logits_last: Vec<f32>,
    pub cache: SkinTokensQwenCache,
}

pub struct SkinTokensQwenDecode {
    /// BF16-rounded logits predicting the token after `token_id`.
    pub logits_last: Vec<f32>,
    pub cache: SkinTokensQwenCache,
}

#[derive(Clone, Debug)]
pub struct SkinTokensQwenDecodeTap {
    pub logits_last: Vec<f32>,
    pub key0_head_major: Vec<f32>,
    pub value0_head_major: Vec<f32>,
}

pub struct SkinTokensQwenBeamDecode {
    /// Beam-major `[beams, vocab]` host logits.
    pub logits: Vec<f32>,
    pub cache: SkinTokensQwenBeamCache,
}

/// Validation-only first-beam-step boundaries. Every operator tensor is
/// flattened in the same batch/sequence-major order as the official hooks.
#[derive(Clone, Debug)]
pub struct SkinTokensQwenLayerOperatorTap {
    pub input_norm: Vec<f32>,
    pub query_projected: Vec<f32>,
    pub key_projected: Vec<f32>,
    pub value_projected: Vec<f32>,
    pub query_normalized: Vec<f32>,
    pub key_normalized: Vec<f32>,
    pub attention_raw: Vec<f32>,
    pub attention_projected: Vec<f32>,
    pub attention_residual: Vec<f32>,
    pub post_attention_norm: Vec<f32>,
    pub gate_projected: Vec<f32>,
    pub up_projected: Vec<f32>,
    pub mlp_activated: Vec<f32>,
    pub down_projected: Vec<f32>,
    pub hidden: Vec<f32>,
}

#[derive(Clone, Debug)]
pub struct SkinTokensQwenBeamDecodeTap {
    pub input_hidden: Vec<f32>,
    pub rope_cos: Vec<f32>,
    pub rope_sin: Vec<f32>,
    pub layer0: SkinTokensQwenLayerOperatorTap,
    pub layer_hidden: Vec<Vec<f32>>,
    pub final_norm: Vec<f32>,
}

/// Optional validation data. `hidden` is token-major `[sequence, 896]`, while
/// K/V use the exact flattened HF layout `[8, sequence, 128]`.
#[derive(Clone, Debug)]
pub struct SkinTokensQwenLayerTap {
    pub layer: usize,
    pub sequence: usize,
    pub hidden: Vec<f32>,
    pub key_head_major: Vec<f32>,
    pub value_head_major: Vec<f32>,
}

/// Layer-0 operator boundaries used to isolate porting differences without
/// conflating them with attention or residual drift. Shapes are flattened in
/// the same token-major order returned by HuggingFace module hooks.
#[derive(Clone, Debug)]
pub struct SkinTokensQwenProjectionTap {
    pub sequence: usize,
    pub input_norm: Vec<f32>,
    pub query_projected: Vec<f32>,
    pub key_projected: Vec<f32>,
    pub value_projected: Vec<f32>,
    pub query_normalized: Vec<f32>,
    pub key_normalized: Vec<f32>,
    pub query_rope: Vec<f32>,
    pub key_rope: Vec<f32>,
    pub rope_cos: Vec<f32>,
    pub rope_sin: Vec<f32>,
}

pub fn skin_tokens_qwen_prefill(
    weights: &SkinTokensWeights,
    prepared: &SkinTokensQwenPrepared,
    input_embeddings: &[f32],
) -> Result<SkinTokensQwenPrefill> {
    skin_tokens_qwen_prefill_inner(weights, prepared, input_embeddings, None, &[])
        .map(|(run, _)| run)
}

pub fn skin_tokens_qwen_prefill_controlled(
    weights: &SkinTokensWeights,
    prepared: &SkinTokensQwenPrepared,
    input_embeddings: &[f32],
    on_layer: Option<&mut dyn FnMut(usize, usize) -> Result<()>>,
) -> Result<SkinTokensQwenPrefill> {
    skin_tokens_qwen_prefill_inner(weights, prepared, input_embeddings, on_layer, &[])
        .map(|(run, _)| run)
}

pub fn skin_tokens_qwen_prefill_tapped(
    weights: &SkinTokensWeights,
    prepared: &SkinTokensQwenPrepared,
    input_embeddings: &[f32],
    tap_layers: &[usize],
) -> Result<(SkinTokensQwenPrefill, Vec<SkinTokensQwenLayerTap>)> {
    if tap_layers
        .iter()
        .any(|&layer| layer >= SKIN_TOKENS_QWEN_LAYERS)
    {
        return Err(DiffusionError::workflow(
            "SkinTokens Qwen tap layer is outside decoder depth",
        ));
    }
    skin_tokens_qwen_prefill_inner(weights, prepared, input_embeddings, None, tap_layers)
}

/// Append one generated token to an existing un-repeated K/V cache and return
/// logits for the following token. This mirrors HF `generate`: prefill logits
/// choose the first generated ID, then that ID is embedded and evaluated at
/// `cache.sequence` before the next sampling decision.
pub fn skin_tokens_qwen_decode_step(
    weights: &SkinTokensWeights,
    prepared: &SkinTokensQwenPrepared,
    cache: SkinTokensQwenCache,
    token_id: u32,
) -> Result<SkinTokensQwenDecode> {
    skin_tokens_qwen_decode_step_inner(weights, prepared, cache, token_id, false).map(|(run, _)| run)
}

/// Validation-only decode path with layer-0 cache downloads.
pub fn skin_tokens_qwen_decode_step_tapped(
    weights: &SkinTokensWeights,
    prepared: &SkinTokensQwenPrepared,
    cache: SkinTokensQwenCache,
    token_id: u32,
) -> Result<(SkinTokensQwenDecode, SkinTokensQwenDecodeTap)> {
    let (run, tap) = skin_tokens_qwen_decode_step_inner(weights, prepared, cache, token_id, true)?;
    Ok((run, tap.expect("decode tap requested")))
}

fn skin_tokens_qwen_decode_step_inner(
    weights: &SkinTokensWeights,
    prepared: &SkinTokensQwenPrepared,
    cache: SkinTokensQwenCache,
    token_id: u32,
    tap_layer0: bool,
) -> Result<(SkinTokensQwenDecode, Option<SkinTokensQwenDecodeTap>)> {
    if token_id as usize >= SKIN_TOKENS_VOCAB {
        return Err(DiffusionError::workflow(format!(
            "SkinTokens Qwen token {token_id} is outside vocabulary {SKIN_TOKENS_VOCAB}",
        )));
    }
    if cache.sequence >= SKIN_TOKENS_QWEN_CONTEXT {
        return Err(DiffusionError::workflow(format!(
            "SkinTokens Qwen cache length {} reached context {}",
            cache.sequence, SKIN_TOKENS_QWEN_CONTEXT,
        )));
    }
    if cache.layers.len() != SKIN_TOKENS_QWEN_LAYERS {
        return Err(DiffusionError::workflow(format!(
            "SkinTokens Qwen cache has {} layers, expected {}",
            cache.layers.len(),
            SKIN_TOKENS_QWEN_LAYERS,
        )));
    }
    let embedding = prepared.embedding(token_id);
    let mut hidden = gpu_upload(&embedding, 1, SKIN_TOKENS_QWEN_WIDTH)
        .map_err(DiffusionError::model)?;
    let (rope_cos, rope_sin) = rope_tables(prepared, cache.sequence, 1)?;
    let next_sequence = cache.sequence + 1;
    let mut layers = Vec::with_capacity(SKIN_TOKENS_QWEN_LAYERS);
    let mut key0_head_major = Vec::new();
    let mut value0_head_major = Vec::new();
    for (layer, prior) in cache.layers.into_iter().enumerate() {
        let output = qwen_decode_layer(
            weights,
            prepared,
            layer,
            hidden,
            prior,
            &rope_cos,
            &rope_sin,
        )?;
        hidden = output.hidden;
        if tap_layer0 && layer == 0 {
            key0_head_major = token_major_to_head_major(
                &gpu_download(&output.key).map_err(DiffusionError::model)?,
                next_sequence,
                SKIN_TOKENS_QWEN_KV_HEADS,
                SKIN_TOKENS_QWEN_HEAD_DIM,
            );
            value0_head_major = token_major_to_head_major(
                &gpu_download(&output.value).map_err(DiffusionError::model)?,
                next_sequence,
                SKIN_TOKENS_QWEN_KV_HEADS,
                SKIN_TOKENS_QWEN_HEAD_DIM,
            );
        }
        layers.push(SkinTokensQwenLayerCache {
            key: output.key,
            value: output.value,
        });
    }
    let hidden = norm_bf16(
        &hidden,
        SKIN_TOKENS_QWEN_WIDTH,
        "transformer.model.norm",
        &prepared.final_norm,
    )?;
    let logits = linear_bf16(
        weights,
        &hidden,
        "transformer.lm_head.weight",
        SKIN_TOKENS_VOCAB,
    )?;
    let logits_last = gpu_download(&logits).map_err(DiffusionError::model)?;
    let tap = tap_layer0.then(|| SkinTokensQwenDecodeTap {
        logits_last: logits_last.clone(),
        key0_head_major,
        value0_head_major,
    });
    Ok((
        SkinTokensQwenDecode {
            logits_last,
            cache: SkinTokensQwenCache {
                sequence: next_sequence,
                layers,
            },
        },
        tap,
    ))
}

/// Batched beam decode. `token_ids[i]` extends `parent_beams[i]`, allowing the
/// cache reorder chosen by beam search to remain entirely on device. The
/// first call normally uses ten copies of prefill (`parent_beams = 0..10`),
/// while later calls may duplicate or discard arbitrary parents.
pub fn skin_tokens_qwen_decode_beams(
    weights: &SkinTokensWeights,
    prepared: &SkinTokensQwenPrepared,
    cache: SkinTokensQwenBeamCache,
    token_ids: &[u32],
    parent_beams: &[u32],
) -> Result<SkinTokensQwenBeamDecode> {
    skin_tokens_qwen_decode_beams_inner(
        weights,
        prepared,
        cache,
        token_ids,
        parent_beams,
        false,
    )
    .map(|(decode, _)| decode)
}

pub fn skin_tokens_qwen_decode_beams_tapped(
    weights: &SkinTokensWeights,
    prepared: &SkinTokensQwenPrepared,
    cache: SkinTokensQwenBeamCache,
    token_ids: &[u32],
    parent_beams: &[u32],
) -> Result<(SkinTokensQwenBeamDecode, SkinTokensQwenBeamDecodeTap)> {
    let (decode, tap) = skin_tokens_qwen_decode_beams_inner(
        weights,
        prepared,
        cache,
        token_ids,
        parent_beams,
        true,
    )?;
    Ok((decode, tap.expect("beam decode tap requested")))
}

fn skin_tokens_qwen_decode_beams_inner(
    weights: &SkinTokensWeights,
    prepared: &SkinTokensQwenPrepared,
    cache: SkinTokensQwenBeamCache,
    token_ids: &[u32],
    parent_beams: &[u32],
    tapped: bool,
) -> Result<(SkinTokensQwenBeamDecode, Option<SkinTokensQwenBeamDecodeTap>)> {
    let beams = token_ids.len();
    if beams == 0 || parent_beams.len() != beams {
        return Err(DiffusionError::workflow(
            "SkinTokens Qwen beam tokens/parents are empty or mismatched",
        ));
    }
    if cache.sequence >= SKIN_TOKENS_QWEN_CONTEXT
        || cache.layers.len() != SKIN_TOKENS_QWEN_LAYERS
        || parent_beams.iter().any(|&parent| parent as usize >= cache.beams)
        || token_ids.iter().any(|&token| token as usize >= SKIN_TOKENS_VOCAB)
    {
        return Err(DiffusionError::workflow(
            "SkinTokens Qwen beam cache/token contract is invalid",
        ));
    }
    let mut embeddings = Vec::with_capacity(beams * SKIN_TOKENS_QWEN_WIDTH);
    for &token in token_ids {
        embeddings.extend_from_slice(&prepared.embedding(token));
    }
    let mut hidden = gpu_upload(&embeddings, beams, SKIN_TOKENS_QWEN_WIDTH)
        .map_err(DiffusionError::model)?;
    let input_hidden = if tapped {
        gpu_download(&hidden).map_err(DiffusionError::model)?
    } else {
        Vec::new()
    };
    let (rope_cos_one, rope_sin_one) = rope_table_values(prepared, cache.sequence, 1);
    let mut rope_cos = Vec::with_capacity(beams * rope_cos_one.len());
    let mut rope_sin = Vec::with_capacity(beams * rope_sin_one.len());
    for _ in 0..beams {
        rope_cos.extend_from_slice(&rope_cos_one);
        rope_sin.extend_from_slice(&rope_sin_one);
    }
    let rope_cos = gpu_upload(&rope_cos, beams, SKIN_TOKENS_QWEN_HEAD_DIM / 2)
        .map_err(DiffusionError::model)?;
    let rope_sin = gpu_upload(&rope_sin, beams, SKIN_TOKENS_QWEN_HEAD_DIM / 2)
        .map_err(DiffusionError::model)?;
    let rope_cos_tap = if tapped {
        gpu_download(&rope_cos).map_err(DiffusionError::model)?
    } else {
        Vec::new()
    };
    let rope_sin_tap = if tapped {
        gpu_download(&rope_sin).map_err(DiffusionError::model)?
    } else {
        Vec::new()
    };
    let prior_beams = cache.beams;
    let prior_sequence = cache.sequence;
    let mut layers = Vec::with_capacity(SKIN_TOKENS_QWEN_LAYERS);
    let mut layer_hidden = Vec::with_capacity(if tapped {
        SKIN_TOKENS_QWEN_LAYERS
    } else {
        0
    });
    let mut layer0_tap = None;
    for (layer, prior) in cache.layers.into_iter().enumerate() {
        let (output, operator_tap) = qwen_decode_beam_layer(
            weights,
            prepared,
            layer,
            hidden,
            prior,
            prior_beams,
            prior_sequence,
            parent_beams,
            &rope_cos,
            &rope_sin,
            tapped && layer == 0,
        )?;
        if tapped {
            layer_hidden.push(gpu_download(&output.hidden).map_err(DiffusionError::model)?);
        }
        if operator_tap.is_some() {
            layer0_tap = operator_tap;
        }
        hidden = output.hidden;
        layers.push(SkinTokensQwenLayerCache {
            key: output.key,
            value: output.value,
        });
    }
    let hidden = norm_bf16(
        &hidden,
        SKIN_TOKENS_QWEN_WIDTH,
        "transformer.model.norm",
        &prepared.final_norm,
    )?;
    let final_norm = if tapped {
        gpu_download(&hidden).map_err(DiffusionError::model)?
    } else {
        Vec::new()
    };
    let logits = linear_bf16(
        weights,
        &hidden,
        "transformer.lm_head.weight",
        SKIN_TOKENS_VOCAB,
    )?;
    Ok((
        SkinTokensQwenBeamDecode {
            logits: gpu_download(&logits).map_err(DiffusionError::model)?,
            cache: SkinTokensQwenBeamCache {
                beams,
                sequence: prior_sequence + 1,
                layers,
            },
        },
        tapped.then(|| SkinTokensQwenBeamDecodeTap {
            input_hidden,
            rope_cos: rope_cos_tap,
            rope_sin: rope_sin_tap,
            layer0: layer0_tap.expect("layer zero tap requested"),
            layer_hidden,
            final_norm,
        }),
    ))
}

#[derive(Clone)]
struct GenerationBeam {
    ids: Vec<u32>,
    grammar: SkinTokensGrammar,
    score: f32,
}

#[derive(Clone)]
struct GenerationCandidate {
    parent: usize,
    token: u32,
    score: f32,
    beam: GenerationBeam,
    finished: bool,
    sample_key: f32,
}

/// End-to-end native TokenRig Qwen generation from the resident 512-row
/// conditioner prefix. BOS and the articulation class embedding are appended
/// internally. The callback is invoked after every accepted beam step and can
/// cancel by returning an error. Default parameters are the released demo
/// settings with the corrected strict FSQ grammar.
pub fn skin_tokens_qwen_generate(
    weights: &SkinTokensWeights,
    prepared: &SkinTokensQwenPrepared,
    conditioner_prefix: &GpuTensor,
    params: &SkinTokensGenerationParams,
    on_step: Option<&mut dyn FnMut(&SkinTokensGenerationProgress) -> Result<()>>,
) -> Result<SkinTokensGeneration> {
    skin_tokens_qwen_generate_inner(
        weights,
        prepared,
        conditioner_prefix,
        params,
        on_step,
        None,
    )
}

/// Validator entry point that records the sampled finite continuations and
/// surviving beam slots without changing production generation behavior.
pub fn skin_tokens_qwen_generate_traced(
    weights: &SkinTokensWeights,
    prepared: &SkinTokensQwenPrepared,
    conditioner_prefix: &GpuTensor,
    params: &SkinTokensGenerationParams,
) -> Result<(SkinTokensGeneration, Vec<SkinTokensBeamSelectionTrace>)> {
    let mut trace = Vec::new();
    let generation = skin_tokens_qwen_generate_inner(
        weights,
        prepared,
        conditioner_prefix,
        params,
        None,
        Some(&mut trace),
    )?;
    Ok((generation, trace))
}

fn skin_tokens_qwen_generate_inner(
    weights: &SkinTokensWeights,
    prepared: &SkinTokensQwenPrepared,
    conditioner_prefix: &GpuTensor,
    params: &SkinTokensGenerationParams,
    mut on_step: Option<&mut dyn FnMut(&SkinTokensGenerationProgress) -> Result<()>>,
    mut beam_trace: Option<&mut Vec<SkinTokensBeamSelectionTrace>>,
) -> Result<SkinTokensGeneration> {
    if params.num_beams == 0
        || params.max_length == 0
        || params.max_length > SKIN_TOKENS_QWEN_CONTEXT
        || params.temperature <= 0.0
        || !(0.0..=1.0).contains(&params.top_p)
        || params.repetition_penalty <= 0.0
    {
        return Err(DiffusionError::workflow(
            "SkinTokens Qwen generation parameters are invalid",
        ));
    }
    let prefill = skin_tokens_qwen_prefill_conditioner(weights, prepared, conditioner_prefix)?;
    let start = [
        SKIN_TOKENS_TOKEN_BOS,
        SKIN_TOKENS_TOKEN_CLASS_ARTICULATION_XL,
    ];
    let grammar = SkinTokensGrammar::from_tokens(&start)?;
    let initial: Vec<GenerationBeam> = (0..params.num_beams)
        .map(|beam| GenerationBeam {
            ids: Vec::new(),
            grammar: grammar.clone(),
            // HF uses this finite sentinel for every beam except the first.
            // Softmax underflows it to zero probability, while the slots stay
            // available as fixed-shape fillers until enough real beams exist.
            score: if beam == 0 { 0.0 } else { -1.0e9 },
        })
        .collect();
    let mut initial_logits = Vec::with_capacity(params.num_beams * SKIN_TOKENS_VOCAB);
    for _ in 0..params.num_beams {
        initial_logits.extend_from_slice(&prefill.logits_last);
    }
    let mut rng = SkinTokensSamplingRng::new(params.seed);
    let (mut beams, mut finished, first_tokens, first_parents) = select_generation_beams(
        &initial,
        &initial_logits,
        params,
        &mut rng,
        beam_trace.as_deref_mut(),
    )?;
    if !finished.is_empty() && beams.is_empty() {
        return finalize_generation(best_finished(&finished), params.grammar);
    }
    let mut cache = prefill.cache.expand_beams(params.num_beams)?;
    debug_assert_eq!(first_tokens.len(), beams.len());
    debug_assert_eq!(first_parents.len(), beams.len());
    let mut decoded = skin_tokens_qwen_decode_beams(
        weights,
        prepared,
        cache,
        &first_tokens,
        &first_parents,
    )?;
    cache = decoded.cache;
    loop {
        if let Some(callback) = on_step.as_deref_mut() {
            callback(&SkinTokensGenerationProgress {
                generated: beams.first().map_or(0, |beam| beam.ids.len()),
                max_length: params.max_length,
                active_beams: beams.len(),
                phase: beams
                    .first()
                    .map(|beam| beam.grammar.phase())
                    .unwrap_or(SkinTokensGenerationPhase::Skeleton),
            })?;
        }
        if beams.is_empty() || beams[0].ids.len() >= params.max_length {
            break;
        }
        let (next, just_finished, tokens, parents) = select_generation_beams(
            &beams,
            &decoded.logits,
            params,
            &mut rng,
            beam_trace.as_deref_mut(),
        )?;
        finished.extend(just_finished);
        if next.is_empty() {
            beams.clear();
            break;
        }
        decoded = skin_tokens_qwen_decode_beams(weights, prepared, cache, &tokens, &parents)?;
        cache = decoded.cache;
        beams = next;
    }
    let best = if finished.is_empty() {
        beams
            .iter()
            .max_by(|left, right| left.score.total_cmp(&right.score))
            .cloned()
            .ok_or_else(|| DiffusionError::workflow("SkinTokens generation produced no beam"))?
    } else {
        best_finished(&finished)
    };
    finalize_generation(best, params.grammar)
}

/// Append TokenRig's BOS/class embeddings to the resident 512-row
/// Michelangelo prefix and prefill without a device-to-host round trip.
pub fn skin_tokens_qwen_prefill_conditioner(
    weights: &SkinTokensWeights,
    prepared: &SkinTokensQwenPrepared,
    conditioner_prefix: &GpuTensor,
) -> Result<SkinTokensQwenPrefill> {
    if conditioner_prefix.rows() != 512
        || conditioner_prefix.cols() != SKIN_TOKENS_QWEN_WIDTH
        || conditioner_prefix.is_half()
    {
        return Err(DiffusionError::workflow(format!(
            "SkinTokens conditioner prefix is {}x{} half={}, expected resident f32/BF16 512x{}",
            conditioner_prefix.rows(),
            conditioner_prefix.cols(),
            conditioner_prefix.is_half(),
            SKIN_TOKENS_QWEN_WIDTH,
        )));
    }
    let mut start_embeddings = prepared.embedding(SKIN_TOKENS_TOKEN_BOS);
    start_embeddings.extend_from_slice(&prepared.embedding(
        SKIN_TOKENS_TOKEN_CLASS_ARTICULATION_XL,
    ));
    let start = gpu_upload(&start_embeddings, 2, SKIN_TOKENS_QWEN_WIDTH)
        .map_err(DiffusionError::model)?;
    let hidden = gpu_concat_rows(conditioner_prefix, &start).map_err(DiffusionError::model)?;
    skin_tokens_qwen_prefill_hidden_inner(weights, prepared, hidden, 514, None, &[])
        .map(|(prefill, _)| prefill)
}

fn select_generation_beams(
    beams: &[GenerationBeam],
    logits: &[f32],
    params: &SkinTokensGenerationParams,
    rng: &mut SkinTokensSamplingRng,
    trace: Option<&mut Vec<SkinTokensBeamSelectionTrace>>,
) -> Result<(Vec<GenerationBeam>, Vec<GenerationBeam>, Vec<u32>, Vec<u32>)> {
    if logits.len() != beams.len() * SKIN_TOKENS_VOCAB {
        return Err(DiffusionError::workflow(
            "SkinTokens beam logits shape is invalid",
        ));
    }
    let mut candidates = Vec::new();
    for (parent, beam) in beams.iter().enumerate() {
        let row = &logits[parent * SKIN_TOKENS_VOCAB..(parent + 1) * SKIN_TOKENS_VOCAB];
        let maximum = row.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let log_sum = maximum
            + row
                .iter()
                .map(|value| (*value - maximum).exp() as f64)
                .sum::<f64>()
                .ln() as f32;
        let valid = generation_valid_ids(beam, params.grammar)?;
        let mut local = Vec::with_capacity(valid.len());
        valid.for_each(|token| {
            let mut value = row[token as usize] - log_sum;
            if beam.ids.contains(&token) && params.repetition_penalty != 1.0 {
                value = if value < 0.0 {
                    value * params.repetition_penalty
                } else {
                    value / params.repetition_penalty
                };
            }
            value /= params.temperature;
            local.push((token, value));
        });
        torch_generation_warp(&mut local, params);
        for (token, log_prob) in local {
            let mut next = beam.clone();
            next.ids.push(token);
            next.score += log_prob;
            let compatibility_eos = params.grammar
                == SkinTokensGenerationGrammar::OfficialOffByOneCompatibility
                && token == SKIN_TOKENS_TOKEN_GLOBAL_EOS;
            if !compatibility_eos {
                next.grammar.push(token)?;
            }
            let finished = compatibility_eos
                || matches!(next.grammar.phase(), SkinTokensGenerationPhase::Complete { .. });
            // `torch.multinomial(..., replacement=False)` samples the flattened
            // `[num_beams, vocab]` distribution with the Gumbel/exponential
            // race.  Its CUDA exponential fill gives every *flat vocabulary
            // index* a Philox value, including masked zero-probability entries;
            // drawing one host random number per retained candidate changes the
            // result as soon as the grammar leaves more than one beam alive.
            let flat_index = parent * SKIN_TOKENS_VOCAB + token as usize;
            let exponential = rng.torch_cuda_exponential(
                flat_index,
                params.num_beams * SKIN_TOKENS_VOCAB,
            );
            let sample_key = if params.do_sample {
                next.score - exponential.ln()
            } else {
                next.score
            };
            candidates.push(GenerationCandidate {
                parent,
                token,
                score: next.score,
                beam: next,
                finished,
                sample_key,
            });
        }
    }
    if params.do_sample {
        // PyTorch reserves one CUDA Philox range for the entire flattened
        // distribution on every multinomial call, not for the finite entries
        // that happened to survive the logits processors.
        rng.finish_torch_cuda_exponential(params.num_beams * SKIN_TOKENS_VOCAB);
    }
    candidates.sort_by(|left, right| right.sample_key.total_cmp(&left.sample_key));
    candidates.truncate((params.num_beams * 2).min(candidates.len()));
    let sampled_flat_indices = trace.as_ref().map(|_| {
        candidates
            .iter()
            .filter(|candidate| candidate.score > -5.0e8)
            .map(|candidate| {
                (candidate.parent * SKIN_TOKENS_VOCAB + candidate.token as usize) as u32
            })
            .collect()
    });
    let sampled_scores = trace.as_ref().map(|_| {
        candidates
            .iter()
            .filter(|candidate| candidate.score > -5.0e8)
            .map(|candidate| candidate.score)
            .collect()
    });
    // HF only admits finished sequences from the first `num_beams` sampled
    // continuations. Running beams, however, are selected by a second CUDA
    // top-k over all `2 * num_beams` samples after finished entries are masked.
    let finished: Vec<GenerationBeam> = candidates
        .iter()
        .take(params.num_beams)
        .filter(|candidate| candidate.finished)
        .map(|candidate| candidate.beam.clone())
        .collect();
    let candidates = torch_cuda_running_topk(candidates, params.num_beams);
    let traced_parents = trace
        .as_ref()
        .map(|_| candidates.iter().map(|candidate| candidate.parent as u32).collect());
    let traced_tokens = trace
        .as_ref()
        .map(|_| candidates.iter().map(|candidate| candidate.token).collect());
    let mut running = Vec::new();
    let mut tokens = Vec::new();
    let mut parents = Vec::new();
    for candidate in candidates {
        if !candidate.finished {
            tokens.push(candidate.token);
            parents.push(candidate.parent as u32);
            running.push(candidate.beam);
        }
    }
    if let Some(trace) = trace {
        trace.push(SkinTokensBeamSelectionTrace {
            sampled_flat_indices: sampled_flat_indices.expect("trace requested"),
            sampled_scores: sampled_scores.expect("trace requested"),
            // HF records the second top-k result before dropping entries that
            // satisfy stopping criteria. That distinction is visible on the
            // terminal all-EOS step even though no cache row survives it.
            running_parent_ids: traced_parents.expect("trace requested"),
            running_token_ids: traced_tokens.expect("trace requested"),
        });
    }
    Ok((running, finished, tokens, parents))
}

fn torch_generation_warp(local: &mut Vec<(u32, f32)>, params: &SkinTokensGenerationParams) {
    local.sort_by(|left, right| right.1.total_cmp(&left.1));
    let minimum_keep = if params.num_beams > 1 { 2 } else { 1 };
    let effective_top_k = params.top_k.max(minimum_keep);
    if params.top_k > 0 && local.len() > effective_top_k {
        // HF's TopKLogitsWarper masks values *below* the kth score. Equal
        // logits at the boundary all survive; BF16 logits make such ties
        // common enough that truncating to exactly k changes sampling.
        let threshold = local[effective_top_k - 1].1;
        local.retain(|item| item.1 >= threshold);
    }
    if params.top_p < 1.0 && !local.is_empty() {
        let max = local[0].1;
        let probabilities: Vec<f32> = local
            .iter()
            .map(|item| (item.1 - max).exp())
            .collect();
        let sum = probabilities.iter().copied().sum::<f32>();
        let minimum_keep = minimum_keep.min(local.len());
        let mut cumulative_low = 0.0f32;
        let mut remove = 0usize;
        // TopPLogitsWarper sorts ascending and removes entries whose
        // cumulative probability is <= 1-p, then protects the largest
        // two entries for beam sampling. `local` is descending, so walk
        // it backwards to preserve the same boundary rule.
        for probability in probabilities.iter().rev() {
            if local.len() - remove <= minimum_keep {
                break;
            }
            cumulative_low += *probability / sum;
            if cumulative_low <= 1.0 - params.top_p {
                remove += 1;
            } else {
                break;
            }
        }
        local.truncate(local.len() - remove);
    }
}

fn torch_cuda_running_topk(
    candidates: Vec<GenerationCandidate>,
    output_len: usize,
) -> Vec<GenerationCandidate> {
    let output_len = output_len.min(candidates.len());
    if output_len == 0 {
        return Vec::new();
    }
    if output_len > 32 {
        let mut candidates = candidates;
        candidates.sort_by(|left, right| {
            let left = left.score + if left.finished { -1.0e9 } else { 0.0 };
            let right = right.score + if right.finished { -1.0e9 } else { 0.0 };
            right.total_cmp(&left)
        });
        candidates.truncate(output_len);
        return candidates;
    }

    let masked_score = |candidate: &GenerationCandidate| {
        candidate.score + if candidate.finished { -1.0e9 } else { 0.0 }
    };
    let mut ranked_scores: Vec<f32> = candidates.iter().map(masked_score).collect();
    ranked_scores.sort_by(|left, right| right.total_cmp(left));
    let threshold = ranked_scores[output_len - 1];
    // `gatherTopK` first writes every value strictly above the radix-selected
    // threshold in input order, followed by the first threshold-equal values.
    let mut selected: Vec<GenerationCandidate> = candidates
        .iter()
        .filter(|candidate| masked_score(candidate) > threshold)
        .cloned()
        .collect();
    selected.extend(
        candidates
            .iter()
            .filter(|candidate| masked_score(candidate) == threshold)
            .take(output_len - selected.len())
            .cloned(),
    );
    debug_assert_eq!(selected.len(), output_len);

    // Torch sorts the ten gathered values with its unstable 32-element CUDA
    // bitonic network. Its exact equal-score swaps matter: they determine beam
    // slots, and the slot is part of the flattened Philox sample index on the
    // next step.
    let mut keys = [0.0f32; 32];
    let mut values: [Option<GenerationCandidate>; 32] = std::array::from_fn(|_| None);
    let mut valid = [false; 32];
    for (index, candidate) in selected.into_iter().enumerate() {
        keys[index] = masked_score(&candidate);
        values[index] = Some(candidate);
        valid[index] = true;
    }
    let bitonic_swap = |left: usize,
                        right: usize,
                        direction: bool,
                        keys: &mut [f32; 32],
                        values: &mut [Option<GenerationCandidate>; 32],
                        valid: &mut [bool; 32]| {
        let swap = (keys[left] > keys[right] && valid[left]) || !valid[right];
        if swap == direction {
            keys.swap(left, right);
            values.swap(left, right);
            valid.swap(left, right);
        }
    };
    let mut size = 2usize;
    while size < 32 {
        let mut stride = size / 2;
        while stride > 0 {
            for thread in 0..16 {
                let position = 2 * thread - (thread & (stride - 1));
                bitonic_swap(
                    position,
                    position + stride,
                    thread & (size / 2) != 0,
                    &mut keys,
                    &mut values,
                    &mut valid,
                );
            }
            stride /= 2;
        }
        size *= 2;
    }
    let mut stride = 16usize;
    while stride > 0 {
        for thread in 0..16 {
            let position = 2 * thread - (thread & (stride - 1));
            bitonic_swap(
                position,
                position + stride,
                false,
                &mut keys,
                &mut values,
                &mut valid,
            );
        }
        stride /= 2;
    }
    values
        .into_iter()
        .take(output_len)
        .map(|candidate| candidate.expect("valid bitonic output"))
        .collect()
}

fn generation_valid_ids(
    beam: &GenerationBeam,
    policy: SkinTokensGenerationGrammar,
) -> Result<crate::skin_tokens_tokenizer::SkinTokensValidIds> {
    if policy == SkinTokensGenerationGrammar::OfficialOffByOneCompatibility {
        if let SkinTokensGenerationPhase::Skin { bones, generated } = beam.grammar.phase() {
            if generated + 1 == bones * 4 {
                return Ok(crate::skin_tokens_tokenizer::SkinTokensValidIds::One(
                    SKIN_TOKENS_TOKEN_GLOBAL_EOS,
                ));
            }
        }
    }
    beam.grammar.valid_next()
}

fn best_finished(beams: &[GenerationBeam]) -> GenerationBeam {
    beams
        .iter()
        .max_by(|left, right| {
            (left.score / left.ids.len().max(1) as f32)
                .total_cmp(&(right.score / right.ids.len().max(1) as f32))
        })
        .expect("non-empty finished beams")
        .clone()
}

fn finalize_generation(
    beam: GenerationBeam,
    policy: SkinTokensGenerationGrammar,
) -> Result<SkinTokensGeneration> {
    let mut full_ids = vec![
        SKIN_TOKENS_TOKEN_BOS,
        SKIN_TOKENS_TOKEN_CLASS_ARTICULATION_XL,
    ];
    full_ids.extend_from_slice(&beam.ids);
    let skeleton_end = full_ids
        .iter()
        .position(|&token| token == SKIN_TOKENS_TOKEN_SKELETON_EOS)
        .ok_or_else(|| DiffusionError::workflow("SkinTokens generation has no skeleton EOS"))?;
    let global_end = full_ids
        .iter()
        .position(|&token| token == SKIN_TOKENS_TOKEN_GLOBAL_EOS)
        .ok_or_else(|| DiffusionError::workflow("SkinTokens generation has no global EOS"))?;
    let skeleton_ids = full_ids[..=skeleton_end].to_vec();
    let mut payload = full_ids[skeleton_end + 1..global_end].to_vec();
    if policy == SkinTokensGenerationGrammar::OfficialOffByOneCompatibility {
        // Upstream decode silently maps EOS-offset 32768 to FSQ code zero.
        payload.push(SKIN_TOKENS_FSQ_OFFSET);
    }
    if payload.len() % 4 != 0 {
        return Err(DiffusionError::workflow(format!(
            "SkinTokens FSQ payload has {} IDs, expected a multiple of four",
            payload.len(),
        )));
    }
    let mut fsq_indices = Vec::with_capacity(payload.len() / 4);
    for group in payload.chunks_exact(4) {
        let mut output = [0u32; 4];
        for (slot, &token) in output.iter_mut().zip(group) {
            *slot = token.checked_sub(SKIN_TOKENS_FSQ_OFFSET).ok_or_else(|| {
                DiffusionError::workflow("SkinTokens FSQ token is below codebook offset")
            })?;
            if *slot >= 32_768 {
                return Err(DiffusionError::workflow(
                    "SkinTokens FSQ token is outside codebook",
                ));
            }
        }
        fsq_indices.push(output);
    }
    Ok(SkinTokensGeneration {
        generated_ids: beam.ids,
        full_ids,
        skeleton_ids,
        fsq_indices,
        score: beam.score,
        grammar: policy,
    })
}

struct SkinTokensSamplingRng {
    seed: u64,
    philox_offset: u64,
}

impl SkinTokensSamplingRng {
    fn new(seed: u64) -> Self {
        Self { seed, philox_offset: 0 }
    }

    // Torch 2.7's CUDA distribution kernel uses 256-thread blocks, caps the
    // grid at six blocks on each of the RTX 4090's 128 SMs, and emits a
    // `float4` per Philox invocation.  Compatibility generation is explicitly
    // pinned to that released oracle; strict production grammar remains the
    // default and merely gains deterministic sampling from this implementation.
    const TORCH_CUDA_BLOCK_THREADS: usize = 256;
    const TORCH_CUDA_MAX_BLOCKS_4090: usize = 128 * 6;
    const TORCH_CUDA_UNROLL: usize = 4;

    fn torch_cuda_policy(elements: usize) -> (usize, u64) {
        let blocks = elements
            .div_ceil(Self::TORCH_CUDA_BLOCK_THREADS)
            .min(Self::TORCH_CUDA_MAX_BLOCKS_4090)
            .max(1);
        let thread_span = blocks * Self::TORCH_CUDA_BLOCK_THREADS;
        let iterations = elements.div_ceil(thread_span * Self::TORCH_CUDA_UNROLL);
        (thread_span, (iterations * Self::TORCH_CUDA_UNROLL) as u64)
    }

    fn torch_cuda_exponential(&self, flat_index: usize, elements: usize) -> f32 {
        let (thread_span, _) = Self::torch_cuda_policy(elements);
        let iteration_span = thread_span * Self::TORCH_CUDA_UNROLL;
        let iteration = flat_index / iteration_span;
        let within = flat_index % iteration_span;
        let component = within / thread_span;
        let subsequence = within % thread_span;
        debug_assert!(component < Self::TORCH_CUDA_UNROLL);
        let counter = self.philox_offset / 4 + iteration as u64;
        let words = philox4x32_10(self.seed, subsequence as u64, counter);
        let uniform = (words[component] as f32).mul_add(
            2.328_306_4e-10,
            1.164_153_2e-10,
        );
        // This is `at::transformation::exponential<float>` on CUDA.  cuRAND
        // returns (0, 1], while exponential excludes zero.
        if uniform >= 1.0 - f32::EPSILON / 2.0 {
            f32::EPSILON / 2.0
        } else {
            -uniform.ln()
        }
    }

    fn finish_torch_cuda_exponential(&mut self, elements: usize) {
        let (_, counter_offset) = Self::torch_cuda_policy(elements);
        self.philox_offset = self.philox_offset.wrapping_add(counter_offset);
    }
}

fn philox4x32_10(seed: u64, subsequence: u64, counter: u64) -> [u32; 4] {
    const PHILOX_M0: u32 = 0xd251_1f53;
    const PHILOX_M1: u32 = 0xcd9e_8d57;
    const PHILOX_W0: u32 = 0x9e37_79b9;
    const PHILOX_W1: u32 = 0xbb67_ae85;

    let mut value = [
        counter as u32,
        (counter >> 32) as u32,
        subsequence as u32,
        (subsequence >> 32) as u32,
    ];
    let mut key = [seed as u32, (seed >> 32) as u32];
    for _ in 0..10 {
        let product0 = PHILOX_M0 as u64 * value[0] as u64;
        let product1 = PHILOX_M1 as u64 * value[2] as u64;
        value = [
            (product1 >> 32) as u32 ^ value[1] ^ key[0],
            product1 as u32,
            (product0 >> 32) as u32 ^ value[3] ^ key[1],
            product0 as u32,
        ];
        key[0] = key[0].wrapping_add(PHILOX_W0);
        key[1] = key[1].wrapping_add(PHILOX_W1);
    }
    value
}

/// Validation-only layer-0 Q/K/V projection path. Production prefill keeps
/// these tensors on device and does not pay any of the downloads below.
pub fn skin_tokens_qwen_projection_tap(
    weights: &SkinTokensWeights,
    prepared: &SkinTokensQwenPrepared,
    input_embeddings: &[f32],
) -> Result<SkinTokensQwenProjectionTap> {
    if input_embeddings.is_empty() || input_embeddings.len() % SKIN_TOKENS_QWEN_WIDTH != 0 {
        return Err(DiffusionError::workflow(
            "SkinTokens Qwen projection tap requires complete input rows",
        ));
    }
    let sequence = input_embeddings.len() / SKIN_TOKENS_QWEN_WIDTH;
    let hidden = gpu_upload(input_embeddings, sequence, SKIN_TOKENS_QWEN_WIDTH)
        .map_err(DiffusionError::model)?;
    let prefix = layer_prefix(0);
    let input_norm = norm_bf16(
        &hidden,
        SKIN_TOKENS_QWEN_WIDTH,
        &format!("{prefix}.input_layernorm"),
        &prepared.input_norm[0],
    )?;
    let (query_projected, key_projected, value_projected) =
        qkv_bf16(weights, &input_norm, &prefix)?;
    let query_normalized = norm_bf16(
        &query_projected,
        SKIN_TOKENS_QWEN_HEAD_DIM,
        &format!("{prefix}.self_attn.q_norm"),
        &prepared.q_norm[0],
    )?;
    let key_normalized = norm_bf16(
        &key_projected,
        SKIN_TOKENS_QWEN_HEAD_DIM,
        &format!("{prefix}.self_attn.k_norm"),
        &prepared.k_norm[0],
    )?;
    let (rope_cos, rope_sin) = rope_tables(prepared, 0, sequence)?;
    let query_rope = gpu_rope_half_bf16(
        &query_normalized,
        SKIN_TOKENS_QWEN_HEADS,
        SKIN_TOKENS_QWEN_HEAD_DIM / 2,
        &rope_cos,
        &rope_sin,
    )
    .map_err(DiffusionError::model)?;
    let key_rope = gpu_rope_half_bf16(
        &key_normalized,
        SKIN_TOKENS_QWEN_KV_HEADS,
        SKIN_TOKENS_QWEN_HEAD_DIM / 2,
        &rope_cos,
        &rope_sin,
    )
    .map_err(DiffusionError::model)?;
    Ok(SkinTokensQwenProjectionTap {
        sequence,
        input_norm: gpu_download(&input_norm).map_err(DiffusionError::model)?,
        query_projected: gpu_download(&query_projected).map_err(DiffusionError::model)?,
        key_projected: gpu_download(&key_projected).map_err(DiffusionError::model)?,
        value_projected: gpu_download(&value_projected).map_err(DiffusionError::model)?,
        query_normalized: gpu_download(&query_normalized).map_err(DiffusionError::model)?,
        key_normalized: gpu_download(&key_normalized).map_err(DiffusionError::model)?,
        query_rope: gpu_download(&query_rope).map_err(DiffusionError::model)?,
        key_rope: gpu_download(&key_rope).map_err(DiffusionError::model)?,
        rope_cos: gpu_download(&rope_cos).map_err(DiffusionError::model)?,
        rope_sin: gpu_download(&rope_sin).map_err(DiffusionError::model)?,
    })
}

fn skin_tokens_qwen_prefill_inner(
    weights: &SkinTokensWeights,
    prepared: &SkinTokensQwenPrepared,
    input_embeddings: &[f32],
    on_layer: Option<&mut dyn FnMut(usize, usize) -> Result<()>>,
    tap_layers: &[usize],
) -> Result<(SkinTokensQwenPrefill, Vec<SkinTokensQwenLayerTap>)> {
    if input_embeddings.is_empty()
        || input_embeddings.len() % SKIN_TOKENS_QWEN_WIDTH != 0
    {
        return Err(DiffusionError::workflow(format!(
            "SkinTokens Qwen inputs_embeds has {} values, expected non-empty rows of {}",
            input_embeddings.len(),
            SKIN_TOKENS_QWEN_WIDTH,
        )));
    }
    let sequence = input_embeddings.len() / SKIN_TOKENS_QWEN_WIDTH;
    if sequence > SKIN_TOKENS_QWEN_CONTEXT {
        return Err(DiffusionError::workflow(format!(
            "SkinTokens Qwen sequence {sequence} exceeds context {SKIN_TOKENS_QWEN_CONTEXT}",
        )));
    }

    let hidden = gpu_upload(input_embeddings, sequence, SKIN_TOKENS_QWEN_WIDTH)
        .map_err(DiffusionError::model)?;
    skin_tokens_qwen_prefill_hidden_inner(
        weights,
        prepared,
        hidden,
        sequence,
        on_layer,
        tap_layers,
    )
}

fn skin_tokens_qwen_prefill_hidden_inner(
    weights: &SkinTokensWeights,
    prepared: &SkinTokensQwenPrepared,
    mut hidden: GpuTensor,
    sequence: usize,
    mut on_layer: Option<&mut dyn FnMut(usize, usize) -> Result<()>>,
    tap_layers: &[usize],
) -> Result<(SkinTokensQwenPrefill, Vec<SkinTokensQwenLayerTap>)> {
    let (rope_cos, rope_sin) = rope_tables(prepared, 0, sequence)?;
    let mut caches = Vec::with_capacity(SKIN_TOKENS_QWEN_LAYERS);
    let mut taps = Vec::with_capacity(tap_layers.len());
    for layer in 0..SKIN_TOKENS_QWEN_LAYERS {
        if let Some(callback) = on_layer.as_deref_mut() {
            callback(layer + 1, SKIN_TOKENS_QWEN_LAYERS)?;
        }
        let output = qwen_prefill_layer(
            weights,
            prepared,
            layer,
            hidden,
            &rope_cos,
            &rope_sin,
        )?;
        hidden = output.hidden;
        if tap_layers.contains(&layer) {
            let key = gpu_download(&output.key).map_err(DiffusionError::model)?;
            let value = gpu_download(&output.value).map_err(DiffusionError::model)?;
            taps.push(SkinTokensQwenLayerTap {
                layer,
                sequence,
                hidden: gpu_download(&hidden).map_err(DiffusionError::model)?,
                key_head_major: token_major_to_head_major(
                    &key,
                    sequence,
                    SKIN_TOKENS_QWEN_KV_HEADS,
                    SKIN_TOKENS_QWEN_HEAD_DIM,
                ),
                value_head_major: token_major_to_head_major(
                    &value,
                    sequence,
                    SKIN_TOKENS_QWEN_KV_HEADS,
                    SKIN_TOKENS_QWEN_HEAD_DIM,
                ),
            });
        }
        caches.push(SkinTokensQwenLayerCache {
            key: output.key,
            value: output.value,
        });
    }

    let hidden = norm_bf16(
        &hidden,
        SKIN_TOKENS_QWEN_WIDTH,
        "transformer.model.norm",
        &prepared.final_norm,
    )?;
    let last = gpu_slice_rows(&hidden, sequence - 1, 1).map_err(DiffusionError::model)?;
    let logits = linear_bf16(
        weights,
        &last,
        "transformer.lm_head.weight",
        SKIN_TOKENS_VOCAB,
    )?;
    let logits_last = gpu_download(&logits).map_err(DiffusionError::model)?;
    debug_assert_eq!(logits_last.len(), SKIN_TOKENS_VOCAB);
    Ok((
        SkinTokensQwenPrefill {
            logits_last,
            cache: SkinTokensQwenCache {
                sequence,
                layers: caches,
            },
        },
        taps,
    ))
}

struct LayerOutput {
    hidden: GpuTensor,
    key: GpuTensor,
    value: GpuTensor,
}

fn qwen_prefill_layer(
    weights: &SkinTokensWeights,
    prepared: &SkinTokensQwenPrepared,
    layer: usize,
    hidden: GpuTensor,
    rope_cos: &GpuTensor,
    rope_sin: &GpuTensor,
) -> Result<LayerOutput> {
    let prefix = layer_prefix(layer);
    let normed = norm_bf16(
        &hidden,
        SKIN_TOKENS_QWEN_WIDTH,
        &format!("{prefix}.input_layernorm"),
        &prepared.input_norm[layer],
    )?;

    let (q, key, value) = qkv_bf16(weights, &normed, &prefix)?;
    let q = norm_bf16(
        &q,
        SKIN_TOKENS_QWEN_HEAD_DIM,
        &format!("{prefix}.self_attn.q_norm"),
        &prepared.q_norm[layer],
    )?;
    let key = norm_bf16(
        &key,
        SKIN_TOKENS_QWEN_HEAD_DIM,
        &format!("{prefix}.self_attn.k_norm"),
        &prepared.k_norm[layer],
    )?;
    let q = gpu_rope_half_bf16(
        &q,
        SKIN_TOKENS_QWEN_HEADS,
        SKIN_TOKENS_QWEN_HEAD_DIM / 2,
        rope_cos,
        rope_sin,
    )
    .map_err(DiffusionError::model)?;
    let key = gpu_rope_half_bf16(
        &key,
        SKIN_TOKENS_QWEN_KV_HEADS,
        SKIN_TOKENS_QWEN_HEAD_DIM / 2,
        rope_cos,
        rope_sin,
    )
    .map_err(DiffusionError::model)?;

    let (repeated_key, repeated_value) = repeat_kv_heads(&key, &value)?;
    let attention = gpu_attention_packed_causal_bf16(
        &q,
        &repeated_key,
        &repeated_value,
        SKIN_TOKENS_QWEN_HEADS,
        1.0 / (SKIN_TOKENS_QWEN_HEAD_DIM as f32).sqrt(),
    )
    .map_err(DiffusionError::model)?;
    let attention = gpu_bf16_round(&attention).map_err(DiffusionError::model)?;
    let attention = linear_bf16(
        weights,
        &attention,
        &format!("{prefix}.self_attn.o_proj.weight"),
        SKIN_TOKENS_QWEN_WIDTH,
    )?;
    let residual = gpu_add_bf16(&hidden, &attention).map_err(DiffusionError::model)?;

    let normed = norm_bf16(
        &residual,
        SKIN_TOKENS_QWEN_WIDTH,
        &format!("{prefix}.post_attention_layernorm"),
        &prepared.post_attention_norm[layer],
    )?;
    let up_gate = up_gate_bf16(weights, &normed, &prefix)?;
    let activated = gpu_swiglu_value_gate(&up_gate).map_err(DiffusionError::model)?;
    let activated = gpu_bf16_round(&activated).map_err(DiffusionError::model)?;
    let update = linear_bf16(
        weights,
        &activated,
        &format!("{prefix}.mlp.down_proj.weight"),
        SKIN_TOKENS_QWEN_WIDTH,
    )?;
    let hidden = gpu_add_bf16(&residual, &update).map_err(DiffusionError::model)?;
    Ok(LayerOutput { hidden, key, value })
}

fn qwen_decode_layer(
    weights: &SkinTokensWeights,
    prepared: &SkinTokensQwenPrepared,
    layer: usize,
    hidden: GpuTensor,
    prior: SkinTokensQwenLayerCache,
    rope_cos: &GpuTensor,
    rope_sin: &GpuTensor,
) -> Result<LayerOutput> {
    let prefix = layer_prefix(layer);
    let kv_width = SKIN_TOKENS_QWEN_KV_HEADS * SKIN_TOKENS_QWEN_HEAD_DIM;
    if prior.key.cols() != kv_width
        || prior.value.cols() != kv_width
        || prior.key.rows() != prior.value.rows()
    {
        return Err(DiffusionError::workflow(format!(
            "SkinTokens Qwen layer {layer} has malformed K/V cache",
        )));
    }
    let normed = norm_bf16(
        &hidden,
        SKIN_TOKENS_QWEN_WIDTH,
        &format!("{prefix}.input_layernorm"),
        &prepared.input_norm[layer],
    )?;
    let (q, key_step, value_step) = qkv_bf16(weights, &normed, &prefix)?;
    let q = norm_bf16(
        &q,
        SKIN_TOKENS_QWEN_HEAD_DIM,
        &format!("{prefix}.self_attn.q_norm"),
        &prepared.q_norm[layer],
    )?;
    let key_step = norm_bf16(
        &key_step,
        SKIN_TOKENS_QWEN_HEAD_DIM,
        &format!("{prefix}.self_attn.k_norm"),
        &prepared.k_norm[layer],
    )?;
    let q = gpu_rope_half_bf16(
        &q,
        SKIN_TOKENS_QWEN_HEADS,
        SKIN_TOKENS_QWEN_HEAD_DIM / 2,
        rope_cos,
        rope_sin,
    )
    .map_err(DiffusionError::model)?;
    let key_step = gpu_rope_half_bf16(
        &key_step,
        SKIN_TOKENS_QWEN_KV_HEADS,
        SKIN_TOKENS_QWEN_HEAD_DIM / 2,
        rope_cos,
        rope_sin,
    )
    .map_err(DiffusionError::model)?;
    let key = gpu_concat_rows(&prior.key, &key_step).map_err(DiffusionError::model)?;
    let value = gpu_concat_rows(&prior.value, &value_step).map_err(DiffusionError::model)?;
    let (repeated_key, repeated_value) = repeat_kv_heads(&key, &value)?;
    let attention = gpu_attention_packed_cross_bf16(
        &q,
        &repeated_key,
        &repeated_value,
        SKIN_TOKENS_QWEN_HEADS,
        1.0 / (SKIN_TOKENS_QWEN_HEAD_DIM as f32).sqrt(),
    )
    .map_err(DiffusionError::model)?;
    let attention = gpu_bf16_round(&attention).map_err(DiffusionError::model)?;
    let attention = linear_bf16(
        weights,
        &attention,
        &format!("{prefix}.self_attn.o_proj.weight"),
        SKIN_TOKENS_QWEN_WIDTH,
    )?;
    let residual = gpu_add_bf16(&hidden, &attention).map_err(DiffusionError::model)?;
    let normed = norm_bf16(
        &residual,
        SKIN_TOKENS_QWEN_WIDTH,
        &format!("{prefix}.post_attention_layernorm"),
        &prepared.post_attention_norm[layer],
    )?;
    let up_gate = up_gate_bf16(weights, &normed, &prefix)?;
    let activated = gpu_swiglu_value_gate(&up_gate).map_err(DiffusionError::model)?;
    let activated = gpu_bf16_round(&activated).map_err(DiffusionError::model)?;
    let update = linear_bf16(
        weights,
        &activated,
        &format!("{prefix}.mlp.down_proj.weight"),
        SKIN_TOKENS_QWEN_WIDTH,
    )?;
    let hidden = gpu_add_bf16(&residual, &update).map_err(DiffusionError::model)?;
    Ok(LayerOutput { hidden, key, value })
}

#[allow(clippy::too_many_arguments)]
fn qwen_decode_beam_layer(
    weights: &SkinTokensWeights,
    prepared: &SkinTokensQwenPrepared,
    layer: usize,
    hidden: GpuTensor,
    prior: SkinTokensQwenLayerCache,
    prior_beams: usize,
    prior_sequence: usize,
    parents: &[u32],
    rope_cos: &GpuTensor,
    rope_sin: &GpuTensor,
    tapped: bool,
) -> Result<(LayerOutput, Option<SkinTokensQwenLayerOperatorTap>)> {
    let beams = parents.len();
    let prefix = layer_prefix(layer);
    let kv_width = SKIN_TOKENS_QWEN_KV_HEADS * SKIN_TOKENS_QWEN_HEAD_DIM;
    if prior.key.cols() != kv_width
        || prior.value.cols() != kv_width
        || prior.key.rows() != prior_beams * prior_sequence
        || prior.value.rows() != prior_beams * prior_sequence
    {
        return Err(DiffusionError::workflow(format!(
            "SkinTokens Qwen layer {layer} has malformed beam K/V cache",
        )));
    }
    let normed = norm_bf16(
        &hidden,
        SKIN_TOKENS_QWEN_WIDTH,
        &format!("{prefix}.input_layernorm"),
        &prepared.input_norm[layer],
    )?;
    let input_norm = if tapped {
        gpu_download(&normed).map_err(DiffusionError::model)?
    } else {
        Vec::new()
    };
    let (q, key_step, value_step) = qkv_bf16(weights, &normed, &prefix)?;
    let query_projected = if tapped {
        gpu_download(&q).map_err(DiffusionError::model)?
    } else {
        Vec::new()
    };
    let key_projected = if tapped {
        gpu_download(&key_step).map_err(DiffusionError::model)?
    } else {
        Vec::new()
    };
    let value_projected = if tapped {
        gpu_download(&value_step).map_err(DiffusionError::model)?
    } else {
        Vec::new()
    };
    let q = norm_bf16(
        &q,
        SKIN_TOKENS_QWEN_HEAD_DIM,
        &format!("{prefix}.self_attn.q_norm"),
        &prepared.q_norm[layer],
    )?;
    let key_step = norm_bf16(
        &key_step,
        SKIN_TOKENS_QWEN_HEAD_DIM,
        &format!("{prefix}.self_attn.k_norm"),
        &prepared.k_norm[layer],
    )?;
    let query_normalized = if tapped {
        gpu_download(&q).map_err(DiffusionError::model)?
    } else {
        Vec::new()
    };
    let key_normalized = if tapped {
        gpu_download(&key_step).map_err(DiffusionError::model)?
    } else {
        Vec::new()
    };
    let q = gpu_rope_half_bf16(
        &q,
        SKIN_TOKENS_QWEN_HEADS,
        SKIN_TOKENS_QWEN_HEAD_DIM / 2,
        rope_cos,
        rope_sin,
    )
    .map_err(DiffusionError::model)?;
    let key_step = gpu_rope_half_bf16(
        &key_step,
        SKIN_TOKENS_QWEN_KV_HEADS,
        SKIN_TOKENS_QWEN_HEAD_DIM / 2,
        rope_cos,
        rope_sin,
    )
    .map_err(DiffusionError::model)?;
    let key = gpu_beam_cache_reorder_append(
        &prior.key,
        &key_step,
        parents,
        prior_beams,
        prior_sequence,
    )
    .map_err(DiffusionError::model)?;
    let value = gpu_beam_cache_reorder_append(
        &prior.value,
        &value_step,
        parents,
        prior_beams,
        prior_sequence,
    )
    .map_err(DiffusionError::model)?;
    debug_assert_eq!(key.rows(), beams * (prior_sequence + 1));
    let attention = gpu_attention_gqa_decode_bf16(
        &q,
        &key,
        &value,
        SKIN_TOKENS_QWEN_HEADS,
        SKIN_TOKENS_QWEN_KV_HEADS,
        1.0 / (SKIN_TOKENS_QWEN_HEAD_DIM as f32).sqrt(),
    )
    .map_err(DiffusionError::model)?;
    let attention = gpu_bf16_round(&attention).map_err(DiffusionError::model)?;
    let attention_raw = if tapped {
        gpu_download(&attention).map_err(DiffusionError::model)?
    } else {
        Vec::new()
    };
    let attention = linear_bf16(
        weights,
        &attention,
        &format!("{prefix}.self_attn.o_proj.weight"),
        SKIN_TOKENS_QWEN_WIDTH,
    )?;
    let attention_projected = if tapped {
        gpu_download(&attention).map_err(DiffusionError::model)?
    } else {
        Vec::new()
    };
    let residual = gpu_add_bf16(&hidden, &attention).map_err(DiffusionError::model)?;
    let attention_residual = if tapped {
        gpu_download(&residual).map_err(DiffusionError::model)?
    } else {
        Vec::new()
    };
    let normed = norm_bf16(
        &residual,
        SKIN_TOKENS_QWEN_WIDTH,
        &format!("{prefix}.post_attention_layernorm"),
        &prepared.post_attention_norm[layer],
    )?;
    let post_attention_norm = if tapped {
        gpu_download(&normed).map_err(DiffusionError::model)?
    } else {
        Vec::new()
    };
    let up_gate = up_gate_bf16(weights, &normed, &prefix)?;
    let up_projected_tensor =
        gpu_slice_cols(&up_gate, 0, SKIN_TOKENS_QWEN_FFN).map_err(DiffusionError::model)?;
    let gate_projected_tensor = gpu_slice_cols(
        &up_gate,
        SKIN_TOKENS_QWEN_FFN,
        SKIN_TOKENS_QWEN_FFN,
    )
    .map_err(DiffusionError::model)?;
    let up_projected = if tapped {
        gpu_download(&up_projected_tensor).map_err(DiffusionError::model)?
    } else {
        Vec::new()
    };
    let gate_projected = if tapped {
        gpu_download(&gate_projected_tensor).map_err(DiffusionError::model)?
    } else {
        Vec::new()
    };
    let activated = gpu_swiglu_value_gate(&up_gate).map_err(DiffusionError::model)?;
    let activated = gpu_bf16_round(&activated).map_err(DiffusionError::model)?;
    let mlp_activated = if tapped {
        gpu_download(&activated).map_err(DiffusionError::model)?
    } else {
        Vec::new()
    };
    let update = linear_bf16(
        weights,
        &activated,
        &format!("{prefix}.mlp.down_proj.weight"),
        SKIN_TOKENS_QWEN_WIDTH,
    )?;
    let down_projected = if tapped {
        gpu_download(&update).map_err(DiffusionError::model)?
    } else {
        Vec::new()
    };
    let hidden = gpu_add_bf16(&residual, &update).map_err(DiffusionError::model)?;
    let hidden_tap = if tapped {
        gpu_download(&hidden).map_err(DiffusionError::model)?
    } else {
        Vec::new()
    };
    Ok((
        LayerOutput { hidden, key, value },
        tapped.then(|| SkinTokensQwenLayerOperatorTap {
            input_norm,
            query_projected,
            key_projected,
            value_projected,
            query_normalized,
            key_normalized,
            attention_raw,
            attention_projected,
            attention_residual,
            post_attention_norm,
            gate_projected,
            up_projected,
            mlp_activated,
            down_projected,
            hidden: hidden_tap,
        }),
    ))
}

fn repeat_kv_heads(key: &GpuTensor, value: &GpuTensor) -> Result<(GpuTensor, GpuTensor)> {
    let group = SKIN_TOKENS_QWEN_HEADS / SKIN_TOKENS_QWEN_KV_HEADS;
    let mut key_heads = Vec::with_capacity(SKIN_TOKENS_QWEN_HEADS);
    let mut value_heads = Vec::with_capacity(SKIN_TOKENS_QWEN_HEADS);
    for q_head in 0..SKIN_TOKENS_QWEN_HEADS {
        let kv_head = q_head / group;
        key_heads.push(
            gpu_slice_cols(
                key,
                kv_head * SKIN_TOKENS_QWEN_HEAD_DIM,
                SKIN_TOKENS_QWEN_HEAD_DIM,
            )
            .map_err(DiffusionError::model)?,
        );
        value_heads.push(
            gpu_slice_cols(
                value,
                kv_head * SKIN_TOKENS_QWEN_HEAD_DIM,
                SKIN_TOKENS_QWEN_HEAD_DIM,
            )
            .map_err(DiffusionError::model)?,
        );
    }
    let key_refs: Vec<&GpuTensor> = key_heads.iter().collect();
    let value_refs: Vec<&GpuTensor> = value_heads.iter().collect();
    Ok((
        gpu_concat_cols(&key_refs).map_err(DiffusionError::model)?,
        gpu_concat_cols(&value_refs).map_err(DiffusionError::model)?,
    ))
}

fn rope_tables(
    prepared: &SkinTokensQwenPrepared,
    position_start: usize,
    rows: usize,
) -> Result<(GpuTensor, GpuTensor)> {
    let (cos, sin) = rope_table_values(prepared, position_start, rows);
    let half = SKIN_TOKENS_QWEN_HEAD_DIM / 2;
    Ok((
        gpu_upload(&cos, rows, half).map_err(DiffusionError::model)?,
        gpu_upload(&sin, rows, half).map_err(DiffusionError::model)?,
    ))
}

fn rope_table_values(
    prepared: &SkinTokensQwenPrepared,
    position_start: usize,
    rows: usize,
) -> (Vec<f32>, Vec<f32>) {
    let half = SKIN_TOKENS_QWEN_HEAD_DIM / 2;
    let mut cos = vec![0.0f32; rows * half];
    let mut sin = vec![0.0f32; rows * half];
    for row in 0..rows {
        let position = position_start + row;
        for index in 0..half {
            let angle = position as f32 * prepared.rope_inv_freq[index];
            // Transformers constructs RoPE in f32, then casts the tables to
            // the query dtype before rotation.
            cos[row * half + index] = round_to_bf16(angle.cos());
            sin[row * half + index] = round_to_bf16(angle.sin());
        }
    }
    (cos, sin)
}

fn round_to_bf16(value: f32) -> f32 {
    let bits = value.to_bits();
    let rounding_bias = 0x7fffu32 + ((bits >> 16) & 1);
    f32::from_bits(bits.wrapping_add(rounding_bias) & 0xffff_0000)
}

fn token_major_to_head_major(
    input: &[f32],
    rows: usize,
    heads: usize,
    head_dim: usize,
) -> Vec<f32> {
    debug_assert_eq!(input.len(), rows * heads * head_dim);
    let mut output = vec![0.0f32; input.len()];
    for head in 0..heads {
        for row in 0..rows {
            let source = (row * heads + head) * head_dim;
            let destination = (head * rows + row) * head_dim;
            output[destination..destination + head_dim]
                .copy_from_slice(&input[source..source + head_dim]);
        }
    }
    output
}

/// Drop only the streamed Qwen matrices. The broader SkinTokens unload uses
/// the parent `skin-tokens-tokenrig-bf16` prefix and therefore also catches
/// these qualified entries alongside the mesh/VAE encoders.
pub fn unload_skin_tokens_qwen_weights() -> Result<usize> {
    gpu_weight_cache_evict_prefix(&format!("{SKIN_TOKENS_QWEN_NAMESPACE}::"))
        .map_err(DiffusionError::model)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn released_qwen_geometry_is_self_consistent() {
        assert_eq!(SKIN_TOKENS_QWEN_HEADS * SKIN_TOKENS_QWEN_HEAD_DIM, 2048);
        assert_eq!(
            SKIN_TOKENS_QWEN_KV_HEADS * SKIN_TOKENS_QWEN_HEAD_DIM,
            1024
        );
        assert_eq!(
            SKIN_TOKENS_QWEN_HEADS % SKIN_TOKENS_QWEN_KV_HEADS,
            0
        );
        assert_eq!(SKIN_TOKENS_QWEN_HEAD_DIM % 2, 0);
    }

    #[test]
    fn bf16_rounding_is_ties_to_even() {
        assert_eq!(round_to_bf16(1.0), 1.0);
        assert_eq!(
            round_to_bf16(f32::from_bits(0x3f80_8000)).to_bits(),
            0x3f80_0000
        );
        assert_eq!(
            round_to_bf16(f32::from_bits(0x3f81_8000)).to_bits(),
            0x3f82_0000
        );
    }

    #[test]
    fn validation_cache_layout_matches_huggingface() {
        let rows = 3;
        let heads = 2;
        let dim = 2;
        let input: Vec<f32> = (0..rows * heads * dim).map(|v| v as f32).collect();
        assert_eq!(
            token_major_to_head_major(&input, rows, heads, dim),
            vec![0.0, 1.0, 4.0, 5.0, 8.0, 9.0, 2.0, 3.0, 6.0, 7.0, 10.0, 11.0]
        );
    }

    #[test]
    fn official_rope_frequency_buffer_is_bf16() {
        assert_eq!(
            round_to_bf16(1.0 / SKIN_TOKENS_QWEN_ROPE_THETA.powf(2.0 / 128.0)),
            0.8046875,
        );
    }

    #[test]
    fn production_generation_defaults_to_strict_usable_skin() {
        let params = SkinTokensGenerationParams::default();
        assert_eq!(params.grammar, SkinTokensGenerationGrammar::Strict);
        assert_eq!(params.max_length, 2_048);
        assert_eq!(params.top_k, 5);
        assert_eq!(params.top_p, 0.95);
        assert_eq!(params.temperature, 1.0);
        assert_eq!(params.repetition_penalty, 2.0);
        assert_eq!(params.num_beams, 10);
    }

    #[test]
    fn torch_cuda_philox_matches_official_seeded_stream() {
        assert_eq!(
            philox4x32_10(424_242, 128, 0),
            [0x6a56_c993, 0x7096_ab17, 0x2086_30bb, 0x57b9_6453],
        );
        assert_eq!(
            philox4x32_10(424_242, 129, 0),
            [0x1323_1230, 0x4fb8_aa4d, 0x2f1d_e0e8, 0x1468_7f63],
        );
        assert_eq!(
            philox4x32_10(424_242, 99, 5),
            [0xdf97_e276, 0x2b1c_b671, 0xfb31_32f9, 0xb0a9_3f8a],
        );
    }

    #[test]
    fn torch_cuda_multinomial_ranks_official_step_five() {
        // Full official Torch 2.7 CUDA trace at raw generation index five.
        // These are already ordered by `torch.multinomial`'s exponential
        // race; the two leading accumulated scores are close enough to catch
        // both a wrong flat-index mapping and a wrong Philox call offset.
        let flat_indices = [
            297_421, 132_240, 99, 66_170, 33_132, 231_349, 98, 198_313,
            99_206, 66_169, 99_207, 264_385, 132_241, 165_281, 198_312,
            33_133, 97, 165_278, 132_242, 165_279,
        ];
        let scores = [
            -6.416_754, -6.285_044_7, -5.430_873_4, -5.552_345,
            -6.069_917, -5.881_680_5, -5.180_873_4, -5.680_643,
            -5.488_97, -5.489_845, -5.738_97, -5.919_368_3,
            -5.597_544_7, -6.918_766_5, -5.930_643, -5.257_417,
            -5.555_873_4, -5.981_266_5, -5.785_044_7, -5.731_266_5,
        ];
        let mut rng = SkinTokensSamplingRng::new(424_242);
        for _ in 0..5 {
            rng.finish_torch_cuda_exponential(10 * SKIN_TOKENS_VOCAB);
        }
        let keys: Vec<f32> = flat_indices
            .iter()
            .zip(scores)
            .map(|(&index, score)| {
                score
                    - rng
                        .torch_cuda_exponential(index, 10 * SKIN_TOKENS_VOCAB)
                        .ln()
            })
            .collect();
        assert!(keys.windows(2).all(|pair| pair[0] > pair[1]), "{keys:?}");
    }

    #[test]
    fn torch_sampling_warp_keeps_topk_ties_and_two_beam_tokens() {
        let mut params = SkinTokensGenerationParams {
            top_k: 2,
            top_p: 1.0,
            num_beams: 10,
            ..SkinTokensGenerationParams::default()
        };
        let mut tied = vec![(0, 5.0), (1, 4.0), (2, 4.0), (3, 3.0)];
        torch_generation_warp(&mut tied, &params);
        assert_eq!(tied, [(0, 5.0), (1, 4.0), (2, 4.0)]);

        params.top_k = 0;
        params.top_p = 0.5;
        let mut concentrated = vec![(0, 0.0), (1, -4.0), (2, -8.0)];
        torch_generation_warp(&mut concentrated, &params);
        assert_eq!(concentrated, [(0, 0.0), (1, -4.0)]);
    }

    #[test]
    fn torch_cuda_running_topk_preserves_official_tie_order() {
        let scores = [
            -3.678_828_7,
            -3.214_670_7,
            -3.652_170_7,
            -3.287_267_4,
            -4.276_853_6,
            -4.053_828_7,
            -4.474_767_7,
            -5.413_535_6,
            -4.915_353_3,
            -4.800_722,
            -5.063_468,
            -3.828_35,
            -3.152_170_7,
            -3.652_170_7,
            -4.238_222,
            -4.265_85,
            -5.395_098_7,
            -3.349_767_4,
            -3.738_222,
            -3.787_267_4,
        ];
        let grammar = SkinTokensGrammar::from_tokens(&[
            SKIN_TOKENS_TOKEN_BOS,
            SKIN_TOKENS_TOKEN_CLASS_ARTICULATION_XL,
        ])
        .unwrap();
        let candidates: Vec<GenerationCandidate> = scores
            .into_iter()
            .enumerate()
            .map(|(index, score)| GenerationCandidate {
                parent: 0,
                token: index as u32,
                score,
                beam: GenerationBeam {
                    ids: Vec::new(),
                    grammar: grammar.clone(),
                    score,
                },
                finished: false,
                sample_key: 0.0,
            })
            .collect();
        let selected: Vec<u32> = torch_cuda_running_topk(candidates, 10)
            .into_iter()
            .map(|candidate| candidate.token)
            .collect();
        assert_eq!(selected, [12, 1, 3, 17, 13, 2, 0, 18, 19, 11]);
    }

    #[test]
    fn strict_generation_preserves_four_real_fsq_codes_per_bone() {
        let generated = vec![
            128,
            128,
            128,
            SKIN_TOKENS_TOKEN_SKELETON_EOS,
            SKIN_TOKENS_FSQ_OFFSET,
            SKIN_TOKENS_FSQ_OFFSET + 1,
            SKIN_TOKENS_FSQ_OFFSET + 2,
            SKIN_TOKENS_FSQ_OFFSET + 3,
            SKIN_TOKENS_TOKEN_GLOBAL_EOS,
        ];
        let mut full = vec![
            SKIN_TOKENS_TOKEN_BOS,
            SKIN_TOKENS_TOKEN_CLASS_ARTICULATION_XL,
        ];
        full.extend_from_slice(&generated);
        let result = finalize_generation(
            GenerationBeam {
                ids: generated.clone(),
                grammar: SkinTokensGrammar::from_tokens(&full).unwrap(),
                score: -1.25,
            },
            SkinTokensGenerationGrammar::Strict,
        )
        .unwrap();
        assert_eq!(result.generated_ids, generated);
        assert_eq!(result.full_ids, full);
        assert_eq!(result.skeleton_ids.len(), 6);
        assert_eq!(result.fsq_indices, vec![[0, 1, 2, 3]]);
        assert_eq!(result.grammar, SkinTokensGenerationGrammar::Strict);
    }

    #[test]
    fn official_compatibility_maps_only_the_missing_last_fsq_to_zero() {
        let generated = vec![
            128,
            128,
            128,
            SKIN_TOKENS_TOKEN_SKELETON_EOS,
            SKIN_TOKENS_FSQ_OFFSET,
            SKIN_TOKENS_FSQ_OFFSET + 1,
            SKIN_TOKENS_FSQ_OFFSET + 2,
            SKIN_TOKENS_TOKEN_GLOBAL_EOS,
        ];
        let mut grammar_ids = vec![
            SKIN_TOKENS_TOKEN_BOS,
            SKIN_TOKENS_TOKEN_CLASS_ARTICULATION_XL,
        ];
        grammar_ids.extend_from_slice(&generated[..generated.len() - 1]);
        let result = finalize_generation(
            GenerationBeam {
                ids: generated.clone(),
                grammar: SkinTokensGrammar::from_tokens(&grammar_ids).unwrap(),
                score: -2.5,
            },
            SkinTokensGenerationGrammar::OfficialOffByOneCompatibility,
        )
        .unwrap();
        assert_eq!(result.generated_ids, generated);
        assert_eq!(result.fsq_indices, vec![[0, 1, 2, 0]]);
        assert_eq!(
            result.grammar,
            SkinTokensGenerationGrammar::OfficialOffByOneCompatibility,
        );
    }
}
