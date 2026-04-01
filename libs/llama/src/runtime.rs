use std::collections::BTreeMap;

use makepad_ggml::{
    backend::metal::{
        execute_compiled_graph, prepare_graph, try_get_rows_ggml_bytes, try_matmul_nt_ggml_bytes,
        try_rms_norm_mul_f32, BufferStorageMode, MetalCompiledGraph, MetalDeviceFeatures,
        MetalGraphSession, MetalGraphTensorWrite, MetalPreparedGraph, MetalRuntime,
    },
    f16_to_f32, f32_to_f16, ggml_row_size_for_type, BufferUsage, Context, Graph, Op, Prec, Tensor,
    TensorId, TensorLayout, TensorType, UnaryOp, GGML_ROPE_TYPE_IMROPE, GGML_ROPE_TYPE_MROPE,
};

use crate::error::{LlamaError, Result};
use crate::weights::LoadedGgufWeights;

#[derive(Clone, Debug)]
pub enum ProbeInputKind {
    TokenIds {
        token_embedding_name: String,
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
    expanded[..n_tokens].copy_from_slice(positions);
    expanded[n_tokens..2 * n_tokens].copy_from_slice(positions);
    expanded[2 * n_tokens..3 * n_tokens].copy_from_slice(positions);
    Ok(expanded)
}

fn causal_mask_f16_bytes(n_tokens: usize) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(n_tokens * n_tokens * std::mem::size_of::<u16>());
    let zero = f32_to_f16(0.0);
    let neg_inf = f32_to_f16(f32::NEG_INFINITY);
    for query in 0..n_tokens {
        for key in 0..n_tokens {
            let value = if key > query { neg_inf } else { zero };
            bytes.extend_from_slice(&value.to_le_bytes());
        }
    }
    bytes
}

fn causal_mask_f32_bytes(n_tokens: usize) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(n_tokens * n_tokens * std::mem::size_of::<f32>());
    for query in 0..n_tokens {
        for key in 0..n_tokens {
            let value = if key > query { f32::NEG_INFINITY } else { 0.0 };
            bytes.extend_from_slice(&value.to_le_bytes());
        }
    }
    bytes
}

fn position_causal_mask_f16_bytes(key_count: usize, positions: &[i32]) -> Result<Vec<u8>> {
    let mut bytes = Vec::with_capacity(
        positions
            .len()
            .checked_mul(key_count)
            .and_then(|v| v.checked_mul(std::mem::size_of::<u16>()))
            .ok_or_else(|| LlamaError::format("overflow computing attention decode mask bytes"))?,
    );
    let zero = f32_to_f16(0.0);
    let neg_inf = f32_to_f16(f32::NEG_INFINITY);
    for &position in positions {
        let position = usize::try_from(position)
            .map_err(|_| LlamaError::format(format!("negative attention position {}", position)))?;
        if position >= key_count {
            return Err(LlamaError::format(format!(
                "attention position {} exceeds key_count {}",
                position, key_count
            )));
        }
        for key in 0..key_count {
            let value = if key > position { neg_inf } else { zero };
            bytes.extend_from_slice(&value.to_le_bytes());
        }
    }
    Ok(bytes)
}

fn position_causal_mask_f32_bytes(key_count: usize, positions: &[i32]) -> Result<Vec<u8>> {
    let mut bytes = Vec::with_capacity(
        positions
            .len()
            .checked_mul(key_count)
            .and_then(|v| v.checked_mul(std::mem::size_of::<f32>()))
            .ok_or_else(|| LlamaError::format("overflow computing attention decode mask bytes"))?,
    );
    for &position in positions {
        let position = usize::try_from(position)
            .map_err(|_| LlamaError::format(format!("negative attention position {}", position)))?;
        if position >= key_count {
            return Err(LlamaError::format(format!(
                "attention position {} exceeds key_count {}",
                position, key_count
            )));
        }
        for key in 0..key_count {
            let value = if key > position {
                f32::NEG_INFINITY
            } else {
                0.0
            };
            bytes.extend_from_slice(&value.to_le_bytes());
        }
    }
    Ok(bytes)
}

fn should_use_flash_attention(n_tokens: usize) -> bool {
    n_tokens == 1
}

fn attention_mask_tensor_type(n_tokens: usize) -> TensorType {
    if should_use_flash_attention(n_tokens) {
        TensorType::F16
    } else {
        TensorType::F32
    }
}

