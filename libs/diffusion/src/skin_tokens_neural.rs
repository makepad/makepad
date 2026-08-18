//! Native CUDA continuous-prefix encoders for SkinTokens.
//!
//! This module owns the SkinVAE condition encoder and Michelangelo mesh
//! encoder.  Both checkpoints are BF16 and PyTorch autocast establishes BF16
//! boundaries after every linear/attention/residual block; device tensors use
//! f32 storage but are explicitly rounded at those boundaries.

use crate::backend::{
    gpu_add_bf16, gpu_attention_packed_bf16, gpu_attention_packed_cross_bf16,
    gpu_bf16_round, gpu_download, gpu_gather_cols, gpu_gelu_erf, gpu_layer_norm_pytorch,
    gpu_linear_nt_cached_bf16_bias_epilogue, gpu_linear_nt_cached_bf16_f32acc,
    gpu_linear_nt_cached_bf16_mm, gpu_rms_norm_mul_bf16,
    gpu_skintokens_michelangelo_fourier, gpu_slice_cols, gpu_upload, gpu_weight_cache_ensure,
    gpu_weight_cache_evict_prefix, GpuLinearPart, GpuTensor,
};
use crate::skin_tokens::SkinTokensWeights;
use crate::skin_tokens_condition::{
    embed_condition_rows, select_condition_rows, SkinTokensConditionKind,
    SkinTokensConditionSelection,
};
use crate::{DiffusionError, Result};
use makepad_ggml::quant::GGML_TYPE_BF16;
use makepad_mlx::MlxDType;

pub const SKIN_TOKENS_NEURAL_NAMESPACE: &str = "skin-tokens-tokenrig-bf16";

fn check_cancel(cancel: Option<&dyn Fn() -> bool>) -> Result<()> {
    if cancel.is_some_and(|is_cancelled| is_cancelled()) {
        Err(DiffusionError::Cancelled)
    } else {
        Ok(())
    }
}

/// First native neural gate: exact deterministic query selection followed by
/// the shared 54-channel Fourier projection. The device tensors remain
/// resident for the attention encoders; optional downloads are exposed only
/// for validation.
pub struct SkinTokensProjectedCondition {
    pub selection: SkinTokensConditionSelection,
    pub query: GpuTensor,
    pub key_value: GpuTensor,
}

pub struct SkinTokensVaeCondition {
    pub selection: SkinTokensConditionSelection,
    pub block0: GpuTensor,
    pub block1: GpuTensor,
    pub block2: GpuTensor,
    pub normalized: GpuTensor,
    pub latents: GpuTensor,
}

#[derive(Clone, Debug)]
pub struct SkinTokensVaeCrossTap {
    pub norm2: Vec<f32>,
    pub norm_cross: Vec<f32>,
    pub q: Vec<f32>,
    pub k: Vec<f32>,
    pub v: Vec<f32>,
    pub attention_out: Vec<f32>,
    pub norm3: Vec<f32>,
    pub ff_in: Vec<f32>,
    pub ff_out: Vec<f32>,
}

pub struct SkinTokensMeshPrefix {
    pub selection: SkinTokensConditionSelection,
    pub encoded: GpuTensor,
    pub prefix: GpuTensor,
}

#[derive(Clone, Debug)]
pub struct SkinTokensMeshEncoderTap {
    pub cross: SkinTokensMeshCrossTap,
    pub cross_attention: Vec<f32>,
    pub blocks: Vec<Vec<f32>>,
    pub normalized: Vec<f32>,
    pub output_linear: Vec<f32>,
    pub prefix: Vec<f32>,
}

#[derive(Clone, Debug)]
pub struct SkinTokensMeshCrossTap {
    pub ln1: Vec<f32>,
    pub ln2: Vec<f32>,
    pub q: Vec<f32>,
    /// Standard stream-major `[K, V]`; the official `c_kv` hook is
    /// head-first and must be permuted before direct comparison.
    pub kv: Vec<f32>,
    pub flash: Vec<f32>,
    pub to_out: Vec<f32>,
    pub ln3: Vec<f32>,
    pub ff_in: Vec<f32>,
    pub gelu: Vec<f32>,
    pub ff_out: Vec<f32>,
}

impl SkinTokensVaeCondition {
    pub fn block_f32(&self, index: usize) -> Result<Vec<f32>> {
        let block = match index {
            0 => &self.block0,
            1 => &self.block1,
            2 => &self.block2,
            _ => {
                return Err(DiffusionError::workflow(format!(
                    "SkinTokens VAE condition block {index} is outside 0..3",
                )))
            }
        };
        gpu_download(block).map_err(DiffusionError::model)
    }

    pub fn normalized_f32(&self) -> Result<Vec<f32>> {
        gpu_download(&self.normalized).map_err(DiffusionError::model)
    }

    pub fn latents_f32(&self) -> Result<Vec<f32>> {
        gpu_download(&self.latents).map_err(DiffusionError::model)
    }
}

impl SkinTokensProjectedCondition {
    pub fn query_f32(&self) -> Result<Vec<f32>> {
        gpu_download(&self.query).map_err(DiffusionError::model)
    }

    pub fn key_value_f32(&self) -> Result<Vec<f32>> {
        gpu_download(&self.key_value).map_err(DiffusionError::model)
    }
}

