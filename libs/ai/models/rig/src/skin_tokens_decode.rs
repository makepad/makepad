//! Native SkinTokens FSQ and per-joint skin-weight decoder.
//!
//! The neural output is one independent sigmoid column per generated joint.
//! The official `tokenrig.decode` returns raw columns. Its Blender exporter
//! transfers those columns independently, retains the strongest four joints,
//! and normalizes only those four. The production native path deliberately
//! keeps the same seam; it does not normalize dense rows in this module.

use crate::backend::{
    gpu_add_bf16, gpu_attention_packed_bf16, gpu_attention_packed_composite_bf16,
    gpu_attention_packed_cross_bf16, gpu_attention_packed_cross_composite_bf16,
    gpu_bf16_round, gpu_concat_rows, gpu_download, gpu_gelu_erf, gpu_slice_cols,
    gpu_slice_rows, gpu_upload, gpu_weight_cache_evict_prefix, GpuTensor,
};
use crate::skin_tokens::{
    fsq_index_to_digits, SkinTokensWeights, SKIN_TOKENS_FSQ_VOCAB,
    SKIN_TOKENS_PER_BONE, SKIN_TOKENS_SAMPLE_COUNT, SKIN_TOKENS_VAE_COND_TOKENS,
    SKIN_TOKENS_VAE_HEADS, SKIN_TOKENS_VAE_LATENT, SKIN_TOKENS_VAE_WIDTH,
};
use crate::skin_tokens_neural::{
    fp32_layer_norm_to_bf16, layer_norm, linear_bf16, linear_bf16_bias_epilogue,
    linear_bf16_optional_bias, packed_linear_bf16, SKIN_TOKENS_NEURAL_NAMESPACE,
};
use crate::{emit_progress, DiffusionError, ProgressHook, Result};

pub const SKIN_TOKENS_DECODER_SELF_BLOCKS: usize = 10;
pub const SKIN_TOKENS_DECODER_CROSS_BLOCK: usize = 10;

/// Validation-only block-zero operator replay. These variants accept an
/// official captured input and execute exactly one native operator, avoiding
/// propagation from an earlier attention or residual mismatch.
#[doc(hidden)]
#[derive(Clone, Copy, Debug)]
pub enum SkinTokensDecoderBlock0Replay {
    AttentionOut,
    Norm3,
    FfIn,
    Gelu,
    FfOut,
    FfOutBiasEpilogue,
}

/// Observable stage boundaries used by the official Torch parity gate.
pub struct SkinTokensDecodeTap {
    pub indices_to_codes: GpuTensor,
    pub post_quant: GpuTensor,
    pub block0: GpuTensor,
    pub block4: GpuTensor,
    pub block9: GpuTensor,
    pub block0_norm1: GpuTensor,
    pub block0_q: GpuTensor,
    pub block0_k: GpuTensor,
    pub block0_v: GpuTensor,
    pub block0_attention: GpuTensor,
    pub block0_attention_out: GpuTensor,
    pub block0_attention_residual: GpuTensor,
    pub block0_norm3: GpuTensor,
    pub block0_ff_in: GpuTensor,
    pub block0_gelu: GpuTensor,
    pub block0_ff_out: GpuTensor,
    pub query_projection_rows: GpuTensor,
    pub cross_rows: GpuTensor,
    pub norm_rows: GpuTensor,
    pub raw_weight_logits: GpuTensor,
    pub cross_norm2_rows: GpuTensor,
    pub cross_norm_cache: GpuTensor,
    pub cross_q_rows: GpuTensor,
    pub cross_k: GpuTensor,
    pub cross_v: GpuTensor,
    pub cross_attention_rows: GpuTensor,
    pub cross_attention_out_rows: GpuTensor,
    pub cross_norm3_rows: GpuTensor,
    pub cross_ff_in_rows: GpuTensor,
    pub cross_ff_out_rows: GpuTensor,
    pub sampled_weights: GpuTensor,
}

struct SkinTokensSelfBlockTap {
    norm1: GpuTensor,
    q: GpuTensor,
    k: GpuTensor,
    v: GpuTensor,
    attention: GpuTensor,
    attention_out: GpuTensor,
    attention_residual: GpuTensor,
    norm3: GpuTensor,
    ff_in: GpuTensor,
    gelu: GpuTensor,
    ff_out: GpuTensor,
}

struct SkinTokensCrossBlockTap {
    norm2_rows: GpuTensor,
    norm_cache: GpuTensor,
    q_rows: GpuTensor,
    k: GpuTensor,
    v: GpuTensor,
    attention_rows: GpuTensor,
    attention_out_rows: GpuTensor,
    norm3_rows: GpuTensor,
    ff_in_rows: GpuTensor,
    ff_out_rows: GpuTensor,
}

struct SkinTokensDecodeInternal {
    sampled_weights: Option<GpuTensor>,
    sampled_weights_host: Vec<f32>,
    indices_to_codes: Option<GpuTensor>,
    post_quant: Option<GpuTensor>,
    block0: Option<GpuTensor>,
    block4: Option<GpuTensor>,
    block9: Option<GpuTensor>,
    block0_operator: Option<SkinTokensSelfBlockTap>,
    query_projection_rows: Option<GpuTensor>,
    cross_rows: Option<GpuTensor>,
    norm_rows: Option<GpuTensor>,
    raw_weight_logits: Option<GpuTensor>,
    cross_operator: Option<SkinTokensCrossBlockTap>,
}

impl SkinTokensDecodeTap {
    pub fn indices_to_codes_f32(&self) -> Result<Vec<f32>> {
        gpu_download(&self.indices_to_codes).map_err(DiffusionError::model)
    }

    pub fn post_quant_f32(&self) -> Result<Vec<f32>> {
        gpu_download(&self.post_quant).map_err(DiffusionError::model)
    }

    pub fn block_f32(&self, block: usize) -> Result<Vec<f32>> {
        let tensor = match block {
            0 => &self.block0,
            4 => &self.block4,
            9 => &self.block9,
            _ => {
                return Err(DiffusionError::workflow(format!(
                    "SkinTokens decoder tap block {block} is not captured; expected 0, 4, or 9",
                )))
            }
        };
        gpu_download(tensor).map_err(DiffusionError::model)
    }

    pub fn sampled_weights_f32(&self) -> Result<Vec<f32>> {
        gpu_download(&self.sampled_weights).map_err(DiffusionError::model)
    }

