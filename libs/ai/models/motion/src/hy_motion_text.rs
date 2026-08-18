//! Native HY-Motion text conditioning.
//!
//! The full 1.0 checkpoint uses Qwen3-8B's final normalized hidden state for
//! token context. Weight tensors stream from sharded safetensors into the CUDA
//! cache and never require a whole-model host copy. Precision is a per-run
//! contract rather than an implicit image-diffusion environment setting.

use std::path::Path;

use crate::backend::{
    gpu_add, gpu_add_bf16, gpu_attention_packed_causal_bf16,
    gpu_attention_packed_causal_f16, gpu_bf16_round, gpu_concat_cols, gpu_download,
    gpu_linear_nt_cached_bf16_f32acc, gpu_linear_nt_cached_f16_f32acc, gpu_rms_norm_mul,
    gpu_rms_norm_mul_bf16, gpu_rope_half, gpu_slice_cols, gpu_swiglu_value_gate, gpu_upload,
    gpu_weight_cache_ensure, gpu_weight_cache_evict_prefix, GpuLinearPart, GpuTensor,
};
use makepad_ai_h3::h3::H3ShardedWeights;
use makepad_ai_h3::h3_tokenizer::H3Tokenizer;
use crate::hy_motion::HY_MOTION_TEXT_TOKENS;
use crate::{DiffusionError, Result};
use makepad_ggml::quant::GGML_TYPE_BF16;
use makepad_ai_loader::MlxDType;

pub const HY_MOTION_QWEN_NAMESPACE: &str = "hy-motion-qwen3-8b";
pub const HY_MOTION_QWEN_VOCAB: usize = 151_936;
pub const HY_MOTION_QWEN_HIDDEN: usize = 4096;
pub const HY_MOTION_QWEN_LAYERS: usize = 36;
pub const HY_MOTION_QWEN_Q_HEADS: usize = 32;
pub const HY_MOTION_QWEN_KV_HEADS: usize = 8;
pub const HY_MOTION_QWEN_HEAD_DIM: usize = 128;
pub const HY_MOTION_QWEN_FFN: usize = 12_288;
pub const HY_MOTION_QWEN_ROPE_THETA: f32 = 1_000_000.0;
pub const HY_MOTION_QWEN_RMS_EPS: f32 = 1.0e-6;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HyMotionQwenPrecision {
    /// BF16 operands and f32 accumulation with explicit BF16 operator
    /// boundaries, closest to the released PyTorch dtype contract.
    Bf16,
    /// F16 tensor-core operands with f32 accumulation/output. This is the
    /// validated HY-Motion production contract: fixed-seed context cosine
    /// 0.9998866 and downstream sampled-latent cosine 0.9999861.
    F16,
}

/// Verbatim released HY-Motion system instruction, including its leading and
/// trailing newline. Those whitespace tokens affect every causal hidden row.
pub const HY_MOTION_SYSTEM_PROMPT: &str = "\n    Summarize human motion only from the user text for representation: action categories, key body-part movements, order/transitions, trajectory/direction, posture; include style/emotion/speed only if present. Explicitly capture laterality (left/right) when mentioned; do not guess. If multiple actions are described, indicate the count of distinct actions (e.g., actions=3) and their order. Do not invent missing info. Keep one concise paragraph.\n";

pub fn hy_motion_render_qwen_prompt(prompt: &str) -> String {
    format!(
        "<|im_start|>system\n{HY_MOTION_SYSTEM_PROMPT}<|im_end|>\n<|im_start|>user\n{prompt}<|im_end|>\n"
    )
}