fn projection_names(kind: SkinTokensConditionKind) -> (&'static str, &'static str, usize) {
    match kind {
        SkinTokensConditionKind::SkinVae => (
            "vae.model.cond_encoder.proj_in.weight",
            "vae.model.cond_encoder.proj_in.bias",
            768,
        ),
        SkinTokensConditionKind::Michelangelo => (
            "mesh_encoder.encoder.input_proj.weight",
            "mesh_encoder.encoder.input_proj.bias",
            512,
        ),
    }
}

fn ensure_bf16_linear<'a>(
    weights: &SkinTokensWeights,
    name: &'a str,
    output_cols: usize,
    input_cols: usize,
) -> Result<GpuLinearPart<'a>> {
    let (dtype, shape) = weights.tensor_dtype_shape(name)?;
    if dtype != MlxDType::BF16
        || shape != [output_cols as u64, input_cols as u64]
    {
        return Err(DiffusionError::model(format!(
            "SkinTokens linear '{name}' is {dtype:?} {shape:?}, expected BF16 [{output_cols}, {input_cols}]",
        )));
    }
    gpu_weight_cache_ensure(
        SKIN_TOKENS_NEURAL_NAMESPACE,
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

/// Map a stream-major output column (`Q heads`, then `K heads`, then `V
/// heads`) to the released Tripo/Michelangelo projection column. Their
/// processors reshape to `[heads, streams, head_dim]` *before* splitting,
/// rather than the usual `[streams, heads, head_dim]` layout.
pub(crate) fn official_packed_source_column(
    standard_column: usize,
    width: usize,
    heads: usize,
    streams: usize,
) -> Result<usize> {
    if heads == 0 || streams == 0 || width % heads != 0 {
        return Err(DiffusionError::model(format!(
            "invalid packed projection geometry width={width} heads={heads} streams={streams}",
        )));
    }
    if standard_column >= width * streams {
        return Err(DiffusionError::model(format!(
            "packed projection column {standard_column} is outside {}",
            width * streams,
        )));
    }
    let head_dim = width / heads;
    let stream = standard_column / width;
    let within_stream = standard_column % width;
    let head = within_stream / head_dim;
    let dim = within_stream % head_dim;
    Ok(head * streams * head_dim + stream * head_dim + dim)
}

pub(crate) fn standard_packed_column_from_official(
    official_column: usize,
    width: usize,
    heads: usize,
    streams: usize,
) -> Result<usize> {
    if heads == 0 || streams == 0 || width % heads != 0 {
        return Err(DiffusionError::model(format!(
            "invalid packed projection geometry width={width} heads={heads} streams={streams}",
        )));
    }
    if official_column >= width * streams {
        return Err(DiffusionError::model(format!(
            "packed projection column {official_column} is outside {}",
            width * streams,
        )));
    }
    let head_dim = width / heads;
    let head = official_column / (streams * head_dim);
    let within_head = official_column % (streams * head_dim);
    let stream = within_head / head_dim;
    let dim = within_head % head_dim;
    Ok(stream * width + head * head_dim + dim)
}

/// Permute BF16 matrix rows once while filling the CUDA cache. A linear's
/// matrix rows are its activation output columns, so this makes the runtime
/// Q/K/V slices conventional and avoids a gather on every encoder block.
/// The source checkpoint bytes are never modified.
pub(crate) fn permute_official_packed_bf16_rows(
    official: &[u8],
    input_cols: usize,
    width: usize,
    heads: usize,
    streams: usize,
) -> Result<Vec<u8>> {
    let rows = width
        .checked_mul(streams)
        .ok_or_else(|| DiffusionError::model("packed projection row count overflow"))?;
    let row_bytes = input_cols
        .checked_mul(2)
        .ok_or_else(|| DiffusionError::model("packed projection row width overflow"))?;
    let expected = rows
        .checked_mul(row_bytes)
        .ok_or_else(|| DiffusionError::model("packed projection byte count overflow"))?;
    if official.len() != expected {
        return Err(DiffusionError::model(format!(
            "packed projection has {} bytes, expected {expected}",
            official.len(),
        )));
    }
    let mut standard = vec![0u8; expected];
    for standard_row in 0..rows {
        let official_row =
            official_packed_source_column(standard_row, width, heads, streams)?;
        let source = official_row * row_bytes;
        let destination = standard_row * row_bytes;
        standard[destination..destination + row_bytes]
            .copy_from_slice(&official[source..source + row_bytes]);
    }
    Ok(standard)
}

/// Cache a Tripo-style projection from either separate Q/K/V matrices or one
/// already-concatenated QKV matrix. `source_names` are concatenated in their
/// official stream order before the output-column permutation.
pub(crate) fn ensure_packed_bf16_linear<'a>(
    weights: &SkinTokensWeights,
    source_names: &[&str],
    cache_key: &'a str,
    width: usize,
    heads: usize,
    input_cols: usize,
) -> Result<GpuLinearPart<'a>> {
    let streams = source_names.len();
    if streams < 2 {
        return Err(DiffusionError::model(
            "packed projection requires at least two streams",
        ));
    }
    for name in source_names {
        let (dtype, shape) = weights.tensor_dtype_shape(name)?;
        if dtype != MlxDType::BF16 || shape != [width as u64, input_cols as u64] {
            return Err(DiffusionError::model(format!(
                "SkinTokens packed projection '{name}' is {dtype:?} {shape:?}, expected BF16 [{width}, {input_cols}]",
            )));
        }
    }
    gpu_weight_cache_ensure(
        SKIN_TOKENS_NEURAL_NAMESPACE,
        cache_key,
        GGML_TYPE_BF16,
        width * streams,
        input_cols,
        false,
        || {
            let mut official = Vec::with_capacity(width * streams * input_cols * 2);
            for name in source_names {
                official.extend_from_slice(
                    &weights.tensor_bytes(name).map_err(|error| error.to_string())?,
                );
            }
            permute_official_packed_bf16_rows(
                &official,
                input_cols,
                width,
                heads,
                streams,
            )
            .map_err(|error| error.to_string())
        },
    )
    .map_err(DiffusionError::model)?;
    Ok(GpuLinearPart {
        bt_ggml_type: GGML_TYPE_BF16,
        n: width * streams,
        cache_key,
        bytes: &[],
    })
}