    pub fn block0_operator_f32(&self, name: &str) -> Result<Vec<f32>> {
        let tensor = match name {
            "norm1" => &self.block0_norm1,
            "q" => &self.block0_q,
            "k" => &self.block0_k,
            "v" => &self.block0_v,
            "attention" => &self.block0_attention,
            "attention_out" => &self.block0_attention_out,
            "attention_residual" => &self.block0_attention_residual,
            "norm3" => &self.block0_norm3,
            "ff_in" => &self.block0_ff_in,
            "gelu" => &self.block0_gelu,
            "ff_out" => &self.block0_ff_out,
            _ => {
                return Err(DiffusionError::workflow(format!(
                    "unknown SkinTokens decoder block-zero tap '{name}'",
                )))
            }
        };
        gpu_download(tensor).map_err(DiffusionError::model)
    }

    pub fn output_stage_f32(&self, name: &str) -> Result<Vec<f32>> {
        let tensor = match name {
            "query_projection_rows" => &self.query_projection_rows,
            "decoder_cross_rows" => &self.cross_rows,
            "decoder_norm_rows" => &self.norm_rows,
            "raw_weight_logits" => &self.raw_weight_logits,
            _ => {
                return Err(DiffusionError::workflow(format!(
                    "unknown SkinTokens decoder output tap '{name}'",
                )))
            }
        };
        gpu_download(tensor).map_err(DiffusionError::model)
    }

    pub fn cross_operator_f32(&self, name: &str) -> Result<Vec<f32>> {
        let tensor = match name {
            "norm2_rows" => &self.cross_norm2_rows,
            "norm_cache" => &self.cross_norm_cache,
            "q_rows" => &self.cross_q_rows,
            "k" => &self.cross_k,
            "v" => &self.cross_v,
            "attention_rows" => &self.cross_attention_rows,
            "attention_out_rows" => &self.cross_attention_out_rows,
            "norm3_rows" => &self.cross_norm3_rows,
            "ff_in_rows" => &self.cross_ff_in_rows,
            "ff_out_rows" => &self.cross_ff_out_rows,
            _ => {
                return Err(DiffusionError::workflow(format!(
                    "unknown SkinTokens decoder cross tap '{name}'",
                )))
            }
        };
        gpu_download(tensor).map_err(DiffusionError::model)
    }
}

const DECODER_SAMPLE_ROWS: [usize; 5] = [0, 1, 127, 12_345, 53_999];

fn tap_selected_rows(tensor: &GpuTensor) -> Result<GpuTensor> {
    let mut rows = Vec::with_capacity(DECODER_SAMPLE_ROWS.len() * tensor.cols());
    let host = gpu_download(tensor).map_err(DiffusionError::model)?;
    for &row in &DECODER_SAMPLE_ROWS {
        let start = row * tensor.cols();
        rows.extend_from_slice(&host[start..start + tensor.cols()]);
    }
    gpu_upload(&rows, DECODER_SAMPLE_ROWS.len(), tensor.cols()).map_err(DiffusionError::model)
}

/// Exact official FSQ `indices_to_codes` host boundary, before BF16 linear
/// projection. The five digits are normalized with integer half-width 4:
/// `(digit - 4) / 4`, yielding `[-1, 0.75]` for each `[8]` level.
pub fn fsq_indices_to_normalized_codes(indices: &[usize]) -> Result<Vec<f32>> {
    let mut output = Vec::with_capacity(indices.len() * 5);
    for &index in indices {
        if index >= SKIN_TOKENS_FSQ_VOCAB {
            return Err(DiffusionError::workflow(format!(
                "SkinTokens FSQ index {index} is outside {SKIN_TOKENS_FSQ_VOCAB}",
            )));
        }
        for digit in fsq_index_to_digits(index)? {
            output.push((digit as f32 - 4.0) * 0.25);
        }
    }
    Ok(output)
}

fn fsq_indices_to_codes_device(
    weights: &SkinTokensWeights,
    indices: &[usize],
) -> Result<GpuTensor> {
    let normalized = fsq_indices_to_normalized_codes(indices)?;
    let normalized = gpu_upload(&normalized, indices.len(), 5).map_err(DiffusionError::model)?;
    linear_bf16(
        weights,
        &normalized,
        "vae.model.FSQ.project_out.weight",
        "vae.model.FSQ.project_out.bias",
        SKIN_TOKENS_VAE_LATENT,
    )
}

