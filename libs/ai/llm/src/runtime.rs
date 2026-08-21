use std::collections::{BTreeMap, BTreeSet};

use crate::metal_compiled::{
    create_context_main_buffer, create_context_main_buffer_no_copy, create_context_ro_buffer,
    execute_compiled_graph, execute_compiled_graph_in_active_batch,
    execute_compiled_graph_with_buffer_inputs, prepare_graph, warmup_affine_qmm_weights,
    MetalCompiledGraph, MetalContextBuffers, MetalGraphSession, MetalGraphTensorBufferCopy,
    MetalGraphTensorWrite, MetalPreparedGraph,
};
use crate::{
    ggml_row_size_for_type, BufferUsage, Context, GluOp, Graph, InitParams, Op, Prec, SortOrder,
    Tensor, TensorId, TensorType, TriType, UnaryOp, GGML_ROPE_TYPE_IMROPE, GGML_ROPE_TYPE_MROPE,
};
use makepad_ai_cuda::quant::{f16_to_f32, f32_to_f16, get_rows_ggml_bytes_cpu};
use makepad_ai_metal::{
    try_matmul_nt_ggml_bytes, try_rms_norm_mul_f32, BufferStorageMode, MetalBuffer,
    MetalDeviceFeatures, MetalRuntime,
};

use crate::error::{LlamaError, Result};
use crate::weights::LoadedGgufWeights;

#[derive(Clone, Debug)]
pub enum ProbeInputKind {
    TokenIds {
        token_embedding_name: String,
        token_embedding_scale: Option<f32>,
    },
    Embeddings {
        hidden_size: u32,
        input_type: TensorType,
    },
}

#[derive(Clone, Debug)]
pub struct LogitsProbeSpec {
    pub input: ProbeInputKind,
    pub output_norm_name: String,
    pub output_name: String,
    pub rms_epsilon: f32,
    pub final_logit_softcap: Option<f32>,
}

#[derive(Clone, Copy, Debug)]
pub struct GraphBatch {
    pub n_tokens: usize,
    pub n_outputs: usize,
}

#[derive(Clone, Debug)]
pub struct LogitsProbeGraph {
    pub graph: Graph,
    pub input_primary: TensorId,
    pub output_ids: TensorId,
    pub input_embed: TensorId,
    pub selected_embed: TensorId,
    pub result_norm: TensorId,
    pub result_output: TensorId,
}

pub struct CompiledLogitsProbeMetal {
    spec: LogitsProbeSpec,
    probe: LogitsProbeGraph,
    session: MetalGraphSession,
}

impl CompiledLogitsProbeMetal {
    pub fn runtime(&self) -> &MetalRuntime {
        self.session.runtime()
    }

    pub fn probe(&self) -> &LogitsProbeGraph {
        &self.probe
    }

    pub fn execute(
        &self,
        ctx: &Context,
        input: LogitsProbeInput<'_>,
        output_ids: &[i32],
    ) -> Result<LogitsProbeRun> {
        execute_prepared_logits_probe_metal(
            self.session.runtime(),
            ctx,
            &self.spec,
            &self.probe,
            self.session.compiled(),
            input,
            output_ids,
        )
    }
}

pub enum LogitsProbeInput<'a> {
    TokenIds(&'a [i32]),
    EmbeddingsF32 { data: &'a [f32], n_tokens: usize },
}

#[derive(Clone, Debug)]
pub struct LogitsProbeRun {
    pub logits: Vec<f32>,
    pub n_outputs: usize,
    pub vocab_size: usize,
}

#[derive(Clone, Debug)]
pub enum AttentionQueryLayout {
    Plain,
    InterleavedQueryGate { gate_activation: UnaryOp },
}

#[derive(Clone, Debug)]
pub struct AttentionRopeSpec {
    pub n_dims: i32,
    pub sections: [i32; 4],
    pub mode: i32,
    pub n_ctx_orig: i32,
    pub freq_base: f32,
    pub freq_scale: f32,
    pub ext_factor: f32,
    pub attn_factor: f32,
    pub beta_fast: f32,
    pub beta_slow: f32,
}

fn rope_position_component_count(rope: &AttentionRopeSpec) -> usize {
    if rope.mode == GGML_ROPE_TYPE_IMROPE || (rope.mode & GGML_ROPE_TYPE_MROPE) != 0 {
        4
    } else {
        1
    }
}

fn rope_position_tensor_len(rope: &AttentionRopeSpec, n_tokens: usize) -> Result<i64> {
    let n_positions = n_tokens
        .checked_mul(rope_position_component_count(rope))
        .ok_or_else(|| LlamaError::format("overflow computing rope position tensor length"))?;
    i64::try_from(n_positions)
        .map_err(|_| LlamaError::format("rope position length does not fit in i64"))
}

fn encode_rope_positions(
    rope: &AttentionRopeSpec,
    positions: &[i32],
    n_tokens: usize,
) -> Result<Vec<i32>> {
    let n_components = rope_position_component_count(rope);
    if n_components == 1 {
        if positions.len() != n_tokens {
            return Err(LlamaError::format(format!(
                "rope positions length mismatch: got {}, expected {}",
                positions.len(),
                n_tokens
            )));
        }
        return Ok(positions.to_vec());
    }

    let expanded_len = n_tokens
        .checked_mul(n_components)
        .ok_or_else(|| LlamaError::format("overflow computing expanded rope positions"))?;
    if positions.len() == expanded_len {
        return Ok(positions.to_vec());
    }
    if positions.len() != n_tokens {
        return Err(LlamaError::format(format!(
            "mrope positions length mismatch: got {}, expected {} or {}",
            positions.len(),
            n_tokens,
            expanded_len
        )));
    }

    let mut expanded = vec![0_i32; expanded_len];
    for component in 0..n_components {
        let start = component * n_tokens;
        let end = start + n_tokens;
        expanded[start..end].copy_from_slice(positions);
    }
    Ok(expanded)
}

fn causal_window_key_start(position: usize, causal_window: Option<usize>) -> usize {
    causal_window
        .map(|window| position.saturating_add(1).saturating_sub(window))
        .unwrap_or(0)
}

fn causal_mask_f16_bytes_with_window(n_tokens: usize, causal_window: Option<usize>) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(n_tokens * n_tokens * std::mem::size_of::<u16>());
    let zero = f32_to_f16(0.0);
    let neg_inf = f32_to_f16(f32::NEG_INFINITY);
    for query in 0..n_tokens {
        let key_start = causal_window_key_start(query, causal_window);
        for key in 0..n_tokens {
            let value = if key > query || key < key_start {
                neg_inf
            } else {
                zero
            };
            bytes.extend_from_slice(&value.to_le_bytes());
        }
    }
    bytes
}

fn causal_mask_f32_bytes_with_window(n_tokens: usize, causal_window: Option<usize>) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(n_tokens * n_tokens * std::mem::size_of::<f32>());
    for query in 0..n_tokens {
        let key_start = causal_window_key_start(query, causal_window);
        for key in 0..n_tokens {
            let value = if key > query || key < key_start {
                f32::NEG_INFINITY
            } else {
                0.0
            };
            bytes.extend_from_slice(&value.to_le_bytes());
        }
    }
    bytes
}

/// The inclusive absolute cache-row span one batch token may attend over.
///
/// `upper` is the row the token itself occupies; `start` is the first row it
/// is allowed to see, which is `lower` (the base of its slot's region) pushed
/// forward by any sliding-window limit. Everything outside `start..=upper` is
/// `-inf` in the mask.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct MaskRowSpan {
    start: usize,
    upper: usize,
}

/// Resolve one batch token's mask span from its absolute cache rows.
///
/// With a single sequence `lower` is 0 and this reduces to "attend to
/// `0..=position`", which is the whole behaviour this function used to have.
/// With a unified multi-slot arena, `lower` is the slot's `kv_base`, so the
/// window arithmetic still runs in WITHIN-SLOT coordinates (`upper - lower`)
/// and the result is shifted back into absolute rows. That shift is what
/// makes the mask block-diagonal without any tensor-shape change.
fn mask_row_span(
    upper: i32,
    lower: i32,
    key_count: usize,
    causal_window: Option<usize>,
) -> Result<MaskRowSpan> {
    let upper = usize::try_from(upper)
        .map_err(|_| LlamaError::format(format!("negative attention position {}", upper)))?;
    let lower = usize::try_from(lower).map_err(|_| {
        LlamaError::format(format!("negative attention key lower bound {}", lower))
    })?;
    if upper >= key_count {
        return Err(LlamaError::format(format!(
            "attention position {} exceeds key_count {}",
            upper, key_count
        )));
    }
    if lower > upper {
        return Err(LlamaError::format(format!(
            "attention key lower bound {} exceeds its position {}",
            lower, upper
        )));
    }
    Ok(MaskRowSpan {
        start: lower + causal_window_key_start(upper - lower, causal_window),
        upper,
    })
}

/// Per-token mask spans for a whole batch.
///
/// `upper_rows` are the absolute cache rows the batch's tokens occupy.
/// `lower_rows`, when present, are the per-token slot bases; when absent every
/// token may see from row 0, which is the single-sequence behaviour.
fn mask_row_spans(
    key_count: usize,
    upper_rows: &[i32],
    lower_rows: Option<&[i32]>,
    causal_window: Option<usize>,
) -> Result<Vec<MaskRowSpan>> {
    if let Some(lower_rows) = lower_rows {
        if lower_rows.len() != upper_rows.len() {
            return Err(LlamaError::format(format!(
                "attention mask needs {} key lower bounds, got {}",
                upper_rows.len(),
                lower_rows.len()
            )));
        }
    }
    upper_rows
        .iter()
        .enumerate()
        .map(|(index, &upper)| {
            let lower = lower_rows.map(|rows| rows[index]).unwrap_or(0);
            mask_row_span(upper, lower, key_count, causal_window)
        })
        .collect()
}

fn position_causal_mask_f16_bytes_with_window(
    key_count: usize,
    upper_rows: &[i32],
    lower_rows: Option<&[i32]>,
    causal_window: Option<usize>,
) -> Result<Vec<u8>> {
    let spans = mask_row_spans(key_count, upper_rows, lower_rows, causal_window)?;
    let mut bytes = Vec::with_capacity(
        spans
            .len()
            .checked_mul(key_count)
            .and_then(|v| v.checked_mul(std::mem::size_of::<u16>()))
            .ok_or_else(|| LlamaError::format("overflow computing attention decode mask bytes"))?,
    );
    let zero = f32_to_f16(0.0);
    let neg_inf = f32_to_f16(f32::NEG_INFINITY);
    for span in spans {
        for key in 0..key_count {
            let value = if key > span.upper || key < span.start {
                neg_inf
            } else {
                zero
            };
            bytes.extend_from_slice(&value.to_le_bytes());
        }
    }
    Ok(bytes)
}

fn position_causal_mask_f32_bytes_with_window(
    key_count: usize,
    upper_rows: &[i32],
    lower_rows: Option<&[i32]>,
    causal_window: Option<usize>,
) -> Result<Vec<u8>> {
    let spans = mask_row_spans(key_count, upper_rows, lower_rows, causal_window)?;
    let mut bytes = Vec::with_capacity(
        spans
            .len()
            .checked_mul(key_count)
            .and_then(|v| v.checked_mul(std::mem::size_of::<f32>()))
            .ok_or_else(|| LlamaError::format("overflow computing attention decode mask bytes"))?,
    );
    for span in spans {
        for key in 0..key_count {
            let value = if key > span.upper || key < span.start {
                f32::NEG_INFINITY
            } else {
                0.0
            };
            bytes.extend_from_slice(&value.to_le_bytes());
        }
    }
    Ok(bytes)
}

fn flash_attention_supported_head_dim(head_dim: u32) -> bool {
    matches!(
        head_dim,
        32 | 40 | 48 | 64 | 72 | 80 | 96 | 112 | 128 | 192 | 256 | 576
    )
}

fn should_use_flash_attention(head_dim: u32, n_tokens: usize) -> bool {
    if std::env::var("LLAMA_NO_FLASH_ATTN").is_ok() {
        return false;
    }
    if !flash_attention_supported_head_dim(head_dim) {
        return false;
    }
    // Decode-sized online-softmax flash stays for n_tokens < 20.
    // Prefill uses llama.cpp fattn-mma-f16 (D=256, ncols 8x8) only.
    // The naive flash was measured slower (1713 -> 1671). Rollback:
    // MKLLM_DISABLE_FATTN_MMA=1 restores batched GEMM for large n.
    if n_tokens < 20 {
        return true;
    }
    let fattn_disabled = std::env::var_os("MKLLM_DISABLE_FATTN_MMA")
        .map(|v| v == "1")
        .unwrap_or(false);
    !fattn_disabled && head_dim == 256
}

fn attention_mask_tensor_type(head_dim: u32, n_tokens: usize) -> TensorType {
    if should_use_flash_attention(head_dim, n_tokens) {
        TensorType::F16
    } else {
        TensorType::F32
    }
}

fn attention_mask_bytes_for_tensor(
    ctx: &Context,
    tensor_id: TensorId,
    n_tokens: usize,
    causal_window: Option<usize>,
) -> Result<Vec<u8>> {
    let tensor = require_tensor(ctx, tensor_id)?;
    match tensor.desc.ty {
        TensorType::F16 => Ok(causal_mask_f16_bytes_with_window(n_tokens, causal_window)),
        TensorType::F32 => Ok(causal_mask_f32_bytes_with_window(n_tokens, causal_window)),
        other => Err(LlamaError::unsupported(format!(
            "unsupported attention block mask tensor type {}",
            other.name()
        ))),
    }
}

fn position_attention_mask_bytes_for_tensor(
    ctx: &Context,
    tensor_id: TensorId,
    key_count: usize,
    upper_rows: &[i32],
    lower_rows: Option<&[i32]>,
    causal_window: Option<usize>,
) -> Result<Vec<u8>> {
    let tensor = require_tensor(ctx, tensor_id)?;
    match tensor.desc.ty {
        TensorType::F16 => position_causal_mask_f16_bytes_with_window(
            key_count,
            upper_rows,
            lower_rows,
            causal_window,
        ),
        TensorType::F32 => position_causal_mask_f32_bytes_with_window(
            key_count,
            upper_rows,
            lower_rows,
            causal_window,
        ),
        other => Err(LlamaError::unsupported(format!(
            "unsupported attention decode mask tensor type {}",
            other.name()
        ))),
    }
}

#[derive(Clone, Debug)]
pub struct AttentionBlockSpec {
    pub input: ProbeInputKind,
    pub input_norm_name: String,
    pub q_proj_name: String,
    pub q_proj_scale_name: Option<String>,
    pub q_layout: AttentionQueryLayout,
    pub k_proj_name: String,
    pub k_proj_scale_name: Option<String>,
    pub v_proj_name: Option<String>,
    pub v_proj_scale_name: Option<String>,
    pub output_proj_name: String,
    pub output_proj_scale_name: Option<String>,
    pub q_norm_name: Option<String>,
    pub k_norm_name: Option<String>,
    pub v_norm_epsilon: Option<f32>,
    pub q_head_dim: u32,
    pub q_head_count: u32,
    pub k_head_dim: u32,
    pub kv_head_count: u32,
    pub v_head_dim: u32,
    pub rms_epsilon: f32,
    pub rope: Option<AttentionRopeSpec>,
    pub rope_factors_name: Option<String>,
    pub attention_scale: f32,
    pub causal: bool,
    pub causal_window: Option<u32>,
    pub residual: bool,
}

#[derive(Clone, Debug)]
pub struct AttentionBlockGraph {
    pub graph: Graph,
    pub input_primary: TensorId,
    pub input_positions: Option<TensorId>,
    pub input_mask: Option<TensorId>,
    pub input_embed: TensorId,
    pub result_output: TensorId,
}

pub struct CompiledAttentionBlockMetal {
    spec: AttentionBlockSpec,
    block: AttentionBlockGraph,
    session: MetalGraphSession,
}

impl CompiledAttentionBlockMetal {
    pub fn runtime(&self) -> &MetalRuntime {
        self.session.runtime()
    }

    pub fn block(&self) -> &AttentionBlockGraph {
        &self.block
    }

    pub fn execute(
        &self,
        ctx: &Context,
        input: LogitsProbeInput<'_>,
        positions: &[i32],
    ) -> Result<AttentionBlockRun> {
        execute_prepared_attention_block_metal(
            self.session.runtime(),
            ctx,
            &self.spec,
            &self.block,
            self.session.compiled(),
            input,
            positions,
        )
    }
}

#[derive(Clone, Debug)]
pub struct AttentionBlockRun {
    pub hidden: Vec<f32>,
    pub n_tokens: usize,
    pub hidden_size: usize,
}

#[derive(Clone, Debug)]
pub struct AttentionKvCacheSpec {
    pub max_context: u32,
    pub max_sequences: u32,
    pub k_type: TensorType,
    pub v_type: TensorType,
}

#[derive(Clone, Debug)]
pub struct AttentionDecodeSpec {
    pub block: AttentionBlockSpec,
    pub cache: AttentionKvCacheSpec,
    pub cache_layer_index: u32,
    pub write_kv: bool,
}

#[derive(Clone, Debug)]
pub struct AttentionDecodeGraph {
    pub graph: Graph,
    pub input_primary: TensorId,
    pub input_positions: TensorId,
    pub input_write_indices: TensorId,
    pub input_rope_positions: Option<TensorId>,
    pub input_mask: Option<TensorId>,
    pub k_cache: TensorId,
    pub v_cache: TensorId,
    pub k_cache_view: TensorId,
    pub v_cache_view: TensorId,
    pub graph_key_count: usize,
    pub result_output: TensorId,
}

pub struct CompiledAttentionDecodeMetal {
    spec: AttentionDecodeSpec,
    decode: AttentionDecodeGraph,
    session: MetalGraphSession,
}

impl CompiledAttentionDecodeMetal {
    pub fn runtime(&self) -> &MetalRuntime {
        self.session.runtime()
    }

    pub fn decode(&self) -> &AttentionDecodeGraph {
        &self.decode
    }

    pub fn execute(
        &self,
        ctx: &mut Context,
        input: LogitsProbeInput<'_>,
        positions: &[i32],
        cache_tokens: usize,
    ) -> Result<AttentionBlockRun> {
        execute_prepared_attention_decode_metal(
            self.session.runtime(),
            ctx,
            &self.spec,
            &self.decode,
            self.session.compiled(),
            input,
            positions,
            cache_tokens,
        )
    }
}

#[derive(Clone, Debug)]
pub struct DeltaNetRecurrentBlockSpec {
    pub input: ProbeInputKind,
    pub embedding_length: u32,
    pub input_norm_name: String,
    pub merged_input_proj_name: Option<String>,
    pub qkv_proj_name: String,
    pub qkv_proj_scale_name: Option<String>,
    pub z_proj_name: String,
    pub z_proj_scale_name: Option<String>,
    pub beta_proj_name: String,
    pub beta_proj_scale_name: Option<String>,
    pub alpha_proj_name: String,
    pub alpha_proj_scale_name: Option<String>,
    pub dt_bias_name: String,
    pub a_name: String,
    pub conv_kernel_name: String,
    pub norm_name: String,
    pub output_proj_name: String,
    pub output_proj_scale_name: Option<String>,
    pub key_head_dim: u32,
    pub key_head_count: u32,
    pub value_head_dim: u32,
    pub value_head_count: u32,
    pub rms_epsilon: f32,
    pub residual: bool,
}

#[derive(Clone, Debug)]
pub struct DeltaNetRecurrentStateSpec {
    pub max_sequences: u32,
    pub r_type: TensorType,
    pub s_type: TensorType,
}

#[derive(Clone, Debug)]
pub struct DeltaNetRecurrentDecodeSpec {
    pub block: DeltaNetRecurrentBlockSpec,
    pub cache: DeltaNetRecurrentStateSpec,
}

#[derive(Clone, Debug)]
pub struct DeltaNetRecurrentDecodeGraph {
    pub graph: Graph,
    pub input_primary: TensorId,
    pub input_state_rows: TensorId,
    pub r_cache: TensorId,
    pub s_cache: TensorId,
    pub result_output: TensorId,
}

pub struct CompiledDeltaNetRecurrentDecodeMetal {
    spec: DeltaNetRecurrentDecodeSpec,
    decode: DeltaNetRecurrentDecodeGraph,
    session: MetalGraphSession,
}

impl CompiledDeltaNetRecurrentDecodeMetal {
    pub fn runtime(&self) -> &MetalRuntime {
        self.session.runtime()
    }

    pub fn decode(&self) -> &DeltaNetRecurrentDecodeGraph {
        &self.decode
    }

    pub fn execute(
        &self,
        ctx: &mut Context,
        input: LogitsProbeInput<'_>,
    ) -> Result<AttentionBlockRun> {
        execute_prepared_delta_net_recurrent_decode_metal(
            self.session.runtime(),
            ctx,
            &self.spec,
            &self.decode,
            self.session.compiled(),
            input,
        )
    }
}

#[derive(Clone, Debug)]
pub struct RmsNormSpec {
    pub weight_name: String,
    pub epsilon: f32,
}

#[derive(Clone, Copy, Debug)]
pub enum ExpertGatingFunc {
    SoftMax,
    Sigmoid,
    Identity,
}

#[derive(Clone, Debug)]
pub struct DenseGatedFfnSpec {
    pub gate_proj_name: String,
    pub up_proj_name: String,
    pub down_proj_name: String,
    pub gate_proj_scale_name: Option<String>,
    pub up_proj_scale_name: Option<String>,
    pub down_proj_scale_name: Option<String>,
    pub gate_activation: UnaryOp,
}

#[derive(Clone, Debug)]
pub struct DenseLayerFfnSpec {
    pub input_norm: Option<RmsNormSpec>,
    pub ffn: DenseGatedFfnSpec,
}

#[derive(Clone, Debug)]
pub struct HybridPerLayerInputProjectSpec {
    pub token_embedding_name: String,
    pub token_embedding_scale: Option<f32>,
    pub model_proj_name: String,
    pub model_proj_scale: Option<f32>,
    pub proj_norm: RmsNormSpec,
    pub hidden_size: u32,
    pub layer_count: u32,
    pub combine_scale: Option<f32>,
}

#[derive(Clone, Debug)]
pub struct HybridPerLayerInputLayerSpec {
    pub input_gate_name: String,
    pub proj_name: String,
    pub post_norm: RmsNormSpec,
    pub activation: UnaryOp,
}

#[derive(Clone, Debug)]
pub struct MoeSharedExpertSpec {
    pub ffn: DenseGatedFfnSpec,
    pub output_gate_name: Option<String>,
    pub output_gate_activation: UnaryOp,
}

#[derive(Clone, Debug)]
pub struct MoeFfnSpec {
    pub input: ProbeInputKind,
    pub input_norm: Option<RmsNormSpec>,
    pub router_proj_name: String,
    pub expert_count: u32,
    pub expert_used_count: u32,
    pub gating_func: ExpertGatingFunc,
    pub normalize_selected_weights: bool,
    pub weight_scale: f32,
    pub merged_gate_up_proj_name: Option<String>,
    pub gate_proj_name: Option<String>,
    pub gate_proj_scale_name: Option<String>,
    pub up_proj_name: String,
    pub up_proj_scale_name: Option<String>,
    pub down_proj_name: String,
    pub down_proj_scale_name: Option<String>,
    pub activation: UnaryOp,
    pub shared_expert: Option<MoeSharedExpertSpec>,
}

#[derive(Clone, Debug)]
pub struct MoeFfnGraph {
    pub graph: Graph,
    pub input_primary: TensorId,
    pub input_embed: TensorId,
    pub selected_experts: TensorId,
    pub result_output: TensorId,
}

pub struct CompiledMoeFfnMetal {
    spec: MoeFfnSpec,
    block: MoeFfnGraph,
    session: MetalGraphSession,
}

impl CompiledMoeFfnMetal {
    pub fn runtime(&self) -> &MetalRuntime {
        self.session.runtime()
    }

    pub fn block(&self) -> &MoeFfnGraph {
        &self.block
    }

    pub fn execute(&self, ctx: &Context, input: LogitsProbeInput<'_>) -> Result<MoeFfnRun> {
        execute_prepared_moe_ffn_metal(
            self.session.runtime(),
            ctx,
            &self.spec,
            &self.block,
            self.session.compiled(),
            input,
        )
    }
}

#[derive(Clone, Debug)]
pub struct MoeFfnRun {
    pub hidden: Vec<f32>,
    pub n_tokens: usize,
    pub hidden_size: usize,
    pub selected_experts: Vec<i32>,
    pub expert_used_count: usize,
}

#[derive(Clone, Debug)]
pub enum HybridLayerFfnSpec {
    Dense(DenseLayerFfnSpec),
    Moe(MoeFfnSpec),
}

#[derive(Clone, Debug)]
pub enum HybridLayerSpec {
    Attention {
        layer_index: u32,
        decode: AttentionDecodeSpec,
        post_attention_norm: Option<RmsNormSpec>,
        ffn: HybridLayerFfnSpec,
        post_ffn_norm: Option<RmsNormSpec>,
        per_layer_input: Option<HybridPerLayerInputLayerSpec>,
        output_scale_name: Option<String>,
    },
    Recurrent {
        layer_index: u32,
        decode: DeltaNetRecurrentDecodeSpec,
        ffn: HybridLayerFfnSpec,
    },
}

/// Post-final-norm hidden rows (llama.cpp's `t_h_nextn`) are written into a
/// persistent device tensor instead of only being read back to the host, so
/// the MTP draft graph can consume them without a round trip. Setting this
/// also keeps the last block at full `n_tokens` width and moves the
/// `n_outputs` narrowing to just before the LM head, so every batch position
/// gets a hidden row while the (very wide) logits stay narrow.
#[derive(Clone, Debug)]
pub struct HybridHiddenCarrySpec {
    /// Name of the `[hidden_size, rows]` F32 State tensor.
    pub tensor_name: String,
    pub hidden_size: u32,
}

/// The Qwen3.5/3.8 `blk.N.nextn.*` prologue that turns the trailing decoder
/// block into an MTP draft head:
/// `eh_proj(concat(rms_norm(embed(token), enorm), rms_norm(h_prev, hnorm)))`.
/// Mirrors llama.cpp `llama_model_qwen35::graph_mtp`.
#[derive(Clone, Debug)]
pub struct HybridMtpPrologueSpec {
    pub enorm: RmsNormSpec,
    pub hnorm: RmsNormSpec,
    pub eh_proj_name: String,
    pub eh_proj_scale_name: Option<String>,
    /// Name of the shared hidden-carry tensor the previous-position hidden
    /// rows are read from (same tensor the main graph writes).
    pub hidden_carry_name: String,
    pub hidden_size: u32,
}

#[derive(Clone, Debug)]
pub struct HybridDecodeSpec {
    pub input: ProbeInputKind,
    pub output_norm_name: String,
    pub output_name: String,
    pub rms_epsilon: f32,
    pub final_logit_softcap: Option<f32>,
    pub per_layer_input: Option<HybridPerLayerInputProjectSpec>,
    pub layers: Vec<HybridLayerSpec>,
    /// Write post-final-norm hidden rows to a persistent device tensor.
    pub hidden_carry: Option<HybridHiddenCarrySpec>,
    /// Present only on the MTP draft spec.
    pub mtp_prologue: Option<HybridMtpPrologueSpec>,
    /// Run the recurrent scan one token at a time and store the state after
    /// every token in its own cache row. Costs one extra state read+write per
    /// extra token, and makes speculative rollback a row-index change instead
    /// of a re-forward. Only set on the speculative verify spec.
    pub recurrent_checkpoints: bool,
}

#[derive(Clone, Debug)]
pub struct HybridAttentionCacheView {
    pub layer_index: u32,
    pub input_mask: Option<TensorId>,
    pub k_cache_view: TensorId,
    pub v_cache_view: TensorId,
    pub k_head_dim: i64,
    pub v_head_dim: i64,
    pub kv_head_count: i64,
    pub max_context: usize,
    pub graph_key_count: usize,
    pub max_sequences: i64,
    pub causal_window: Option<usize>,
}

#[derive(Clone, Debug)]
pub struct HybridDecodeBatchLayout {
    pub positions: Vec<i32>,
    pub attention_write_indices: Vec<i32>,
    pub attention_key_count: usize,
    pub recurrent_state_rows: Vec<i32>,
    pub output_ids: Vec<i32>,
    /// Pre-expanded M-RoPE positions, `[c0 over all tokens][c1][c2][c3]`.
    /// When absent, rope uses `positions` broadcast across components (the
    /// text-only behavior). Image spans need this: their h/w components
    /// diverge from the linear sequence index, while `positions` stays the
    /// linear index used for cache writes and attention masking.
    pub rope_positions: Option<Vec<i32>>,
    /// Hidden-carry rows this batch reads (MTP draft graph only), one per
    /// token: row `i` supplies the `h_{pos-1}` input for batch token `i`.
    pub hidden_read_rows: Vec<i32>,
    /// Hidden-carry rows this batch writes, one per token (graphs with
    /// `hidden_carry` set). Row `i` receives the post-final-norm hidden of
    /// batch token `i`.
    pub hidden_write_rows: Vec<i32>,
    /// First absolute cache row each token is allowed to attend to, one per
    /// token. Empty means "row 0" for every token, which is the
    /// single-sequence behaviour.
    ///
    /// This is the whole of the multi-slot attention contract. Slots share one
    /// flat KV arena; slot `s` owns rows `[kv_base(s), kv_base(s) + cap)` and
    /// writes at `kv_base(s) + position`. Setting the lower bound to
    /// `kv_base(s)` makes the mask block-diagonal, so a slot sees its own
    /// history and nothing else — with no change to any tensor shape, and so
    /// no change to any CUDA kernel.
    pub attention_key_lower_bounds: Vec<i32>,
}

impl HybridDecodeBatchLayout {
    pub fn from_contiguous_positions(
        positions: &[i32],
        attention_key_count: usize,
    ) -> Result<Self> {
        let output_ids = (0..positions.len())
            .map(|index| {
                i32::try_from(index).map_err(|_| {
                    LlamaError::format("hybrid decode output index does not fit in i32")
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Self::from_contiguous_positions_and_outputs(positions, attention_key_count, &output_ids)
    }

    pub fn from_contiguous_positions_and_outputs(
        positions: &[i32],
        attention_key_count: usize,
        output_ids: &[i32],
    ) -> Result<Self> {
        if positions.is_empty() {
            return Err(LlamaError::format(
                "hybrid decode batch layout requires at least one position",
            ));
        }
        if output_ids.is_empty() {
            return Err(LlamaError::format(
                "hybrid decode batch layout requires at least one output id",
            ));
        }
        if output_ids.iter().copied().any(|row| {
            row < 0
                || usize::try_from(row)
                    .ok()
                    .map(|row| row >= positions.len())
                    .unwrap_or(true)
        }) {
            return Err(LlamaError::format(format!(
                "hybrid decode output ids {:?} exceed batch size {}",
                output_ids,
                positions.len()
            )));
        }
        Ok(Self {
            positions: positions.to_vec(),
            attention_write_indices: positions.to_vec(),
            attention_key_count,
            recurrent_state_rows: vec![0],
            output_ids: output_ids.to_vec(),
            rope_positions: None,
            hidden_read_rows: Vec::new(),
            hidden_write_rows: Vec::new(),
            attention_key_lower_bounds: Vec::new(),
        })
    }

    /// Per-token attention lower bounds, or `None` when every token may attend
    /// from row 0 (the single-sequence case).
    pub(crate) fn attention_key_lower_bounds(&self) -> Option<&[i32]> {
        (!self.attention_key_lower_bounds.is_empty()).then_some(&self.attention_key_lower_bounds)
    }

    fn validate(&self) -> Result<()> {
        if self.positions.is_empty() {
            return Err(LlamaError::format(
                "hybrid decode batch layout requires at least one position",
            ));
        }
        if self.attention_write_indices.len() != self.positions.len() {
            return Err(LlamaError::format(format!(
                "hybrid decode batch layout write-index length mismatch: got {}, expected {}",
                self.attention_write_indices.len(),
                self.positions.len()
            )));
        }
        if self.attention_key_count == 0 {
            return Err(LlamaError::format(
                "hybrid decode batch layout requires attention_key_count >= 1",
            ));
        }
        if !self.attention_key_lower_bounds.is_empty()
            && self.attention_key_lower_bounds.len() != self.positions.len()
        {
            return Err(LlamaError::format(format!(
                "hybrid decode batch layout needs {} attention key lower bounds, got {}",
                self.positions.len(),
                self.attention_key_lower_bounds.len()
            )));
        }
        for (index, &lower) in self.attention_key_lower_bounds.iter().enumerate() {
            let upper = self.attention_write_indices[index];
            if lower < 0 || lower > upper {
                return Err(LlamaError::format(format!(
                    "hybrid decode batch layout attention key lower bound {} for token {} is not \
                     within [0, {}]",
                    lower, index, upper
                )));
            }
        }
        if self.output_ids.is_empty() {
            return Err(LlamaError::format(
                "hybrid decode batch layout requires at least one output id",
            ));
        }
        if self.output_ids.iter().copied().any(|row| {
            row < 0
                || usize::try_from(row)
                    .ok()
                    .map(|row| row >= self.positions.len())
                    .unwrap_or(true)
        }) {
            return Err(LlamaError::format(format!(
                "hybrid decode batch layout output ids {:?} exceed batch size {}",
                self.output_ids,
                self.positions.len()
            )));
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct HybridAttentionCacheIds {
    pub k_cache: TensorId,
    pub v_cache: TensorId,
}

#[derive(Clone, Debug)]
pub struct HybridRecurrentCacheIds {
    pub r_cache: TensorId,
    pub s_cache: TensorId,
}

#[derive(Clone, Debug, Default)]
pub struct HybridSharedCacheTensorIds {
    pub attention: BTreeMap<u32, HybridAttentionCacheIds>,
    pub recurrent: BTreeMap<u32, HybridRecurrentCacheIds>,
}

#[derive(Clone, Debug)]
pub struct HybridMoeSelection {
    pub layer_index: u32,
    pub selected_experts: TensorId,
    pub expert_used_count: usize,
}

#[derive(Clone, Debug)]
pub struct HybridDecodeGraph {
    pub graph: Graph,
    pub input_primary: TensorId,
    pub input_per_layer_primary: Option<TensorId>,
    pub input_output_ids: TensorId,
    pub input_positions: Option<TensorId>,
    pub input_attention_write_indices: Option<TensorId>,
    pub input_rope_positions: Option<TensorId>,
    pub input_recurrent_state_rows: Option<TensorId>,
    /// MTP draft graph: which hidden-carry rows feed this batch.
    pub input_hidden_read_rows: Option<TensorId>,
    /// Graphs with `hidden_carry`: which hidden-carry rows this batch writes.
    pub input_hidden_write_rows: Option<TensorId>,
    pub attention_cache_views: Vec<HybridAttentionCacheView>,
    pub moe_selected_experts: Vec<HybridMoeSelection>,
    pub state_updates: Vec<TensorId>,
    /// Debug-only graph outputs selected by `MAKEPAD_LLAMA_TRACE_ALL`.
    /// Keeping the ids on the shared graph makes Metal and CUDA pin and
    /// read back the exact same intermediate tensors for parity diagnosis.
    pub debug_outputs: Vec<TensorId>,
    pub result_hidden: TensorId,
    pub result_logits: TensorId,
}

pub struct CompiledHybridDecodeMetal {
    spec: HybridDecodeSpec,
    decode: HybridDecodeGraph,
    graph_ctx: Context,
    session: MetalGraphSession,
}

impl CompiledHybridDecodeMetal {
    pub fn runtime(&self) -> &MetalRuntime {
        self.session.runtime()
    }

    pub fn decode(&self) -> &HybridDecodeGraph {
        &self.decode
    }

    pub fn graph_ctx(&self) -> &Context {
        &self.graph_ctx
    }

    pub fn graph_ctx_mut(&mut self) -> &mut Context {
        &mut self.graph_ctx
    }

    pub fn execute(
        &mut self,
        input: LogitsProbeInput<'_>,
        positions: &[i32],
        cache_tokens: usize,
    ) -> Result<HybridDecodeRun> {
        let mut layout =
            HybridDecodeBatchLayout::from_contiguous_positions(positions, cache_tokens)?;
        if self.decode.input_recurrent_state_rows.is_none() {
            layout.recurrent_state_rows.clear();
        }
        self.execute_with_layout(input, &layout)
    }

    pub fn execute_with_layout(
        &mut self,
        input: LogitsProbeInput<'_>,
        layout: &HybridDecodeBatchLayout,
    ) -> Result<HybridDecodeRun> {
        execute_prepared_hybrid_decode_metal(
            self.session.runtime(),
            &mut self.graph_ctx,
            &self.spec,
            &self.decode,
            self.session.compiled(),
            input,
            layout,
            HybridDecodeOutputConfig::FULL,
        )
    }

    pub fn execute_logits_only(
        &mut self,
        input: LogitsProbeInput<'_>,
        positions: &[i32],
        cache_tokens: usize,
    ) -> Result<HybridDecodeRun> {
        let mut layout =
            HybridDecodeBatchLayout::from_contiguous_positions(positions, cache_tokens)?;
        if self.decode.input_recurrent_state_rows.is_none() {
            layout.recurrent_state_rows.clear();
        }
        self.execute_logits_only_with_layout(input, &layout)
    }

    pub fn execute_logits_only_with_layout(
        &mut self,
        input: LogitsProbeInput<'_>,
        layout: &HybridDecodeBatchLayout,
    ) -> Result<HybridDecodeRun> {
        execute_prepared_hybrid_decode_metal(
            self.session.runtime(),
            &mut self.graph_ctx,
            &self.spec,
            &self.decode,
            self.session.compiled(),
            input,
            layout,
            HybridDecodeOutputConfig::LOGITS_ONLY,
        )
    }
}

#[derive(Clone, Debug)]
pub struct HybridDecodeRun {
    pub hidden: Vec<f32>,
    pub logits: Vec<f32>,
    pub n_tokens: usize,
    pub hidden_size: usize,
    pub vocab_size: usize,
    pub selected_experts: Vec<(u32, Vec<i32>)>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HybridDecodeOutputConfig {
    pub capture_hidden: bool,
    pub capture_selected_experts: bool,
}

impl HybridDecodeOutputConfig {
    pub const FULL: Self = Self {
        capture_hidden: true,
        capture_selected_experts: true,
    };

    pub const LOGITS_ONLY: Self = Self {
        capture_hidden: false,
        capture_selected_experts: false,
    };
}

impl Default for HybridDecodeOutputConfig {
    fn default() -> Self {
        Self::FULL
    }
}

#[derive(Clone, Debug)]
pub struct HybridCacheSpec {
    pub n_ctx_seq: u32,
    pub n_seq_max: u32,
    pub attention_layers: Vec<u32>,
    pub recurrent_layers: Vec<u32>,
    pub attention_k_width: u64,
    pub attention_v_width: u64,
    pub recurrent_r_width: u64,
    pub recurrent_s_width: u64,
    pub attention_k_type: TensorType,
    pub attention_v_type: TensorType,
    pub recurrent_r_type: TensorType,
    pub recurrent_s_type: TensorType,
}

#[derive(Clone, Copy, Debug)]
pub struct HybridCacheShape {
    pub n_ctx_seq: u32,
    pub n_seq_max: u32,
}

#[derive(Clone, Copy, Debug)]
pub struct HybridCacheTypes {
    pub attention_k_type: TensorType,
    pub attention_v_type: TensorType,
    pub recurrent_r_type: TensorType,
    pub recurrent_s_type: TensorType,
}

#[derive(Clone, Debug)]
pub struct HybridCacheTemplate {
    pub attention_layers: Vec<u32>,
    pub recurrent_layers: Vec<u32>,
    pub attention_k_width: u64,
    pub attention_v_width: u64,
    pub recurrent_r_width: u64,
    pub recurrent_s_width: u64,
}

impl HybridCacheTemplate {
    pub fn materialize(&self, shape: HybridCacheShape, types: HybridCacheTypes) -> HybridCacheSpec {
        HybridCacheSpec {
            n_ctx_seq: shape.n_ctx_seq,
            n_seq_max: shape.n_seq_max,
            attention_layers: self.attention_layers.clone(),
            recurrent_layers: self.recurrent_layers.clone(),
            attention_k_width: self.attention_k_width,
            attention_v_width: self.attention_v_width,
            recurrent_r_width: self.recurrent_r_width,
            recurrent_s_width: self.recurrent_s_width,
            attention_k_type: types.attention_k_type,
            attention_v_type: types.attention_v_type,
            recurrent_r_type: types.recurrent_r_type,
            recurrent_s_type: types.recurrent_s_type,
        }
    }
}

#[derive(Clone, Debug)]
pub struct HybridCacheLayout {
    pub spec: HybridCacheSpec,
    pub attention_k_bytes_per_layer: usize,
    pub attention_v_bytes_per_layer: usize,
    pub recurrent_r_bytes_per_layer: usize,
    pub recurrent_s_bytes_per_layer: usize,
    pub total_attention_k_bytes: usize,
    pub total_attention_v_bytes: usize,
    pub total_recurrent_r_bytes: usize,
    pub total_recurrent_s_bytes: usize,
    pub total_bytes: usize,
}

impl HybridCacheLayout {
    pub fn new(spec: HybridCacheSpec) -> Result<Self> {
        let attention_k_bytes_per_layer = bytes_for_elements(
            spec.attention_k_type,
            spec.attention_k_width
                .checked_mul(u64::from(spec.n_ctx_seq))
                .and_then(|v| v.checked_mul(u64::from(spec.n_seq_max)))
                .ok_or_else(|| LlamaError::format("overflow computing key-cache elements"))?,
        )?;
        let attention_v_bytes_per_layer = bytes_for_elements(
            spec.attention_v_type,
            spec.attention_v_width
                .checked_mul(u64::from(spec.n_ctx_seq))
                .and_then(|v| v.checked_mul(u64::from(spec.n_seq_max)))
                .ok_or_else(|| LlamaError::format("overflow computing value-cache elements"))?,
        )?;
        let recurrent_r_bytes_per_layer = bytes_for_elements(
            spec.recurrent_r_type,
            spec.recurrent_r_width
                .checked_mul(u64::from(spec.n_seq_max))
                .ok_or_else(|| LlamaError::format("overflow computing recurrent-r elements"))?,
        )?;
        let recurrent_s_bytes_per_layer = bytes_for_elements(
            spec.recurrent_s_type,
            spec.recurrent_s_width
                .checked_mul(u64::from(spec.n_seq_max))
                .ok_or_else(|| LlamaError::format("overflow computing recurrent-s elements"))?,
        )?;

        let total_attention_k_bytes = attention_k_bytes_per_layer
            .checked_mul(spec.attention_layers.len())
            .ok_or_else(|| LlamaError::format("overflow computing total key-cache bytes"))?;
        let total_attention_v_bytes = attention_v_bytes_per_layer
            .checked_mul(spec.attention_layers.len())
            .ok_or_else(|| LlamaError::format("overflow computing total value-cache bytes"))?;
        let total_recurrent_r_bytes = recurrent_r_bytes_per_layer
            .checked_mul(spec.recurrent_layers.len())
            .ok_or_else(|| LlamaError::format("overflow computing total recurrent-r bytes"))?;
        let total_recurrent_s_bytes = recurrent_s_bytes_per_layer
            .checked_mul(spec.recurrent_layers.len())
            .ok_or_else(|| LlamaError::format("overflow computing total recurrent-s bytes"))?;
        let total_bytes = total_attention_k_bytes
            .checked_add(total_attention_v_bytes)
            .and_then(|v| v.checked_add(total_recurrent_r_bytes))
            .and_then(|v| v.checked_add(total_recurrent_s_bytes))
            .ok_or_else(|| LlamaError::format("overflow computing total hybrid-cache bytes"))?;

        Ok(Self {
            spec,
            attention_k_bytes_per_layer,
            attention_v_bytes_per_layer,
            recurrent_r_bytes_per_layer,
            recurrent_s_bytes_per_layer,
            total_attention_k_bytes,
            total_attention_v_bytes,
            total_recurrent_r_bytes,
            total_recurrent_s_bytes,
            total_bytes,
        })
    }
}

pub fn build_logits_probe_graph(
    ctx: &mut Context,
    tensor_ids: &BTreeMap<String, TensorId>,
    spec: &LogitsProbeSpec,
    batch: GraphBatch,
) -> Result<LogitsProbeGraph> {
    let input_primary = match &spec.input {
        ProbeInputKind::TokenIds { .. } => ctx
            .new_named_tensor(
                "probe.inp_tokens",
                TensorType::I32,
                1,
                &[batch.n_tokens as i64],
                BufferUsage::Activations,
            )
            .map_err(LlamaError::format)?,
        ProbeInputKind::Embeddings {
            hidden_size,
            input_type,
        } => ctx
            .new_named_tensor(
                "probe.inp_embd",
                *input_type,
                2,
                &[i64::from(*hidden_size), batch.n_tokens as i64],
                BufferUsage::Activations,
            )
            .map_err(LlamaError::format)?,
    };
    mark_input(ctx, input_primary)?;

    let output_ids = ctx
        .new_named_tensor(
            "probe.out_ids",
            TensorType::I32,
            1,
            &[batch.n_outputs as i64],
            BufferUsage::Activations,
        )
        .map_err(LlamaError::format)?;
    mark_input(ctx, output_ids)?;

    let input_embed = match &spec.input {
        ProbeInputKind::TokenIds {
            token_embedding_name,
            token_embedding_scale,
        } => {
            let token_embd = require_tensor_id(tensor_ids, token_embedding_name)?;
            let input_embed = ctx
                .get_rows(token_embd, input_primary, BufferUsage::Activations)
                .map_err(LlamaError::format)?;
            apply_optional_input_scale(
                ctx,
                input_embed,
                *token_embedding_scale,
                "probe.input_embed_scaled",
            )?
        }
        ProbeInputKind::Embeddings { .. } => input_primary,
    };
    ctx.set_tensor_name(input_embed, "probe.input_embed")
        .map_err(LlamaError::format)?;

    let selected_embed = ctx
        .get_rows(input_embed, output_ids, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(selected_embed, "probe.selected_embed")
        .map_err(LlamaError::format)?;

    let result_norm = ctx
        .rms_norm_eps(selected_embed, spec.rms_epsilon, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(result_norm, "probe.result_norm")
        .map_err(LlamaError::format)?;

    let output_norm = require_tensor_id(tensor_ids, &spec.output_norm_name)?;
    let result_norm_scaled = ctx
        .binary_like_a(Op::Mul, result_norm, output_norm, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(result_norm_scaled, "probe.result_norm_scaled")
        .map_err(LlamaError::format)?;

    let output = require_tensor_id(tensor_ids, &spec.output_name)?;
    let mut result_output = ctx
        .mul_mat(output, result_norm_scaled, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    if let Some(softcap) = spec.final_logit_softcap {
        result_output = ctx
            .scale(result_output, 1.0 / softcap, BufferUsage::Activations)
            .map_err(LlamaError::format)?;
        result_output = ctx
            .unary(result_output, UnaryOp::Tanh, BufferUsage::Activations)
            .map_err(LlamaError::format)?;
        result_output = ctx
            .scale(result_output, softcap, BufferUsage::Activations)
            .map_err(LlamaError::format)?;
    }
    ctx.set_tensor_name(result_output, "probe.result_output")
        .map_err(LlamaError::format)?;
    mark_output(ctx, result_output)?;

    let mut graph = Graph::new();
    graph
        .build_forward_expand(ctx, result_output)
        .map_err(LlamaError::format)?;

    Ok(LogitsProbeGraph {
        graph,
        input_primary,
        output_ids,
        input_embed,
        selected_embed,
        result_norm,
        result_output,
    })
}

pub fn prepare_logits_probe_graph(
    ctx: &mut Context,
    tensor_ids: &BTreeMap<String, TensorId>,
    spec: &LogitsProbeSpec,
    batch: GraphBatch,
    features: MetalDeviceFeatures,
) -> Result<(LogitsProbeGraph, MetalPreparedGraph)> {
    let probe = build_logits_probe_graph(ctx, tensor_ids, spec, batch)?;
    let prepared = prepare_graph(ctx, &probe.graph, features).map_err(LlamaError::format)?;
    Ok((probe, prepared))
}

pub fn compile_logits_probe_metal(
    weights: &mut LoadedGgufWeights,
    spec: &LogitsProbeSpec,
    batch: GraphBatch,
) -> Result<CompiledLogitsProbeMetal> {
    let runtime = MetalRuntime::new().map_err(LlamaError::unsupported)?;
    let (probe, prepared) = prepare_logits_probe_graph(
        &mut weights.ctx,
        &weights.tensor_ids,
        spec,
        batch,
        runtime.features(),
    )?;
    let session = MetalGraphSession::from_runtime(
        runtime,
        &weights.ctx,
        &prepared,
        BufferStorageMode::Private,
        BufferStorageMode::Private,
    )
    .map_err(LlamaError::format)?;

    Ok(CompiledLogitsProbeMetal {
        spec: spec.clone(),
        probe,
        session,
    })
}

pub fn execute_prepared_logits_probe_metal(
    runtime: &MetalRuntime,
    ctx: &Context,
    spec: &LogitsProbeSpec,
    probe: &LogitsProbeGraph,
    compiled: &MetalCompiledGraph,
    input: LogitsProbeInput<'_>,
    output_ids: &[i32],
) -> Result<LogitsProbeRun> {
    if output_ids.is_empty() {
        return Err(LlamaError::format(
            "logits probe requires at least one output id",
        ));
    }

    let input_primary = match (&spec.input, input) {
        (ProbeInputKind::TokenIds { .. }, LogitsProbeInput::TokenIds(token_ids)) => {
            i32_slice_as_bytes(token_ids).to_vec()
        }
        (
            ProbeInputKind::Embeddings {
                hidden_size,
                input_type,
            },
            LogitsProbeInput::EmbeddingsF32 { data, n_tokens },
        ) => {
            if *input_type != TensorType::F32 {
                return Err(LlamaError::unsupported(format!(
                    "graph Metal probe currently expects F32 embeddings, got {}",
                    input_type.name()
                )));
            }
            let expected = (*hidden_size as usize)
                .checked_mul(n_tokens)
                .ok_or_else(|| LlamaError::format("overflow computing embedding input size"))?;
            if data.len() != expected {
                return Err(LlamaError::format(format!(
                    "embedding input length mismatch: got {}, expected {}",
                    data.len(),
                    expected
                )));
            }
            f32_slice_as_bytes(data).to_vec()
        }
        (ProbeInputKind::TokenIds { .. }, LogitsProbeInput::EmbeddingsF32 { .. }) => {
            return Err(LlamaError::format(
                "logits probe spec expects token ids but embeddings were provided",
            ));
        }
        (ProbeInputKind::Embeddings { .. }, LogitsProbeInput::TokenIds(_)) => {
            return Err(LlamaError::format(
                "logits probe spec expects embeddings but token ids were provided",
            ));
        }
    };

    let execution = execute_compiled_graph(
        runtime,
        ctx,
        compiled,
        &[
            MetalGraphTensorWrite {
                tensor_id: probe.input_primary,
                bytes: &input_primary,
            },
            MetalGraphTensorWrite {
                tensor_id: probe.output_ids,
                bytes: i32_slice_as_bytes(output_ids),
            },
        ],
        &[probe.result_output],
    )
    .map_err(LlamaError::format)?;

    let result_bytes = execution
        .outputs
        .get(&probe.result_output)
        .ok_or_else(|| LlamaError::format("compiled probe did not produce result_output bytes"))?;
    let logits = f32_bytes_to_vec(result_bytes)?;
    let output = ctx
        .tensor(probe.result_output)
        .ok_or_else(|| LlamaError::format("probe result_output tensor is invalid"))?;
    let vocab_size = ne_usize(output, 0)?;
    let n_outputs = ne_usize(output, 1)?;

    Ok(LogitsProbeRun {
        logits,
        n_outputs,
        vocab_size,
    })
}

pub fn execute_logits_probe_graph_metal(
    weights: &mut LoadedGgufWeights,
    spec: &LogitsProbeSpec,
    input: LogitsProbeInput<'_>,
    output_ids: &[i32],
) -> Result<LogitsProbeRun> {
    let compiled = compile_logits_probe_metal(
        weights,
        spec,
        GraphBatch {
            n_tokens: match &input {
                LogitsProbeInput::TokenIds(token_ids) => token_ids.len(),
                LogitsProbeInput::EmbeddingsF32 { n_tokens, .. } => *n_tokens,
            },
            n_outputs: output_ids.len(),
        },
    )?;
    compiled.execute(&weights.ctx, input, output_ids)
}

pub fn execute_logits_probe_graph_metal_cached(
    compiled: &CompiledLogitsProbeMetal,
    weights: &LoadedGgufWeights,
    input: LogitsProbeInput<'_>,
    output_ids: &[i32],
) -> Result<LogitsProbeRun> {
    compiled.execute(&weights.ctx, input, output_ids)
}

fn build_attention_mha_output(
    ctx: &mut Context,
    q: TensorId,
    k: TensorId,
    v: TensorId,
    input_mask: Option<TensorId>,
    q_head_dim: u32,
    attention_scale: f32,
    n_tokens: usize,
    allow_flash_attention: bool,
    prefix: &str,
) -> Result<TensorId> {
    let v_trans = {
        let v_tensor = require_tensor(ctx, v)?;
        v_tensor.nb[1] > v_tensor.nb[2]
    };
    let n_stream = require_tensor(ctx, k)?.ne[3];
    let q_tensor = require_tensor(ctx, q)?.clone();
    if n_stream <= 0 || q_tensor.ne[2] % n_stream != 0 {
        return Err(LlamaError::format(format!(
            "attention q tensor shape {:?} is incompatible with stream count {}",
            q_tensor.ne, n_stream
        )));
    }
    let n_stream_usize = usize::try_from(n_stream)
        .map_err(|_| LlamaError::format("attention stream count does not fit in usize"))?;

    let q = ctx
        .view_4d(
            q,
            q_tensor.ne[0],
            q_tensor.ne[1],
            q_tensor.ne[2] / n_stream,
            n_stream,
            q_tensor.nb[1],
            q_tensor.nb[2],
            q_tensor.nb[3] / n_stream_usize,
            0,
        )
        .map_err(LlamaError::format)?;
    let q = ctx.permute(q, [0, 2, 1, 3]).map_err(LlamaError::format)?;
    let mut k = ctx.permute(k, [0, 2, 1, 3]).map_err(LlamaError::format)?;
    let mut v = ctx.permute(v, [0, 2, 1, 3]).map_err(LlamaError::format)?;

    let use_flash_attention =
        allow_flash_attention && should_use_flash_attention(q_head_dim, n_tokens);

    if use_flash_attention {
        if v_trans {
            v = ctx.transpose(v).map_err(LlamaError::format)?;
        }

        if require_tensor(ctx, k)?.desc.ty == TensorType::F32 {
            k = cast_tensor_to_type(ctx, k, TensorType::F16, BufferUsage::Activations)?;
            ctx.set_tensor_name(k, format!("{prefix}.k_flash"))
                .map_err(LlamaError::format)?;
        }

        if require_tensor(ctx, v)?.desc.ty == TensorType::F32 {
            v = cast_tensor_to_type(ctx, v, TensorType::F16, BufferUsage::Activations)?;
            ctx.set_tensor_name(v, format!("{prefix}.v_flash"))
                .map_err(LlamaError::format)?;
        }

        let attn = ctx
            .flash_attn_ext(
                q,
                k,
                v,
                input_mask,
                attention_scale,
                0.0,
                0.0,
                BufferUsage::Activations,
            )
            .map_err(LlamaError::format)?;
        ctx.flash_attn_ext_set_prec(attn, Prec::F32)
            .map_err(LlamaError::format)?;
        let attn_tensor = require_tensor(ctx, attn)?.clone();
        let attn = ctx
            .reshape(
                attn,
                &[
                    attn_tensor.ne[0] * attn_tensor.ne[1],
                    attn_tensor.ne[2] * attn_tensor.ne[3],
                ],
            )
            .map_err(LlamaError::format)?;
        ctx.set_tensor_name(attn, format!("{prefix}.attn_flat"))
            .map_err(LlamaError::format)?;
        return Ok(attn);
    }

    let mut kq = ctx
        .mul_mat(k, q, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(kq, format!("{prefix}.kq"))
        .map_err(LlamaError::format)?;
    kq = ctx
        .soft_max_ext(
            kq,
            input_mask,
            attention_scale,
            0.0,
            BufferUsage::Activations,
        )
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(kq, format!("{prefix}.kq_soft_max"))
        .map_err(LlamaError::format)?;

    if !v_trans {
        v = ctx.transpose(v).map_err(LlamaError::format)?;
        v = ctx.cont(v).map_err(LlamaError::format)?;
    }
    let v_for_matmul = v;
    ctx.set_tensor_name(v_for_matmul, format!("{prefix}.v_cont"))
        .map_err(LlamaError::format)?;

    let kqv = ctx
        .mul_mat(v_for_matmul, kq, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(kqv, format!("{prefix}.kqv"))
        .map_err(LlamaError::format)?;
    let attn = ctx.permute(kqv, [0, 2, 1, 3]).map_err(LlamaError::format)?;
    let attn_tensor = require_tensor(ctx, attn)?.clone();
    let attn = ctx
        .cont_2d(
            attn,
            attn_tensor.ne[0] * attn_tensor.ne[1],
            attn_tensor.ne[2] * attn_tensor.ne[3],
        )
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(attn, format!("{prefix}.attn_flat"))
        .map_err(LlamaError::format)?;
    Ok(attn)
}

pub fn build_attention_block_graph(
    ctx: &mut Context,
    tensor_ids: &BTreeMap<String, TensorId>,
    spec: &AttentionBlockSpec,
    n_tokens: usize,
) -> Result<AttentionBlockGraph> {
    if n_tokens == 0 {
        return Err(LlamaError::format(
            "attention block graph requires at least one token",
        ));
    }

    let input_primary = match &spec.input {
        ProbeInputKind::TokenIds { .. } => ctx
            .new_named_tensor(
                "attn.inp_tokens",
                TensorType::I32,
                1,
                &[n_tokens as i64],
                BufferUsage::Activations,
            )
            .map_err(LlamaError::format)?,
        ProbeInputKind::Embeddings {
            hidden_size,
            input_type,
        } => ctx
            .new_named_tensor(
                "attn.inp_embd",
                *input_type,
                2,
                &[i64::from(*hidden_size), n_tokens as i64],
                BufferUsage::Activations,
            )
            .map_err(LlamaError::format)?,
    };
    mark_input(ctx, input_primary)?;

    let input_positions = if spec.rope.is_some() {
        let rope = spec.rope.as_ref().unwrap();
        let positions = ctx
            .new_named_tensor(
                "attn.inp_pos",
                TensorType::I32,
                1,
                &[rope_position_tensor_len(rope, n_tokens)?],
                BufferUsage::Activations,
            )
            .map_err(LlamaError::format)?;
        mark_input(ctx, positions)?;
        Some(positions)
    } else {
        None
    };
    let input_mask = if spec.causal {
        let mask = ctx
            .new_named_tensor(
                "attn.kq_mask",
                attention_mask_tensor_type(spec.q_head_dim, n_tokens),
                4,
                &[n_tokens as i64, n_tokens as i64, 1, 1],
                BufferUsage::Activations,
            )
            .map_err(LlamaError::format)?;
        mark_input(ctx, mask)?;
        Some(mask)
    } else {
        None
    };

    let input_embed = match &spec.input {
        ProbeInputKind::TokenIds {
            token_embedding_name,
            token_embedding_scale,
        } => {
            let token_embd = require_tensor_id(tensor_ids, token_embedding_name)?;
            let input_embed = ctx
                .get_rows(token_embd, input_primary, BufferUsage::Activations)
                .map_err(LlamaError::format)?;
            apply_optional_input_scale(
                ctx,
                input_embed,
                *token_embedding_scale,
                "attn.input_embed_scaled",
            )?
        }
        ProbeInputKind::Embeddings { .. } => input_primary,
    };
    ctx.set_tensor_name(input_embed, "attn.input_embed")
        .map_err(LlamaError::format)?;

    let input_norm = build_rms_norm_mul(
        ctx,
        tensor_ids,
        input_embed,
        spec.rms_epsilon,
        &spec.input_norm_name,
        "attn.input_norm",
    )?;

    let q_weight = require_tensor_id(tensor_ids, &spec.q_proj_name)?;
    let mut q_proj = ctx
        .mul_mat(q_weight, input_norm, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    q_proj = apply_optional_proj_scale(
        ctx,
        tensor_ids,
        q_proj,
        &spec.q_proj_scale_name,
        "attn.q_proj_scaled",
    )?;
    ctx.set_tensor_name(q_proj, "attn.q_proj")
        .map_err(LlamaError::format)?;

    let q_gate_stride = usize::try_from(spec.q_head_dim)
        .ok()
        .and_then(|v| v.checked_mul(2))
        .and_then(|v| v.checked_mul(std::mem::size_of::<f32>()))
        .ok_or_else(|| LlamaError::format("overflow computing q/gate stride"))?;
    let q_token_stride = q_gate_stride
        .checked_mul(usize::try_from(spec.q_head_count).map_err(|_| {
            LlamaError::format(format!(
                "q_head_count {} does not fit in usize",
                spec.q_head_count
            ))
        })?)
        .ok_or_else(|| LlamaError::format("overflow computing q/gate token stride"))?;
    let gate_offset = usize::try_from(spec.q_head_dim)
        .ok()
        .and_then(|v| v.checked_mul(std::mem::size_of::<f32>()))
        .ok_or_else(|| LlamaError::format("overflow computing gate offset"))?;

    let (mut q_states, gate) = match spec.q_layout {
        AttentionQueryLayout::Plain => {
            let q = ctx
                .reshape(
                    q_proj,
                    &[
                        i64::from(spec.q_head_dim),
                        i64::from(spec.q_head_count),
                        n_tokens as i64,
                    ],
                )
                .map_err(LlamaError::format)?;
            (q, None)
        }
        AttentionQueryLayout::InterleavedQueryGate { gate_activation } => {
            let q = ctx
                .view_3d(
                    q_proj,
                    i64::from(spec.q_head_dim),
                    i64::from(spec.q_head_count),
                    n_tokens as i64,
                    q_gate_stride,
                    q_token_stride,
                    0,
                )
                .map_err(LlamaError::format)?;
            let gate_view = ctx
                .view_3d(
                    q_proj,
                    i64::from(spec.q_head_dim),
                    i64::from(spec.q_head_count),
                    n_tokens as i64,
                    q_gate_stride,
                    q_token_stride,
                    gate_offset,
                )
                .map_err(LlamaError::format)?;
            let gate_cont = ctx
                .cont_2d(
                    gate_view,
                    i64::from(spec.q_head_dim) * i64::from(spec.q_head_count),
                    n_tokens as i64,
                )
                .map_err(LlamaError::format)?;
            let gate = ctx
                .unary(gate_cont, gate_activation, BufferUsage::Activations)
                .map_err(LlamaError::format)?;
            ctx.set_tensor_name(gate, "attn.gate")
                .map_err(LlamaError::format)?;
            (q, Some(gate))
        }
    };
    ctx.set_tensor_name(q_states, "attn.q_states")
        .map_err(LlamaError::format)?;
    let q_pre_store = ctx
        .view_2d(
            q_states,
            i64::from(spec.q_head_dim) * i64::from(spec.q_head_count),
            n_tokens as i64,
            ctx.tensor(q_states)
                .ok_or_else(|| LlamaError::format("invalid q_states tensor"))?
                .nb[2],
            0,
        )
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(q_pre_store, "attn.q_pre_store")
        .map_err(LlamaError::format)?;

    if let Some(q_norm_name) = &spec.q_norm_name {
        q_states = build_rms_norm_mul(
            ctx,
            tensor_ids,
            q_states,
            spec.rms_epsilon,
            q_norm_name,
            "attn.q_norm",
        )?;
    }
    let q_norm_store = ctx
        .view_2d(
            q_states,
            i64::from(spec.q_head_dim) * i64::from(spec.q_head_count),
            n_tokens as i64,
            ctx.tensor(q_states)
                .ok_or_else(|| LlamaError::format("invalid q_states tensor"))?
                .nb[2],
            0,
        )
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(q_norm_store, "attn.q_norm_store")
        .map_err(LlamaError::format)?;

    let k_weight = require_tensor_id(tensor_ids, &spec.k_proj_name)?;
    let mut k_states = ctx
        .mul_mat(k_weight, input_norm, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    k_states = apply_optional_proj_scale(
        ctx,
        tensor_ids,
        k_states,
        &spec.k_proj_scale_name,
        "attn.k_states_scaled",
    )?;
    k_states = ctx
        .reshape(
            k_states,
            &[
                i64::from(spec.k_head_dim),
                i64::from(spec.kv_head_count),
                n_tokens as i64,
            ],
        )
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(k_states, "attn.k_states")
        .map_err(LlamaError::format)?;

    let mut v_states = if let Some(v_proj_name) = &spec.v_proj_name {
        let v_weight = require_tensor_id(tensor_ids, v_proj_name)?;
        let mut v_states = ctx
            .mul_mat(v_weight, input_norm, BufferUsage::Activations)
            .map_err(LlamaError::format)?;
        v_states = apply_optional_proj_scale(
            ctx,
            tensor_ids,
            v_states,
            &spec.v_proj_scale_name,
            "attn.v_states_scaled",
        )?;
        v_states = ctx
            .reshape(
                v_states,
                &[
                    i64::from(spec.v_head_dim),
                    i64::from(spec.kv_head_count),
                    n_tokens as i64,
                ],
            )
            .map_err(LlamaError::format)?;
        ctx.set_tensor_name(v_states, "attn.v_states")
            .map_err(LlamaError::format)?;
        v_states
    } else {
        k_states
    };

    if let Some(k_norm_name) = &spec.k_norm_name {
        k_states = build_rms_norm_mul(
            ctx,
            tensor_ids,
            k_states,
            spec.rms_epsilon,
            k_norm_name,
            "attn.k_norm",
        )?;
    }
    if let Some(v_norm_epsilon) = spec.v_norm_epsilon {
        v_states = build_rms_norm(ctx, v_states, v_norm_epsilon, "attn.v_norm")?;
    }
    let k_norm_store = ctx
        .view_2d(
            k_states,
            i64::from(spec.k_head_dim) * i64::from(spec.kv_head_count),
            n_tokens as i64,
            ctx.tensor(k_states)
                .ok_or_else(|| LlamaError::format("invalid k_states tensor"))?
                .nb[2],
            0,
        )
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(k_norm_store, "attn.k_norm_store")
        .map_err(LlamaError::format)?;

    if let Some(rope) = &spec.rope {
        let positions = input_positions.ok_or_else(|| {
            LlamaError::format(
                "attention block rope was requested without an input positions tensor",
            )
        })?;
        let rope_factors = spec
            .rope_factors_name
            .as_ref()
            .map(|name| require_tensor_id(tensor_ids, name))
            .transpose()?;
        q_states = ctx
            .rope_multi(
                q_states,
                positions,
                rope_factors,
                rope.n_dims,
                rope.sections,
                rope.mode,
                rope.n_ctx_orig,
                rope.freq_base,
                rope.freq_scale,
                rope.ext_factor,
                rope.attn_factor,
                rope.beta_fast,
                rope.beta_slow,
                BufferUsage::Activations,
            )
            .map_err(LlamaError::format)?;
        k_states = ctx
            .rope_multi(
                k_states,
                positions,
                rope_factors,
                rope.n_dims,
                rope.sections,
                rope.mode,
                rope.n_ctx_orig,
                rope.freq_base,
                rope.freq_scale,
                rope.ext_factor,
                rope.attn_factor,
                rope.beta_fast,
                rope.beta_slow,
                BufferUsage::Activations,
            )
            .map_err(LlamaError::format)?;
    }

    let q_store = ctx
        .view_2d(
            q_states,
            i64::from(spec.q_head_dim) * i64::from(spec.q_head_count),
            n_tokens as i64,
            ctx.tensor(q_states)
                .ok_or_else(|| LlamaError::format("invalid q_states tensor"))?
                .nb[2],
            0,
        )
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(q_store, "attn.q_store")
        .map_err(LlamaError::format)?;
    let k_store = ctx
        .view_2d(
            k_states,
            i64::from(spec.k_head_dim) * i64::from(spec.kv_head_count),
            n_tokens as i64,
            ctx.tensor(k_states)
                .ok_or_else(|| LlamaError::format("invalid k_states tensor"))?
                .nb[2],
            0,
        )
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(k_store, "attn.k_store")
        .map_err(LlamaError::format)?;
    let v_store = ctx
        .view_2d(
            v_states,
            i64::from(spec.v_head_dim) * i64::from(spec.kv_head_count),
            n_tokens as i64,
            ctx.tensor(v_states)
                .ok_or_else(|| LlamaError::format("invalid v_states tensor"))?
                .nb[2],
            0,
        )
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(v_store, "attn.v_store")
        .map_err(LlamaError::format)?;

    let mut attn = build_attention_mha_output(
        ctx,
        q_states,
        k_states,
        v_states,
        input_mask,
        spec.q_head_dim,
        spec.attention_scale,
        n_tokens,
        true,
        "attn",
    )?;

    if let Some(gate) = gate {
        attn = ctx
            .binary_like_a(Op::Mul, attn, gate, BufferUsage::Activations)
            .map_err(LlamaError::format)?;
        ctx.set_tensor_name(attn, "attn.attn_gated")
            .map_err(LlamaError::format)?;
    }

    let output_weight = require_tensor_id(tensor_ids, &spec.output_proj_name)?;
    let mut result_output = ctx
        .mul_mat(output_weight, attn, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    result_output = apply_optional_proj_scale(
        ctx,
        tensor_ids,
        result_output,
        &spec.output_proj_scale_name,
        "attn.output_proj_scaled",
    )?;
    ctx.set_tensor_name(result_output, "attn.output_proj")
        .map_err(LlamaError::format)?;

    if spec.residual {
        result_output = ctx
            .binary_like_a(
                Op::Add,
                result_output,
                input_embed,
                BufferUsage::Activations,
            )
            .map_err(LlamaError::format)?;
        ctx.set_tensor_name(result_output, "attn.output_residual")
            .map_err(LlamaError::format)?;
    }
    mark_output(ctx, result_output)?;

    let mut graph = Graph::new();
    graph
        .build_forward_expand(ctx, result_output)
        .map_err(LlamaError::format)?;

    Ok(AttentionBlockGraph {
        graph,
        input_primary,
        input_positions,
        input_mask,
        input_embed,
        result_output,
    })
}

pub fn prepare_attention_block_graph(
    ctx: &mut Context,
    tensor_ids: &BTreeMap<String, TensorId>,
    spec: &AttentionBlockSpec,
    n_tokens: usize,
    features: MetalDeviceFeatures,
) -> Result<(AttentionBlockGraph, MetalPreparedGraph)> {
    let block = build_attention_block_graph(ctx, tensor_ids, spec, n_tokens)?;
    let prepared = prepare_graph(ctx, &block.graph, features).map_err(LlamaError::format)?;
    Ok((block, prepared))
}

pub fn compile_attention_block_metal(
    weights: &mut LoadedGgufWeights,
    spec: &AttentionBlockSpec,
    n_tokens: usize,
) -> Result<CompiledAttentionBlockMetal> {
    let runtime = MetalRuntime::new().map_err(LlamaError::unsupported)?;
    let (block, prepared) = prepare_attention_block_graph(
        &mut weights.ctx,
        &weights.tensor_ids,
        spec,
        n_tokens,
        runtime.features(),
    )?;
    let session = MetalGraphSession::from_runtime(
        runtime,
        &weights.ctx,
        &prepared,
        BufferStorageMode::Private,
        BufferStorageMode::Private,
    )
    .map_err(LlamaError::format)?;

    Ok(CompiledAttentionBlockMetal {
        spec: spec.clone(),
        block,
        session,
    })
}

pub fn execute_prepared_attention_block_metal(
    runtime: &MetalRuntime,
    ctx: &Context,
    spec: &AttentionBlockSpec,
    block: &AttentionBlockGraph,
    compiled: &MetalCompiledGraph,
    input: LogitsProbeInput<'_>,
    positions: &[i32],
) -> Result<AttentionBlockRun> {
    let input_primary = match (&spec.input, input) {
        (ProbeInputKind::TokenIds { .. }, LogitsProbeInput::TokenIds(token_ids)) => {
            if token_ids.len() != positions.len() {
                return Err(LlamaError::format(format!(
                    "attention block token/position length mismatch: {} vs {}",
                    token_ids.len(),
                    positions.len()
                )));
            }
            i32_slice_as_bytes(token_ids).to_vec()
        }
        (
            ProbeInputKind::Embeddings {
                hidden_size,
                input_type,
            },
            LogitsProbeInput::EmbeddingsF32 { data, n_tokens },
        ) => {
            if *input_type != TensorType::F32 {
                return Err(LlamaError::unsupported(format!(
                    "attention block currently expects F32 embeddings, got {}",
                    input_type.name()
                )));
            }
            if n_tokens != positions.len() {
                return Err(LlamaError::format(format!(
                    "attention block embedding/position length mismatch: {} vs {}",
                    n_tokens,
                    positions.len()
                )));
            }
            let expected = (*hidden_size as usize)
                .checked_mul(n_tokens)
                .ok_or_else(|| {
                    LlamaError::format("overflow computing attention embedding input size")
                })?;
            if data.len() != expected {
                return Err(LlamaError::format(format!(
                    "attention embedding input length mismatch: got {}, expected {}",
                    data.len(),
                    expected
                )));
            }
            f32_slice_as_bytes(data).to_vec()
        }
        (ProbeInputKind::TokenIds { .. }, LogitsProbeInput::EmbeddingsF32 { .. }) => {
            return Err(LlamaError::format(
                "attention block spec expects token ids but embeddings were provided",
            ));
        }
        (ProbeInputKind::Embeddings { .. }, LogitsProbeInput::TokenIds(_)) => {
            return Err(LlamaError::format(
                "attention block spec expects embeddings but token ids were provided",
            ));
        }
    };

    let mut writes = vec![MetalGraphTensorWrite {
        tensor_id: block.input_primary,
        bytes: &input_primary,
    }];
    let n_tokens = positions.len();
    let rope_positions = spec
        .rope
        .as_ref()
        .map(|rope| encode_rope_positions(rope, positions, n_tokens))
        .transpose()?;
    if let Some(input_positions) = block.input_positions {
        writes.push(MetalGraphTensorWrite {
            tensor_id: input_positions,
            bytes: i32_slice_as_bytes(rope_positions.as_deref().ok_or_else(|| {
                LlamaError::format("attention block rope positions were not prepared")
            })?),
        });
    } else if !positions.is_empty() {
        return Err(LlamaError::format(
            "attention block received positions for a graph that does not use rope",
        ));
    }
    let mask_bytes = block
        .input_mask
        .map(|input_mask| {
            attention_mask_bytes_for_tensor(
                ctx,
                input_mask,
                n_tokens,
                spec.causal_window
                    .map(|window| usize::try_from(window).unwrap_or(usize::MAX)),
            )
        })
        .transpose()?;
    if let Some(input_mask) = block.input_mask {
        writes.push(MetalGraphTensorWrite {
            tensor_id: input_mask,
            bytes: mask_bytes.as_deref().ok_or_else(|| {
                LlamaError::format("attention block causal mask was not prepared")
            })?,
        });
    }

    let execution = execute_compiled_graph(runtime, ctx, compiled, &writes, &[block.result_output])
        .map_err(LlamaError::format)?;

    let result_bytes = execution.outputs.get(&block.result_output).ok_or_else(|| {
        LlamaError::format("compiled attention block did not produce result bytes")
    })?;
    let hidden = f32_bytes_to_vec(result_bytes)?;
    let output = ctx
        .tensor(block.result_output)
        .ok_or_else(|| LlamaError::format("attention result_output tensor is invalid"))?;

    Ok(AttentionBlockRun {
        hidden,
        hidden_size: ne_usize(output, 0)?,
        n_tokens: ne_usize(output, 1)?,
    })
}

pub fn execute_attention_block_graph_metal(
    weights: &mut LoadedGgufWeights,
    spec: &AttentionBlockSpec,
    input: LogitsProbeInput<'_>,
    positions: &[i32],
) -> Result<AttentionBlockRun> {
    let compiled = compile_attention_block_metal(weights, spec, positions.len())?;
    compiled.execute(&weights.ctx, input, positions)
}

pub fn execute_attention_block_graph_metal_cached(
    compiled: &CompiledAttentionBlockMetal,
    weights: &LoadedGgufWeights,
    input: LogitsProbeInput<'_>,
    positions: &[i32],
) -> Result<AttentionBlockRun> {
    compiled.execute(&weights.ctx, input, positions)
}

#[derive(Clone, Debug)]
struct BuiltAttentionDecode {
    input_mask: Option<TensorId>,
    k_cache: TensorId,
    v_cache: TensorId,
    k_cache_written: TensorId,
    v_cache_written: TensorId,
    k_cache_view: TensorId,
    v_cache_view: TensorId,
    result_output: TensorId,
}

fn build_attention_decode_from_hidden(
    ctx: &mut Context,
    tensor_ids: &BTreeMap<String, TensorId>,
    spec: &AttentionDecodeSpec,
    shared_cache: Option<&HybridAttentionCacheIds>,
    input_embed: TensorId,
    _input_positions: TensorId,
    input_write_indices: TensorId,
    input_rope_positions: Option<TensorId>,
    n_tokens: usize,
    attention_key_count: usize,
    shared_input_mask: Option<TensorId>,
    prefix: &str,
) -> Result<BuiltAttentionDecode> {
    let block = &spec.block;
    let n_tokens_i64 =
        i64::try_from(n_tokens).map_err(|_| LlamaError::format("n_tokens does not fit in i64"))?;
    let max_context = usize::try_from(spec.cache.max_context).map_err(|_| {
        LlamaError::format(format!(
            "attention decode max_context {} does not fit in usize",
            spec.cache.max_context
        ))
    })?;
    if attention_key_count == 0 || attention_key_count > max_context {
        return Err(LlamaError::format(format!(
            "attention decode key_count {} is outside 1..={}",
            attention_key_count, max_context
        )));
    }

    let input_norm = build_rms_norm_mul(
        ctx,
        tensor_ids,
        input_embed,
        block.rms_epsilon,
        &block.input_norm_name,
        &format!("{prefix}.input_norm"),
    )?;

    let q_weight = require_tensor_id(tensor_ids, &block.q_proj_name)?;
    let mut q_proj = ctx
        .mul_mat(q_weight, input_norm, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    q_proj = apply_optional_proj_scale(
        ctx,
        tensor_ids,
        q_proj,
        &block.q_proj_scale_name,
        &format!("{prefix}.q_proj_scaled"),
    )?;
    ctx.set_tensor_name(q_proj, format!("{prefix}.q_proj"))
        .map_err(LlamaError::format)?;

    let q_gate_stride = usize::try_from(block.q_head_dim)
        .ok()
        .and_then(|v| v.checked_mul(2))
        .and_then(|v| v.checked_mul(std::mem::size_of::<f32>()))
        .ok_or_else(|| LlamaError::format("overflow computing q/gate stride"))?;
    let q_token_stride = q_gate_stride
        .checked_mul(usize::try_from(block.q_head_count).map_err(|_| {
            LlamaError::format(format!(
                "q_head_count {} does not fit in usize",
                block.q_head_count
            ))
        })?)
        .ok_or_else(|| LlamaError::format("overflow computing q/gate token stride"))?;
    let gate_offset = usize::try_from(block.q_head_dim)
        .ok()
        .and_then(|v| v.checked_mul(std::mem::size_of::<f32>()))
        .ok_or_else(|| LlamaError::format("overflow computing gate offset"))?;

    let (mut q_states, gate) = match block.q_layout {
        AttentionQueryLayout::Plain => {
            let q = ctx
                .reshape(
                    q_proj,
                    &[
                        i64::from(block.q_head_dim),
                        i64::from(block.q_head_count),
                        n_tokens_i64,
                    ],
                )
                .map_err(LlamaError::format)?;
            (q, None)
        }
        AttentionQueryLayout::InterleavedQueryGate { gate_activation } => {
            let q = ctx
                .view_3d(
                    q_proj,
                    i64::from(block.q_head_dim),
                    i64::from(block.q_head_count),
                    n_tokens_i64,
                    q_gate_stride,
                    q_token_stride,
                    0,
                )
                .map_err(LlamaError::format)?;
            let gate_view = ctx
                .view_3d(
                    q_proj,
                    i64::from(block.q_head_dim),
                    i64::from(block.q_head_count),
                    n_tokens_i64,
                    q_gate_stride,
                    q_token_stride,
                    gate_offset,
                )
                .map_err(LlamaError::format)?;
            let gate_cont = ctx
                .cont_2d(
                    gate_view,
                    i64::from(block.q_head_dim) * i64::from(block.q_head_count),
                    n_tokens_i64,
                )
                .map_err(LlamaError::format)?;
            let gate = ctx
                .unary(gate_cont, gate_activation, BufferUsage::Activations)
                .map_err(LlamaError::format)?;
            ctx.set_tensor_name(gate, format!("{prefix}.gate"))
                .map_err(LlamaError::format)?;
            (q, Some(gate))
        }
    };
    ctx.set_tensor_name(q_states, format!("{prefix}.q_states"))
        .map_err(LlamaError::format)?;
    let q_pre_store = ctx
        .view_2d(
            q_states,
            i64::from(block.q_head_dim) * i64::from(block.q_head_count),
            n_tokens_i64,
            ctx.tensor(q_states)
                .ok_or_else(|| LlamaError::format("invalid q_states tensor"))?
                .nb[2],
            0,
        )
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(q_pre_store, format!("{prefix}.q_pre_store"))
        .map_err(LlamaError::format)?;

    if let Some(q_norm_name) = &block.q_norm_name {
        q_states = build_rms_norm_mul(
            ctx,
            tensor_ids,
            q_states,
            block.rms_epsilon,
            q_norm_name,
            &format!("{prefix}.q_norm"),
        )?;
    }
    let q_norm_store = ctx
        .view_2d(
            q_states,
            i64::from(block.q_head_dim) * i64::from(block.q_head_count),
            n_tokens_i64,
            ctx.tensor(q_states)
                .ok_or_else(|| LlamaError::format("invalid q_states tensor"))?
                .nb[2],
            0,
        )
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(q_norm_store, format!("{prefix}.q_norm_store"))
        .map_err(LlamaError::format)?;

    let k_weight = require_tensor_id(tensor_ids, &block.k_proj_name)?;
    let mut k_states = ctx
        .mul_mat(k_weight, input_norm, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    k_states = apply_optional_proj_scale(
        ctx,
        tensor_ids,
        k_states,
        &block.k_proj_scale_name,
        &format!("{prefix}.k_states_scaled"),
    )?;
    k_states = ctx
        .reshape(
            k_states,
            &[
                i64::from(block.k_head_dim),
                i64::from(block.kv_head_count),
                n_tokens_i64,
            ],
        )
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(k_states, format!("{prefix}.k_states"))
        .map_err(LlamaError::format)?;

    let mut v_states = if let Some(v_proj_name) = &block.v_proj_name {
        let v_weight = require_tensor_id(tensor_ids, v_proj_name)?;
        let mut v_states = ctx
            .mul_mat(v_weight, input_norm, BufferUsage::Activations)
            .map_err(LlamaError::format)?;
        v_states = apply_optional_proj_scale(
            ctx,
            tensor_ids,
            v_states,
            &block.v_proj_scale_name,
            &format!("{prefix}.v_states_scaled"),
        )?;
        v_states = ctx
            .reshape(
                v_states,
                &[
                    i64::from(block.v_head_dim),
                    i64::from(block.kv_head_count),
                    n_tokens_i64,
                ],
            )
            .map_err(LlamaError::format)?;
        ctx.set_tensor_name(v_states, format!("{prefix}.v_states"))
            .map_err(LlamaError::format)?;
        v_states
    } else {
        k_states
    };

    if let Some(k_norm_name) = &block.k_norm_name {
        k_states = build_rms_norm_mul(
            ctx,
            tensor_ids,
            k_states,
            block.rms_epsilon,
            k_norm_name,
            &format!("{prefix}.k_norm"),
        )?;
    }
    if let Some(v_norm_epsilon) = block.v_norm_epsilon {
        v_states = build_rms_norm(ctx, v_states, v_norm_epsilon, &format!("{prefix}.v_norm"))?;
    }
    let k_norm_store = ctx
        .view_2d(
            k_states,
            i64::from(block.k_head_dim) * i64::from(block.kv_head_count),
            n_tokens_i64,
            ctx.tensor(k_states)
                .ok_or_else(|| LlamaError::format("invalid k_states tensor"))?
                .nb[2],
            0,
        )
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(k_norm_store, format!("{prefix}.k_norm_store"))
        .map_err(LlamaError::format)?;

    // llama-graph.cpp:22-35 / 2056-2057: one attn_inp_kq_mask is shared
    // across layers. A hybrid decode passes that tensor in; standalone
    // decode graphs still allocate a private mask.
    let input_mask = if let Some(shared_input_mask) = shared_input_mask {
        if !block.causal {
            return Err(LlamaError::format(format!(
                "{prefix} received a shared kq mask but the block is not causal"
            )));
        }
        Some(shared_input_mask)
    } else if block.causal {
        let mask = ctx
            .new_named_tensor(
                format!("{prefix}.kq_mask"),
                attention_mask_tensor_type(block.q_head_dim, n_tokens),
                4,
                &[
                    i64::try_from(attention_key_count).map_err(|_| {
                        LlamaError::format("attention decode key_count does not fit in i64")
                    })?,
                    n_tokens_i64,
                    1,
                    1,
                ],
                BufferUsage::Activations,
            )
            .map_err(LlamaError::format)?;
        mark_input(ctx, mask)?;
        Some(mask)
    } else {
        None
    };

    if let Some(rope) = &block.rope {
        let rope_positions = input_rope_positions.ok_or_else(|| {
            LlamaError::format("attention decode rope requested without a rope position tensor")
        })?;
        let rope_factors = block
            .rope_factors_name
            .as_ref()
            .map(|name| require_tensor_id(tensor_ids, name))
            .transpose()?;
        q_states = ctx
            .rope_multi(
                q_states,
                rope_positions,
                rope_factors,
                rope.n_dims,
                rope.sections,
                rope.mode,
                rope.n_ctx_orig,
                rope.freq_base,
                rope.freq_scale,
                rope.ext_factor,
                rope.attn_factor,
                rope.beta_fast,
                rope.beta_slow,
                BufferUsage::Activations,
            )
            .map_err(LlamaError::format)?;
        k_states = ctx
            .rope_multi(
                k_states,
                rope_positions,
                rope_factors,
                rope.n_dims,
                rope.sections,
                rope.mode,
                rope.n_ctx_orig,
                rope.freq_base,
                rope.freq_scale,
                rope.ext_factor,
                rope.attn_factor,
                rope.beta_fast,
                rope.beta_slow,
                BufferUsage::Activations,
            )
            .map_err(LlamaError::format)?;
    }

    let q_store = ctx
        .view_2d(
            q_states,
            i64::from(block.q_head_dim) * i64::from(block.q_head_count),
            n_tokens_i64,
            ctx.tensor(q_states)
                .ok_or_else(|| LlamaError::format("invalid q_states tensor"))?
                .nb[2],
            0,
        )
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(q_store, format!("{prefix}.q_store"))
        .map_err(LlamaError::format)?;

    let k_merged_width = i64::from(block.k_head_dim) * i64::from(block.kv_head_count);
    let v_merged_width = i64::from(block.v_head_dim) * i64::from(block.kv_head_count);
    let k_token_stride = ctx
        .tensor(k_states)
        .ok_or_else(|| LlamaError::format("invalid k_states tensor"))?
        .nb[2];
    let v_token_stride = ctx
        .tensor(v_states)
        .ok_or_else(|| LlamaError::format("invalid v_states tensor"))?
        .nb[2];

    let k_store = ctx
        .view_2d(k_states, k_merged_width, n_tokens_i64, k_token_stride, 0)
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(k_store, format!("{prefix}.k_store"))
        .map_err(LlamaError::format)?;
    let v_store = ctx
        .view_2d(v_states, v_merged_width, n_tokens_i64, v_token_stride, 0)
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(v_store, format!("{prefix}.v_store"))
        .map_err(LlamaError::format)?;

    let (k_cache, v_cache) = if let Some(shared_cache) = shared_cache {
        (shared_cache.k_cache, shared_cache.v_cache)
    } else {
        let k_cache = ctx
            .new_named_tensor(
                format!("{prefix}.k_cache"),
                spec.cache.k_type,
                3,
                &[
                    k_merged_width,
                    i64::from(spec.cache.max_context),
                    i64::from(spec.cache.max_sequences),
                ],
                BufferUsage::State,
            )
            .map_err(LlamaError::format)?;
        let v_cache = ctx
            .new_named_tensor(
                format!("{prefix}.v_cache"),
                spec.cache.v_type,
                3,
                &[
                    v_merged_width,
                    i64::from(spec.cache.max_context),
                    i64::from(spec.cache.max_sequences),
                ],
                BufferUsage::State,
            )
            .map_err(LlamaError::format)?;
        (k_cache, v_cache)
    };

    let (k_cache_written, v_cache_written) = if spec.write_kv {
        (
            ctx.set_rows(k_cache, k_store, input_write_indices, BufferUsage::State)
                .map_err(LlamaError::format)?,
            ctx.set_rows(v_cache, v_store, input_write_indices, BufferUsage::State)
                .map_err(LlamaError::format)?,
        )
    } else {
        (k_cache, v_cache)
    };

    let k_cache_view = ctx
        .view_4d(
            k_cache_written,
            i64::from(block.k_head_dim),
            i64::from(block.kv_head_count),
            i64::try_from(attention_key_count)
                .map_err(|_| LlamaError::format("attention key_count does not fit in i64"))?,
            i64::from(spec.cache.max_sequences),
            row_size(spec.cache.k_type, i64::from(block.k_head_dim))?,
            row_size(spec.cache.k_type, k_merged_width)?,
            row_size(
                spec.cache.k_type,
                k_merged_width * i64::from(spec.cache.max_context),
            )?,
            0,
        )
        .map_err(LlamaError::format)?;
    let v_cache_view = ctx
        .view_4d(
            v_cache_written,
            i64::from(block.v_head_dim),
            i64::from(block.kv_head_count),
            i64::try_from(attention_key_count)
                .map_err(|_| LlamaError::format("attention key_count does not fit in i64"))?,
            i64::from(spec.cache.max_sequences),
            row_size(spec.cache.v_type, i64::from(block.v_head_dim))?,
            row_size(spec.cache.v_type, v_merged_width)?,
            row_size(
                spec.cache.v_type,
                v_merged_width * i64::from(spec.cache.max_context),
            )?,
            0,
        )
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(k_cache_view, format!("{prefix}.k_cache_view"))
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(v_cache_view, format!("{prefix}.v_cache_view"))
        .map_err(LlamaError::format)?;

    let allow_flash_attention =
        !(shared_cache.is_some() && !spec.write_kv && block.causal_window.is_none());
    let mut attn = build_attention_mha_output(
        ctx,
        q_states,
        k_cache_view,
        v_cache_view,
        input_mask,
        block.q_head_dim,
        block.attention_scale,
        n_tokens,
        allow_flash_attention,
        prefix,
    )?;

    if let Some(gate) = gate {
        attn = ctx
            .binary_like_a(Op::Mul, attn, gate, BufferUsage::Activations)
            .map_err(LlamaError::format)?;
        ctx.set_tensor_name(attn, format!("{prefix}.attn_gated"))
            .map_err(LlamaError::format)?;
    }

    let output_weight = require_tensor_id(tensor_ids, &block.output_proj_name)?;
    let mut result_output = ctx
        .mul_mat(output_weight, attn, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    result_output = apply_optional_proj_scale(
        ctx,
        tensor_ids,
        result_output,
        &block.output_proj_scale_name,
        &format!("{prefix}.output_proj_scaled"),
    )?;
    ctx.set_tensor_name(result_output, format!("{prefix}.output_proj"))
        .map_err(LlamaError::format)?;

    if block.residual {
        result_output = ctx
            .binary_like_a(
                Op::Add,
                result_output,
                input_embed,
                BufferUsage::Activations,
            )
            .map_err(LlamaError::format)?;
        ctx.set_tensor_name(result_output, format!("{prefix}.output_residual"))
            .map_err(LlamaError::format)?;
    }

    Ok(BuiltAttentionDecode {
        input_mask,
        k_cache,
        v_cache,
        k_cache_written,
        v_cache_written,
        k_cache_view,
        v_cache_view,
        result_output,
    })
}

pub fn build_attention_decode_graph(
    ctx: &mut Context,
    tensor_ids: &BTreeMap<String, TensorId>,
    spec: &AttentionDecodeSpec,
    n_tokens: usize,
) -> Result<AttentionDecodeGraph> {
    let attention_key_count = usize::try_from(spec.cache.max_context).map_err(|_| {
        LlamaError::format(format!(
            "attention decode max_context {} does not fit in usize",
            spec.cache.max_context
        ))
    })?;
    build_attention_decode_graph_with_key_count(
        ctx,
        tensor_ids,
        spec,
        n_tokens,
        attention_key_count,
    )
}

pub fn build_attention_decode_graph_with_key_count(
    ctx: &mut Context,
    tensor_ids: &BTreeMap<String, TensorId>,
    spec: &AttentionDecodeSpec,
    n_tokens: usize,
    attention_key_count: usize,
) -> Result<AttentionDecodeGraph> {
    if n_tokens == 0 {
        return Err(LlamaError::format(
            "attention decode graph requires at least one token",
        ));
    }

    let block = &spec.block;
    let input_primary = match &block.input {
        ProbeInputKind::TokenIds { .. } => ctx
            .new_named_tensor(
                "attn_decode.inp_tokens",
                TensorType::I32,
                1,
                &[n_tokens as i64],
                BufferUsage::Activations,
            )
            .map_err(LlamaError::format)?,
        ProbeInputKind::Embeddings {
            hidden_size,
            input_type,
        } => ctx
            .new_named_tensor(
                "attn_decode.inp_embd",
                *input_type,
                2,
                &[i64::from(*hidden_size), n_tokens as i64],
                BufferUsage::Activations,
            )
            .map_err(LlamaError::format)?,
    };
    mark_input(ctx, input_primary)?;

    let input_positions = ctx
        .new_named_tensor(
            "attn_decode.inp_pos",
            TensorType::I32,
            1,
            &[n_tokens as i64],
            BufferUsage::Activations,
        )
        .map_err(LlamaError::format)?;
    mark_input(ctx, input_positions)?;
    let input_write_indices = ctx
        .new_named_tensor(
            "attn_decode.inp_k_idxs",
            TensorType::I32,
            1,
            &[n_tokens as i64],
            BufferUsage::Activations,
        )
        .map_err(LlamaError::format)?;
    mark_input(ctx, input_write_indices)?;
    let input_rope_positions = if let Some(rope) = &block.rope {
        let positions = ctx
            .new_named_tensor(
                "attn_decode.inp_rope_pos",
                TensorType::I32,
                1,
                &[rope_position_tensor_len(rope, n_tokens)?],
                BufferUsage::Activations,
            )
            .map_err(LlamaError::format)?;
        mark_input(ctx, positions)?;
        Some(positions)
    } else {
        None
    };

    let input_embed = match &block.input {
        ProbeInputKind::TokenIds {
            token_embedding_name,
            token_embedding_scale,
        } => {
            let token_embd = require_tensor_id(tensor_ids, token_embedding_name)?;
            let input_embed = ctx
                .get_rows(token_embd, input_primary, BufferUsage::Activations)
                .map_err(LlamaError::format)?;
            apply_optional_input_scale(
                ctx,
                input_embed,
                *token_embedding_scale,
                "attn_decode.input_embed_scaled",
            )?
        }
        ProbeInputKind::Embeddings { .. } => input_primary,
    };
    ctx.set_tensor_name(input_embed, "attn_decode.input_embed")
        .map_err(LlamaError::format)?;
    let built = build_attention_decode_from_hidden(
        ctx,
        tensor_ids,
        spec,
        None,
        input_embed,
        input_positions,
        input_write_indices,
        input_rope_positions,
        n_tokens,
        attention_key_count,
        None,
        "attn_decode",
    )?;
    let result_output = built.result_output;
    mark_output(ctx, result_output)?;

    let mut graph = Graph::new();
    graph
        .build_forward_expand(ctx, result_output)
        .map_err(LlamaError::format)?;

    Ok(AttentionDecodeGraph {
        graph,
        input_primary,
        input_positions,
        input_write_indices,
        input_rope_positions,
        input_mask: built.input_mask,
        k_cache: built.k_cache,
        v_cache: built.v_cache,
        k_cache_view: built.k_cache_view,
        v_cache_view: built.v_cache_view,
        graph_key_count: attention_key_count,
        result_output,
    })
}

pub fn prepare_attention_decode_graph(
    ctx: &mut Context,
    tensor_ids: &BTreeMap<String, TensorId>,
    spec: &AttentionDecodeSpec,
    n_tokens: usize,
    features: MetalDeviceFeatures,
) -> Result<(AttentionDecodeGraph, MetalPreparedGraph)> {
    let decode = build_attention_decode_graph(ctx, tensor_ids, spec, n_tokens)?;
    let prepared = prepare_graph(ctx, &decode.graph, features).map_err(LlamaError::format)?;
    Ok((decode, prepared))
}

pub fn prepare_attention_decode_graph_with_key_count(
    ctx: &mut Context,
    tensor_ids: &BTreeMap<String, TensorId>,
    spec: &AttentionDecodeSpec,
    n_tokens: usize,
    attention_key_count: usize,
    features: MetalDeviceFeatures,
) -> Result<(AttentionDecodeGraph, MetalPreparedGraph)> {
    let decode = build_attention_decode_graph_with_key_count(
        ctx,
        tensor_ids,
        spec,
        n_tokens,
        attention_key_count,
    )?;
    let prepared = prepare_graph(ctx, &decode.graph, features).map_err(LlamaError::format)?;
    Ok((decode, prepared))
}

pub fn compile_attention_decode_metal(
    weights: &mut LoadedGgufWeights,
    spec: &AttentionDecodeSpec,
    n_tokens: usize,
) -> Result<CompiledAttentionDecodeMetal> {
    let attention_key_count = usize::try_from(spec.cache.max_context).map_err(|_| {
        LlamaError::format(format!(
            "attention decode max_context {} does not fit in usize",
            spec.cache.max_context
        ))
    })?;
    compile_attention_decode_metal_with_key_count(weights, spec, n_tokens, attention_key_count)
}

pub fn compile_attention_decode_metal_with_key_count(
    weights: &mut LoadedGgufWeights,
    spec: &AttentionDecodeSpec,
    n_tokens: usize,
    attention_key_count: usize,
) -> Result<CompiledAttentionDecodeMetal> {
    let runtime = MetalRuntime::new().map_err(LlamaError::unsupported)?;
    let (decode, prepared) = prepare_attention_decode_graph_with_key_count(
        &mut weights.ctx,
        &weights.tensor_ids,
        spec,
        n_tokens,
        attention_key_count,
        runtime.features(),
    )?;
    let session = MetalGraphSession::from_runtime(
        runtime,
        &weights.ctx,
        &prepared,
        BufferStorageMode::Private,
        BufferStorageMode::Private,
    )
    .map_err(LlamaError::format)?;

    Ok(CompiledAttentionDecodeMetal {
        spec: spec.clone(),
        decode,
        session,
    })
}

enum AttentionPreparedInput<'a> {
    None,
    Bytes(&'a [u8]),
    Buffer {
        source_buffer: &'a MetalBuffer,
        source_offset_bytes: usize,
    },
}

fn execute_prepared_attention_decode_metal_no_readback_inner(
    runtime: &MetalRuntime,
    ctx: &mut Context,
    spec: &AttentionDecodeSpec,
    decode: &AttentionDecodeGraph,
    compiled: &MetalCompiledGraph,
    input_primary: AttentionPreparedInput<'_>,
    positions: &[i32],
    cache_tokens: usize,
    active_batch: bool,
) -> Result<()> {
    if positions.is_empty() {
        return Err(LlamaError::format(
            "attention decode requires at least one position",
        ));
    }
    let max_context = usize::try_from(spec.cache.max_context).map_err(|_| {
        LlamaError::format(format!(
            "max_context {} does not fit in usize",
            spec.cache.max_context
        ))
    })?;
    if cache_tokens == 0 || cache_tokens > max_context {
        return Err(LlamaError::format(format!(
            "attention decode cache_tokens {} is outside 1..={}",
            cache_tokens, max_context
        )));
    }
    if positions.iter().copied().any(|pos| {
        pos < 0
            || usize::try_from(pos)
                .ok()
                .map(|pos| pos >= cache_tokens)
                .unwrap_or(true)
    }) {
        return Err(LlamaError::format(format!(
            "attention decode positions {:?} exceed cache_tokens {}",
            positions, cache_tokens
        )));
    }

    // The graph's key width only needs to COVER the cache: masks are written
    // at full graph width, so keys beyond cache_tokens are -inf'd exactly.
    // (Views are never reconfigured — see attention_mask_write_key_count.)
    if decode.graph_key_count < cache_tokens {
        return Err(LlamaError::format(format!(
            "attention decode graph key_count {} is smaller than cache_tokens {}",
            decode.graph_key_count, cache_tokens
        )));
    }

    let rope_positions = spec
        .block
        .rope
        .as_ref()
        .map(|rope| encode_rope_positions(rope, positions, positions.len()))
        .transpose()?;
    let mut writes = Vec::with_capacity(4);
    let mut buffer_inputs = Vec::with_capacity(1);
    match input_primary {
        AttentionPreparedInput::None => {}
        AttentionPreparedInput::Bytes(input_primary) => {
            writes.push(MetalGraphTensorWrite {
                tensor_id: decode.input_primary,
                bytes: input_primary,
            });
        }
        AttentionPreparedInput::Buffer {
            source_buffer,
            source_offset_bytes,
        } => {
            buffer_inputs.push(MetalGraphTensorBufferCopy {
                tensor_id: decode.input_primary,
                source_buffer,
                source_offset_bytes,
            });
        }
    }
    writes.push(MetalGraphTensorWrite {
        tensor_id: decode.input_write_indices,
        bytes: i32_slice_as_bytes(positions),
    });
    if let Some(input_rope_positions) = decode.input_rope_positions {
        writes.push(MetalGraphTensorWrite {
            tensor_id: input_rope_positions,
            bytes: i32_slice_as_bytes(rope_positions.as_deref().ok_or_else(|| {
                LlamaError::format("attention decode rope positions were not prepared")
            })?),
        });
    }
    let mask_bytes = decode
        .input_mask
        .map(|input_mask| {
            let key_count = attention_mask_write_key_count(ctx, input_mask)?;
            let bytes = position_attention_mask_bytes_for_tensor(
                ctx,
                input_mask,
                key_count,
                positions,
                None,
                spec.block
                    .causal_window
                    .map(|window| usize::try_from(window).unwrap_or(usize::MAX)),
            )?;
            Ok::<Vec<u8>, LlamaError>(bytes)
        })
        .transpose()?;
    if let Some(input_mask) = decode.input_mask {
        writes.push(MetalGraphTensorWrite {
            tensor_id: input_mask,
            bytes: mask_bytes.as_deref().ok_or_else(|| {
                LlamaError::format("attention decode causal mask was not prepared")
            })?,
        });
    }

    if active_batch {
        execute_compiled_graph_in_active_batch(runtime, ctx, compiled, &writes, &buffer_inputs)
            .map_err(LlamaError::format)?;
    } else {
        execute_compiled_graph_with_buffer_inputs(
            runtime,
            ctx,
            compiled,
            &writes,
            &buffer_inputs,
            &[],
        )
        .map_err(LlamaError::format)?;
    }
    Ok(())
}

pub fn execute_prepared_attention_decode_metal_no_readback_prepared_input(
    runtime: &MetalRuntime,
    ctx: &mut Context,
    spec: &AttentionDecodeSpec,
    decode: &AttentionDecodeGraph,
    compiled: &MetalCompiledGraph,
    positions: &[i32],
    cache_tokens: usize,
) -> Result<()> {
    execute_prepared_attention_decode_metal_no_readback_inner(
        runtime,
        ctx,
        spec,
        decode,
        compiled,
        AttentionPreparedInput::None,
        positions,
        cache_tokens,
        false,
    )
}

pub fn execute_prepared_attention_decode_metal_no_readback_prepared_input_in_active_batch(
    runtime: &MetalRuntime,
    ctx: &mut Context,
    spec: &AttentionDecodeSpec,
    decode: &AttentionDecodeGraph,
    compiled: &MetalCompiledGraph,
    positions: &[i32],
    cache_tokens: usize,
) -> Result<()> {
    execute_prepared_attention_decode_metal_no_readback_inner(
        runtime,
        ctx,
        spec,
        decode,
        compiled,
        AttentionPreparedInput::None,
        positions,
        cache_tokens,
        true,
    )
}

pub fn execute_prepared_attention_decode_metal_no_readback_buffer_input(
    runtime: &MetalRuntime,
    ctx: &mut Context,
    spec: &AttentionDecodeSpec,
    decode: &AttentionDecodeGraph,
    compiled: &MetalCompiledGraph,
    source_buffer: &MetalBuffer,
    source_offset_bytes: usize,
    positions: &[i32],
    cache_tokens: usize,
) -> Result<()> {
    execute_prepared_attention_decode_metal_no_readback_inner(
        runtime,
        ctx,
        spec,
        decode,
        compiled,
        AttentionPreparedInput::Buffer {
            source_buffer,
            source_offset_bytes,
        },
        positions,
        cache_tokens,
        false,
    )
}

pub fn execute_prepared_attention_decode_metal_no_readback(
    runtime: &MetalRuntime,
    ctx: &mut Context,
    spec: &AttentionDecodeSpec,
    decode: &AttentionDecodeGraph,
    compiled: &MetalCompiledGraph,
    input: LogitsProbeInput<'_>,
    positions: &[i32],
    cache_tokens: usize,
) -> Result<()> {
    let input_primary = match (&spec.block.input, input) {
        (ProbeInputKind::TokenIds { .. }, LogitsProbeInput::TokenIds(token_ids)) => {
            if token_ids.len() != positions.len() {
                return Err(LlamaError::format(format!(
                    "attention decode token/position length mismatch: {} vs {}",
                    token_ids.len(),
                    positions.len()
                )));
            }
            i32_slice_as_bytes(token_ids).to_vec()
        }
        (
            ProbeInputKind::Embeddings {
                hidden_size,
                input_type,
            },
            LogitsProbeInput::EmbeddingsF32 { data, n_tokens },
        ) => {
            if *input_type != TensorType::F32 {
                return Err(LlamaError::unsupported(format!(
                    "attention decode currently expects F32 embeddings, got {}",
                    input_type.name()
                )));
            }
            if n_tokens != positions.len() {
                return Err(LlamaError::format(format!(
                    "attention decode embedding/position length mismatch: {} vs {}",
                    n_tokens,
                    positions.len()
                )));
            }
            let expected = (*hidden_size as usize)
                .checked_mul(n_tokens)
                .ok_or_else(|| {
                    LlamaError::format("overflow computing attention decode embedding size")
                })?;
            if data.len() != expected {
                return Err(LlamaError::format(format!(
                    "attention decode embedding input length mismatch: got {}, expected {}",
                    data.len(),
                    expected
                )));
            }
            f32_slice_as_bytes(data).to_vec()
        }
        (ProbeInputKind::TokenIds { .. }, LogitsProbeInput::EmbeddingsF32 { .. }) => {
            return Err(LlamaError::format(
                "attention decode spec expects token ids but embeddings were provided",
            ));
        }
        (ProbeInputKind::Embeddings { .. }, LogitsProbeInput::TokenIds(_)) => {
            return Err(LlamaError::format(
                "attention decode spec expects embeddings but token ids were provided",
            ));
        }
    };
    execute_prepared_attention_decode_metal_no_readback_inner(
        runtime,
        ctx,
        spec,
        decode,
        compiled,
        AttentionPreparedInput::Bytes(&input_primary),
        positions,
        cache_tokens,
        false,
    )
}

pub fn execute_prepared_attention_decode_metal(
    runtime: &MetalRuntime,
    ctx: &mut Context,
    spec: &AttentionDecodeSpec,
    decode: &AttentionDecodeGraph,
    compiled: &MetalCompiledGraph,
    input: LogitsProbeInput<'_>,
    positions: &[i32],
    cache_tokens: usize,
) -> Result<AttentionBlockRun> {
    if positions.is_empty() {
        return Err(LlamaError::format(
            "attention decode requires at least one position",
        ));
    }
    let max_context = usize::try_from(spec.cache.max_context).map_err(|_| {
        LlamaError::format(format!(
            "max_context {} does not fit in usize",
            spec.cache.max_context
        ))
    })?;
    if cache_tokens == 0 || cache_tokens > max_context {
        return Err(LlamaError::format(format!(
            "attention decode cache_tokens {} is outside 1..={}",
            cache_tokens, max_context
        )));
    }
    if positions.iter().copied().any(|pos| {
        pos < 0
            || usize::try_from(pos)
                .ok()
                .map(|pos| pos >= cache_tokens)
                .unwrap_or(true)
    }) {
        return Err(LlamaError::format(format!(
            "attention decode positions {:?} exceed cache_tokens {}",
            positions, cache_tokens
        )));
    }

    let input_primary = match (&spec.block.input, input) {
        (ProbeInputKind::TokenIds { .. }, LogitsProbeInput::TokenIds(token_ids)) => {
            if token_ids.len() != positions.len() {
                return Err(LlamaError::format(format!(
                    "attention decode token/position length mismatch: {} vs {}",
                    token_ids.len(),
                    positions.len()
                )));
            }
            i32_slice_as_bytes(token_ids).to_vec()
        }
        (
            ProbeInputKind::Embeddings {
                hidden_size,
                input_type,
            },
            LogitsProbeInput::EmbeddingsF32 { data, n_tokens },
        ) => {
            if *input_type != TensorType::F32 {
                return Err(LlamaError::unsupported(format!(
                    "attention decode currently expects F32 embeddings, got {}",
                    input_type.name()
                )));
            }
            if n_tokens != positions.len() {
                return Err(LlamaError::format(format!(
                    "attention decode embedding/position length mismatch: {} vs {}",
                    n_tokens,
                    positions.len()
                )));
            }
            let expected = (*hidden_size as usize)
                .checked_mul(n_tokens)
                .ok_or_else(|| {
                    LlamaError::format("overflow computing attention decode embedding size")
                })?;
            if data.len() != expected {
                return Err(LlamaError::format(format!(
                    "attention decode embedding input length mismatch: got {}, expected {}",
                    data.len(),
                    expected
                )));
            }
            f32_slice_as_bytes(data).to_vec()
        }
        (ProbeInputKind::TokenIds { .. }, LogitsProbeInput::EmbeddingsF32 { .. }) => {
            return Err(LlamaError::format(
                "attention decode spec expects token ids but embeddings were provided",
            ));
        }
        (ProbeInputKind::Embeddings { .. }, LogitsProbeInput::TokenIds(_)) => {
            return Err(LlamaError::format(
                "attention decode spec expects embeddings but token ids were provided",
            ));
        }
    };

    // The graph's key width only needs to COVER the cache: masks are written
    // at full graph width, so keys beyond cache_tokens are -inf'd exactly.
    // (Views are never reconfigured — see attention_mask_write_key_count.)
    if decode.graph_key_count < cache_tokens {
        return Err(LlamaError::format(format!(
            "attention decode graph key_count {} is smaller than cache_tokens {}",
            decode.graph_key_count, cache_tokens
        )));
    }

    let rope_positions = spec
        .block
        .rope
        .as_ref()
        .map(|rope| encode_rope_positions(rope, positions, positions.len()))
        .transpose()?;
    let mut writes = vec![
        MetalGraphTensorWrite {
            tensor_id: decode.input_primary,
            bytes: &input_primary,
        },
        MetalGraphTensorWrite {
            tensor_id: decode.input_write_indices,
            bytes: i32_slice_as_bytes(positions),
        },
    ];
    if let Some(input_rope_positions) = decode.input_rope_positions {
        writes.push(MetalGraphTensorWrite {
            tensor_id: input_rope_positions,
            bytes: i32_slice_as_bytes(rope_positions.as_deref().ok_or_else(|| {
                LlamaError::format("attention decode rope positions were not prepared")
            })?),
        });
    }
    let mask_bytes = decode
        .input_mask
        .map(|input_mask| {
            let key_count = attention_mask_write_key_count(ctx, input_mask)?;
            let bytes = position_attention_mask_bytes_for_tensor(
                ctx,
                input_mask,
                key_count,
                positions,
                None,
                spec.block
                    .causal_window
                    .map(|window| usize::try_from(window).unwrap_or(usize::MAX)),
            )?;
            Ok::<Vec<u8>, LlamaError>(bytes)
        })
        .transpose()?;
    if let Some(input_mask) = decode.input_mask {
        writes.push(MetalGraphTensorWrite {
            tensor_id: input_mask,
            bytes: mask_bytes.as_deref().ok_or_else(|| {
                LlamaError::format("attention decode causal mask was not prepared")
            })?,
        });
    }

    let execution =
        execute_compiled_graph(runtime, ctx, compiled, &writes, &[decode.result_output])
            .map_err(LlamaError::format)?;

    let result_bytes = execution
        .outputs
        .get(&decode.result_output)
        .ok_or_else(|| {
            LlamaError::format("compiled attention decode did not produce result bytes")
        })?;
    let hidden = f32_bytes_to_vec(result_bytes)?;
    let output = ctx
        .tensor(decode.result_output)
        .ok_or_else(|| LlamaError::format("attention decode result tensor is invalid"))?;

    Ok(AttentionBlockRun {
        hidden,
        hidden_size: ne_usize(output, 0)?,
        n_tokens: ne_usize(output, 1)?,
    })
}

pub fn execute_attention_decode_graph_metal(
    weights: &mut LoadedGgufWeights,
    spec: &AttentionDecodeSpec,
    n_tokens: usize,
    input: LogitsProbeInput<'_>,
    positions: &[i32],
    cache_tokens: usize,
) -> Result<AttentionBlockRun> {
    let compiled = compile_attention_decode_metal(weights, spec, n_tokens)?;
    compiled.execute(&mut weights.ctx, input, positions, cache_tokens)
}

pub fn execute_attention_decode_graph_metal_cached(
    compiled: &CompiledAttentionDecodeMetal,
    weights: &mut LoadedGgufWeights,
    input: LogitsProbeInput<'_>,
    positions: &[i32],
    cache_tokens: usize,
) -> Result<AttentionBlockRun> {
    compiled.execute(&mut weights.ctx, input, positions, cache_tokens)
}

#[derive(Clone, Debug)]
struct BuiltDeltaNetRecurrentDecode {
    r_cache: TensorId,
    s_cache: TensorId,
    /// One entry per written state row: a single row normally, one per batch
    /// token when speculative checkpointing is on.
    r_cache_updates: Vec<TensorId>,
    s_cache_updates: Vec<TensorId>,
    result_output: TensorId,
}

/// The state-cache rows every checkpoint of this batch writes: elements
/// `1..=n_tokens` of the `[resume_row, w_0..w_n]` vector.
fn state_write_rows_all(
    ctx: &mut Context,
    input_state_rows: TensorId,
    recurrent_checkpoints: bool,
) -> Result<TensorId> {
    if !recurrent_checkpoints {
        return Ok(input_state_rows);
    }
    let rows = require_tensor(ctx, input_state_rows)?.ne[0];
    ctx.view_1d(input_state_rows, rows - 1, row_size(TensorType::I32, 1)?)
        .map_err(LlamaError::format)
}


fn build_delta_net_chunking(
    ctx: &mut Context,
    _block: &DeltaNetRecurrentBlockSpec,
    q_conv: TensorId,
    k_conv: TensorId,
    v_conv: TensorId,
    gate: TensorId,
    beta: TensorId,
    state: TensorId,
    n_seq_tokens: i64,
    n_seqs: i64,
    prefix: &str,
) -> Result<(TensorId, TensorId)> {
    let checked_mul = |lhs: i64, rhs: i64, what: &str| {
        lhs.checked_mul(rhs)
            .ok_or_else(|| LlamaError::format(format!("overflow computing {what}")))
    };

    let q_tensor = require_tensor(ctx, q_conv)?.clone();
    let k_tensor = require_tensor(ctx, k_conv)?.clone();
    let v_tensor = require_tensor(ctx, v_conv)?.clone();
    let s_k = q_tensor.ne[0];
    let h_k = q_tensor.ne[1];
    let s_v = v_tensor.ne[0];
    let h_v = v_tensor.ne[1];
    if k_tensor.ne[0] != s_k || k_tensor.ne[1] != h_k {
        return Err(LlamaError::format(format!(
            "delta-net chunking expects q/k head shapes to match, got q=[{}, {}] k=[{}, {}]",
            s_k, h_k, k_tensor.ne[0], k_tensor.ne[1]
        )));
    }
    if s_k != s_v {
        return Err(LlamaError::format(format!(
            "delta-net chunking requires matching key/value head dims, got key={} value={}",
            s_k, s_v
        )));
    }
    if h_v % h_k != 0 {
        return Err(LlamaError::format(format!(
            "delta-net chunking requires value head count {} to be divisible by key head count {}",
            h_v, h_k
        )));
    }

    let q_scaled = ctx
        .scale(
            q_conv,
            1.0f32 / (s_k as f32).sqrt(),
            BufferUsage::Activations,
        )
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(q_scaled, format!("{prefix}.q_in"))
        .map_err(LlamaError::format)?;

    let mut q = ctx
        .permute(q_scaled, [0, 2, 1, 3])
        .map_err(LlamaError::format)?;
    let mut k = ctx
        .permute(k_conv, [0, 2, 1, 3])
        .map_err(LlamaError::format)?;
    let mut v = ctx
        .permute(v_conv, [0, 2, 1, 3])
        .map_err(LlamaError::format)?;
    let mut g = ctx
        .permute(gate, [0, 2, 1, 3])
        .map_err(LlamaError::format)?;
    let mut b = ctx
        .permute(beta, [0, 2, 1, 3])
        .map_err(LlamaError::format)?;

    ctx.set_tensor_name(k, format!("{prefix}.k_in"))
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(v, format!("{prefix}.v_in"))
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(b, format!("{prefix}.b_in"))
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(g, format!("{prefix}.g_in"))
        .map_err(LlamaError::format)?;

    let g_tensor = require_tensor(ctx, g)?.clone();
    let kda = g_tensor.ne[0] == s_k && g_tensor.ne[1] == h_k;
    let chunk_size = if kda { 16 } else { 64 };
    let pad = (chunk_size - (n_seq_tokens % chunk_size)) % chunk_size;
    let n_chunks = (n_seq_tokens + pad) / chunk_size;

    q = ctx
        .pad_4d(q, 0, pad, 0, 0, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    k = ctx
        .pad_4d(k, 0, pad, 0, 0, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    v = ctx
        .pad_4d(v, 0, pad, 0, 0, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    g = ctx
        .pad_4d(g, 0, pad, 0, 0, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    b = ctx
        .pad_4d(b, 0, pad, 0, 0, BufferUsage::Activations)
        .map_err(LlamaError::format)?;

    let mut v_b = ctx
        .binary_like_a(Op::Mul, v, b, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    let mut k_b = ctx
        .binary_like_a(Op::Mul, k, b, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(v_b, format!("{prefix}.v_b"))
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(k_b, format!("{prefix}.k_b"))
        .map_err(LlamaError::format)?;

    q = ctx
        .reshape(
            q,
            &[
                s_k,
                chunk_size,
                n_chunks,
                checked_mul(h_k, n_seqs, "q chunks")?,
            ],
        )
        .map_err(LlamaError::format)?;
    k = ctx
        .reshape(
            k,
            &[
                s_k,
                chunk_size,
                n_chunks,
                checked_mul(h_k, n_seqs, "k chunks")?,
            ],
        )
        .map_err(LlamaError::format)?;
    k_b = ctx
        .reshape(
            k_b,
            &[
                s_k,
                chunk_size,
                n_chunks,
                checked_mul(h_v, n_seqs, "k_beta chunks")?,
            ],
        )
        .map_err(LlamaError::format)?;
    let _v_chunked = ctx
        .reshape(
            v,
            &[
                s_v,
                chunk_size,
                n_chunks,
                checked_mul(h_v, n_seqs, "v chunks")?,
            ],
        )
        .map_err(LlamaError::format)?;
    v_b = ctx
        .reshape(
            v_b,
            &[
                s_v,
                chunk_size,
                n_chunks,
                checked_mul(h_v, n_seqs, "v_beta chunks")?,
            ],
        )
        .map_err(LlamaError::format)?;

    let g0 = require_tensor(ctx, g)?.ne[0];
    g = ctx
        .reshape(
            g,
            &[
                g0,
                chunk_size,
                n_chunks,
                checked_mul(h_v, n_seqs, "gate chunks")?,
            ],
        )
        .map_err(LlamaError::format)?;

    let g_t = ctx.transpose(g).map_err(LlamaError::format)?;
    let g_t = ctx.cont(g_t).map_err(LlamaError::format)?;
    let g_cs = ctx
        .cumsum(g_t, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(g_cs, format!("{prefix}.g_cs"))
        .map_err(LlamaError::format)?;

    let (kb, mut kq) = if kda {
        let chb = checked_mul(
            n_chunks,
            checked_mul(h_k, n_seqs, "kda chunk heads")?,
            "kda chb",
        )?;
        let g_cs_i = ctx
            .reshape(g_cs, &[chunk_size, 1, s_k, chb])
            .map_err(LlamaError::format)?;
        let g_cs_j = ctx
            .reshape(g_cs, &[1, chunk_size, s_k, chb])
            .map_err(LlamaError::format)?;
        let g_cs_j = ctx
            .repeat_4d(
                g_cs_j,
                chunk_size,
                chunk_size,
                s_k,
                chb,
                BufferUsage::Activations,
            )
            .map_err(LlamaError::format)?;

        let mut decay_mask = ctx
            .binary_like_a(Op::Sub, g_cs_j, g_cs_i, BufferUsage::Activations)
            .map_err(LlamaError::format)?;
        decay_mask = ctx
            .tri_with_type(decay_mask, TriType::LowerDiag, BufferUsage::Activations)
            .map_err(LlamaError::format)?;
        decay_mask = ctx
            .unary(decay_mask, UnaryOp::Exp, BufferUsage::Activations)
            .map_err(LlamaError::format)?;
        ctx.set_tensor_name(decay_mask, format!("{prefix}.decay_mask"))
            .map_err(LlamaError::format)?;

        let decay_mask = ctx
            .permute(decay_mask, [2, 1, 0, 3])
            .map_err(LlamaError::format)?;
        let decay_mask = ctx
            .cont_4d(decay_mask, s_k, chunk_size, chunk_size, chb)
            .map_err(LlamaError::format)?;

        let k_b_i = ctx
            .reshape(k_b, &[s_k, chunk_size, 1, chb])
            .map_err(LlamaError::format)?;
        let k_j = ctx
            .reshape(k, &[s_k, 1, chunk_size, chb])
            .map_err(LlamaError::format)?;
        let q_i = ctx
            .reshape(q, &[s_k, chunk_size, 1, chb])
            .map_err(LlamaError::format)?;

        let decay_k_b_i = ctx
            .binary_like_a(Op::Mul, decay_mask, k_b_i, BufferUsage::Activations)
            .map_err(LlamaError::format)?;
        let decay_q_i = ctx
            .binary_like_a(Op::Mul, decay_mask, q_i, BufferUsage::Activations)
            .map_err(LlamaError::format)?;

        let mut kb = ctx
            .mul_mat(decay_k_b_i, k_j, BufferUsage::Activations)
            .map_err(LlamaError::format)?;
        let mut kq = ctx
            .mul_mat(decay_q_i, k_j, BufferUsage::Activations)
            .map_err(LlamaError::format)?;

        kb = ctx
            .reshape(
                kb,
                &[
                    chunk_size,
                    chunk_size,
                    n_chunks,
                    checked_mul(h_v, n_seqs, "kda kb heads")?,
                ],
            )
            .map_err(LlamaError::format)?;
        kb = ctx.transpose(kb).map_err(LlamaError::format)?;
        kb = ctx.cont(kb).map_err(LlamaError::format)?;

        kq = ctx
            .reshape(
                kq,
                &[
                    chunk_size,
                    chunk_size,
                    n_chunks,
                    checked_mul(h_v, n_seqs, "kda kq heads")?,
                ],
            )
            .map_err(LlamaError::format)?;
        kq = ctx.transpose(kq).map_err(LlamaError::format)?;
        kq = ctx.cont(kq).map_err(LlamaError::format)?;

        (kb, kq)
    } else {
        let g_cs_i = g_cs;
        let g_cs_j = ctx
            .reshape(
                g_cs,
                &[
                    1,
                    chunk_size,
                    n_chunks,
                    checked_mul(h_v, n_seqs, "g chunk heads")?,
                ],
            )
            .map_err(LlamaError::format)?;
        let g_cs_j = ctx
            .repeat_4d(
                g_cs_j,
                chunk_size,
                chunk_size,
                n_chunks,
                checked_mul(h_v, n_seqs, "g repeated chunk heads")?,
                BufferUsage::Activations,
            )
            .map_err(LlamaError::format)?;

        let mut decay_mask = ctx
            .binary_like_a(Op::Sub, g_cs_j, g_cs_i, BufferUsage::Activations)
            .map_err(LlamaError::format)?;
        decay_mask = ctx
            .tri_with_type(decay_mask, TriType::LowerDiag, BufferUsage::Activations)
            .map_err(LlamaError::format)?;
        decay_mask = ctx
            .unary(decay_mask, UnaryOp::Exp, BufferUsage::Activations)
            .map_err(LlamaError::format)?;
        ctx.set_tensor_name(decay_mask, format!("{prefix}.decay_mask"))
            .map_err(LlamaError::format)?;

        let mut kb = ctx
            .mul_mat(k, k_b, BufferUsage::Activations)
            .map_err(LlamaError::format)?;
        kb = ctx
            .binary_like_a(Op::Mul, kb, decay_mask, BufferUsage::Activations)
            .map_err(LlamaError::format)?;

        let mut kq = ctx
            .mul_mat(k, q, BufferUsage::Activations)
            .map_err(LlamaError::format)?;
        kq = ctx
            .binary_like_a(Op::Mul, kq, decay_mask, BufferUsage::Activations)
            .map_err(LlamaError::format)?;

        (kb, kq)
    };

    kq = ctx
        .tri_with_type(kq, TriType::LowerDiag, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(kq, format!("{prefix}.kq"))
        .map_err(LlamaError::format)?;

    let mut attn = ctx
        .tri_with_type(kb, TriType::Lower, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(attn, format!("{prefix}.attn"))
        .map_err(LlamaError::format)?;

    let identity = ctx
        .view_1d(attn, chunk_size, 0)
        .map_err(LlamaError::format)?;
    let identity = ctx
        .fill(identity, 1.0, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    let identity = ctx
        .diag(identity, BufferUsage::Activations)
        .map_err(LlamaError::format)?;

    let lhs = ctx
        .binary_like_a(Op::Add, attn, identity, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(lhs, format!("{prefix}.dnet_add_ch_lhs"))
        .map_err(LlamaError::format)?;

    attn = ctx
        .unary(attn, UnaryOp::Neg, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(attn, format!("{prefix}.attn_pre_solve"))
        .map_err(LlamaError::format)?;

    let lin_solve = ctx
        .solve_tri(lhs, attn, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    attn = ctx
        .binary_like_a(Op::Add, lin_solve, identity, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(attn, format!("{prefix}.dnet_add_ch_attn_solved"))
        .map_err(LlamaError::format)?;

    let v_b_t = ctx.transpose(v_b).map_err(LlamaError::format)?;
    let v_b_t = ctx.cont(v_b_t).map_err(LlamaError::format)?;
    v = ctx
        .mul_mat(v_b_t, attn, BufferUsage::Activations)
        .map_err(LlamaError::format)?;

    let g_exp = ctx
        .unary(g_cs, UnaryOp::Exp, BufferUsage::Activations)
        .map_err(LlamaError::format)?;

    k_b = ctx.transpose(k_b).map_err(LlamaError::format)?;
    k_b = ctx.cont(k_b).map_err(LlamaError::format)?;
    let kbg = ctx
        .binary_like_a(Op::Mul, k_b, g_exp, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(kbg, format!("{prefix}.k_beta_g_exp"))
        .map_err(LlamaError::format)?;

    let k_cd = ctx
        .mul_mat(kbg, attn, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(k_cd, format!("{prefix}.k_cumdecay"))
        .map_err(LlamaError::format)?;

    let g_exp_t = ctx.transpose(g_exp).map_err(LlamaError::format)?;
    let g_exp_t = ctx.cont(g_exp_t).map_err(LlamaError::format)?;
    let q_g_exp = ctx
        .binary_like_a(Op::Mul, q, g_exp_t, BufferUsage::Activations)
        .map_err(LlamaError::format)?;

    let g_cs_tensor = require_tensor(ctx, g_cs)?.clone();
    let g_last = ctx
        .view_4d(
            g_cs,
            1,
            g_cs_tensor.ne[1],
            g_cs_tensor.ne[2],
            g_cs_tensor.ne[3],
            g_cs_tensor.nb[1],
            g_cs_tensor.nb[2],
            g_cs_tensor.nb[3],
            row_size(g_cs_tensor.desc.ty, g_cs_tensor.ne[0] - 1)?,
        )
        .map_err(LlamaError::format)?;
    let g_last = ctx.cont(g_last).map_err(LlamaError::format)?;
    ctx.set_tensor_name(g_last, format!("{prefix}.g_last"))
        .map_err(LlamaError::format)?;

    let g_last_exp = ctx
        .unary(g_last, UnaryOp::Exp, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    let g_last_exp_t = ctx.transpose(g_last_exp).map_err(LlamaError::format)?;
    ctx.set_tensor_name(g_last_exp_t, format!("{prefix}.g_last_exp_t"))
        .map_err(LlamaError::format)?;

    let g_diff = ctx
        .binary_like_a(Op::Sub, g_cs, g_last, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    let g_diff = ctx
        .unary(g_diff, UnaryOp::Neg, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(g_diff, format!("{prefix}.g_diff"))
        .map_err(LlamaError::format)?;

    let g_diff_exp = ctx
        .unary(g_diff, UnaryOp::Exp, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    let g_diff_exp_t = ctx.transpose(g_diff_exp).map_err(LlamaError::format)?;
    let g_diff_exp_t = ctx.cont(g_diff_exp_t).map_err(LlamaError::format)?;

    let kg = ctx
        .binary_like_a(Op::Mul, k, g_diff_exp_t, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(kg, format!("{prefix}.key_gdiff"))
        .map_err(LlamaError::format)?;

    let kg_t = ctx.transpose(kg).map_err(LlamaError::format)?;
    let kg_t = ctx.cont(kg_t).map_err(LlamaError::format)?;
    ctx.set_tensor_name(kg_t, format!("{prefix}.key_gdiff_t"))
        .map_err(LlamaError::format)?;

    let mut state = ctx
        .reshape(
            state,
            &[s_v, s_v, 1, checked_mul(h_v, n_seqs, "state chunks")?],
        )
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(state, format!("{prefix}.dnet_add_ch_state"))
        .map_err(LlamaError::format)?;

    let v_t = ctx.transpose(v).map_err(LlamaError::format)?;
    let v_t = ctx.cont(v_t).map_err(LlamaError::format)?;

    for chunk in 0..n_chunks {
        let ch_k_cd = view_dim2_slice_2d(ctx, k_cd, chunk)?;
        let ch_v_t = view_dim2_slice_2d(ctx, v_t, chunk)?;
        let ch_kq = view_dim2_slice_2d(ctx, kq, chunk)?;
        let ch_q_g_exp = view_dim2_slice_2d(ctx, q_g_exp, chunk)?;
        let ch_kg_t = view_dim2_slice_2d(ctx, kg_t, chunk)?;

        let v_t_p = ctx
            .mul_mat(ch_k_cd, state, BufferUsage::Activations)
            .map_err(LlamaError::format)?;
        let v_t_new = ctx
            .binary_like_a(Op::Sub, ch_v_t, v_t_p, BufferUsage::Activations)
            .map_err(LlamaError::format)?;
        let v_attn = ctx
            .mul_mat(v_t_new, ch_kq, BufferUsage::Activations)
            .map_err(LlamaError::format)?;
        let attn_inter = ctx
            .mul_mat(state, ch_q_g_exp, BufferUsage::Activations)
            .map_err(LlamaError::format)?;
        let o_ch = ctx
            .binary_like_a(Op::Add, attn_inter, v_attn, BufferUsage::Activations)
            .map_err(LlamaError::format)?;

        let v_tensor = require_tensor(ctx, v)?.clone();
        v = ctx
            .set_inplace(v, o_ch, v_tensor.nb[1], v_tensor.nb[2], v_tensor.nb[3], {
                v_tensor.nb[2]
                    .checked_mul(usize::try_from(chunk).map_err(|_| {
                        LlamaError::format(format!("chunk index {} does not fit in usize", chunk))
                    })?)
                    .ok_or_else(|| LlamaError::format("chunk output offset overflow"))?
            })
            .map_err(LlamaError::format)?;

        let kgv = ctx
            .mul_mat(ch_kg_t, v_t_new, BufferUsage::Activations)
            .map_err(LlamaError::format)?;
        let ch_g_last_exp_t = view_dim2_slice_2d(ctx, g_last_exp_t, chunk)?;
        state = ctx
            .binary_like_a(Op::Mul, state, ch_g_last_exp_t, BufferUsage::Activations)
            .map_err(LlamaError::format)?;
        state = ctx
            .binary_like_a(Op::Add, state, kgv, BufferUsage::Activations)
            .map_err(LlamaError::format)?;
    }

    let output = ctx
        .view_4d(
            v,
            s_v,
            n_seq_tokens,
            h_v,
            n_seqs,
            row_size(TensorType::F32, s_v)?,
            row_size(
                TensorType::F32,
                checked_mul(
                    s_v,
                    checked_mul(chunk_size, n_chunks, "output chunk span")?,
                    "output row span",
                )?,
            )?,
            row_size(
                TensorType::F32,
                checked_mul(
                    checked_mul(
                        s_v,
                        checked_mul(chunk_size, n_chunks, "output plane span")?,
                        "output plane base",
                    )?,
                    h_v,
                    "output batch span",
                )?,
            )?,
            0,
        )
        .map_err(LlamaError::format)?;
    let output = ctx
        .permute(output, [0, 2, 1, 3])
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(output, format!("{prefix}.output_view"))
        .map_err(LlamaError::format)?;

    let state = ctx
        .reshape(state, &[s_v, s_v, h_v, n_seqs])
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(state, format!("{prefix}.output_state"))
        .map_err(LlamaError::format)?;

    Ok((output, state))
}

fn build_delta_net_recurrent_decode_from_hidden(
    ctx: &mut Context,
    tensor_ids: &BTreeMap<String, TensorId>,
    spec: &DeltaNetRecurrentDecodeSpec,
    shared_cache: Option<&HybridRecurrentCacheIds>,
    input_state_rows: TensorId,
    input_embed: TensorId,
    n_tokens: usize,
    recurrent_checkpoints: bool,
    prefix: &str,
) -> Result<BuiltDeltaNetRecurrentDecode> {
    let block = &spec.block;
    let n_tokens_i64 =
        i64::try_from(n_tokens).map_err(|_| LlamaError::format("n_tokens does not fit in i64"))?;
    // Checkpointing repurposes the state-row input as `[resume_row, w_0..w_n]`
    // for a single sequence, so the row count no longer encodes n_seqs.
    let n_seqs = if recurrent_checkpoints {
        let rows = require_tensor(ctx, input_state_rows)?.ne[0];
        if rows != n_tokens_i64 + 1 {
            return Err(LlamaError::format(format!(
                "checkpointed delta-net decode expects {} state rows, got {}",
                n_tokens_i64 + 1,
                rows
            )));
        }
        1
    } else {
        require_tensor(ctx, input_state_rows)?.ne[0]
    };
    if n_seqs <= 0 {
        return Err(LlamaError::format(
            "delta-net recurrent decode requires at least one active sequence",
        ));
    }
    if n_tokens_i64 % n_seqs != 0 {
        return Err(LlamaError::format(format!(
            "delta-net recurrent decode token count {} is not divisible by active sequence count {}",
            n_tokens_i64, n_seqs
        )));
    }
    let n_seq_tokens = n_tokens_i64 / n_seqs;
    let value_hidden_size = i64::from(block.value_head_dim) * i64::from(block.value_head_count);
    let qkv_dim = i64::from(block.key_head_dim)
        .checked_mul(i64::from(block.key_head_count))
        .and_then(|v| v.checked_mul(2))
        .and_then(|v| v.checked_add(value_hidden_size))
        .ok_or_else(|| LlamaError::format("overflow computing delta-net qkv width"))?;

    let input_norm = build_rms_norm_mul(
        ctx,
        tensor_ids,
        input_embed,
        block.rms_epsilon,
        &block.input_norm_name,
        &format!("{prefix}.input_norm"),
    )?;

    let (qkv_mixed, z, mut beta, mut alpha) = if let Some(merged_input_proj_name) =
        &block.merged_input_proj_name
    {
        if block.qkv_proj_scale_name.is_some()
            || block.z_proj_scale_name.is_some()
            || block.beta_proj_scale_name.is_some()
            || block.alpha_proj_scale_name.is_some()
        {
            return Err(LlamaError::unsupported(format!(
                    "delta-net recurrent merged input projection currently requires unscaled branches: {}",
                    merged_input_proj_name
                )));
        }

        let merged_weight = require_tensor_id(tensor_ids, merged_input_proj_name)?;
        let mut merged = ctx
            .mul_mat(merged_weight, input_norm, BufferUsage::Activations)
            .map_err(LlamaError::format)?;
        let merged_width = qkv_dim
            .checked_add(value_hidden_size)
            .and_then(|value| value.checked_add(i64::from(block.value_head_count) * 2))
            .ok_or_else(|| LlamaError::format("overflow computing delta-net merged input width"))?;
        merged = ctx
            .reshape(merged, &[merged_width, n_seq_tokens, n_seqs])
            .map_err(LlamaError::format)?;
        ctx.set_tensor_name(merged, format!("{prefix}.merged_input_proj"))
            .map_err(LlamaError::format)?;
        let merged_tensor = ctx
            .tensor(merged)
            .ok_or_else(|| LlamaError::format("invalid delta-net merged input tensor"))?
            .clone();

        let qkv_mixed = ctx
            .view_3d(
                merged,
                qkv_dim,
                n_seq_tokens,
                n_seqs,
                merged_tensor.nb[1],
                merged_tensor.nb[2],
                0,
            )
            .map_err(LlamaError::format)?;
        ctx.set_tensor_name(qkv_mixed, format!("{prefix}.qkv_mixed"))
            .map_err(LlamaError::format)?;

        let z_offset = usize::try_from(qkv_dim)
            .ok()
            .and_then(|value| value.checked_mul(merged_tensor.nb[0]))
            .ok_or_else(|| LlamaError::format("overflow computing delta-net merged z offset"))?;
        let z = ctx
            .view_3d(
                merged,
                value_hidden_size,
                n_seq_tokens,
                n_seqs,
                merged_tensor.nb[1],
                merged_tensor.nb[2],
                z_offset,
            )
            .map_err(LlamaError::format)?;
        ctx.set_tensor_name(z, format!("{prefix}.z"))
            .map_err(LlamaError::format)?;

        let beta_width = i64::from(block.value_head_count);
        let beta_offset =
            usize::try_from(qkv_dim.checked_add(value_hidden_size).ok_or_else(|| {
                LlamaError::format("overflow computing delta-net merged beta base")
            })?)
            .ok()
            .and_then(|value| value.checked_mul(merged_tensor.nb[0]))
            .ok_or_else(|| LlamaError::format("overflow computing delta-net merged beta offset"))?;
        let beta = ctx
            .view_3d(
                merged,
                beta_width,
                n_seq_tokens,
                n_seqs,
                merged_tensor.nb[1],
                merged_tensor.nb[2],
                beta_offset,
            )
            .map_err(LlamaError::format)?;
        ctx.set_tensor_name(beta, format!("{prefix}.beta_raw"))
            .map_err(LlamaError::format)?;

        let alpha_offset = usize::try_from(
            qkv_dim
                .checked_add(value_hidden_size)
                .and_then(|value| value.checked_add(beta_width))
                .ok_or_else(|| {
                    LlamaError::format("overflow computing delta-net merged alpha base")
                })?,
        )
        .ok()
        .and_then(|value| value.checked_mul(merged_tensor.nb[0]))
        .ok_or_else(|| LlamaError::format("overflow computing delta-net merged alpha offset"))?;
        let alpha = ctx
            .view_3d(
                merged,
                beta_width,
                n_seq_tokens,
                n_seqs,
                merged_tensor.nb[1],
                merged_tensor.nb[2],
                alpha_offset,
            )
            .map_err(LlamaError::format)?;
        ctx.set_tensor_name(alpha, format!("{prefix}.alpha_raw"))
            .map_err(LlamaError::format)?;

        (qkv_mixed, z, beta, alpha)
    } else {
        let qkv_weight = require_tensor_id(tensor_ids, &block.qkv_proj_name)?;
        let mut qkv_mixed = ctx
            .mul_mat(qkv_weight, input_norm, BufferUsage::Activations)
            .map_err(LlamaError::format)?;
        qkv_mixed = apply_optional_proj_scale(
            ctx,
            tensor_ids,
            qkv_mixed,
            &block.qkv_proj_scale_name,
            &format!("{prefix}.qkv_mixed_scaled"),
        )?;
        qkv_mixed = ctx
            .reshape(qkv_mixed, &[qkv_dim, n_seq_tokens, n_seqs])
            .map_err(LlamaError::format)?;
        ctx.set_tensor_name(qkv_mixed, format!("{prefix}.qkv_mixed"))
            .map_err(LlamaError::format)?;

        let z_weight = require_tensor_id(tensor_ids, &block.z_proj_name)?;
        let mut z = ctx
            .mul_mat(z_weight, input_norm, BufferUsage::Activations)
            .map_err(LlamaError::format)?;
        z = apply_optional_proj_scale(
            ctx,
            tensor_ids,
            z,
            &block.z_proj_scale_name,
            &format!("{prefix}.z_scaled"),
        )?;
        ctx.set_tensor_name(z, format!("{prefix}.z"))
            .map_err(LlamaError::format)?;

        let beta_weight = require_tensor_id(tensor_ids, &block.beta_proj_name)?;
        let mut beta = ctx
            .mul_mat(beta_weight, input_norm, BufferUsage::Activations)
            .map_err(LlamaError::format)?;
        beta = apply_optional_proj_scale(
            ctx,
            tensor_ids,
            beta,
            &block.beta_proj_scale_name,
            &format!("{prefix}.beta_scaled"),
        )?;
        beta = ctx
            .reshape(
                beta,
                &[i64::from(block.value_head_count), n_seq_tokens, n_seqs],
            )
            .map_err(LlamaError::format)?;
        ctx.set_tensor_name(beta, format!("{prefix}.beta_raw"))
            .map_err(LlamaError::format)?;

        let alpha_weight = require_tensor_id(tensor_ids, &block.alpha_proj_name)?;
        let mut alpha = ctx
            .mul_mat(alpha_weight, input_norm, BufferUsage::Activations)
            .map_err(LlamaError::format)?;
        alpha = apply_optional_proj_scale(
            ctx,
            tensor_ids,
            alpha,
            &block.alpha_proj_scale_name,
            &format!("{prefix}.alpha_scaled"),
        )?;
        alpha = ctx
            .reshape(
                alpha,
                &[i64::from(block.value_head_count), n_seq_tokens, n_seqs],
            )
            .map_err(LlamaError::format)?;
        (qkv_mixed, z, beta, alpha)
    };

    beta = ctx
        .reshape(
            beta,
            &[1, i64::from(block.value_head_count), n_seq_tokens, n_seqs],
        )
        .map_err(LlamaError::format)?;
    beta = ctx
        .unary(beta, UnaryOp::Sigmoid, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(beta, format!("{prefix}.beta"))
        .map_err(LlamaError::format)?;

    let dt_bias = require_tensor_id(tensor_ids, &block.dt_bias_name)?;
    alpha = ctx
        .binary_like_a(Op::Add, alpha, dt_bias, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    alpha = ctx
        .unary(alpha, UnaryOp::SoftPlus, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    let ssm_a = require_tensor_id(tensor_ids, &block.a_name)?;
    let mut gate = ctx
        .binary_like_a(Op::Mul, alpha, ssm_a, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    gate = ctx
        .reshape(
            gate,
            &[1, i64::from(block.value_head_count), n_seq_tokens, n_seqs],
        )
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(gate, format!("{prefix}.gate"))
        .map_err(LlamaError::format)?;

    let conv_kernel_id = require_tensor_id(tensor_ids, &block.conv_kernel_name)?;
    let conv_kernel = require_tensor(ctx, conv_kernel_id)?;
    let conv_prefix = conv_kernel.ne[0]
        .checked_sub(1)
        .ok_or_else(|| LlamaError::format("delta-net conv kernel size underflow"))?;
    if conv_kernel.ne[1] != qkv_dim {
        return Err(LlamaError::format(format!(
            "delta-net conv kernel width mismatch: kernel={}, expected={}",
            conv_kernel.ne[1], qkv_dim
        )));
    }

    let r_width = conv_prefix
        .checked_mul(qkv_dim)
        .ok_or_else(|| LlamaError::format("overflow computing recurrent-r width"))?;
    let s_width = i64::from(block.value_head_dim)
        .checked_mul(i64::from(block.value_head_dim))
        .and_then(|v| v.checked_mul(i64::from(block.value_head_count)))
        .ok_or_else(|| LlamaError::format("overflow computing recurrent-s width"))?;

    let (r_cache, s_cache) = if let Some(shared_cache) = shared_cache {
        (shared_cache.r_cache, shared_cache.s_cache)
    } else {
        let r_cache = ctx
            .new_named_tensor(
                format!("{prefix}.r_cache"),
                spec.cache.r_type,
                2,
                &[r_width, n_seqs],
                BufferUsage::State,
            )
            .map_err(LlamaError::format)?;
        let s_cache = ctx
            .new_named_tensor(
                format!("{prefix}.s_cache"),
                spec.cache.s_type,
                2,
                &[s_width, n_seqs],
                BufferUsage::State,
            )
            .map_err(LlamaError::format)?;
        (r_cache, s_cache)
    };

    // With checkpointing the layout carries `n_tokens + 1` row indices:
    // element 0 is the row the scan resumes from and elements `1..=n_tokens`
    // receive the state after each token, so undoing a rejected speculative
    // draft is a change of resume row rather than a re-forward.
    let state_read_rows = if recurrent_checkpoints {
        ctx.view_1d(input_state_rows, 1, 0)
            .map_err(LlamaError::format)?
    } else {
        input_state_rows
    };

    let active_r_cache = ctx
        .get_rows(r_cache, state_read_rows, BufferUsage::State)
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(active_r_cache, format!("{prefix}.conv_states"))
        .map_err(LlamaError::format)?;
    let conv_states = ctx
        .view_3d(
            active_r_cache,
            conv_prefix,
            qkv_dim,
            n_seqs,
            row_size(TensorType::F32, conv_prefix)?,
            row_size(TensorType::F32, r_width)?,
            0,
        )
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(conv_states, format!("{prefix}.conv_states_reshaped"))
        .map_err(LlamaError::format)?;

    let mut qkv_mixed_t = ctx.transpose(qkv_mixed).map_err(LlamaError::format)?;
    // Debug bisection hatch: materialize the transpose before the concat.
    if std::env::var("MAKEPAD_LLAMA_DN_CONT_CONCAT").is_ok() {
        qkv_mixed_t = ctx.cont(qkv_mixed_t).map_err(LlamaError::format)?;
    }
    ctx.set_tensor_name(qkv_mixed_t, format!("{prefix}.qkv_mixed_transposed"))
        .map_err(LlamaError::format)?;
    let conv_input = ctx
        .concat(conv_states, qkv_mixed_t, 0, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(conv_input, format!("{prefix}.conv_input"))
        .map_err(LlamaError::format)?;

    let conv_input_tensor = require_tensor(ctx, conv_input)?.clone();
    // The conv state after token `t` is the `conv_prefix`-wide window of
    // `conv_input` ending at that token, so every checkpoint is a view at a
    // different offset — no extra compute, only the row write.
    // Every conv-state checkpoint is the same `conv_prefix`-wide window of
    // `conv_input` shifted by one token, so all of them are one strided view:
    // dim2 walks the checkpoints with a stride of a single element. That keeps
    // the checkpointed graph at ONE cont + ONE set_rows per layer instead of
    // one pair per token.
    let mut r_cache_updates = Vec::with_capacity(1);
    {
        let element = row_size(conv_input_tensor.desc.ty, 1)?;
        let (rows, offset, checkpoint_stride) = if recurrent_checkpoints {
            (n_tokens as i64, element, element)
        } else {
            (
                n_seqs,
                row_size(conv_input_tensor.desc.ty, n_seq_tokens)?,
                conv_input_tensor.nb[2],
            )
        };
        let conv_states_at = ctx
            .view_3d(
                conv_input,
                conv_prefix,
                qkv_dim,
                rows,
                conv_input_tensor.nb[1],
                checkpoint_stride,
                offset,
            )
            .map_err(LlamaError::format)?;
        let conv_states_rows = ctx
            .cont_2d(conv_states_at, r_width, rows)
            .map_err(LlamaError::format)?;
        ctx.set_tensor_name(conv_states_rows, format!("{prefix}.last_conv_states_rows"))
            .map_err(LlamaError::format)?;
        let write_rows = state_write_rows_all(ctx, input_state_rows, recurrent_checkpoints)?;
        r_cache_updates.push(
            ctx.set_rows(r_cache, conv_states_rows, write_rows, BufferUsage::State)
                .map_err(LlamaError::format)?,
        );
    }

    let active_s_cache = ctx
        .get_rows(s_cache, state_read_rows, BufferUsage::State)
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(active_s_cache, format!("{prefix}.active_s_cache"))
        .map_err(LlamaError::format)?;
    let state = ctx
        .view_4d(
            active_s_cache,
            i64::from(block.value_head_dim),
            i64::from(block.value_head_dim),
            i64::from(block.value_head_count),
            n_seqs,
            row_size(TensorType::F32, i64::from(block.value_head_dim))?,
            row_size(
                TensorType::F32,
                i64::from(block.value_head_dim) * i64::from(block.value_head_dim),
            )?,
            row_size(TensorType::F32, s_width)?,
            0,
        )
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(state, format!("{prefix}.state"))
        .map_err(LlamaError::format)?;

    let conv_output = ctx
        .ssm_conv(conv_input, conv_kernel_id, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(conv_output, format!("{prefix}.conv_raw"))
        .map_err(LlamaError::format)?;
    let conv_output = ctx
        .unary(conv_output, UnaryOp::Silu, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(conv_output, format!("{prefix}.conv_output"))
        .map_err(LlamaError::format)?;

    // Debug bisection hatch: bypass the causal conv, reading q/k/v straight
    // from the projection (same [qkv_dim, n_tokens, n_seqs] shape).
    let conv_output = if std::env::var("MAKEPAD_LLAMA_DN_NO_CONV").is_ok() {
        qkv_mixed
    } else {
        conv_output
    };
    let conv_output_tensor = require_tensor(ctx, conv_output)?.clone();
    let qk_heads_width = i64::from(block.key_head_dim) * i64::from(block.key_head_count);
    let q_conv = ctx
        .view_4d(
            conv_output,
            i64::from(block.key_head_dim),
            i64::from(block.key_head_count),
            n_seq_tokens,
            n_seqs,
            row_size(conv_output_tensor.desc.ty, i64::from(block.key_head_dim))?,
            row_size(conv_output_tensor.desc.ty, qkv_dim)?,
            row_size(conv_output_tensor.desc.ty, qkv_dim * n_seq_tokens)?,
            0,
        )
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(q_conv, format!("{prefix}.q_conv"))
        .map_err(LlamaError::format)?;
    let k_conv = ctx
        .view_4d(
            conv_output,
            i64::from(block.key_head_dim),
            i64::from(block.key_head_count),
            n_seq_tokens,
            n_seqs,
            row_size(conv_output_tensor.desc.ty, i64::from(block.key_head_dim))?,
            row_size(conv_output_tensor.desc.ty, qkv_dim)?,
            row_size(conv_output_tensor.desc.ty, qkv_dim * n_seq_tokens)?,
            row_size(conv_output_tensor.desc.ty, qk_heads_width)?,
        )
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(k_conv, format!("{prefix}.k_conv"))
        .map_err(LlamaError::format)?;
    let v_conv = ctx
        .view_4d(
            conv_output,
            i64::from(block.value_head_dim),
            i64::from(block.value_head_count),
            n_seq_tokens,
            n_seqs,
            row_size(conv_output_tensor.desc.ty, i64::from(block.value_head_dim))?,
            row_size(conv_output_tensor.desc.ty, qkv_dim)?,
            row_size(conv_output_tensor.desc.ty, qkv_dim * n_seq_tokens)?,
            row_size(conv_output_tensor.desc.ty, qk_heads_width * 2)?,
        )
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(v_conv, format!("{prefix}.v_conv"))
        .map_err(LlamaError::format)?;

    let use_fused_delta_net = true;

    // Debug bisection hatch: skip the pre-delta q/k L2 norm.
    let skip_qk_norm = std::env::var("MAKEPAD_LLAMA_DN_NO_QKNORM").is_ok();
    let mut q_conv = if skip_qk_norm {
        q_conv
    } else {
        ctx.l2_norm_eps(q_conv, block.rms_epsilon, BufferUsage::Activations)
            .map_err(LlamaError::format)?
    };
    ctx.set_tensor_name(q_conv, format!("{prefix}.q_conv_predelta"))
        .map_err(LlamaError::format)?;
    let mut k_conv = if skip_qk_norm {
        k_conv
    } else {
        ctx.l2_norm_eps(k_conv, block.rms_epsilon, BufferUsage::Activations)
            .map_err(LlamaError::format)?
    };
    ctx.set_tensor_name(k_conv, format!("{prefix}.k_conv_predelta"))
        .map_err(LlamaError::format)?;
    if block.value_head_count != block.key_head_count && !use_fused_delta_net {
        q_conv = ctx
            .repeat_4d(
                q_conv,
                i64::from(block.key_head_dim),
                i64::from(block.value_head_count),
                n_seq_tokens,
                n_seqs,
                BufferUsage::Activations,
            )
            .map_err(LlamaError::format)?;
        k_conv = ctx
            .repeat_4d(
                k_conv,
                i64::from(block.key_head_dim),
                i64::from(block.value_head_count),
                n_seq_tokens,
                n_seqs,
                BufferUsage::Activations,
            )
            .map_err(LlamaError::format)?;
        ctx.set_tensor_name(q_conv, format!("{prefix}.q_conv_predelta"))
            .map_err(LlamaError::format)?;
        ctx.set_tensor_name(k_conv, format!("{prefix}.k_conv_predelta"))
            .map_err(LlamaError::format)?;
    }

    // Speculative verify batches need the recurrent state after EVERY token,
    // not just the last one, so a rejected draft can be undone by resuming the
    // next step from an earlier cache row instead of re-running the forward.
    // The fused kernel emits them from the same single call, so the
    // checkpointed graph costs one extra state store per token and keeps the
    // node count of an ordinary decode.
    let (output, new_state, checkpoint_s_updates) = if use_fused_delta_net {
        let gated_delta = ctx
            .gated_delta_net_with_checkpoints(
                q_conv,
                k_conv,
                v_conv,
                gate,
                beta,
                state,
                recurrent_checkpoints,
                BufferUsage::Activations,
            )
            .map_err(LlamaError::format)?;
        ctx.set_tensor_name(
            gated_delta,
            if n_seq_tokens == 1 {
                format!("{prefix}.fgdn_ar")
            } else {
                format!("{prefix}.fgdn_ch")
            },
        )
        .map_err(LlamaError::format)?;

        let output = ctx
            .view_4d(
                gated_delta,
                i64::from(block.value_head_dim),
                i64::from(block.value_head_count),
                n_seq_tokens,
                n_seqs,
                row_size(TensorType::F32, i64::from(block.value_head_dim))?,
                row_size(TensorType::F32, value_hidden_size)?,
                row_size(TensorType::F32, value_hidden_size * n_seq_tokens)?,
                0,
            )
            .map_err(LlamaError::format)?;
        ctx.set_tensor_name(output, format!("{prefix}.output_view"))
            .map_err(LlamaError::format)?;

        let new_state = ctx
            .view_4d(
                gated_delta,
                i64::from(block.value_head_dim),
                i64::from(block.value_head_dim),
                i64::from(block.value_head_count),
                n_seqs,
                row_size(TensorType::F32, i64::from(block.value_head_dim))?,
                row_size(
                    TensorType::F32,
                    i64::from(block.value_head_dim) * i64::from(block.value_head_dim),
                )?,
                row_size(
                    TensorType::F32,
                    i64::from(block.value_head_dim)
                        * i64::from(block.value_head_dim)
                        * i64::from(block.value_head_count),
                )?,
                row_size(
                    TensorType::F32,
                    value_hidden_size * n_seq_tokens * n_seqs
                        + if recurrent_checkpoints {
                            s_width * n_seqs * (n_seq_tokens - 1)
                        } else {
                            0
                        },
                )?,
            )
            .map_err(LlamaError::format)?;
        ctx.set_tensor_name(new_state, format!("{prefix}.output_state"))
            .map_err(LlamaError::format)?;
        let checkpoint_updates = if recurrent_checkpoints {
            // The n state planes are contiguous right after the outputs, so
            // they are one `[s_width, n_tokens]` view and one `set_rows`.
            let planes = ctx
                .view_2d(
                    gated_delta,
                    s_width,
                    n_seq_tokens,
                    row_size(TensorType::F32, s_width)?,
                    row_size(TensorType::F32, value_hidden_size * n_seq_tokens * n_seqs)?,
                )
                .map_err(LlamaError::format)?;
            ctx.set_tensor_name(planes, format!("{prefix}.state_checkpoints"))
                .map_err(LlamaError::format)?;
            let write_rows = state_write_rows_all(ctx, input_state_rows, true)?;
            vec![ctx
                .set_rows(s_cache, planes, write_rows, BufferUsage::State)
                .map_err(LlamaError::format)?]
        } else {
            Vec::new()
        };
        (output, new_state, checkpoint_updates)
    } else if n_seq_tokens == 1 {
        let q_scaled = ctx
            .scale(
                q_conv,
                1.0f32 / (block.key_head_dim as f32).sqrt(),
                BufferUsage::Activations,
            )
            .map_err(LlamaError::format)?;
        ctx.set_tensor_name(q_scaled, format!("{prefix}.q_in"))
            .map_err(LlamaError::format)?;
        let q_ar = ctx
            .permute(q_scaled, [0, 2, 1, 3])
            .map_err(LlamaError::format)?;
        let k_ar = ctx
            .permute(k_conv, [0, 2, 1, 3])
            .map_err(LlamaError::format)?;
        let v_ar = ctx
            .permute(v_conv, [0, 2, 1, 3])
            .map_err(LlamaError::format)?;
        ctx.set_tensor_name(k_ar, format!("{prefix}.k_in"))
            .map_err(LlamaError::format)?;
        ctx.set_tensor_name(v_ar, format!("{prefix}.v_in"))
            .map_err(LlamaError::format)?;

        let gate_ar = ctx
            .reshape(gate, &[1, 1, i64::from(block.value_head_count), n_seqs])
            .map_err(LlamaError::format)?;
        let beta_ar = ctx
            .reshape(beta, &[1, 1, i64::from(block.value_head_count), n_seqs])
            .map_err(LlamaError::format)?;
        let gate_exp = ctx
            .unary(gate_ar, UnaryOp::Exp, BufferUsage::Activations)
            .map_err(LlamaError::format)?;
        let state_scaled = ctx
            .binary_like_a(Op::Mul, state, gate_exp, BufferUsage::Activations)
            .map_err(LlamaError::format)?;
        let sk = ctx
            .binary_like_a(Op::Mul, state_scaled, k_ar, BufferUsage::Activations)
            .map_err(LlamaError::format)?;
        let sk = ctx
            .sum_rows(sk, BufferUsage::Activations)
            .map_err(LlamaError::format)?;
        let sk_t = ctx.transpose(sk).map_err(LlamaError::format)?;
        let d = ctx
            .binary_like_a(Op::Sub, v_ar, sk_t, BufferUsage::Activations)
            .map_err(LlamaError::format)?;
        let d = ctx
            .binary_like_a(Op::Mul, d, beta_ar, BufferUsage::Activations)
            .map_err(LlamaError::format)?;
        let d_t = ctx.transpose(d).map_err(LlamaError::format)?;
        let k_rep = ctx
            .repeat(k_ar, state_scaled, BufferUsage::Activations)
            .map_err(LlamaError::format)?;
        let kd = ctx
            .binary_like_a(Op::Mul, k_rep, d_t, BufferUsage::Activations)
            .map_err(LlamaError::format)?;
        let new_state = ctx
            .binary_like_a(Op::Add, state_scaled, kd, BufferUsage::Activations)
            .map_err(LlamaError::format)?;
        ctx.set_tensor_name(new_state, format!("{prefix}.output_state"))
            .map_err(LlamaError::format)?;
        let s_q = ctx
            .binary_like_a(Op::Mul, new_state, q_ar, BufferUsage::Activations)
            .map_err(LlamaError::format)?;
        let output = ctx
            .sum_rows(s_q, BufferUsage::Activations)
            .map_err(LlamaError::format)?;
        let output = ctx
            .permute(output, [2, 0, 1, 3])
            .map_err(LlamaError::format)?;
        ctx.set_tensor_name(output, format!("{prefix}.output_view"))
            .map_err(LlamaError::format)?;
        (output, new_state, Vec::new())
    } else {
        let (output, new_state) = build_delta_net_chunking(
            ctx,
            block,
            q_conv,
            k_conv,
            v_conv,
            gate,
            beta,
            state,
            n_seq_tokens,
            n_seqs,
            prefix,
        )?;
        (output, new_state, Vec::new())
    };

    // Debug bisection hatch: pass raw v through instead of the delta-net
    // attention result (the state save still runs).
    let output = if std::env::var("MAKEPAD_LLAMA_DN_RAW_V").is_ok() {
        v_conv
    } else {
        output
    };
    let s_cache_updates = if checkpoint_s_updates.is_empty() {
        let new_state_nb3 = require_tensor(ctx, new_state)?.nb[3];
        let new_state_rows = ctx
            .view_2d(new_state, s_width, n_seqs, new_state_nb3, 0)
            .map_err(LlamaError::format)?;
        vec![ctx
            .set_rows(s_cache, new_state_rows, input_state_rows, BufferUsage::State)
            .map_err(LlamaError::format)?]
    } else {
        checkpoint_s_updates
    };

    let z = ctx
        .reshape(
            z,
            &[
                i64::from(block.value_head_dim),
                i64::from(block.value_head_count),
                n_seq_tokens,
                n_seqs,
            ],
        )
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(z, format!("{prefix}.z_4d"))
        .map_err(LlamaError::format)?;
    let output_rms = ctx
        .rms_norm_eps(output, block.rms_epsilon, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(output_rms, format!("{prefix}.output_rms"))
        .map_err(LlamaError::format)?;
    let norm_weight = require_tensor_id(tensor_ids, &block.norm_name)?;
    let output_norm = ctx
        .binary_like_a(Op::Mul, output_rms, norm_weight, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(output_norm, format!("{prefix}.output_norm"))
        .map_err(LlamaError::format)?;
    let z_silu = ctx
        .unary(z, UnaryOp::Silu, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(z_silu, format!("{prefix}.z_silu"))
        .map_err(LlamaError::format)?;
    let gated_output = ctx
        .binary_like_a(Op::Mul, output_norm, z_silu, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(gated_output, format!("{prefix}.gated_output"))
        .map_err(LlamaError::format)?;

    let final_output = ctx
        .reshape(gated_output, &[value_hidden_size, n_seq_tokens, n_seqs])
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(final_output, format!("{prefix}.final_output"))
        .map_err(LlamaError::format)?;
    let output_weight = require_tensor_id(tensor_ids, &block.output_proj_name)?;
    let mut linear_attn_out = ctx
        .mul_mat(output_weight, final_output, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    linear_attn_out = apply_optional_proj_scale(
        ctx,
        tensor_ids,
        linear_attn_out,
        &block.output_proj_scale_name,
        &format!("{prefix}.output_scaled"),
    )?;
    ctx.set_tensor_name(linear_attn_out, format!("{prefix}.linear_attn_out"))
        .map_err(LlamaError::format)?;
    let mut result_output = ctx
        .reshape(
            linear_attn_out,
            &[i64::from(block.embedding_length), n_seq_tokens * n_seqs],
        )
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(result_output, format!("{prefix}.output_reshaped"))
        .map_err(LlamaError::format)?;
    if block.residual {
        result_output = ctx
            .binary_like_a(
                Op::Add,
                result_output,
                input_embed,
                BufferUsage::Activations,
            )
            .map_err(LlamaError::format)?;
    }
    ctx.set_tensor_name(result_output, format!("{prefix}.output"))
        .map_err(LlamaError::format)?;

    Ok(BuiltDeltaNetRecurrentDecode {
        r_cache,
        s_cache,
        r_cache_updates,
        s_cache_updates,
        result_output,
    })
}

pub fn build_delta_net_recurrent_decode_graph(
    ctx: &mut Context,
    tensor_ids: &BTreeMap<String, TensorId>,
    spec: &DeltaNetRecurrentDecodeSpec,
    n_tokens: usize,
) -> Result<DeltaNetRecurrentDecodeGraph> {
    if n_tokens == 0 {
        return Err(LlamaError::format(
            "delta-net recurrent decode graph requires at least one token",
        ));
    }
    if spec.cache.r_type != TensorType::F32 || spec.cache.s_type != TensorType::F32 {
        return Err(LlamaError::unsupported(format!(
            "delta-net recurrent decode currently requires F32 caches, got r={} s={}",
            spec.cache.r_type.name(),
            spec.cache.s_type.name()
        )));
    }

    let block = &spec.block;
    let n_tokens_i64 =
        i64::try_from(n_tokens).map_err(|_| LlamaError::format("n_tokens does not fit in i64"))?;

    let input_primary = match &block.input {
        ProbeInputKind::TokenIds { .. } => ctx
            .new_named_tensor(
                "recur_decode.inp_tokens",
                TensorType::I32,
                1,
                &[n_tokens_i64],
                BufferUsage::Activations,
            )
            .map_err(LlamaError::format)?,
        ProbeInputKind::Embeddings {
            hidden_size,
            input_type,
        } => ctx
            .new_named_tensor(
                "recur_decode.inp_embd",
                *input_type,
                2,
                &[i64::from(*hidden_size), n_tokens_i64],
                BufferUsage::Activations,
            )
            .map_err(LlamaError::format)?,
    };
    mark_input(ctx, input_primary)?;
    let input_state_rows = ctx
        .new_named_tensor(
            "recur_decode.inp_state_rows",
            TensorType::I32,
            1,
            &[1],
            BufferUsage::Activations,
        )
        .map_err(LlamaError::format)?;
    mark_input(ctx, input_state_rows)?;

    let input_embed = match &block.input {
        ProbeInputKind::TokenIds {
            token_embedding_name,
            token_embedding_scale,
        } => {
            let token_embd = require_tensor_id(tensor_ids, token_embedding_name)?;
            let input_embed = ctx
                .get_rows(token_embd, input_primary, BufferUsage::Activations)
                .map_err(LlamaError::format)?;
            apply_optional_input_scale(
                ctx,
                input_embed,
                *token_embedding_scale,
                "recur_decode.input_embed_scaled",
            )?
        }
        ProbeInputKind::Embeddings { .. } => input_primary,
    };
    ctx.set_tensor_name(input_embed, "recur_decode.input_embed")
        .map_err(LlamaError::format)?;
    let built = build_delta_net_recurrent_decode_from_hidden(
        ctx,
        tensor_ids,
        spec,
        None,
        input_state_rows,
        input_embed,
        n_tokens,
        false,
        "recur_decode",
    )?;
    let result_output = built.result_output;
    mark_output(ctx, result_output)?;

    let mut graph = Graph::new();
    graph
        .build_forward_expand(ctx, built.r_cache_updates[0])
        .map_err(LlamaError::format)?;
    graph
        .build_forward_expand(ctx, built.s_cache_updates[0])
        .map_err(LlamaError::format)?;
    graph
        .build_forward_expand(ctx, result_output)
        .map_err(LlamaError::format)?;

    Ok(DeltaNetRecurrentDecodeGraph {
        graph,
        input_primary,
        input_state_rows,
        r_cache: built.r_cache,
        s_cache: built.s_cache,
        result_output,
    })
}

pub fn prepare_delta_net_recurrent_decode_graph(
    ctx: &mut Context,
    tensor_ids: &BTreeMap<String, TensorId>,
    spec: &DeltaNetRecurrentDecodeSpec,
    n_tokens: usize,
    features: MetalDeviceFeatures,
) -> Result<(DeltaNetRecurrentDecodeGraph, MetalPreparedGraph)> {
    let decode = build_delta_net_recurrent_decode_graph(ctx, tensor_ids, spec, n_tokens)?;
    let prepared = prepare_graph(ctx, &decode.graph, features).map_err(LlamaError::format)?;
    Ok((decode, prepared))
}

pub fn compile_delta_net_recurrent_decode_metal(
    weights: &mut LoadedGgufWeights,
    spec: &DeltaNetRecurrentDecodeSpec,
    n_tokens: usize,
) -> Result<CompiledDeltaNetRecurrentDecodeMetal> {
    let runtime = MetalRuntime::new().map_err(LlamaError::unsupported)?;
    let (decode, prepared) = prepare_delta_net_recurrent_decode_graph(
        &mut weights.ctx,
        &weights.tensor_ids,
        spec,
        n_tokens,
        runtime.features(),
    )?;
    let session = MetalGraphSession::from_runtime(
        runtime,
        &weights.ctx,
        &prepared,
        BufferStorageMode::Private,
        BufferStorageMode::Private,
    )
    .map_err(LlamaError::format)?;

    Ok(CompiledDeltaNetRecurrentDecodeMetal {
        spec: spec.clone(),
        decode,
        session,
    })
}

pub fn execute_prepared_delta_net_recurrent_decode_metal(
    runtime: &MetalRuntime,
    ctx: &mut Context,
    spec: &DeltaNetRecurrentDecodeSpec,
    decode: &DeltaNetRecurrentDecodeGraph,
    compiled: &MetalCompiledGraph,
    input: LogitsProbeInput<'_>,
) -> Result<AttentionBlockRun> {
    let input_primary = match (&spec.block.input, input) {
        (ProbeInputKind::TokenIds { .. }, LogitsProbeInput::TokenIds(token_ids)) => {
            i32_slice_as_bytes(token_ids).to_vec()
        }
        (
            ProbeInputKind::Embeddings {
                hidden_size,
                input_type,
            },
            LogitsProbeInput::EmbeddingsF32 { data, n_tokens },
        ) => {
            if *input_type != TensorType::F32 {
                return Err(LlamaError::unsupported(format!(
                    "delta-net recurrent decode currently expects F32 embeddings, got {}",
                    input_type.name()
                )));
            }
            let expected = (*hidden_size as usize)
                .checked_mul(n_tokens)
                .ok_or_else(|| {
                    LlamaError::format("overflow computing delta-net embedding input size")
                })?;
            if data.len() != expected {
                return Err(LlamaError::format(format!(
                    "delta-net recurrent embedding input length mismatch: got {}, expected {}",
                    data.len(),
                    expected
                )));
            }
            f32_slice_as_bytes(data).to_vec()
        }
        (ProbeInputKind::TokenIds { .. }, LogitsProbeInput::EmbeddingsF32 { .. }) => {
            return Err(LlamaError::format(
                "delta-net recurrent spec expects token ids but embeddings were provided",
            ));
        }
        (ProbeInputKind::Embeddings { .. }, LogitsProbeInput::TokenIds(_)) => {
            return Err(LlamaError::format(
                "delta-net recurrent spec expects embeddings but token ids were provided",
            ));
        }
    };

    let execution = execute_compiled_graph(
        runtime,
        ctx,
        compiled,
        &[
            MetalGraphTensorWrite {
                tensor_id: decode.input_primary,
                bytes: &input_primary,
            },
            MetalGraphTensorWrite {
                tensor_id: decode.input_state_rows,
                bytes: i32_slice_as_bytes(&[0]),
            },
        ],
        &[decode.result_output],
    )
    .map_err(LlamaError::format)?;

    let result_bytes = execution
        .outputs
        .get(&decode.result_output)
        .ok_or_else(|| {
            LlamaError::format("compiled delta-net recurrent decode did not produce result bytes")
        })?;
    let output = ctx
        .tensor(decode.result_output)
        .ok_or_else(|| LlamaError::format("delta-net recurrent result tensor is invalid"))?;
    let hidden = tensor_bytes_to_f32_vec(result_bytes, output.desc.ty)?;

    Ok(AttentionBlockRun {
        hidden,
        hidden_size: ne_usize(output, 0)?,
        n_tokens: ne_usize(output, 1)?,
    })
}

/// [`execute_prepared_delta_net_recurrent_decode_metal`], but also reading
/// back arbitrary intermediate tensors (by graph tensor id) as f32 data —
/// the instrument for bisecting wrong-output bugs inside the block.
pub fn execute_prepared_delta_net_recurrent_decode_metal_with_taps(
    runtime: &MetalRuntime,
    ctx: &mut Context,
    spec: &DeltaNetRecurrentDecodeSpec,
    decode: &DeltaNetRecurrentDecodeGraph,
    compiled: &MetalCompiledGraph,
    input: LogitsProbeInput<'_>,
    taps: &[TensorId],
) -> Result<(AttentionBlockRun, Vec<Vec<f32>>)> {
    let input_primary = match (&spec.block.input, input) {
        (ProbeInputKind::TokenIds { .. }, LogitsProbeInput::TokenIds(token_ids)) => {
            i32_slice_as_bytes(token_ids).to_vec()
        }
        _ => {
            return Err(LlamaError::format(
                "delta-net taps execute currently supports token-id input only",
            ))
        }
    };

    let mut outputs = vec![decode.result_output];
    outputs.extend_from_slice(taps);
    let execution = execute_compiled_graph(
        runtime,
        ctx,
        compiled,
        &[
            MetalGraphTensorWrite {
                tensor_id: decode.input_primary,
                bytes: &input_primary,
            },
            MetalGraphTensorWrite {
                tensor_id: decode.input_state_rows,
                bytes: i32_slice_as_bytes(&[0]),
            },
        ],
        &outputs,
    )
    .map_err(LlamaError::format)?;

    let result_bytes = execution
        .outputs
        .get(&decode.result_output)
        .ok_or_else(|| {
            LlamaError::format("compiled delta-net recurrent decode did not produce result bytes")
        })?;
    let output = ctx
        .tensor(decode.result_output)
        .ok_or_else(|| LlamaError::format("delta-net recurrent result tensor is invalid"))?;
    let hidden = tensor_bytes_to_f32_vec(result_bytes, output.desc.ty)?;
    let run = AttentionBlockRun {
        hidden,
        hidden_size: ne_usize(output, 0)?,
        n_tokens: ne_usize(output, 1)?,
    };

    let mut tap_values = Vec::with_capacity(taps.len());
    for &tap in taps {
        let bytes = execution.outputs.get(&tap).ok_or_else(|| {
            LlamaError::format(format!("delta-net tap tensor {tap} produced no bytes"))
        })?;
        let tensor = ctx
            .tensor(tap)
            .ok_or_else(|| LlamaError::format("delta-net tap tensor is invalid"))?;
        tap_values.push(tensor_bytes_to_f32_vec(bytes, tensor.desc.ty)?);
    }
    Ok((run, tap_values))
}

impl CompiledDeltaNetRecurrentDecodeMetal {
    /// The compiled graph, for offline inspection (buffer layout debugging).
    pub fn compiled_graph(&self) -> &MetalCompiledGraph {
        self.session.compiled()
    }

    pub fn execute_with_taps(
        &self,
        ctx: &mut Context,
        input: LogitsProbeInput<'_>,
        taps: &[TensorId],
    ) -> Result<(AttentionBlockRun, Vec<Vec<f32>>)> {
        execute_prepared_delta_net_recurrent_decode_metal_with_taps(
            self.session.runtime(),
            ctx,
            &self.spec,
            &self.decode,
            self.session.compiled(),
            input,
            taps,
        )
    }
}

pub fn execute_delta_net_recurrent_decode_graph_metal(
    weights: &mut LoadedGgufWeights,
    spec: &DeltaNetRecurrentDecodeSpec,
    n_tokens: usize,
    input: LogitsProbeInput<'_>,
) -> Result<AttentionBlockRun> {
    let compiled = compile_delta_net_recurrent_decode_metal(weights, spec, n_tokens)?;
    compiled.execute(&mut weights.ctx, input)
}

pub fn execute_delta_net_recurrent_decode_graph_metal_cached(
    compiled: &CompiledDeltaNetRecurrentDecodeMetal,
    weights: &mut LoadedGgufWeights,
    input: LogitsProbeInput<'_>,
) -> Result<AttentionBlockRun> {
    compiled.execute(&mut weights.ctx, input)
}

#[derive(Clone, Debug)]
struct BuiltMoeFfn {
    selected_experts: TensorId,
    result_output: TensorId,
}

#[derive(Clone, Debug)]
struct BuiltHybridLayerFfn {
    selected_experts: Option<(TensorId, usize)>,
    result_output: TensorId,
}

fn build_dense_layer_ffn_from_hidden(
    ctx: &mut Context,
    tensor_ids: &BTreeMap<String, TensorId>,
    spec: &DenseLayerFfnSpec,
    input_embed: TensorId,
    prefix: &str,
) -> Result<TensorId> {
    let input_hidden = if let Some(norm) = &spec.input_norm {
        build_rms_norm_mul(
            ctx,
            tensor_ids,
            input_embed,
            norm.epsilon,
            &norm.weight_name,
            &format!("{prefix}.input_norm"),
        )?
    } else {
        input_embed
    };

    let result_output = build_dense_gated_ffn(ctx, tensor_ids, input_hidden, &spec.ffn, prefix)?;
    ctx.set_tensor_name(result_output, format!("{prefix}.result_output"))
        .map_err(LlamaError::format)?;
    Ok(result_output)
}

fn build_moe_ffn_from_hidden(
    ctx: &mut Context,
    tensor_ids: &BTreeMap<String, TensorId>,
    spec: &MoeFfnSpec,
    input_embed: TensorId,
    n_tokens: usize,
    prefix: &str,
) -> Result<BuiltMoeFfn> {
    let input_hidden = if let Some(norm) = &spec.input_norm {
        build_rms_norm_mul(
            ctx,
            tensor_ids,
            input_embed,
            norm.epsilon,
            &norm.weight_name,
            &format!("{prefix}.input_norm"),
        )?
    } else {
        input_embed
    };

    let router_weight = require_tensor_id(tensor_ids, &spec.router_proj_name)?;
    let logits = ctx
        .mul_mat(router_weight, input_hidden, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(logits, format!("{prefix}.router_logits"))
        .map_err(LlamaError::format)?;

    let probs = match spec.gating_func {
        ExpertGatingFunc::SoftMax => ctx
            .soft_max(logits, BufferUsage::Activations)
            .map_err(LlamaError::format)?,
        ExpertGatingFunc::Sigmoid => ctx
            .unary(logits, UnaryOp::Sigmoid, BufferUsage::Activations)
            .map_err(LlamaError::format)?,
        ExpertGatingFunc::Identity => logits,
    };
    ctx.set_tensor_name(probs, format!("{prefix}.router_probs"))
        .map_err(LlamaError::format)?;

    let selected_experts_full = ctx
        .argsort(probs, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    ctx.tensor_mut(selected_experts_full)
        .ok_or_else(|| LlamaError::format("invalid moe selected_experts argsort tensor"))?
        .set_op_param_i32(0, SortOrder::Desc as i32);
    ctx.set_tensor_name(
        selected_experts_full,
        format!("{prefix}.selected_experts_argsort"),
    )
    .map_err(LlamaError::format)?;
    let selected_experts_full_tensor = ctx
        .tensor(selected_experts_full)
        .ok_or_else(|| LlamaError::format("invalid moe selected_experts argsort layout"))?
        .clone();
    let selected_experts = ctx
        .view_4d(
            selected_experts_full,
            i64::from(spec.expert_used_count),
            selected_experts_full_tensor.ne[1],
            selected_experts_full_tensor.ne[2],
            selected_experts_full_tensor.ne[3],
            selected_experts_full_tensor.nb[1],
            selected_experts_full_tensor.nb[2],
            selected_experts_full_tensor.nb[3],
            0,
        )
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(selected_experts, format!("{prefix}.selected_experts"))
        .map_err(LlamaError::format)?;

    let probs_3d = ctx
        .reshape(
            probs,
            &[
                1,
                i64::from(spec.expert_count),
                i64::try_from(n_tokens)
                    .map_err(|_| LlamaError::format("n_tokens does not fit in i64"))?,
            ],
        )
        .map_err(LlamaError::format)?;
    let weights_3d = ctx
        .get_rows(probs_3d, selected_experts, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    let mut weights = ctx
        .reshape(
            weights_3d,
            &[
                i64::from(spec.expert_used_count),
                i64::try_from(n_tokens)
                    .map_err(|_| LlamaError::format("n_tokens does not fit in i64"))?,
            ],
        )
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(weights, format!("{prefix}.selected_weights"))
        .map_err(LlamaError::format)?;

    if matches!(spec.gating_func, ExpertGatingFunc::Identity) {
        weights = ctx
            .soft_max(weights, BufferUsage::Activations)
            .map_err(LlamaError::format)?;
        ctx.set_tensor_name(weights, format!("{prefix}.selected_weights_softmax"))
            .map_err(LlamaError::format)?;
    }

    if spec.normalize_selected_weights {
        let mut weights_sum = ctx
            .sum_rows(weights, BufferUsage::Activations)
            .map_err(LlamaError::format)?;
        weights_sum = ctx
            .clamp(
                weights_sum,
                6.103_515_6e-5,
                f32::INFINITY,
                BufferUsage::Activations,
            )
            .map_err(LlamaError::format)?;
        weights = ctx
            .binary_like_a(Op::Div, weights, weights_sum, BufferUsage::Activations)
            .map_err(LlamaError::format)?;
        ctx.set_tensor_name(weights, format!("{prefix}.selected_weights_norm"))
            .map_err(LlamaError::format)?;
    }

    let mut weights = ctx
        .reshape(
            weights,
            &[
                1,
                i64::from(spec.expert_used_count),
                i64::try_from(n_tokens)
                    .map_err(|_| LlamaError::format("n_tokens does not fit in i64"))?,
            ],
        )
        .map_err(LlamaError::format)?;

    if spec.weight_scale != 1.0 {
        weights = ctx
            .scale(weights, spec.weight_scale, BufferUsage::Activations)
            .map_err(LlamaError::format)?;
        ctx.set_tensor_name(weights, format!("{prefix}.selected_weights_scaled"))
            .map_err(LlamaError::format)?;
    }

    let input_3d = ctx
        .reshape(
            input_hidden,
            &[
                ctx.tensor(input_hidden)
                    .ok_or_else(|| LlamaError::format("invalid moe ffn input tensor"))?
                    .ne[0],
                1,
                i64::try_from(n_tokens)
                    .map_err(|_| LlamaError::format("n_tokens does not fit in i64"))?,
            ],
        )
        .map_err(LlamaError::format)?;

    let apply_selected_expert_scale = |ctx: &mut Context,
                                       tensor: TensorId,
                                       scale_name: &str,
                                       scaled_name: String|
     -> Result<TensorId> {
        let scale_weight = require_tensor_id(tensor_ids, scale_name)?;
        let scale = ctx
            .reshape(scale_weight, &[1, i64::from(spec.expert_count), 1])
            .map_err(LlamaError::format)?;
        let scale = ctx
            .repeat_4d(
                scale,
                1,
                i64::from(spec.expert_count),
                i64::try_from(n_tokens)
                    .map_err(|_| LlamaError::format("n_tokens does not fit in i64"))?,
                1,
                BufferUsage::Activations,
            )
            .map_err(LlamaError::format)?;
        let scale = ctx
            .get_rows(scale, selected_experts, BufferUsage::Activations)
            .map_err(LlamaError::format)?;
        let scaled = ctx
            .binary_like_a(Op::Mul, tensor, scale, BufferUsage::Activations)
            .map_err(LlamaError::format)?;
        ctx.set_tensor_name(scaled, scaled_name)
            .map_err(LlamaError::format)?;
        Ok(scaled)
    };

    let (gate, up) = if let Some(merged_name) = &spec.merged_gate_up_proj_name {
        let merged_weight = require_tensor_id(tensor_ids, merged_name)?;
        let mut gate_up = ctx
            .mul_mat_id(
                merged_weight,
                input_3d,
                selected_experts,
                BufferUsage::Activations,
            )
            .map_err(LlamaError::format)?;
        ctx.set_tensor_name(gate_up, format!("{prefix}.gate_up"))
            .map_err(LlamaError::format)?;
        if let Some(scale_name) = &spec.up_proj_scale_name {
            gate_up = apply_selected_expert_scale(
                ctx,
                gate_up,
                scale_name,
                format!("{prefix}.gate_up_scaled"),
            )?;
        }
        let gate_up_tensor = ctx
            .tensor(gate_up)
            .ok_or_else(|| LlamaError::format("invalid merged gate/up tensor"))?
            .clone();
        if gate_up_tensor.ne[0] % 2 != 0 {
            return Err(LlamaError::format(format!(
                "merged gate/up width {} is not divisible by 2",
                gate_up_tensor.ne[0]
            )));
        }
        let ff_width = gate_up_tensor.ne[0] / 2;
        let gate = ctx
            .view_3d(
                gate_up,
                ff_width,
                gate_up_tensor.ne[1],
                gate_up_tensor.ne[2],
                gate_up_tensor.nb[1],
                gate_up_tensor.nb[2],
                0,
            )
            .map_err(LlamaError::format)?;
        ctx.set_tensor_name(gate, format!("{prefix}.gate"))
            .map_err(LlamaError::format)?;
        let up = ctx
            .view_3d(
                gate_up,
                ff_width,
                gate_up_tensor.ne[1],
                gate_up_tensor.ne[2],
                gate_up_tensor.nb[1],
                gate_up_tensor.nb[2],
                usize::try_from(ff_width)
                    .ok()
                    .and_then(|ff_width| ff_width.checked_mul(gate_up_tensor.nb[0]))
                    .ok_or_else(|| {
                        LlamaError::format("overflow computing merged gate/up offset")
                    })?,
            )
            .map_err(LlamaError::format)?;
        ctx.set_tensor_name(up, format!("{prefix}.up"))
            .map_err(LlamaError::format)?;
        (Some(gate), up)
    } else {
        let up_weight = require_tensor_id(tensor_ids, &spec.up_proj_name)?;
        let mut up = ctx
            .mul_mat_id(
                up_weight,
                input_3d,
                selected_experts,
                BufferUsage::Activations,
            )
            .map_err(LlamaError::format)?;
        ctx.set_tensor_name(up, format!("{prefix}.up"))
            .map_err(LlamaError::format)?;
        if let Some(scale_name) = &spec.up_proj_scale_name {
            up = apply_selected_expert_scale(ctx, up, scale_name, format!("{prefix}.up_scaled"))?;
        }
        let gate = if let Some(name) = &spec.gate_proj_name {
            let gate_weight = require_tensor_id(tensor_ids, name)?;
            let mut gate = ctx
                .mul_mat_id(
                    gate_weight,
                    input_3d,
                    selected_experts,
                    BufferUsage::Activations,
                )
                .map_err(LlamaError::format)?;
            ctx.set_tensor_name(gate, format!("{prefix}.gate"))
                .map_err(LlamaError::format)?;
            if let Some(scale_name) = &spec.gate_proj_scale_name {
                gate = apply_selected_expert_scale(
                    ctx,
                    gate,
                    scale_name,
                    format!("{prefix}.gate_scaled"),
                )?;
            }
            Some(gate)
        } else {
            None
        };
        (gate, up)
    };

    let activated = if let Some(gate) = gate {
        build_split_gated_hidden(ctx, gate, up, spec.activation, prefix)?
    } else {
        ctx.unary(up, spec.activation, BufferUsage::Activations)
            .map_err(LlamaError::format)?
    };
    ctx.set_tensor_name(activated, format!("{prefix}.hidden"))
        .map_err(LlamaError::format)?;

    let down_weight = require_tensor_id(tensor_ids, &spec.down_proj_name)?;
    let mut experts = ctx
        .mul_mat_id(
            down_weight,
            activated,
            selected_experts,
            BufferUsage::Activations,
        )
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(experts, format!("{prefix}.down"))
        .map_err(LlamaError::format)?;
    if let Some(scale_name) = &spec.down_proj_scale_name {
        experts =
            apply_selected_expert_scale(ctx, experts, scale_name, format!("{prefix}.down_scaled"))?;
    }

    let experts = ctx
        .binary_like_a(Op::Mul, experts, weights, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(experts, format!("{prefix}.weighted"))
        .map_err(LlamaError::format)?;

    let experts_tensor = ctx
        .tensor(experts)
        .ok_or_else(|| LlamaError::format("invalid weighted expert tensor"))?
        .clone();
    let mut moe_out = ctx
        .view_2d(
            experts,
            experts_tensor.ne[0],
            experts_tensor.ne[2],
            experts_tensor.nb[2],
            0,
        )
        .map_err(LlamaError::format)?;
    for slot in 1..usize::try_from(spec.expert_used_count)
        .map_err(|_| LlamaError::format("expert_used_count does not fit in usize"))?
    {
        let view = ctx
            .view_2d(
                experts,
                experts_tensor.ne[0],
                experts_tensor.ne[2],
                experts_tensor.nb[2],
                slot.checked_mul(experts_tensor.nb[1]).ok_or_else(|| {
                    LlamaError::format("overflow computing routed expert view offset")
                })?,
            )
            .map_err(LlamaError::format)?;
        moe_out = ctx
            .binary_like_a(Op::Add, moe_out, view, BufferUsage::Activations)
            .map_err(LlamaError::format)?;
    }
    if spec.expert_used_count == 1 {
        moe_out = ctx
            .cont_2d(moe_out, experts_tensor.ne[0], experts_tensor.ne[2])
            .map_err(LlamaError::format)?;
    }
    ctx.set_tensor_name(moe_out, format!("{prefix}.moe_out"))
        .map_err(LlamaError::format)?;

    let result_output = if let Some(shared) = &spec.shared_expert {
        let mut shared_out = build_dense_gated_ffn(
            ctx,
            tensor_ids,
            input_hidden,
            &shared.ffn,
            &format!("{prefix}.shared"),
        )?;
        if let Some(output_gate_name) = &shared.output_gate_name {
            let gate_weight = require_tensor_id(tensor_ids, output_gate_name)?;
            let gate = ctx
                .mul_mat(gate_weight, input_hidden, BufferUsage::Activations)
                .map_err(LlamaError::format)?;
            let gate = ctx
                .unary(
                    gate,
                    shared.output_gate_activation,
                    BufferUsage::Activations,
                )
                .map_err(LlamaError::format)?;
            shared_out = ctx
                .binary_like_a(Op::Mul, shared_out, gate, BufferUsage::Activations)
                .map_err(LlamaError::format)?;
            ctx.set_tensor_name(shared_out, format!("{prefix}.shared_gated"))
                .map_err(LlamaError::format)?;
        }
        ctx.binary_like_a(Op::Add, moe_out, shared_out, BufferUsage::Activations)
            .map_err(LlamaError::format)?
    } else {
        moe_out
    };
    ctx.set_tensor_name(result_output, format!("{prefix}.result_output"))
        .map_err(LlamaError::format)?;

    Ok(BuiltMoeFfn {
        selected_experts,
        result_output,
    })
}

fn build_hybrid_layer_ffn_from_hidden(
    ctx: &mut Context,
    tensor_ids: &BTreeMap<String, TensorId>,
    spec: &HybridLayerFfnSpec,
    input_embed: TensorId,
    n_tokens: usize,
    prefix: &str,
) -> Result<BuiltHybridLayerFfn> {
    match spec {
        HybridLayerFfnSpec::Dense(spec) => Ok(BuiltHybridLayerFfn {
            selected_experts: None,
            result_output: build_dense_layer_ffn_from_hidden(
                ctx,
                tensor_ids,
                spec,
                input_embed,
                prefix,
            )?,
        }),
        HybridLayerFfnSpec::Moe(spec) => {
            let moe =
                build_moe_ffn_from_hidden(ctx, tensor_ids, spec, input_embed, n_tokens, prefix)?;
            Ok(BuiltHybridLayerFfn {
                selected_experts: Some((
                    moe.selected_experts,
                    usize::try_from(spec.expert_used_count).map_err(|_| {
                        LlamaError::format("expert_used_count does not fit in usize")
                    })?,
                )),
                result_output: moe.result_output,
            })
        }
    }
}

pub fn build_moe_ffn_graph(
    ctx: &mut Context,
    tensor_ids: &BTreeMap<String, TensorId>,
    spec: &MoeFfnSpec,
    n_tokens: usize,
) -> Result<MoeFfnGraph> {
    if n_tokens == 0 {
        return Err(LlamaError::format(
            "moe ffn graph requires at least one token",
        ));
    }
    if spec.expert_used_count == 0 {
        return Err(LlamaError::format(
            "moe ffn graph requires at least one routed expert",
        ));
    }
    if spec.expert_used_count > spec.expert_count {
        return Err(LlamaError::format(format!(
            "moe ffn expert_used_count {} exceeds expert_count {}",
            spec.expert_used_count, spec.expert_count
        )));
    }

    let input_primary = match &spec.input {
        ProbeInputKind::TokenIds { .. } => ctx
            .new_named_tensor(
                "moe_ffn.inp_tokens",
                TensorType::I32,
                1,
                &[n_tokens as i64],
                BufferUsage::Activations,
            )
            .map_err(LlamaError::format)?,
        ProbeInputKind::Embeddings {
            hidden_size,
            input_type,
        } => ctx
            .new_named_tensor(
                "moe_ffn.inp_embd",
                *input_type,
                2,
                &[i64::from(*hidden_size), n_tokens as i64],
                BufferUsage::Activations,
            )
            .map_err(LlamaError::format)?,
    };
    mark_input(ctx, input_primary)?;

    let input_embed = match &spec.input {
        ProbeInputKind::TokenIds {
            token_embedding_name,
            token_embedding_scale,
        } => {
            let token_embd = require_tensor_id(tensor_ids, token_embedding_name)?;
            let input_embed = ctx
                .get_rows(token_embd, input_primary, BufferUsage::Activations)
                .map_err(LlamaError::format)?;
            apply_optional_input_scale(
                ctx,
                input_embed,
                *token_embedding_scale,
                "moe_ffn.input_embed_scaled",
            )?
        }
        ProbeInputKind::Embeddings { .. } => input_primary,
    };
    ctx.set_tensor_name(input_embed, "moe_ffn.input_embed")
        .map_err(LlamaError::format)?;

    let built = build_moe_ffn_from_hidden(ctx, tensor_ids, spec, input_embed, n_tokens, "moe_ffn")?;
    let result_output = built.result_output;
    mark_output(ctx, result_output)?;

    let mut graph = Graph::new();
    graph
        .build_forward_expand(ctx, result_output)
        .map_err(LlamaError::format)?;

    Ok(MoeFfnGraph {
        graph,
        input_primary,
        input_embed,
        selected_experts: built.selected_experts,
        result_output,
    })
}

pub fn prepare_moe_ffn_graph(
    ctx: &mut Context,
    tensor_ids: &BTreeMap<String, TensorId>,
    spec: &MoeFfnSpec,
    n_tokens: usize,
    features: MetalDeviceFeatures,
) -> Result<(MoeFfnGraph, MetalPreparedGraph)> {
    let block = build_moe_ffn_graph(ctx, tensor_ids, spec, n_tokens)?;
    let prepared = prepare_graph(ctx, &block.graph, features).map_err(LlamaError::format)?;
    Ok((block, prepared))
}

pub fn compile_moe_ffn_metal(
    weights: &mut LoadedGgufWeights,
    spec: &MoeFfnSpec,
    n_tokens: usize,
) -> Result<CompiledMoeFfnMetal> {
    let runtime = MetalRuntime::new().map_err(LlamaError::unsupported)?;
    let (block, prepared) = prepare_moe_ffn_graph(
        &mut weights.ctx,
        &weights.tensor_ids,
        spec,
        n_tokens,
        runtime.features(),
    )?;
    let session = MetalGraphSession::from_runtime(
        runtime,
        &weights.ctx,
        &prepared,
        BufferStorageMode::Private,
        BufferStorageMode::Private,
    )
    .map_err(LlamaError::format)?;

    Ok(CompiledMoeFfnMetal {
        spec: spec.clone(),
        block,
        session,
    })
}

pub fn execute_prepared_moe_ffn_metal(
    runtime: &MetalRuntime,
    ctx: &Context,
    spec: &MoeFfnSpec,
    block: &MoeFfnGraph,
    compiled: &MetalCompiledGraph,
    input: LogitsProbeInput<'_>,
) -> Result<MoeFfnRun> {
    let input_primary = match (&spec.input, input) {
        (ProbeInputKind::TokenIds { .. }, LogitsProbeInput::TokenIds(token_ids)) => {
            i32_slice_as_bytes(token_ids).to_vec()
        }
        (
            ProbeInputKind::Embeddings {
                hidden_size,
                input_type,
            },
            LogitsProbeInput::EmbeddingsF32 { data, n_tokens },
        ) => {
            if *input_type != TensorType::F32 {
                return Err(LlamaError::unsupported(format!(
                    "moe ffn currently expects F32 embeddings, got {}",
                    input_type.name()
                )));
            }
            let expected = (*hidden_size as usize)
                .checked_mul(n_tokens)
                .ok_or_else(|| LlamaError::format("overflow computing moe ffn embedding size"))?;
            if data.len() != expected {
                return Err(LlamaError::format(format!(
                    "moe ffn embedding input length mismatch: got {}, expected {}",
                    data.len(),
                    expected
                )));
            }
            f32_slice_as_bytes(data).to_vec()
        }
        (ProbeInputKind::TokenIds { .. }, LogitsProbeInput::EmbeddingsF32 { .. }) => {
            return Err(LlamaError::format(
                "moe ffn spec expects token ids but embeddings were provided",
            ));
        }
        (ProbeInputKind::Embeddings { .. }, LogitsProbeInput::TokenIds(_)) => {
            return Err(LlamaError::format(
                "moe ffn spec expects embeddings but token ids were provided",
            ));
        }
    };

    let execution = execute_compiled_graph(
        runtime,
        ctx,
        compiled,
        &[MetalGraphTensorWrite {
            tensor_id: block.input_primary,
            bytes: &input_primary,
        }],
        &[block.result_output, block.selected_experts],
    )
    .map_err(LlamaError::format)?;

    let result_bytes = execution
        .outputs
        .get(&block.result_output)
        .ok_or_else(|| LlamaError::format("compiled moe ffn did not produce result bytes"))?;
    let hidden = f32_bytes_to_vec(result_bytes)?;
    let selected_experts = execution
        .outputs
        .get(&block.selected_experts)
        .ok_or_else(|| LlamaError::format("compiled moe ffn did not produce selected experts"))?;
    let output = ctx
        .tensor(block.result_output)
        .ok_or_else(|| LlamaError::format("moe ffn result_output tensor is invalid"))?;

    Ok(MoeFfnRun {
        hidden,
        hidden_size: ne_usize(output, 0)?,
        n_tokens: ne_usize(output, 1)?,
        selected_experts: selected_experts
            .chunks_exact(std::mem::size_of::<i32>())
            .map(|chunk| i32::from_le_bytes(chunk.try_into().unwrap()))
            .collect(),
        expert_used_count: usize::try_from(spec.expert_used_count)
            .map_err(|_| LlamaError::format("expert_used_count does not fit in usize"))?,
    })
}

pub fn execute_moe_ffn_graph_metal(
    weights: &mut LoadedGgufWeights,
    spec: &MoeFfnSpec,
    n_tokens: usize,
    input: LogitsProbeInput<'_>,
) -> Result<MoeFfnRun> {
    let compiled = compile_moe_ffn_metal(weights, spec, n_tokens)?;
    compiled.execute(&weights.ctx, input)
}

pub fn execute_moe_ffn_graph_metal_cached(
    compiled: &CompiledMoeFfnMetal,
    weights: &LoadedGgufWeights,
    input: LogitsProbeInput<'_>,
) -> Result<MoeFfnRun> {
    compiled.execute(&weights.ctx, input)
}

pub fn allocate_hybrid_shared_cache_tensors(
    ctx: &mut Context,
    tensor_ids: &BTreeMap<String, TensorId>,
    spec: &HybridDecodeSpec,
) -> Result<HybridSharedCacheTensorIds> {
    let mut shared = HybridSharedCacheTensorIds::default();
    let mut allocated_attention = BTreeMap::<u32, HybridAttentionCacheIds>::new();

    for layer in &spec.layers {
        match layer {
            HybridLayerSpec::Attention {
                layer_index,
                decode,
                ..
            } => {
                let k_width =
                    i64::from(decode.block.k_head_dim) * i64::from(decode.block.kv_head_count);
                let v_width =
                    i64::from(decode.block.v_head_dim) * i64::from(decode.block.kv_head_count);
                let cache_layer_index = decode.cache_layer_index;
                let cache_ids = if let Some(existing) = allocated_attention.get(&cache_layer_index)
                {
                    let existing_k = require_tensor(ctx, existing.k_cache)?;
                    let existing_v = require_tensor(ctx, existing.v_cache)?;
                    let expected_k = [
                        k_width,
                        i64::from(decode.cache.max_context),
                        i64::from(decode.cache.max_sequences),
                    ];
                    let expected_v = [
                        v_width,
                        i64::from(decode.cache.max_context),
                        i64::from(decode.cache.max_sequences),
                    ];
                    if existing_k.desc.ty != decode.cache.k_type
                        || existing_v.desc.ty != decode.cache.v_type
                        || existing_k.ne[..3] != expected_k
                        || existing_v.ne[..3] != expected_v
                    {
                        return Err(LlamaError::format(format!(
                            "hybrid attention cache alias mismatch: layer {} reuses cache layer {} with incompatible shape or type",
                            layer_index, cache_layer_index
                        )));
                    }
                    existing.clone()
                } else {
                    let k_cache = ctx
                        .new_named_tensor(
                            format!("hybrid_cache.layer{cache_layer_index}.k_cache"),
                            decode.cache.k_type,
                            3,
                            &[
                                k_width,
                                i64::from(decode.cache.max_context),
                                i64::from(decode.cache.max_sequences),
                            ],
                            BufferUsage::State,
                        )
                        .map_err(LlamaError::format)?;
                    let v_cache = ctx
                        .new_named_tensor(
                            format!("hybrid_cache.layer{cache_layer_index}.v_cache"),
                            decode.cache.v_type,
                            3,
                            &[
                                v_width,
                                i64::from(decode.cache.max_context),
                                i64::from(decode.cache.max_sequences),
                            ],
                            BufferUsage::State,
                        )
                        .map_err(LlamaError::format)?;
                    let cache_ids = HybridAttentionCacheIds { k_cache, v_cache };
                    allocated_attention.insert(cache_layer_index, cache_ids.clone());
                    cache_ids
                };
                shared.attention.insert(*layer_index, cache_ids);
            }
            HybridLayerSpec::Recurrent {
                layer_index,
                decode,
                ..
            } => {
                let conv_kernel_id = require_tensor_id(tensor_ids, &decode.block.conv_kernel_name)?;
                let conv_kernel = require_tensor(ctx, conv_kernel_id)?;
                let conv_prefix = conv_kernel.ne[0]
                    .checked_sub(1)
                    .ok_or_else(|| LlamaError::format("delta-net conv kernel size underflow"))?;
                let qkv_dim = i64::from(decode.block.key_head_dim)
                    .checked_mul(i64::from(decode.block.key_head_count))
                    .and_then(|v| v.checked_mul(2))
                    .and_then(|v| {
                        v.checked_add(
                            i64::from(decode.block.value_head_dim)
                                * i64::from(decode.block.value_head_count),
                        )
                    })
                    .ok_or_else(|| LlamaError::format("overflow computing shared qkv width"))?;
                let r_width = conv_prefix.checked_mul(qkv_dim).ok_or_else(|| {
                    LlamaError::format("overflow computing shared recurrent-r width")
                })?;
                let s_width = i64::from(decode.block.value_head_dim)
                    .checked_mul(i64::from(decode.block.value_head_dim))
                    .and_then(|v| v.checked_mul(i64::from(decode.block.value_head_count)))
                    .ok_or_else(|| {
                        LlamaError::format("overflow computing shared recurrent-s width")
                    })?;
                let n_seqs = i64::from(decode.cache.max_sequences);
                let r_cache = ctx
                    .new_named_tensor(
                        format!("hybrid_cache.layer{layer_index}.r_cache"),
                        decode.cache.r_type,
                        2,
                        &[r_width, n_seqs],
                        BufferUsage::State,
                    )
                    .map_err(LlamaError::format)?;
                let s_cache = ctx
                    .new_named_tensor(
                        format!("hybrid_cache.layer{layer_index}.s_cache"),
                        decode.cache.s_type,
                        2,
                        &[s_width, n_seqs],
                        BufferUsage::State,
                    )
                    .map_err(LlamaError::format)?;
                shared
                    .recurrent
                    .insert(*layer_index, HybridRecurrentCacheIds { r_cache, s_cache });
            }
        }
    }

    Ok(shared)
}

fn view_contiguous_3d_axis2_slice_as_2d(
    ctx: &mut Context,
    tensor_id: TensorId,
    slice_index: u32,
    tensor_name: &str,
) -> Result<TensorId> {
    let tensor = require_tensor(ctx, tensor_id)?;
    let width = tensor.ne[0];
    let height = tensor.ne[1];
    let depth = tensor.ne[2];
    if width <= 0 || height <= 0 || depth <= 0 {
        return Err(LlamaError::format(format!(
            "expected non-empty 3D tensor for '{}', got shape {:?}",
            tensor.name().unwrap_or("<unnamed>"),
            tensor.ne
        )));
    }
    let slice_index_i64 = i64::from(slice_index);
    if slice_index_i64 >= depth {
        return Err(LlamaError::format(format!(
            "slice index {} is outside 0..{} for '{}'",
            slice_index,
            depth,
            tensor.name().unwrap_or("<unnamed>")
        )));
    }
    let row_stride = row_size(tensor.desc.ty, width)?;
    let offset = usize::try_from(slice_index_i64)
        .ok()
        .and_then(|value| value.checked_mul(tensor.nb[2]))
        .ok_or_else(|| LlamaError::format("overflow computing per-layer slice offset"))?;
    let slice = ctx
        .view_2d(tensor_id, width, height, row_stride, offset)
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(slice, tensor_name)
        .map_err(LlamaError::format)?;
    Ok(slice)
}

fn build_hybrid_per_layer_inputs(
    ctx: &mut Context,
    tensor_ids: &BTreeMap<String, TensorId>,
    input: &ProbeInputKind,
    input_per_layer_primary: TensorId,
    input_embed: TensorId,
    spec: &HybridPerLayerInputProjectSpec,
    n_tokens: usize,
    prefix: &str,
) -> Result<TensorId> {
    let n_tokens_i64 =
        i64::try_from(n_tokens).map_err(|_| LlamaError::format("n_tokens does not fit in i64"))?;
    let mut selected = match input {
        ProbeInputKind::TokenIds { .. } => {
            let token_embd = require_tensor_id(tensor_ids, &spec.token_embedding_name)?;
            ctx.get_rows(
                token_embd,
                input_per_layer_primary,
                BufferUsage::Activations,
            )
            .map_err(LlamaError::format)?
        }
        ProbeInputKind::Embeddings { .. } => {
            return Err(LlamaError::unsupported(
                "hybrid per-layer inputs with embedding primaries are not implemented".to_string(),
            ));
        }
    };
    ctx.set_tensor_name(selected, &format!("{prefix}.selected"))
        .map_err(LlamaError::format)?;
    selected = apply_optional_input_scale(
        ctx,
        selected,
        spec.token_embedding_scale,
        &format!("{prefix}.selected_scaled"),
    )?;
    selected = ctx
        .reshape(
            selected,
            &[
                i64::from(spec.hidden_size),
                i64::from(spec.layer_count),
                n_tokens_i64,
            ],
        )
        .map_err(LlamaError::format)?;
    selected = ctx.cont(selected).map_err(LlamaError::format)?;
    ctx.set_tensor_name(selected, &format!("{prefix}.selected_reshaped"))
        .map_err(LlamaError::format)?;

    let model_proj_weight = require_tensor_id(tensor_ids, &spec.model_proj_name)?;
    let mut model_proj = ctx
        .mul_mat(model_proj_weight, input_embed, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(model_proj, &format!("{prefix}.model_proj"))
        .map_err(LlamaError::format)?;
    model_proj = apply_optional_input_scale(
        ctx,
        model_proj,
        spec.model_proj_scale,
        &format!("{prefix}.model_proj_scaled"),
    )?;
    model_proj = ctx
        .reshape(
            model_proj,
            &[
                i64::from(spec.hidden_size),
                i64::from(spec.layer_count),
                n_tokens_i64,
            ],
        )
        .map_err(LlamaError::format)?;
    model_proj = ctx.cont(model_proj).map_err(LlamaError::format)?;
    ctx.set_tensor_name(model_proj, &format!("{prefix}.model_proj_reshaped"))
        .map_err(LlamaError::format)?;
    model_proj = build_rms_norm_mul(
        ctx,
        tensor_ids,
        model_proj,
        spec.proj_norm.epsilon,
        &spec.proj_norm.weight_name,
        &format!("{prefix}.model_proj_norm"),
    )?;

    let mut combined = ctx
        .binary_like_a(Op::Add, model_proj, selected, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(combined, &format!("{prefix}.combined"))
        .map_err(LlamaError::format)?;
    combined = apply_optional_input_scale(
        ctx,
        combined,
        spec.combine_scale,
        &format!("{prefix}.combined_scaled"),
    )?;
    combined = ctx
        .permute(combined, [0, 2, 1, 3])
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(combined, &format!("{prefix}.permuted"))
        .map_err(LlamaError::format)?;
    combined = ctx.cont(combined).map_err(LlamaError::format)?;
    ctx.set_tensor_name(combined, &format!("{prefix}.ready"))
        .map_err(LlamaError::format)?;
    Ok(combined)
}

fn build_hybrid_per_layer_residual(
    ctx: &mut Context,
    tensor_ids: &BTreeMap<String, TensorId>,
    hidden: TensorId,
    shared_per_layer_inputs: TensorId,
    spec: &HybridPerLayerInputLayerSpec,
    layer_index: u32,
    output_ids: Option<TensorId>,
    n_tokens: usize,
    prefix: &str,
) -> Result<TensorId> {
    let gate_weight = require_tensor_id(tensor_ids, &spec.input_gate_name)?;
    let mut gate = ctx
        .mul_mat(gate_weight, hidden, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(gate, &format!("{prefix}.gate"))
        .map_err(LlamaError::format)?;
    gate = ctx
        .unary(gate, spec.activation, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(gate, &format!("{prefix}.gate_act"))
        .map_err(LlamaError::format)?;

    let mut layer_inputs = view_contiguous_3d_axis2_slice_as_2d(
        ctx,
        shared_per_layer_inputs,
        layer_index,
        &format!("{prefix}.slice"),
    )?;
    if let Some(output_ids) = output_ids {
        layer_inputs = ctx
            .get_rows(layer_inputs, output_ids, BufferUsage::Activations)
            .map_err(LlamaError::format)?;
        ctx.set_tensor_name(layer_inputs, &format!("{prefix}.slice.out_ids"))
            .map_err(LlamaError::format)?;
    } else {
        let expected_tokens = i64::try_from(n_tokens)
            .map_err(|_| LlamaError::format("n_tokens does not fit in i64"))?;
        let layer_inputs_tensor = require_tensor(ctx, layer_inputs)?;
        if layer_inputs_tensor.ne[1] != expected_tokens {
            return Err(LlamaError::format(format!(
                "per-layer slice token count mismatch: expected {}, got {}",
                expected_tokens, layer_inputs_tensor.ne[1]
            )));
        }
    }

    let gated = ctx
        .binary_like_a(Op::Mul, gate, layer_inputs, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(gated, &format!("{prefix}.gated"))
        .map_err(LlamaError::format)?;

    let proj_weight = require_tensor_id(tensor_ids, &spec.proj_name)?;
    let projected = ctx
        .mul_mat(proj_weight, gated, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(projected, &format!("{prefix}.projected"))
        .map_err(LlamaError::format)?;
    let projected = build_rms_norm_mul(
        ctx,
        tensor_ids,
        projected,
        spec.post_norm.epsilon,
        &spec.post_norm.weight_name,
        &format!("{prefix}.post_norm"),
    )?;
    let residual = ctx
        .binary_like_a(Op::Add, hidden, projected, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(residual, &format!("{prefix}.residual"))
        .map_err(LlamaError::format)?;
    Ok(residual)
}

fn build_hybrid_decode_graph_impl(
    ctx: &mut Context,
    tensor_ids: &BTreeMap<String, TensorId>,
    spec: &HybridDecodeSpec,
    shared_cache: Option<&HybridSharedCacheTensorIds>,
    n_tokens: usize,
    n_outputs: usize,
    attention_key_count: usize,
) -> Result<HybridDecodeGraph> {
    if n_tokens == 0 {
        return Err(LlamaError::format(
            "hybrid decode graph requires at least one token",
        ));
    }
    if n_outputs == 0 || n_outputs > n_tokens {
        return Err(LlamaError::format(format!(
            "hybrid decode graph requires 1 <= n_outputs <= n_tokens, got n_outputs={} n_tokens={}",
            n_outputs, n_tokens
        )));
    }

    let input_primary = match &spec.input {
        ProbeInputKind::TokenIds { .. } => ctx
            .new_named_tensor(
                "hybrid_decode.inp_tokens",
                TensorType::I32,
                1,
                &[n_tokens as i64],
                BufferUsage::Activations,
            )
            .map_err(LlamaError::format)?,
        ProbeInputKind::Embeddings {
            hidden_size,
            input_type,
        } => ctx
            .new_named_tensor(
                "hybrid_decode.inp_embd",
                *input_type,
                2,
                &[i64::from(*hidden_size), n_tokens as i64],
                BufferUsage::Activations,
            )
            .map_err(LlamaError::format)?,
    };
    mark_input(ctx, input_primary)?;
    let input_per_layer_primary = if spec.per_layer_input.is_some() {
        match &spec.input {
            ProbeInputKind::TokenIds { .. } => {
                let per_layer_primary = ctx
                    .new_named_tensor(
                        "hybrid_decode.inp_per_layer_tokens",
                        TensorType::I32,
                        1,
                        &[n_tokens as i64],
                        BufferUsage::Activations,
                    )
                    .map_err(LlamaError::format)?;
                mark_input(ctx, per_layer_primary)?;
                Some(per_layer_primary)
            }
            ProbeInputKind::Embeddings { .. } => None,
        }
    } else {
        None
    };
    let input_output_ids = ctx
        .new_named_tensor(
            "hybrid_decode.inp_out_ids",
            TensorType::I32,
            1,
            &[i64::try_from(n_outputs)
                .map_err(|_| LlamaError::format("hybrid decode n_outputs does not fit in i64"))?],
            BufferUsage::Activations,
        )
        .map_err(LlamaError::format)?;
    mark_input(ctx, input_output_ids)?;

    let has_attention = spec
        .layers
        .iter()
        .any(|layer| matches!(layer, HybridLayerSpec::Attention { .. }));
    let has_recurrent = spec
        .layers
        .iter()
        .any(|layer| matches!(layer, HybridLayerSpec::Recurrent { .. }));
    let input_positions = if has_attention {
        let positions = ctx
            .new_named_tensor(
                "hybrid_decode.inp_pos",
                TensorType::I32,
                1,
                &[n_tokens as i64],
                BufferUsage::Activations,
            )
            .map_err(LlamaError::format)?;
        mark_input(ctx, positions)?;
        Some(positions)
    } else {
        None
    };
    let input_attention_write_indices = if has_attention {
        let indices = ctx
            .new_named_tensor(
                "hybrid_decode.inp_k_idxs",
                TensorType::I32,
                1,
                &[n_tokens as i64],
                BufferUsage::Activations,
            )
            .map_err(LlamaError::format)?;
        mark_input(ctx, indices)?;
        Some(indices)
    } else {
        None
    };
    let rope_position_components = spec
        .layers
        .iter()
        .filter_map(|layer| match layer {
            HybridLayerSpec::Attention { decode, .. } => decode.block.rope.as_ref(),
            HybridLayerSpec::Recurrent { .. } => None,
        })
        .map(rope_position_component_count)
        .max()
        .unwrap_or(0);
    let input_rope_positions = if rope_position_components > 0 {
        let positions = ctx
            .new_named_tensor(
                "hybrid_decode.inp_rope_pos",
                TensorType::I32,
                1,
                &[
                    i64::try_from(n_tokens.checked_mul(rope_position_components).ok_or_else(
                        || {
                            LlamaError::format(
                                "overflow computing hybrid decode rope position length",
                            )
                        },
                    )?)
                    .map_err(|_| {
                        LlamaError::format("hybrid rope position length does not fit in i64")
                    })?,
                ],
                BufferUsage::Activations,
            )
            .map_err(LlamaError::format)?;
        mark_input(ctx, positions)?;
        Some(positions)
    } else {
        None
    };
    let recurrent_checkpoints = spec.recurrent_checkpoints;
    // Row indices for the recurrent state cache. Without checkpointing this
    // is the single row read and written by every recurrent layer. With
    // checkpointing (speculative verify batches) it is `n_tokens + 1` long:
    // element 0 is the row the scan starts from, elements 1..=n_tokens are
    // the rows the per-token states are written to, so a rejected draft is
    // rolled back by simply reading a different row next step.
    let state_row_count = if recurrent_checkpoints {
        n_tokens
            .checked_add(1)
            .ok_or_else(|| LlamaError::format("overflow computing recurrent state row count"))?
    } else {
        1
    };
    let input_recurrent_state_rows = if has_recurrent {
        let rows = ctx
            .new_named_tensor(
                "hybrid_decode.inp_state_rows",
                TensorType::I32,
                1,
                &[i64::try_from(state_row_count).map_err(|_| {
                    LlamaError::format("recurrent state row count does not fit in i64")
                })?],
                BufferUsage::Activations,
            )
            .map_err(LlamaError::format)?;
        mark_input(ctx, rows)?;
        Some(rows)
    } else {
        None
    };
    let input_hidden_read_rows = if spec.mtp_prologue.is_some() {
        let rows = ctx
            .new_named_tensor(
                "hybrid_decode.inp_h_read_rows",
                TensorType::I32,
                1,
                &[n_tokens as i64],
                BufferUsage::Activations,
            )
            .map_err(LlamaError::format)?;
        mark_input(ctx, rows)?;
        Some(rows)
    } else {
        None
    };
    let input_hidden_write_rows = if spec.hidden_carry.is_some() {
        let rows = ctx
            .new_named_tensor(
                "hybrid_decode.inp_h_write_rows",
                TensorType::I32,
                1,
                &[n_tokens as i64],
                BufferUsage::Activations,
            )
            .map_err(LlamaError::format)?;
        mark_input(ctx, rows)?;
        Some(rows)
    } else {
        None
    };

    // llama-graph.cpp:22-35 / 2056-2057: one attn_inp_kq_mask (F32 host +
    // CAST F16 in their graph). We keep the F16 device tensor but share one
    // allocation per unique (dtype, window, n_kv, n_tokens) instead of
    // rebuilding 16 identical masks every token.
    let mut shared_kq_masks: BTreeMap<(u32, Option<u32>, i64, i64), TensorId> = BTreeMap::new();
    if has_attention {
        let n_kv = i64::try_from(attention_key_count).map_err(|_| {
            LlamaError::format("attention decode key_count does not fit in i64")
        })?;
        let n_tok = i64::try_from(n_tokens)
            .map_err(|_| LlamaError::format("n_tokens does not fit in i64"))?;
        for layer in &spec.layers {
            let HybridLayerSpec::Attention { decode, .. } = layer else {
                continue;
            };
            if !decode.block.causal {
                continue;
            }
            let ty = attention_mask_tensor_type(decode.block.q_head_dim, n_tokens);
            let key = (ty as u32, decode.block.causal_window, n_kv, n_tok);
            if shared_kq_masks.contains_key(&key) {
                continue;
            }
            let name = if shared_kq_masks.is_empty() {
                "hybrid_decode.attn_inp_kq_mask".to_string()
            } else {
                format!(
                    "hybrid_decode.attn_inp_kq_mask_{}",
                    shared_kq_masks.len()
                )
            };
            let mask = ctx
                .new_named_tensor(name, ty, 4, &[n_kv, n_tok, 1, 1], BufferUsage::Activations)
                .map_err(LlamaError::format)?;
            mark_input(ctx, mask)?;
            shared_kq_masks.insert(key, mask);
        }
    }

    let mut hidden = match &spec.input {
        ProbeInputKind::TokenIds {
            token_embedding_name,
            token_embedding_scale,
        } => {
            let token_embd = require_tensor_id(tensor_ids, token_embedding_name)?;
            let input_embed = ctx
                .get_rows(token_embd, input_primary, BufferUsage::Activations)
                .map_err(LlamaError::format)?;
            apply_optional_input_scale(
                ctx,
                input_embed,
                *token_embedding_scale,
                "hybrid_decode.input_embed_scaled",
            )?
        }
        ProbeInputKind::Embeddings { .. } => input_primary,
    };
    ctx.set_tensor_name(hidden, "hybrid_decode.input_embed")
        .map_err(LlamaError::format)?;

    // MTP draft prologue (llama.cpp `graph_mtp`): the draft block consumes the
    // embedding of the token at position p together with the main model's
    // post-final-norm hidden state at position p-1.
    if let Some(prologue) = &spec.mtp_prologue {
        let carry = require_tensor_id(tensor_ids, &prologue.hidden_carry_name)?;
        let read_rows = input_hidden_read_rows
            .ok_or_else(|| LlamaError::format("mtp prologue requires hidden read rows input"))?;
        let h_prev = ctx
            .get_rows(carry, read_rows, BufferUsage::Activations)
            .map_err(LlamaError::format)?;
        ctx.set_tensor_name(h_prev, "hybrid_decode.mtp_h_input")
            .map_err(LlamaError::format)?;
        let h_norm = build_rms_norm_mul(
            ctx,
            tensor_ids,
            h_prev,
            prologue.hnorm.epsilon,
            &prologue.hnorm.weight_name,
            "hybrid_decode.mtp_hnorm",
        )?;
        let e_norm = build_rms_norm_mul(
            ctx,
            tensor_ids,
            hidden,
            prologue.enorm.epsilon,
            &prologue.enorm.weight_name,
            "hybrid_decode.mtp_enorm",
        )?;
        let concat = ctx
            .concat(e_norm, h_norm, 0, BufferUsage::Activations)
            .map_err(LlamaError::format)?;
        ctx.set_tensor_name(concat, "hybrid_decode.mtp_concat")
            .map_err(LlamaError::format)?;
        let eh_proj = require_tensor_id(tensor_ids, &prologue.eh_proj_name)?;
        let projected = ctx
            .mul_mat(eh_proj, concat, BufferUsage::Activations)
            .map_err(LlamaError::format)?;
        hidden = apply_optional_proj_scale(
            ctx,
            tensor_ids,
            projected,
            &prologue.eh_proj_scale_name,
            "hybrid_decode.mtp_eh_proj_scaled",
        )?;
        ctx.set_tensor_name(hidden, "hybrid_decode.mtp_eh_proj")
            .map_err(LlamaError::format)?;
    }

    let shared_per_layer_inputs = spec
        .per_layer_input
        .as_ref()
        .map(|per_layer_input| {
            let per_layer_input_hidden = ctx.cont(hidden).map_err(LlamaError::format)?;
            ctx.set_tensor_name(
                per_layer_input_hidden,
                "hybrid_decode.input_embed.per_layer_copy",
            )
            .map_err(LlamaError::format)?;
            build_hybrid_per_layer_inputs(
                ctx,
                tensor_ids,
                &spec.input,
                input_per_layer_primary.unwrap_or(input_primary),
                per_layer_input_hidden,
                per_layer_input,
                n_tokens,
                "hybrid_decode.per_layer_input",
            )
        })
        .transpose()?;

    let mut attention_cache_views = Vec::new();
    let mut moe_selected_experts = Vec::new();
    let mut state_updates = Vec::new();
    let mut current_shared_attention = shared_cache.map(|cache| cache.attention.clone());
    let last_layer = spec.layers.len().checked_sub(1).ok_or_else(|| {
        LlamaError::format("hybrid decode spec requires at least one transformer layer")
    })?;

    for (layer_offset, layer) in spec.layers.iter().enumerate() {
        // With a hidden carry the narrowing moves past the final norm so
        // every batch position produces a hidden row; only the LM head runs
        // at `n_outputs` width.
        let is_last_layer = layer_offset == last_layer && spec.hidden_carry.is_none();
        match layer {
            HybridLayerSpec::Attention {
                layer_index,
                decode,
                post_attention_norm,
                ffn,
                post_ffn_norm,
                per_layer_input,
                output_scale_name,
            } => {
                let prefix = format!("hybrid_decode.layer{layer_index}");
                let mut decode = decode.clone();
                decode.block.residual = false;
                let positions = input_positions.ok_or_else(|| {
                    LlamaError::format(format!(
                        "attention layer {} requires position input",
                        layer_index
                    ))
                })?;
                let shared_input_mask = if decode.block.causal {
                    let ty = attention_mask_tensor_type(decode.block.q_head_dim, n_tokens);
                    let n_kv = i64::try_from(attention_key_count).map_err(|_| {
                        LlamaError::format("attention decode key_count does not fit in i64")
                    })?;
                    let n_tok = i64::try_from(n_tokens)
                        .map_err(|_| LlamaError::format("n_tokens does not fit in i64"))?;
                    let key = (ty as u32, decode.block.causal_window, n_kv, n_tok);
                    Some(*shared_kq_masks.get(&key).ok_or_else(|| {
                        LlamaError::format(format!(
                            "attention layer {} is missing a shared kq mask",
                            layer_index
                        ))
                    })?)
                } else {
                    None
                };
                let attn = build_attention_decode_from_hidden(
                    ctx,
                    tensor_ids,
                    &decode,
                    current_shared_attention
                        .as_ref()
                        .and_then(|cache| cache.get(&decode.cache_layer_index)),
                    hidden,
                    positions,
                    input_attention_write_indices.ok_or_else(|| {
                        LlamaError::format(format!(
                            "attention layer {} requires cache write indices",
                            layer_index
                        ))
                    })?,
                    input_rope_positions,
                    n_tokens,
                    attention_key_count,
                    shared_input_mask,
                    &format!("{prefix}.attn"),
                )?;
                if let Some(current_shared_attention) = current_shared_attention.as_mut() {
                    current_shared_attention.insert(
                        decode.cache_layer_index,
                        HybridAttentionCacheIds {
                            k_cache: attn.k_cache_written,
                            v_cache: attn.v_cache_written,
                        },
                    );
                }
                attention_cache_views.push(HybridAttentionCacheView {
                    layer_index: *layer_index,
                    input_mask: attn.input_mask,
                    k_cache_view: attn.k_cache_view,
                    v_cache_view: attn.v_cache_view,
                    k_head_dim: i64::from(decode.block.k_head_dim),
                    v_head_dim: i64::from(decode.block.v_head_dim),
                    kv_head_count: i64::from(decode.block.kv_head_count),
                    max_context: usize::try_from(decode.cache.max_context).map_err(|_| {
                        LlamaError::format(format!(
                            "attention layer {} max_context {} does not fit in usize",
                            layer_index, decode.cache.max_context
                        ))
                    })?,
                    graph_key_count: attention_key_count,
                    max_sequences: i64::from(decode.cache.max_sequences),
                    causal_window: decode
                        .block
                        .causal_window
                        .map(|window| usize::try_from(window).unwrap_or(usize::MAX)),
                });
                let mut layer_output = attn.result_output;
                let mut residual_input = hidden;
                if is_last_layer {
                    layer_output = ctx
                        .get_rows(layer_output, input_output_ids, BufferUsage::Activations)
                        .map_err(LlamaError::format)?;
                    ctx.set_tensor_name(layer_output, format!("{prefix}.attn.out_ids"))
                        .map_err(LlamaError::format)?;
                    residual_input = ctx
                        .get_rows(residual_input, input_output_ids, BufferUsage::Activations)
                        .map_err(LlamaError::format)?;
                    ctx.set_tensor_name(residual_input, format!("{prefix}.residual_in.out_ids"))
                        .map_err(LlamaError::format)?;
                }
                if let Some(norm) = post_attention_norm {
                    layer_output = build_rms_norm_mul(
                        ctx,
                        tensor_ids,
                        layer_output,
                        norm.epsilon,
                        &norm.weight_name,
                        &format!("{prefix}.attn_post_norm"),
                    )?;
                }
                let residual = ctx
                    .binary_like_a(
                        Op::Add,
                        layer_output,
                        residual_input,
                        BufferUsage::Activations,
                    )
                    .map_err(LlamaError::format)?;
                ctx.set_tensor_name(residual, format!("{prefix}.attn_residual"))
                    .map_err(LlamaError::format)?;
                let ffn = build_hybrid_layer_ffn_from_hidden(
                    ctx,
                    tensor_ids,
                    ffn,
                    residual,
                    if is_last_layer { n_outputs } else { n_tokens },
                    &format!("{prefix}.ffn"),
                )?;
                if let Some((selected_experts, expert_used_count)) = ffn.selected_experts {
                    moe_selected_experts.push(HybridMoeSelection {
                        layer_index: *layer_index,
                        selected_experts,
                        expert_used_count,
                    });
                }
                let ffn_output = if let Some(norm) = post_ffn_norm {
                    build_rms_norm_mul(
                        ctx,
                        tensor_ids,
                        ffn.result_output,
                        norm.epsilon,
                        &norm.weight_name,
                        &format!("{prefix}.ffn_post_norm"),
                    )?
                } else {
                    ffn.result_output
                };
                hidden = ctx
                    .binary_like_a(Op::Add, ffn_output, residual, BufferUsage::Activations)
                    .map_err(LlamaError::format)?;
                ctx.set_tensor_name(hidden, format!("{prefix}.pe_in"))
                    .map_err(LlamaError::format)?;
                if let Some(per_layer_input) = per_layer_input {
                    let shared_per_layer_inputs =
                        shared_per_layer_inputs.as_ref().ok_or_else(|| {
                            LlamaError::format(format!(
                                "layer {} requires shared per-layer inputs, but none were built",
                                layer_index
                            ))
                        })?;
                    hidden = build_hybrid_per_layer_residual(
                        ctx,
                        tensor_ids,
                        hidden,
                        *shared_per_layer_inputs,
                        per_layer_input,
                        *layer_index,
                        if is_last_layer {
                            Some(input_output_ids)
                        } else {
                            None
                        },
                        if is_last_layer { n_outputs } else { n_tokens },
                        &format!("{prefix}.per_layer_input"),
                    )?;
                }
                if let Some(scale_name) = output_scale_name {
                    let scale = require_tensor_id(tensor_ids, scale_name)?;
                    hidden = ctx
                        .binary_like_a(Op::Mul, hidden, scale, BufferUsage::Activations)
                        .map_err(LlamaError::format)?;
                    ctx.set_tensor_name(hidden, format!("{prefix}.post_scale"))
                        .map_err(LlamaError::format)?;
                }
                ctx.set_tensor_name(hidden, format!("{prefix}.post_ffn"))
                    .map_err(LlamaError::format)?;
            }
            HybridLayerSpec::Recurrent {
                layer_index,
                decode,
                ffn,
            } => {
                let prefix = format!("hybrid_decode.layer{layer_index}");
                let mut decode = decode.clone();
                decode.block.residual = false;
                let recur = build_delta_net_recurrent_decode_from_hidden(
                    ctx,
                    tensor_ids,
                    &decode,
                    shared_cache.and_then(|cache| cache.recurrent.get(layer_index)),
                    input_recurrent_state_rows.ok_or_else(|| {
                        LlamaError::format(format!(
                            "recurrent layer {} requires recurrent state rows",
                            layer_index
                        ))
                    })?,
                    hidden,
                    n_tokens,
                    recurrent_checkpoints,
                    &format!("{prefix}.recur"),
                )?;
                state_updates.extend(recur.r_cache_updates.iter().copied());
                state_updates.extend(recur.s_cache_updates.iter().copied());
                let mut layer_output = recur.result_output;
                let mut residual_input = hidden;
                if is_last_layer {
                    layer_output = ctx
                        .get_rows(layer_output, input_output_ids, BufferUsage::Activations)
                        .map_err(LlamaError::format)?;
                    ctx.set_tensor_name(layer_output, format!("{prefix}.recur.out_ids"))
                        .map_err(LlamaError::format)?;
                    residual_input = ctx
                        .get_rows(residual_input, input_output_ids, BufferUsage::Activations)
                        .map_err(LlamaError::format)?;
                    ctx.set_tensor_name(residual_input, format!("{prefix}.residual_in.out_ids"))
                        .map_err(LlamaError::format)?;
                }
                let residual = ctx
                    .binary_like_a(
                        Op::Add,
                        layer_output,
                        residual_input,
                        BufferUsage::Activations,
                    )
                    .map_err(LlamaError::format)?;
                ctx.set_tensor_name(residual, format!("{prefix}.attn_residual"))
                    .map_err(LlamaError::format)?;
                let ffn = build_hybrid_layer_ffn_from_hidden(
                    ctx,
                    tensor_ids,
                    ffn,
                    residual,
                    if is_last_layer { n_outputs } else { n_tokens },
                    &format!("{prefix}.ffn"),
                )?;
                if let Some((selected_experts, expert_used_count)) = ffn.selected_experts {
                    moe_selected_experts.push(HybridMoeSelection {
                        layer_index: *layer_index,
                        selected_experts,
                        expert_used_count,
                    });
                }
                hidden = ctx
                    .binary_like_a(
                        Op::Add,
                        ffn.result_output,
                        residual,
                        BufferUsage::Activations,
                    )
                    .map_err(LlamaError::format)?;
                ctx.set_tensor_name(hidden, format!("{prefix}.post_ffn"))
                    .map_err(LlamaError::format)?;
            }
        }
    }

    let result_hidden = hidden;
    ctx.set_tensor_name(result_hidden, "hybrid_decode.result_hidden")
        .map_err(LlamaError::format)?;
    mark_output(ctx, result_hidden)?;

    let result_norm = build_rms_norm_mul(
        ctx,
        tensor_ids,
        result_hidden,
        spec.rms_epsilon,
        &spec.output_norm_name,
        "hybrid_decode.result_norm",
    )?;
    // Publish the post-final-norm rows (llama.cpp's `t_h_nextn`) to the shared
    // carry tensor so the MTP draft graph reads them straight out of device
    // memory — llama.cpp round-trips these rows through the host instead.
    let hidden_carry_update = match &spec.hidden_carry {
        Some(carry) => {
            let carry_id = require_tensor_id(tensor_ids, &carry.tensor_name)?;
            let write_rows = input_hidden_write_rows.ok_or_else(|| {
                LlamaError::format("hidden carry requires the hidden write rows input")
            })?;
            let update = ctx
                .set_rows(carry_id, result_norm, write_rows, BufferUsage::State)
                .map_err(LlamaError::format)?;
            ctx.set_tensor_name(update, "hybrid_decode.hidden_carry_update")
                .map_err(LlamaError::format)?;
            Some(update)
        }
        None => None,
    };
    // Without a carry the narrowing already happened inside the last block.
    let head_input = if spec.hidden_carry.is_some() {
        let narrowed = ctx
            .get_rows(result_norm, input_output_ids, BufferUsage::Activations)
            .map_err(LlamaError::format)?;
        ctx.set_tensor_name(narrowed, "hybrid_decode.result_norm.out_ids")
            .map_err(LlamaError::format)?;
        narrowed
    } else {
        result_norm
    };
    let output_weight = require_tensor_id(tensor_ids, &spec.output_name)?;
    let mut result_logits = ctx
        .mul_mat(output_weight, head_input, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    if let Some(softcap) = spec.final_logit_softcap {
        result_logits = ctx
            .scale(result_logits, 1.0 / softcap, BufferUsage::Activations)
            .map_err(LlamaError::format)?;
        result_logits = ctx
            .unary(result_logits, UnaryOp::Tanh, BufferUsage::Activations)
            .map_err(LlamaError::format)?;
        result_logits = ctx
            .scale(result_logits, softcap, BufferUsage::Activations)
            .map_err(LlamaError::format)?;
    }
    ctx.set_tensor_name(result_logits, "hybrid_decode.result_logits")
        .map_err(LlamaError::format)?;
    mark_output(ctx, result_logits)?;

    let mut graph = Graph::new();
    for &update in &state_updates {
        graph
            .build_forward_expand(ctx, update)
            .map_err(LlamaError::format)?;
    }
    if let Some(update) = hidden_carry_update {
        graph
            .build_forward_expand(ctx, update)
            .map_err(LlamaError::format)?;
    }
    graph
        .build_forward_expand(ctx, result_hidden)
        .map_err(LlamaError::format)?;
    graph
        .build_forward_expand(ctx, result_logits)
        .map_err(LlamaError::format)?;

    let debug_outputs = if std::env::var_os("MAKEPAD_LLAMA_TRACE_ALL").is_some() {
        graph.nodes.clone()
    } else {
        Vec::new()
    };
    for &output in &debug_outputs {
        mark_output(ctx, output)?;
    }

    Ok(HybridDecodeGraph {
        graph,
        input_primary,
        input_per_layer_primary,
        input_output_ids,
        input_positions,
        input_attention_write_indices,
        input_rope_positions,
        input_recurrent_state_rows,
        input_hidden_read_rows,
        input_hidden_write_rows,
        attention_cache_views,
        moe_selected_experts,
        state_updates,
        debug_outputs,
        result_hidden,
        result_logits,
    })
}

fn default_attention_key_count(spec: &HybridDecodeSpec) -> Result<usize> {
    for layer in &spec.layers {
        if let HybridLayerSpec::Attention { decode, .. } = layer {
            return usize::try_from(decode.cache.max_context).map_err(|_| {
                LlamaError::format(format!(
                    "attention max_context {} does not fit in usize",
                    decode.cache.max_context
                ))
            });
        }
    }
    Ok(1)
}

pub fn build_hybrid_decode_graph(
    ctx: &mut Context,
    tensor_ids: &BTreeMap<String, TensorId>,
    spec: &HybridDecodeSpec,
    shared_cache: Option<&HybridSharedCacheTensorIds>,
    n_tokens: usize,
) -> Result<HybridDecodeGraph> {
    build_hybrid_decode_graph_impl(
        ctx,
        tensor_ids,
        spec,
        shared_cache,
        n_tokens,
        n_tokens,
        default_attention_key_count(spec)?,
    )
}

pub fn build_hybrid_decode_graph_with_outputs(
    ctx: &mut Context,
    tensor_ids: &BTreeMap<String, TensorId>,
    spec: &HybridDecodeSpec,
    shared_cache: Option<&HybridSharedCacheTensorIds>,
    n_tokens: usize,
    n_outputs: usize,
) -> Result<HybridDecodeGraph> {
    build_hybrid_decode_graph_impl(
        ctx,
        tensor_ids,
        spec,
        shared_cache,
        n_tokens,
        n_outputs,
        default_attention_key_count(spec)?,
    )
}

pub fn build_hybrid_decode_graph_with_attention_key_count(
    ctx: &mut Context,
    tensor_ids: &BTreeMap<String, TensorId>,
    spec: &HybridDecodeSpec,
    shared_cache: Option<&HybridSharedCacheTensorIds>,
    n_tokens: usize,
    n_outputs: usize,
    attention_key_count: usize,
) -> Result<HybridDecodeGraph> {
    build_hybrid_decode_graph_impl(
        ctx,
        tensor_ids,
        spec,
        shared_cache,
        n_tokens,
        n_outputs,
        attention_key_count,
    )
}

pub fn prepare_hybrid_decode_graph(
    ctx: &mut Context,
    tensor_ids: &BTreeMap<String, TensorId>,
    spec: &HybridDecodeSpec,
    shared_cache: Option<&HybridSharedCacheTensorIds>,
    n_tokens: usize,
    features: MetalDeviceFeatures,
) -> Result<(HybridDecodeGraph, MetalPreparedGraph)> {
    let decode = build_hybrid_decode_graph(ctx, tensor_ids, spec, shared_cache, n_tokens)?;
    let prepared = prepare_graph(ctx, &decode.graph, features).map_err(LlamaError::format)?;
    Ok((decode, prepared))
}

pub fn prepare_hybrid_decode_graph_with_outputs(
    ctx: &mut Context,
    tensor_ids: &BTreeMap<String, TensorId>,
    spec: &HybridDecodeSpec,
    shared_cache: Option<&HybridSharedCacheTensorIds>,
    n_tokens: usize,
    n_outputs: usize,
    features: MetalDeviceFeatures,
) -> Result<(HybridDecodeGraph, MetalPreparedGraph)> {
    let decode = build_hybrid_decode_graph_with_outputs(
        ctx,
        tensor_ids,
        spec,
        shared_cache,
        n_tokens,
        n_outputs,
    )?;
    let prepared = prepare_graph(ctx, &decode.graph, features).map_err(LlamaError::format)?;
    Ok((decode, prepared))
}

pub fn prepare_hybrid_decode_graph_with_attention_key_count(
    ctx: &mut Context,
    tensor_ids: &BTreeMap<String, TensorId>,
    spec: &HybridDecodeSpec,
    shared_cache: Option<&HybridSharedCacheTensorIds>,
    n_tokens: usize,
    n_outputs: usize,
    attention_key_count: usize,
    features: MetalDeviceFeatures,
) -> Result<(HybridDecodeGraph, MetalPreparedGraph)> {
    let decode = build_hybrid_decode_graph_with_attention_key_count(
        ctx,
        tensor_ids,
        spec,
        shared_cache,
        n_tokens,
        n_outputs,
        attention_key_count,
    )?;
    let prepared = prepare_graph(ctx, &decode.graph, features).map_err(LlamaError::format)?;
    Ok((decode, prepared))
}

pub fn create_metal_context_buffer(ctx: &Context) -> Result<MetalContextBuffers> {
    let runtime = MetalRuntime::new().map_err(LlamaError::unsupported)?;
    create_metal_context_buffer_with_runtime(&runtime, ctx)
}

pub fn create_metal_context_buffer_with_runtime(
    runtime: &MetalRuntime,
    ctx: &Context,
) -> Result<MetalContextBuffers> {
    // Two-region (mapped weight) contexts get a read-only no-copy buffer
    // over the mapping plus a no-copy dirty buffer over the arena.
    let ro_buffer = create_context_ro_buffer(runtime, ctx).map_err(LlamaError::format)?;
    let ro_split = if ro_buffer.is_some() { ctx.ro_split() } else { 0 };
    // Zero-copy shared weights on unified memory (one resident copy
    // instead of CPU arena + private GPU duplicate).
    // MAKEPAD_GGML_COPY_WEIGHTS=1 restores the old copying path (A/B;
    // single-arena contexts only — mapped weights are never copied).
    let main_buffer = if ro_buffer.is_none() && std::env::var_os("MAKEPAD_GGML_COPY_WEIGHTS").is_some()
    {
        create_context_main_buffer(runtime, ctx, BufferStorageMode::Private)
            .map_err(LlamaError::format)?
    } else {
        create_context_main_buffer_no_copy(runtime, ctx, BufferStorageMode::Private)
            .map_err(LlamaError::format)?
    };
    let _ = warmup_affine_qmm_weights(runtime, ctx);
    Ok(MetalContextBuffers {
        ro_buffer,
        ro_split,
        main_buffer,
    })
}

pub(crate) struct ImportedHybridGraphContext {
    pub(crate) ctx: Context,
    pub(crate) tensor_ids: BTreeMap<String, TensorId>,
    pub(crate) shared_cache: Option<HybridSharedCacheTensorIds>,
}

fn import_tensor_alias_cached(
    dst_ctx: &mut Context,
    src_ctx: &Context,
    imported_ids: &mut BTreeMap<TensorId, TensorId>,
    src_id: TensorId,
) -> Result<TensorId> {
    if let Some(&dst_id) = imported_ids.get(&src_id) {
        return Ok(dst_id);
    }
    let dst_id = dst_ctx
        .import_tensor_alias_from(src_ctx, src_id)
        .map_err(LlamaError::format)?;
    imported_ids.insert(src_id, dst_id);
    Ok(dst_id)
}

fn import_hybrid_shared_cache_aliases(
    dst_ctx: &mut Context,
    src_ctx: &Context,
    imported_ids: &mut BTreeMap<TensorId, TensorId>,
    shared_cache: &HybridSharedCacheTensorIds,
) -> Result<HybridSharedCacheTensorIds> {
    let mut imported = HybridSharedCacheTensorIds::default();

    for (&layer_index, cache_ids) in &shared_cache.attention {
        imported.attention.insert(
            layer_index,
            HybridAttentionCacheIds {
                k_cache: import_tensor_alias_cached(
                    dst_ctx,
                    src_ctx,
                    imported_ids,
                    cache_ids.k_cache,
                )?,
                v_cache: import_tensor_alias_cached(
                    dst_ctx,
                    src_ctx,
                    imported_ids,
                    cache_ids.v_cache,
                )?,
            },
        );
    }

    for (&layer_index, cache_ids) in &shared_cache.recurrent {
        imported.recurrent.insert(
            layer_index,
            HybridRecurrentCacheIds {
                r_cache: import_tensor_alias_cached(
                    dst_ctx,
                    src_ctx,
                    imported_ids,
                    cache_ids.r_cache,
                )?,
                s_cache: import_tensor_alias_cached(
                    dst_ctx,
                    src_ctx,
                    imported_ids,
                    cache_ids.s_cache,
                )?,
            },
        );
    }

    Ok(imported)
}

pub(crate) fn import_hybrid_graph_context(
    weights: &LoadedGgufWeights,
    shared_cache: Option<&HybridSharedCacheTensorIds>,
    copy_main_buffer: bool,
) -> Result<ImportedHybridGraphContext> {
    if copy_main_buffer && weights.ctx.ro_split() != 0 {
        return Err(LlamaError::format(
            "mapped weight contexts require a shared main buffer (the copying path is unsupported)",
        ));
    }
    let mut ctx = Context::new(InitParams {
        mem_size: weights.ctx.mem_size(),
        mem_buffer: Some(if copy_main_buffer {
            weights.ctx.mem_buffer().to_vec()
        } else {
            Vec::new()
        }),
        no_alloc: true,
    });
    let mut imported_ids = BTreeMap::new();
    let mut tensor_ids = BTreeMap::new();

    for (name, &tensor_id) in &weights.tensor_ids {
        let imported_id =
            import_tensor_alias_cached(&mut ctx, &weights.ctx, &mut imported_ids, tensor_id)?;
        tensor_ids.insert(name.clone(), imported_id);
    }

    let shared_cache = shared_cache
        .map(|cache| {
            import_hybrid_shared_cache_aliases(&mut ctx, &weights.ctx, &mut imported_ids, cache)
        })
        .transpose()?;

    Ok(ImportedHybridGraphContext {
        ctx,
        tensor_ids,
        shared_cache,
    })
}

pub(crate) fn reserve_hybrid_decode_main_buffer_size(
    weights: &LoadedGgufWeights,
    spec: &HybridDecodeSpec,
    shared_cache: Option<&HybridSharedCacheTensorIds>,
    n_tokens: usize,
    n_outputs: usize,
    features: MetalDeviceFeatures,
) -> Result<usize> {
    let ImportedHybridGraphContext {
        mut ctx,
        tensor_ids,
        shared_cache,
    } = import_hybrid_graph_context(weights, shared_cache, false)?;
    let prepared = if n_outputs == n_tokens {
        let (_, prepared) = prepare_hybrid_decode_graph(
            &mut ctx,
            &tensor_ids,
            spec,
            shared_cache.as_ref(),
            n_tokens,
            features,
        )?;
        prepared
    } else {
        let (_, prepared) = prepare_hybrid_decode_graph_with_outputs(
            &mut ctx,
            &tensor_ids,
            spec,
            shared_cache.as_ref(),
            n_tokens,
            n_outputs,
            features,
        )?;
        prepared
    };
    Ok(prepared.main_buffer_size)
}

fn compile_hybrid_decode_metal_impl(
    weights: &mut LoadedGgufWeights,
    spec: &HybridDecodeSpec,
    shared_runtime: Option<&MetalRuntime>,
    shared_cache: Option<&HybridSharedCacheTensorIds>,
    shared_buffers: Option<&MetalContextBuffers>,
    n_tokens: usize,
    n_outputs: usize,
    attention_key_count: Option<usize>,
) -> Result<CompiledHybridDecodeMetal> {
    let runtime = if let Some(runtime) = shared_runtime {
        runtime.clone()
    } else {
        MetalRuntime::new().map_err(LlamaError::unsupported)?
    };
    let ImportedHybridGraphContext {
        mut ctx,
        tensor_ids,
        shared_cache,
    } = import_hybrid_graph_context(weights, shared_cache, shared_buffers.is_none())?;
    let (decode, prepared) = if let Some(attention_key_count) = attention_key_count {
        prepare_hybrid_decode_graph_with_attention_key_count(
            &mut ctx,
            &tensor_ids,
            spec,
            shared_cache.as_ref(),
            n_tokens,
            n_outputs,
            attention_key_count,
            runtime.features(),
        )?
    } else if n_outputs == n_tokens {
        prepare_hybrid_decode_graph(
            &mut ctx,
            &tensor_ids,
            spec,
            shared_cache.as_ref(),
            n_tokens,
            runtime.features(),
        )?
    } else {
        prepare_hybrid_decode_graph_with_outputs(
            &mut ctx,
            &tensor_ids,
            spec,
            shared_cache.as_ref(),
            n_tokens,
            n_outputs,
            runtime.features(),
        )?
    };
    let session = if let Some(buffers) = shared_buffers {
        MetalGraphSession::from_runtime_with_context_buffers(
            runtime,
            &prepared,
            buffers,
            BufferStorageMode::Private,
        )
    } else {
        MetalGraphSession::from_runtime(
            runtime,
            &ctx,
            &prepared,
            BufferStorageMode::Private,
            BufferStorageMode::Private,
        )
    }
    .map_err(LlamaError::format)?;

    Ok(CompiledHybridDecodeMetal {
        spec: spec.clone(),
        decode,
        graph_ctx: ctx,
        session,
    })
}

pub fn compile_hybrid_decode_metal(
    weights: &mut LoadedGgufWeights,
    spec: &HybridDecodeSpec,
    n_tokens: usize,
) -> Result<CompiledHybridDecodeMetal> {
    compile_hybrid_decode_metal_impl(weights, spec, None, None, None, n_tokens, n_tokens, None)
}

pub fn compile_hybrid_decode_metal_with_outputs(
    weights: &mut LoadedGgufWeights,
    spec: &HybridDecodeSpec,
    n_tokens: usize,
    n_outputs: usize,
) -> Result<CompiledHybridDecodeMetal> {
    compile_hybrid_decode_metal_impl(weights, spec, None, None, None, n_tokens, n_outputs, None)
}

pub fn compile_hybrid_decode_metal_with_shared_state(
    weights: &mut LoadedGgufWeights,
    spec: &HybridDecodeSpec,
    shared_cache: &HybridSharedCacheTensorIds,
    shared_buffers: &MetalContextBuffers,
    n_tokens: usize,
) -> Result<CompiledHybridDecodeMetal> {
    compile_hybrid_decode_metal_impl(
        weights,
        spec,
        None,
        Some(shared_cache),
        Some(shared_buffers),
        n_tokens,
        n_tokens,
        None,
    )
}

pub fn compile_hybrid_decode_metal_with_shared_state_and_outputs(
    weights: &mut LoadedGgufWeights,
    spec: &HybridDecodeSpec,
    shared_cache: &HybridSharedCacheTensorIds,
    shared_buffers: &MetalContextBuffers,
    n_tokens: usize,
    n_outputs: usize,
) -> Result<CompiledHybridDecodeMetal> {
    compile_hybrid_decode_metal_impl(
        weights,
        spec,
        None,
        Some(shared_cache),
        Some(shared_buffers),
        n_tokens,
        n_outputs,
        None,
    )
}

pub fn compile_hybrid_decode_metal_with_shared_runtime_and_state(
    weights: &mut LoadedGgufWeights,
    spec: &HybridDecodeSpec,
    shared_runtime: &MetalRuntime,
    shared_cache: &HybridSharedCacheTensorIds,
    shared_buffers: &MetalContextBuffers,
    n_tokens: usize,
) -> Result<CompiledHybridDecodeMetal> {
    compile_hybrid_decode_metal_impl(
        weights,
        spec,
        Some(shared_runtime),
        Some(shared_cache),
        Some(shared_buffers),
        n_tokens,
        n_tokens,
        None,
    )
}

pub fn compile_hybrid_decode_metal_with_shared_runtime_and_state_and_outputs(
    weights: &mut LoadedGgufWeights,
    spec: &HybridDecodeSpec,
    shared_runtime: &MetalRuntime,
    shared_cache: &HybridSharedCacheTensorIds,
    shared_buffers: &MetalContextBuffers,
    n_tokens: usize,
    n_outputs: usize,
) -> Result<CompiledHybridDecodeMetal> {
    compile_hybrid_decode_metal_impl(
        weights,
        spec,
        Some(shared_runtime),
        Some(shared_cache),
        Some(shared_buffers),
        n_tokens,
        n_outputs,
        None,
    )
}

pub fn compile_hybrid_decode_metal_with_shared_runtime_and_state_and_outputs_and_attention_key_count(
    weights: &mut LoadedGgufWeights,
    spec: &HybridDecodeSpec,
    shared_runtime: &MetalRuntime,
    shared_cache: &HybridSharedCacheTensorIds,
    shared_buffers: &MetalContextBuffers,
    n_tokens: usize,
    n_outputs: usize,
    attention_key_count: usize,
) -> Result<CompiledHybridDecodeMetal> {
    compile_hybrid_decode_metal_impl(
        weights,
        spec,
        Some(shared_runtime),
        Some(shared_cache),
        Some(shared_buffers),
        n_tokens,
        n_outputs,
        Some(attention_key_count),
    )
}

pub fn compile_hybrid_prompt_processing_metal(
    weights: &mut LoadedGgufWeights,
    spec: &HybridDecodeSpec,
    n_tokens: usize,
) -> Result<CompiledHybridDecodeMetal> {
    compile_hybrid_decode_metal(weights, spec, n_tokens)
}

pub fn compile_hybrid_prompt_processing_metal_with_outputs(
    weights: &mut LoadedGgufWeights,
    spec: &HybridDecodeSpec,
    n_tokens: usize,
    n_outputs: usize,
) -> Result<CompiledHybridDecodeMetal> {
    compile_hybrid_decode_metal_with_outputs(weights, spec, n_tokens, n_outputs)
}

pub fn compile_hybrid_prompt_processing_metal_with_shared_state(
    weights: &mut LoadedGgufWeights,
    spec: &HybridDecodeSpec,
    shared_cache: &HybridSharedCacheTensorIds,
    shared_buffers: &MetalContextBuffers,
    n_tokens: usize,
) -> Result<CompiledHybridDecodeMetal> {
    compile_hybrid_decode_metal_with_shared_state(
        weights,
        spec,
        shared_cache,
        shared_buffers,
        n_tokens,
    )
}

pub fn compile_hybrid_prompt_processing_metal_with_shared_state_and_outputs(
    weights: &mut LoadedGgufWeights,
    spec: &HybridDecodeSpec,
    shared_cache: &HybridSharedCacheTensorIds,
    shared_buffers: &MetalContextBuffers,
    n_tokens: usize,
    n_outputs: usize,
) -> Result<CompiledHybridDecodeMetal> {
    compile_hybrid_decode_metal_with_shared_state_and_outputs(
        weights,
        spec,
        shared_cache,
        shared_buffers,
        n_tokens,
        n_outputs,
    )
}

pub fn compile_hybrid_prompt_processing_metal_with_shared_runtime_and_state(
    weights: &mut LoadedGgufWeights,
    spec: &HybridDecodeSpec,
    shared_runtime: &MetalRuntime,
    shared_cache: &HybridSharedCacheTensorIds,
    shared_buffers: &MetalContextBuffers,
    n_tokens: usize,
) -> Result<CompiledHybridDecodeMetal> {
    compile_hybrid_decode_metal_with_shared_runtime_and_state(
        weights,
        spec,
        shared_runtime,
        shared_cache,
        shared_buffers,
        n_tokens,
    )
}

pub fn compile_hybrid_prompt_processing_metal_with_shared_runtime_and_state_and_outputs(
    weights: &mut LoadedGgufWeights,
    spec: &HybridDecodeSpec,
    shared_runtime: &MetalRuntime,
    shared_cache: &HybridSharedCacheTensorIds,
    shared_buffers: &MetalContextBuffers,
    n_tokens: usize,
    n_outputs: usize,
) -> Result<CompiledHybridDecodeMetal> {
    compile_hybrid_decode_metal_with_shared_runtime_and_state_and_outputs(
        weights,
        spec,
        shared_runtime,
        shared_cache,
        shared_buffers,
        n_tokens,
        n_outputs,
    )
}

pub fn compile_hybrid_token_generation_metal(
    weights: &mut LoadedGgufWeights,
    spec: &HybridDecodeSpec,
) -> Result<CompiledHybridDecodeMetal> {
    compile_hybrid_decode_metal(weights, spec, 1)
}

pub fn compile_hybrid_token_generation_metal_with_shared_state(
    weights: &mut LoadedGgufWeights,
    spec: &HybridDecodeSpec,
    shared_cache: &HybridSharedCacheTensorIds,
    shared_buffers: &MetalContextBuffers,
) -> Result<CompiledHybridDecodeMetal> {
    compile_hybrid_decode_metal_with_shared_state(
        weights,
        spec,
        shared_cache,
        shared_buffers,
        1,
    )
}

pub fn compile_hybrid_token_generation_metal_with_shared_runtime_and_state(
    weights: &mut LoadedGgufWeights,
    spec: &HybridDecodeSpec,
    shared_runtime: &MetalRuntime,
    shared_cache: &HybridSharedCacheTensorIds,
    shared_buffers: &MetalContextBuffers,
) -> Result<CompiledHybridDecodeMetal> {
    compile_hybrid_decode_metal_with_shared_runtime_and_state(
        weights,
        spec,
        shared_runtime,
        shared_cache,
        shared_buffers,
        1,
    )
}

/// Backend-neutral validation of one hybrid-decode step: exactly the checks
/// the Metal executor has always run, in the same order, extracted so every
/// execution backend enforces the identical contract.
pub(crate) fn validate_hybrid_decode_layout(
    ctx: &Context,
    spec: &HybridDecodeSpec,
    decode: &HybridDecodeGraph,
    layout: &HybridDecodeBatchLayout,
) -> Result<()> {
    layout.validate()?;
    let positions = layout.positions.as_slice();
    let cache_tokens = layout.attention_key_count;
    let expected_outputs = ne_usize(
        ctx.tensor(decode.input_output_ids)
            .ok_or_else(|| LlamaError::format("hybrid decode output-id tensor is invalid"))?,
        0,
    )?;
    if layout.output_ids.len() != expected_outputs {
        return Err(LlamaError::format(format!(
            "hybrid decode output-id length mismatch: got {}, expected {}",
            layout.output_ids.len(),
            expected_outputs
        )));
    }
    if decode.input_positions.is_some() {
        if positions.is_empty() {
            return Err(LlamaError::format(
                "hybrid decode requires at least one position",
            ));
        }
        for cache_view in &decode.attention_cache_views {
            if cache_tokens == 0 || cache_tokens > cache_view.max_context {
                return Err(LlamaError::format(format!(
                    "hybrid decode cache_tokens {} is outside 1..={} for attention layer {}",
                    cache_tokens, cache_view.max_context, cache_view.layer_index
                )));
            }
        }
        if positions.iter().copied().any(|pos| {
            pos < 0
                || usize::try_from(pos)
                    .ok()
                    .map(|pos| pos >= cache_tokens)
                    .unwrap_or(true)
        }) {
            return Err(LlamaError::format(format!(
                "hybrid decode positions {:?} exceed cache_tokens {}",
                positions, cache_tokens
            )));
        }
        for cache_view in &decode.attention_cache_views {
            if layout.attention_write_indices.iter().copied().any(|row| {
                row < 0
                    || usize::try_from(row)
                        .ok()
                        .map(|row| row >= cache_view.max_context)
                        .unwrap_or(true)
            }) {
                return Err(LlamaError::format(format!(
                    "hybrid decode attention write indices {:?} exceed max_context {} for attention layer {}",
                    layout.attention_write_indices, cache_view.max_context, cache_view.layer_index
                )));
            }
        }
    } else if !positions.is_empty() && std::env::var("MAKEPAD_LLAMA_MAX_BLOCKS").is_err() {
        // Positions with no attention layers is normally a caller bug, but a
        // block-truncated debug graph (MAKEPAD_LLAMA_MAX_BLOCKS) may cut off
        // every attention layer on purpose.
        return Err(LlamaError::format(
            "hybrid decode received positions for a graph without attention layers",
        ));
    }
    if let Some(input_state_rows) = decode.input_recurrent_state_rows {
        let expected = ne_usize(
            ctx.tensor(input_state_rows).ok_or_else(|| {
                LlamaError::format("hybrid recurrent state row tensor is invalid")
            })?,
            0,
        )?;
        if layout.recurrent_state_rows.len() != expected {
            return Err(LlamaError::format(format!(
                "hybrid decode recurrent state row length mismatch: got {}, expected {}",
                layout.recurrent_state_rows.len(),
                expected
            )));
        }
        let recurrent_max_sequences = spec
            .layers
            .iter()
            .filter_map(|layer| match layer {
                HybridLayerSpec::Attention { .. } => None,
                HybridLayerSpec::Recurrent { decode, .. } => Some(decode.cache.max_sequences),
            })
            .max()
            .unwrap_or(0) as usize;
        if layout.recurrent_state_rows.iter().copied().any(|row| {
            row < 0
                || usize::try_from(row)
                    .ok()
                    .map(|row| row >= recurrent_max_sequences)
                    .unwrap_or(true)
        }) {
            return Err(LlamaError::format(format!(
                "hybrid decode recurrent state rows {:?} exceed max_sequences {}",
                layout.recurrent_state_rows, recurrent_max_sequences
            )));
        }
    } else if !layout.recurrent_state_rows.is_empty() {
        return Err(LlamaError::format(
            "hybrid decode received recurrent state rows for a graph without recurrent layers",
        ));
    }
    Ok(())
}

/// Backend-neutral construction of every per-step input write (token ids or
/// embeddings, output ids, KV write indices, rope positions, recurrent state
/// rows, per-layer attention masks) as owned byte buffers keyed by tensor.
/// This byte layout IS the execution contract; the Metal oracle consumes it
/// directly and other backends upload the identical bytes.
pub(crate) fn build_hybrid_decode_writes(
    ctx: &Context,
    spec: &HybridDecodeSpec,
    decode: &HybridDecodeGraph,
    input: LogitsProbeInput<'_>,
    layout: &HybridDecodeBatchLayout,
) -> Result<Vec<(TensorId, Vec<u8>)>> {
    let positions = layout.positions.as_slice();
    let cache_tokens = layout.attention_key_count;
    let input_primary = match (&spec.input, input) {
        (ProbeInputKind::TokenIds { .. }, LogitsProbeInput::TokenIds(token_ids)) => {
            if decode.input_positions.is_some() && token_ids.len() != positions.len() {
                return Err(LlamaError::format(format!(
                    "hybrid decode token/position length mismatch: {} vs {}",
                    token_ids.len(),
                    positions.len()
                )));
            }
            i32_slice_as_bytes(token_ids).to_vec()
        }
        (
            ProbeInputKind::Embeddings {
                hidden_size,
                input_type,
            },
            LogitsProbeInput::EmbeddingsF32 { data, n_tokens },
        ) => {
            if *input_type != TensorType::F32 {
                return Err(LlamaError::unsupported(format!(
                    "hybrid decode currently expects F32 embeddings, got {}",
                    input_type.name()
                )));
            }
            if decode.input_positions.is_some() && n_tokens != positions.len() {
                return Err(LlamaError::format(format!(
                    "hybrid decode embedding/position length mismatch: {} vs {}",
                    n_tokens,
                    positions.len()
                )));
            }
            let expected = (*hidden_size as usize)
                .checked_mul(n_tokens)
                .ok_or_else(|| {
                    LlamaError::format("overflow computing hybrid decode embedding size")
                })?;
            if data.len() != expected {
                return Err(LlamaError::format(format!(
                    "hybrid decode embedding input length mismatch: got {}, expected {}",
                    data.len(),
                    expected
                )));
            }
            f32_slice_as_bytes(data).to_vec()
        }
        (ProbeInputKind::TokenIds { .. }, LogitsProbeInput::EmbeddingsF32 { .. }) => {
            return Err(LlamaError::format(
                "hybrid decode spec expects token ids but embeddings were provided",
            ));
        }
        (ProbeInputKind::Embeddings { .. }, LogitsProbeInput::TokenIds(_)) => {
            return Err(LlamaError::format(
                "hybrid decode spec expects embeddings but token ids were provided",
            ));
        }
    };

    // The graph's key width only needs to COVER the cache: masks are written
    // at full graph width, so keys beyond cache_tokens are -inf'd exactly.
    // (Views are never reconfigured — see attention_mask_write_key_count.)
    for cache_view in &decode.attention_cache_views {
        if cache_view.graph_key_count < cache_tokens {
            return Err(LlamaError::format(format!(
                "hybrid decode graph key_count {} is smaller than cache_tokens {} for attention layer {}",
                cache_view.graph_key_count, cache_tokens, cache_view.layer_index
            )));
        }
    }

    let mut writes: Vec<(TensorId, Vec<u8>)> = Vec::new();
    if let Some(input_per_layer_primary) = decode.input_per_layer_primary {
        writes.push((input_per_layer_primary, input_primary.clone()));
    }
    writes.insert(0, (decode.input_primary, input_primary));
    writes.push((
        decode.input_output_ids,
        i32_slice_as_bytes(&layout.output_ids).to_vec(),
    ));
    if let Some(input_attention_write_indices) = decode.input_attention_write_indices {
        writes.push((
            input_attention_write_indices,
            i32_slice_as_bytes(&layout.attention_write_indices).to_vec(),
        ));
    }
    let hybrid_rope_positions = if decode.input_rope_positions.is_some() {
        let rope = spec
            .layers
            .iter()
            .find_map(|layer| match layer {
                HybridLayerSpec::Attention { decode, .. } => decode.block.rope.as_ref(),
                HybridLayerSpec::Recurrent { .. } => None,
            })
            .ok_or_else(|| {
                LlamaError::format(
                    "hybrid decode has rope position input without an attention rope spec",
                )
            })?;
        // Image spans carry pre-expanded 4-component M-RoPE positions on the
        // layout; text batches fall back to broadcasting the linear positions.
        let rope_source = layout.rope_positions.as_deref().unwrap_or(positions);
        Some(encode_rope_positions(rope, rope_source, positions.len())?)
    } else {
        None
    };
    if let Some(input_rope_positions) = decode.input_rope_positions {
        writes.push((
            input_rope_positions,
            i32_slice_as_bytes(hybrid_rope_positions.as_deref().ok_or_else(|| {
                LlamaError::format("hybrid decode rope positions were not prepared")
            })?)
            .to_vec(),
        ));
    }
    if let Some(input_recurrent_state_rows) = decode.input_recurrent_state_rows {
        writes.push((
            input_recurrent_state_rows,
            i32_slice_as_bytes(&layout.recurrent_state_rows).to_vec(),
        ));
    }
    if let Some(input_hidden_read_rows) = decode.input_hidden_read_rows {
        if layout.hidden_read_rows.len() != positions.len() {
            return Err(LlamaError::format(format!(
                "hybrid decode layout needs {} hidden read rows, got {}",
                positions.len(),
                layout.hidden_read_rows.len()
            )));
        }
        writes.push((
            input_hidden_read_rows,
            i32_slice_as_bytes(&layout.hidden_read_rows).to_vec(),
        ));
    }
    if let Some(input_hidden_write_rows) = decode.input_hidden_write_rows {
        if layout.hidden_write_rows.len() != positions.len() {
            return Err(LlamaError::format(format!(
                "hybrid decode layout needs {} hidden write rows, got {}",
                positions.len(),
                layout.hidden_write_rows.len()
            )));
        }
        writes.push((
            input_hidden_write_rows,
            i32_slice_as_bytes(&layout.hidden_write_rows).to_vec(),
        ));
    }
    let mut written_masks = BTreeSet::new();
    for cache_view in &decode.attention_cache_views {
        if let Some(input_mask) = cache_view.input_mask {
            if !written_masks.insert(input_mask) {
                continue;
            }
            let key_count = attention_mask_write_key_count(ctx, input_mask)?;
            // The mask is indexed by ABSOLUTE cache row, so its upper bound is
            // the row each token writes, not the token's within-slot position.
            // For a single sequence the two are equal (`attention_write_indices`
            // is `positions` verbatim), so this is byte-identical to keying off
            // `positions`; for a unified multi-slot arena the write index is
            // `kv_base + position` and only the write index is correct.
            let bytes = position_attention_mask_bytes_for_tensor(
                ctx,
                input_mask,
                key_count,
                &layout.attention_write_indices,
                layout.attention_key_lower_bounds(),
                cache_view.causal_window,
            )?;
            writes.push((input_mask, bytes));
        }
    }
    Ok(writes)
}

/// Backend-neutral readback shaping: turn per-tensor output bytes into a
/// `HybridDecodeRun` (logits always; hidden/experts per output config).
pub(crate) fn collect_hybrid_decode_run(
    ctx: &Context,
    decode: &HybridDecodeGraph,
    output_config: HybridDecodeOutputConfig,
    outputs: &BTreeMap<TensorId, Vec<u8>>,
) -> Result<HybridDecodeRun> {
    let logits = outputs
        .get(&decode.result_logits)
        .ok_or_else(|| LlamaError::format("hybrid decode did not produce logits bytes"))?;
    let logits = f32_bytes_to_vec(logits)?;
    let logits_tensor = ctx
        .tensor(decode.result_logits)
        .ok_or_else(|| LlamaError::format("hybrid decode result_logits tensor is invalid"))?;
    let vocab_size = ne_usize(logits_tensor, 0)?;
    let n_tokens = logits
        .len()
        .checked_div(vocab_size.max(1))
        .ok_or_else(|| LlamaError::format("invalid hybrid decode logits shape"))?;

    let (hidden, hidden_size) = if output_config.capture_hidden {
        let hidden = outputs
            .get(&decode.result_hidden)
            .ok_or_else(|| LlamaError::format("hybrid decode did not produce hidden bytes"))?;
        let hidden = f32_bytes_to_vec(hidden)?;
        let hidden_tensor = ctx
            .tensor(decode.result_hidden)
            .ok_or_else(|| LlamaError::format("hybrid decode result_hidden tensor is invalid"))?;
        (hidden, ne_usize(hidden_tensor, 0)?)
    } else {
        (Vec::new(), 0)
    };

    let selected_experts = if output_config.capture_selected_experts {
        let mut selected_experts = Vec::with_capacity(decode.moe_selected_experts.len());
        for selection in &decode.moe_selected_experts {
            let bytes = outputs.get(&selection.selected_experts).ok_or_else(|| {
                LlamaError::format(format!(
                    "hybrid decode did not produce selected experts for layer {}",
                    selection.layer_index
                ))
            })?;
            let experts = bytes
                .chunks_exact(std::mem::size_of::<i32>())
                .map(|chunk| i32::from_le_bytes(chunk.try_into().unwrap()))
                .collect::<Vec<_>>();
            selected_experts.push((selection.layer_index, experts));
        }
        selected_experts
    } else {
        Vec::new()
    };

    Ok(HybridDecodeRun {
        hidden,
        logits,
        n_tokens,
        hidden_size,
        vocab_size,
        selected_experts,
    })
}

/// Print compact, deterministic stats for graph intermediates selected by
/// `MAKEPAD_LLAMA_TRACE_ALL`. This intentionally lives above either backend
/// so a Metal/CUDA trace compares identical tensor ids, names, and bytes.
pub(crate) fn debug_trace_outputs(
    backend: &str,
    ctx: &Context,
    decode: &HybridDecodeGraph,
    outputs: &BTreeMap<TensorId, Vec<u8>>,
) -> Result<()> {
    if decode.debug_outputs.is_empty() {
        return Ok(());
    }
    eprintln!(
        "trace.begin backend={} nodes={}",
        backend,
        decode.debug_outputs.len()
    );
    for (index, &tensor_id) in decode.debug_outputs.iter().enumerate() {
        let tensor = ctx
            .tensor(tensor_id)
            .ok_or_else(|| LlamaError::format(format!("invalid trace tensor {tensor_id}")))?;
        let bytes = outputs.get(&tensor_id).ok_or_else(|| {
            LlamaError::format(format!("trace tensor {tensor_id} was not read back"))
        })?;
        let values = match tensor.desc.ty {
            TensorType::F32 => f32_bytes_to_vec(bytes)?,
            TensorType::F16 => bytes
                .chunks_exact(2)
                .map(|chunk| {
                    makepad_ai_cuda::quant::f16_to_f32(u16::from_le_bytes(
                        chunk.try_into().unwrap(),
                    ))
                })
                .collect(),
            _ => {
                eprintln!(
                    "trace.node backend={} index={} id={} op={:?} ty={} name={} bytes={}",
                    backend,
                    index,
                    tensor_id,
                    tensor.op,
                    tensor.desc.ty.name(),
                    tensor.name().unwrap_or("<unnamed>"),
                    bytes.len()
                );
                continue;
            }
        };
        let mut sum = 0.0f64;
        let mut abs_sum = 0.0f64;
        let mut l2 = 0.0f64;
        let mut max_abs = 0.0f32;
        let mut nans = 0usize;
        for &value in &values {
            if value.is_nan() {
                nans += 1;
                continue;
            }
            let value64 = f64::from(value);
            sum += value64;
            abs_sum += value64.abs();
            l2 += value64 * value64;
            max_abs = max_abs.max(value.abs());
        }
        eprintln!(
            "trace.node backend={} index={} id={} op={:?} name={} n={} sum={:.9e} abs={:.9e} l2={:.9e} max={:.9e} nans={} head={:?}",
            backend,
            index,
            tensor_id,
            tensor.op,
            tensor.name().unwrap_or("<unnamed>"),
            values.len(),
            sum,
            abs_sum,
            l2.sqrt(),
            max_abs,
            nans,
            &values[..values.len().min(4)]
        );
    }
    eprintln!("trace.end backend={backend}");
    Ok(())
}

pub fn execute_prepared_hybrid_decode_metal(
    runtime: &MetalRuntime,
    ctx: &mut Context,
    spec: &HybridDecodeSpec,
    decode: &HybridDecodeGraph,
    compiled: &MetalCompiledGraph,
    input: LogitsProbeInput<'_>,
    layout: &HybridDecodeBatchLayout,
    output_config: HybridDecodeOutputConfig,
) -> Result<HybridDecodeRun> {
    validate_hybrid_decode_layout(ctx, spec, decode, layout)?;
    let owned_writes = build_hybrid_decode_writes(ctx, spec, decode, input, layout)?;
    let writes = owned_writes
        .iter()
        .map(|(tensor_id, bytes)| MetalGraphTensorWrite {
            tensor_id: *tensor_id,
            bytes,
        })
        .collect::<Vec<_>>();

    let mut outputs = vec![decode.result_logits];
    if output_config.capture_hidden {
        outputs.push(decode.result_hidden);
    }
    if output_config.capture_selected_experts {
        outputs.extend(
            decode
                .moe_selected_experts
                .iter()
                .map(|sel| sel.selected_experts),
        );
    }
    outputs.extend(decode.debug_outputs.iter().copied());
    let execution = execute_compiled_graph(runtime, ctx, compiled, &writes, &outputs)
        .map_err(LlamaError::format)?;

    debug_trace_outputs("metal", ctx, decode, &execution.outputs)?;

    collect_hybrid_decode_run(ctx, decode, output_config, &execution.outputs)
}

pub fn execute_hybrid_decode_graph_metal(
    weights: &mut LoadedGgufWeights,
    spec: &HybridDecodeSpec,
    n_tokens: usize,
    input: LogitsProbeInput<'_>,
    positions: &[i32],
    cache_tokens: usize,
) -> Result<HybridDecodeRun> {
    let mut compiled = compile_hybrid_decode_metal(weights, spec, n_tokens)?;
    compiled.execute(input, positions, cache_tokens)
}

pub fn execute_hybrid_decode_graph_metal_cached(
    compiled: &mut CompiledHybridDecodeMetal,
    input: LogitsProbeInput<'_>,
    positions: &[i32],
    cache_tokens: usize,
) -> Result<HybridDecodeRun> {
    compiled.execute(input, positions, cache_tokens)
}

pub fn execute_hybrid_decode_graph_metal_cached_logits_only(
    compiled: &mut CompiledHybridDecodeMetal,
    input: LogitsProbeInput<'_>,
    positions: &[i32],
    cache_tokens: usize,
) -> Result<HybridDecodeRun> {
    compiled.execute_logits_only(input, positions, cache_tokens)
}

pub fn execute_logits_probe_metal(
    weights: &LoadedGgufWeights,
    spec: &LogitsProbeSpec,
    input: LogitsProbeInput<'_>,
    output_ids: &[i32],
) -> Result<LogitsProbeRun> {
    if output_ids.is_empty() {
        return Err(LlamaError::format(
            "logits probe requires at least one output id",
        ));
    }

    let input_embed = match (&spec.input, input) {
        (
            ProbeInputKind::TokenIds {
                token_embedding_name,
                token_embedding_scale,
            },
            LogitsProbeInput::TokenIds(token_ids),
        ) => {
            let token_embd_id = weights.require_tensor_id(token_embedding_name)?;
            let token_embd = require_tensor(&weights.ctx, token_embd_id)?;
            let hidden_size = ne_usize(token_embd, 0)?;
            let vocab_size = ne_usize(token_embd, 1)?;
            get_rows_ggml_bytes_cpu(
                weights
                    .ctx
                    .tensor_data(token_embd_id)
                    .map_err(LlamaError::format)?,
                token_embd.desc.ty.ggml_type(),
                hidden_size,
                vocab_size,
                token_ids,
            )
            .ok_or_else(|| {
                LlamaError::unsupported(format!(
                    "CPU get_rows is unavailable or unsupported for {}",
                    token_embd.desc.ty.name()
                ))
            })
            .map(|mut input_embed| {
                apply_optional_input_scale_f32(&mut input_embed, *token_embedding_scale);
                input_embed
            })?
        }
        (
            ProbeInputKind::Embeddings {
                hidden_size,
                input_type,
            },
            LogitsProbeInput::EmbeddingsF32 { data, n_tokens },
        ) => {
            if *input_type != TensorType::F32 {
                return Err(LlamaError::unsupported(format!(
                    "eager Metal probe currently expects F32 embeddings, got {}",
                    input_type.name()
                )));
            }
            let expected = (*hidden_size as usize)
                .checked_mul(n_tokens)
                .ok_or_else(|| LlamaError::format("overflow computing embedding input size"))?;
            if data.len() != expected {
                return Err(LlamaError::format(format!(
                    "embedding input length mismatch: got {}, expected {}",
                    data.len(),
                    expected
                )));
            }
            data.to_vec()
        }
        (ProbeInputKind::TokenIds { .. }, LogitsProbeInput::EmbeddingsF32 { .. }) => {
            return Err(LlamaError::format(
                "logits probe spec expects token ids but embeddings were provided",
            ));
        }
        (ProbeInputKind::Embeddings { .. }, LogitsProbeInput::TokenIds(_)) => {
            return Err(LlamaError::format(
                "logits probe spec expects embeddings but token ids were provided",
            ));
        }
    };

    let hidden_size = match &spec.input {
        ProbeInputKind::TokenIds {
            token_embedding_name,
            ..
        } => {
            let token_embd_id = weights.require_tensor_id(token_embedding_name)?;
            let token_embd = require_tensor(&weights.ctx, token_embd_id)?;
            ne_usize(token_embd, 0)?
        }
        ProbeInputKind::Embeddings { hidden_size, .. } => *hidden_size as usize,
    };
    let n_tokens = input_embed
        .len()
        .checked_div(hidden_size)
        .ok_or_else(|| LlamaError::format("invalid hidden size for probe input"))?;

    for &row in output_ids {
        let row_ok = usize::try_from(row).ok().is_some_and(|row| row < n_tokens);
        if !row_ok {
            return Err(LlamaError::format(format!(
                "probe output row {} is out of range {}",
                row, n_tokens
            )));
        }
    }

    let selected = get_rows_ggml_bytes_cpu(
        f32_slice_as_bytes(&input_embed),
        TensorType::F32.ggml_type(),
        hidden_size,
        n_tokens,
        output_ids,
    )
    .ok_or_else(|| LlamaError::unsupported("CPU F32 get_rows is unavailable".to_string()))?;

    let output_norm_id = require_tensor_id(tensor_ids(weights), &spec.output_norm_name)?;
    let output_norm = read_tensor_as_f32(weights, output_norm_id)?;
    let normed = try_rms_norm_mul_f32(
        &selected,
        &[output_ids.len(), hidden_size],
        &output_norm,
        &[hidden_size],
        spec.rms_epsilon,
    )
    .ok_or_else(|| LlamaError::unsupported("Metal rms_norm_mul is unavailable".to_string()))?;

    let output_id = require_tensor_id(tensor_ids(weights), &spec.output_name)?;
    let output = require_tensor(&weights.ctx, output_id)?;
    let vocab_size = ne_usize(output, 1)?;
    let mut logits = try_matmul_nt_ggml_bytes(
        &normed,
        weights
            .ctx
            .tensor_data(output_id)
            .map_err(LlamaError::format)?,
        output.desc.ty.ggml_type(),
        output_ids.len(),
        hidden_size,
        vocab_size,
    )
    .ok_or_else(|| {
        LlamaError::unsupported(format!(
            "Metal quantized matmul is unavailable or unsupported for {}",
            output.desc.ty.name()
        ))
    })?;
    apply_optional_logit_softcap_f32(&mut logits, spec.final_logit_softcap);

    Ok(LogitsProbeRun {
        logits,
        n_outputs: output_ids.len(),
        vocab_size,
    })
}

fn build_dense_gated_ffn(
    ctx: &mut Context,
    tensor_ids: &BTreeMap<String, TensorId>,
    input: TensorId,
    spec: &DenseGatedFfnSpec,
    prefix: &str,
) -> Result<TensorId> {
    let gate_weight = require_tensor_id(tensor_ids, &spec.gate_proj_name)?;
    let mut gate = ctx
        .mul_mat(gate_weight, input, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(gate, &format!("{prefix}.gate"))
        .map_err(LlamaError::format)?;
    if let Some(scale_name) = &spec.gate_proj_scale_name {
        let scale = require_tensor_id(tensor_ids, scale_name)?;
        gate = ctx
            .binary_like_a(Op::Mul, gate, scale, BufferUsage::Activations)
            .map_err(LlamaError::format)?;
        ctx.set_tensor_name(gate, &format!("{prefix}.gate_scaled"))
            .map_err(LlamaError::format)?;
    }

    let up_weight = require_tensor_id(tensor_ids, &spec.up_proj_name)?;
    let mut up = ctx
        .mul_mat(up_weight, input, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(up, &format!("{prefix}.up"))
        .map_err(LlamaError::format)?;
    if let Some(scale_name) = &spec.up_proj_scale_name {
        let scale = require_tensor_id(tensor_ids, scale_name)?;
        up = ctx
            .binary_like_a(Op::Mul, up, scale, BufferUsage::Activations)
            .map_err(LlamaError::format)?;
        ctx.set_tensor_name(up, &format!("{prefix}.up_scaled"))
            .map_err(LlamaError::format)?;
    }

    let hidden = build_split_gated_hidden(ctx, gate, up, spec.gate_activation, prefix)?;

    let down_weight = require_tensor_id(tensor_ids, &spec.down_proj_name)?;
    let mut output = ctx
        .mul_mat(down_weight, hidden, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(output, &format!("{prefix}.output"))
        .map_err(LlamaError::format)?;
    if let Some(scale_name) = &spec.down_proj_scale_name {
        let scale = require_tensor_id(tensor_ids, scale_name)?;
        output = ctx
            .binary_like_a(Op::Mul, output, scale, BufferUsage::Activations)
            .map_err(LlamaError::format)?;
        ctx.set_tensor_name(output, &format!("{prefix}.output_scaled"))
            .map_err(LlamaError::format)?;
    }

    Ok(output)
}

fn build_split_gated_hidden(
    ctx: &mut Context,
    gate: TensorId,
    up: TensorId,
    activation: UnaryOp,
    prefix: &str,
) -> Result<TensorId> {
    let hidden = match activation {
        UnaryOp::Silu => ctx
            .glu_split(gate, up, GluOp::Swiglu, BufferUsage::Activations)
            .map_err(LlamaError::format)?,
        UnaryOp::Gelu => ctx
            .glu_split(gate, up, GluOp::Geglu, BufferUsage::Activations)
            .map_err(LlamaError::format)?,
        UnaryOp::GeluErf => ctx
            .glu_split(gate, up, GluOp::GegluErf, BufferUsage::Activations)
            .map_err(LlamaError::format)?,
        UnaryOp::GeluQuick => ctx
            .glu_split(gate, up, GluOp::GegluQuick, BufferUsage::Activations)
            .map_err(LlamaError::format)?,
        UnaryOp::Relu => ctx
            .glu_split(gate, up, GluOp::Reglu, BufferUsage::Activations)
            .map_err(LlamaError::format)?,
        _ => {
            let gate = ctx
                .unary(gate, activation, BufferUsage::Activations)
                .map_err(LlamaError::format)?;
            ctx.set_tensor_name(gate, &format!("{prefix}.gate_act"))
                .map_err(LlamaError::format)?;
            ctx.binary_like_a(Op::Mul, gate, up, BufferUsage::Activations)
                .map_err(LlamaError::format)?
        }
    };
    ctx.set_tensor_name(hidden, &format!("{prefix}.hidden"))
        .map_err(LlamaError::format)?;
    Ok(hidden)
}

fn apply_optional_proj_scale(
    ctx: &mut Context,
    tensor_ids: &BTreeMap<String, TensorId>,
    tensor: TensorId,
    scale_name: &Option<String>,
    tensor_name: &str,
) -> Result<TensorId> {
    let Some(scale_name) = scale_name.as_ref() else {
        return Ok(tensor);
    };
    let scale = require_tensor_id(tensor_ids, scale_name)?;
    let scaled = ctx
        .binary_like_a(Op::Mul, tensor, scale, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(scaled, tensor_name)
        .map_err(LlamaError::format)?;
    Ok(scaled)
}

fn apply_optional_input_scale(
    ctx: &mut Context,
    tensor: TensorId,
    scale: Option<f32>,
    tensor_name: &str,
) -> Result<TensorId> {
    let Some(scale) = scale else {
        return Ok(tensor);
    };
    let scaled = ctx
        .scale(tensor, scale, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(scaled, tensor_name)
        .map_err(LlamaError::format)?;
    Ok(scaled)
}

fn apply_optional_input_scale_f32(data: &mut [f32], scale: Option<f32>) {
    let Some(scale) = scale else {
        return;
    };
    for value in data {
        *value *= scale;
    }
}

fn apply_optional_logit_softcap_f32(data: &mut [f32], softcap: Option<f32>) {
    let Some(softcap) = softcap else {
        return;
    };
    if softcap <= 0.0 {
        return;
    }
    for value in data {
        *value = (*value / softcap).tanh() * softcap;
    }
}

fn build_rms_norm(
    ctx: &mut Context,
    src: TensorId,
    epsilon: f32,
    tensor_name: &str,
) -> Result<TensorId> {
    let norm = ctx
        .rms_norm_eps(src, epsilon, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(norm, tensor_name)
        .map_err(LlamaError::format)?;
    Ok(norm)
}

fn build_rms_norm_mul(
    ctx: &mut Context,
    tensor_ids: &BTreeMap<String, TensorId>,
    src: TensorId,
    epsilon: f32,
    weight_name: &str,
    tensor_name: &str,
) -> Result<TensorId> {
    let norm = ctx
        .rms_norm_eps(src, epsilon, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    let weight = require_tensor_id(tensor_ids, weight_name)?;
    let scaled = ctx
        .binary_like_a(Op::Mul, norm, weight, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(scaled, tensor_name)
        .map_err(LlamaError::format)?;
    Ok(scaled)
}




/// Masks are ALWAYS written at the graph's full key width: the causal fill
/// puts -inf on every key beyond a query's position, which exactly covers
/// the unwritten cache tail when graph_key_count > cache_tokens. Never
/// shrink the mask or reconfigure the cache views instead — the flash op
/// reads PERMUTE nodes whose dims were baked at graph build, so a view
/// reconfigure never reaches the kernel and the width mismatch corrupts
/// attention (kernel iterates graph_key_count keys against a narrower mask).
fn attention_mask_write_key_count(ctx: &Context, tensor_id: TensorId) -> Result<usize> {
    let tensor = require_tensor(ctx, tensor_id)?;
    ne_usize(tensor, 0)
}



fn row_size(ty: TensorType, ne: i64) -> Result<usize> {
    ggml_row_size_for_type(ty, ne).map_err(LlamaError::format)
}

fn bytes_for_elements(ty: TensorType, elements: u64) -> Result<usize> {
    let scalar_size = ty.scalar_size_bytes().ok_or_else(|| {
        LlamaError::unsupported(format!(
            "cache layout requires scalar tensor types, got {}",
            ty.name()
        ))
    })?;
    usize::try_from(elements)
        .ok()
        .and_then(|elements| elements.checked_mul(scalar_size))
        .ok_or_else(|| {
            LlamaError::format(format!(
                "overflow computing byte size for {} elements of {}",
                elements,
                ty.name()
            ))
        })
}

fn mark_input(ctx: &mut Context, id: TensorId) -> Result<()> {
    ctx.tensor_mut(id)
        .ok_or_else(|| LlamaError::format(format!("invalid input tensor {}", id)))?
        .set_input();
    Ok(())
}

fn mark_output(ctx: &mut Context, id: TensorId) -> Result<()> {
    ctx.tensor_mut(id)
        .ok_or_else(|| LlamaError::format(format!("invalid output tensor {}", id)))?
        .set_output();
    Ok(())
}

fn require_tensor_id(tensor_ids: &BTreeMap<String, TensorId>, name: &str) -> Result<TensorId> {
    tensor_ids
        .get(name)
        .copied()
        .ok_or_else(|| LlamaError::format(format!("missing resident tensor '{}'", name)))
}

fn tensor_ids(weights: &LoadedGgufWeights) -> &BTreeMap<String, TensorId> {
    &weights.tensor_ids
}

fn require_tensor(ctx: &Context, id: TensorId) -> Result<&Tensor> {
    ctx.tensor(id)
        .ok_or_else(|| LlamaError::format(format!("invalid tensor id {}", id)))
}

fn cast_tensor_to_type(
    ctx: &mut Context,
    src: TensorId,
    ty: TensorType,
    usage: BufferUsage,
) -> Result<TensorId> {
    let tensor = require_tensor(ctx, src)?.clone();
    if tensor.desc.ty == ty {
        return Ok(src);
    }

    let cast = ctx
        .new_tensor(
            ty,
            tensor.desc.layout.rank(),
            tensor.desc.layout.extents(),
            usage,
        )
        .map_err(LlamaError::format)?;
    ctx.cpy(src, cast, usage).map_err(LlamaError::format)
}

fn view_dim2_slice_2d(ctx: &mut Context, tensor_id: TensorId, index: i64) -> Result<TensorId> {
    let tensor = require_tensor(ctx, tensor_id)?.clone();
    let index = usize::try_from(index)
        .map_err(|_| LlamaError::format(format!("slice index {} does not fit in usize", index)))?;
    let offset = tensor.nb[2]
        .checked_mul(index)
        .ok_or_else(|| LlamaError::format("slice offset overflow for dim2 view"))?;
    ctx.view_4d(
        tensor_id,
        tensor.ne[0],
        tensor.ne[1],
        1,
        tensor.ne[3],
        tensor.nb[1],
        tensor.nb[2],
        tensor.nb[3],
        offset,
    )
    .map_err(LlamaError::format)
}

fn ne_usize(tensor: &Tensor, dim: usize) -> Result<usize> {
    usize::try_from(tensor.ne[dim]).map_err(|_| {
        LlamaError::format(format!(
            "tensor '{}' dimension {} does not fit in usize",
            tensor.name().unwrap_or("<unnamed>"),
            tensor.ne[dim]
        ))
    })
}

fn read_tensor_as_f32(weights: &LoadedGgufWeights, id: TensorId) -> Result<Vec<f32>> {
    let tensor = require_tensor(&weights.ctx, id)?;
    let bytes = weights.ctx.tensor_data(id).map_err(LlamaError::format)?;
    match tensor.desc.ty {
        TensorType::F32 => bytes
            .chunks_exact(4)
            .map(|chunk| Ok(f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]])))
            .collect(),
        TensorType::F16 => bytes
            .chunks_exact(2)
            .map(|chunk| Ok(f16_to_f32(u16::from_le_bytes([chunk[0], chunk[1]]))))
            .collect(),
        TensorType::BF16 => bytes
            .chunks_exact(2)
            .map(|chunk| {
                let bits = u32::from(u16::from_le_bytes([chunk[0], chunk[1]])) << 16;
                Ok(f32::from_bits(bits))
            })
            .collect(),
        other => Err(LlamaError::unsupported(format!(
            "expected scalar tensor for eager probe, got {}",
            other.name()
        ))),
    }
}

fn f32_slice_as_bytes(slice: &[f32]) -> &[u8] {
    unsafe { std::slice::from_raw_parts(slice.as_ptr() as *const u8, std::mem::size_of_val(slice)) }
}

fn i32_slice_as_bytes(slice: &[i32]) -> &[u8] {
    unsafe { std::slice::from_raw_parts(slice.as_ptr() as *const u8, std::mem::size_of_val(slice)) }
}

fn f32_bytes_to_vec(bytes: &[u8]) -> Result<Vec<f32>> {
    if bytes.len() % std::mem::size_of::<f32>() != 0 {
        return Err(LlamaError::format(format!(
            "f32 output byte length {} is not divisible by {}",
            bytes.len(),
            std::mem::size_of::<f32>()
        )));
    }

    bytes
        .chunks_exact(4)
        .map(|chunk| Ok(f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]])))
        .collect()
}

fn tensor_bytes_to_f32_vec(bytes: &[u8], ty: TensorType) -> Result<Vec<f32>> {
    match ty {
        TensorType::F32 => f32_bytes_to_vec(bytes),
        TensorType::F16 => {
            if bytes.len() % std::mem::size_of::<u16>() != 0 {
                return Err(LlamaError::format(format!(
                    "f16 output byte length {} is not divisible by {}",
                    bytes.len(),
                    std::mem::size_of::<u16>()
                )));
            }
            bytes
                .chunks_exact(2)
                .map(|chunk| Ok(f16_to_f32(u16::from_le_bytes([chunk[0], chunk[1]]))))
                .collect()
        }
        TensorType::BF16 => {
            if bytes.len() % std::mem::size_of::<u16>() != 0 {
                return Err(LlamaError::format(format!(
                    "bf16 output byte length {} is not divisible by {}",
                    bytes.len(),
                    std::mem::size_of::<u16>()
                )));
            }
            bytes
                .chunks_exact(2)
                .map(|chunk| {
                    Ok(f32::from_bits(
                        u32::from(u16::from_le_bytes([chunk[0], chunk[1]])) << 16,
                    ))
                })
                .collect()
        }
        other => Err(LlamaError::unsupported(format!(
            "cannot decode tensor output as f32 from {}",
            other.name()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::InitParams;

    /// The single-sequence mask exactly as it was built before slot lower
    /// bounds existed. Kept verbatim as an independent reference so the
    /// "unslotted output is unchanged" gate cannot drift with the code it
    /// guards.
    fn reference_single_sequence_mask_f32(
        key_count: usize,
        positions: &[i32],
        causal_window: Option<usize>,
    ) -> Vec<u8> {
        let mut bytes = Vec::new();
        for &position in positions {
            let position = usize::try_from(position).expect("non-negative position");
            let key_start = causal_window_key_start(position, causal_window);
            for key in 0..key_count {
                let value = if key > position || key < key_start {
                    f32::NEG_INFINITY
                } else {
                    0.0
                };
                bytes.extend_from_slice(&value.to_le_bytes());
            }
        }
        bytes
    }

    /// Absolute key rows token `token` is allowed to attend to, read back out
    /// of an f32 mask blob.
    fn allowed_keys_f32(bytes: &[u8], key_count: usize, token: usize) -> Vec<usize> {
        (0..key_count)
            .filter(|key| {
                let at = (token * key_count + key) * std::mem::size_of::<f32>();
                let value = f32::from_le_bytes(bytes[at..at + 4].try_into().expect("f32 slice"));
                value == 0.0
            })
            .collect()
    }

    #[test]
    fn unslotted_mask_is_byte_identical_to_the_single_sequence_mask() {
        // The solo lane's determinism gate rests on this: with no lower
        // bounds the batched code path must emit the same bytes the
        // single-sequence path always did, for both mask types and with or
        // without a sliding window.
        for causal_window in [None, Some(4), Some(1)] {
            for positions in [
                vec![0i32],
                vec![7i32],
                vec![0i32, 1, 2, 3],
                vec![5i32, 6, 7],
            ] {
                let key_count = 16;
                let f32_bytes = position_causal_mask_f32_bytes_with_window(
                    key_count,
                    &positions,
                    None,
                    causal_window,
                )
                .expect("f32 mask");
                assert_eq!(
                    f32_bytes,
                    reference_single_sequence_mask_f32(key_count, &positions, causal_window),
                    "f32 mask changed for positions {positions:?} window {causal_window:?}"
                );

                // The f16 blob must agree cell for cell with the f32 one.
                let f16_bytes = position_causal_mask_f16_bytes_with_window(
                    key_count,
                    &positions,
                    None,
                    causal_window,
                )
                .expect("f16 mask");
                assert_eq!(f16_bytes.len(), positions.len() * key_count * 2);
                let zero = f32_to_f16(0.0).to_le_bytes();
                for token in 0..positions.len() {
                    let allowed = allowed_keys_f32(&f32_bytes, key_count, token);
                    for key in 0..key_count {
                        let at = (token * key_count + key) * 2;
                        let is_zero = f16_bytes[at..at + 2] == zero;
                        assert_eq!(
                            is_zero,
                            allowed.contains(&key),
                            "f16/f32 disagree at token {token} key {key}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn slot_lower_bounds_make_the_mask_block_diagonal() {
        // Two slots in one flat arena of 8 rows each. Slot 0 is 3 tokens in,
        // slot 1 is 2 tokens in; each decodes its next token.
        const CAP: i32 = 8;
        let upper_rows = vec![0 * CAP + 3, 1 * CAP + 2];
        let lower_rows = vec![0 * CAP, 1 * CAP];
        let key_count = 16;
        let bytes = position_causal_mask_f32_bytes_with_window(
            key_count,
            &upper_rows,
            Some(&lower_rows),
            None,
        )
        .expect("mask");

        // Slot 0 sees its own rows 0..=3 and nothing of slot 1.
        assert_eq!(allowed_keys_f32(&bytes, key_count, 0), vec![0, 1, 2, 3]);
        // Slot 1 sees only its own region, rows 8..=10.
        assert_eq!(allowed_keys_f32(&bytes, key_count, 1), vec![8, 9, 10]);
    }

    #[test]
    fn slot_zero_sees_exactly_what_a_solo_session_sees() {
        // The load-bearing property behind "1 client runs at today's speed
        // with today's output": a slot-0 token in a batched mask has the same
        // allowed set as the same token decoded alone.
        const CAP: i32 = 32;
        let key_count = 64;
        let batched = position_causal_mask_f32_bytes_with_window(
            key_count,
            &[5, CAP + 9],
            Some(&[0, CAP]),
            None,
        )
        .expect("batched mask");
        let solo = position_causal_mask_f32_bytes_with_window(key_count, &[5], None, None)
            .expect("solo mask");
        assert_eq!(
            allowed_keys_f32(&batched, key_count, 0),
            allowed_keys_f32(&solo, key_count, 0)
        );
        // And a zero base really is the same bytes, not merely the same set.
        let zero_based =
            position_causal_mask_f32_bytes_with_window(key_count, &[5], Some(&[0]), None)
                .expect("zero-based mask");
        assert_eq!(zero_based, solo);
    }

    #[test]
    fn the_causal_window_is_measured_inside_the_slot() {
        // A window of 3 on a token at within-slot position 5 must reach back
        // to within-slot 3, i.e. absolute 11 — never across the slot base
        // into another slot's history.
        const CAP: i32 = 8;
        let key_count = 24;
        let bytes = position_causal_mask_f32_bytes_with_window(
            key_count,
            &[CAP + 5],
            Some(&[CAP]),
            Some(3),
        )
        .expect("mask");
        let allowed = allowed_keys_f32(&bytes, key_count, 0);
        assert_eq!(allowed.iter().copied().min(), Some(11));
        assert_eq!(allowed.iter().copied().max(), Some(13));
        assert!(
            allowed.iter().all(|&key| key >= CAP as usize),
            "window reached below the slot base: {allowed:?}"
        );
    }

    #[test]
    fn mask_rejects_bounds_it_cannot_honour() {
        // Lower above upper would silently produce an all -inf row, and a row
        // with no visible key makes attention emit zeros rather than fail.
        assert!(
            position_causal_mask_f32_bytes_with_window(16, &[3], Some(&[9]), None).is_err(),
            "lower bound above the position must be refused"
        );
        assert!(
            position_causal_mask_f32_bytes_with_window(16, &[3, 4], Some(&[0]), None).is_err(),
            "a short lower-bound list must be refused"
        );
        assert!(
            position_causal_mask_f32_bytes_with_window(4, &[9], None, None).is_err(),
            "a position past key_count must be refused"
        );
    }

    #[test]
    fn layout_validates_slot_lower_bounds() {
        let mut layout =
            HybridDecodeBatchLayout::from_contiguous_positions(&[0, 1], 8).expect("layout");
        assert!(layout.attention_key_lower_bounds().is_none());
        layout.validate().expect("unslotted layout is valid");

        layout.attention_key_lower_bounds = vec![0];
        assert!(
            layout.validate().is_err(),
            "one lower bound for two tokens must be refused"
        );

        layout.attention_key_lower_bounds = vec![0, 5];
        assert!(
            layout.validate().is_err(),
            "a lower bound above the token's own row must be refused"
        );

        layout.attention_key_lower_bounds = vec![0, 1];
        layout.validate().expect("in-range lower bounds are valid");
        assert_eq!(layout.attention_key_lower_bounds(), Some(&[0, 1][..]));
    }

    #[test]
    fn hybrid_cache_template_materializes_types_and_shape() {
        let template = HybridCacheTemplate {
            attention_layers: vec![3, 7],
            recurrent_layers: vec![0, 1, 2],
            attention_k_width: 128,
            attention_v_width: 64,
            recurrent_r_width: 4096,
            recurrent_s_width: 8192,
        };

        let spec = template.materialize(
            HybridCacheShape {
                n_ctx_seq: 4096,
                n_seq_max: 8,
            },
            HybridCacheTypes {
                attention_k_type: TensorType::F16,
                attention_v_type: TensorType::F16,
                recurrent_r_type: TensorType::F32,
                recurrent_s_type: TensorType::F32,
            },
        );

        assert_eq!(spec.n_ctx_seq, 4096);
        assert_eq!(spec.n_seq_max, 8);
        assert_eq!(spec.attention_layers, vec![3, 7]);
        assert_eq!(spec.recurrent_layers, vec![0, 1, 2]);
        assert_eq!(spec.attention_k_type, TensorType::F16);
        assert_eq!(spec.recurrent_s_type, TensorType::F32);
    }

    #[test]
    fn delta_net_chunking_matches_autoregressive_reference_on_metal_when_available() {
        let _runtime = match MetalRuntime::new() {
            Ok(runtime) => runtime,
            Err(_) => return,
        };

        let s_v = 32_i64;
        let h_k = 2_i64;
        let h_v = 2_i64;
        let n_tokens = 2_i64;
        let n_seqs = 1_i64;

        let mut ctx = Context::new(InitParams {
            mem_size: 8 << 20,
            mem_buffer: None,
            no_alloc: false,
        });

        let q = ctx
            .new_tensor_4d(
                TensorType::F32,
                s_v,
                h_k,
                n_tokens,
                n_seqs,
                BufferUsage::Activations,
            )
            .unwrap();
        let k = ctx
            .new_tensor_4d(
                TensorType::F32,
                s_v,
                h_k,
                n_tokens,
                n_seqs,
                BufferUsage::Activations,
            )
            .unwrap();
        let v = ctx
            .new_tensor_4d(
                TensorType::F32,
                s_v,
                h_v,
                n_tokens,
                n_seqs,
                BufferUsage::Activations,
            )
            .unwrap();
        let g = ctx
            .new_tensor_4d(
                TensorType::F32,
                1,
                h_v,
                n_tokens,
                n_seqs,
                BufferUsage::Activations,
            )
            .unwrap();
        let beta = ctx
            .new_tensor_4d(
                TensorType::F32,
                1,
                h_v,
                n_tokens,
                n_seqs,
                BufferUsage::Activations,
            )
            .unwrap();
        let state = ctx
            .new_tensor_4d(
                TensorType::F32,
                s_v,
                s_v,
                h_v,
                n_seqs,
                BufferUsage::Activations,
            )
            .unwrap();

        let dummy_block = DeltaNetRecurrentBlockSpec {
            input: ProbeInputKind::Embeddings {
                hidden_size: (s_v * h_v) as u32,
                input_type: TensorType::F32,
            },
            embedding_length: (s_v * h_v) as u32,
            input_norm_name: String::new(),
            merged_input_proj_name: None,
            qkv_proj_name: String::new(),
            qkv_proj_scale_name: None,
            z_proj_name: String::new(),
            z_proj_scale_name: None,
            beta_proj_name: String::new(),
            beta_proj_scale_name: None,
            alpha_proj_name: String::new(),
            alpha_proj_scale_name: None,
            dt_bias_name: String::new(),
            a_name: String::new(),
            conv_kernel_name: String::new(),
            norm_name: String::new(),
            output_proj_name: String::new(),
            output_proj_scale_name: None,
            key_head_dim: s_v as u32,
            key_head_count: h_k as u32,
            value_head_dim: s_v as u32,
            value_head_count: h_v as u32,
            rms_epsilon: 1.0e-6,
            residual: true,
        };

        let (output, new_state) = build_delta_net_chunking(
            &mut ctx,
            &dummy_block,
            q,
            k,
            v,
            g,
            beta,
            state,
            n_tokens,
            n_seqs,
            "test",
        )
        .unwrap();
        let output_cont = ctx.cont_2d(output, s_v * h_v, n_tokens * n_seqs).unwrap();
        let state_cont = ctx.cont_2d(new_state, s_v * s_v, h_v * n_seqs).unwrap();

        let q_values = patterned_f32s((s_v * h_k * n_tokens * n_seqs) as usize, -0.21, 0.009);
        let k_values = patterned_f32s((s_v * h_k * n_tokens * n_seqs) as usize, 0.14, -0.007);
        let v_values = patterned_f32s((s_v * h_v * n_tokens * n_seqs) as usize, 0.05, 0.005);
        let g_values = patterned_f32s((h_v * n_tokens * n_seqs) as usize, -0.03, 0.0005);
        let beta_values = patterned_f32s((h_v * n_tokens * n_seqs) as usize, 0.6, -0.001);
        let state_values = patterned_f32s((s_v * s_v * h_v * n_seqs) as usize, -0.05, 0.0004);

        ctx.write_tensor_data(q, &f32s_to_bytes(&q_values)).unwrap();
        ctx.write_tensor_data(k, &f32s_to_bytes(&k_values)).unwrap();
        ctx.write_tensor_data(v, &f32s_to_bytes(&v_values)).unwrap();
        ctx.write_tensor_data(g, &f32s_to_bytes(&g_values)).unwrap();
        ctx.write_tensor_data(beta, &f32s_to_bytes(&beta_values))
            .unwrap();
        ctx.write_tensor_data(state, &f32s_to_bytes(&state_values))
            .unwrap();

        for name in ["test.dnet_add_ch_lhs", "test.attn_pre_solve"] {
            let tensor_id = ctx.get_tensor(name).unwrap();
            let tensor = ctx.tensor(tensor_id).unwrap();
            assert_eq!(tensor.nb[0], std::mem::size_of::<f32>());
            assert_eq!(
                tensor.nb[1],
                ggml_row_size_for_type(tensor.desc.ty, tensor.ne[0]).unwrap()
            );
            assert_eq!(
                tensor.nb[2],
                tensor.nb[1] * usize::try_from(tensor.ne[1]).unwrap()
            );
            assert_eq!(
                tensor.nb[3],
                tensor.nb[2] * usize::try_from(tensor.ne[2]).unwrap()
            );
        }

        for checkpoint_name in [
            "test.q_in",
            "test.k_in",
            "test.v_in",
            "test.b_in",
            "test.g_in",
            "test.v_b",
            "test.k_b",
            "test.g_cs",
            "test.decay_mask",
            "test.kq",
            "test.attn",
            "test.dnet_add_ch_lhs",
            "test.attn_pre_solve",
        ] {
            let runtime = match MetalRuntime::new() {
                Ok(runtime) => runtime,
                Err(_) => return,
            };
            let checkpoint = checkpoint_root(&mut ctx, checkpoint_name).unwrap();
            let mut graph = Graph::new();
            graph.build_forward_expand(&ctx, checkpoint).unwrap();
            let prepared = prepare_graph(&ctx, &graph, runtime.features()).unwrap();
            let session = MetalGraphSession::from_runtime(
                runtime,
                &ctx,
                &prepared,
                BufferStorageMode::Shared,
                BufferStorageMode::Shared,
            )
            .unwrap();
            let execution = session
                .execute(&ctx, &[], &[checkpoint])
                .unwrap_or_else(|err| {
                    panic!(
                        "delta_net_chunking checkpoint '{}' failed: {}",
                        checkpoint_name, err
                    )
                });
            let values = bytes_to_f32s(execution.outputs.get(&checkpoint).unwrap());
            if values.iter().any(|value| !value.is_finite()) {
                panic!(
                    "delta_net_chunking checkpoint '{}' produced non-finite values",
                    checkpoint_name
                );
            }
        }

        let solve_runtime = match MetalRuntime::new() {
            Ok(runtime) => runtime,
            Err(_) => return,
        };
        let lhs_checkpoint = checkpoint_root(&mut ctx, "test.dnet_add_ch_lhs").unwrap();
        let rhs_checkpoint = checkpoint_root(&mut ctx, "test.attn_pre_solve").unwrap();
        let mut lhs_graph = Graph::new();
        lhs_graph
            .build_forward_expand(&ctx, lhs_checkpoint)
            .unwrap();
        lhs_graph
            .build_forward_expand(&ctx, rhs_checkpoint)
            .unwrap();
        let lhs_runtime = match MetalRuntime::new() {
            Ok(runtime) => runtime,
            Err(_) => return,
        };
        let lhs_prepared = prepare_graph(&ctx, &lhs_graph, lhs_runtime.features()).unwrap();
        let lhs_session = MetalGraphSession::from_runtime(
            lhs_runtime,
            &ctx,
            &lhs_prepared,
            BufferStorageMode::Shared,
            BufferStorageMode::Shared,
        )
        .unwrap();
        let lhs_execution = lhs_session
            .execute(&ctx, &[], &[lhs_checkpoint, rhs_checkpoint])
            .unwrap();
        let lhs_values = bytes_to_f32s(lhs_execution.outputs.get(&lhs_checkpoint).unwrap());
        let rhs_values = bytes_to_f32s(lhs_execution.outputs.get(&rhs_checkpoint).unwrap());
        let mut min_abs_diag = f32::INFINITY;
        let mut max_abs_diag = 0.0f32;
        for batch in 0..usize::try_from(h_v * n_seqs).unwrap() {
            let batch_base = batch * 64 * 64;
            for row in 0..64usize {
                let value = lhs_values[batch_base + row * 64 + row].abs();
                min_abs_diag = min_abs_diag.min(value);
                max_abs_diag = max_abs_diag.max(value);
            }
        }
        if min_abs_diag < 1.0e-6 {
            panic!(
                "delta_net_chunking lhs diagonal is degenerate: min_abs_diag={} max_abs_diag={}",
                min_abs_diag, max_abs_diag
            );
        }
        let cpu_solve =
            cpu_solve_tri_f32(&lhs_values, &rhs_values, 64, 64, (h_v * n_seqs) as usize);
        if cpu_solve.iter().any(|value| !value.is_finite()) {
            panic!("delta_net_chunking cpu replayed solve_tri produced non-finite values");
        }
        let lhs_reshaped = ctx
            .reshape(lhs_checkpoint, &[64, 64, 1, h_v * n_seqs])
            .unwrap();
        let rhs_reshaped = ctx
            .reshape(rhs_checkpoint, &[64, 64, 1, h_v * n_seqs])
            .unwrap();
        let solve_replayed = ctx
            .solve_tri(lhs_reshaped, rhs_reshaped, BufferUsage::Activations)
            .unwrap();
        let solve_replayed_cont = ctx.cont_2d(solve_replayed, 64 * 64, h_v * n_seqs).unwrap();
        let mut solve_graph = Graph::new();
        solve_graph
            .build_forward_expand(&ctx, solve_replayed_cont)
            .unwrap();
        let solve_prepared = prepare_graph(&ctx, &solve_graph, solve_runtime.features()).unwrap();
        let solve_session = MetalGraphSession::from_runtime(
            solve_runtime,
            &ctx,
            &solve_prepared,
            BufferStorageMode::Shared,
            BufferStorageMode::Shared,
        )
        .unwrap();
        let solve_execution = solve_session
            .execute(&ctx, &[], &[solve_replayed_cont])
            .unwrap();
        let solve_values =
            bytes_to_f32s(solve_execution.outputs.get(&solve_replayed_cont).unwrap());
        if solve_values.iter().any(|value| !value.is_finite()) {
            panic!("delta_net_chunking replayed solve_tri produced non-finite values");
        }
        let mut solve_max_abs_diff = 0.0f32;
        for (actual, expected) in solve_values.iter().zip(cpu_solve.iter()) {
            solve_max_abs_diff = solve_max_abs_diff.max((actual - expected).abs());
        }
        if solve_max_abs_diff > 1.0e-4 {
            panic!(
                "delta_net_chunking replayed solve_tri mismatch: max_abs_diff={}",
                solve_max_abs_diff
            );
        }

        let attn_checkpoint = ctx.get_tensor("test.dnet_add_ch_attn_solved").unwrap();
        let v_b_checkpoint = ctx.get_tensor("test.v_b").unwrap();
        let v_b_checkpoint = ctx
            .reshape(v_b_checkpoint, &[s_v, 64, 1, h_v * n_seqs])
            .unwrap();
        let v_b_t = ctx.transpose(v_b_checkpoint).unwrap();
        let v_b_t = ctx.cont(v_b_t).unwrap();
        assert_replayed_mul_mat_matches_cpu(&mut ctx, v_b_t, attn_checkpoint, "v_beta_attn");

        let kbg_checkpoint = ctx.get_tensor("test.k_beta_g_exp").unwrap();
        assert_replayed_mul_mat_matches_cpu(
            &mut ctx,
            kbg_checkpoint,
            attn_checkpoint,
            "k_cumdecay",
        );

        let state_checkpoint = ctx.get_tensor("test.dnet_add_ch_state").unwrap();
        let k_cd_checkpoint = ctx.get_tensor("test.k_cumdecay").unwrap();
        let ch_k_cd = view_dim2_slice_2d(&mut ctx, k_cd_checkpoint, 0).unwrap();
        assert_replayed_mul_mat_matches_cpu(&mut ctx, ch_k_cd, state_checkpoint, "v_prime");

        let v_chunked = ctx
            .mul_mat(v_b_t, attn_checkpoint, BufferUsage::Activations)
            .unwrap();
        let v_t = ctx.transpose(v_chunked).unwrap();
        let v_t = ctx.cont(v_t).unwrap();
        let ch_v_t = view_dim2_slice_2d(&mut ctx, v_t, 0).unwrap();
        let v_t_p = ctx
            .mul_mat(ch_k_cd, state_checkpoint, BufferUsage::Activations)
            .unwrap();
        let v_t_new = ctx
            .binary_like_a(Op::Sub, ch_v_t, v_t_p, BufferUsage::Activations)
            .unwrap();

        let kq_checkpoint = ctx.get_tensor("test.kq").unwrap();
        let ch_kq = view_dim2_slice_2d(&mut ctx, kq_checkpoint, 0).unwrap();
        assert_replayed_mul_mat_matches_cpu(&mut ctx, v_t_new, ch_kq, "v_attn");

        let q_scaled = ctx.get_tensor("test.q_in").unwrap();
        let q_chunked = ctx.permute(q_scaled, [0, 2, 1, 3]).unwrap();
        let q_chunked = ctx
            .pad_4d(
                q_chunked,
                0,
                (64 - (n_tokens % 64)) % 64,
                0,
                0,
                BufferUsage::Activations,
            )
            .unwrap();
        let q_chunked = ctx.reshape(q_chunked, &[s_v, 64, 1, h_k * n_seqs]).unwrap();
        let g_cs_checkpoint = ctx.get_tensor("test.g_cs").unwrap();
        let g_exp = ctx
            .unary(g_cs_checkpoint, UnaryOp::Exp, BufferUsage::Activations)
            .unwrap();
        let g_exp_t = ctx.transpose(g_exp).unwrap();
        let g_exp_t = ctx.cont(g_exp_t).unwrap();
        let q_g_exp = ctx
            .binary_like_a(Op::Mul, q_chunked, g_exp_t, BufferUsage::Activations)
            .unwrap();
        let ch_q_g_exp = view_dim2_slice_2d(&mut ctx, q_g_exp, 0).unwrap();
        assert_replayed_mul_mat_matches_cpu(&mut ctx, state_checkpoint, ch_q_g_exp, "attn_inter");

        let kg_t_checkpoint = ctx.get_tensor("test.key_gdiff_t").unwrap();
        let ch_kg_t = view_dim2_slice_2d(&mut ctx, kg_t_checkpoint, 0).unwrap();
        assert_replayed_mul_mat_matches_cpu(&mut ctx, ch_kg_t, v_t_new, "kgv");

        let runtime = match MetalRuntime::new() {
            Ok(runtime) => runtime,
            Err(_) => return,
        };
        let mut graph = Graph::new();
        graph.build_forward_expand(&ctx, output_cont).unwrap();
        graph.build_forward_expand(&ctx, state_cont).unwrap();

        let prepared = prepare_graph(&ctx, &graph, runtime.features()).unwrap();
        let session = MetalGraphSession::from_runtime(
            runtime,
            &ctx,
            &prepared,
            BufferStorageMode::Shared,
            BufferStorageMode::Shared,
        )
        .unwrap();
        let execution = session
            .execute(&ctx, &[], &[output_cont, state_cont])
            .unwrap();
        let actual_output = bytes_to_f32s(execution.outputs.get(&output_cont).unwrap());
        let actual_state = bytes_to_f32s(execution.outputs.get(&state_cont).unwrap());

        let (expected_output, expected_state) = cpu_gated_delta_net_f32(
            &q_values,
            &k_values,
            &v_values,
            &g_values,
            &beta_values,
            &state_values,
            s_v as usize,
            h_k as usize,
            h_v as usize,
            n_tokens as usize,
            n_seqs as usize,
        );

        assert_eq!(actual_output.len(), expected_output.len());
        assert_eq!(actual_state.len(), expected_state.len());
        let mut output_max_abs_diff = 0.0f32;
        for (a, e) in actual_output.iter().zip(expected_output.iter()) {
            output_max_abs_diff = output_max_abs_diff.max((a - e).abs());
        }
        let mut state_max_abs_diff = 0.0f32;
        for (a, e) in actual_state.iter().zip(expected_state.iter()) {
            state_max_abs_diff = state_max_abs_diff.max((a - e).abs());
        }
        assert!(
            output_max_abs_diff < 1.0e-4 && state_max_abs_diff < 1.0e-4,
            "delta_net_chunking mismatch: output_max_abs_diff={} state_max_abs_diff={}",
            output_max_abs_diff,
            state_max_abs_diff
        );
    }

    fn cpu_gated_delta_net_f32(
        q: &[f32],
        k: &[f32],
        v: &[f32],
        g: &[f32],
        beta: &[f32],
        state: &[f32],
        s_v: usize,
        h_k: usize,
        h_v: usize,
        n_tokens: usize,
        n_seqs: usize,
    ) -> (Vec<f32>, Vec<f32>) {
        assert_eq!(q.len(), s_v * h_k * n_tokens * n_seqs);
        assert_eq!(k.len(), s_v * h_k * n_tokens * n_seqs);
        assert_eq!(v.len(), s_v * h_v * n_tokens * n_seqs);
        assert_eq!(beta.len(), h_v * n_tokens * n_seqs);
        assert_eq!(state.len(), s_v * s_v * h_v * n_seqs);
        assert!(
            g.len() == h_v * n_tokens * n_seqs || g.len() == s_v * h_v * n_tokens * n_seqs,
            "gate tensor must be scalar or per-channel"
        );

        let kda = g.len() == s_v * h_v * n_tokens * n_seqs;
        let scale = 1.0f32 / (s_v as f32).sqrt();
        let mut attn_out = vec![0.0f32; s_v * h_v * n_tokens * n_seqs];
        let mut state_out = state.to_vec();
        let mut delta = vec![0.0f32; s_v];

        for seq in 0..n_seqs {
            for head in 0..h_v {
                let q_head = head % h_k;
                let k_head = head % h_k;
                let state_base = (seq * h_v + head) * s_v * s_v;

                for token in 0..n_tokens {
                    let q_base = ((seq * n_tokens + token) * h_k + q_head) * s_v;
                    let k_base = ((seq * n_tokens + token) * h_k + k_head) * s_v;
                    let v_base = ((seq * n_tokens + token) * h_v + head) * s_v;
                    let beta_idx = (seq * n_tokens + token) * h_v + head;
                    let beta_val = beta[beta_idx];

                    if kda {
                        let g_base = ((seq * n_tokens + token) * h_v + head) * s_v;
                        for row in 0..s_v {
                            let row_base = state_base + row * s_v;
                            for col in 0..s_v {
                                state_out[row_base + col] *= g[g_base + col].exp();
                            }
                        }
                    } else {
                        let g_exp = g[beta_idx].exp();
                        for idx in 0..(s_v * s_v) {
                            state_out[state_base + idx] *= g_exp;
                        }
                    }

                    for row in 0..s_v {
                        let row_base = state_base + row * s_v;
                        let mut sum = 0.0f32;
                        for col in 0..s_v {
                            sum += state_out[row_base + col] * k[k_base + col];
                        }
                        delta[row] = (v[v_base + row] - sum) * beta_val;
                    }

                    for row in 0..s_v {
                        let row_base = state_base + row * s_v;
                        for col in 0..s_v {
                            state_out[row_base + col] += k[k_base + col] * delta[row];
                        }
                    }

                    let out_base = ((seq * n_tokens + token) * h_v + head) * s_v;
                    for row in 0..s_v {
                        let row_base = state_base + row * s_v;
                        let mut sum = 0.0f32;
                        for col in 0..s_v {
                            sum += state_out[row_base + col] * q[q_base + col];
                        }
                        attn_out[out_base + row] = sum * scale;
                    }
                }
            }
        }

        (attn_out, state_out)
    }

    fn execute_tensor_f32(ctx: &mut Context, tensor_id: TensorId) -> Vec<f32> {
        let runtime = match MetalRuntime::new() {
            Ok(runtime) => runtime,
            Err(_) => return Vec::new(),
        };
        let root = checkpoint_root_direct(ctx, tensor_id).unwrap();
        let mut graph = Graph::new();
        graph.build_forward_expand(ctx, root).unwrap();
        let prepared = prepare_graph(ctx, &graph, runtime.features()).unwrap();
        let session = MetalGraphSession::from_runtime(
            runtime,
            ctx,
            &prepared,
            BufferStorageMode::Shared,
            BufferStorageMode::Shared,
        )
        .unwrap();
        let execution = session.execute(ctx, &[], &[root]).unwrap();
        bytes_to_f32s(execution.outputs.get(&root).unwrap())
    }

    fn assert_replayed_mul_mat_matches_cpu(
        ctx: &mut Context,
        lhs: TensorId,
        rhs: TensorId,
        label: &str,
    ) {
        let lhs_tensor = ctx.tensor(lhs).unwrap().clone();
        let rhs_tensor = ctx.tensor(rhs).unwrap().clone();
        assert_eq!(
            lhs_tensor.ne[0], rhs_tensor.ne[0],
            "delta_net_chunking {} inner dimension mismatch: lhs={:?} rhs={:?}",
            label, lhs_tensor.ne, rhs_tensor.ne
        );
        assert_eq!(
            lhs_tensor.ne[2] * lhs_tensor.ne[3],
            rhs_tensor.ne[2] * rhs_tensor.ne[3],
            "delta_net_chunking {} batch dimension mismatch: lhs={:?} rhs={:?}",
            label,
            lhs_tensor.ne,
            rhs_tensor.ne
        );

        let lhs_values = execute_tensor_f32(ctx, lhs);
        let rhs_values = execute_tensor_f32(ctx, rhs);
        let k = usize::try_from(lhs_tensor.ne[0]).unwrap();
        let m = usize::try_from(lhs_tensor.ne[1]).unwrap();
        let n = usize::try_from(rhs_tensor.ne[1]).unwrap();
        let batches = usize::try_from(lhs_tensor.ne[2] * lhs_tensor.ne[3]).unwrap();
        let expected = cpu_mul_mat_batched_f32(&lhs_values, &rhs_values, k, m, n, batches);

        let replayed = ctx.mul_mat(lhs, rhs, BufferUsage::Activations).unwrap();
        let actual = execute_tensor_f32(ctx, replayed);

        let mut max_abs_diff = 0.0f32;
        for (actual, expected) in actual.iter().zip(expected.iter()) {
            max_abs_diff = max_abs_diff.max((actual - expected).abs());
        }
        if max_abs_diff > 1.0e-2 {
            panic!(
                "delta_net_chunking replayed {} mismatch: max_abs_diff={}",
                label, max_abs_diff
            );
        }
    }

    fn f32s_to_bytes(values: &[f32]) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(values.len() * std::mem::size_of::<f32>());
        for value in values {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes
    }

    fn bytes_to_f32s(bytes: &[u8]) -> Vec<f32> {
        bytes
            .chunks_exact(4)
            .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect()
    }

    fn patterned_f32s(len: usize, base: f32, step: f32) -> Vec<f32> {
        (0..len).map(|idx| base + step * idx as f32).collect()
    }

    fn cpu_solve_tri_f32(a: &[f32], b: &[f32], n: usize, k: usize, batches: usize) -> Vec<f32> {
        let mut out = vec![0.0f32; b.len()];
        for batch in 0..batches {
            let a_batch = &a[batch * n * n..(batch + 1) * n * n];
            let b_batch = &b[batch * k * n..(batch + 1) * k * n];
            let out_batch = &mut out[batch * k * n..(batch + 1) * k * n];
            for col in 0..k {
                for row in 0..n {
                    let mut sum = 0.0f32;
                    for idx in 0..row {
                        sum += a_batch[row * n + idx] * out_batch[col + idx * k];
                    }
                    out_batch[col + row * k] =
                        (b_batch[col + row * k] - sum) / a_batch[row * n + row];
                }
            }
        }
        out
    }

    fn cpu_mul_mat_batched_f32(
        a: &[f32],
        b: &[f32],
        k: usize,
        m: usize,
        n: usize,
        batches: usize,
    ) -> Vec<f32> {
        let mut out = vec![0.0f32; m * n * batches];
        for batch in 0..batches {
            let a_batch = &a[batch * k * m..(batch + 1) * k * m];
            let b_batch = &b[batch * k * n..(batch + 1) * k * n];
            let out_batch = &mut out[batch * m * n..(batch + 1) * m * n];
            for col in 0..n {
                for row in 0..m {
                    let mut sum = 0.0f32;
                    for kk in 0..k {
                        sum += a_batch[row * k + kk] * b_batch[col * k + kk];
                    }
                    out_batch[col * m + row] = sum;
                }
            }
        }
        out
    }

    fn checkpoint_root(ctx: &mut Context, name: &str) -> std::result::Result<TensorId, String> {
        let tensor_id = ctx
            .get_tensor(name)
            .ok_or_else(|| format!("missing tensor '{name}'"))?;
        let tensor = ctx
            .tensor(tensor_id)
            .ok_or_else(|| format!("invalid tensor id {tensor_id} for checkpoint {name}"))?
            .clone();
        if tensor.desc.layout.rank() <= 2 {
            ctx.cont_2d(tensor_id, tensor.ne[0], tensor.ne[1])
        } else {
            ctx.cont_2d(
                tensor_id,
                tensor.ne[0] * tensor.ne[1],
                tensor.ne[2] * tensor.ne[3],
            )
        }
    }

    fn checkpoint_root_direct(
        ctx: &mut Context,
        tensor_id: TensorId,
    ) -> std::result::Result<TensorId, String> {
        let tensor = ctx
            .tensor(tensor_id)
            .ok_or_else(|| format!("invalid tensor id {tensor_id}"))?
            .clone();
        if tensor.desc.layout.rank() <= 2 {
            ctx.cont_2d(tensor_id, tensor.ne[0], tensor.ne[1])
        } else {
            ctx.cont_2d(
                tensor_id,
                tensor.ne[0] * tensor.ne[1],
                tensor.ne[2] * tensor.ne[3],
            )
        }
    }
}