pub(crate) fn linear_bf16_optional_bias(
    weights: &SkinTokensWeights,
    input: &GpuTensor,
    weight_name: &str,
    bias_name: Option<&str>,
    output_cols: usize,
) -> Result<GpuTensor> {
    let part = ensure_bf16_linear(weights, weight_name, output_cols, input.cols())?;
    let bias = match bias_name {
        Some(name) => {
            let bias = weights.tensor_f32(name)?;
            if bias.len() != output_cols {
                return Err(DiffusionError::model(format!(
                    "SkinTokens bias '{name}' has {} values, expected {output_cols}",
                    bias.len(),
                )));
            }
            bias
        }
        None => Vec::new(),
    };
    let output = gpu_linear_nt_cached_bf16_f32acc(
        input,
        SKIN_TOKENS_NEURAL_NAMESPACE,
        &[part],
        &bias,
    )
    .map_err(DiffusionError::model)?;
    gpu_bf16_round(&output).map_err(DiffusionError::model)
}

pub(crate) fn linear_bf16(
    weights: &SkinTokensWeights,
    input: &GpuTensor,
    weight_name: &str,
    bias_name: &str,
    output_cols: usize,
) -> Result<GpuTensor> {
    linear_bf16_optional_bias(
        weights,
        input,
        weight_name,
        Some(bias_name),
        output_cols,
    )
}

/// Exact pinned-Torch biased BF16 linear, exposed within diffusion for
/// SkinTokens stages that have an independent official-input replay gate.
/// Keeping cache preparation here avoids duplicating the conditioner/decoder
/// checkpoint contract.
pub(crate) fn linear_bf16_bias_epilogue(
    weights: &SkinTokensWeights,
    input: &GpuTensor,
    weight_name: &str,
    bias_name: &str,
    output_cols: usize,
) -> Result<GpuTensor> {
    let part = ensure_bf16_linear(weights, weight_name, output_cols, input.cols())?;
    let bias = weights.tensor_f32(bias_name)?;
    if bias.len() != output_cols {
        return Err(DiffusionError::model(format!(
            "SkinTokens bias '{bias_name}' has {} values, expected {output_cols}",
            bias.len(),
        )));
    }
    gpu_linear_nt_cached_bf16_bias_epilogue(
        input,
        SKIN_TOKENS_NEURAL_NAMESPACE,
        &[part],
        &bias,
    )
    .map_err(DiffusionError::model)
}

/// Exact pinned-Torch bias-free BF16 linear for independently replayed
/// SkinTokens boundaries. The generic linear remains on its legacy path.
pub(crate) fn linear_bf16_mm(
    weights: &SkinTokensWeights,
    input: &GpuTensor,
    weight_name: &str,
    output_cols: usize,
) -> Result<GpuTensor> {
    let part = ensure_bf16_linear(weights, weight_name, output_cols, input.cols())?;
    gpu_linear_nt_cached_bf16_mm(input, SKIN_TOKENS_NEURAL_NAMESPACE, &[part])
        .map_err(DiffusionError::model)
}

fn combined_packed_linear_bf16_mm(
    weights: &SkinTokensWeights,
    input: &GpuTensor,
    source_name: &str,
    width: usize,
    heads: usize,
    streams: usize,
) -> Result<GpuTensor> {
    let official = linear_bf16_mm(weights, input, source_name, width * streams)?;
    let indices = (0..width * streams)
        .map(|standard| official_packed_source_column(standard, width, heads, streams))
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .map(|index| index as u32)
        .collect::<Vec<_>>();
    gpu_gather_cols(&official, &indices).map_err(DiffusionError::model)
}

pub(crate) fn packed_linear_bf16(
    weights: &SkinTokensWeights,
    input: &GpuTensor,
    source_names: &[&str],
    cache_key: &str,
    width: usize,
    heads: usize,
) -> Result<GpuTensor> {
    let part = ensure_packed_bf16_linear(
        weights,
        source_names,
        cache_key,
        width,
        heads,
        input.cols(),
    )?;
    let output = gpu_linear_nt_cached_bf16_f32acc(
        input,
        SKIN_TOKENS_NEURAL_NAMESPACE,
        &[part],
        &[],
    )
    .map_err(DiffusionError::model)?;
    gpu_bf16_round(&output).map_err(DiffusionError::model)
}