fn decoder_self_block(
    weights: &SkinTokensWeights,
    hidden: &GpuTensor,
    block: usize,
    capture_taps: bool,
) -> Result<(GpuTensor, Option<SkinTokensSelfBlockTap>)> {
    let prefix = format!("vae.model.decoder.blocks.{block}");
    let normalized = fp32_layer_norm_to_bf16(
        weights,
        hidden,
        &format!("{prefix}.norm1"),
        1.0e-5,
    )?;
    let norm1_tap = capture_taps
        .then(|| gpu_slice_rows(&normalized, 0, normalized.rows()))
        .transpose()
        .map_err(DiffusionError::model)?;
    let query_name = format!("{prefix}.attn1.to_q.weight");
    let key_name = format!("{prefix}.attn1.to_k.weight");
    let value_name = format!("{prefix}.attn1.to_v.weight");
    let packed_key = format!("{prefix}.attn1.to_qkv.weight::standard-packed");
    let qkv = packed_linear_bf16(
        weights,
        &normalized,
        &[&query_name, &key_name, &value_name],
        &packed_key,
        SKIN_TOKENS_VAE_WIDTH,
        SKIN_TOKENS_VAE_HEADS,
    )?;
    let q = gpu_slice_cols(&qkv, 0, SKIN_TOKENS_VAE_WIDTH)
        .map_err(DiffusionError::model)?;
    let k = gpu_slice_cols(&qkv, SKIN_TOKENS_VAE_WIDTH, SKIN_TOKENS_VAE_WIDTH)
        .map_err(DiffusionError::model)?;
    let v = gpu_slice_cols(
        &qkv,
        SKIN_TOKENS_VAE_WIDTH * 2,
        SKIN_TOKENS_VAE_WIDTH,
    )
    .map_err(DiffusionError::model)?;
    let q_tap = capture_taps
        .then(|| gpu_slice_rows(&q, 0, q.rows()))
        .transpose()
        .map_err(DiffusionError::model)?;
    let k_tap = capture_taps
        .then(|| gpu_slice_rows(&k, 0, k.rows()))
        .transpose()
        .map_err(DiffusionError::model)?;
    let v_tap = capture_taps
        .then(|| gpu_slice_rows(&v, 0, v.rows()))
        .transpose()
        .map_err(DiffusionError::model)?;
    let attention = gpu_attention_packed_bf16(
        &q,
        &k,
        &v,
        SKIN_TOKENS_VAE_HEADS,
        1.0 / (64.0f32).sqrt(),
    )
    .map_err(DiffusionError::model)?;
    let attention = gpu_bf16_round(&attention).map_err(DiffusionError::model)?;
    let attention_tap = capture_taps
        .then(|| gpu_slice_rows(&attention, 0, attention.rows()))
        .transpose()
        .map_err(DiffusionError::model)?;
    let attention = linear_bf16(
        weights,
        &attention,
        &format!("{prefix}.attn1.to_out.0.weight"),
        &format!("{prefix}.attn1.to_out.0.bias"),
        SKIN_TOKENS_VAE_WIDTH,
    )?;
    let attention_out_tap = capture_taps
        .then(|| gpu_slice_rows(&attention, 0, attention.rows()))
        .transpose()
        .map_err(DiffusionError::model)?;
    let hidden = gpu_add_bf16(hidden, &attention).map_err(DiffusionError::model)?;
    let attention_residual_tap = capture_taps
        .then(|| gpu_slice_rows(&hidden, 0, hidden.rows()))
        .transpose()
        .map_err(DiffusionError::model)?;
    let normalized_ff =
        fp32_layer_norm_to_bf16(weights, &hidden, &format!("{prefix}.norm3"), 1.0e-5)?;
    let norm3_tap = capture_taps
        .then(|| gpu_slice_rows(&normalized_ff, 0, normalized_ff.rows()))
        .transpose()
        .map_err(DiffusionError::model)?;
    let expanded = linear_bf16(
        weights,
        &normalized_ff,
        &format!("{prefix}.ff.net.0.proj.weight"),
        &format!("{prefix}.ff.net.0.proj.bias"),
        SKIN_TOKENS_VAE_WIDTH * 4,
    )?;
    let ff_in_tap = capture_taps
        .then(|| gpu_slice_rows(&expanded, 0, expanded.rows()))
        .transpose()
        .map_err(DiffusionError::model)?;
    let activated = gpu_gelu_erf(&expanded).map_err(DiffusionError::model)?;
    // CUDA GELU uses f32 storage, while Torch preserves the BF16 activation
    // dtype. The following BF16 linear already performs this conversion on
    // its operand; make an explicit rounded copy only for the tapped gate.
    let gelu_tap = capture_taps
        .then(|| gpu_bf16_round(&activated))
        .transpose()
        .map_err(DiffusionError::model)?;
    let update = linear_bf16_bias_epilogue(
        weights,
        &activated,
        &format!("{prefix}.ff.net.2.weight"),
        &format!("{prefix}.ff.net.2.bias"),
        SKIN_TOKENS_VAE_WIDTH,
    )?;
    let ff_out_tap = capture_taps
        .then(|| gpu_slice_rows(&update, 0, update.rows()))
        .transpose()
        .map_err(DiffusionError::model)?;
    let output = gpu_add_bf16(&hidden, &update).map_err(DiffusionError::model)?;
    let taps = capture_taps.then(|| SkinTokensSelfBlockTap {
        norm1: norm1_tap.expect("norm1 tap requested"),
        q: q_tap.expect("q tap requested"),
        k: k_tap.expect("k tap requested"),
        v: v_tap.expect("v tap requested"),
        attention: attention_tap.expect("attention tap requested"),
        attention_out: attention_out_tap.expect("attention output tap requested"),
        attention_residual: attention_residual_tap.expect("attention residual tap requested"),
        norm3: norm3_tap.expect("norm3 tap requested"),
        ff_in: ff_in_tap.expect("FF input tap requested"),
        gelu: gelu_tap.expect("GELU tap requested"),
        ff_out: ff_out_tap.expect("FF output tap requested"),
    });
    Ok((output, taps))
}

/// Replay one decoder block-zero operator from a captured official input.
/// Production decoding never calls this; it exists to distinguish an
/// operator mismatch from error propagated by an earlier stage.
#[doc(hidden)]
pub fn replay_skin_tokens_decoder_block0_operator(
    weights: &SkinTokensWeights,
    input: &GpuTensor,
    operator: SkinTokensDecoderBlock0Replay,
) -> Result<GpuTensor> {
    let prefix = "vae.model.decoder.blocks.0";
    let expected_cols = match operator {
        SkinTokensDecoderBlock0Replay::Gelu
        | SkinTokensDecoderBlock0Replay::FfOut
        | SkinTokensDecoderBlock0Replay::FfOutBiasEpilogue => {
            SKIN_TOKENS_VAE_WIDTH * 4
        }
        _ => SKIN_TOKENS_VAE_WIDTH,
    };
    if input.cols() != expected_cols {
        return Err(DiffusionError::workflow(format!(
            "SkinTokens decoder block-zero {operator:?} replay input is {}x{}, expected Nx{expected_cols}",
            input.rows(),
            input.cols(),
        )));
    }
    match operator {
        SkinTokensDecoderBlock0Replay::AttentionOut => linear_bf16(
            weights,
            input,
            &format!("{prefix}.attn1.to_out.0.weight"),
            &format!("{prefix}.attn1.to_out.0.bias"),
            SKIN_TOKENS_VAE_WIDTH,
        ),
        SkinTokensDecoderBlock0Replay::Norm3 => {
            fp32_layer_norm_to_bf16(weights, input, &format!("{prefix}.norm3"), 1.0e-5)
        }
        SkinTokensDecoderBlock0Replay::FfIn => linear_bf16(
            weights,
            input,
            &format!("{prefix}.ff.net.0.proj.weight"),
            &format!("{prefix}.ff.net.0.proj.bias"),
            SKIN_TOKENS_VAE_WIDTH * 4,
        ),
        SkinTokensDecoderBlock0Replay::Gelu => {
            let activated = gpu_gelu_erf(input).map_err(DiffusionError::model)?;
            gpu_bf16_round(&activated).map_err(DiffusionError::model)
        }
        SkinTokensDecoderBlock0Replay::FfOut => linear_bf16(
            weights,
            input,
            &format!("{prefix}.ff.net.2.weight"),
            &format!("{prefix}.ff.net.2.bias"),
            SKIN_TOKENS_VAE_WIDTH,
        ),
        SkinTokensDecoderBlock0Replay::FfOutBiasEpilogue => linear_bf16_bias_epilogue(
            weights,
            input,
            &format!("{prefix}.ff.net.2.weight"),
            &format!("{prefix}.ff.net.2.bias"),
            SKIN_TOKENS_VAE_WIDTH,
        ),
    }
}