pub struct HyMotionQwenTokenizer {
    tokenizer: H3Tokenizer,
    crop_start: usize,
    pad_token_id: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HyMotionQwenTokens {
    /// Complete causal prefix: system instruction, role wrappers, prompt and
    /// trailing `<|im_end|>`/newline.
    pub input_ids: Vec<u32>,
    /// First row retained by the HY-Motion context crop.
    pub crop_start: usize,
    /// Real rows after the crop; no right-padding rows are included.
    pub text_tokens: usize,
    /// Qwen's right-padding token. Execution retains the reference's padded
    /// 229-row GEMM/SDPA shapes for numeric parity, then crops real rows.
    pub pad_token_id: u32,
}

impl HyMotionQwenTokenizer {
    pub fn load(model_dir: impl AsRef<Path>) -> Result<Self> {
        let tokenizer = H3Tokenizer::load(model_dir.as_ref())?;
        for token in ["<|im_start|>", "<|im_end|>"] {
            if tokenizer.token_id(token).is_none() {
                return Err(DiffusionError::model(format!(
                    "HY-Motion Qwen tokenizer is missing {token}"
                )));
            }
        }
        let marker = "<BOC>";
        let marker_full = tokenizer.encode(&hy_motion_render_qwen_prompt(marker));
        let marker_ids = tokenizer.encode(marker);
        let crop_start = find_subsequence(&marker_full, &marker_ids).ok_or_else(|| {
            DiffusionError::model("HY-Motion Qwen crop marker was not found in chat template")
        })?;
        let pad_token_id = tokenizer.token_id("<|endoftext|>").ok_or_else(|| {
            DiffusionError::model("HY-Motion Qwen tokenizer is missing <|endoftext|>")
        })?;
        Ok(Self {
            tokenizer,
            crop_start,
            pad_token_id,
        })
    }

    pub fn crop_start(&self) -> usize {
        self.crop_start
    }

    pub fn tokenize(&self, prompt: &str) -> Result<HyMotionQwenTokens> {
        let mut input_ids = self.tokenizer.encode(&hy_motion_render_qwen_prompt(prompt));
        let max_full_tokens = self
            .crop_start
            .checked_add(HY_MOTION_TEXT_TOKENS)
            .ok_or_else(|| DiffusionError::workflow("HY-Motion Qwen token limit overflow"))?;
        input_ids.truncate(max_full_tokens);
        if input_ids.len() <= self.crop_start {
            return Err(DiffusionError::workflow(
                "HY-Motion prompt produced no context tokens after template crop",
            ));
        }
        let text_tokens = input_ids.len() - self.crop_start;
        Ok(HyMotionQwenTokens {
            input_ids,
            crop_start: self.crop_start,
            text_tokens,
            pad_token_id: self.pad_token_id,
        })
    }
}

fn find_subsequence(haystack: &[u32], needle: &[u32]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn round_to_bf16(value: f32) -> f32 {
    let bits = value.to_bits();
    let rounding_bias = 0x7fffu32 + ((bits >> 16) & 1);
    f32::from_bits(bits.wrapping_add(rounding_bias) & 0xffff_0000)
}

fn layer_prefix(layer: usize) -> String {
    format!("model.layers.{layer}")
}

fn require_bf16_shape(
    weights: &H3ShardedWeights,
    name: &str,
    expected: &[u64],
) -> Result<()> {
    let (dtype, shape) = weights.tensor_dtype_shape(name)?;
    if dtype != MlxDType::BF16 || shape != expected {
        return Err(DiffusionError::model(format!(
            "HY-Motion Qwen tensor {name}: got {dtype:?} {shape:?}, expected BF16 {expected:?}"
        )));
    }
    Ok(())
}

fn ensure_linear<'a>(
    weights: &H3ShardedWeights,
    name: &'a str,
    n: usize,
    k: usize,
    precision: HyMotionQwenPrecision,
) -> Result<GpuLinearPart<'a>> {
    let want_a16 = precision == HyMotionQwenPrecision::F16;
    gpu_weight_cache_ensure(
        HY_MOTION_QWEN_NAMESPACE,
        name,
        GGML_TYPE_BF16,
        n,
        k,
        want_a16,
        || weights.tensor_bytes(name).map_err(|error| error.to_string()),
    )
    .map_err(DiffusionError::model)?;
    Ok(GpuLinearPart {
        bt_ggml_type: GGML_TYPE_BF16,
        n,
        cache_key: name,
        bytes: &[],
    })
}

fn linear_for_precision(
    weights: &H3ShardedWeights,
    input: &GpuTensor,
    name: &str,
    output_cols: usize,
    precision: HyMotionQwenPrecision,
) -> Result<GpuTensor> {
    let part = ensure_linear(weights, name, output_cols, input.cols(), precision)?;
    match precision {
        HyMotionQwenPrecision::Bf16 => {
            let output = gpu_linear_nt_cached_bf16_f32acc(
                input,
                HY_MOTION_QWEN_NAMESPACE,
                &[part],
                &[],
            )
            .map_err(DiffusionError::model)?;
            gpu_bf16_round(&output).map_err(DiffusionError::model)
        }
        HyMotionQwenPrecision::F16 => gpu_linear_nt_cached_f16_f32acc(
            input,
            HY_MOTION_QWEN_NAMESPACE,
            &[part],
            &[],
        )
        .map_err(DiffusionError::model),
    }
}