pub(crate) fn layer_norm(
    weights: &SkinTokensWeights,
    input: &GpuTensor,
    prefix: &str,
    eps: f32,
) -> Result<GpuTensor> {
    let scale_name = format!("{prefix}.weight");
    let bias_name = format!("{prefix}.bias");
    let scale = weights.tensor_f32(&scale_name)?;
    let bias = weights.tensor_f32(&bias_name)?;
    gpu_layer_norm_pytorch(input, &scale, &bias, eps).map_err(DiffusionError::model)
}

/// Diffusers `FP32LayerNorm`: compute normalization in f32, then restore the
/// incoming BF16 activation dtype. This differs from ordinary `nn.LayerNorm`
/// under CUDA autocast, whose output remains f32 in the Michelangelo tower
/// and at the VAE's final `norm_out` boundary.
pub(crate) fn fp32_layer_norm_to_bf16(
    weights: &SkinTokensWeights,
    input: &GpuTensor,
    prefix: &str,
    eps: f32,
) -> Result<GpuTensor> {
    let normalized = layer_norm(weights, input, prefix, eps)?;
    gpu_bf16_round(&normalized).map_err(DiffusionError::model)
}

pub(crate) fn tripo_feed_forward(
    weights: &SkinTokensWeights,
    hidden: &GpuTensor,
    block_prefix: &str,
) -> Result<GpuTensor> {
    let normalized = fp32_layer_norm_to_bf16(
        weights,
        hidden,
        &format!("{block_prefix}.norm3"),
        1.0e-5,
    )?;
    let expanded = linear_bf16(
        weights,
        &normalized,
        &format!("{block_prefix}.ff.net.0.proj.weight"),
        &format!("{block_prefix}.ff.net.0.proj.bias"),
        hidden.cols() * 4,
    )?;
    let activated = gpu_gelu_erf(&expanded).map_err(DiffusionError::model)?;
    let update = linear_bf16(
        weights,
        &activated,
        &format!("{block_prefix}.ff.net.2.weight"),
        &format!("{block_prefix}.ff.net.2.bias"),
        hidden.cols(),
    )?;
    gpu_add_bf16(hidden, &update).map_err(DiffusionError::model)
}

fn download_if(capture: bool, tensor: &GpuTensor) -> Result<Vec<f32>> {
    if capture {
        gpu_download(tensor).map_err(DiffusionError::model)
    } else {
        Ok(Vec::new())
    }
}

fn vae_cross_block_inner(
    weights: &SkinTokensWeights,
    hidden: &GpuTensor,
    encoder: &GpuTensor,
    block: usize,
    capture_taps: bool,
) -> Result<(GpuTensor, Option<SkinTokensVaeCrossTap>)> {
    let prefix = format!("vae.model.cond_encoder.blocks.{block}");
    let normalized =
        fp32_layer_norm_to_bf16(weights, hidden, &format!("{prefix}.norm2"), 1.0e-5)?;
    let norm2_tap = download_if(capture_taps, &normalized)?;
    let normalized_encoder = layer_norm(
        weights,
        encoder,
        &format!("{prefix}.attn2.norm_cross"),
        1.0e-5,
    )?;
    let norm_cross_tap = download_if(capture_taps, &normalized_encoder)?;
    let q = linear_bf16_optional_bias(
        weights,
        &normalized,
        &format!("{prefix}.attn2.to_q.weight"),
        None,
        hidden.cols(),
    )?;
    let q_tap = download_if(capture_taps, &q)?;
    let key_name = format!("{prefix}.attn2.to_k.weight");
    let value_name = format!("{prefix}.attn2.to_v.weight");
    let packed_key = format!("{prefix}.attn2.to_kv.weight::standard-packed");
    let kv = packed_linear_bf16(
        weights,
        &normalized_encoder,
        &[&key_name, &value_name],
        &packed_key,
        hidden.cols(),
        12,
    )?;
    let k = gpu_slice_cols(&kv, 0, hidden.cols()).map_err(DiffusionError::model)?;
    let v = gpu_slice_cols(&kv, hidden.cols(), hidden.cols()).map_err(DiffusionError::model)?;
    let k_tap = download_if(capture_taps, &k)?;
    let v_tap = download_if(capture_taps, &v)?;
    let attention = gpu_attention_packed_cross_bf16(
        &q,
        &k,
        &v,
        12,
        1.0 / (64.0f32).sqrt(),
    )
    .map_err(DiffusionError::model)?;
    let attention = gpu_bf16_round(&attention).map_err(DiffusionError::model)?;
    let attention = linear_bf16(
        weights,
        &attention,
        &format!("{prefix}.attn2.to_out.0.weight"),
        &format!("{prefix}.attn2.to_out.0.bias"),
        hidden.cols(),
    )?;
    let attention_out_tap = download_if(capture_taps, &attention)?;
    let hidden = gpu_add_bf16(hidden, &attention).map_err(DiffusionError::model)?;
    let normalized_ff =
        fp32_layer_norm_to_bf16(weights, &hidden, &format!("{prefix}.norm3"), 1.0e-5)?;
    let norm3_tap = download_if(capture_taps, &normalized_ff)?;
    let expanded = linear_bf16(
        weights,
        &normalized_ff,
        &format!("{prefix}.ff.net.0.proj.weight"),
        &format!("{prefix}.ff.net.0.proj.bias"),
        hidden.cols() * 4,
    )?;
    let ff_in_tap = download_if(capture_taps, &expanded)?;
    let activated = gpu_gelu_erf(&expanded).map_err(DiffusionError::model)?;
    let update = linear_bf16(
        weights,
        &activated,
        &format!("{prefix}.ff.net.2.weight"),
        &format!("{prefix}.ff.net.2.bias"),
        hidden.cols(),
    )?;
    let ff_out_tap = download_if(capture_taps, &update)?;
    let output = gpu_add_bf16(&hidden, &update).map_err(DiffusionError::model)?;
    let taps = capture_taps.then_some(SkinTokensVaeCrossTap {
        norm2: norm2_tap,
        norm_cross: norm_cross_tap,
        q: q_tap,
        k: k_tap,
        v: v_tap,
        attention_out: attention_out_tap,
        norm3: norm3_tap,
        ff_in: ff_in_tap,
        ff_out: ff_out_tap,
    });
    Ok((output, taps))
}