/// Replay decoder block zero's BF16 self-attention from official Q/K/V taps.
#[doc(hidden)]
pub fn replay_skin_tokens_decoder_block0_attention(
    query: &GpuTensor,
    key: &GpuTensor,
    value: &GpuTensor,
) -> Result<GpuTensor> {
    if query.rows() != key.rows()
        || query.rows() != value.rows()
        || query.cols() != SKIN_TOKENS_VAE_WIDTH
        || key.cols() != SKIN_TOKENS_VAE_WIDTH
        || value.cols() != SKIN_TOKENS_VAE_WIDTH
    {
        return Err(DiffusionError::workflow(format!(
            "SkinTokens decoder block-zero attention replay has Q={}x{}, K={}x{}, V={}x{}",
            query.rows(),
            query.cols(),
            key.rows(),
            key.cols(),
            value.rows(),
            value.cols(),
        )));
    }
    let attention = gpu_attention_packed_bf16(
        query,
        key,
        value,
        SKIN_TOKENS_VAE_HEADS,
        1.0 / (64.0f32).sqrt(),
    )
    .map_err(DiffusionError::model)?;
    gpu_bf16_round(&attention).map_err(DiffusionError::model)
}

/// Replay decoder block zero's attention through the materialized BF16
/// cuBLAS/softmax path instead of the production flash kernel. This exists
/// only to localize flash reduction-order differences.
#[doc(hidden)]
pub fn replay_skin_tokens_decoder_block0_attention_composite(
    query: &GpuTensor,
    key: &GpuTensor,
    value: &GpuTensor,
) -> Result<GpuTensor> {
    if query.rows() != key.rows()
        || query.rows() != value.rows()
        || query.cols() != SKIN_TOKENS_VAE_WIDTH
        || key.cols() != SKIN_TOKENS_VAE_WIDTH
        || value.cols() != SKIN_TOKENS_VAE_WIDTH
    {
        return Err(DiffusionError::workflow(format!(
            "SkinTokens decoder block-zero composite attention replay has Q={}x{}, K={}x{}, V={}x{}",
            query.rows(),
            query.cols(),
            key.rows(),
            key.cols(),
            value.rows(),
            value.cols(),
        )));
    }
    let attention = gpu_attention_packed_composite_bf16(
        query,
        key,
        value,
        SKIN_TOKENS_VAE_HEADS,
        1.0 / (64.0f32).sqrt(),
    )
    .map_err(DiffusionError::model)?;
    gpu_bf16_round(&attention).map_err(DiffusionError::model)
}

/// Replay the decoder cross-attention kernel from official Q/K/V taps. Query
/// may contain only selected rows; K/V contain the full compact decoder cache.
#[doc(hidden)]
pub fn replay_skin_tokens_decoder_cross_attention(
    query: &GpuTensor,
    key: &GpuTensor,
    value: &GpuTensor,
) -> Result<GpuTensor> {
    if query.rows() == 0
        || key.rows() != value.rows()
        || query.cols() != SKIN_TOKENS_VAE_WIDTH
        || key.cols() != SKIN_TOKENS_VAE_WIDTH
        || value.cols() != SKIN_TOKENS_VAE_WIDTH
    {
        return Err(DiffusionError::workflow(format!(
            "SkinTokens decoder cross-attention replay has Q={}x{}, K={}x{}, V={}x{}",
            query.rows(),
            query.cols(),
            key.rows(),
            key.cols(),
            value.rows(),
            value.cols(),
        )));
    }
    let attention = gpu_attention_packed_cross_bf16(
        query,
        key,
        value,
        SKIN_TOKENS_VAE_HEADS,
        1.0 / (64.0f32).sqrt(),
    )
    .map_err(DiffusionError::model)?;
    gpu_bf16_round(&attention).map_err(DiffusionError::model)
}

/// Replay decoder cross attention through the materialized BF16 path for a
/// direct reduction-order comparison against the production flash kernel.
#[doc(hidden)]
pub fn replay_skin_tokens_decoder_cross_attention_composite(
    query: &GpuTensor,
    key: &GpuTensor,
    value: &GpuTensor,
) -> Result<GpuTensor> {
    if query.rows() == 0
        || key.rows() != value.rows()
        || query.cols() != SKIN_TOKENS_VAE_WIDTH
        || key.cols() != SKIN_TOKENS_VAE_WIDTH
        || value.cols() != SKIN_TOKENS_VAE_WIDTH
    {
        return Err(DiffusionError::workflow(format!(
            "SkinTokens decoder composite cross-attention replay has Q={}x{}, K={}x{}, V={}x{}",
            query.rows(),
            query.cols(),
            key.rows(),
            key.cols(),
            value.rows(),
            value.cols(),
        )));
    }
    let attention = gpu_attention_packed_cross_composite_bf16(
        query,
        key,
        value,
        SKIN_TOKENS_VAE_HEADS,
        1.0 / (64.0f32).sqrt(),
    )
    .map_err(DiffusionError::model)?;
    gpu_bf16_round(&attention).map_err(DiffusionError::model)
}

/// Replay a decoder BF16 residual add from two official captured inputs.
#[doc(hidden)]
pub fn replay_skin_tokens_decoder_bf16_residual(
    left: &GpuTensor,
    right: &GpuTensor,
) -> Result<GpuTensor> {
    if left.rows() != right.rows() || left.cols() != right.cols() {
        return Err(DiffusionError::workflow(format!(
            "SkinTokens decoder residual replay shape mismatch: left={}x{}, right={}x{}",
            left.rows(),
            left.cols(),
            right.rows(),
            right.cols(),
        )));
    }
    gpu_add_bf16(left, right).map_err(DiffusionError::model)
}