/// Small host constants and model contract needed by repeated Qwen encodes.
pub struct HyMotionQwenPrepared {
    input_norm: Vec<Vec<f32>>,
    post_attention_norm: Vec<Vec<f32>>,
    q_norm: Vec<Vec<f32>>,
    k_norm: Vec<Vec<f32>>,
    final_norm: Vec<f32>,
    rope_inv_freq: Vec<f32>,
}

impl HyMotionQwenPrepared {
    pub fn prepare(weights: &H3ShardedWeights) -> Result<Self> {
        require_bf16_shape(
            weights,
            "model.embed_tokens.weight",
            &[HY_MOTION_QWEN_VOCAB as u64, HY_MOTION_QWEN_HIDDEN as u64],
        )?;
        require_bf16_shape(
            weights,
            "model.norm.weight",
            &[HY_MOTION_QWEN_HIDDEN as u64],
        )?;
        let mut input_norm = Vec::with_capacity(HY_MOTION_QWEN_LAYERS);
        let mut post_attention_norm = Vec::with_capacity(HY_MOTION_QWEN_LAYERS);
        let mut q_norm = Vec::with_capacity(HY_MOTION_QWEN_LAYERS);
        let mut k_norm = Vec::with_capacity(HY_MOTION_QWEN_LAYERS);
        for layer in 0..HY_MOTION_QWEN_LAYERS {
            let prefix = layer_prefix(layer);
            for (suffix, shape) in [
                ("input_layernorm.weight", vec![HY_MOTION_QWEN_HIDDEN as u64]),
                (
                    "post_attention_layernorm.weight",
                    vec![HY_MOTION_QWEN_HIDDEN as u64],
                ),
                ("self_attn.q_norm.weight", vec![HY_MOTION_QWEN_HEAD_DIM as u64]),
                ("self_attn.k_norm.weight", vec![HY_MOTION_QWEN_HEAD_DIM as u64]),
                (
                    "self_attn.q_proj.weight",
                    vec![
                        (HY_MOTION_QWEN_Q_HEADS * HY_MOTION_QWEN_HEAD_DIM) as u64,
                        HY_MOTION_QWEN_HIDDEN as u64,
                    ],
                ),
                (
                    "self_attn.k_proj.weight",
                    vec![
                        (HY_MOTION_QWEN_KV_HEADS * HY_MOTION_QWEN_HEAD_DIM) as u64,
                        HY_MOTION_QWEN_HIDDEN as u64,
                    ],
                ),
                (
                    "self_attn.v_proj.weight",
                    vec![
                        (HY_MOTION_QWEN_KV_HEADS * HY_MOTION_QWEN_HEAD_DIM) as u64,
                        HY_MOTION_QWEN_HIDDEN as u64,
                    ],
                ),
                (
                    "self_attn.o_proj.weight",
                    vec![HY_MOTION_QWEN_HIDDEN as u64, HY_MOTION_QWEN_HIDDEN as u64],
                ),
                (
                    "mlp.gate_proj.weight",
                    vec![HY_MOTION_QWEN_FFN as u64, HY_MOTION_QWEN_HIDDEN as u64],
                ),
                (
                    "mlp.up_proj.weight",
                    vec![HY_MOTION_QWEN_FFN as u64, HY_MOTION_QWEN_HIDDEN as u64],
                ),
                (
                    "mlp.down_proj.weight",
                    vec![HY_MOTION_QWEN_HIDDEN as u64, HY_MOTION_QWEN_FFN as u64],
                ),
            ] {
                require_bf16_shape(weights, &format!("{prefix}.{suffix}"), &shape)?;
            }
            input_norm.push(weights.tensor_f32(&format!(
                "{prefix}.input_layernorm.weight"
            ))?);
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
        let final_norm = weights.tensor_f32("model.norm.weight")?;
        let half = HY_MOTION_QWEN_HEAD_DIM / 2;
        let rope_inv_freq = (0..half)
            .map(|index| {
                1.0 / HY_MOTION_QWEN_ROPE_THETA
                    .powf(2.0 * index as f32 / HY_MOTION_QWEN_HEAD_DIM as f32)
            })
            .collect();
        Ok(Self {
            input_norm,
            post_attention_norm,
            q_norm,
            k_norm,
            final_norm,
            rope_inv_freq,
        })
    }
}

#[derive(Clone, Debug)]
pub struct HyMotionQwenRun {
    /// Cropped real context rows in token-major order (`text_tokens x 4096`).
    pub context: Vec<f32>,
    pub input_ids: Vec<u32>,
    pub crop_start: usize,
    pub text_tokens: usize,
}

/// Validation-only full-prefix hidden state at a HuggingFace-compatible
/// hidden-state index: 0 is embeddings, 1..35 are layer outputs 0..34, and
/// 36 is the final normalized output after layer 35.
#[derive(Clone, Debug)]
pub struct HyMotionQwenTap {
    pub stage: usize,
    pub hidden_states: Vec<f32>,
}

#[derive(Clone, Debug)]
pub struct HyMotionQwenOperatorTap {
    pub name: &'static str,
    pub rows: usize,
    pub cols: usize,
    pub values: Vec<f32>,
}

pub fn hy_motion_qwen_encode(
    weights: &H3ShardedWeights,
    prepared: &HyMotionQwenPrepared,
    tokens: &HyMotionQwenTokens,
) -> Result<HyMotionQwenRun> {
    hy_motion_qwen_encode_precision(
        weights,
        prepared,
        tokens,
        HyMotionQwenPrecision::F16,
    )
}

pub fn hy_motion_qwen_encode_precision(
    weights: &H3ShardedWeights,
    prepared: &HyMotionQwenPrepared,
    tokens: &HyMotionQwenTokens,
    precision: HyMotionQwenPrecision,
) -> Result<HyMotionQwenRun> {
    hy_motion_qwen_encode_inner(weights, prepared, tokens, None, &[], false, precision)
        .map(|(run, _, _)| run)
}

pub fn hy_motion_qwen_encode_progress(
    weights: &H3ShardedWeights,
    prepared: &HyMotionQwenPrepared,
    tokens: &HyMotionQwenTokens,
    mut on_layer: Option<&mut dyn FnMut(usize, usize)>,
) -> Result<HyMotionQwenRun> {
    let mut adapter = |done: usize, total: usize| {
        if let Some(callback) = on_layer.as_deref_mut() {
            callback(done, total);
        }
        Ok(())
    };
    hy_motion_qwen_encode_inner(
        weights,
        prepared,
        tokens,
        Some(&mut adapter),
        &[],
        false,
        HyMotionQwenPrecision::F16,
    )
    .map(|(run, _, _)| run)
}

/// Production F16 encode with a fallible per-layer callback. Returning an
/// error (normally [`DiffusionError::Cancelled`]) stops before the next
/// decoder layer.
pub fn hy_motion_qwen_encode_controlled(
    weights: &H3ShardedWeights,
    prepared: &HyMotionQwenPrepared,
    tokens: &HyMotionQwenTokens,
    on_layer: Option<&mut dyn FnMut(usize, usize) -> Result<()>>,
) -> Result<HyMotionQwenRun> {
    hy_motion_qwen_encode_inner(
        weights,
        prepared,
        tokens,
        on_layer,
        &[],
        false,
        HyMotionQwenPrecision::F16,
    )
    .map(|(run, _, _)| run)
}

pub fn hy_motion_qwen_encode_tapped(
    weights: &H3ShardedWeights,
    prepared: &HyMotionQwenPrepared,
    tokens: &HyMotionQwenTokens,
    tap_stages: &[usize],
    precision: HyMotionQwenPrecision,
) -> Result<(
    HyMotionQwenRun,
    Vec<HyMotionQwenTap>,
    Vec<HyMotionQwenOperatorTap>,
)> {
    if tap_stages.iter().any(|&stage| stage > HY_MOTION_QWEN_LAYERS) {
        return Err(DiffusionError::workflow(
            "HY-Motion Qwen tap stage exceeds final hidden-state index",
        ));
    }
    hy_motion_qwen_encode_inner(weights, prepared, tokens, None, tap_stages, true, precision)
}

fn hy_motion_qwen_encode_inner(
    weights: &H3ShardedWeights,
    prepared: &HyMotionQwenPrepared,
    tokens: &HyMotionQwenTokens,
    mut on_layer: Option<&mut dyn FnMut(usize, usize) -> Result<()>>,
    tap_stages: &[usize],
    capture_layer0_ops: bool,
    precision: HyMotionQwenPrecision,
) -> Result<(
    HyMotionQwenRun,
    Vec<HyMotionQwenTap>,
    Vec<HyMotionQwenOperatorTap>,
)> {
    let real_sequence = tokens.input_ids.len();
    let sequence = tokens.crop_start + HY_MOTION_TEXT_TOKENS;
    if real_sequence == 0
        || tokens.crop_start + tokens.text_tokens > real_sequence
        || real_sequence > sequence
        || tokens.text_tokens == 0
        || tokens.text_tokens > HY_MOTION_TEXT_TOKENS
    {
        return Err(DiffusionError::workflow(
            "HY-Motion Qwen token/crop shape mismatch",
        ));
    }

    let mut execution_ids = tokens.input_ids.clone();
    execution_ids.resize(sequence, tokens.pad_token_id);
    let mut embeddings = vec![0.0f32; sequence * HY_MOTION_QWEN_HIDDEN];
    for (row, &token) in execution_ids.iter().enumerate() {
        let values = weights.tensor_row_f32("model.embed_tokens.weight", token as u64)?;
        if values.len() != HY_MOTION_QWEN_HIDDEN {
            return Err(DiffusionError::model(format!(
                "HY-Motion Qwen embedding row {token} has {} values",
                values.len()
            )));
        }
        embeddings[row * HY_MOTION_QWEN_HIDDEN..(row + 1) * HY_MOTION_QWEN_HIDDEN]
            .copy_from_slice(&values);
    }
    let mut hidden = gpu_upload(&embeddings, sequence, HY_MOTION_QWEN_HIDDEN)
        .map_err(DiffusionError::model)?;
    let mut taps = Vec::with_capacity(tap_stages.len());
    if tap_stages.contains(&0) {
        taps.push(HyMotionQwenTap {
            stage: 0,
            hidden_states: embeddings.clone(),
        });
    }

    let half = HY_MOTION_QWEN_HEAD_DIM / 2;
    let mut cos = vec![0.0f32; sequence * half];
    let mut sin = vec![0.0f32; sequence * half];
    for position in 0..sequence {
        for index in 0..half {
            let angle = position as f32 * prepared.rope_inv_freq[index];
            // Transformers casts Qwen's RoPE tables to the BF16 query dtype
            // before applying rotation. Preserve that boundary even though
            // the native device API stores activations in f32 buffers.
            cos[position * half + index] = match precision {
                HyMotionQwenPrecision::Bf16 => round_to_bf16(angle.cos()),
                HyMotionQwenPrecision::F16 => angle.cos(),
            };
            sin[position * half + index] = match precision {
                HyMotionQwenPrecision::Bf16 => round_to_bf16(angle.sin()),
                HyMotionQwenPrecision::F16 => angle.sin(),
            };
        }
    }
    let rope_cos = gpu_upload(&cos, sequence, half).map_err(DiffusionError::model)?;
    let rope_sin = gpu_upload(&sin, sequence, half).map_err(DiffusionError::model)?;
    let mut operator_taps = Vec::new();

    for layer in 0..HY_MOTION_QWEN_LAYERS {
        if let Some(callback) = on_layer.as_deref_mut() {
            callback(layer + 1, HY_MOTION_QWEN_LAYERS)?;
        }
        let debug = if layer == 0 && capture_layer0_ops {
            Some(&mut operator_taps)
        } else {
            None
        };
        hidden = qwen_layer(
            weights,
            prepared,
            layer,
            hidden,
            &rope_cos,
            &rope_sin,
            sequence,
            debug,
            precision,
        )?;
        // HuggingFace records the first 35 raw decoder-layer outputs, then
        // replaces index 36 with the final normalized layer-35 output.
        let stage = layer + 1;
        if stage < HY_MOTION_QWEN_LAYERS && tap_stages.contains(&stage) {
            taps.push(HyMotionQwenTap {
                stage,
                hidden_states: gpu_download(&hidden).map_err(DiffusionError::model)?,
            });
        }
    }
    let hidden = norm_for_precision(
        &hidden,
        HY_MOTION_QWEN_HIDDEN,
        "model.norm",
        &prepared.final_norm,
        precision,
    )?;
    let hidden = gpu_download(&hidden).map_err(DiffusionError::model)?;
    if tap_stages.contains(&HY_MOTION_QWEN_LAYERS) {
        taps.push(HyMotionQwenTap {
            stage: HY_MOTION_QWEN_LAYERS,
            hidden_states: hidden.clone(),
        });
    }
    let start = tokens.crop_start * HY_MOTION_QWEN_HIDDEN;
    let end = start + tokens.text_tokens * HY_MOTION_QWEN_HIDDEN;
    Ok((
        HyMotionQwenRun {
            context: hidden[start..end].to_vec(),
            input_ids: tokens.input_ids.clone(),
            crop_start: tokens.crop_start,
            text_tokens: tokens.text_tokens,
        },
        taps,
        operator_taps,
    ))
}

fn tap_operator(
    taps: &mut Option<&mut Vec<HyMotionQwenOperatorTap>>,
    name: &'static str,
    tensor: &GpuTensor,
) -> Result<()> {
    if let Some(taps) = taps.as_deref_mut() {
        taps.push(HyMotionQwenOperatorTap {
            name,
            rows: tensor.rows(),
            cols: tensor.cols(),
            values: gpu_download(tensor).map_err(DiffusionError::model)?,
        });
    }
    Ok(())
}

fn norm_for_precision(
    input: &GpuTensor,
    group_cols: usize,
    cache_key: &str,
    scale: &[f32],
    precision: HyMotionQwenPrecision,
) -> Result<GpuTensor> {
    match precision {
        HyMotionQwenPrecision::Bf16 => gpu_rms_norm_mul_bf16(
            input,
            group_cols,
            HY_MOTION_QWEN_NAMESPACE,
            cache_key,
            scale,
            HY_MOTION_QWEN_RMS_EPS,
        ),
        HyMotionQwenPrecision::F16 => gpu_rms_norm_mul(
            input,
            group_cols,
            HY_MOTION_QWEN_NAMESPACE,
            cache_key,
            scale,
            HY_MOTION_QWEN_RMS_EPS,
        ),
    }
    .map_err(DiffusionError::model)
}

fn round_for_precision(
    tensor: GpuTensor,
    precision: HyMotionQwenPrecision,
) -> Result<GpuTensor> {
    match precision {
        HyMotionQwenPrecision::Bf16 => gpu_bf16_round(&tensor).map_err(DiffusionError::model),
        HyMotionQwenPrecision::F16 => Ok(tensor),
    }
}

fn add_for_precision(
    left: &GpuTensor,
    right: &GpuTensor,
    precision: HyMotionQwenPrecision,
) -> Result<GpuTensor> {
    match precision {
        HyMotionQwenPrecision::Bf16 => gpu_add_bf16(left, right),
        HyMotionQwenPrecision::F16 => gpu_add(left, right),
    }
    .map_err(DiffusionError::model)
}

fn qwen_layer(
    weights: &H3ShardedWeights,
    prepared: &HyMotionQwenPrepared,
    layer: usize,
    hidden: GpuTensor,
    rope_cos: &GpuTensor,
    rope_sin: &GpuTensor,
    sequence: usize,
    mut debug: Option<&mut Vec<HyMotionQwenOperatorTap>>,
    precision: HyMotionQwenPrecision,
) -> Result<GpuTensor> {
    let prefix = layer_prefix(layer);
    let q_width = HY_MOTION_QWEN_Q_HEADS * HY_MOTION_QWEN_HEAD_DIM;
    let kv_width = HY_MOTION_QWEN_KV_HEADS * HY_MOTION_QWEN_HEAD_DIM;
    let normed = norm_for_precision(
        &hidden,
        HY_MOTION_QWEN_HIDDEN,
        &format!("{prefix}.input_layernorm"),
        &prepared.input_norm[layer],
        precision,
    )?;
    tap_operator(&mut debug, "l0_input_norm", &normed)?;

    let q_name = format!("{prefix}.self_attn.q_proj.weight");
    let k_name = format!("{prefix}.self_attn.k_proj.weight");
    let v_name = format!("{prefix}.self_attn.v_proj.weight");
    let qkv_parts = [
        ensure_linear(weights, &q_name, q_width, HY_MOTION_QWEN_HIDDEN, precision)?,
        ensure_linear(weights, &k_name, kv_width, HY_MOTION_QWEN_HIDDEN, precision)?,
        ensure_linear(weights, &v_name, kv_width, HY_MOTION_QWEN_HIDDEN, precision)?,
    ];
    let qkv = match precision {
        HyMotionQwenPrecision::Bf16 => gpu_linear_nt_cached_bf16_f32acc(
            &normed,
            HY_MOTION_QWEN_NAMESPACE,
            &qkv_parts,
            &[],
        ),
        HyMotionQwenPrecision::F16 => gpu_linear_nt_cached_f16_f32acc(
            &normed,
            HY_MOTION_QWEN_NAMESPACE,
            &qkv_parts,
            &[],
        ),
    }
    .map_err(DiffusionError::model)?;
    let qkv = round_for_precision(qkv, precision)?;
    let q = gpu_slice_cols(&qkv, 0, q_width).map_err(DiffusionError::model)?;
    let k = gpu_slice_cols(&qkv, q_width, kv_width).map_err(DiffusionError::model)?;
    let v = gpu_slice_cols(&qkv, q_width + kv_width, kv_width)
        .map_err(DiffusionError::model)?;
    tap_operator(&mut debug, "l0_q_proj", &q)?;
    tap_operator(&mut debug, "l0_k_proj", &k)?;
    tap_operator(&mut debug, "l0_v_proj", &v)?;
    let q = norm_for_precision(
        &q,
        HY_MOTION_QWEN_HEAD_DIM,
        &format!("{prefix}.self_attn.q_norm"),
        &prepared.q_norm[layer],
        precision,
    )?;
    tap_operator(&mut debug, "l0_q_norm", &q)?;
    let k = norm_for_precision(
        &k,
        HY_MOTION_QWEN_HEAD_DIM,
        &format!("{prefix}.self_attn.k_norm"),
        &prepared.k_norm[layer],
        precision,
    )?;
    tap_operator(&mut debug, "l0_k_norm", &k)?;
    let q = gpu_rope_half(&q, HY_MOTION_QWEN_Q_HEADS, half_dim(), rope_cos, rope_sin)
        .map_err(DiffusionError::model)?;
    let q = round_for_precision(q, precision)?;
    let k = gpu_rope_half(&k, HY_MOTION_QWEN_KV_HEADS, half_dim(), rope_cos, rope_sin)
        .map_err(DiffusionError::model)?;
    let k = round_for_precision(k, precision)?;

    let group = HY_MOTION_QWEN_Q_HEADS / HY_MOTION_QWEN_KV_HEADS;
    let mut k_heads = Vec::with_capacity(HY_MOTION_QWEN_Q_HEADS);
    let mut v_heads = Vec::with_capacity(HY_MOTION_QWEN_Q_HEADS);
    for q_head in 0..HY_MOTION_QWEN_Q_HEADS {
        let kv_head = q_head / group;
        k_heads.push(
            gpu_slice_cols(&k, kv_head * HY_MOTION_QWEN_HEAD_DIM, HY_MOTION_QWEN_HEAD_DIM)
                .map_err(DiffusionError::model)?,
        );
        v_heads.push(
            gpu_slice_cols(&v, kv_head * HY_MOTION_QWEN_HEAD_DIM, HY_MOTION_QWEN_HEAD_DIM)
                .map_err(DiffusionError::model)?,
        );
    }
    let k_refs: Vec<&GpuTensor> = k_heads.iter().collect();
    let v_refs: Vec<&GpuTensor> = v_heads.iter().collect();
    let k = gpu_concat_cols(&k_refs).map_err(DiffusionError::model)?;
    let v = gpu_concat_cols(&v_refs).map_err(DiffusionError::model)?;
    let attention = match precision {
        HyMotionQwenPrecision::Bf16 => gpu_attention_packed_causal_bf16(
            &q,
            &k,
            &v,
            HY_MOTION_QWEN_Q_HEADS,
            1.0 / (HY_MOTION_QWEN_HEAD_DIM as f32).sqrt(),
        ),
        HyMotionQwenPrecision::F16 => gpu_attention_packed_causal_f16(
            &q,
            &k,
            &v,
            HY_MOTION_QWEN_Q_HEADS,
            1.0 / (HY_MOTION_QWEN_HEAD_DIM as f32).sqrt(),
        ),
    }
    .map_err(DiffusionError::model)?;
    let attention = round_for_precision(attention, precision)?;
    tap_operator(&mut debug, "l0_attention_raw", &attention)?;
    let attention = linear_for_precision(
        weights,
        &attention,
        &format!("{prefix}.self_attn.o_proj.weight"),
        HY_MOTION_QWEN_HIDDEN,
        precision,
    )?;
    tap_operator(&mut debug, "l0_o_proj", &attention)?;
    let hidden = add_for_precision(&hidden, &attention, precision)?;

    let normed = norm_for_precision(
        &hidden,
        HY_MOTION_QWEN_HIDDEN,
        &format!("{prefix}.post_attention_layernorm"),
        &prepared.post_attention_norm[layer],
        precision,
    )?;
    tap_operator(&mut debug, "l0_post_attention_norm", &normed)?;
    let up_name = format!("{prefix}.mlp.up_proj.weight");
    let gate_name = format!("{prefix}.mlp.gate_proj.weight");
    let mlp_parts = [
        ensure_linear(
            weights,
            &up_name,
            HY_MOTION_QWEN_FFN,
            HY_MOTION_QWEN_HIDDEN,
            precision,
        )?,
        ensure_linear(
            weights,
            &gate_name,
            HY_MOTION_QWEN_FFN,
            HY_MOTION_QWEN_HIDDEN,
            precision,
        )?,
    ];
    let up_gate = match precision {
        HyMotionQwenPrecision::Bf16 => gpu_linear_nt_cached_bf16_f32acc(
            &normed,
            HY_MOTION_QWEN_NAMESPACE,
            &mlp_parts,
            &[],
        ),
        HyMotionQwenPrecision::F16 => gpu_linear_nt_cached_f16_f32acc(
            &normed,
            HY_MOTION_QWEN_NAMESPACE,
            &mlp_parts,
            &[],
        ),
    }
    .map_err(DiffusionError::model)?;
    let up_gate = round_for_precision(up_gate, precision)?;
    if debug.is_some() {
        let up =
            gpu_slice_cols(&up_gate, 0, HY_MOTION_QWEN_FFN).map_err(DiffusionError::model)?;
        let gate = gpu_slice_cols(&up_gate, HY_MOTION_QWEN_FFN, HY_MOTION_QWEN_FFN)
            .map_err(DiffusionError::model)?;
        tap_operator(&mut debug, "l0_up_proj", &up)?;
        tap_operator(&mut debug, "l0_gate_proj", &gate)?;
    }
    let activated = gpu_swiglu_value_gate(&up_gate).map_err(DiffusionError::model)?;
    let activated = round_for_precision(activated, precision)?;
    tap_operator(&mut debug, "l0_mlp_activated", &activated)?;
    let update = linear_for_precision(
        weights,
        &activated,
        &format!("{prefix}.mlp.down_proj.weight"),
        HY_MOTION_QWEN_HIDDEN,
        precision,
    )?;
    tap_operator(&mut debug, "l0_down_proj", &update)?;
    let output = add_for_precision(&hidden, &update, precision)?;
    tap_operator(&mut debug, "l0_output", &output)?;
    debug_assert_eq!(output.rows(), sequence);
    Ok(output)
}

const fn half_dim() -> usize {
    HY_MOTION_QWEN_HEAD_DIM / 2
}

pub fn hy_motion_qwen_evict() -> Result<usize> {
    gpu_weight_cache_evict_prefix(&format!("{HY_MOTION_QWEN_NAMESPACE}::"))
        .map_err(DiffusionError::model)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn released_chat_template_is_byte_exact() {
        let rendered = hy_motion_render_qwen_prompt(
            "A person walks forward naturally at a steady pace.",
        );
        assert!(rendered.starts_with("<|im_start|>system\n\n    Summarize"));
        assert!(rendered.ends_with(
            "<|im_start|>user\nA person walks forward naturally at a steady pace.<|im_end|>\n"
        ));
        assert_eq!(rendered.matches("<|im_start|>").count(), 2);
        assert_eq!(rendered.matches("<|im_end|>").count(), 2);
    }

    #[test]
    fn full_qwen_contract_matches_context_width() {
        assert_eq!(
            HY_MOTION_QWEN_HIDDEN,
            crate::hy_motion::HY_MOTION_CONTEXT_DIM
        );
        assert_eq!(HY_MOTION_QWEN_Q_HEADS * HY_MOTION_QWEN_HEAD_DIM, 4096);
        assert_eq!(HY_MOTION_QWEN_KV_HEADS * HY_MOTION_QWEN_HEAD_DIM, 1024);
        assert_eq!(half_dim(), 64);
    }

    #[test]
    fn subsequence_finds_crop_marker() {
        assert_eq!(find_subsequence(&[1, 2, 3, 2, 3], &[2, 3]), Some(1));
        assert_eq!(find_subsequence(&[1, 2], &[3]), None);
    }

    #[test]
    fn bf16_rounding_is_ties_to_even() {
        assert_eq!(round_to_bf16(1.0), 1.0);
        assert_eq!(round_to_bf16(f32::from_bits(0x3f80_8000)).to_bits(), 0x3f80_0000);
        assert_eq!(round_to_bf16(f32::from_bits(0x3f81_8000)).to_bits(), 0x3f82_0000);
    }
}