fn vae_self_block(
    weights: &SkinTokensWeights,
    hidden: &GpuTensor,
    block: usize,
) -> Result<GpuTensor> {
    let prefix = format!("vae.model.cond_encoder.blocks.{block}");
    let normalized =
        fp32_layer_norm_to_bf16(weights, hidden, &format!("{prefix}.norm1"), 1.0e-5)?;
    let query_name = format!("{prefix}.attn1.to_q.weight");
    let key_name = format!("{prefix}.attn1.to_k.weight");
    let value_name = format!("{prefix}.attn1.to_v.weight");
    let packed_key = format!("{prefix}.attn1.to_qkv.weight::standard-packed");
    let qkv = packed_linear_bf16(
        weights,
        &normalized,
        &[&query_name, &key_name, &value_name],
        &packed_key,
        hidden.cols(),
        12,
    )?;
    let q = gpu_slice_cols(&qkv, 0, hidden.cols()).map_err(DiffusionError::model)?;
    let k = gpu_slice_cols(&qkv, hidden.cols(), hidden.cols()).map_err(DiffusionError::model)?;
    let v =
        gpu_slice_cols(&qkv, hidden.cols() * 2, hidden.cols()).map_err(DiffusionError::model)?;
    let attention = gpu_attention_packed_bf16(&q, &k, &v, 12, 1.0 / (64.0f32).sqrt())
        .map_err(DiffusionError::model)?;
    let attention = gpu_bf16_round(&attention).map_err(DiffusionError::model)?;
    let attention = linear_bf16(
        weights,
        &attention,
        &format!("{prefix}.attn1.to_out.0.weight"),
        &format!("{prefix}.attn1.to_out.0.bias"),
        hidden.cols(),
    )?;
    let hidden = gpu_add_bf16(hidden, &attention).map_err(DiffusionError::model)?;
    tripo_feed_forward(weights, &hidden, &prefix)
}

fn michelangelo_cross_block_inner(
    weights: &SkinTokensWeights,
    hidden: &GpuTensor,
    encoder: &GpuTensor,
    capture_taps: bool,
) -> Result<(GpuTensor, Option<SkinTokensMeshCrossTap>)> {
    const PREFIX: &str = "mesh_encoder.encoder.cross_attn";
    let q_input = layer_norm(weights, hidden, &format!("{PREFIX}.ln_1"), 1.0e-5)?;
    let kv_input = layer_norm(weights, encoder, &format!("{PREFIX}.ln_2"), 1.0e-5)?;
    let ln1_tap = download_if(capture_taps, &q_input)?;
    let ln2_tap = download_if(capture_taps, &kv_input)?;
    let q = linear_bf16_mm(
        weights,
        &q_input,
        &format!("{PREFIX}.attn.c_q.weight"),
        512,
    )?;
    let q_tap = download_if(capture_taps, &q)?;
    let kv = combined_packed_linear_bf16_mm(
        weights,
        &kv_input,
        &format!("{PREFIX}.attn.c_kv.weight"),
        512,
        8,
        2,
    )?;
    let kv_tap = download_if(capture_taps, &kv)?;
    let k = gpu_slice_cols(&kv, 0, 512).map_err(DiffusionError::model)?;
    let v = gpu_slice_cols(&kv, 512, 512).map_err(DiffusionError::model)?;
    let attention = gpu_attention_packed_cross_bf16(&q, &k, &v, 8, 1.0 / 8.0)
        .map_err(DiffusionError::model)?;
    let attention = gpu_bf16_round(&attention).map_err(DiffusionError::model)?;
    let flash_tap = download_if(capture_taps, &attention)?;
    let attention = linear_bf16_bias_epilogue(
        weights,
        &attention,
        &format!("{PREFIX}.attn.c_proj.weight"),
        &format!("{PREFIX}.attn.c_proj.bias"),
        512,
    )?;
    let to_out_tap = download_if(capture_taps, &attention)?;
    let hidden = gpu_add_bf16(hidden, &attention).map_err(DiffusionError::model)?;
    let normalized = layer_norm(weights, &hidden, &format!("{PREFIX}.ln_3"), 1.0e-5)?;
    let ln3_tap = download_if(capture_taps, &normalized)?;
    let expanded = linear_bf16_bias_epilogue(
        weights,
        &normalized,
        &format!("{PREFIX}.mlp.c_fc.weight"),
        &format!("{PREFIX}.mlp.c_fc.bias"),
        hidden.cols() * 4,
    )?;
    let ff_in_tap = download_if(capture_taps, &expanded)?;
    let activated = gpu_gelu_erf(&expanded).map_err(DiffusionError::model)?;
    let activated = gpu_bf16_round(&activated).map_err(DiffusionError::model)?;
    let gelu_tap = download_if(capture_taps, &activated)?;
    let update = linear_bf16_bias_epilogue(
        weights,
        &activated,
        &format!("{PREFIX}.mlp.c_proj.weight"),
        &format!("{PREFIX}.mlp.c_proj.bias"),
        hidden.cols(),
    )?;
    let ff_out_tap = download_if(capture_taps, &update)?;
    let output = gpu_add_bf16(&hidden, &update).map_err(DiffusionError::model)?;
    let taps = capture_taps.then_some(SkinTokensMeshCrossTap {
        ln1: ln1_tap,
        ln2: ln2_tap,
        q: q_tap,
        kv: kv_tap,
        flash: flash_tap,
        to_out: to_out_tap,
        ln3: ln3_tap,
        ff_in: ff_in_tap,
        gelu: gelu_tap,
        ff_out: ff_out_tap,
    });
    Ok((output, taps))
}