fn decoder_cross_block(
    weights: &SkinTokensWeights,
    query_features: &GpuTensor,
    decoder_cache: &GpuTensor,
    capture_taps: bool,
) -> Result<(GpuTensor, Option<SkinTokensCrossBlockTap>)> {
    let prefix = format!(
        "vae.model.decoder.blocks.{SKIN_TOKENS_DECODER_CROSS_BLOCK}"
    );
    let normalized = fp32_layer_norm_to_bf16(
        weights,
        query_features,
        &format!("{prefix}.norm2"),
        1.0e-5,
    )?;
    let norm2_rows = capture_taps.then(|| tap_selected_rows(&normalized)).transpose()?;
    let normalized_cache = layer_norm(
        weights,
        decoder_cache,
        &format!("{prefix}.attn2.norm_cross"),
        1.0e-5,
    )?;
    let norm_cache = capture_taps
        .then(|| gpu_slice_rows(&normalized_cache, 0, normalized_cache.rows()))
        .transpose()
        .map_err(DiffusionError::model)?;
    let q = linear_bf16_optional_bias(
        weights,
        &normalized,
        &format!("{prefix}.attn2.to_q.weight"),
        None,
        SKIN_TOKENS_VAE_WIDTH,
    )?;
    let q_rows = capture_taps.then(|| tap_selected_rows(&q)).transpose()?;
    let key_name = format!("{prefix}.attn2.to_k.weight");
    let value_name = format!("{prefix}.attn2.to_v.weight");
    let packed_key = format!("{prefix}.attn2.to_kv.weight::standard-packed");
    let kv = packed_linear_bf16(
        weights,
        &normalized_cache,
        &[&key_name, &value_name],
        &packed_key,
        SKIN_TOKENS_VAE_WIDTH,
        SKIN_TOKENS_VAE_HEADS,
    )?;
    let k = gpu_slice_cols(&kv, 0, SKIN_TOKENS_VAE_WIDTH)
        .map_err(DiffusionError::model)?;
    let v = gpu_slice_cols(&kv, SKIN_TOKENS_VAE_WIDTH, SKIN_TOKENS_VAE_WIDTH)
        .map_err(DiffusionError::model)?;
    let k_tap = capture_taps
        .then(|| gpu_slice_rows(&k, 0, k.rows()))
        .transpose()
        .map_err(DiffusionError::model)?;
    let v_tap = capture_taps
        .then(|| gpu_slice_rows(&v, 0, v.rows()))
        .transpose()
        .map_err(DiffusionError::model)?;
    let attention = gpu_attention_packed_cross_bf16(
        &q,
        &k,
        &v,
        SKIN_TOKENS_VAE_HEADS,
        1.0 / (64.0f32).sqrt(),
    )
    .map_err(DiffusionError::model)?;
    let attention = gpu_bf16_round(&attention).map_err(DiffusionError::model)?;
    let attention_rows = capture_taps.then(|| tap_selected_rows(&attention)).transpose()?;
    let attention = linear_bf16(
        weights,
        &attention,
        &format!("{prefix}.attn2.to_out.0.weight"),
        &format!("{prefix}.attn2.to_out.0.bias"),
        SKIN_TOKENS_VAE_WIDTH,
    )?;
    let attention_out_rows = capture_taps.then(|| tap_selected_rows(&attention)).transpose()?;
    let hidden = gpu_add_bf16(query_features, &attention)
        .map_err(DiffusionError::model)?;
    let norm3 = fp32_layer_norm_to_bf16(
        weights,
        &hidden,
        &format!("{prefix}.norm3"),
        1.0e-5,
    )?;
    let norm3_rows = capture_taps.then(|| tap_selected_rows(&norm3)).transpose()?;
    let expanded = linear_bf16(
        weights,
        &norm3,
        &format!("{prefix}.ff.net.0.proj.weight"),
        &format!("{prefix}.ff.net.0.proj.bias"),
        SKIN_TOKENS_VAE_WIDTH * 4,
    )?;
    let ff_in_rows = capture_taps.then(|| tap_selected_rows(&expanded)).transpose()?;
    let activated = crate::backend::gpu_gelu_erf(&expanded).map_err(DiffusionError::model)?;
    let update = linear_bf16(
        weights,
        &activated,
        &format!("{prefix}.ff.net.2.weight"),
        &format!("{prefix}.ff.net.2.bias"),
        SKIN_TOKENS_VAE_WIDTH,
    )?;
    let ff_out_rows = capture_taps.then(|| tap_selected_rows(&update)).transpose()?;
    let output = gpu_add_bf16(&hidden, &update).map_err(DiffusionError::model)?;
    let taps = capture_taps.then(|| SkinTokensCrossBlockTap {
        norm2_rows: norm2_rows.expect("cross norm2 rows requested"),
        norm_cache: norm_cache.expect("cross norm cache requested"),
        q_rows: q_rows.expect("cross q rows requested"),
        k: k_tap.expect("cross k requested"),
        v: v_tap.expect("cross v requested"),
        attention_rows: attention_rows.expect("cross attention rows requested"),
        attention_out_rows: attention_out_rows.expect("cross attention output rows requested"),
        norm3_rows: norm3_rows.expect("cross norm3 rows requested"),
        ff_in_rows: ff_in_rows.expect("cross FF input rows requested"),
        ff_out_rows: ff_out_rows.expect("cross FF output rows requested"),
    });
    Ok((output, taps))
}