fn attention_mask_bytes_for_tensor(
    ctx: &Context,
    tensor_id: TensorId,
    n_tokens: usize,
) -> Result<Vec<u8>> {
    let tensor = require_tensor(ctx, tensor_id)?;
    match tensor.desc.ty {
        TensorType::F16 => Ok(causal_mask_f16_bytes(n_tokens)),
        TensorType::F32 => Ok(causal_mask_f32_bytes(n_tokens)),
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
    positions: &[i32],
) -> Result<Vec<u8>> {
    let tensor = require_tensor(ctx, tensor_id)?;
    match tensor.desc.ty {
        TensorType::F16 => position_causal_mask_f16_bytes(key_count, positions),
        TensorType::F32 => position_causal_mask_f32_bytes(key_count, positions),
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
    pub q_layout: AttentionQueryLayout,
    pub k_proj_name: String,
    pub v_proj_name: String,
    pub output_proj_name: String,
    pub q_norm_name: Option<String>,
    pub k_norm_name: Option<String>,
    pub q_head_dim: u32,
    pub q_head_count: u32,
    pub k_head_dim: u32,
    pub kv_head_count: u32,
    pub v_head_dim: u32,
    pub rms_epsilon: f32,
    pub rope: Option<AttentionRopeSpec>,
    pub causal: bool,
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
}

#[derive(Clone, Debug)]
pub struct AttentionDecodeGraph {
    pub graph: Graph,
    pub input_primary: TensorId,
    pub input_positions: TensorId,
    pub input_rope_positions: Option<TensorId>,
    pub input_mask: Option<TensorId>,
    pub k_cache: TensorId,
    pub v_cache: TensorId,
    pub k_cache_view: TensorId,
    pub v_cache_view: TensorId,
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
    pub input_norm_name: String,
    pub qkv_proj_name: String,
    pub z_proj_name: String,
    pub beta_proj_name: String,
    pub alpha_proj_name: String,
    pub dt_bias_name: String,
    pub a_name: String,
    pub conv_kernel_name: String,
    pub norm_name: String,
    pub output_proj_name: String,
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
    pub gate_activation: UnaryOp,
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
    pub up_proj_name: String,
    pub down_proj_name: String,
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
pub enum HybridLayerSpec {
    Attention {
        layer_index: u32,
        decode: AttentionDecodeSpec,
        ffn: MoeFfnSpec,
    },
    Recurrent {
        layer_index: u32,
        decode: DeltaNetRecurrentDecodeSpec,
        ffn: MoeFfnSpec,
    },
}

#[derive(Clone, Debug)]
pub struct HybridDecodeSpec {
    pub input: ProbeInputKind,
    pub output_norm_name: String,
    pub output_name: String,
    pub rms_epsilon: f32,
    pub layers: Vec<HybridLayerSpec>,
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
    pub max_sequences: i64,
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
    pub input_positions: Option<TensorId>,
    pub input_rope_positions: Option<TensorId>,
    pub attention_cache_views: Vec<HybridAttentionCacheView>,
    pub moe_selected_experts: Vec<HybridMoeSelection>,
    pub state_updates: Vec<TensorId>,
    pub result_hidden: TensorId,
    pub result_logits: TensorId,
}

pub struct CompiledHybridDecodeMetal {
    spec: HybridDecodeSpec,
    decode: HybridDecodeGraph,
    session: MetalGraphSession,
}

impl CompiledHybridDecodeMetal {
    pub fn runtime(&self) -> &MetalRuntime {
        self.session.runtime()
    }

    pub fn decode(&self) -> &HybridDecodeGraph {
        &self.decode
    }

    pub fn execute(
        &self,
        ctx: &mut Context,
        input: LogitsProbeInput<'_>,
        positions: &[i32],
        cache_tokens: usize,
    ) -> Result<HybridDecodeRun> {
        execute_prepared_hybrid_decode_metal(
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
pub struct HybridDecodeRun {
    pub hidden: Vec<f32>,
    pub logits: Vec<f32>,
    pub n_tokens: usize,
    pub hidden_size: usize,
    pub vocab_size: usize,
    pub selected_experts: Vec<(u32, Vec<i32>)>,
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
                .ok_or_else(|| LlamaError::format("overflow computing key-cache elements"))?,
        )?;
        let attention_v_bytes_per_layer = bytes_for_elements(
            spec.attention_v_type,
            spec.attention_v_width
                .checked_mul(u64::from(spec.n_ctx_seq))
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
        } => {
            let token_embd = require_tensor_id(tensor_ids, token_embedding_name)?;
            ctx.get_rows(token_embd, input_primary, BufferUsage::Activations)
                .map_err(LlamaError::format)?
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
    let result_output = ctx
        .mul_mat(output, result_norm_scaled, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
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
    n_tokens: usize,
    prefix: &str,
) -> Result<TensorId> {
    if should_use_flash_attention(n_tokens) {
        let attn = ctx
            .flash_attn_ext(
                q,
                k,
                v,
                input_mask,
                1.0f32 / (q_head_dim as f32).sqrt(),
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
        .scale(
            kq,
            1.0f32 / (q_head_dim as f32).sqrt(),
            BufferUsage::Activations,
        )
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(kq, format!("{prefix}.kq_scaled"))
        .map_err(LlamaError::format)?;
    if let Some(input_mask) = input_mask {
        kq = ctx
            .binary_like_a(Op::Add, kq, input_mask, BufferUsage::Activations)
            .map_err(LlamaError::format)?;
        ctx.set_tensor_name(kq, format!("{prefix}.kq_masked"))
            .map_err(LlamaError::format)?;
    }
    kq = ctx
        .soft_max(kq, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(kq, format!("{prefix}.kq_soft_max"))
        .map_err(LlamaError::format)?;

    let v_transposed = ctx.transpose(v).map_err(LlamaError::format)?;
    let v_for_matmul = ctx.cont(v_transposed).map_err(LlamaError::format)?;
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
                attention_mask_tensor_type(n_tokens),
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
        } => {
            let token_embd = require_tensor_id(tensor_ids, token_embedding_name)?;
            ctx.get_rows(token_embd, input_primary, BufferUsage::Activations)
                .map_err(LlamaError::format)?
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
    let q_proj = ctx
        .mul_mat(q_weight, input_norm, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
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

    let v_weight = require_tensor_id(tensor_ids, &spec.v_proj_name)?;
    let mut v_states = ctx
        .mul_mat(v_weight, input_norm, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
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

    if let Some(rope) = &spec.rope {
        let positions = input_positions.ok_or_else(|| {
            LlamaError::format(
                "attention block rope was requested without an input positions tensor",
            )
        })?;
        q_states = ctx
            .rope_multi(
                q_states,
                positions,
                None,
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
                None,
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

    q_states = ctx
        .reshape(
            q_states,
            &[
                i64::from(spec.q_head_dim),
                i64::from(spec.q_head_count),
                n_tokens as i64,
                1,
            ],
        )
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(q_states, "attn.q_states_4d")
        .map_err(LlamaError::format)?;
    k_states = ctx
        .reshape(
            k_states,
            &[
                i64::from(spec.k_head_dim),
                i64::from(spec.kv_head_count),
                n_tokens as i64,
                1,
            ],
        )
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(k_states, "attn.k_states_4d")
        .map_err(LlamaError::format)?;
    v_states = ctx
        .reshape(
            v_states,
            &[
                i64::from(spec.v_head_dim),
                i64::from(spec.kv_head_count),
                n_tokens as i64,
                1,
            ],
        )
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(v_states, "attn.v_states_4d")
        .map_err(LlamaError::format)?;

    let q_attn = ctx
        .permute(q_states, [0, 2, 1, 3])
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(q_attn, "attn.q_attn")
        .map_err(LlamaError::format)?;
    let k_attn = ctx
        .permute(k_states, [0, 2, 1, 3])
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(k_attn, "attn.k_attn")
        .map_err(LlamaError::format)?;
    let v_attn = ctx
        .permute(v_states, [0, 2, 1, 3])
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(v_attn, "attn.v_attn")
        .map_err(LlamaError::format)?;

    let mut attn = build_attention_mha_output(
        ctx,
        q_attn,
        k_attn,
        v_attn,
        input_mask,
        spec.q_head_dim,
        n_tokens,
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
        .map(|input_mask| attention_mask_bytes_for_tensor(ctx, input_mask, n_tokens))
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
    k_cache_view: TensorId,
    v_cache_view: TensorId,
    result_output: TensorId,
}

fn build_attention_decode_from_hidden(
    ctx: &mut Context,
    tensor_ids: &BTreeMap<String, TensorId>,
    spec: &AttentionDecodeSpec,
    input_embed: TensorId,
    input_positions: TensorId,
    input_rope_positions: Option<TensorId>,
    n_tokens: usize,
    prefix: &str,
) -> Result<BuiltAttentionDecode> {
    let block = &spec.block;
    let n_tokens_i64 =
        i64::try_from(n_tokens).map_err(|_| LlamaError::format("n_tokens does not fit in i64"))?;

    let input_norm = build_rms_norm_mul(
        ctx,
        tensor_ids,
        input_embed,
        block.rms_epsilon,
        &block.input_norm_name,
        &format!("{prefix}.input_norm"),
    )?;

    let q_weight = require_tensor_id(tensor_ids, &block.q_proj_name)?;
    let q_proj = ctx
        .mul_mat(q_weight, input_norm, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
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

    let v_weight = require_tensor_id(tensor_ids, &block.v_proj_name)?;
    let mut v_states = ctx
        .mul_mat(v_weight, input_norm, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
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

    let input_mask = if block.causal {
        let mask = ctx
            .new_named_tensor(
                format!("{prefix}.kq_mask"),
                attention_mask_tensor_type(n_tokens),
                4,
                &[i64::from(spec.cache.max_context), n_tokens_i64, 1, 1],
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
        q_states = ctx
            .rope_multi(
                q_states,
                rope_positions,
                None,
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
                None,
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

    q_states = ctx
        .reshape(
            q_states,
            &[
                i64::from(block.q_head_dim),
                i64::from(block.q_head_count),
                n_tokens_i64,
                1,
            ],
        )
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(q_states, format!("{prefix}.q_states_4d"))
        .map_err(LlamaError::format)?;

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

    let k_cache_written = ctx
        .set_rows(k_cache, k_store, input_positions, BufferUsage::State)
        .map_err(LlamaError::format)?;
    let v_cache_written = ctx
        .set_rows(v_cache, v_store, input_positions, BufferUsage::State)
        .map_err(LlamaError::format)?;

    let k_cache_view = ctx
        .view_4d(
            k_cache_written,
            i64::from(block.k_head_dim),
            i64::from(spec.cache.max_context),
            i64::from(block.kv_head_count),
            i64::from(spec.cache.max_sequences),
            row_size(spec.cache.k_type, k_merged_width)?,
            row_size(spec.cache.k_type, i64::from(block.k_head_dim))?,
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
            i64::from(spec.cache.max_context),
            i64::from(block.kv_head_count),
            i64::from(spec.cache.max_sequences),
            row_size(spec.cache.v_type, v_merged_width)?,
            row_size(spec.cache.v_type, i64::from(block.v_head_dim))?,
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

    let q_attn = ctx
        .permute(q_states, [0, 2, 1, 3])
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(q_attn, format!("{prefix}.q_attn"))
        .map_err(LlamaError::format)?;
    let mut attn = build_attention_mha_output(
        ctx,
        q_attn,
        k_cache_view,
        v_cache_view,
        input_mask,
        block.q_head_dim,
        n_tokens,
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
        } => {
            let token_embd = require_tensor_id(tensor_ids, token_embedding_name)?;
            ctx.get_rows(token_embd, input_primary, BufferUsage::Activations)
                .map_err(LlamaError::format)?
        }
        ProbeInputKind::Embeddings { .. } => input_primary,
    };
    ctx.set_tensor_name(input_embed, "attn_decode.input_embed")
        .map_err(LlamaError::format)?;
    let built = build_attention_decode_from_hidden(
        ctx,
        tensor_ids,
        spec,
        input_embed,
        input_positions,
        input_rope_positions,
        n_tokens,
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
        input_rope_positions,
        input_mask: built.input_mask,
        k_cache: built.k_cache,
        v_cache: built.v_cache,
        k_cache_view: built.k_cache_view,
        v_cache_view: built.v_cache_view,
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

pub fn compile_attention_decode_metal(
    weights: &mut LoadedGgufWeights,
    spec: &AttentionDecodeSpec,
    n_tokens: usize,
) -> Result<CompiledAttentionDecodeMetal> {
    let runtime = MetalRuntime::new().map_err(LlamaError::unsupported)?;
    let (decode, prepared) = prepare_attention_decode_graph(
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

    Ok(CompiledAttentionDecodeMetal {
        spec: spec.clone(),
        decode,
        session,
    })
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

    configure_attention_cache_view(
        ctx,
        decode.k_cache_view,
        i64::from(spec.block.k_head_dim),
        cache_tokens,
        i64::from(spec.block.kv_head_count),
        i64::from(spec.cache.max_sequences),
    )?;
    configure_attention_cache_view(
        ctx,
        decode.v_cache_view,
        i64::from(spec.block.v_head_dim),
        cache_tokens,
        i64::from(spec.block.kv_head_count),
        i64::from(spec.cache.max_sequences),
    )?;
    if let Some(input_mask) = decode.input_mask {
        configure_attention_mask_view(ctx, input_mask, cache_tokens, positions.len())?;
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
            tensor_id: decode.input_positions,
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
            position_attention_mask_bytes_for_tensor(ctx, input_mask, cache_tokens, positions)
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
    r_cache_update: TensorId,
    s_cache_update: TensorId,
    result_output: TensorId,
}

fn build_delta_net_recurrent_decode_from_hidden(
    ctx: &mut Context,
    tensor_ids: &BTreeMap<String, TensorId>,
    spec: &DeltaNetRecurrentDecodeSpec,
    input_embed: TensorId,
    n_tokens: usize,
    prefix: &str,
) -> Result<BuiltDeltaNetRecurrentDecode> {
    let block = &spec.block;
    let n_seqs = i64::from(spec.cache.max_sequences);
    let n_tokens_i64 =
        i64::try_from(n_tokens).map_err(|_| LlamaError::format("n_tokens does not fit in i64"))?;
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

    let qkv_weight = require_tensor_id(tensor_ids, &block.qkv_proj_name)?;
    let mut qkv_mixed = ctx
        .mul_mat(qkv_weight, input_norm, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    qkv_mixed = ctx
        .reshape(qkv_mixed, &[qkv_dim, n_tokens_i64, n_seqs])
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(qkv_mixed, format!("{prefix}.qkv_mixed"))
        .map_err(LlamaError::format)?;

    let z_weight = require_tensor_id(tensor_ids, &block.z_proj_name)?;
    let z = ctx
        .mul_mat(z_weight, input_norm, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(z, format!("{prefix}.z"))
        .map_err(LlamaError::format)?;

    let beta_weight = require_tensor_id(tensor_ids, &block.beta_proj_name)?;
    let mut beta = ctx
        .mul_mat(beta_weight, input_norm, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    beta = ctx
        .reshape(
            beta,
            &[1, i64::from(block.value_head_count), n_tokens_i64, n_seqs],
        )
        .map_err(LlamaError::format)?;
    beta = ctx
        .unary(beta, UnaryOp::Sigmoid, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(beta, format!("{prefix}.beta"))
        .map_err(LlamaError::format)?;

    let alpha_weight = require_tensor_id(tensor_ids, &block.alpha_proj_name)?;
    let mut alpha = ctx
        .mul_mat(alpha_weight, input_norm, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    alpha = ctx
        .reshape(
            alpha,
            &[i64::from(block.value_head_count), n_tokens_i64, n_seqs],
        )
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
            &[1, i64::from(block.value_head_count), n_tokens_i64, n_seqs],
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

    let conv_states = ctx
        .view_3d(
            r_cache,
            conv_prefix,
            qkv_dim,
            n_seqs,
            row_size(spec.cache.r_type, conv_prefix)?,
            row_size(spec.cache.r_type, r_width)?,
            0,
        )
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(conv_states, format!("{prefix}.conv_states"))
        .map_err(LlamaError::format)?;

    let qkv_mixed_t = ctx.transpose(qkv_mixed).map_err(LlamaError::format)?;
    let conv_input = ctx
        .concat(conv_states, qkv_mixed_t, 0, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(conv_input, format!("{prefix}.conv_input"))
        .map_err(LlamaError::format)?;

    let conv_input_tensor = require_tensor(ctx, conv_input)?.clone();
    let last_conv_states = ctx
        .view_3d(
            conv_input,
            conv_prefix,
            qkv_dim,
            n_seqs,
            conv_input_tensor.nb[1],
            conv_input_tensor.nb[2],
            row_size(conv_input_tensor.desc.ty, n_tokens_i64)?,
        )
        .map_err(LlamaError::format)?;
    let last_conv_states_flat = ctx
        .view_1d(last_conv_states, r_width * n_seqs, 0)
        .map_err(LlamaError::format)?;
    let r_cache_flat = ctx
        .view_1d(r_cache, r_width * n_seqs, 0)
        .map_err(LlamaError::format)?;
    let r_cache_update = ctx
        .cpy(last_conv_states_flat, r_cache_flat, BufferUsage::State)
        .map_err(LlamaError::format)?;

    let state = ctx
        .view_4d(
            s_cache,
            i64::from(block.value_head_dim),
            i64::from(block.value_head_dim),
            i64::from(block.value_head_count),
            n_seqs,
            row_size(spec.cache.s_type, i64::from(block.value_head_dim))?,
            row_size(
                spec.cache.s_type,
                i64::from(block.value_head_dim) * i64::from(block.value_head_dim),
            )?,
            row_size(spec.cache.s_type, s_width)?,
            0,
        )
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(state, format!("{prefix}.state"))
        .map_err(LlamaError::format)?;

    let conv_output = ctx
        .ssm_conv(conv_input, conv_kernel_id, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    let conv_output = ctx
        .unary(conv_output, UnaryOp::Silu, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(conv_output, format!("{prefix}.conv_output"))
        .map_err(LlamaError::format)?;

    let conv_output_tensor = require_tensor(ctx, conv_output)?.clone();
    let qk_heads_width = i64::from(block.key_head_dim) * i64::from(block.key_head_count);
    let q_conv = ctx
        .view_4d(
            conv_output,
            i64::from(block.key_head_dim),
            i64::from(block.key_head_count),
            n_tokens_i64,
            n_seqs,
            row_size(conv_output_tensor.desc.ty, i64::from(block.key_head_dim))?,
            row_size(conv_output_tensor.desc.ty, qkv_dim)?,
            row_size(conv_output_tensor.desc.ty, qkv_dim * n_tokens_i64)?,
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
            n_tokens_i64,
            n_seqs,
            row_size(conv_output_tensor.desc.ty, i64::from(block.key_head_dim))?,
            row_size(conv_output_tensor.desc.ty, qkv_dim)?,
            row_size(conv_output_tensor.desc.ty, qkv_dim * n_tokens_i64)?,
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
            n_tokens_i64,
            n_seqs,
            row_size(conv_output_tensor.desc.ty, i64::from(block.value_head_dim))?,
            row_size(conv_output_tensor.desc.ty, qkv_dim)?,
            row_size(conv_output_tensor.desc.ty, qkv_dim * n_tokens_i64)?,
            row_size(conv_output_tensor.desc.ty, qk_heads_width * 2)?,
        )
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(v_conv, format!("{prefix}.v_conv"))
        .map_err(LlamaError::format)?;

    let q_conv = ctx
        .l2_norm_eps(q_conv, block.rms_epsilon, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(q_conv, format!("{prefix}.q_conv_predelta"))
        .map_err(LlamaError::format)?;
    let k_conv = ctx
        .l2_norm_eps(k_conv, block.rms_epsilon, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(k_conv, format!("{prefix}.k_conv_predelta"))
        .map_err(LlamaError::format)?;

    let delta = ctx
        .gated_delta_net(
            q_conv,
            k_conv,
            v_conv,
            gate,
            beta,
            state,
            BufferUsage::Activations,
        )
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(delta, format!("{prefix}.delta"))
        .map_err(LlamaError::format)?;

    let output = ctx
        .view_4d(
            delta,
            i64::from(block.value_head_dim),
            i64::from(block.value_head_count),
            n_tokens_i64,
            n_seqs,
            row_size(TensorType::F32, i64::from(block.value_head_dim))?,
            row_size(TensorType::F32, value_hidden_size)?,
            row_size(TensorType::F32, value_hidden_size * n_tokens_i64)?,
            0,
        )
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(output, format!("{prefix}.output_view"))
        .map_err(LlamaError::format)?;
    let new_state = ctx
        .view_4d(
            delta,
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
            row_size(TensorType::F32, value_hidden_size * n_tokens_i64 * n_seqs)?,
        )
        .map_err(LlamaError::format)?;

    let new_state_flat = ctx
        .view_1d(new_state, s_width * n_seqs, 0)
        .map_err(LlamaError::format)?;
    let s_cache_flat = ctx
        .view_1d(s_cache, s_width * n_seqs, 0)
        .map_err(LlamaError::format)?;
    let s_cache_update = ctx
        .cpy(new_state_flat, s_cache_flat, BufferUsage::State)
        .map_err(LlamaError::format)?;

    let z = ctx
        .reshape(
            z,
            &[
                i64::from(block.value_head_dim),
                i64::from(block.value_head_count),
                n_tokens_i64,
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
        .reshape(gated_output, &[value_hidden_size, n_tokens_i64 * n_seqs])
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(final_output, format!("{prefix}.final_output"))
        .map_err(LlamaError::format)?;
    let output_weight = require_tensor_id(tensor_ids, &block.output_proj_name)?;
    let mut result_output = ctx
        .mul_mat(output_weight, final_output, BufferUsage::Activations)
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
        r_cache_update,
        s_cache_update,
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
    if spec.cache.max_sequences != 1 {
        return Err(LlamaError::unsupported(format!(
            "delta-net recurrent decode currently requires max_sequences=1, got {}",
            spec.cache.max_sequences
        )));
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

    let input_embed = match &block.input {
        ProbeInputKind::TokenIds {
            token_embedding_name,
        } => {
            let token_embd = require_tensor_id(tensor_ids, token_embedding_name)?;
            ctx.get_rows(token_embd, input_primary, BufferUsage::Activations)
                .map_err(LlamaError::format)?
        }
        ProbeInputKind::Embeddings { .. } => input_primary,
    };
    ctx.set_tensor_name(input_embed, "recur_decode.input_embed")
        .map_err(LlamaError::format)?;
    let built = build_delta_net_recurrent_decode_from_hidden(
        ctx,
        tensor_ids,
        spec,
        input_embed,
        n_tokens,
        "recur_decode",
    )?;
    let result_output = built.result_output;
    mark_output(ctx, result_output)?;

    let mut graph = Graph::new();
    graph
        .build_forward_expand(ctx, built.r_cache_update)
        .map_err(LlamaError::format)?;
    graph
        .build_forward_expand(ctx, built.s_cache_update)
        .map_err(LlamaError::format)?;
    graph
        .build_forward_expand(ctx, result_output)
        .map_err(LlamaError::format)?;

    Ok(DeltaNetRecurrentDecodeGraph {
        graph,
        input_primary,
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
        &[MetalGraphTensorWrite {
            tensor_id: decode.input_primary,
            bytes: &input_primary,
        }],
        &[decode.result_output],
    )
    .map_err(LlamaError::format)?;

    let result_bytes = execution
        .outputs
        .get(&decode.result_output)
        .ok_or_else(|| {
            LlamaError::format("compiled delta-net recurrent decode did not produce result bytes")
        })?;
    let hidden = f32_bytes_to_vec(result_bytes)?;
    let output = ctx
        .tensor(decode.result_output)
        .ok_or_else(|| LlamaError::format("delta-net recurrent result tensor is invalid"))?;

    Ok(AttentionBlockRun {
        hidden,
        hidden_size: ne_usize(output, 0)?,
        n_tokens: ne_usize(output, 1)?,
    })
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

    let selected_experts = ctx
        .top_k(
            probs,
            i64::from(spec.expert_used_count),
            BufferUsage::Activations,
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

    let (gate, up) = if let Some(merged_name) = &spec.merged_gate_up_proj_name {
        let merged_weight = require_tensor_id(tensor_ids, merged_name)?;
        let gate_up = ctx
            .mul_mat_id(
                merged_weight,
                input_3d,
                selected_experts,
                BufferUsage::Activations,
            )
            .map_err(LlamaError::format)?;
        ctx.set_tensor_name(gate_up, format!("{prefix}.gate_up"))
            .map_err(LlamaError::format)?;
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
        (Some(gate), up)
    } else {
        let up_weight = require_tensor_id(tensor_ids, &spec.up_proj_name)?;
        let up = ctx
            .mul_mat_id(
                up_weight,
                input_3d,
                selected_experts,
                BufferUsage::Activations,
            )
            .map_err(LlamaError::format)?;
        ctx.set_tensor_name(up, format!("{prefix}.up"))
            .map_err(LlamaError::format)?;
        let gate = if let Some(name) = &spec.gate_proj_name {
            let gate_weight = require_tensor_id(tensor_ids, name)?;
            let gate = ctx
                .mul_mat_id(
                    gate_weight,
                    input_3d,
                    selected_experts,
                    BufferUsage::Activations,
                )
                .map_err(LlamaError::format)?;
            ctx.set_tensor_name(gate, format!("{prefix}.gate"))
                .map_err(LlamaError::format)?;
            Some(gate)
        } else {
            None
        };
        (gate, up)
    };

    let activated = if let Some(gate) = gate {
        let gate = ctx
            .unary(gate, spec.activation, BufferUsage::Activations)
            .map_err(LlamaError::format)?;
        ctx.set_tensor_name(gate, format!("{prefix}.gate_act"))
            .map_err(LlamaError::format)?;
        ctx.binary_like_a(Op::Mul, gate, up, BufferUsage::Activations)
            .map_err(LlamaError::format)?
    } else {
        ctx.unary(up, spec.activation, BufferUsage::Activations)
            .map_err(LlamaError::format)?
    };
    ctx.set_tensor_name(activated, format!("{prefix}.hidden"))
        .map_err(LlamaError::format)?;

    let down_weight = require_tensor_id(tensor_ids, &spec.down_proj_name)?;
    let experts = ctx
        .mul_mat_id(
            down_weight,
            activated,
            selected_experts,
            BufferUsage::Activations,
        )
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(experts, format!("{prefix}.down"))
        .map_err(LlamaError::format)?;

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
        } => {
            let token_embd = require_tensor_id(tensor_ids, token_embedding_name)?;
            ctx.get_rows(token_embd, input_primary, BufferUsage::Activations)
                .map_err(LlamaError::format)?
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

pub fn build_hybrid_decode_graph(
    ctx: &mut Context,
    tensor_ids: &BTreeMap<String, TensorId>,
    spec: &HybridDecodeSpec,
    n_tokens: usize,
) -> Result<HybridDecodeGraph> {
    if n_tokens == 0 {
        return Err(LlamaError::format(
            "hybrid decode graph requires at least one token",
        ));
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

    let has_attention = spec
        .layers
        .iter()
        .any(|layer| matches!(layer, HybridLayerSpec::Attention { .. }));
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

    let mut hidden = match &spec.input {
        ProbeInputKind::TokenIds {
            token_embedding_name,
        } => {
            let token_embd = require_tensor_id(tensor_ids, token_embedding_name)?;
            ctx.get_rows(token_embd, input_primary, BufferUsage::Activations)
                .map_err(LlamaError::format)?
        }
        ProbeInputKind::Embeddings { .. } => input_primary,
    };
    ctx.set_tensor_name(hidden, "hybrid_decode.input_embed")
        .map_err(LlamaError::format)?;

    let mut attention_cache_views = Vec::new();
    let mut moe_selected_experts = Vec::new();
    let mut state_updates = Vec::new();

    for layer in &spec.layers {
        match layer {
            HybridLayerSpec::Attention {
                layer_index,
                decode,
                ffn,
            } => {
                let prefix = format!("hybrid_decode.layer{layer_index}");
                let positions = input_positions.ok_or_else(|| {
                    LlamaError::format(format!(
                        "attention layer {} requires position input",
                        layer_index
                    ))
                })?;
                let attn = build_attention_decode_from_hidden(
                    ctx,
                    tensor_ids,
                    decode,
                    hidden,
                    positions,
                    input_rope_positions,
                    n_tokens,
                    &format!("{prefix}.attn"),
                )?;
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
                    max_sequences: i64::from(decode.cache.max_sequences),
                });
                let residual = attn.result_output;
                let moe = build_moe_ffn_from_hidden(
                    ctx,
                    tensor_ids,
                    ffn,
                    residual,
                    n_tokens,
                    &format!("{prefix}.moe"),
                )?;
                moe_selected_experts.push(HybridMoeSelection {
                    layer_index: *layer_index,
                    selected_experts: moe.selected_experts,
                    expert_used_count: usize::try_from(ffn.expert_used_count).map_err(|_| {
                        LlamaError::format("expert_used_count does not fit in usize")
                    })?,
                });
                hidden = ctx
                    .binary_like_a(
                        Op::Add,
                        moe.result_output,
                        residual,
                        BufferUsage::Activations,
                    )
                    .map_err(LlamaError::format)?;
                ctx.set_tensor_name(hidden, format!("{prefix}.post_moe"))
                    .map_err(LlamaError::format)?;
            }
            HybridLayerSpec::Recurrent {
                layer_index,
                decode,
                ffn,
            } => {
                let prefix = format!("hybrid_decode.layer{layer_index}");
                let recur = build_delta_net_recurrent_decode_from_hidden(
                    ctx,
                    tensor_ids,
                    decode,
                    hidden,
                    n_tokens,
                    &format!("{prefix}.recur"),
                )?;
                state_updates.push(recur.r_cache_update);
                state_updates.push(recur.s_cache_update);
                let residual = recur.result_output;
                let moe = build_moe_ffn_from_hidden(
                    ctx,
                    tensor_ids,
                    ffn,
                    residual,
                    n_tokens,
                    &format!("{prefix}.moe"),
                )?;
                moe_selected_experts.push(HybridMoeSelection {
                    layer_index: *layer_index,
                    selected_experts: moe.selected_experts,
                    expert_used_count: usize::try_from(ffn.expert_used_count).map_err(|_| {
                        LlamaError::format("expert_used_count does not fit in usize")
                    })?,
                });
                hidden = ctx
                    .binary_like_a(
                        Op::Add,
                        moe.result_output,
                        residual,
                        BufferUsage::Activations,
                    )
                    .map_err(LlamaError::format)?;
                ctx.set_tensor_name(hidden, format!("{prefix}.post_moe"))
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
    let output_weight = require_tensor_id(tensor_ids, &spec.output_name)?;
    let result_logits = ctx
        .mul_mat(output_weight, result_norm, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(result_logits, "hybrid_decode.result_logits")
        .map_err(LlamaError::format)?;
    mark_output(ctx, result_logits)?;

    let mut graph = Graph::new();
    for &update in &state_updates {
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

    Ok(HybridDecodeGraph {
        graph,
        input_primary,
        input_positions,
        input_rope_positions,
        attention_cache_views,
        moe_selected_experts,
        state_updates,
        result_hidden,
        result_logits,
    })
}

pub fn prepare_hybrid_decode_graph(
    ctx: &mut Context,
    tensor_ids: &BTreeMap<String, TensorId>,
    spec: &HybridDecodeSpec,
    n_tokens: usize,
    features: MetalDeviceFeatures,
) -> Result<(HybridDecodeGraph, MetalPreparedGraph)> {
    let decode = build_hybrid_decode_graph(ctx, tensor_ids, spec, n_tokens)?;
    let prepared = prepare_graph(ctx, &decode.graph, features).map_err(LlamaError::format)?;
    Ok((decode, prepared))
}

pub fn compile_hybrid_decode_metal(
    weights: &mut LoadedGgufWeights,
    spec: &HybridDecodeSpec,
    n_tokens: usize,
) -> Result<CompiledHybridDecodeMetal> {
    let runtime = MetalRuntime::new().map_err(LlamaError::unsupported)?;
    let (decode, prepared) = prepare_hybrid_decode_graph(
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

    Ok(CompiledHybridDecodeMetal {
        spec: spec.clone(),
        decode,
        session,
    })
}

pub fn execute_prepared_hybrid_decode_metal(
    runtime: &MetalRuntime,
    ctx: &mut Context,
    spec: &HybridDecodeSpec,
    decode: &HybridDecodeGraph,
    compiled: &MetalCompiledGraph,
    input: LogitsProbeInput<'_>,
    positions: &[i32],
    cache_tokens: usize,
) -> Result<HybridDecodeRun> {
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
    } else if !positions.is_empty() {
        return Err(LlamaError::format(
            "hybrid decode received positions for a graph without attention layers",
        ));
    }

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

    for cache_view in &decode.attention_cache_views {
        configure_attention_cache_view(
            ctx,
            cache_view.k_cache_view,
            cache_view.k_head_dim,
            cache_tokens,
            cache_view.kv_head_count,
            cache_view.max_sequences,
        )?;
        configure_attention_cache_view(
            ctx,
            cache_view.v_cache_view,
            cache_view.v_head_dim,
            cache_tokens,
            cache_view.kv_head_count,
            cache_view.max_sequences,
        )?;
        if let Some(input_mask) = cache_view.input_mask {
            configure_attention_mask_view(ctx, input_mask, cache_tokens, positions.len())?;
        }
    }

    let mut writes = vec![MetalGraphTensorWrite {
        tensor_id: decode.input_primary,
        bytes: &input_primary,
    }];
    if let Some(input_positions) = decode.input_positions {
        writes.push(MetalGraphTensorWrite {
            tensor_id: input_positions,
            bytes: i32_slice_as_bytes(positions),
        });
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
        Some(encode_rope_positions(rope, positions, positions.len())?)
    } else {
        None
    };
    if let Some(input_rope_positions) = decode.input_rope_positions {
        writes.push(MetalGraphTensorWrite {
            tensor_id: input_rope_positions,
            bytes: i32_slice_as_bytes(hybrid_rope_positions.as_deref().ok_or_else(|| {
                LlamaError::format("hybrid decode rope positions were not prepared")
            })?),
        });
    }
    let attention_mask_bytes = decode
        .attention_cache_views
        .iter()
        .filter_map(|cache_view| {
            cache_view.input_mask.map(|input_mask| {
                position_attention_mask_bytes_for_tensor(ctx, input_mask, cache_tokens, positions)
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let mut attention_mask_index = 0usize;
    for cache_view in &decode.attention_cache_views {
        if cache_view.input_mask.is_some() {
            let bytes = attention_mask_bytes
                .get(attention_mask_index)
                .ok_or_else(|| {
                    LlamaError::format("hybrid decode mask storage is unexpectedly empty")
                })?;
            attention_mask_index += 1;
            writes.push(MetalGraphTensorWrite {
                tensor_id: cache_view.input_mask.ok_or_else(|| {
                    LlamaError::format(format!(
                        "hybrid decode attention layer {} lost its input mask",
                        cache_view.layer_index
                    ))
                })?,
                bytes,
            });
        }
    }

    let mut outputs = vec![decode.result_hidden, decode.result_logits];
    outputs.extend(
        decode
            .moe_selected_experts
            .iter()
            .map(|sel| sel.selected_experts),
    );
    let execution = execute_compiled_graph(runtime, ctx, compiled, &writes, &outputs)
        .map_err(LlamaError::format)?;

    let hidden = execution
        .outputs
        .get(&decode.result_hidden)
        .ok_or_else(|| LlamaError::format("hybrid decode did not produce hidden bytes"))?;
    let hidden = f32_bytes_to_vec(hidden)?;
    let logits = execution
        .outputs
        .get(&decode.result_logits)
        .ok_or_else(|| LlamaError::format("hybrid decode did not produce logits bytes"))?;
    let logits = f32_bytes_to_vec(logits)?;
    let hidden_tensor = ctx
        .tensor(decode.result_hidden)
        .ok_or_else(|| LlamaError::format("hybrid decode result_hidden tensor is invalid"))?;
    let logits_tensor = ctx
        .tensor(decode.result_logits)
        .ok_or_else(|| LlamaError::format("hybrid decode result_logits tensor is invalid"))?;

    let mut selected_experts = Vec::with_capacity(decode.moe_selected_experts.len());
    for selection in &decode.moe_selected_experts {
        let bytes = execution
            .outputs
            .get(&selection.selected_experts)
            .ok_or_else(|| {
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

    Ok(HybridDecodeRun {
        hidden,
        logits,
        n_tokens: ne_usize(hidden_tensor, 1)?,
        hidden_size: ne_usize(hidden_tensor, 0)?,
        vocab_size: ne_usize(logits_tensor, 0)?,
        selected_experts,
    })
}

pub fn execute_hybrid_decode_graph_metal(
    weights: &mut LoadedGgufWeights,
    spec: &HybridDecodeSpec,
    n_tokens: usize,
    input: LogitsProbeInput<'_>,
    positions: &[i32],
    cache_tokens: usize,
) -> Result<HybridDecodeRun> {
    let compiled = compile_hybrid_decode_metal(weights, spec, n_tokens)?;
    compiled.execute(&mut weights.ctx, input, positions, cache_tokens)
}

pub fn execute_hybrid_decode_graph_metal_cached(
    compiled: &CompiledHybridDecodeMetal,
    weights: &mut LoadedGgufWeights,
    input: LogitsProbeInput<'_>,
    positions: &[i32],
    cache_tokens: usize,
) -> Result<HybridDecodeRun> {
    compiled.execute(&mut weights.ctx, input, positions, cache_tokens)
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
            },
            LogitsProbeInput::TokenIds(token_ids),
        ) => {
            let token_embd_id = weights.require_tensor_id(token_embedding_name)?;
            let token_embd = require_tensor(&weights.ctx, token_embd_id)?;
            let hidden_size = ne_usize(token_embd, 0)?;
            let vocab_size = ne_usize(token_embd, 1)?;
            try_get_rows_ggml_bytes(
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
                    "Metal get_rows is unavailable or unsupported for {}",
                    token_embd.desc.ty.name()
                ))
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

    let selected = try_get_rows_ggml_bytes(
        f32_slice_as_bytes(&input_embed),
        TensorType::F32.ggml_type(),
        hidden_size,
        n_tokens,
        output_ids,
    )
    .ok_or_else(|| LlamaError::unsupported("Metal F32 get_rows is unavailable".to_string()))?;

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
    let logits = try_matmul_nt_ggml_bytes(
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
    let gate = ctx
        .mul_mat(gate_weight, input, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(gate, &format!("{prefix}.gate"))
        .map_err(LlamaError::format)?;

    let up_weight = require_tensor_id(tensor_ids, &spec.up_proj_name)?;
    let up = ctx
        .mul_mat(up_weight, input, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(up, &format!("{prefix}.up"))
        .map_err(LlamaError::format)?;

    let gate = ctx
        .unary(gate, spec.gate_activation, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(gate, &format!("{prefix}.gate_act"))
        .map_err(LlamaError::format)?;

    let hidden = ctx
        .binary_like_a(Op::Mul, gate, up, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(hidden, &format!("{prefix}.hidden"))
        .map_err(LlamaError::format)?;

    let down_weight = require_tensor_id(tensor_ids, &spec.down_proj_name)?;
    let output = ctx
        .mul_mat(down_weight, hidden, BufferUsage::Activations)
        .map_err(LlamaError::format)?;
    ctx.set_tensor_name(output, &format!("{prefix}.output"))
        .map_err(LlamaError::format)?;

    Ok(output)
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

fn configure_attention_cache_view(
    ctx: &mut Context,
    tensor_id: TensorId,
    ne0: i64,
    ne1: usize,
    ne2: i64,
    ne3: i64,
) -> Result<()> {
    let tensor = ctx
        .tensor(tensor_id)
        .ok_or_else(|| LlamaError::format(format!("invalid tensor id {}", tensor_id)))?;
    let strides = tensor.nb;
    let layout = TensorLayout::from_parts(
        4,
        &[
            ne0,
            i64::try_from(ne1).map_err(|_| {
                LlamaError::format(format!("cache length {} does not fit in i64", ne1))
            })?,
            ne2,
            ne3,
        ],
        &strides,
    )
    .map_err(LlamaError::format)?;
    ctx.set_tensor_layout(tensor_id, layout)
        .map_err(LlamaError::format)
}

fn configure_attention_mask_view(
    ctx: &mut Context,
    tensor_id: TensorId,
    key_count: usize,
    query_count: usize,
) -> Result<()> {
    let tensor = ctx
        .tensor(tensor_id)
        .ok_or_else(|| LlamaError::format(format!("invalid tensor id {}", tensor_id)))?;
    let strides = tensor.nb;
    let layout = TensorLayout::from_parts(
        4,
        &[
            i64::try_from(key_count).map_err(|_| {
                LlamaError::format(format!("mask key count {} does not fit in i64", key_count))
            })?,
            i64::try_from(query_count).map_err(|_| {
                LlamaError::format(format!(
                    "mask query count {} does not fit in i64",
                    query_count
                ))
            })?,
            tensor.ne[2],
            tensor.ne[3],
        ],
        &strides,
    )
    .map_err(LlamaError::format)?;
    ctx.set_tensor_layout(tensor_id, layout)
        .map_err(LlamaError::format)
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

#[cfg(test)]
mod tests {
    use super::{HybridCacheShape, HybridCacheTemplate, HybridCacheTypes};
    use makepad_ggml::TensorType;

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
}