fn michelangelo_self_block(
    weights: &SkinTokensWeights,
    hidden: &GpuTensor,
    block: usize,
) -> Result<GpuTensor> {
    let prefix = format!("mesh_encoder.encoder.self_attn.resblocks.{block}");
    let normalized = layer_norm(weights, hidden, &format!("{prefix}.ln_1"), 1.0e-5)?;
    let qkv = combined_packed_linear_bf16_mm(
        weights,
        &normalized,
        &format!("{prefix}.attn.c_qkv.weight"),
        512,
        8,
        3,
    )?;
    let q = gpu_slice_cols(&qkv, 0, 512).map_err(DiffusionError::model)?;
    let k = gpu_slice_cols(&qkv, 512, 512).map_err(DiffusionError::model)?;
    let v = gpu_slice_cols(&qkv, 1024, 512).map_err(DiffusionError::model)?;
    let attention = gpu_attention_packed_bf16(&q, &k, &v, 8, 1.0 / 8.0)
        .map_err(DiffusionError::model)?;
    let attention = gpu_bf16_round(&attention).map_err(DiffusionError::model)?;
    let attention = linear_bf16_bias_epilogue(
        weights,
        &attention,
        &format!("{prefix}.attn.c_proj.weight"),
        &format!("{prefix}.attn.c_proj.bias"),
        512,
    )?;
    let hidden = gpu_add_bf16(hidden, &attention).map_err(DiffusionError::model)?;
    let normalized = layer_norm(weights, &hidden, &format!("{prefix}.ln_2"), 1.0e-5)?;
    let expanded = linear_bf16_bias_epilogue(
        weights,
        &normalized,
        &format!("{prefix}.mlp.c_fc.weight"),
        &format!("{prefix}.mlp.c_fc.bias"),
        hidden.cols() * 4,
    )?;
    let activated = gpu_gelu_erf(&expanded).map_err(DiffusionError::model)?;
    let activated = gpu_bf16_round(&activated).map_err(DiffusionError::model)?;
    let update = linear_bf16_bias_epilogue(
        weights,
        &activated,
        &format!("{prefix}.mlp.c_proj.weight"),
        &format!("{prefix}.mlp.c_proj.bias"),
        hidden.cols(),
    )?;
    gpu_add_bf16(&hidden, &update).map_err(DiffusionError::model)
}

pub fn project_condition(
    weights: &SkinTokensWeights,
    condition: &[f32],
    request_seed: u64,
    kind: SkinTokensConditionKind,
) -> Result<SkinTokensProjectedCondition> {
    let selection = select_condition_rows(condition, request_seed, kind)?;
    let (query_embed, key_value_embed) = match kind {
        SkinTokensConditionKind::SkinVae => {
            let query_embed = embed_condition_rows(&selection.selected, kind)?;
            let key_value_embed = embed_condition_rows(condition, kind)?;
            (
                gpu_upload(&query_embed, kind.tokens(), 54).map_err(DiffusionError::model)?,
                gpu_upload(&key_value_embed, condition.len() / 6, 54)
                    .map_err(DiffusionError::model)?,
            )
        }
        SkinTokensConditionKind::Michelangelo => {
            let query = gpu_upload(&selection.selected, kind.tokens(), 6)
                .map_err(DiffusionError::model)?;
            let key_value =
                gpu_upload(condition, condition.len() / 6, 6).map_err(DiffusionError::model)?;
            (
                gpu_skintokens_michelangelo_fourier(&query).map_err(DiffusionError::model)?,
                gpu_skintokens_michelangelo_fourier(&key_value)
                    .map_err(DiffusionError::model)?,
            )
        }
    };
    let (weight_name, bias_name, width) = projection_names(kind);
    let query = linear_bf16_bias_epilogue(
        weights,
        &query_embed,
        weight_name,
        bias_name,
        width,
    )?;
    let key_value = linear_bf16_bias_epilogue(
        weights,
        &key_value_embed,
        weight_name,
        bias_name,
        width,
    )?;
    Ok(SkinTokensProjectedCondition {
        selection,
        query,
        key_value,
    })
}