fn decode_skin_tokens_joint_internal(
    weights: &SkinTokensWeights,
    fsq_indices: &[usize],
    condition: &[f32],
    condition_latents: &GpuTensor,
    projected_query: Option<&GpuTensor>,
    capture_taps: bool,
    retain_device_output: bool,
    mut progress: Option<ProgressHook<'_>>,
) -> Result<SkinTokensDecodeInternal> {
    if fsq_indices.len() != SKIN_TOKENS_PER_BONE {
        return Err(DiffusionError::workflow(format!(
            "SkinTokens joint decoder received {} FSQ symbols, expected {SKIN_TOKENS_PER_BONE}",
            fsq_indices.len(),
        )));
    }
    if condition.len() != SKIN_TOKENS_SAMPLE_COUNT * 6 {
        return Err(DiffusionError::workflow(format!(
            "SkinTokens joint decoder condition has {} scalars, expected {}",
            condition.len(),
            SKIN_TOKENS_SAMPLE_COUNT * 6,
        )));
    }
    if condition_latents.rows() != SKIN_TOKENS_VAE_COND_TOKENS
        || condition_latents.cols() != SKIN_TOKENS_VAE_LATENT
    {
        return Err(DiffusionError::workflow(format!(
            "SkinTokens condition latents are {}x{}, expected {}x{}",
            condition_latents.rows(),
            condition_latents.cols(),
            SKIN_TOKENS_VAE_COND_TOKENS,
            SKIN_TOKENS_VAE_LATENT,
        )));
    }
    emit_progress(&mut progress, "SkinTokens FSQ decode", 0.0)?;
    let indices_to_codes = fsq_indices_to_codes_device(weights, fsq_indices)?;
    let indices_to_codes_tap = if capture_taps {
        Some(
            gpu_slice_rows(&indices_to_codes, 0, indices_to_codes.rows())
                .map_err(DiffusionError::model)?,
        )
    } else {
        None
    };
    let latent_sequence = gpu_concat_rows(&indices_to_codes, condition_latents)
        .map_err(DiffusionError::model)?;
    let post_quant = linear_bf16(
        weights,
        &latent_sequence,
        "vae.model.post_quant.weight",
        "vae.model.post_quant.bias",
        SKIN_TOKENS_VAE_WIDTH,
    )?;
    let post_quant_tap = if capture_taps {
        Some(
            gpu_slice_rows(&post_quant, 0, post_quant.rows()).map_err(DiffusionError::model)?,
        )
    } else {
        None
    };
    emit_progress(&mut progress, "SkinTokens decoder self attention", 0.08)?;

    let mut hidden = post_quant;
    let mut block0 = None;
    let mut block4 = None;
    let mut block9 = None;
    let mut block0_operator = None;
    for block in 0..SKIN_TOKENS_DECODER_SELF_BLOCKS {
        let (next_hidden, operator_tap) =
            decoder_self_block(weights, &hidden, block, capture_taps && block == 0)?;
        hidden = next_hidden;
        if operator_tap.is_some() {
            block0_operator = operator_tap;
        }
        if capture_taps {
            match block {
                0 => {
                block0 = Some(
                    gpu_slice_rows(&hidden, 0, hidden.rows()).map_err(DiffusionError::model)?,
                )
                }
                4 => {
                block4 = Some(
                    gpu_slice_rows(&hidden, 0, hidden.rows()).map_err(DiffusionError::model)?,
                )
                }
                9 => {
                block9 = Some(
                    gpu_slice_rows(&hidden, 0, hidden.rows()).map_err(DiffusionError::model)?,
                )
                }
                _ => {}
            }
        }
        emit_progress(
            &mut progress,
            "SkinTokens decoder self attention",
            0.08 + 0.62 * (block + 1) as f64 / SKIN_TOKENS_DECODER_SELF_BLOCKS as f64,
        )?;
    }

    let owned_projected_query;
    let projected_query = match projected_query {
        Some(projected) => projected,
        None => {
            owned_projected_query = prepare_decoder_query(weights, condition)?;
            &owned_projected_query
        }
    };
    let query_projection_rows = if capture_taps {
        Some(tap_selected_rows(projected_query)?)
    } else {
        None
    };
    emit_progress(&mut progress, "SkinTokens decoder point query", 0.78)?;
    let (queried, cross_operator) =
        decoder_cross_block(weights, projected_query, &hidden, capture_taps)?;
    let cross_rows = if capture_taps {
        Some(tap_selected_rows(&queried)?)
    } else {
        None
    };
    let normalized = layer_norm(
        weights,
        &queried,
        "vae.model.decoder.norm_out",
        1.0e-5,
    )?;
    let norm_rows = if capture_taps {
        Some(tap_selected_rows(&normalized)?)
    } else {
        None
    };
    let raw_logits = linear_bf16(
        weights,
        &normalized,
        "vae.model.decoder.proj_out.weight",
        "vae.model.decoder.proj_out.bias",
        1,
    )?;
    let raw_weight_logits_tap = if capture_taps {
        Some(
            gpu_slice_rows(&raw_logits, 0, raw_logits.rows())
                .map_err(DiffusionError::model)?,
        )
    } else {
        None
    };
    // The existing CUDA sigmoid primitive is internal to the Qwen backend.
    // One 54k-column host sigmoid is tiny beside the 768-wide cross-attention,
    // and makes the exact activation boundary explicit until it is promoted.
    let mut sampled_weights_host = gpu_download(&raw_logits).map_err(DiffusionError::model)?;
    for value in &mut sampled_weights_host {
        *value = round_to_bf16(1.0 / (1.0 + (-*value).exp()));
    }
    let sampled_weights = if retain_device_output {
        Some(
            gpu_upload(&sampled_weights_host, SKIN_TOKENS_SAMPLE_COUNT, 1)
                .map_err(DiffusionError::model)?,
        )
    } else {
        None
    };
    emit_progress(&mut progress, "SkinTokens decoder point query", 1.0)?;
    Ok(SkinTokensDecodeInternal {
        sampled_weights,
        sampled_weights_host,
        indices_to_codes: indices_to_codes_tap,
        post_quant: post_quant_tap,
        block0,
        block4,
        block9,
        block0_operator,
        query_projection_rows,
        cross_rows,
        norm_rows,
        raw_weight_logits: raw_weight_logits_tap,
        cross_operator,
    })
}

fn prepare_decoder_query(weights: &SkinTokensWeights, condition: &[f32]) -> Result<GpuTensor> {
    if condition.len() != SKIN_TOKENS_SAMPLE_COUNT * 6 {
        return Err(DiffusionError::workflow(format!(
            "SkinTokens decoder condition has {} scalars, expected {}",
            condition.len(),
            SKIN_TOKENS_SAMPLE_COUNT * 6,
        )));
    }
    let embedded = embed_decoder_query_rows_bf16(condition)?;
    let embedded = gpu_upload(&embedded, SKIN_TOKENS_SAMPLE_COUNT, 54)
        .map_err(DiffusionError::model)?;
    linear_bf16(
        weights,
        &embedded,
        "vae.model.decoder.proj_query.weight",
        "vae.model.decoder.proj_query.bias",
        SKIN_TOKENS_VAE_WIDTH,
    )
}