/// Encode the sampled point cloud through the complete three-block SkinVAE
/// condition tower and its 768->512 conditional latent projection.
fn encode_vae_condition_inner(
    weights: &SkinTokensWeights,
    condition: &[f32],
    request_seed: u64,
    capture_taps: bool,
    cancel: Option<&dyn Fn() -> bool>,
) -> Result<(SkinTokensVaeCondition, Option<SkinTokensVaeCrossTap>)> {
    check_cancel(cancel)?;
    let projected = project_condition(
        weights,
        condition,
        request_seed,
        SkinTokensConditionKind::SkinVae,
    )?;
    check_cancel(cancel)?;
    let (block0, taps) = vae_cross_block_inner(
        weights,
        &projected.query,
        &projected.key_value,
        0,
        capture_taps,
    )?;
    check_cancel(cancel)?;
    let block1 = vae_self_block(weights, &block0, 1)?;
    check_cancel(cancel)?;
    let block2 = vae_self_block(weights, &block1, 2)?;
    check_cancel(cancel)?;
    // torch.nn.LayerNorm runs outside autocast and returns f32 here; the
    // following BF16 linear re-rounds its activation operand exactly once.
    let normalized = layer_norm(
        weights,
        &block2,
        "vae.model.cond_encoder.norm_out",
        1.0e-5,
    )?;
    let latents = linear_bf16(
        weights,
        &normalized,
        "vae.model.cond_quant.weight",
        "vae.model.cond_quant.bias",
        512,
    )?;
    Ok((
        SkinTokensVaeCondition {
            selection: projected.selection,
            block0,
            block1,
            block2,
            normalized,
            latents,
        },
        taps,
    ))
}

pub fn encode_vae_condition(
    weights: &SkinTokensWeights,
    condition: &[f32],
    request_seed: u64,
) -> Result<SkinTokensVaeCondition> {
    encode_vae_condition_inner(weights, condition, request_seed, false, None)
        .map(|(result, _)| result)
}

/// Production VAE encoder with cooperative cancellation before projection and
/// between transformer blocks. An in-flight CUDA kernel remains atomic.
pub fn encode_vae_condition_controlled(
    weights: &SkinTokensWeights,
    condition: &[f32],
    request_seed: u64,
    cancel: Option<&dyn Fn() -> bool>,
) -> Result<SkinTokensVaeCondition> {
    encode_vae_condition_inner(weights, condition, request_seed, false, cancel)
        .map(|(result, _)| result)
}

pub fn encode_vae_condition_tapped(
    weights: &SkinTokensWeights,
    condition: &[f32],
    request_seed: u64,
) -> Result<(SkinTokensVaeCondition, SkinTokensVaeCrossTap)> {
    let (result, taps) =
        encode_vae_condition_inner(weights, condition, request_seed, true, None)?;
    Ok((result, taps.expect("capture_taps requested")))
}

fn encode_mesh_prefix_inner(
    weights: &SkinTokensWeights,
    condition: &[f32],
    capture_taps: bool,
    cancel: Option<&dyn Fn() -> bool>,
) -> Result<(SkinTokensMeshPrefix, Option<SkinTokensMeshEncoderTap>)> {
    // The released Michelangelo encoder hard-codes np.default_rng(seed=0) in
    // eval mode. This intentionally does not inherit the request seed.
    check_cancel(cancel)?;
    let projected = project_condition(
        weights,
        condition,
        0,
        SkinTokensConditionKind::Michelangelo,
    )?;
    check_cancel(cancel)?;
    let (mut cross, cross_operator_taps) = michelangelo_cross_block_inner(
        weights,
        &projected.query,
        &projected.key_value,
        capture_taps,
    )?;
    check_cancel(cancel)?;
    let cross_tap = if capture_taps {
        gpu_download(&cross).map_err(DiffusionError::model)?
    } else {
        Vec::new()
    };
    let mut block_taps = Vec::with_capacity(if capture_taps { 8 } else { 0 });
    for block in 0..8 {
        cross = michelangelo_self_block(weights, &cross, block)?;
        check_cancel(cancel)?;
        if capture_taps {
            block_taps.push(gpu_download(&cross).map_err(DiffusionError::model)?);
        }
    }
    // Like torch.nn.LayerNorm under autocast, the final encoder norm remains
    // f32 until the following BF16 linear consumes it.
    let encoded = layer_norm(
        weights,
        &cross,
        "mesh_encoder.encoder.ln_post",
        1.0e-5,
    )?;
    let normalized_tap = if capture_taps {
        gpu_download(&encoded).map_err(DiffusionError::model)?
    } else {
        Vec::new()
    };
    let output_linear = linear_bf16_bias_epilogue(
        weights,
        &encoded,
        "output_proj.0.weight",
        "output_proj.0.bias",
        896,
    )?;
    let output_linear_tap = if capture_taps {
        gpu_download(&output_linear).map_err(DiffusionError::model)?
    } else {
        Vec::new()
    };
    let rms_scale = weights.tensor_f32("output_proj.1.weight")?;
    let prefix = gpu_rms_norm_mul_bf16(
        &output_linear,
        896,
        SKIN_TOKENS_NEURAL_NAMESPACE,
        "output_proj.1",
        &rms_scale,
        f32::EPSILON,
    )
    .map_err(DiffusionError::model)?;
    let prefix_tap = if capture_taps {
        gpu_download(&prefix).map_err(DiffusionError::model)?
    } else {
        Vec::new()
    };
    let result = SkinTokensMeshPrefix {
        selection: projected.selection,
        encoded,
        prefix,
    };
    let taps = if capture_taps {
        Some(SkinTokensMeshEncoderTap {
            cross: cross_operator_taps.expect("cross taps requested"),
            cross_attention: cross_tap,
            blocks: block_taps,
            normalized: normalized_tap,
            output_linear: output_linear_tap,
            prefix: prefix_tap,
        })
    } else {
        None
    };
    Ok((result, taps))
}