/// Build the decoder's PMPE query rows with the exact released BF16 boundary.
///
/// This deliberately differs from the continuous-condition encoder. The
/// official `_decode` first casts all sampled point/normal rows to `z.dtype`
/// (BF16), then evaluates the PMPE pointwise operations in BF16. The encoder
/// evaluates the same embedder directly on its f32 condition input. Computing
/// decoder PMPE in f32 and rounding only at the following linear changes the
/// high-frequency channels enough to move `decoder.proj_query` by ~8e-2.
fn embed_decoder_query_rows_bf16(condition: &[f32]) -> Result<Vec<f32>> {
    if condition.len() % 6 != 0 {
        return Err(DiffusionError::workflow(format!(
            "SkinTokens decoder condition has {} scalars, not Nx6",
            condition.len(),
        )));
    }
    const POWERS: [f32; 8] = [1.0, 2.0, 4.0, 8.0, 16.0, 32.0, 64.0, 128.0];
    let frequencies = POWERS.map(|power| round_to_bf16(power * std::f32::consts::PI));
    let mut phase_constants = [0.0f32; 8];
    for (index, phase) in phase_constants.iter_mut().enumerate() {
        let fraction = (index + 1) as f32 / 8.0;
        *phase = round_to_bf16(
            (8.0f32.powf(1.0 - fraction) + fraction) * (2.0 * std::f32::consts::PI),
        );
    }

    let mut output = Vec::with_capacity(condition.len() / 6 * 54);
    for row in condition.chunks_exact(6) {
        let position = [
            round_to_bf16(row[0]),
            round_to_bf16(row[1]),
            round_to_bf16(row[2]),
        ];
        output.extend_from_slice(&position);
        for &coordinate in &position {
            for (index, &frequency) in frequencies.iter().enumerate() {
                let embedded = round_to_bf16(coordinate * frequency);
                // Keep the two scalar multiplies separate: upstream spells
                // this as `x * torch.pi * 0.5`, and each BF16 tensor op
                // writes a BF16 result before the following operation.
                let phase = round_to_bf16(coordinate * std::f32::consts::PI);
                let phase = round_to_bf16(phase * 0.5);
                let phase = round_to_bf16(phase + phase_constants[index]);
                output.push(round_to_bf16(
                    round_to_bf16(embedded.sin()) + round_to_bf16(phase.sin()),
                ));
            }
        }
        for &coordinate in &position {
            for (index, &frequency) in frequencies.iter().enumerate() {
                let embedded = round_to_bf16(coordinate * frequency);
                let phase = round_to_bf16(coordinate * std::f32::consts::PI);
                let phase = round_to_bf16(phase * 0.5);
                let phase = round_to_bf16(phase + phase_constants[index]);
                output.push(round_to_bf16(
                    round_to_bf16(embedded.cos()) + round_to_bf16(phase.cos()),
                ));
            }
        }
        output.extend(row[3..6].iter().copied().map(round_to_bf16));
    }
    Ok(output)
}

/// Decode exactly one joint's four generated FSQ symbols into 54,000 raw
/// sampled-point weights while retaining official-oracle stage taps.
/// Production uses [`decode_skin_tokens_joint`] and pays for none of these
/// validation-only device copies.
pub fn decode_skin_tokens_joint_tapped(
    weights: &SkinTokensWeights,
    fsq_indices: &[usize],
    condition: &[f32],
    condition_latents: &GpuTensor,
    progress: Option<ProgressHook<'_>>,
) -> Result<SkinTokensDecodeTap> {
    let decoded = decode_skin_tokens_joint_internal(
        weights,
        fsq_indices,
        condition,
        condition_latents,
        None,
        true,
        true,
        progress,
    )?;
    let block0_operator = decoded
        .block0_operator
        .expect("decoder block-zero taps were requested");
    let cross_operator = decoded
        .cross_operator
        .expect("decoder cross taps were requested");
    Ok(SkinTokensDecodeTap {
        indices_to_codes: decoded
            .indices_to_codes
            .expect("decoder FSQ tap was requested"),
        post_quant: decoded.post_quant.expect("decoder post-quant tap was requested"),
        block0: decoded.block0.expect("decoder block zero tap was requested"),
        block4: decoded.block4.expect("decoder block four tap was requested"),
        block9: decoded.block9.expect("decoder block nine tap was requested"),
        block0_norm1: block0_operator.norm1,
        block0_q: block0_operator.q,
        block0_k: block0_operator.k,
        block0_v: block0_operator.v,
        block0_attention: block0_operator.attention,
        block0_attention_out: block0_operator.attention_out,
        block0_attention_residual: block0_operator.attention_residual,
        block0_norm3: block0_operator.norm3,
        block0_ff_in: block0_operator.ff_in,
        block0_gelu: block0_operator.gelu,
        block0_ff_out: block0_operator.ff_out,
        query_projection_rows: decoded
            .query_projection_rows
            .expect("decoder query rows were requested"),
        cross_rows: decoded.cross_rows.expect("decoder cross rows were requested"),
        norm_rows: decoded.norm_rows.expect("decoder norm rows were requested"),
        raw_weight_logits: decoded
            .raw_weight_logits
            .expect("decoder logits were requested"),
        cross_norm2_rows: cross_operator.norm2_rows,
        cross_norm_cache: cross_operator.norm_cache,
        cross_q_rows: cross_operator.q_rows,
        cross_k: cross_operator.k,
        cross_v: cross_operator.v,
        cross_attention_rows: cross_operator.attention_rows,
        cross_attention_out_rows: cross_operator.attention_out_rows,
        cross_norm3_rows: cross_operator.norm3_rows,
        cross_ff_in_rows: cross_operator.ff_in_rows,
        cross_ff_out_rows: cross_operator.ff_out_rows,
        sampled_weights: decoded
            .sampled_weights
            .expect("decoder device output was requested"),
    })
}

fn round_to_bf16(value: f32) -> f32 {
    let bits = value.to_bits();
    let rounded = bits.wrapping_add(0x7fff + ((bits >> 16) & 1));
    f32::from_bits(rounded & 0xffff_0000)
}

/// Decode one joint without retaining validation taps.
pub fn decode_skin_tokens_joint(
    weights: &SkinTokensWeights,
    fsq_indices: &[usize],
    condition: &[f32],
    condition_latents: &GpuTensor,
    progress: Option<ProgressHook<'_>>,
) -> Result<GpuTensor> {
    Ok(decode_skin_tokens_joint_internal(
        weights,
        fsq_indices,
        condition,
        condition_latents,
        None,
        false,
        true,
        progress,
    )?
    .sampled_weights
    .expect("decoder device output was requested"))
}

/// Production multi-joint decode. `fsq_indices` is joint-major with exactly
/// four symbols per joint. The returned host matrix is sample-major `[54000,
/// J]`, preserving the official raw independent sigmoid columns; no across-
/// joint normalization or top-k selection is applied here.
///
/// Decoder matrices are cached by stable tensor name during the first joint
/// and reused for every later joint. Progress is reported at every joint seam,
/// so cancellation never waits longer than one decoder column.
pub fn decode_skin_tokens_weights(
    weights: &SkinTokensWeights,
    fsq_indices: &[usize],
    condition: &[f32],
    condition_latents: &GpuTensor,
    mut progress: Option<ProgressHook<'_>>,
) -> Result<Vec<f32>> {
    if fsq_indices.is_empty() || fsq_indices.len() % SKIN_TOKENS_PER_BONE != 0 {
        return Err(DiffusionError::workflow(format!(
            "SkinTokens multi-joint decoder received {} FSQ symbols, expected a non-empty multiple of {SKIN_TOKENS_PER_BONE}",
            fsq_indices.len(),
        )));
    }
    let joints = fsq_indices.len() / SKIN_TOKENS_PER_BONE;
    let element_count = SKIN_TOKENS_SAMPLE_COUNT
        .checked_mul(joints)
        .ok_or_else(|| DiffusionError::workflow("SkinTokens dense weight shape overflows"))?;
    let mut dense = vec![0.0f32; element_count];
    emit_progress(&mut progress, "SkinTokens decode weights", 0.0)?;
    // Point Fourier features and their 54->768 projection are invariant over
    // joints. Keep the projection resident for the whole request instead of
    // repeating a 54k-row GEMM for every skin column.
    let projected_query = prepare_decoder_query(weights, condition)?;
    for (joint, symbols) in fsq_indices
        .chunks_exact(SKIN_TOKENS_PER_BONE)
        .enumerate()
    {
        // Preserve the decoder's per-block cancellation points while mapping
        // its local progress into this joint's monotonic slice. Construct no
        // closure at all on the performance-sensitive no-progress path.
        let decoded = if progress.is_some() {
            let mut joint_progress = |label: &str, fraction: f64| {
                emit_progress(
                    &mut progress,
                    label,
                    (joint as f64 + fraction.clamp(0.0, 1.0)) / joints as f64,
                )
            };
            decode_skin_tokens_joint_internal(
                weights,
                symbols,
                condition,
                condition_latents,
                Some(&projected_query),
                false,
                false,
                Some(&mut joint_progress),
            )?
        } else {
            decode_skin_tokens_joint_internal(
                weights,
                symbols,
                condition,
                condition_latents,
                Some(&projected_query),
                false,
                false,
                None,
            )?
        };
        let column = decoded.sampled_weights_host;
        if column.len() != SKIN_TOKENS_SAMPLE_COUNT {
            return Err(DiffusionError::model(format!(
                "SkinTokens joint {joint} decoded {} weights, expected {SKIN_TOKENS_SAMPLE_COUNT}",
                column.len(),
            )));
        }
        for (sample, weight) in column.into_iter().enumerate() {
            dense[sample * joints + joint] = weight;
        }
    }
    Ok(dense)
}

/// Optional dense-weight utility for consumers which explicitly require every
/// N x J row to sum to one. This is **not** part of official TokenRig decode:
/// official inference transfers raw sigmoid columns through cKDTree/IDW and
/// only the later Blender top-k export normalizes the four retained values.
/// The native playable-character path mirrors that ordering and does not call
/// this helper before transfer or top-four selection.
pub fn normalize_dense_skin_weight_rows_optional(
    raw: &mut [f32],
    rows: usize,
    joints: usize,
) -> Result<()> {
    let expected = rows
        .checked_mul(joints)
        .ok_or_else(|| DiffusionError::workflow("SkinTokens weight shape overflows"))?;
    if joints == 0 || raw.len() != expected {
        return Err(DiffusionError::workflow(format!(
            "SkinTokens weights have {} values, expected {rows}x{joints}={expected}",
            raw.len(),
        )));
    }
    for (row_index, row) in raw.chunks_exact_mut(joints).enumerate() {
        let mut sum = 0.0f32;
        for &weight in row.iter() {
            if !weight.is_finite() || weight < 0.0 {
                return Err(DiffusionError::workflow(format!(
                    "SkinTokens weight row {row_index} contains invalid value {weight}",
                )));
            }
            sum += weight;
        }
        if !sum.is_finite() || sum <= 0.0 {
            return Err(DiffusionError::workflow(format!(
                "SkinTokens weight row {row_index} has invalid sum {sum}",
            )));
        }
        for weight in row {
            *weight /= sum;
        }
    }
    Ok(())
}

/// Evict the shared streamed SkinTokens matrices. Kept here as a decoder-side
/// lifecycle entry point so a service can unload even when it uses only this
/// module directly.
pub fn unload_skin_tokens_decode_weights() -> Result<()> {
    gpu_weight_cache_evict_prefix(SKIN_TOKENS_NEURAL_NAMESPACE)
        .map_err(DiffusionError::model)
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fsq_indices_to_codes_matches_mixed_radix_examples() {
        let codes = fsq_indices_to_normalized_codes(&[0, 1, 12_345, 32_767]).unwrap();
        assert_eq!(&codes[..5], &[-1.0; 5]);
        assert_eq!(&codes[5..10], &[-0.75, -1.0, -1.0, -1.0, -1.0]);
        assert_eq!(&codes[15..20], &[0.75; 5]);
        assert!(fsq_indices_to_normalized_codes(&[32_768]).is_err());
    }

    #[test]
    fn optional_weight_row_normalization_is_explicit() {
        let mut weights = vec![0.1, 0.3, 0.6, 2.0, 2.0, 4.0];
        normalize_dense_skin_weight_rows_optional(&mut weights, 2, 3).unwrap();
        assert!((weights[0] - 0.1).abs() < 1e-7);
        assert!((weights[1] - 0.3).abs() < 1e-7);
        assert!((weights[2] - 0.6).abs() < 1e-7);
        assert!((weights[3] - 0.25).abs() < 1e-7);
        assert!((weights[4] - 0.25).abs() < 1e-7);
        assert!((weights[5] - 0.5).abs() < 1e-7);
        assert!(normalize_dense_skin_weight_rows_optional(&mut [0.0, 0.0], 1, 2).is_err());
        assert!(normalize_dense_skin_weight_rows_optional(&mut [f32::NAN], 1, 1).is_err());
    }

    #[test]
    fn decoder_query_embedding_preserves_official_bf16_pointwise_boundary() {
        let row = [
            -0.4502623,
            -0.01830144,
            -0.9563238,
            0.76786184,
            0.05207949,
            0.638495,
        ];
        let embedded = embed_decoder_query_rows_bf16(&row).unwrap();
        assert_eq!(embedded.len(), 54);
        assert_eq!(
            &embedded[..12],
            &[
                -0.451171875,
                -0.018310546875,
                -0.95703125,
                -0.12109375,
                -0.92578125,
                0.099609375,
                1.90625,
                -0.32421875,
                -0.04296875,
                -0.20703125,
                0.30078125,
                0.9140625,
            ],
        );
        assert_eq!(&embedded[51..], &[0.76953125, 0.052001953125, 0.63671875]);
        assert!(embed_decoder_query_rows_bf16(&row[..5]).is_err());
    }

}