/// Encode a sampled point cloud to the 512×896 continuous Qwen prefix. Mesh
/// query selection is fixed-seed official eval behavior; request stochasticity
/// begins only in constrained Qwen generation.
pub fn encode_mesh_prefix(
    weights: &SkinTokensWeights,
    condition: &[f32],
) -> Result<SkinTokensMeshPrefix> {
    encode_mesh_prefix_inner(weights, condition, false, None).map(|(result, _)| result)
}

/// Production Michelangelo encoder with cooperative cancellation before its
/// projection and between all nine attention blocks.
pub fn encode_mesh_prefix_controlled(
    weights: &SkinTokensWeights,
    condition: &[f32],
    cancel: Option<&dyn Fn() -> bool>,
) -> Result<SkinTokensMeshPrefix> {
    encode_mesh_prefix_inner(weights, condition, false, cancel).map(|(result, _)| result)
}

pub fn encode_mesh_prefix_tapped(
    weights: &SkinTokensWeights,
    condition: &[f32],
) -> Result<(SkinTokensMeshPrefix, SkinTokensMeshEncoderTap)> {
    let (result, taps) = encode_mesh_prefix_inner(weights, condition, true, None)?;
    Ok((result, taps.expect("capture_taps requested")))
}

/// Evict every streamed TokenRig matrix. Service unload calls this after
/// dropping resident activation/model state.
pub fn unload_skin_tokens_neural_weights() -> Result<()> {
    gpu_weight_cache_evict_prefix(SKIN_TOKENS_NEURAL_NAMESPACE)
        .map_err(DiffusionError::model)
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::{
        check_cancel, official_packed_source_column, permute_official_packed_bf16_rows,
        standard_packed_column_from_official,
    };

    #[test]
    fn controlled_encoders_surface_cancelled() {
        assert!(check_cancel(None).is_ok());
        let keep_running = || false;
        assert!(check_cancel(Some(&keep_running)).is_ok());
        let cancel = || true;
        assert!(matches!(
            check_cancel(Some(&cancel)),
            Err(crate::DiffusionError::Cancelled)
        ));
    }

    #[test]
    fn packed_projection_mapping_and_inverse_are_exact() {
        let width = 12;
        let heads = 3;
        let streams = 3;
        let expected_q = [0, 1, 2, 3, 12, 13, 14, 15, 24, 25, 26, 27];
        for (standard, expected) in expected_q.into_iter().enumerate() {
            assert_eq!(
                official_packed_source_column(standard, width, heads, streams).unwrap(),
                expected,
            );
        }
        let columns = width * streams;
        let mut seen = vec![false; columns];
        for standard in 0..columns {
            let official =
                official_packed_source_column(standard, width, heads, streams).unwrap();
            assert!(!seen[official]);
            seen[official] = true;
            assert_eq!(
                standard_packed_column_from_official(official, width, heads, streams).unwrap(),
                standard,
            );
        }
        assert!(seen.into_iter().all(|value| value));
    }

    #[test]
    fn packed_projection_permuter_moves_whole_bf16_rows() {
        let width = 4;
        let heads = 2;
        let streams = 2;
        let input_cols = 3;
        let mut official = Vec::new();
        for row in 0..width * streams {
            for col in 0..input_cols {
                official.extend_from_slice(&((row * 10 + col) as u16).to_le_bytes());
            }
        }
        let standard = permute_official_packed_bf16_rows(
            &official,
            input_cols,
            width,
            heads,
            streams,
        )
        .unwrap();
        let words = standard
            .chunks_exact(2)
            .map(|bytes| u16::from_le_bytes([bytes[0], bytes[1]]))
            .collect::<Vec<_>>();
        // Official rows [head0 K,V; head1 K,V] become [all K; all V].
        let expected_rows = [0, 1, 4, 5, 2, 3, 6, 7];
        for (standard_row, official_row) in expected_rows.into_iter().enumerate() {
            assert_eq!(
                &words[standard_row * input_cols..(standard_row + 1) * input_cols],
                &[
                    (official_row * 10) as u16,
                    (official_row * 10 + 1) as u16,
                    (official_row * 10 + 2) as u16,
                ],
            );
        }
    }
}
