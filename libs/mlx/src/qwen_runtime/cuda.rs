use super::*;
use makepad_ggml::backend::cuda::{
    CudaBuffer, CudaGraphExec, CudaMappedHostU32Buffer, CudaRuntime,
};
use std::error::Error;
use std::sync::{Arc, Mutex};

type CudaResult<T> = std::result::Result<T, String>;

pub(super) fn try_cuda_generation_backend(
    runtime_session: Arc<MlxQwen35MoeRuntimeSession>,
    capacity_tokens: usize,
    do_sample: bool,
) -> std::result::Result<
    Option<Box<dyn crate::qwen_runtime::lazy::QwenGenerationBackend>>,
    Box<dyn Error>,
> {
    if !makepad_ggml::backend::cuda::is_available() {
        return Ok(None);
    }
    Ok(Some(Box::new(QwenCudaGenerationBackend::new(
        runtime_session,
        capacity_tokens,
        do_sample,
    )?)))
}

#[allow(dead_code)]
pub(super) fn debug_prefill_top1s(
    runtime_session: &MlxQwen35MoeRuntimeSession,
    prompt_token_ids: &[u32],
    steps: usize,
) -> CudaResult<Vec<u32>> {
    let runtime = runtime_session
        .cuda_text_runtime()
        .map_err(|err| err.to_string())?;
    let steps = steps.min(prompt_token_ids.len());
    let runtime = runtime
        .lock()
        .map_err(|_| "qwen cuda runtime mutex poisoned".to_string())?;
    let mut session = runtime
        .new_decode_session(steps.saturating_add(1), &[])
        .map_err(|err| format!("debug compare new_decode_session failed: {err}"))?;
    let mut top1s = Vec::with_capacity(steps);
    for (position, &token_id) in prompt_token_ids.iter().take(steps).enumerate() {
        runtime
            .eval_token_logits(&mut session, token_id, position)
            .map_err(|err| format!("debug compare eval_token_logits failed at position {position}: {err}"))?;
        let logits = runtime
            .cuda
            .read_f32s(&session.workspace.logits, runtime.vocab_size)
            .map_err(|err| format!("debug compare read logits failed at position {position}: {err}"))?;
        top1s.push(argmax_index(&logits) as u32);
    }
    Ok(top1s)
}

fn debug_recurrent_reference(
    runtime_session: &MlxQwen35MoeRuntimeSession,
    layer: &crate::MlxQwen35MoeLayerTensors,
    input_hidden: &[f32],
    state_before: &MlxQwen35MoeRecurrentDecodeState,
) -> CudaResult<QwenRecurrentReferenceDebug> {
    let recurrent = layer
        .recurrent
        .as_ref()
        .ok_or_else(|| format!("layer {} missing recurrent tensors", layer.index))?;
    let hidden_norm = runtime_session
        .rms_norm_weighted_f32(
            input_hidden,
            &layer.attn_norm,
            runtime_session.weights.snapshot.config.text_config.rms_norm_eps,
        )
        .map_err(|err| err.to_string())?;
    let input_words = f32_to_bf16_words(&hidden_norm);
    let qkv = cpu_project_vector_bf16_words_fallback(runtime_session, &input_words, &recurrent.wqkv)?;
    let gate_z =
        cpu_project_vector_bf16_words_fallback(runtime_session, &input_words, &recurrent.wqkv_gate)?;
    let beta_logits =
        cpu_project_vector_bf16_words_fallback(runtime_session, &input_words, &recurrent.ssm_beta)?;
    let alpha =
        cpu_project_vector_bf16_words_fallback(runtime_session, &input_words, &recurrent.ssm_alpha)?;
    let conv_kernel = runtime_session
        .conv1d_kernel_f32(
            &recurrent.ssm_conv1d,
            runtime_session.dims.ssm_conv_kernel as usize,
            qkv.len(),
        )
        .map_err(|err| err.to_string())?;
    let mut conv_state = state_before.conv_state.clone();
    let conv_out = apply_ssm_conv_with_state_f32(
        &qkv,
        &mut conv_state,
        &conv_kernel,
        runtime_session.dims.ssm_conv_kernel as usize,
    )
    .map_err(|err| err.to_string())?;
    let (q_raw, k_raw, v) = split_recurrent_qkv_projection(
        &conv_out,
        runtime_session.dims.ssm_state_size as usize,
        runtime_session.dims.ssm_group_count as usize,
        runtime_session
            .dims
            .recurrent_value_head_dim()
            .map_err(|err| err.to_string())? as usize,
        runtime_session.dims.ssm_time_step_rank as usize,
    )
    .map_err(|err| err.to_string())?;
    let mut query_kernel = rms_norm_rows_no_scale_f32(
        &q_raw,
        runtime_session.dims.ssm_group_count as usize,
        runtime_session.dims.ssm_state_size as usize,
        runtime_session.weights.snapshot.config.text_config.rms_norm_eps,
    )
    .map_err(|err| err.to_string())?;
    let mut key_kernel = rms_norm_rows_no_scale_f32(
        &k_raw,
        runtime_session.dims.ssm_group_count as usize,
        runtime_session.dims.ssm_state_size as usize,
        runtime_session.weights.snapshot.config.text_config.rms_norm_eps,
    )
    .map_err(|err| err.to_string())?;
    let inv_scale = (runtime_session.dims.ssm_state_size as f32).sqrt().recip();
    scale_in_place(&mut query_kernel, inv_scale);
    scale_in_place(&mut key_kernel, inv_scale);

    let beta = beta_logits
        .iter()
        .copied()
        .map(sigmoid_f32)
        .collect::<Vec<_>>();
    let dt_bias = runtime_session
        .vector_tensor_f32_cached(&recurrent.ssm_dt)
        .map_err(|err| err.to_string())?;
    let a_log = runtime_session
        .vector_tensor_f32_cached(&recurrent.ssm_a)
        .map_err(|err| err.to_string())?;
    let decay_log = a_log
        .iter()
        .copied()
        .zip(alpha.iter().copied())
        .zip(dt_bias.iter().copied())
        .map(|((a_log, alpha), dt_bias)| -(a_log.exp()) * softplus_f32(alpha + dt_bias))
        .collect::<Vec<_>>();
    let decay_gate = decay_log.iter().copied().map(f32::exp).collect::<Vec<_>>();

    let mut recurrent_query = query_kernel.clone();
    scale_in_place(&mut recurrent_query, inv_scale);
    let mut ssm_state = state_before.ssm_state.clone();
    let recurrent_out = gated_delta_net_step_f32(
        &recurrent_query,
        &key_kernel,
        &v,
        &decay_gate,
        &beta,
        &mut ssm_state,
        runtime_session.dims.ssm_state_size as usize,
        runtime_session.dims.ssm_group_count as usize,
        runtime_session
            .dims
            .recurrent_value_head_dim()
            .map_err(|err| err.to_string())? as usize,
        runtime_session.dims.ssm_time_step_rank as usize,
    )
    .map_err(|err| err.to_string())?;
    let ssm_norm_weights = runtime_session
        .vector_tensor_f32_cached(&recurrent.ssm_norm)
        .map_err(|err| err.to_string())?;
    let recurrent_out_norm = rms_norm_rows_shared_weight_f32(
        &recurrent_out,
        ssm_norm_weights.as_ref(),
        runtime_session.dims.ssm_time_step_rank as usize,
        runtime_session
            .dims
            .recurrent_value_head_dim()
            .map_err(|err| err.to_string())? as usize,
        runtime_session.weights.snapshot.config.text_config.rms_norm_eps,
    )
    .map_err(|err| err.to_string())?;
    let mut recurrent_gated = recurrent_out_norm.clone();
    apply_silu_gate_in_place(&mut recurrent_gated, &gate_z).map_err(|err| err.to_string())?;
    let recurrent_proj = cpu_project_vector_bf16_words_fallback(
        runtime_session,
        &f32_to_bf16_words(&recurrent_gated),
        &recurrent.ssm_out,
    )?;

    Ok(QwenRecurrentReferenceDebug {
        hidden_norm,
        qkv,
        gate_z,
        beta_logits,
        alpha,
        conv_out,
        q_raw,
        k_raw,
        v,
        query_kernel,
        key_kernel,
        beta,
        decay_log,
        recurrent_out,
        recurrent_out_norm,
        recurrent_gated,
        recurrent_proj,
    })
}

fn cpu_project_vector_bf16_words_fallback(
    runtime_session: &MlxQwen35MoeRuntimeSession,
    input_words: &[u16],
    weight_name: &str,
) -> CudaResult<Vec<f32>> {
    let weight_entry = runtime_session
        .weights
        .tensor(weight_name)
        .map_err(|err| err.to_string())?;
    match weight_entry.dtype {
        MlxDType::BF16 => dense_bf16_matmul_t_f32(&runtime_session.weights, input_words, weight_name)
            .map_err(|err| err.to_string()),
        MlxDType::U32 => {
            let quantization = runtime_session
                .weights
                .quantization_for_tensor(weight_name)
                .map_err(|err| err.to_string())?
                .ok_or_else(|| format!("tensor {weight_name} is missing quantization config"))?;
            let actual_weight_name = runtime_session
                .weights
                .actual_tensor_name(weight_name)
                .map_err(|err| err.to_string())?;
            let (actual_scales_name, actual_biases_name) = actual_affine_qparam_names(actual_weight_name);
            let scales_entry = runtime_session
                .weights
                .tensor(&actual_scales_name)
                .map_err(|err| err.to_string())?;
            let packed_words = runtime_session
                .weights
                .read_u32_tensor_words_cached(weight_name)
                .map_err(|err| err.to_string())?;
            let scale_words = runtime_session
                .weights
                .read_bf16_tensor_words_cached(&actual_scales_name)
                .map_err(|err| err.to_string())?;
            let bias_words = runtime_session
                .weights
                .read_bf16_tensor_words_cached(&actual_biases_name)
                .map_err(|err| err.to_string())?;
            affine_quantized_matmul_fallback(
                input_words,
                packed_words.as_slice(),
                scale_words.as_slice(),
                bias_words.as_slice(),
                weight_entry.shape[0] as usize,
                weight_entry.shape[1] as usize,
                scales_entry.shape[1] as usize,
                quantization.group_size as u64,
                quantization.bits,
            )
            .map_err(|err| err.to_string())
        }
        other => Err(format!(
            "tensor {weight_name} expected BF16 or U32, got {:?}",
            other
        )),
    }
}

fn debug_attention_reference(
    runtime_session: &MlxQwen35MoeRuntimeSession,
    layer: &crate::MlxQwen35MoeLayerTensors,
    input_hidden: &[f32],
    position: usize,
) -> CudaResult<QwenAttentionReferenceDebug> {
    let attention = layer
        .attention
        .as_ref()
        .ok_or_else(|| format!("layer {} missing attention tensors", layer.index))?;
    let hidden_norm = runtime_session
        .rms_norm_weighted_f32(
            input_hidden,
            &layer.attn_norm,
            runtime_session.weights.snapshot.config.text_config.rms_norm_eps,
        )
        .map_err(|err| err.to_string())?;
    let input_words = f32_to_bf16_words(&hidden_norm);
    let value = cpu_project_vector_bf16_words_fallback(runtime_session, &input_words, &attention.wv)?;
    let mut key =
        cpu_project_vector_bf16_words_fallback(runtime_session, &input_words, &attention.wk)?;
    let k_norm_weights = runtime_session
        .vector_tensor_f32_cached(&attention.attn_k_norm)
        .map_err(|err| err.to_string())?;
    key = rms_norm_rows_shared_weight_f32(
        &key,
        k_norm_weights.as_ref(),
        runtime_session.dims.attention_head_count_kv as usize,
        runtime_session.dims.attention_key_length as usize,
        runtime_session.weights.snapshot.config.text_config.rms_norm_eps,
    )
    .map_err(|err| err.to_string())?;
    apply_qwen_mrope_rows_in_place(
        &mut key,
        runtime_session.dims.attention_head_count_kv as usize,
        runtime_session.dims.attention_key_length as usize,
        runtime_session.attention_rotary_dim(),
        qwen_text_mrope_positions(position as u32),
        runtime_session.rope_sections4().map_err(|err| err.to_string())?,
        runtime_session.weights.snapshot.config.text_config.rope_parameters.rope_theta,
    )
    .map_err(|err| err.to_string())?;
    Ok(QwenAttentionReferenceDebug {
        hidden_norm,
        value,
        key_rope: key,
    })
}

pub(super) struct CudaQwenTextRuntime {
    cuda: CudaRuntime,
    token_embd: CudaAffineTensor,
    output_norm: CudaBuffer,
    output: CudaAffineTensor,
    layers: Vec<CudaQwenLayer>,
    hidden_size: usize,
    vocab_size: usize,
    attention_query_width: usize,
    attention_qg_width: usize,
    attention_kv_width: usize,
    attention_heads: usize,
    attention_kv_heads: usize,
    attention_head_dim: usize,
    attention_q_heads_per_kv: usize,
    recurrent_q_width: usize,
    recurrent_v_width: usize,
    recurrent_qkv_width: usize,
    recurrent_num_k_heads: usize,
    recurrent_num_v_heads: usize,
    recurrent_head_k_dim: usize,
    recurrent_head_v_dim: usize,
    expert_count: usize,
    experts_used_count: usize,
    expert_intermediate: usize,
    shared_expert_intermediate: usize,
    rotary_dim: usize,
    rope_theta: f32,
    rope_sections4: [u32; 4],
    rms_norm_eps: f32,
}

enum CudaQwenLayer {
    Attention(CudaQwenAttentionLayer),
    Recurrent(CudaQwenRecurrentLayer),
}

struct CudaQwenAttentionLayer {
    attn_norm: CudaBuffer,
    post_attention_norm: CudaBuffer,
    wq: CudaAffineTensor,
    wk: CudaAffineTensor,
    wv: CudaAffineTensor,
    wo: CudaAffineTensor,
    q_norm: CudaBuffer,
    k_norm: CudaBuffer,
    moe: CudaQwenMoeLayer,
}

struct CudaQwenRecurrentLayer {
    attn_norm: CudaBuffer,
    post_attention_norm: CudaBuffer,
    wqkv: CudaAffineTensor,
    wqkv_gate: CudaAffineTensor,
    ssm_beta: CudaAffineTensor,
    ssm_alpha: CudaAffineTensor,
    ssm_out: CudaAffineTensor,
    ssm_conv1d: CudaBuffer,
    ssm_dt: CudaBuffer,
    ssm_a: CudaBuffer,
    ssm_norm: CudaBuffer,
    moe: CudaQwenMoeLayer,
}

struct CudaQwenMoeLayer {
    ffn_gate_inp: CudaAffineTensor,
    ffn_gate_up_exps: Option<CudaAffineTensor>,
    ffn_gate_exps: Option<CudaAffineTensor>,
    ffn_up_exps: Option<CudaAffineTensor>,
    ffn_down_exps: CudaAffineTensor,
    ffn_gate_inp_shexp: CudaAffineTensor,
    ffn_gate_shexp: CudaAffineTensor,
    ffn_up_shexp: CudaAffineTensor,
    ffn_down_shexp: CudaAffineTensor,
}

struct CudaAffineTensor {
    packed_weights: CudaBuffer,
    scales: CudaBuffer,
    biases: CudaBuffer,
    bits: u32,
    out_rows: usize,
    weight_words_per_row: usize,
    qparams_per_row: usize,
    plane_count: usize,
    weight_words_per_plane: usize,
    qparams_words_per_plane: usize,
}

enum CudaQwenLayerState {
    Attention(CudaQwenAttentionLayerState),
    Recurrent(CudaQwenRecurrentLayerState),
}

struct CudaQwenAttentionLayerState {
    key_cache: CudaBuffer,
    value_cache: CudaBuffer,
    stored_tokens: usize,
}

struct CudaQwenRecurrentLayerState {
    conv_state: CudaBuffer,
    gated_delta: CudaBuffer,
}

struct CudaQwenWorkspace {
    hidden_a: CudaBuffer,
    hidden_b: CudaBuffer,
    hidden_norm: CudaBuffer,
    hidden_bf16: CudaBuffer,
    qg_out: CudaBuffer,
    query: CudaBuffer,
    gate: CudaBuffer,
    key: CudaBuffer,
    value: CudaBuffer,
    attention_logits: CudaBuffer,
    attn_out: CudaBuffer,
    attn_gated: CudaBuffer,
    attn_bf16: CudaBuffer,
    attn_proj: CudaBuffer,
    residual: CudaBuffer,
    ffn_input: CudaBuffer,
    moe_router_logits: CudaBuffer,
    moe_route_indices: CudaBuffer,
    moe_route_weights: CudaBuffer,
    moe_routed_accum: CudaBuffer,
    moe_output: CudaBuffer,
    moe_shared_gate_scalar: CudaBuffer,
    moe_shared_gate: CudaBuffer,
    moe_shared_up: CudaBuffer,
    moe_shared_act: CudaBuffer,
    moe_shared_down: CudaBuffer,
    moe_expert_gate_up: CudaBuffer,
    moe_expert_gate_up_batch: CudaBuffer,
    moe_expert_gate: CudaBuffer,
    moe_expert_up: CudaBuffer,
    moe_expert_act: CudaBuffer,
    moe_expert_act_batch: CudaBuffer,
    moe_expert_down: CudaBuffer,
    moe_expert_down_batch: CudaBuffer,
    moe_expert_act_bf16: CudaBuffer,
    moe_expert_act_bf16_batch: CudaBuffer,
    recurrent_qkv: CudaBuffer,
    recurrent_gate_z: CudaBuffer,
    recurrent_beta_logits: CudaBuffer,
    recurrent_beta: CudaBuffer,
    recurrent_alpha: CudaBuffer,
    recurrent_conv: CudaBuffer,
    recurrent_q: CudaBuffer,
    recurrent_k: CudaBuffer,
    recurrent_v: CudaBuffer,
    recurrent_decay: CudaBuffer,
    recurrent_out_norm: CudaBuffer,
    recurrent_gated: CudaBuffer,
    recurrent_gated_bf16: CudaBuffer,
    recurrent_proj: CudaBuffer,
    final_norm: CudaBuffer,
    logits: CudaBuffer,
    argmax_out: CudaBuffer,
    disallowed_token_ids: CudaBuffer,
}

struct CudaQwenDecodeSession {
    capacity_tokens: usize,
    layer_states: Vec<CudaQwenLayerState>,
    workspace: CudaQwenWorkspace,
    disallowed_count: usize,
}

struct CudaQwenGraphTokenState {
    token_id: CudaMappedHostU32Buffer,
    position: CudaMappedHostU32Buffer,
    seq_len: CudaMappedHostU32Buffer,
    start_slot: CudaMappedHostU32Buffer,
    disallowed_count: CudaMappedHostU32Buffer,
}

struct CudaQwenDeviceTokenState {
    token_id: CudaBuffer,
    position: CudaBuffer,
    seq_len: CudaBuffer,
    start_slot: CudaBuffer,
    disallowed_count: CudaBuffer,
}

struct CudaQwenPrefillGraph {
    exec: CudaGraphExec,
    token_state: CudaQwenDeviceTokenState,
}

struct CudaQwenDecodeGraph {
    exec: CudaGraphExec,
    token_state: CudaQwenGraphTokenState,
    argmax_out: CudaMappedHostU32Buffer,
}

struct CudaQwenDecodeChunkGraph {
    exec: CudaGraphExec,
    input_token_id: CudaMappedHostU32Buffer,
    positions: CudaMappedHostU32Buffer,
    seq_lens: CudaMappedHostU32Buffer,
    start_slots: CudaMappedHostU32Buffer,
    disallowed_count: CudaMappedHostU32Buffer,
    step_tokens: CudaBuffer,
    step_count: usize,
}

struct QwenDebugPrefillCompare {
    runtime_session: Arc<MlxQwen35MoeRuntimeSession>,
    reference_state: MlxQwen35MoeDecodeState,
    steps: usize,
    observed_tokens: Vec<u32>,
    reported_mismatch: bool,
}

struct QwenRecurrentReferenceDebug {
    hidden_norm: Vec<f32>,
    qkv: Vec<f32>,
    gate_z: Vec<f32>,
    beta_logits: Vec<f32>,
    alpha: Vec<f32>,
    conv_out: Vec<f32>,
    q_raw: Vec<f32>,
    k_raw: Vec<f32>,
    v: Vec<f32>,
    query_kernel: Vec<f32>,
    key_kernel: Vec<f32>,
    beta: Vec<f32>,
    decay_log: Vec<f32>,
    recurrent_out: Vec<f32>,
    recurrent_out_norm: Vec<f32>,
    recurrent_gated: Vec<f32>,
    recurrent_proj: Vec<f32>,
}

struct QwenAttentionReferenceDebug {
    hidden_norm: Vec<f32>,
    value: Vec<f32>,
    key_rope: Vec<f32>,
}

pub(super) struct QwenCudaGenerationBackend {
    runtime: Arc<Mutex<CudaQwenTextRuntime>>,
    session: CudaQwenDecodeSession,
    prefill_graph: Option<CudaQwenPrefillGraph>,
    decode_graph: Option<CudaQwenDecodeGraph>,
    decode_chunk_graphs: Vec<CudaQwenDecodeChunkGraph>,
    sampling: QwenSamplingOptions,
    disallowed_token_ids: Vec<u32>,
    rng: QwenSamplingRng,
    debug_compare: Option<QwenDebugPrefillCompare>,
}

impl QwenCudaGenerationBackend {
    fn allow_decode_graph(debug_compare_enabled: bool) -> bool {
        std::env::var("MAKEPAD_MLX_QWEN_ENABLE_GRAPH").ok().as_deref() != Some("0")
            && !debug_compare_enabled
            && std::env::var("MAKEPAD_MLX_QWEN_TRACE_LAYERS").ok().as_deref() != Some("1")
            && std::env::var("MAKEPAD_MLX_QWEN_TRACE_MOE").ok().as_deref() != Some("1")
            && std::env::var("MAKEPAD_MLX_QWEN_COMPARE_ATTENTION_LAYER").is_err()
            && std::env::var("MAKEPAD_MLX_QWEN_COMPARE_RECURRENT_LAYER").is_err()
            && std::env::var("MAKEPAD_MLX_QWEN_COMPARE_MOE_LAYER").is_err()
    }

    fn new(
        runtime_session: Arc<MlxQwen35MoeRuntimeSession>,
        capacity_tokens: usize,
        do_sample: bool,
    ) -> std::result::Result<Self, Box<dyn Error>> {
        let runtime = runtime_session.cuda_text_runtime()?;
        let disallowed_token_ids = runtime_session.generation_disallowed_token_ids();
        let sampling = runtime_session.sampling_options(do_sample);
        let debug_compare = std::env::var("MAKEPAD_MLX_QWEN_COMPARE_PREFILL")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .filter(|steps| *steps > 0)
            .map(|steps| {
                runtime_session
                    .new_decode_state()
                    .map(|reference_state| QwenDebugPrefillCompare {
                        runtime_session: runtime_session.clone(),
                        reference_state,
                        steps,
                        observed_tokens: Vec::new(),
                        reported_mismatch: false,
                    })
                    .map_err(|err| err.to_string())
            })
            .transpose()?;
        let runtime_guard = runtime
            .lock()
            .map_err(|_| "qwen cuda runtime mutex poisoned".to_string())?;
        let mut session = runtime_guard.new_decode_session(capacity_tokens, &disallowed_token_ids)?;
        let (prefill_graph, decode_graph, decode_chunk_graphs) =
            if Self::allow_decode_graph(debug_compare.is_some()) {
                (
                    Some(runtime_guard.capture_prefill_graph(&mut session)?),
                    Some(runtime_guard.capture_decode_graph(&mut session)?),
                    Vec::new(),
                )
        } else {
            (None, None, Vec::new())
        };
        drop(runtime_guard);
        Ok(Self {
            runtime,
            session,
            prefill_graph,
            decode_graph,
            decode_chunk_graphs,
            sampling,
            disallowed_token_ids,
            rng: QwenSamplingRng::new(0),
            debug_compare,
        })
    }

    fn maybe_debug_compare_prefill_step(
        &mut self,
        token_id: u32,
        position: usize,
        cuda_top1: u32,
    ) -> CudaResult<()> {
        let Some(debug_compare) = self.debug_compare.as_mut() else {
            return Ok(());
        };
        if position >= debug_compare.steps || self.sampling.do_sample {
            debug_compare.observed_tokens.push(token_id);
            return Ok(());
        }
        let logits = debug_compare
            .runtime_session
            .eval_token_logits_reference_f32(token_id, position, &mut debug_compare.reference_state)
            .map_err(|err| err.to_string())?;
        let reference_top1 = argmax_index(&logits) as u32;
        eprintln!(
            "[qwen-prefill-compare] position={position} input_token={token_id} ref_top1={reference_top1} cuda_top1={cuda_top1}"
        );
        if position + 1 <= debug_compare.steps {
            let runtime = self
                .runtime
                .lock()
                .map_err(|_| "qwen cuda runtime mutex poisoned".to_string())?;
            runtime.debug_compare_decode_state(
                &self.session,
                &debug_compare.reference_state,
                position,
            )?;
        }
        if reference_top1 != cuda_top1 && !debug_compare.reported_mismatch {
            debug_compare.reported_mismatch = true;
        }
        debug_compare.observed_tokens.push(token_id);
        Ok(())
    }

    fn eval_and_select(&mut self, token_id: u32, position: usize) -> CudaResult<u32> {
        let runtime = self
            .runtime
            .lock()
            .map_err(|_| "qwen cuda runtime mutex poisoned".to_string())?;
        if let Some(decode_graph) = self.decode_graph.as_ref() {
            return runtime.eval_and_select_graph(
                &mut self.session,
                decode_graph,
                token_id,
                position,
                self.sampling.do_sample,
                &self.disallowed_token_ids,
                &self.sampling,
                &mut self.rng,
            );
        }
        let reference_state_before = self
            .debug_compare
            .as_ref()
            .filter(|debug_compare| position + 1 == debug_compare.steps && !self.sampling.do_sample)
            .map(|debug_compare| debug_compare.reference_state.clone());
        if let (Some(debug_compare), Some(reference_state_before)) =
            (self.debug_compare.as_ref(), reference_state_before.as_ref())
        {
            runtime.eval_token_logits_with_layer_compare(
                &mut self.session,
                token_id,
                position,
                &debug_compare.runtime_session,
                reference_state_before,
            )?;
        } else {
            runtime.eval_token_logits(&mut self.session, token_id, position)?;
        }
        if self.sampling.do_sample {
            let logits = runtime
                .cuda
                .read_f32s(&self.session.workspace.logits, runtime.vocab_size)?;
            return sample_token_from_logits_f32(
                &logits,
                &self.disallowed_token_ids,
                &self.sampling,
                &mut self.rng,
            )
            .map(|token| token.token_id);
        }
        runtime.cuda.masked_argmax_f32(
            &self.session.workspace.logits,
            &self.session.workspace.disallowed_token_ids,
            self.session.disallowed_count,
            &self.session.workspace.argmax_out,
            runtime.vocab_size,
        )?;
        let top1 = runtime.cuda.read_u32(&self.session.workspace.argmax_out)?;
        drop(runtime);
        self.maybe_debug_compare_prefill_step(token_id, position, top1)?;
        Ok(top1)
    }
}

impl crate::qwen_runtime::lazy::QwenGenerationBackend for QwenCudaGenerationBackend {
    fn preferred_generation_stride(&self) -> usize {
        1
    }

    fn prefill_prompt(&mut self, prompt_token_ids: &[u32]) -> CudaResult<u32> {
        if let Some(prefill_graph) = self.prefill_graph.as_ref() {
            let runtime = self
                .runtime
                .lock()
                .map_err(|_| "qwen cuda runtime mutex poisoned".to_string())?;
            for (position, &token_id) in prompt_token_ids.iter().enumerate() {
                runtime.write_device_token_state(
                    &prefill_graph.token_state,
                    token_id,
                    position,
                    self.disallowed_token_ids.len(),
                )?;
                runtime.cuda.launch_graph(&prefill_graph.exec)?;
            }
            self.session.disallowed_count = self.disallowed_token_ids.len();
            runtime.set_attention_stored_tokens(&mut self.session, prompt_token_ids.len());
            if self.sampling.do_sample {
                runtime.cuda.synchronize()?;
                let logits = runtime
                    .cuda
                    .read_f32s(&self.session.workspace.logits, runtime.vocab_size)?;
                return sample_token_from_logits_f32(
                    &logits,
                    &self.disallowed_token_ids,
                    &self.sampling,
                    &mut self.rng,
                )
                .map(|token| token.token_id);
            }
            runtime.cuda.masked_argmax_f32(
                &self.session.workspace.logits,
                &self.session.workspace.disallowed_token_ids,
                self.session.disallowed_count,
                &self.session.workspace.argmax_out,
                runtime.vocab_size,
            )?;
            return runtime.cuda.read_u32(&self.session.workspace.argmax_out);
        }
        let mut next_token_id = None;
        for (position, &token_id) in prompt_token_ids.iter().enumerate() {
            next_token_id = Some(self.eval_and_select(token_id, position)?);
        }
        next_token_id.ok_or_else(|| "generation requires at least one prompt token".to_string())
    }

    fn eval_next_token(&mut self, token_id: u32, position: usize) -> CudaResult<u32> {
        self.eval_and_select(token_id, position)
    }

    fn eval_token_chunk(
        &mut self,
        token_id: u32,
        position: usize,
        token_count: usize,
    ) -> CudaResult<Vec<u32>> {
        if token_count == 0 {
            return Ok(Vec::new());
        }
        if self.sampling.do_sample || self.decode_chunk_graphs.is_empty() {
            let mut out = Vec::with_capacity(token_count);
            let mut current_token_id = token_id;
            let mut current_position = position;
            for _ in 0..token_count {
                let next_token_id = self.eval_and_select(current_token_id, current_position)?;
                out.push(next_token_id);
                current_token_id = next_token_id;
                current_position += 1;
            }
            return Ok(out);
        }

        let runtime = self
            .runtime
            .lock()
            .map_err(|_| "qwen cuda runtime mutex poisoned".to_string())?;
        let mut out = Vec::with_capacity(token_count);
        let mut current_token_id = token_id;
        let mut current_position = position;
        let mut remaining = token_count;

        while remaining > 0 {
            if let Some(decode_chunk_graph) = self
                .decode_chunk_graphs
                .iter()
                .find(|graph| graph.step_count <= remaining)
            {
                let chunk_tokens = runtime.eval_token_chunk_graph(
                    &mut self.session,
                    decode_chunk_graph,
                    current_token_id,
                    current_position,
                    &self.disallowed_token_ids,
                )?;
                current_token_id = *chunk_tokens
                    .last()
                    .ok_or_else(|| "qwen decode chunk graph returned no tokens".to_string())?;
                current_position += chunk_tokens.len();
                remaining -= chunk_tokens.len();
                out.extend(chunk_tokens);
            } else {
                let next_token_id = runtime.eval_and_select_graph(
                    &mut self.session,
                    self.decode_graph
                        .as_ref()
                        .ok_or_else(|| "missing qwen single-step decode graph".to_string())?,
                    current_token_id,
                    current_position,
                    false,
                    &self.disallowed_token_ids,
                    &self.sampling,
                    &mut self.rng,
                )?;
                out.push(next_token_id);
                current_token_id = next_token_id;
                current_position += 1;
                remaining -= 1;
            }
        }

        Ok(out)
    }
}

impl CudaQwenTextRuntime {
    pub(super) fn load(runtime_session: &MlxQwen35MoeRuntimeSession) -> CudaResult<Self> {
        let cuda = CudaRuntime::load()?;
        let dims = &runtime_session.dims;
        let text = &runtime_session.weights.snapshot.config.text_config;
        let recurrent_head_v_dim = runtime_session
            .dims
            .recurrent_value_head_dim()
            .map_err(|err| err.to_string())? as usize;
        let mut layers = Vec::with_capacity(runtime_session.tensors.layers.len());
        for layer in &runtime_session.tensors.layers {
            let attn_norm = load_vector_f32(&cuda, &runtime_session.weights, &layer.attn_norm)?;
            let post_attention_norm =
                load_vector_f32(&cuda, &runtime_session.weights, &layer.post_attention_norm)?;
            let moe = CudaQwenMoeLayer::load(&cuda, &runtime_session.weights, &layer.moe)?;
            match layer.kind {
                MlxQwen35MoeLayerKind::Attention => {
                    let attention = layer
                        .attention
                        .as_ref()
                        .ok_or_else(|| format!("layer {} missing attention tensors", layer.index))?;
                    layers.push(CudaQwenLayer::Attention(CudaQwenAttentionLayer {
                        attn_norm,
                        post_attention_norm,
                        wq: CudaAffineTensor::load(&cuda, &runtime_session.weights, &attention.wq)?,
                        wk: CudaAffineTensor::load(&cuda, &runtime_session.weights, &attention.wk)?,
                        wv: CudaAffineTensor::load(&cuda, &runtime_session.weights, &attention.wv)?,
                        wo: CudaAffineTensor::load(&cuda, &runtime_session.weights, &attention.wo)?,
                        q_norm: load_vector_f32(
                            &cuda,
                            &runtime_session.weights,
                            &attention.attn_q_norm,
                        )?,
                        k_norm: load_vector_f32(
                            &cuda,
                            &runtime_session.weights,
                            &attention.attn_k_norm,
                        )?,
                        moe,
                    }));
                }
                MlxQwen35MoeLayerKind::Recurrent => {
                    let recurrent = layer
                        .recurrent
                        .as_ref()
                        .ok_or_else(|| format!("layer {} missing recurrent tensors", layer.index))?;
                    let conv_kernel = runtime_session
                        .conv1d_kernel_f32(
                            &recurrent.ssm_conv1d,
                            dims.ssm_conv_kernel as usize,
                            (dims.ssm_inner_size + 2 * dims.ssm_group_count * dims.ssm_state_size)
                                as usize,
                        )
                        .map_err(|err| err.to_string())?;
                    layers.push(CudaQwenLayer::Recurrent(CudaQwenRecurrentLayer {
                        attn_norm,
                        post_attention_norm,
                        wqkv: CudaAffineTensor::load(
                            &cuda,
                            &runtime_session.weights,
                            &recurrent.wqkv,
                        )?,
                        wqkv_gate: CudaAffineTensor::load(
                            &cuda,
                            &runtime_session.weights,
                            &recurrent.wqkv_gate,
                        )?,
                        ssm_beta: CudaAffineTensor::load(
                            &cuda,
                            &runtime_session.weights,
                            &recurrent.ssm_beta,
                        )?,
                        ssm_alpha: CudaAffineTensor::load(
                            &cuda,
                            &runtime_session.weights,
                            &recurrent.ssm_alpha,
                        )?,
                        ssm_out: CudaAffineTensor::load(
                            &cuda,
                            &runtime_session.weights,
                            &recurrent.ssm_out,
                        )?,
                        ssm_conv1d: cuda.load_bytes(f32s_as_le_bytes(&conv_kernel))?,
                        ssm_dt: load_vector_f32(&cuda, &runtime_session.weights, &recurrent.ssm_dt)?,
                        ssm_a: load_vector_f32(&cuda, &runtime_session.weights, &recurrent.ssm_a)?,
                        ssm_norm: load_vector_f32(
                            &cuda,
                            &runtime_session.weights,
                            &recurrent.ssm_norm,
                        )?,
                        moe,
                    }));
                }
            }
        }

        Ok(Self {
            token_embd: CudaAffineTensor::load(
                &cuda,
                &runtime_session.weights,
                &runtime_session.tensors.globals.token_embd,
            )?,
            output_norm: load_vector_f32(
                &cuda,
                &runtime_session.weights,
                &runtime_session.tensors.globals.output_norm,
            )?,
            output: CudaAffineTensor::load(
                &cuda,
                &runtime_session.weights,
                &runtime_session.tensors.globals.output,
            )?,
            layers,
            hidden_size: dims.embedding_length as usize,
            vocab_size: dims.vocab_size as usize,
            attention_query_width: (dims.attention_head_count * dims.attention_key_length) as usize,
            attention_qg_width: (dims.attention_head_count * dims.attention_key_length * 2) as usize,
            attention_kv_width: (dims.attention_head_count_kv * dims.attention_key_length) as usize,
            attention_heads: dims.attention_head_count as usize,
            attention_kv_heads: dims.attention_head_count_kv as usize,
            attention_head_dim: dims.attention_key_length as usize,
            attention_q_heads_per_kv: (dims.attention_head_count / dims.attention_head_count_kv)
                as usize,
            recurrent_q_width: (dims.ssm_group_count * dims.ssm_state_size) as usize,
            recurrent_v_width: (dims.ssm_time_step_rank * dims.recurrent_value_head_dim().map_err(|err| err.to_string())?)
                as usize,
            recurrent_qkv_width: (dims.ssm_inner_size
                + 2 * dims.ssm_group_count * dims.ssm_state_size) as usize,
            recurrent_num_k_heads: dims.ssm_group_count as usize,
            recurrent_num_v_heads: dims.ssm_time_step_rank as usize,
            recurrent_head_k_dim: dims.ssm_state_size as usize,
            recurrent_head_v_dim,
            expert_count: dims.expert_count as usize,
            experts_used_count: dims.expert_used_count as usize,
            expert_intermediate: text.moe_intermediate_size as usize,
            shared_expert_intermediate: text.shared_expert_intermediate_size as usize,
            rotary_dim: runtime_session.attention_rotary_dim(),
            rope_theta: text.rope_parameters.rope_theta,
            rope_sections4: runtime_session.rope_sections4().map_err(|err| err.to_string())?,
            rms_norm_eps: text.rms_norm_eps,
            cuda,
        })
    }

    fn new_decode_session(
        &self,
        capacity_tokens: usize,
        disallowed_token_ids: &[u32],
    ) -> CudaResult<CudaQwenDecodeSession> {
        let mut layer_states = Vec::with_capacity(self.layers.len());
        for layer in &self.layers {
            match layer {
                CudaQwenLayer::Attention(_) => layer_states.push(CudaQwenLayerState::Attention(
                    CudaQwenAttentionLayerState {
                        key_cache: self.cuda.alloc_bytes(
                            capacity_tokens
                                .checked_mul(self.attention_kv_width)
                                .and_then(|v| v.checked_mul(2))
                                .ok_or_else(|| "qwen attention key cache size overflow".to_string())?,
                        )?,
                        value_cache: self.cuda.alloc_bytes(
                            capacity_tokens
                                .checked_mul(self.attention_kv_width)
                                .and_then(|v| v.checked_mul(2))
                                .ok_or_else(|| "qwen attention value cache size overflow".to_string())?,
                        )?,
                        stored_tokens: 0,
                    },
                )),
                CudaQwenLayer::Recurrent(_) => {
                    let state_width = self
                        .recurrent_num_v_heads
                        .checked_mul(self.recurrent_head_v_dim)
                        .and_then(|v| v.checked_mul(self.recurrent_head_k_dim))
                        .ok_or_else(|| "qwen recurrent state width overflow".to_string())?;
                    let conv_state_width = self
                        .recurrent_qkv_width
                        .checked_mul(self.layers_conv_prefix())
                        .ok_or_else(|| "qwen recurrent conv state width overflow".to_string())?;
                    let gated_delta_len = self
                        .recurrent_v_width
                        .checked_add(state_width)
                        .ok_or_else(|| "qwen gated delta buffer length overflow".to_string())?;
                    let conv_state = self.cuda.alloc_f32(conv_state_width)?;
                    zero_buffer_f32(&self.cuda, &conv_state, conv_state_width)?;
                    let gated_delta = self.cuda.alloc_f32(gated_delta_len)?;
                    zero_buffer_f32(&self.cuda, &gated_delta, gated_delta_len)?;
                    layer_states.push(CudaQwenLayerState::Recurrent(
                        CudaQwenRecurrentLayerState {
                            conv_state,
                            gated_delta,
                        },
                    ));
                }
            }
        }
        let workspace = CudaQwenWorkspace {
            hidden_a: self.cuda.alloc_f32(self.hidden_size)?,
            hidden_b: self.cuda.alloc_f32(self.hidden_size)?,
            hidden_norm: self.cuda.alloc_f32(self.hidden_size)?,
            hidden_bf16: self.cuda.alloc_bytes(self.hidden_size * std::mem::size_of::<u16>())?,
            qg_out: self.cuda.alloc_f32(self.attention_qg_width)?,
            query: self
                .cuda
                .alloc_f32(self.attention_query_width.max(self.recurrent_q_width))?,
            gate: self.cuda.alloc_f32(self.attention_query_width)?,
            key: self
                .cuda
                .alloc_f32(self.attention_kv_width.max(self.recurrent_q_width))?,
            value: self.cuda.alloc_f32(self.attention_kv_width)?,
            attention_logits: self
                .cuda
                .alloc_f32(self.attention_heads * capacity_tokens)?,
            attn_out: self.cuda.alloc_f32(self.attention_query_width)?,
            attn_gated: self.cuda.alloc_f32(self.attention_query_width)?,
            attn_bf16: self
                .cuda
                .alloc_bytes(self.attention_query_width * std::mem::size_of::<u16>())?,
            attn_proj: self.cuda.alloc_f32(self.hidden_size)?,
            residual: self.cuda.alloc_f32(self.hidden_size)?,
            ffn_input: self.cuda.alloc_f32(self.hidden_size)?,
            moe_router_logits: self.cuda.alloc_f32(self.expert_count)?,
            moe_route_indices: self.cuda.alloc_u32(self.experts_used_count)?,
            moe_route_weights: self.cuda.alloc_f32(self.experts_used_count)?,
            moe_routed_accum: self.cuda.alloc_f32(self.hidden_size)?,
            moe_output: self.cuda.alloc_f32(self.hidden_size)?,
            moe_shared_gate_scalar: self.cuda.alloc_f32(1)?,
            moe_shared_gate: self.cuda.alloc_f32(self.shared_expert_intermediate)?,
            moe_shared_up: self.cuda.alloc_f32(self.shared_expert_intermediate)?,
            moe_shared_act: self.cuda.alloc_f32(self.shared_expert_intermediate)?,
            moe_shared_down: self.cuda.alloc_f32(self.hidden_size)?,
            moe_expert_gate_up: self.cuda.alloc_f32(self.expert_intermediate * 2)?,
            moe_expert_gate_up_batch: self
                .cuda
                .alloc_f32(self.experts_used_count * self.expert_intermediate * 2)?,
            moe_expert_gate: self.cuda.alloc_f32(self.expert_intermediate)?,
            moe_expert_up: self.cuda.alloc_f32(self.expert_intermediate)?,
            moe_expert_act: self.cuda.alloc_f32(self.expert_intermediate)?,
            moe_expert_act_batch: self
                .cuda
                .alloc_f32(self.experts_used_count * self.expert_intermediate)?,
            moe_expert_down: self.cuda.alloc_f32(self.hidden_size)?,
            moe_expert_down_batch: self
                .cuda
                .alloc_f32(self.experts_used_count * self.hidden_size)?,
            moe_expert_act_bf16: self
                .cuda
                .alloc_bytes(self.expert_intermediate * std::mem::size_of::<u16>())?,
            moe_expert_act_bf16_batch: self
                .cuda
                .alloc_bytes(
                    self.experts_used_count
                        * self.expert_intermediate
                        * std::mem::size_of::<u16>(),
                )?,
            recurrent_qkv: self.cuda.alloc_f32(self.recurrent_qkv_width)?,
            recurrent_gate_z: self.cuda.alloc_f32(self.recurrent_v_width)?,
            recurrent_beta_logits: self.cuda.alloc_f32(self.recurrent_num_v_heads)?,
            recurrent_beta: self.cuda.alloc_f32(self.recurrent_num_v_heads)?,
            recurrent_alpha: self.cuda.alloc_f32(self.recurrent_num_v_heads)?,
            recurrent_conv: self.cuda.alloc_f32(self.recurrent_qkv_width)?,
            recurrent_q: self.cuda.alloc_f32(self.recurrent_q_width)?,
            recurrent_k: self.cuda.alloc_f32(self.recurrent_q_width)?,
            recurrent_v: self.cuda.alloc_f32(self.recurrent_v_width)?,
            recurrent_decay: self.cuda.alloc_f32(self.recurrent_num_v_heads)?,
            recurrent_out_norm: self.cuda.alloc_f32(self.recurrent_v_width)?,
            recurrent_gated: self.cuda.alloc_f32(self.recurrent_v_width)?,
            recurrent_gated_bf16: self
                .cuda
                .alloc_bytes(self.recurrent_v_width * std::mem::size_of::<u16>())?,
            recurrent_proj: self.cuda.alloc_f32(self.hidden_size)?,
            final_norm: self.cuda.alloc_f32(self.hidden_size)?,
            logits: self.cuda.alloc_f32(self.vocab_size)?,
            argmax_out: self.cuda.alloc_u32(1)?,
            disallowed_token_ids: self.cuda.load_bytes(u32s_as_le_bytes(disallowed_token_ids))?,
        };
        Ok(CudaQwenDecodeSession {
            capacity_tokens,
            layer_states,
            workspace,
            disallowed_count: disallowed_token_ids.len(),
        })
    }

    fn alloc_graph_token_state(&self) -> CudaResult<CudaQwenGraphTokenState> {
        Ok(CudaQwenGraphTokenState {
            token_id: self.cuda.alloc_mapped_u32(1)?,
            position: self.cuda.alloc_mapped_u32(1)?,
            seq_len: self.cuda.alloc_mapped_u32(1)?,
            start_slot: self.cuda.alloc_mapped_u32(1)?,
            disallowed_count: self.cuda.alloc_mapped_u32(1)?,
        })
    }

    fn alloc_device_token_state(&self) -> CudaResult<CudaQwenDeviceTokenState> {
        Ok(CudaQwenDeviceTokenState {
            token_id: self.cuda.alloc_u32(1)?,
            position: self.cuda.alloc_u32(1)?,
            seq_len: self.cuda.alloc_u32(1)?,
            start_slot: self.cuda.alloc_u32(1)?,
            disallowed_count: self.cuda.alloc_u32(1)?,
        })
    }

    fn reset_decode_session(&self, session: &mut CudaQwenDecodeSession) -> CudaResult<()> {
        for layer_state in &mut session.layer_states {
            match layer_state {
                CudaQwenLayerState::Attention(state) => {
                    state.stored_tokens = 0;
                }
                CudaQwenLayerState::Recurrent(state) => {
                    zero_buffer_f32(
                        &self.cuda,
                        &state.conv_state,
                        self.recurrent_qkv_width * self.layers_conv_prefix(),
                    )?;
                    let state_width = self
                        .recurrent_num_v_heads
                        .checked_mul(self.recurrent_head_v_dim)
                        .and_then(|v| v.checked_mul(self.recurrent_head_k_dim))
                        .ok_or_else(|| "qwen recurrent state width overflow".to_string())?;
                    zero_buffer_f32(
                        &self.cuda,
                        &state.gated_delta,
                        self.recurrent_v_width + state_width,
                    )?;
                }
            }
        }
        Ok(())
    }

    fn write_graph_token_state(
        &self,
        token_state: &CudaQwenGraphTokenState,
        token_id: u32,
        position: usize,
        disallowed_count: usize,
    ) -> CudaResult<()> {
        let seq_len = position
            .checked_add(1)
            .ok_or_else(|| "qwen graph sequence length overflow".to_string())?;
        token_state.token_id.write_u32(0, token_id)?;
        token_state.position.write_u32(0, position as u32)?;
        token_state.seq_len.write_u32(0, seq_len as u32)?;
        token_state.start_slot.write_u32(0, 0)?;
        token_state
            .disallowed_count
            .write_u32(0, disallowed_count as u32)?;
        Ok(())
    }

    fn write_device_token_state(
        &self,
        token_state: &CudaQwenDeviceTokenState,
        token_id: u32,
        position: usize,
        disallowed_count: usize,
    ) -> CudaResult<()> {
        let seq_len = position
            .checked_add(1)
            .ok_or_else(|| "qwen graph sequence length overflow".to_string())?;
        self.cuda.write_u32(&token_state.token_id, token_id)?;
        self.cuda.write_u32(&token_state.position, position as u32)?;
        self.cuda.write_u32(&token_state.seq_len, seq_len as u32)?;
        self.cuda.write_u32(&token_state.start_slot, 0)?;
        self.cuda
            .write_u32(&token_state.disallowed_count, disallowed_count as u32)?;
        Ok(())
    }

    fn capture_prefill_graph(
        &self,
        session: &mut CudaQwenDecodeSession,
    ) -> CudaResult<CudaQwenPrefillGraph> {
        let token_state = self.alloc_device_token_state()?;
        self.reset_decode_session(session)?;
        self.write_device_token_state(&token_state, 0, 0, 0)?;
        self.cuda.begin_capture()?;
        self.eval_token_logits_graph_ptrs(
            session,
            token_state.token_id.device_u32_ptr(),
            token_state.position.device_u32_ptr(),
            token_state.seq_len.device_u32_ptr(),
            token_state.start_slot.device_u32_ptr(),
        )?;
        let exec = self
            .cuda
            .end_capture()?
            .instantiate()
            .map_err(|err| err.to_string())?;
        self.reset_decode_session(session)?;
        Ok(CudaQwenPrefillGraph { exec, token_state })
    }

    fn capture_decode_graph(
        &self,
        session: &mut CudaQwenDecodeSession,
    ) -> CudaResult<CudaQwenDecodeGraph> {
        let token_state = self.alloc_graph_token_state()?;
        let argmax_out = self.cuda.alloc_mapped_u32(1)?;
        self.reset_decode_session(session)?;
        self.write_graph_token_state(&token_state, 0, 0, 0)?;
        argmax_out.write_u32(0, 0)?;
        self.cuda.begin_capture()?;
        self.eval_token_logits_graph(session, &token_state)?;
        self.cuda.masked_argmax_f32_device_u32_ptr(
            &session.workspace.logits,
            &session.workspace.disallowed_token_ids,
            token_state.disallowed_count.device_u32_ptr(),
            argmax_out.device_u32_mut_ptr(),
            self.vocab_size,
        )?;
        let exec = self
            .cuda
            .end_capture()?
            .instantiate()
            .map_err(|err| err.to_string())?;
        self.reset_decode_session(session)?;
        Ok(CudaQwenDecodeGraph {
            exec,
            token_state,
            argmax_out,
        })
    }

    #[allow(dead_code)]
    fn capture_decode_chunk_graph(
        &self,
        session: &mut CudaQwenDecodeSession,
        step_count: usize,
    ) -> CudaResult<CudaQwenDecodeChunkGraph> {
        if step_count < 2 {
            return Err("qwen decode chunk graph requires at least 2 steps".to_string());
        }
        let input_token_id = self.cuda.alloc_mapped_u32(1)?;
        let positions = self.cuda.alloc_mapped_u32(step_count)?;
        let seq_lens = self.cuda.alloc_mapped_u32(step_count)?;
        let start_slots = self.cuda.alloc_mapped_u32(step_count)?;
        let disallowed_count = self.cuda.alloc_mapped_u32(1)?;
        let step_tokens = self.cuda.alloc_u32(step_count)?;
        input_token_id.write_u32(0, 0)?;
        for step in 0..step_count {
            positions.write_u32(step, step as u32)?;
            seq_lens.write_u32(step, (step + 1) as u32)?;
            start_slots.write_u32(step, 0)?;
        }
        disallowed_count.write_u32(0, 0)?;
        self.cuda.zero_bytes(
            &step_tokens,
            step_count
                .checked_mul(std::mem::size_of::<u32>())
                .ok_or_else(|| "qwen chunk graph token buffer size overflow".to_string())?,
        )?;
        self.reset_decode_session(session)?;
        self.cuda.begin_capture()?;
        for step in 0..step_count {
            let token_id_device_u32 = if step == 0 {
                input_token_id.device_u32_ptr()
            } else {
                unsafe { step_tokens.device_u32_ptr().add(step - 1) }
            };
            self.eval_token_logits_graph_ptrs(
                session,
                token_id_device_u32,
                unsafe { positions.device_u32_ptr().add(step) },
                unsafe { seq_lens.device_u32_ptr().add(step) },
                unsafe { start_slots.device_u32_ptr().add(step) },
            )?;
            self.cuda.masked_argmax_f32_device_u32_ptr(
                &session.workspace.logits,
                &session.workspace.disallowed_token_ids,
                disallowed_count.device_u32_ptr(),
                unsafe { step_tokens.device_u32_mut_ptr().add(step) },
                self.vocab_size,
            )?;
        }
        let exec = self
            .cuda
            .end_capture()?
            .instantiate()
            .map_err(|err| err.to_string())?;
        self.reset_decode_session(session)?;
        Ok(CudaQwenDecodeChunkGraph {
            exec,
            input_token_id,
            positions,
            seq_lens,
            start_slots,
            disallowed_count,
            step_tokens,
            step_count,
        })
    }

    fn write_decode_chunk_graph_state(
        &self,
        decode_chunk_graph: &CudaQwenDecodeChunkGraph,
        token_id: u32,
        position: usize,
        disallowed_count: usize,
    ) -> CudaResult<()> {
        decode_chunk_graph.input_token_id.write_u32(0, token_id)?;
        decode_chunk_graph
            .disallowed_count
            .write_u32(0, disallowed_count as u32)?;
        for step in 0..decode_chunk_graph.step_count {
            let step_position = position
                .checked_add(step)
                .ok_or_else(|| "qwen chunk graph position overflow".to_string())?;
            let step_seq_len = step_position
                .checked_add(1)
                .ok_or_else(|| "qwen chunk graph sequence length overflow".to_string())?;
            decode_chunk_graph
                .positions
                .write_u32(step, step_position as u32)?;
            decode_chunk_graph
                .seq_lens
                .write_u32(step, step_seq_len as u32)?;
            decode_chunk_graph.start_slots.write_u32(step, 0)?;
        }
        Ok(())
    }

    fn set_attention_stored_tokens(
        &self,
        session: &mut CudaQwenDecodeSession,
        stored_tokens: usize,
    ) {
        for layer_state in &mut session.layer_states {
            if let CudaQwenLayerState::Attention(state) = layer_state {
                state.stored_tokens = stored_tokens;
            }
        }
    }

    fn eval_and_select_graph(
        &self,
        session: &mut CudaQwenDecodeSession,
        decode_graph: &CudaQwenDecodeGraph,
        token_id: u32,
        position: usize,
        do_sample: bool,
        disallowed_token_ids: &[u32],
        sampling: &QwenSamplingOptions,
        rng: &mut QwenSamplingRng,
    ) -> CudaResult<u32> {
        if position >= session.capacity_tokens {
            return Err(format!(
                "qwen cuda session capacity {} exceeded by position {}",
                session.capacity_tokens, position
            ));
        }
        self.write_graph_token_state(
            &decode_graph.token_state,
            token_id,
            position,
            disallowed_token_ids.len(),
        )?;
        self.cuda.launch_graph(&decode_graph.exec)?;
        self.set_attention_stored_tokens(session, position + 1);
        self.cuda.synchronize()?;
        if do_sample {
            let logits = self
                .cuda
                .read_f32s(&session.workspace.logits, self.vocab_size)?;
            return sample_token_from_logits_f32(logits.as_slice(), disallowed_token_ids, sampling, rng)
                .map(|token| token.token_id);
        }
        decode_graph.argmax_out.read_u32(0)
    }

    fn eval_token_chunk_graph(
        &self,
        session: &mut CudaQwenDecodeSession,
        decode_chunk_graph: &CudaQwenDecodeChunkGraph,
        token_id: u32,
        position: usize,
        disallowed_token_ids: &[u32],
    ) -> CudaResult<Vec<u32>> {
        self.write_decode_chunk_graph_state(
            decode_chunk_graph,
            token_id,
            position,
            disallowed_token_ids.len(),
        )?;
        self.cuda.launch_graph(&decode_chunk_graph.exec)?;
        self.set_attention_stored_tokens(session, position + decode_chunk_graph.step_count);
        self.cuda.synchronize()?;
        self.cuda
            .read_u32s(&decode_chunk_graph.step_tokens, decode_chunk_graph.step_count)
    }

    fn layers_conv_prefix(&self) -> usize {
        self.layers
            .iter()
            .find_map(|layer| match layer {
                CudaQwenLayer::Recurrent(recurrent) => {
                    Some(recurrent.ssm_conv1d.size_bytes() / std::mem::size_of::<f32>() / self.recurrent_qkv_width - 1)
                }
                CudaQwenLayer::Attention(_) => None,
            })
            .unwrap_or(0)
    }

    fn eval_token_logits(
        &self,
        session: &mut CudaQwenDecodeSession,
        token_id: u32,
        position: usize,
    ) -> CudaResult<()> {
        let trace_layers =
            std::env::var("MAKEPAD_MLX_QWEN_TRACE_LAYERS").ok().as_deref() == Some("1");
        if position >= session.capacity_tokens {
            return Err(format!(
                "qwen cuda session capacity {} exceeded by position {}",
                session.capacity_tokens, position
            ));
        }
        self.token_embd.get_row(
            &self.cuda,
            token_id as usize,
            &session.workspace.hidden_a,
        )?;
        let mut hidden_is_a = true;
        for (layer_index, (layer, state)) in self
            .layers
            .iter()
            .zip(session.layer_states.iter_mut())
            .enumerate()
        {
            if trace_layers {
                let kind = match layer {
                    CudaQwenLayer::Attention(_) => "attention",
                    CudaQwenLayer::Recurrent(_) => "recurrent",
                };
                eprintln!("[qwen-layer-trace] enter layer={layer_index} kind={kind}");
            }
            match (layer, state) {
                (CudaQwenLayer::Attention(layer), CudaQwenLayerState::Attention(state)) => {
                    self.eval_attention_layer(
                        layer,
                        state,
                        &mut session.workspace,
                        position,
                        hidden_is_a,
                    )?;
                }
                (CudaQwenLayer::Recurrent(layer), CudaQwenLayerState::Recurrent(state)) => {
                    self.eval_recurrent_layer(
                        layer,
                        state,
                        &mut session.workspace,
                        hidden_is_a,
                    )?;
                }
                _ => return Err("qwen cuda layer/state mismatch".to_string()),
            }
            if trace_layers {
                eprintln!("[qwen-layer-trace] done layer={layer_index}");
            }
            hidden_is_a = !hidden_is_a;
        }
        let final_hidden = if hidden_is_a {
            &session.workspace.hidden_a
        } else {
            &session.workspace.hidden_b
        };
        self.cuda.rms_norm_row_weighted_f32_f32weights_precise(
            final_hidden,
            &self.output_norm,
            &session.workspace.final_norm,
            self.hidden_size,
            self.rms_norm_eps,
        )?;
        self.cuda
            .f32_to_bf16(&session.workspace.final_norm, &session.workspace.hidden_bf16, self.hidden_size)?;
        self.output.matvec(
            &self.cuda,
            &session.workspace.hidden_bf16,
            &session.workspace.logits,
        )?;
        Ok(())
    }

    fn eval_token_logits_graph_ptrs(
        &self,
        session: &mut CudaQwenDecodeSession,
        token_id_device_u32: *const u32,
        position_device_u32: *const u32,
        seq_len_device_u32: *const u32,
        start_slot_device_u32: *const u32,
    ) -> CudaResult<()> {
        self.token_embd.get_row_device_u32_ptr(
            &self.cuda,
            token_id_device_u32,
            &session.workspace.hidden_a,
        )?;
        let mut hidden_is_a = true;
        for (layer, state) in self.layers.iter().zip(session.layer_states.iter_mut()) {
            match (layer, state) {
                (CudaQwenLayer::Attention(layer), CudaQwenLayerState::Attention(state)) => {
                    self.eval_attention_layer_graph(
                        layer,
                        state,
                        &mut session.workspace,
                        position_device_u32,
                        seq_len_device_u32,
                        start_slot_device_u32,
                        hidden_is_a,
                    )?;
                }
                (CudaQwenLayer::Recurrent(layer), CudaQwenLayerState::Recurrent(state)) => {
                    self.eval_recurrent_layer_graph(
                        layer,
                        state,
                        &mut session.workspace,
                        hidden_is_a,
                    )?;
                }
                _ => return Err("qwen cuda layer/state mismatch".to_string()),
            }
            hidden_is_a = !hidden_is_a;
        }
        let final_hidden = if hidden_is_a {
            &session.workspace.hidden_a
        } else {
            &session.workspace.hidden_b
        };
        self.cuda.rms_norm_row_weighted_f32_f32weights_precise(
            final_hidden,
            &self.output_norm,
            &session.workspace.final_norm,
            self.hidden_size,
            self.rms_norm_eps,
        )?;
        self.cuda.f32_to_bf16(
            &session.workspace.final_norm,
            &session.workspace.hidden_bf16,
            self.hidden_size,
        )?;
        self.output.matvec(
            &self.cuda,
            &session.workspace.hidden_bf16,
            &session.workspace.logits,
        )?;
        Ok(())
    }

    fn eval_token_logits_graph(
        &self,
        session: &mut CudaQwenDecodeSession,
        token_state: &CudaQwenGraphTokenState,
    ) -> CudaResult<()> {
        self.eval_token_logits_graph_ptrs(
            session,
            token_state.token_id.device_u32_ptr(),
            token_state.position.device_u32_ptr(),
            token_state.seq_len.device_u32_ptr(),
            token_state.start_slot.device_u32_ptr(),
        )
    }

    fn eval_token_logits_with_layer_compare(
        &self,
        session: &mut CudaQwenDecodeSession,
        token_id: u32,
        position: usize,
        runtime_session: &MlxQwen35MoeRuntimeSession,
        reference_state_before: &MlxQwen35MoeDecodeState,
    ) -> CudaResult<()> {
        if position >= session.capacity_tokens {
            return Err(format!(
                "qwen cuda session capacity {} exceeded by position {}",
                session.capacity_tokens, position
            ));
        }
        self.token_embd.get_row(
            &self.cuda,
            token_id as usize,
            &session.workspace.hidden_a,
        )?;
        let mut reference_state = reference_state_before.clone();
        let mut reference_hidden = runtime_session
            .token_embedding_f32(token_id)
            .map_err(|err| err.to_string())?;
        let mut hidden_is_a = true;
        let debug_moe_layer = std::env::var("MAKEPAD_MLX_QWEN_COMPARE_MOE_LAYER")
            .ok()
            .and_then(|value| value.parse::<usize>().ok());
        let debug_recurrent_layer = std::env::var("MAKEPAD_MLX_QWEN_COMPARE_RECURRENT_LAYER")
            .ok()
            .and_then(|value| value.parse::<usize>().ok());
        let debug_attention_layer = std::env::var("MAKEPAD_MLX_QWEN_COMPARE_ATTENTION_LAYER")
            .ok()
            .and_then(|value| value.parse::<usize>().ok());
        for (layer_index, (layer, state)) in self
            .layers
            .iter()
            .zip(session.layer_states.iter_mut())
            .enumerate()
        {
            let mut reference_ffn = None;
            let mut reference_ffn_input = None;
            let mut reference_recurrent = None;
            let mut reference_attention = None;
            if debug_attention_layer == Some(layer_index) {
                let reference_layer = runtime_session
                    .tensors
                    .layers
                    .get(layer_index)
                    .ok_or_else(|| format!("qwen reference layer {} out of range", layer_index))?;
                if matches!(
                    reference_state.layers.get(layer_index),
                    Some(MlxQwen35MoeLayerDecodeState::Attention(_))
                ) {
                    reference_attention = Some(debug_attention_reference(
                        runtime_session,
                        reference_layer,
                        &reference_hidden,
                        position,
                    )?);
                }
            }
            if debug_recurrent_layer == Some(layer_index) {
                let reference_layer = runtime_session
                    .tensors
                    .layers
                    .get(layer_index)
                    .ok_or_else(|| format!("qwen reference layer {} out of range", layer_index))?;
                if let Some(MlxQwen35MoeLayerDecodeState::Recurrent(reference_state_before_layer)) =
                    reference_state.layers.get(layer_index)
                {
                    reference_recurrent = Some(debug_recurrent_reference(
                        runtime_session,
                        reference_layer,
                        &reference_hidden,
                        reference_state_before_layer,
                    )?);
                }
            }
            if debug_moe_layer == Some(layer_index) {
                let reference_layer = runtime_session
                    .tensors
                    .layers
                    .get(layer_index)
                    .ok_or_else(|| format!("qwen reference layer {} out of range", layer_index))?;
                let attn_input = runtime_session
                    .rms_norm_weighted_f32(
                        &reference_hidden,
                        &reference_layer.attn_norm,
                        runtime_session.weights.snapshot.config.text_config.rms_norm_eps,
                    )
                    .map_err(|err| err.to_string())?;
                let attn_out = match reference_state.layers.get_mut(layer_index) {
                    Some(MlxQwen35MoeLayerDecodeState::Attention(state)) => runtime_session
                        .apply_attention_layer_decode_reference_f32(
                            reference_layer,
                            &attn_input,
                            position,
                            state,
                        )
                        .map_err(|err| err.to_string())?,
                    Some(MlxQwen35MoeLayerDecodeState::Recurrent(state)) => runtime_session
                        .apply_recurrent_layer_decode_reference_f32(
                            reference_layer,
                            &attn_input,
                            state,
                        )
                        .map_err(|err| err.to_string())?,
                    None => {
                        return Err(
                            format!("missing reference decode state for layer {}", layer_index)
                        )
                    }
                };
                add_residual_in_place(&mut reference_hidden, &attn_out)
                    .map_err(|err| err.to_string())?;
                let ffn_input = runtime_session
                    .rms_norm_weighted_f32(
                        &reference_hidden,
                        &reference_layer.post_attention_norm,
                        runtime_session.weights.snapshot.config.text_config.rms_norm_eps,
                    )
                    .map_err(|err| err.to_string())?;
                reference_ffn_input = Some(ffn_input.clone());
                let ffn_out = runtime_session
                    .apply_moe_ffn_reference_f32(reference_layer.index, &ffn_input)
                    .map_err(|err| err.to_string())?;
                add_residual_in_place(&mut reference_hidden, &ffn_out.output)
                    .map_err(|err| err.to_string())?;
                reference_ffn = Some(ffn_out);
            } else {
                runtime_session
                    .apply_layer_decode_reference_f32(
                        layer_index,
                        position,
                        &mut reference_hidden,
                        &mut reference_state,
                    )
                    .map_err(|err| err.to_string())?;
            }
            match (layer, state) {
                (CudaQwenLayer::Attention(layer), CudaQwenLayerState::Attention(state)) => {
                    self.eval_attention_layer(
                        layer,
                        state,
                        &mut session.workspace,
                        position,
                        hidden_is_a,
                    )?;
                    if let Some(reference_attention) = &reference_attention {
                        let actual_hidden_norm = self
                            .cuda
                            .read_f32s(&session.workspace.hidden_norm, self.hidden_size)?;
                        let actual_value = self
                            .cuda
                            .read_f32s(&session.workspace.value, self.attention_kv_width)?;
                        let actual_key_rope =
                            self.cuda.read_f32s(&session.workspace.key, self.attention_kv_width)?;
                        let actual_key_cache_words = bf16_words_from_le_bytes(
                            &self
                                .cuda
                                .read_bytes(
                                    &state.key_cache,
                                    self.attention_kv_width * std::mem::size_of::<u16>(),
                                )?,
                        )?;
                        let actual_key_cache = bf16_words_to_f32(&actual_key_cache_words);
                        eprintln!(
                            "[qwen-attention-compare] position={position} layer={layer_index} hidden_norm={} value={} key_rope={} key_cache={}",
                            max_abs_diff(&actual_hidden_norm, &reference_attention.hidden_norm),
                            max_abs_diff(&actual_value, &reference_attention.value),
                            max_abs_diff(&actual_key_rope, &reference_attention.key_rope),
                            max_abs_diff(&actual_key_cache, &round_slice_to_bf16(&reference_attention.key_rope)),
                        );
                    }
                }
                (CudaQwenLayer::Recurrent(layer), CudaQwenLayerState::Recurrent(state)) => {
                    self.eval_recurrent_layer(layer, state, &mut session.workspace, hidden_is_a)?;
                    if let Some(reference_recurrent) = &reference_recurrent {
                        let actual_hidden_norm = self
                            .cuda
                            .read_f32s(&session.workspace.hidden_norm, self.hidden_size)?;
                        let actual_qkv = self
                            .cuda
                            .read_f32s(&session.workspace.recurrent_qkv, self.recurrent_qkv_width)?;
                        let actual_gate_z = self
                            .cuda
                            .read_f32s(&session.workspace.recurrent_gate_z, self.recurrent_v_width)?;
                        let actual_beta_logits = self
                            .cuda
                            .read_f32s(
                                &session.workspace.recurrent_beta_logits,
                                self.recurrent_num_v_heads,
                            )?;
                        let actual_alpha = self
                            .cuda
                            .read_f32s(&session.workspace.recurrent_alpha, self.recurrent_num_v_heads)?;
                        let actual_conv_out = self
                            .cuda
                            .read_f32s(&session.workspace.recurrent_conv, self.recurrent_qkv_width)?;
                        let actual_q_raw = self
                            .cuda
                            .read_f32s(&session.workspace.recurrent_q, self.recurrent_q_width)?;
                        let actual_k_raw = self
                            .cuda
                            .read_f32s(&session.workspace.recurrent_k, self.recurrent_q_width)?;
                        let actual_v = self
                            .cuda
                            .read_f32s(&session.workspace.recurrent_v, self.recurrent_v_width)?;
                        let actual_query_kernel = self
                            .cuda
                            .read_f32s(&session.workspace.query, self.recurrent_q_width)?;
                        let actual_key_kernel = self
                            .cuda
                            .read_f32s(&session.workspace.key, self.recurrent_q_width)?;
                        let actual_beta = self
                            .cuda
                            .read_f32s(&session.workspace.recurrent_beta, self.recurrent_num_v_heads)?;
                        let actual_decay_log = self
                            .cuda
                            .read_f32s(&session.workspace.recurrent_decay, self.recurrent_num_v_heads)?;
                        let actual_recurrent_out =
                            self.cuda.read_f32s(&state.gated_delta, self.recurrent_v_width)?;
                        let actual_out_norm = self.cuda.read_f32s(
                            &session.workspace.recurrent_out_norm,
                            self.recurrent_v_width,
                        )?;
                        let actual_gated = self.cuda.read_f32s(
                            &session.workspace.recurrent_gated,
                            self.recurrent_v_width,
                        )?;
                        let actual_proj = self
                            .cuda
                            .read_f32s(&session.workspace.recurrent_proj, self.hidden_size)?;
                        eprintln!(
                            "[qwen-recurrent-compare] position={position} layer={layer_index} hidden_norm={} qkv={} gate_z={} beta_logits={} alpha={} conv={} q_raw={} k_raw={} v={} query={} key={} beta={} decay_log={} recurrent_out={} out_norm={} gated={} proj={}",
                            max_abs_diff(&actual_hidden_norm, &reference_recurrent.hidden_norm),
                            max_abs_diff(&actual_qkv, &reference_recurrent.qkv),
                            max_abs_diff(&actual_gate_z, &reference_recurrent.gate_z),
                            max_abs_diff(&actual_beta_logits, &reference_recurrent.beta_logits),
                            max_abs_diff(&actual_alpha, &reference_recurrent.alpha),
                            max_abs_diff(&actual_conv_out, &reference_recurrent.conv_out),
                            max_abs_diff(&actual_q_raw, &reference_recurrent.q_raw),
                            max_abs_diff(&actual_k_raw, &reference_recurrent.k_raw),
                            max_abs_diff(&actual_v, &reference_recurrent.v),
                            max_abs_diff(&actual_query_kernel, &reference_recurrent.query_kernel),
                            max_abs_diff(&actual_key_kernel, &reference_recurrent.key_kernel),
                            max_abs_diff(&actual_beta, &reference_recurrent.beta),
                            max_abs_diff(&actual_decay_log, &reference_recurrent.decay_log),
                            max_abs_diff(&actual_recurrent_out, &reference_recurrent.recurrent_out),
                            max_abs_diff(&actual_out_norm, &reference_recurrent.recurrent_out_norm),
                            max_abs_diff(&actual_gated, &reference_recurrent.recurrent_gated),
                            max_abs_diff(&actual_proj, &reference_recurrent.recurrent_proj),
                        );
                    }
                }
                _ => return Err("qwen cuda layer/state mismatch".to_string()),
            }
            hidden_is_a = !hidden_is_a;
            let exact_hidden = if hidden_is_a {
                self.cuda
                    .read_f32s(&session.workspace.hidden_a, self.hidden_size)?
            } else {
                self.cuda
                    .read_f32s(&session.workspace.hidden_b, self.hidden_size)?
            };
            eprintln!(
                "[qwen-layer-compare] position={position} layer={layer_index} max_abs_diff={}",
                max_abs_diff(&exact_hidden, &reference_hidden)
            );
            if let (Some(reference_ffn), Some(reference_ffn_input)) =
                (reference_ffn, reference_ffn_input)
            {
                let exact_ffn_input = self
                    .cuda
                    .read_f32s(&session.workspace.ffn_input, self.hidden_size)?;
                let exact_router_logits = self
                    .cuda
                    .read_f32s(&session.workspace.moe_router_logits, self.expert_count)?;
                let (_exact_router_probabilities, exact_routes) =
                    softmax_top_k_routes(&exact_router_logits, self.experts_used_count)?;
                let exact_routed_output = self
                    .cuda
                    .read_f32s(&session.workspace.moe_routed_accum, self.hidden_size)?;
                let exact_shared_output = self
                    .cuda
                    .read_f32s(&session.workspace.moe_shared_down, self.hidden_size)?;
                let exact_moe_output = self
                    .cuda
                    .read_f32s(&session.workspace.moe_output, self.hidden_size)?;
                let exact_shared_gate = self
                    .cuda
                    .read_f32s(&session.workspace.moe_shared_gate_scalar, 1)?
                    .into_iter()
                    .next()
                    .ok_or_else(|| "missing qwen moe shared gate scalar".to_string())?;
                eprintln!(
                    "[qwen-moe-compare] position={position} layer={layer_index} ffn_input_max_abs_diff={} router_max_abs_diff={} routed_output_max_abs_diff={} shared_output_max_abs_diff={} output_max_abs_diff={} shared_gate_ref={} shared_gate_cuda={} ref_routes={:?} cuda_routes={:?}",
                    max_abs_diff(&exact_ffn_input, &reference_ffn_input),
                    max_abs_diff(&exact_router_logits, &reference_ffn.router_logits),
                    max_abs_diff(&exact_routed_output, &reference_ffn.routed_output),
                    max_abs_diff(&exact_shared_output, &reference_ffn.shared_output),
                    max_abs_diff(&exact_moe_output, &reference_ffn.output),
                    reference_ffn.shared_gate,
                    sigmoid_f32(exact_shared_gate),
                    reference_ffn
                        .routed_experts
                        .iter()
                        .map(|route| (route.expert_index, route.weight))
                        .collect::<Vec<_>>(),
                    exact_routes
                        .iter()
                        .map(|route| (route.expert_index, route.weight))
                        .collect::<Vec<_>>(),
                );
            }
        }
        let final_hidden = if hidden_is_a {
            &session.workspace.hidden_a
        } else {
            &session.workspace.hidden_b
        };
        self.cuda.rms_norm_row_weighted_f32_f32weights_precise(
            final_hidden,
            &self.output_norm,
            &session.workspace.final_norm,
            self.hidden_size,
            self.rms_norm_eps,
        )?;
        self.cuda
            .f32_to_bf16(&session.workspace.final_norm, &session.workspace.hidden_bf16, self.hidden_size)?;
        self.output.matvec(
            &self.cuda,
            &session.workspace.hidden_bf16,
            &session.workspace.logits,
        )?;
        Ok(())
    }

    fn eval_attention_layer(
        &self,
        layer: &CudaQwenAttentionLayer,
        state: &mut CudaQwenAttentionLayerState,
        workspace: &mut CudaQwenWorkspace,
        position: usize,
        input_is_a: bool,
    ) -> CudaResult<()> {
        let (input_hidden, _output_hidden) = if input_is_a {
            (&workspace.hidden_a, &workspace.hidden_b)
        } else {
            (&workspace.hidden_b, &workspace.hidden_a)
        };
        self.cuda.rms_norm_row_weighted_f32_f32weights_precise(
            input_hidden,
            &layer.attn_norm,
            &workspace.hidden_norm,
            self.hidden_size,
            self.rms_norm_eps,
        )?;
        self.cuda
            .f32_to_bf16(&workspace.hidden_norm, &workspace.hidden_bf16, self.hidden_size)?;
        layer
            .wq
            .matvec(&self.cuda, &workspace.hidden_bf16, &workspace.qg_out)?;
        layer
            .wk
            .matvec(&self.cuda, &workspace.hidden_bf16, &workspace.key)?;
        layer
            .wv
            .matvec(&self.cuda, &workspace.hidden_bf16, &workspace.value)?;
        self.cuda.qwen_split_interleaved_query_gate_f32(
            &workspace.qg_out,
            &workspace.query,
            &workspace.gate,
            self.attention_heads,
            self.attention_head_dim,
        )?;
        self.cuda.rms_norm_rows_weighted_f32_f32weights_precise(
            &workspace.query,
            &layer.q_norm,
            &workspace.query,
            self.attention_heads,
            self.attention_head_dim,
            self.attention_head_dim,
            self.rms_norm_eps,
        )?;
        self.cuda.rms_norm_rows_weighted_f32_f32weights_precise(
            &workspace.key,
            &layer.k_norm,
            &workspace.key,
            self.attention_kv_heads,
            self.attention_head_dim,
            self.attention_head_dim,
            self.rms_norm_eps,
        )?;
        let positions = qwen_text_mrope_positions(position as u32);
        self.cuda.qwen_mrope_rows_f32(
            &workspace.query,
            &workspace.query,
            self.attention_heads,
            self.attention_head_dim,
            self.rotary_dim,
            self.rope_theta,
            positions,
            self.rope_sections4,
        )?;
        self.cuda.qwen_mrope_rows_f32(
            &workspace.key,
            &workspace.key,
            self.attention_kv_heads,
            self.attention_head_dim,
            self.rotary_dim,
            self.rope_theta,
            positions,
            self.rope_sections4,
        )?;
        self.cuda.kv_append_f32(
            &workspace.key,
            &workspace.value,
            &state.key_cache,
            &state.value_cache,
            self.attention_kv_heads,
            self.attention_head_dim,
            session_capacity_tokens(state, self.attention_kv_width),
            position,
        )?;
        state.stored_tokens = position + 1;
        let kv_row_stride = session_capacity_tokens(state, self.attention_kv_width)
            .checked_mul(self.attention_head_dim)
            .ok_or_else(|| "qwen attention kv row stride overflow".to_string())?;
        self.cuda.attention_logits_seq_f32(
            &workspace.query,
            &state.key_cache,
            &workspace.attention_logits,
            self.attention_heads,
            self.attention_q_heads_per_kv,
            self.attention_head_dim,
            kv_row_stride,
            state.stored_tokens,
            0,
            session_capacity_tokens(state, self.attention_kv_width),
            session_capacity_tokens(state, self.attention_kv_width),
        )?;
        self.cuda.attention_softmax_weighted_sum_f32(
            &workspace.attention_logits,
            &state.value_cache,
            &workspace.attn_out,
            self.attention_heads,
            self.attention_q_heads_per_kv,
            self.attention_head_dim,
            kv_row_stride,
            state.stored_tokens,
            0,
            session_capacity_tokens(state, self.attention_kv_width),
            session_capacity_tokens(state, self.attention_kv_width),
            self.attention_head_dim,
        )?;
        self.cuda.qwen_sigmoid_mul_f32(
            &workspace.attn_out,
            &workspace.gate,
            &workspace.attn_gated,
            self.attention_query_width,
        )?;
        self.cuda
            .f32_to_bf16(&workspace.attn_gated, &workspace.attn_bf16, self.attention_query_width)?;
        layer
            .wo
            .matvec(&self.cuda, &workspace.attn_bf16, &workspace.attn_proj)?;
        self.cuda
            .add_f32(input_hidden, &workspace.attn_proj, &workspace.residual, self.hidden_size)?;
        self.eval_moe_device(
            &layer.moe,
            &layer.post_attention_norm,
            workspace,
            !input_is_a,
        )
    }

    fn eval_attention_layer_graph(
        &self,
        layer: &CudaQwenAttentionLayer,
        _state: &mut CudaQwenAttentionLayerState,
        workspace: &mut CudaQwenWorkspace,
        position_device_u32: *const u32,
        seq_len_device_u32: *const u32,
        start_slot_device_u32: *const u32,
        input_is_a: bool,
    ) -> CudaResult<()> {
        let (input_hidden, _output_hidden) = if input_is_a {
            (&workspace.hidden_a, &workspace.hidden_b)
        } else {
            (&workspace.hidden_b, &workspace.hidden_a)
        };
        self.cuda.rms_norm_row_weighted_f32_f32weights_precise(
            input_hidden,
            &layer.attn_norm,
            &workspace.hidden_norm,
            self.hidden_size,
            self.rms_norm_eps,
        )?;
        self.cuda
            .f32_to_bf16(&workspace.hidden_norm, &workspace.hidden_bf16, self.hidden_size)?;
        layer
            .wq
            .matvec(&self.cuda, &workspace.hidden_bf16, &workspace.qg_out)?;
        layer
            .wk
            .matvec(&self.cuda, &workspace.hidden_bf16, &workspace.key)?;
        layer
            .wv
            .matvec(&self.cuda, &workspace.hidden_bf16, &workspace.value)?;
        self.cuda.qwen_split_interleaved_query_gate_f32(
            &workspace.qg_out,
            &workspace.query,
            &workspace.gate,
            self.attention_heads,
            self.attention_head_dim,
        )?;
        self.cuda.rms_norm_rows_weighted_f32_f32weights_precise(
            &workspace.query,
            &layer.q_norm,
            &workspace.query,
            self.attention_heads,
            self.attention_head_dim,
            self.attention_head_dim,
            self.rms_norm_eps,
        )?;
        self.cuda.rms_norm_rows_weighted_f32_f32weights_precise(
            &workspace.key,
            &layer.k_norm,
            &workspace.key,
            self.attention_kv_heads,
            self.attention_head_dim,
            self.attention_head_dim,
            self.rms_norm_eps,
        )?;
        self.cuda.qwen_mrope_rows_f32_device_u32_ptr(
            &workspace.query,
            &workspace.query,
            self.attention_heads,
            self.attention_head_dim,
            self.rotary_dim,
            self.rope_theta,
            position_device_u32,
            self.rope_sections4,
        )?;
        self.cuda.qwen_mrope_rows_f32_device_u32_ptr(
            &workspace.key,
            &workspace.key,
            self.attention_kv_heads,
            self.attention_head_dim,
            self.rotary_dim,
            self.rope_theta,
            position_device_u32,
            self.rope_sections4,
        )?;
        self.cuda.kv_append_f32_device_u32_ptr(
            &workspace.key,
            &workspace.value,
            &_state.key_cache,
            &_state.value_cache,
            self.attention_kv_heads,
            self.attention_head_dim,
            session_capacity_tokens(_state, self.attention_kv_width),
            position_device_u32,
        )?;
        let kv_row_stride = session_capacity_tokens(_state, self.attention_kv_width)
            .checked_mul(self.attention_head_dim)
            .ok_or_else(|| "qwen attention kv row stride overflow".to_string())?;
        self.cuda.attention_logits_seq_f32_device_u32_ptr(
            &workspace.query,
            &_state.key_cache,
            &workspace.attention_logits,
            self.attention_heads,
            self.attention_q_heads_per_kv,
            self.attention_head_dim,
            kv_row_stride,
            seq_len_device_u32,
            start_slot_device_u32,
            session_capacity_tokens(_state, self.attention_kv_width),
            session_capacity_tokens(_state, self.attention_kv_width),
        )?;
        self.cuda.attention_softmax_weighted_sum_f32_device_u32_ptr(
            &workspace.attention_logits,
            &_state.value_cache,
            &workspace.attn_out,
            self.attention_heads,
            self.attention_q_heads_per_kv,
            self.attention_head_dim,
            kv_row_stride,
            seq_len_device_u32,
            start_slot_device_u32,
            session_capacity_tokens(_state, self.attention_kv_width),
            session_capacity_tokens(_state, self.attention_kv_width),
            self.attention_head_dim,
        )?;
        self.cuda.qwen_sigmoid_mul_f32(
            &workspace.attn_out,
            &workspace.gate,
            &workspace.attn_gated,
            self.attention_query_width,
        )?;
        self.cuda
            .f32_to_bf16(&workspace.attn_gated, &workspace.attn_bf16, self.attention_query_width)?;
        layer
            .wo
            .matvec(&self.cuda, &workspace.attn_bf16, &workspace.attn_proj)?;
        self.cuda
            .add_f32(input_hidden, &workspace.attn_proj, &workspace.residual, self.hidden_size)?;
        self.eval_moe_device(
            &layer.moe,
            &layer.post_attention_norm,
            workspace,
            !input_is_a,
        )
    }

    fn eval_recurrent_layer(
        &self,
        layer: &CudaQwenRecurrentLayer,
        state: &mut CudaQwenRecurrentLayerState,
        workspace: &mut CudaQwenWorkspace,
        input_is_a: bool,
    ) -> CudaResult<()> {
        let (input_hidden, _output_hidden) = if input_is_a {
            (&workspace.hidden_a, &workspace.hidden_b)
        } else {
            (&workspace.hidden_b, &workspace.hidden_a)
        };
        self.cuda.rms_norm_row_weighted_f32_f32weights_precise(
            input_hidden,
            &layer.attn_norm,
            &workspace.hidden_norm,
            self.hidden_size,
            self.rms_norm_eps,
        )?;
        self.cuda
            .f32_to_bf16(&workspace.hidden_norm, &workspace.hidden_bf16, self.hidden_size)?;
        layer
            .wqkv
            .matvec(&self.cuda, &workspace.hidden_bf16, &workspace.recurrent_qkv)?;
        layer
            .wqkv_gate
            .matvec(&self.cuda, &workspace.hidden_bf16, &workspace.recurrent_gate_z)?;
        layer.ssm_beta.matvec(
            &self.cuda,
            &workspace.hidden_bf16,
            &workspace.recurrent_beta_logits,
        )?;
        layer
            .ssm_alpha
            .matvec(&self.cuda, &workspace.hidden_bf16, &workspace.recurrent_alpha)?;
        self.cuda.qwen_ssm_conv_with_state_f32(
            &workspace.recurrent_qkv,
            &state.conv_state,
            &layer.ssm_conv1d,
            &workspace.recurrent_conv,
            self.layers_conv_prefix() + 1,
            self.recurrent_qkv_width,
        )?;
        self.cuda.qwen_split_recurrent_qkv_f32(
            &workspace.recurrent_conv,
            &workspace.recurrent_q,
            &workspace.recurrent_k,
            &workspace.recurrent_v,
            self.recurrent_q_width,
            self.recurrent_v_width,
        )?;
        self.cuda.rms_norm_rows_no_scale_f32_precise(
            &workspace.recurrent_q,
            &workspace.query,
            self.recurrent_num_k_heads,
            self.recurrent_head_k_dim,
            self.recurrent_head_k_dim,
            self.rms_norm_eps,
        )?;
        self.cuda.rms_norm_rows_no_scale_f32_precise(
            &workspace.recurrent_k,
            &workspace.key,
            self.recurrent_num_k_heads,
            self.recurrent_head_k_dim,
            self.recurrent_head_k_dim,
            self.rms_norm_eps,
        )?;
        let inv_scale = (self.recurrent_head_k_dim as f32).sqrt().recip();
        self.cuda
            .scale_f32_inplace(&workspace.query, inv_scale, self.recurrent_q_width)?;
        self.cuda
            .scale_f32_inplace(&workspace.key, inv_scale, self.recurrent_q_width)?;
        self.cuda.qwen_sigmoid_f32(
            &workspace.recurrent_beta_logits,
            &workspace.recurrent_beta,
            self.recurrent_num_v_heads,
        )?;
        self.cuda.qwen_decay_gate_f32(
            &layer.ssm_a,
            &workspace.recurrent_alpha,
            &layer.ssm_dt,
            &workspace.recurrent_decay,
            self.recurrent_num_v_heads,
        )?;
        self.cuda.gated_delta_net_f32_state_offset(
            &workspace.query,
            &workspace.key,
            &workspace.recurrent_v,
            &workspace.recurrent_decay,
            &workspace.recurrent_beta,
            &state.gated_delta,
            self.recurrent_v_width,
            self.recurrent_head_k_dim,
            self.recurrent_num_v_heads,
            1,
            1,
            self.recurrent_head_k_dim,
            self.recurrent_q_width,
            self.recurrent_q_width,
            self.recurrent_head_v_dim,
            self.recurrent_v_width,
            self.recurrent_v_width,
            1,
            self.recurrent_num_v_heads,
            self.recurrent_num_v_heads,
            self.recurrent_num_k_heads,
            1,
            false,
        )?;
        self.cuda.rms_norm_rows_weighted_f32_f32weights_precise(
            &state.gated_delta,
            &layer.ssm_norm,
            &workspace.recurrent_out_norm,
            self.recurrent_num_v_heads,
            self.recurrent_head_v_dim,
            self.recurrent_head_v_dim,
            self.rms_norm_eps,
        )?;
        self.cuda.qwen_silu_mul_f32(
            &workspace.recurrent_out_norm,
            &workspace.recurrent_gate_z,
            &workspace.recurrent_gated,
            self.recurrent_v_width,
        )?;
        self.cuda.f32_to_bf16(
            &workspace.recurrent_gated,
            &workspace.recurrent_gated_bf16,
            self.recurrent_v_width,
        )?;
        layer.ssm_out.matvec(
            &self.cuda,
            &workspace.recurrent_gated_bf16,
            &workspace.recurrent_proj,
        )?;
        self.cuda.add_f32(
            input_hidden,
            &workspace.recurrent_proj,
            &workspace.residual,
            self.hidden_size,
        )?;
        self.eval_moe(
            &layer.moe,
            &layer.post_attention_norm,
            workspace,
            !input_is_a,
        )
    }

    fn eval_recurrent_layer_graph(
        &self,
        layer: &CudaQwenRecurrentLayer,
        state: &mut CudaQwenRecurrentLayerState,
        workspace: &mut CudaQwenWorkspace,
        input_is_a: bool,
    ) -> CudaResult<()> {
        let (input_hidden, _output_hidden) = if input_is_a {
            (&workspace.hidden_a, &workspace.hidden_b)
        } else {
            (&workspace.hidden_b, &workspace.hidden_a)
        };
        self.cuda.rms_norm_row_weighted_f32_f32weights_precise(
            input_hidden,
            &layer.attn_norm,
            &workspace.hidden_norm,
            self.hidden_size,
            self.rms_norm_eps,
        )?;
        self.cuda
            .f32_to_bf16(&workspace.hidden_norm, &workspace.hidden_bf16, self.hidden_size)?;
        layer
            .wqkv
            .matvec(&self.cuda, &workspace.hidden_bf16, &workspace.recurrent_qkv)?;
        layer
            .wqkv_gate
            .matvec(&self.cuda, &workspace.hidden_bf16, &workspace.recurrent_gate_z)?;
        layer.ssm_beta.matvec(
            &self.cuda,
            &workspace.hidden_bf16,
            &workspace.recurrent_beta_logits,
        )?;
        layer
            .ssm_alpha
            .matvec(&self.cuda, &workspace.hidden_bf16, &workspace.recurrent_alpha)?;
        self.cuda.qwen_ssm_conv_with_state_f32(
            &workspace.recurrent_qkv,
            &state.conv_state,
            &layer.ssm_conv1d,
            &workspace.recurrent_conv,
            self.layers_conv_prefix() + 1,
            self.recurrent_qkv_width,
        )?;
        self.cuda.qwen_split_recurrent_qkv_f32(
            &workspace.recurrent_conv,
            &workspace.recurrent_q,
            &workspace.recurrent_k,
            &workspace.recurrent_v,
            self.recurrent_q_width,
            self.recurrent_v_width,
        )?;
        self.cuda.rms_norm_rows_no_scale_f32_precise(
            &workspace.recurrent_q,
            &workspace.query,
            self.recurrent_num_k_heads,
            self.recurrent_head_k_dim,
            self.recurrent_head_k_dim,
            self.rms_norm_eps,
        )?;
        self.cuda.rms_norm_rows_no_scale_f32_precise(
            &workspace.recurrent_k,
            &workspace.key,
            self.recurrent_num_k_heads,
            self.recurrent_head_k_dim,
            self.recurrent_head_k_dim,
            self.rms_norm_eps,
        )?;
        let inv_scale = (self.recurrent_head_k_dim as f32).sqrt().recip();
        self.cuda
            .scale_f32_inplace(&workspace.query, inv_scale, self.recurrent_q_width)?;
        self.cuda
            .scale_f32_inplace(&workspace.key, inv_scale, self.recurrent_q_width)?;
        self.cuda.qwen_sigmoid_f32(
            &workspace.recurrent_beta_logits,
            &workspace.recurrent_beta,
            self.recurrent_num_v_heads,
        )?;
        self.cuda.qwen_decay_gate_f32(
            &layer.ssm_a,
            &workspace.recurrent_alpha,
            &layer.ssm_dt,
            &workspace.recurrent_decay,
            self.recurrent_num_v_heads,
        )?;
        self.cuda.gated_delta_net_f32_state_offset(
            &workspace.query,
            &workspace.key,
            &workspace.recurrent_v,
            &workspace.recurrent_decay,
            &workspace.recurrent_beta,
            &state.gated_delta,
            self.recurrent_v_width,
            self.recurrent_head_k_dim,
            self.recurrent_num_v_heads,
            1,
            1,
            self.recurrent_head_k_dim,
            self.recurrent_q_width,
            self.recurrent_q_width,
            self.recurrent_head_v_dim,
            self.recurrent_v_width,
            self.recurrent_v_width,
            1,
            self.recurrent_num_v_heads,
            self.recurrent_num_v_heads,
            self.recurrent_num_k_heads,
            1,
            false,
        )?;
        self.cuda.rms_norm_rows_weighted_f32_f32weights_precise(
            &state.gated_delta,
            &layer.ssm_norm,
            &workspace.recurrent_out_norm,
            self.recurrent_num_v_heads,
            self.recurrent_head_v_dim,
            self.recurrent_head_v_dim,
            self.rms_norm_eps,
        )?;
        self.cuda.qwen_silu_mul_f32(
            &workspace.recurrent_out_norm,
            &workspace.recurrent_gate_z,
            &workspace.recurrent_gated,
            self.recurrent_v_width,
        )?;
        self.cuda.f32_to_bf16(
            &workspace.recurrent_gated,
            &workspace.recurrent_gated_bf16,
            self.recurrent_v_width,
        )?;
        layer.ssm_out.matvec(
            &self.cuda,
            &workspace.recurrent_gated_bf16,
            &workspace.recurrent_proj,
        )?;
        self.cuda.add_f32(
            input_hidden,
            &workspace.recurrent_proj,
            &workspace.residual,
            self.hidden_size,
        )?;
        self.eval_moe_device(
            &layer.moe,
            &layer.post_attention_norm,
            workspace,
            !input_is_a,
        )
    }

    fn eval_moe(
        &self,
        moe: &CudaQwenMoeLayer,
        ffn_norm: &CudaBuffer,
        workspace: &mut CudaQwenWorkspace,
        output_is_a: bool,
    ) -> CudaResult<()> {
        let trace_moe = std::env::var("MAKEPAD_MLX_QWEN_TRACE_MOE").ok().as_deref() == Some("1");
        let output_hidden = if output_is_a {
            &workspace.hidden_a
        } else {
            &workspace.hidden_b
        };
        self.cuda.rms_norm_row_weighted_f32_f32weights_precise(
            &workspace.residual,
            ffn_norm,
            &workspace.ffn_input,
            self.hidden_size,
            self.rms_norm_eps,
        )?;
        self.cuda
            .f32_to_bf16(&workspace.ffn_input, &workspace.hidden_bf16, self.hidden_size)?;
        moe.ffn_gate_inp
            .matvec(&self.cuda, &workspace.hidden_bf16, &workspace.moe_router_logits)?;
        if trace_moe {
            eprintln!("[qwen-moe-trace] router");
        }
        let router_logits = self
            .cuda
            .read_f32s(&workspace.moe_router_logits, self.expert_count)?;
        let (_router_probabilities, routed_experts) =
            softmax_top_k_routes(&router_logits, self.experts_used_count)?;
        if trace_moe {
            eprintln!("[qwen-moe-trace] topk");
        }
        zero_buffer_f32(&self.cuda, &workspace.moe_routed_accum, self.hidden_size)?;
        for (route_slot, route) in routed_experts.iter().enumerate() {
            if trace_moe {
                eprintln!("[qwen-moe-trace] slot={route_slot} gate_up");
            }
            if let Some(merged) = &moe.ffn_gate_up_exps {
                merged.matvec_plane(
                    &self.cuda,
                    &workspace.hidden_bf16,
                    &workspace.moe_expert_gate_up,
                    route.expert_index as usize,
                )?;
                self.cuda.qwen_swiglu_split_f32(
                    &workspace.moe_expert_gate_up,
                    &workspace.moe_expert_act,
                    self.expert_intermediate,
                    self.expert_intermediate,
                )?;
            } else {
                let gate = moe
                    .ffn_gate_exps
                    .as_ref()
                    .ok_or_else(|| "missing expert gate weights".to_string())?;
                let up = moe
                    .ffn_up_exps
                    .as_ref()
                    .ok_or_else(|| "missing expert up weights".to_string())?;
                gate.matvec_plane(
                    &self.cuda,
                    &workspace.hidden_bf16,
                    &workspace.moe_expert_gate,
                    route.expert_index as usize,
                )?;
                up.matvec_plane(
                    &self.cuda,
                    &workspace.hidden_bf16,
                    &workspace.moe_expert_up,
                    route.expert_index as usize,
                )?;
                self.cuda.qwen_silu_mul_f32(
                    &workspace.moe_expert_up,
                    &workspace.moe_expert_gate,
                    &workspace.moe_expert_act,
                    self.expert_intermediate,
                )?;
            }
            if trace_moe {
                eprintln!("[qwen-moe-trace] slot={route_slot} down");
            }
            self.cuda.f32_to_bf16(
                &workspace.moe_expert_act,
                &workspace.moe_expert_act_bf16,
                self.expert_intermediate,
            )?;
            moe.ffn_down_exps.matvec_plane(
                &self.cuda,
                &workspace.moe_expert_act_bf16,
                &workspace.moe_expert_down,
                route.expert_index as usize,
            )?;
            self.cuda.scale_f32_inplace(
                &workspace.moe_expert_down,
                route.weight,
                self.hidden_size,
            )?;
            self.cuda.add_f32(
                &workspace.moe_routed_accum,
                &workspace.moe_expert_down,
                &workspace.moe_routed_accum,
                self.hidden_size,
            )?;
        }

        if trace_moe {
            eprintln!("[qwen-moe-trace] shared");
        }
        moe.ffn_gate_inp_shexp
            .matvec(&self.cuda, &workspace.hidden_bf16, &workspace.moe_shared_gate_scalar)?;
        moe.ffn_gate_shexp
            .matvec(&self.cuda, &workspace.hidden_bf16, &workspace.moe_shared_gate)?;
        moe.ffn_up_shexp
            .matvec(&self.cuda, &workspace.hidden_bf16, &workspace.moe_shared_up)?;
        self.cuda.qwen_silu_mul_f32(
            &workspace.moe_shared_up,
            &workspace.moe_shared_gate,
            &workspace.moe_shared_act,
            self.shared_expert_intermediate,
        )?;
        self.cuda.f32_to_bf16(
            &workspace.moe_shared_act,
            &workspace.moe_expert_act_bf16,
            self.shared_expert_intermediate,
        )?;
        moe.ffn_down_shexp.matvec(
            &self.cuda,
            &workspace.moe_expert_act_bf16,
            &workspace.moe_shared_down,
        )?;
        let shared_gate = self
            .cuda
            .read_f32s(&workspace.moe_shared_gate_scalar, 1)?
            .into_iter()
            .next()
            .ok_or_else(|| "missing qwen moe shared gate scalar".to_string())?;
        self.cuda.scale_f32_inplace(
            &workspace.moe_shared_down,
            sigmoid_f32(shared_gate),
            self.hidden_size,
        )?;
        self.cuda.add_f32(
            &workspace.moe_routed_accum,
            &workspace.moe_shared_down,
            &workspace.moe_output,
            self.hidden_size,
        )?;
        if trace_moe {
            eprintln!("[qwen-moe-trace] done");
        }
        self.cuda
            .add_f32(&workspace.residual, &workspace.moe_output, output_hidden, self.hidden_size)
    }

    fn eval_moe_device(
        &self,
        moe: &CudaQwenMoeLayer,
        ffn_norm: &CudaBuffer,
        workspace: &mut CudaQwenWorkspace,
        output_is_a: bool,
    ) -> CudaResult<()> {
        let trace_moe = std::env::var("MAKEPAD_MLX_QWEN_TRACE_MOE").ok().as_deref() == Some("1");
        let output_hidden = if output_is_a {
            &workspace.hidden_a
        } else {
            &workspace.hidden_b
        };
        self.cuda.rms_norm_row_weighted_f32_f32weights_precise(
            &workspace.residual,
            ffn_norm,
            &workspace.ffn_input,
            self.hidden_size,
            self.rms_norm_eps,
        )?;
        self.cuda
            .f32_to_bf16(&workspace.ffn_input, &workspace.hidden_bf16, self.hidden_size)?;
        moe.ffn_gate_inp
            .matvec(&self.cuda, &workspace.hidden_bf16, &workspace.moe_router_logits)?;
        if trace_moe {
            eprintln!("[qwen-moe-trace] router");
        }
        self.cuda.qwen_softmax_topk_routes_f32(
            &workspace.moe_router_logits,
            &workspace.moe_route_indices,
            &workspace.moe_route_weights,
            self.expert_count,
            self.experts_used_count,
        )?;
        if trace_moe {
            eprintln!("[qwen-moe-trace] topk");
        }
        zero_buffer_f32(&self.cuda, &workspace.moe_routed_accum, self.hidden_size)?;
        let use_batched_experts = moe.ffn_gate_up_exps.is_some()
            && self.experts_used_count > 1
            && self.experts_used_count <= 8;
        if use_batched_experts {
            if trace_moe {
                eprintln!("[qwen-moe-trace] batched_gate_up");
            }
            moe.ffn_gate_up_exps
                .as_ref()
                .ok_or_else(|| "missing merged expert gate/up weights".to_string())?
                .matvec_planes_device_indices(
                    &self.cuda,
                    &workspace.hidden_bf16,
                    &workspace.moe_expert_gate_up_batch,
                    &workspace.moe_route_indices,
                    self.experts_used_count,
                )?;
            self.cuda.qwen_swiglu_split_batched_f32(
                &workspace.moe_expert_gate_up_batch,
                &workspace.moe_expert_act_batch,
                self.expert_intermediate,
                self.expert_intermediate,
                self.experts_used_count,
            )?;
            self.cuda.f32_to_bf16(
                &workspace.moe_expert_act_batch,
                &workspace.moe_expert_act_bf16_batch,
                self.experts_used_count * self.expert_intermediate,
            )?;
            if trace_moe {
                eprintln!("[qwen-moe-trace] batched_down");
            }
            moe.ffn_down_exps.matvec_planes_device_indices_input_strided(
                &self.cuda,
                &workspace.moe_expert_act_bf16_batch,
                self.expert_intermediate,
                &workspace.moe_expert_down_batch,
                &workspace.moe_route_indices,
                self.experts_used_count,
            )?;
            self.cuda.weighted_sum_rows_f32(
                &workspace.moe_expert_down_batch,
                &workspace.moe_route_weights,
                &workspace.moe_routed_accum,
                self.hidden_size,
                self.experts_used_count,
            )?;
        }
        for route_slot in 0..self.experts_used_count {
            if use_batched_experts {
                break;
            }
            if trace_moe {
                eprintln!("[qwen-moe-trace] slot={route_slot} gate_up");
            }
            if let Some(merged) = &moe.ffn_gate_up_exps {
                merged.matvec_plane_device_index(
                    &self.cuda,
                    &workspace.hidden_bf16,
                    &workspace.moe_expert_gate_up,
                    &workspace.moe_route_indices,
                    route_slot,
                )?;
                self.cuda.qwen_swiglu_split_f32(
                    &workspace.moe_expert_gate_up,
                    &workspace.moe_expert_act,
                    self.expert_intermediate,
                    self.expert_intermediate,
                )?;
            } else {
                let gate = moe
                    .ffn_gate_exps
                    .as_ref()
                    .ok_or_else(|| "missing expert gate weights".to_string())?;
                let up = moe
                    .ffn_up_exps
                    .as_ref()
                    .ok_or_else(|| "missing expert up weights".to_string())?;
                gate.matvec_plane_device_index(
                    &self.cuda,
                    &workspace.hidden_bf16,
                    &workspace.moe_expert_gate,
                    &workspace.moe_route_indices,
                    route_slot,
                )?;
                up.matvec_plane_device_index(
                    &self.cuda,
                    &workspace.hidden_bf16,
                    &workspace.moe_expert_up,
                    &workspace.moe_route_indices,
                    route_slot,
                )?;
                self.cuda.qwen_silu_mul_f32(
                    &workspace.moe_expert_up,
                    &workspace.moe_expert_gate,
                    &workspace.moe_expert_act,
                    self.expert_intermediate,
                )?;
            }
            if trace_moe {
                eprintln!("[qwen-moe-trace] slot={route_slot} down");
            }
            self.cuda.f32_to_bf16(
                &workspace.moe_expert_act,
                &workspace.moe_expert_act_bf16,
                self.expert_intermediate,
            )?;
            moe.ffn_down_exps.matvec_plane_device_index(
                &self.cuda,
                &workspace.moe_expert_act_bf16,
                &workspace.moe_expert_down,
                &workspace.moe_route_indices,
                route_slot,
            )?;
            self.cuda.scale_f32_inplace_device_f32_index(
                &workspace.moe_expert_down,
                &workspace.moe_route_weights,
                route_slot,
                self.hidden_size,
            )?;
            self.cuda.add_f32(
                &workspace.moe_routed_accum,
                &workspace.moe_expert_down,
                &workspace.moe_routed_accum,
                self.hidden_size,
            )?;
        }

        if trace_moe {
            eprintln!("[qwen-moe-trace] shared");
        }
        moe.ffn_gate_inp_shexp
            .matvec(&self.cuda, &workspace.hidden_bf16, &workspace.moe_shared_gate_scalar)?;
        moe.ffn_gate_shexp
            .matvec(&self.cuda, &workspace.hidden_bf16, &workspace.moe_shared_gate)?;
        moe.ffn_up_shexp
            .matvec(&self.cuda, &workspace.hidden_bf16, &workspace.moe_shared_up)?;
        self.cuda.qwen_silu_mul_f32(
            &workspace.moe_shared_up,
            &workspace.moe_shared_gate,
            &workspace.moe_shared_act,
            self.shared_expert_intermediate,
        )?;
        self.cuda.f32_to_bf16(
            &workspace.moe_shared_act,
            &workspace.moe_expert_act_bf16,
            self.shared_expert_intermediate,
        )?;
        moe.ffn_down_shexp.matvec(
            &self.cuda,
            &workspace.moe_expert_act_bf16,
            &workspace.moe_shared_down,
        )?;
        self.cuda.qwen_sigmoid_f32(
            &workspace.moe_shared_gate_scalar,
            &workspace.moe_shared_gate_scalar,
            1,
        )?;
        self.cuda.scale_f32_inplace_device_f32_index(
            &workspace.moe_shared_down,
            &workspace.moe_shared_gate_scalar,
            0,
            self.hidden_size,
        )?;
        self.cuda.add_f32(
            &workspace.moe_routed_accum,
            &workspace.moe_shared_down,
            &workspace.moe_output,
            self.hidden_size,
        )?;
        if trace_moe {
            eprintln!("[qwen-moe-trace] done");
        }
        self.cuda
            .add_f32(&workspace.residual, &workspace.moe_output, output_hidden, self.hidden_size)
    }
}

impl CudaAffineTensor {
    fn load(
        cuda: &CudaRuntime,
        weights: &MlxQwen35MoeIndexedSafetensors,
        weight_name: &str,
    ) -> CudaResult<Self> {
        let quantization = weights
            .quantization_for_tensor(weight_name)
            .map_err(|err| err.to_string())?
            .ok_or_else(|| format!("tensor {weight_name} is missing quantization config"))?;
        if quantization.mode != "affine" || !matches!(quantization.bits, 4 | 8) {
            return Err(format!(
                "tensor {weight_name} uses unsupported quantization {:?}",
                quantization
            ));
        }
        let weight_entry = weights.tensor(weight_name).map_err(|err| err.to_string())?;
        if weight_entry.dtype != MlxDType::U32 {
            return Err(format!(
                "tensor {weight_name} expected U32, got {:?}",
                weight_entry.dtype
            ));
        }
        let actual_weight_name = weights
            .actual_tensor_name(weight_name)
            .map_err(|err| err.to_string())?;
        let (actual_scales_name, actual_biases_name) = actual_affine_qparam_names(actual_weight_name);
        let scales_entry = weights
            .tensor(&actual_scales_name)
            .map_err(|err| err.to_string())?;
        let biases_entry = weights
            .tensor(&actual_biases_name)
            .map_err(|err| err.to_string())?;
        if scales_entry.dtype != MlxDType::BF16 || biases_entry.dtype != MlxDType::BF16 {
            return Err(format!(
                "tensor {weight_name} qparams expected BF16, got {:?} / {:?}",
                scales_entry.dtype, biases_entry.dtype
            ));
        }
        let (out_rows, weight_words_per_row, qparams_per_row, plane_count) =
            match weight_entry.shape.as_slice() {
                [rows, cols] => (
                    *rows as usize,
                    *cols as usize,
                    *scales_entry
                        .shape
                        .get(1)
                        .ok_or_else(|| format!("tensor {weight_name} scales missing rank-2 dim"))?
                        as usize,
                    1usize,
                ),
                [planes, rows, cols] => (
                    *rows as usize,
                    *cols as usize,
                    *scales_entry
                        .shape
                        .get(2)
                        .ok_or_else(|| format!("tensor {weight_name} scales missing rank-3 dim"))?
                        as usize,
                    *planes as usize,
                ),
                other => {
                    return Err(format!(
                        "tensor {weight_name} expected rank 2 or 3, got {:?}",
                        other
                    ))
                }
            };
        let weight_words_per_plane = out_rows
            .checked_mul(weight_words_per_row)
            .ok_or_else(|| format!("tensor {weight_name} plane weight size overflow"))?;
        let qparams_words_per_plane = out_rows
            .checked_mul(qparams_per_row)
            .ok_or_else(|| format!("tensor {weight_name} plane qparam size overflow"))?;
        Ok(Self {
            packed_weights: cuda.load_bytes(
                &weights
                    .read_tensor_bytes(actual_weight_name)
                    .map_err(|err| err.to_string())?,
            )?,
            scales: cuda.load_bytes(
                &weights
                    .read_tensor_bytes(&actual_scales_name)
                    .map_err(|err| err.to_string())?,
            )?,
            biases: cuda.load_bytes(
                &weights
                    .read_tensor_bytes(&actual_biases_name)
                    .map_err(|err| err.to_string())?,
            )?,
            bits: quantization.bits,
            out_rows,
            weight_words_per_row,
            qparams_per_row,
            plane_count,
            weight_words_per_plane,
            qparams_words_per_plane,
        })
    }

    fn matvec(
        &self,
        cuda: &CudaRuntime,
        input_bf16: &CudaBuffer,
        output_f32: &CudaBuffer,
    ) -> CudaResult<()> {
        cuda.affine_qmv_bf16_to_f32_precise(
            input_bf16,
            &self.packed_weights,
            &self.scales,
            &self.biases,
            output_f32,
            self.row_width(),
            self.weight_words_per_row,
            self.qparams_per_row,
            self.out_rows,
            self.bits,
        )
    }

    fn matvec_plane(
        &self,
        cuda: &CudaRuntime,
        input_bf16: &CudaBuffer,
        output_f32: &CudaBuffer,
        plane: usize,
    ) -> CudaResult<()> {
        if plane >= self.plane_count {
            return Err(format!(
                "plane {plane} out of range for tensor with {} planes",
                self.plane_count
            ));
        }
        cuda.affine_qmv_bf16_to_f32_offsets_precise(
            input_bf16,
            &self.packed_weights,
            plane * self.weight_words_per_plane,
            &self.scales,
            plane * self.qparams_words_per_plane,
            &self.biases,
            plane * self.qparams_words_per_plane,
            output_f32,
            self.row_width(),
            self.weight_words_per_row,
            self.qparams_per_row,
            self.out_rows,
            self.bits,
        )
    }

    fn matvec_plane_device_index(
        &self,
        cuda: &CudaRuntime,
        input_bf16: &CudaBuffer,
        output_f32: &CudaBuffer,
        plane_indices_u32: &CudaBuffer,
        plane_slot: usize,
    ) -> CudaResult<()> {
        cuda.affine_qmv_bf16_to_f32_select_plane_precise(
            input_bf16,
            &self.packed_weights,
            &self.scales,
            &self.biases,
            plane_indices_u32,
            plane_slot,
            output_f32,
            self.row_width(),
            self.weight_words_per_row,
            self.qparams_per_row,
            self.out_rows,
            self.weight_words_per_plane,
            self.qparams_words_per_plane,
            self.plane_count,
            self.bits,
        )
    }

    fn matvec_planes_device_indices(
        &self,
        cuda: &CudaRuntime,
        input_bf16: &CudaBuffer,
        output_f32: &CudaBuffer,
        plane_indices_u32: &CudaBuffer,
        selected_count: usize,
    ) -> CudaResult<()> {
        cuda.affine_qmv_bf16_to_f32_select_planes_precise(
            input_bf16,
            &self.packed_weights,
            &self.scales,
            &self.biases,
            plane_indices_u32,
            selected_count,
            output_f32,
            self.row_width(),
            self.weight_words_per_row,
            self.qparams_per_row,
            self.out_rows,
            self.weight_words_per_plane,
            self.qparams_words_per_plane,
            self.plane_count,
            self.bits,
        )
    }

    fn matvec_planes_device_indices_input_strided(
        &self,
        cuda: &CudaRuntime,
        input_bf16: &CudaBuffer,
        input_words_per_slot: usize,
        output_f32: &CudaBuffer,
        plane_indices_u32: &CudaBuffer,
        selected_count: usize,
    ) -> CudaResult<()> {
        cuda.affine_qmv_bf16_to_f32_select_planes_input_offsets_precise(
            input_bf16,
            input_words_per_slot,
            &self.packed_weights,
            &self.scales,
            &self.biases,
            plane_indices_u32,
            selected_count,
            output_f32,
            self.row_width(),
            self.weight_words_per_row,
            self.qparams_per_row,
            self.out_rows,
            self.weight_words_per_plane,
            self.qparams_words_per_plane,
            self.plane_count,
            self.bits,
        )
    }

    fn get_row(
        &self,
        cuda: &CudaRuntime,
        row_index: usize,
        output_f32: &CudaBuffer,
    ) -> CudaResult<()> {
        if self.plane_count != 1 {
            return Err("rank-3 affine tensor does not support row lookup".to_string());
        }
        if row_index >= self.out_rows {
            return Err(format!("row {row_index} out of range for {} rows", self.out_rows));
        }
        cuda.affine_get_row_f32(
            &self.packed_weights,
            &self.scales,
            &self.biases,
            output_f32,
            self.weight_words_per_row,
            self.qparams_per_row,
            row_index,
            self.bits,
        )
    }

    #[cfg(test)]
    fn get_row_device_u32(
        &self,
        cuda: &CudaRuntime,
        row_index_device_u32: &CudaBuffer,
        output_f32: &CudaBuffer,
    ) -> CudaResult<()> {
        if self.plane_count != 1 {
            return Err("rank-3 affine tensor does not support row lookup".to_string());
        }
        cuda.affine_get_row_f32_device_u32(
            &self.packed_weights,
            &self.scales,
            &self.biases,
            output_f32,
            self.weight_words_per_row,
            self.qparams_per_row,
            row_index_device_u32,
            self.bits,
        )
    }

    fn get_row_device_u32_ptr(
        &self,
        cuda: &CudaRuntime,
        row_index_device_u32: *const u32,
        output_f32: &CudaBuffer,
    ) -> CudaResult<()> {
        if self.plane_count != 1 {
            return Err("rank-3 affine tensor does not support row lookup".to_string());
        }
        cuda.affine_get_row_f32_device_u32_ptr(
            &self.packed_weights,
            &self.scales,
            &self.biases,
            output_f32,
            self.weight_words_per_row,
            self.qparams_per_row,
            row_index_device_u32,
            self.bits,
        )
    }

    fn row_width(&self) -> usize {
        self.weight_words_per_row * (32 / self.bits as usize)
    }
}

impl CudaQwenMoeLayer {
    fn load(
        cuda: &CudaRuntime,
        weights: &MlxQwen35MoeIndexedSafetensors,
        moe: &crate::MlxQwen35MoeMoeTensors,
    ) -> CudaResult<Self> {
        Ok(Self {
            ffn_gate_inp: CudaAffineTensor::load(cuda, weights, &moe.ffn_gate_inp)?,
            ffn_gate_up_exps: moe
                .ffn_gate_up_exps
                .as_ref()
                .map(|name| CudaAffineTensor::load(cuda, weights, name))
                .transpose()?,
            ffn_gate_exps: moe
                .ffn_gate_exps
                .as_ref()
                .map(|name| CudaAffineTensor::load(cuda, weights, name))
                .transpose()?,
            ffn_up_exps: moe
                .ffn_up_exps
                .as_ref()
                .map(|name| CudaAffineTensor::load(cuda, weights, name))
                .transpose()?,
            ffn_down_exps: CudaAffineTensor::load(cuda, weights, &moe.ffn_down_exps)?,
            ffn_gate_inp_shexp: CudaAffineTensor::load(cuda, weights, &moe.ffn_gate_inp_shexp)?,
            ffn_gate_shexp: CudaAffineTensor::load(cuda, weights, &moe.ffn_gate_shexp)?,
            ffn_up_shexp: CudaAffineTensor::load(cuda, weights, &moe.ffn_up_shexp)?,
            ffn_down_shexp: CudaAffineTensor::load(cuda, weights, &moe.ffn_down_shexp)?,
        })
    }
}

fn load_vector_f32(
    cuda: &CudaRuntime,
    weights: &MlxQwen35MoeIndexedSafetensors,
    name: &str,
) -> CudaResult<CudaBuffer> {
    let words = weights
        .read_bf16_tensor_words_cached(name)
        .map_err(|err| err.to_string())?;
    let values = words
        .iter()
        .copied()
        .map(qwen_bf16_word_to_f32)
        .collect::<Vec<_>>();
    cuda.load_bytes(f32s_as_le_bytes(&values))
}

fn u32s_as_le_bytes(values: &[u32]) -> &[u8] {
    #[cfg(target_endian = "little")]
    unsafe {
        std::slice::from_raw_parts(
            values.as_ptr().cast::<u8>(),
            values.len() * std::mem::size_of::<u32>(),
        )
    }
    #[cfg(not(target_endian = "little"))]
    {
        unreachable!("u32 byte reinterpret assumes little-endian")
    }
}

fn f32s_as_le_bytes(values: &[f32]) -> &[u8] {
    #[cfg(target_endian = "little")]
    unsafe {
        std::slice::from_raw_parts(
            values.as_ptr().cast::<u8>(),
            values.len() * std::mem::size_of::<f32>(),
        )
    }
    #[cfg(not(target_endian = "little"))]
    {
        unreachable!("f32 byte reinterpret assumes little-endian")
    }
}

fn zero_buffer_f32(cuda: &CudaRuntime, buffer: &CudaBuffer, len: usize) -> CudaResult<()> {
    let len_bytes = len
        .checked_mul(std::mem::size_of::<f32>())
        .ok_or_else(|| "qwen zero buffer byte size overflow".to_string())?;
    cuda.zero_bytes(buffer, len_bytes)
}

fn session_capacity_tokens(state: &CudaQwenAttentionLayerState, kv_width: usize) -> usize {
    state.key_cache.size_bytes() / (kv_width * std::mem::size_of::<u16>())
}

fn argmax_index(values: &[f32]) -> usize {
    let mut best_index = 0usize;
    let mut best_value = f32::NEG_INFINITY;
    for (index, value) in values.iter().copied().enumerate() {
        if value > best_value {
            best_index = index;
            best_value = value;
        }
    }
    best_index
}

fn max_abs_diff(left: &[f32], right: &[f32]) -> f32 {
    left.iter()
        .zip(right.iter())
        .map(|(left, right)| (left - right).abs())
        .fold(0.0f32, f32::max)
}

fn bf16_words_to_f32(words: &[u16]) -> Vec<f32> {
    words.iter()
        .copied()
        .map(qwen_bf16_word_to_f32)
        .collect::<Vec<_>>()
}

fn round_slice_to_bf16(values: &[f32]) -> Vec<f32> {
    values
        .iter()
        .copied()
        .map(qwen_bf16_round_to_f32)
        .collect::<Vec<_>>()
}

fn bf16_words_from_le_bytes(bytes: &[u8]) -> std::result::Result<Vec<u16>, String> {
    if bytes.len() % std::mem::size_of::<u16>() != 0 {
        return Err(format!("bf16 byte length {} is not divisible by 2", bytes.len()));
    }
    Ok(bytes
        .chunks_exact(2)
        .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
        .collect())
}

impl CudaQwenTextRuntime {
    fn debug_compare_decode_state(
        &self,
        session: &CudaQwenDecodeSession,
        reference_state: &MlxQwen35MoeDecodeState,
        position: usize,
    ) -> CudaResult<()> {
        for (layer_index, (cuda_state, reference_layer)) in session
            .layer_states
            .iter()
            .zip(reference_state.layers.iter())
            .enumerate()
        {
            match (cuda_state, reference_layer) {
                (
                    CudaQwenLayerState::Attention(cuda_attn),
                    MlxQwen35MoeLayerDecodeState::Attention(reference_attn),
                ) => {
                    let key_len = reference_attn.key_cache.len();
                    let value_len = reference_attn.value_cache.len();
                    let actual_key = bf16_words_to_f32(&bf16_words_from_le_bytes(
                        &self
                            .cuda
                            .read_bytes(
                                &cuda_attn.key_cache,
                                key_len * std::mem::size_of::<u16>(),
                            )?,
                    )?);
                    let actual_value = bf16_words_to_f32(&bf16_words_from_le_bytes(
                        &self
                            .cuda
                            .read_bytes(
                                &cuda_attn.value_cache,
                                value_len * std::mem::size_of::<u16>(),
                            )?,
                    )?);
                    eprintln!(
                        "[qwen-state-compare] position={position} layer={layer_index} kind=attention key_max_abs_diff={} value_max_abs_diff={}",
                        max_abs_diff(&actual_key, &round_slice_to_bf16(&reference_attn.key_cache)),
                        max_abs_diff(&actual_value, &round_slice_to_bf16(&reference_attn.value_cache)),
                    );
                }
                (
                    CudaQwenLayerState::Recurrent(cuda_recurrent),
                    MlxQwen35MoeLayerDecodeState::Recurrent(reference_recurrent),
                ) => {
                    let actual_conv = self
                        .cuda
                        .read_f32s(&cuda_recurrent.conv_state, reference_recurrent.conv_state.len())?;
                    let actual_state = self
                        .cuda
                        .read_f32s(
                            &cuda_recurrent.gated_delta,
                            self.recurrent_v_width + reference_recurrent.ssm_state.len(),
                        )?;
                    eprintln!(
                        "[qwen-state-compare] position={position} layer={layer_index} kind=recurrent conv_max_abs_diff={} state_max_abs_diff={}",
                        max_abs_diff(&actual_conv, &reference_recurrent.conv_state),
                        max_abs_diff(
                            &actual_state[self.recurrent_v_width..],
                            &reference_recurrent.ssm_state,
                        ),
                    );
                }
                _ => return Err("qwen decode state kind mismatch".to_string()),
            }
        }
        Ok(())
    }

    #[cfg(test)]
    fn debug_eval_moe_from_ffn_input(
        &self,
        layer_index: usize,
        ffn_input: &[f32],
    ) -> CudaResult<MlxQwen35MoeFfnOutput> {
        if ffn_input.len() != self.hidden_size {
            return Err(format!(
                "qwen moe debug input length {} does not match hidden size {}",
                ffn_input.len(),
                self.hidden_size
            ));
        }
        let moe = match self
            .layers
            .get(layer_index)
            .ok_or_else(|| format!("qwen layer {} out of range", layer_index))?
        {
            CudaQwenLayer::Attention(layer) => &layer.moe,
            CudaQwenLayer::Recurrent(layer) => &layer.moe,
        };
        let mut session = self.new_decode_session(1, &[])?;
        let workspace = &mut session.workspace;
        self.cuda
            .write_bytes(&workspace.ffn_input, f32s_as_le_bytes(ffn_input))?;
        self.cuda
            .f32_to_bf16(&workspace.ffn_input, &workspace.hidden_bf16, self.hidden_size)?;
        moe.ffn_gate_inp
            .matvec(&self.cuda, &workspace.hidden_bf16, &workspace.moe_router_logits)?;
        let router_logits = self
            .cuda
            .read_f32s(&workspace.moe_router_logits, self.expert_count)?;
        let (router_probabilities, routed_experts) =
            softmax_top_k_routes(&router_logits, self.experts_used_count)?;
        zero_buffer_f32(&self.cuda, &workspace.moe_routed_accum, self.hidden_size)?;
        for route in &routed_experts {
            if let Some(merged) = &moe.ffn_gate_up_exps {
                merged.matvec_plane(
                    &self.cuda,
                    &workspace.hidden_bf16,
                    &workspace.moe_expert_gate_up,
                    route.expert_index as usize,
                )?;
                self.cuda.qwen_swiglu_split_f32(
                    &workspace.moe_expert_gate_up,
                    &workspace.moe_expert_act,
                    self.expert_intermediate,
                    self.expert_intermediate,
                )?;
            } else {
                let gate = moe
                    .ffn_gate_exps
                    .as_ref()
                    .ok_or_else(|| "missing expert gate weights".to_string())?;
                let up = moe
                    .ffn_up_exps
                    .as_ref()
                    .ok_or_else(|| "missing expert up weights".to_string())?;
                gate.matvec_plane(
                    &self.cuda,
                    &workspace.hidden_bf16,
                    &workspace.moe_expert_gate,
                    route.expert_index as usize,
                )?;
                up.matvec_plane(
                    &self.cuda,
                    &workspace.hidden_bf16,
                    &workspace.moe_expert_up,
                    route.expert_index as usize,
                )?;
                self.cuda.qwen_silu_mul_f32(
                    &workspace.moe_expert_up,
                    &workspace.moe_expert_gate,
                    &workspace.moe_expert_act,
                    self.expert_intermediate,
                )?;
            }
            self.cuda.f32_to_bf16(
                &workspace.moe_expert_act,
                &workspace.moe_expert_act_bf16,
                self.expert_intermediate,
            )?;
            moe.ffn_down_exps.matvec_plane(
                &self.cuda,
                &workspace.moe_expert_act_bf16,
                &workspace.moe_expert_down,
                route.expert_index as usize,
            )?;
            self.cuda.scale_f32_inplace(
                &workspace.moe_expert_down,
                route.weight,
                self.hidden_size,
            )?;
            self.cuda.add_f32(
                &workspace.moe_routed_accum,
                &workspace.moe_expert_down,
                &workspace.moe_routed_accum,
                self.hidden_size,
            )?;
        }
        let routed_output = self
            .cuda
            .read_f32s(&workspace.moe_routed_accum, self.hidden_size)?;

        moe.ffn_gate_inp_shexp
            .matvec(&self.cuda, &workspace.hidden_bf16, &workspace.moe_shared_gate_scalar)?;
        moe.ffn_gate_shexp
            .matvec(&self.cuda, &workspace.hidden_bf16, &workspace.moe_shared_gate)?;
        moe.ffn_up_shexp
            .matvec(&self.cuda, &workspace.hidden_bf16, &workspace.moe_shared_up)?;
        self.cuda.qwen_silu_mul_f32(
            &workspace.moe_shared_up,
            &workspace.moe_shared_gate,
            &workspace.moe_shared_act,
            self.shared_expert_intermediate,
        )?;
        self.cuda.f32_to_bf16(
            &workspace.moe_shared_act,
            &workspace.moe_expert_act_bf16,
            self.shared_expert_intermediate,
        )?;
        moe.ffn_down_shexp.matvec(
            &self.cuda,
            &workspace.moe_expert_act_bf16,
            &workspace.moe_shared_down,
        )?;
        let shared_gate = self
            .cuda
            .read_f32s(&workspace.moe_shared_gate_scalar, 1)?
            .into_iter()
            .next()
            .ok_or_else(|| "missing shared gate scalar".to_string())?;
        self.cuda.scale_f32_inplace(
            &workspace.moe_shared_down,
            sigmoid_f32(shared_gate),
            self.hidden_size,
        )?;
        let shared_output = self
            .cuda
            .read_f32s(&workspace.moe_shared_down, self.hidden_size)?;
        self.cuda.add_f32(
            &workspace.moe_routed_accum,
            &workspace.moe_shared_down,
            &workspace.moe_output,
            self.hidden_size,
        )?;
        let output = self
            .cuda
            .read_f32s(&workspace.moe_output, self.hidden_size)?;
        Ok(MlxQwen35MoeFfnOutput {
            router_logits,
            router_probabilities,
            routed_experts,
            routed_output,
            shared_gate: sigmoid_f32(shared_gate),
            shared_output,
            output,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::qwen_runtime::lazy::QwenGenerationBackend;
    use crate::qwen_runtime::{
        affine_dequantize_row_f32, affine_quantized_matmul_fallback, apply_qwen_mrope_rows_in_place,
        apply_ssm_conv_with_state_f32, gated_delta_net_step_f32, grouped_self_attention_step_f32,
        qwen_bf16_round_to_f32, qwen_f32_to_bf16_word, softmax_top_k_routes,
    };
    use std::path::PathBuf;

    fn f32_bytes(values: &[f32]) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(values.len() * std::mem::size_of::<f32>());
        for value in values {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes
    }

    fn assert_close(label: &str, actual: &[f32], expected: &[f32], tolerance: f32) {
        assert_eq!(
            actual.len(),
            expected.len(),
            "{label} length mismatch: {} vs {}",
            actual.len(),
            expected.len()
        );
        let mut max_abs = 0.0f32;
        let mut max_index = 0usize;
        for (index, (actual, expected)) in actual.iter().zip(expected.iter()).enumerate() {
            let abs = (actual - expected).abs();
            if abs > max_abs {
                max_abs = abs;
                max_index = index;
            }
        }
        assert!(
            max_abs <= tolerance,
            "{label} max_abs_diff={max_abs} at index {max_index}: actual={} expected={}",
            actual[max_index],
            expected[max_index]
        );
    }

    fn cpu_project_vector_bf16_words(
        runtime_session: &MlxQwen35MoeRuntimeSession,
        input_words: &[u16],
        weight_name: &str,
    ) -> Vec<f32> {
        let weight_entry = runtime_session.weights.tensor(weight_name).unwrap();
        match weight_entry.dtype {
            MlxDType::BF16 => {
                dense_bf16_matmul_t_f32(&runtime_session.weights, input_words, weight_name).unwrap()
            }
            MlxDType::U32 => {
                let quantization = runtime_session
                    .weights
                    .quantization_for_tensor(weight_name)
                    .unwrap()
                    .unwrap();
                let actual_weight_name = runtime_session
                    .weights
                    .actual_tensor_name(weight_name)
                    .unwrap();
                let (actual_scales_name, actual_biases_name) =
                    actual_affine_qparam_names(actual_weight_name);
                let scales_entry = runtime_session.weights.tensor(&actual_scales_name).unwrap();
                let packed_words = runtime_session
                    .weights
                    .read_u32_tensor_words_cached(weight_name)
                    .unwrap();
                let scale_words = runtime_session
                    .weights
                    .read_bf16_tensor_words_cached(&actual_scales_name)
                    .unwrap();
                let bias_words = runtime_session
                    .weights
                    .read_bf16_tensor_words_cached(&actual_biases_name)
                    .unwrap();
                affine_quantized_matmul_fallback(
                    input_words,
                    packed_words.as_slice(),
                    scale_words.as_slice(),
                    bias_words.as_slice(),
                    weight_entry.shape[0] as usize,
                    weight_entry.shape[1] as usize,
                    scales_entry.shape[1] as usize,
                    quantization.group_size as u64,
                    quantization.bits,
                )
                .unwrap()
            }
            other => panic!("unsupported dtype for cpu projection test: {:?}", other),
        }
    }

    fn real_qwen_model_dir() -> Option<PathBuf> {
        let path = PathBuf::from("/home/playe/qwen_36_35B_4bit_mlx_vlm");
        path.exists().then_some(path)
    }

    fn round_bf16(values: &[f32]) -> Vec<f32> {
        values
            .iter()
            .copied()
            .map(qwen_bf16_round_to_f32)
            .collect::<Vec<_>>()
    }

    fn u16_bytes(values: &[u16]) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(values.len() * std::mem::size_of::<u16>());
        for value in values {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes
    }

    fn bf16_words_from_bytes(bytes: &[u8]) -> Vec<u16> {
        bytes.chunks_exact(2)
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .collect()
    }

    fn u32_bytes(values: &[u32]) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(values.len() * std::mem::size_of::<u32>());
        for value in values {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes
    }

    fn scalar_u32_buffer(cuda: &CudaRuntime, value: u32) -> CudaBuffer {
        let buffer = cuda.alloc_u32(1).unwrap();
        cuda.write_u32(&buffer, value).unwrap();
        buffer
    }

    #[test]
    fn cuda_qwen_ssm_conv_with_state_matches_reference() {
        if !makepad_ggml::backend::cuda::is_available() {
            return;
        }
        let cuda = CudaRuntime::load().unwrap();
        let current = vec![1.0, 2.0, 10.0, 20.0];
        let initial_state = vec![0.5, 0.25, 5.0, 2.5, 1.5, 0.75, 15.0, 7.5];
        let kernel = vec![
            0.25, 0.50, 0.75, //
            0.10, 0.20, 0.30, //
            1.00, 0.50, 0.25, //
            0.40, 0.30, 0.20, //
        ];

        let mut expected_state = initial_state.clone();
        let expected =
            apply_ssm_conv_with_state_f32(&current, &mut expected_state, &kernel, 3).unwrap();

        let current_buf = cuda.load_bytes(&f32_bytes(&current)).unwrap();
        let state_buf = cuda.load_bytes(&f32_bytes(&initial_state)).unwrap();
        let kernel_buf = cuda.load_bytes(&f32_bytes(&kernel)).unwrap();
        let out_buf = cuda.alloc_f32(current.len()).unwrap();
        cuda.qwen_ssm_conv_with_state_f32(
            &current_buf,
            &state_buf,
            &kernel_buf,
            &out_buf,
            3,
            current.len(),
        )
        .unwrap();

        let actual = cuda.read_f32s(&out_buf, current.len()).unwrap();
        let actual_state = cuda.read_f32s(&state_buf, initial_state.len()).unwrap();
        assert_close("ssm_conv_output", &actual, &expected, 6.5e-2);
        assert_close("ssm_conv_state", &actual_state, &expected_state, 1.0e-5);
    }

    #[test]
    fn cuda_qwen_mrope_rows_matches_reference() {
        if !makepad_ggml::backend::cuda::is_available() {
            return;
        }
        let cuda = CudaRuntime::load().unwrap();
        let input = vec![
            1.0, 2.0, 3.0, 4.0, 10.0, 20.0, 30.0, 40.0, //
            5.0, 6.0, 7.0, 8.0, 50.0, 60.0, 70.0, 80.0, //
        ];
        let mut expected = input.clone();
        apply_qwen_mrope_rows_in_place(
            &mut expected,
            2,
            8,
            8,
            [7, 7, 7, 0],
            [11, 11, 10, 0],
            10_000_000.0,
        )
        .unwrap();

        let input_buf = cuda.load_bytes(&f32_bytes(&input)).unwrap();
        let out_buf = cuda.alloc_f32(input.len()).unwrap();
        cuda.qwen_mrope_rows_f32(
            &input_buf,
            &out_buf,
            2,
            8,
            8,
            10_000_000.0,
            [7, 7, 7, 0],
            [11, 11, 10, 0],
        )
        .unwrap();

        let actual = cuda.read_f32s(&out_buf, input.len()).unwrap();
        assert_close("mrope_rows", &actual, &expected, 1.0e-5);
    }

    #[test]
    fn cuda_qwen_mrope_rows_device_u32_matches_reference() {
        if !makepad_ggml::backend::cuda::is_available() {
            return;
        }
        let cuda = CudaRuntime::load().unwrap();
        let input = vec![
            1.0, 2.0, 3.0, 4.0, 10.0, 20.0, 30.0, 40.0, //
            5.0, 6.0, 7.0, 8.0, 50.0, 60.0, 70.0, 80.0, //
        ];
        let mut expected = input.clone();
        apply_qwen_mrope_rows_in_place(
            &mut expected,
            2,
            8,
            8,
            [7, 7, 7, 0],
            [11, 11, 10, 0],
            10_000_000.0,
        )
        .unwrap();

        let input_buf = cuda.load_bytes(&f32_bytes(&input)).unwrap();
        let out_buf = cuda.alloc_f32(input.len()).unwrap();
        let position_buf = scalar_u32_buffer(&cuda, 7);
        cuda.qwen_mrope_rows_f32_device_u32(
            &input_buf,
            &out_buf,
            2,
            8,
            8,
            10_000_000.0,
            &position_buf,
            [11, 11, 10, 0],
        )
        .unwrap();

        let actual = cuda.read_f32s(&out_buf, input.len()).unwrap();
        assert_close("mrope_rows_device_u32", &actual, &expected, 1.0e-5);
    }

    #[test]
    fn cuda_qwen_gated_delta_matches_reference() {
        if !makepad_ggml::backend::cuda::is_available() {
            return;
        }
        let cuda = CudaRuntime::load().unwrap();
        let q = vec![1.0, 0.5, -0.25, 0.75];
        let k = vec![0.25, -0.5, 0.75, 1.0];
        let v = vec![2.0, 4.0, 6.0, 8.0, 1.5, -2.0, 3.5, 4.5];
        let log_gate: Vec<f32> = vec![-0.1, -0.4];
        let gate = log_gate
            .iter()
            .map(|value| (*value).exp())
            .collect::<Vec<_>>();
        let beta = vec![0.5, 0.25];
        let initial_state = vec![
            0.5, -0.25, 0.75, 1.25, //
            -0.5, 0.25, -0.75, 0.5, //
            1.0, -1.0, 0.5, -0.5, //
            0.25, 0.5, -0.25, -0.75, //
            -0.2, 0.4, -0.6, 0.8, //
            0.1, -0.3, 0.5, -0.7, //
            0.9, -0.8, 0.7, -0.6, //
            0.3, -0.2, 0.1, -0.4, //
        ];

        let mut expected_state = initial_state.clone();
        let q_reference = q.iter().map(|value| *value * 0.5).collect::<Vec<_>>();
        let expected = gated_delta_net_step_f32(
            &q_reference,
            &k,
            &v,
            &gate,
            &beta,
            &mut expected_state,
            4,
            1,
            4,
            2,
        )
        .unwrap();

        let q_buf = cuda.load_bytes(&f32_bytes(&q)).unwrap();
        let k_buf = cuda.load_bytes(&f32_bytes(&k)).unwrap();
        let v_buf = cuda.load_bytes(&f32_bytes(&v)).unwrap();
        let log_gate_buf = cuda.load_bytes(&f32_bytes(&log_gate)).unwrap();
        let beta_buf = cuda.load_bytes(&f32_bytes(&beta)).unwrap();
        let state_offset = expected.len();
        let mut state_dst = vec![0.0f32; state_offset];
        state_dst.extend_from_slice(&initial_state);
        let state_dst_buf = cuda.load_bytes(&f32_bytes(&state_dst)).unwrap();
        cuda.gated_delta_net_f32_state_offset(
            &q_buf,
            &k_buf,
            &v_buf,
            &log_gate_buf,
            &beta_buf,
            &state_dst_buf,
            state_offset,
            4,
            2,
            1,
            1,
            4,
            4,
            4,
            4,
            8,
            8,
            1,
            2,
            2,
            1,
            1,
            false,
        )
        .unwrap();

        let actual_out = cuda.read_f32s(&state_dst_buf, state_offset).unwrap();
        let actual_all = cuda.read_f32s(&state_dst_buf, state_dst.len()).unwrap();
        let actual_state = &actual_all[state_offset..];
        assert_close("gated_delta_output", &actual_out, &expected, 5.0e-3);
        assert_close("gated_delta_state", actual_state, &expected_state, 2.0e-2);
    }

    #[test]
    fn cuda_qwen_gated_delta_matches_reference_across_steps() {
        if !makepad_ggml::backend::cuda::is_available() {
            return;
        }
        let cuda = CudaRuntime::load().unwrap();
        let q_steps = [
            vec![1.0, 0.5, -0.25, 0.75],
            vec![0.25, -0.75, 1.0, 0.5],
            vec![-0.5, 1.25, 0.75, -0.25],
        ];
        let k_steps = [
            vec![0.25, -0.5, 0.75, 1.0],
            vec![1.0, 0.5, -0.25, -0.75],
            vec![0.5, 0.75, -1.0, 0.25],
        ];
        let v_steps = [
            vec![2.0, 4.0, 6.0, 8.0, 1.5, -2.0, 3.5, 4.5],
            vec![1.0, -1.5, 2.5, 3.0, 0.5, 1.5, -0.5, 2.0],
            vec![3.0, 2.0, 1.0, 0.0, 4.0, 3.0, 2.0, 1.0],
        ];
        let log_gate_steps: [Vec<f32>; 3] = [
            vec![-0.1, -0.4],
            vec![-0.2, -0.3],
            vec![-0.5, -0.05],
        ];
        let beta_steps = [
            vec![0.5, 0.25],
            vec![0.4, 0.3],
            vec![0.2, 0.6],
        ];
        let mut expected_state = vec![
            0.5, -0.25, 0.75, 1.25, //
            -0.5, 0.25, -0.75, 0.5, //
            1.0, -1.0, 0.5, -0.5, //
            0.25, 0.5, -0.25, -0.75, //
            -0.2, 0.4, -0.6, 0.8, //
            0.1, -0.3, 0.5, -0.7, //
            0.9, -0.8, 0.7, -0.6, //
            0.3, -0.2, 0.1, -0.4, //
        ];
        let mut state_dst = vec![0.0f32; 8];
        state_dst.extend_from_slice(&expected_state);
        let state_dst_buf = cuda.load_bytes(&f32_bytes(&state_dst)).unwrap();

        for step in 0..q_steps.len() {
            let q_reference = q_steps[step]
                .iter()
                .map(|value| *value * 0.5)
                .collect::<Vec<_>>();
            let gate = log_gate_steps[step]
                .iter()
                .map(|value| value.exp())
                .collect::<Vec<_>>();
            let expected = gated_delta_net_step_f32(
                &q_reference,
                &k_steps[step],
                &v_steps[step],
                &gate,
                &beta_steps[step],
                &mut expected_state,
                4,
                1,
                4,
                2,
            )
            .unwrap();

            let q_buf = cuda.load_bytes(&f32_bytes(&q_steps[step])).unwrap();
            let k_buf = cuda.load_bytes(&f32_bytes(&k_steps[step])).unwrap();
            let v_buf = cuda.load_bytes(&f32_bytes(&v_steps[step])).unwrap();
            let log_gate_buf = cuda.load_bytes(&f32_bytes(&log_gate_steps[step])).unwrap();
            let beta_buf = cuda.load_bytes(&f32_bytes(&beta_steps[step])).unwrap();
            cuda.gated_delta_net_f32_state_offset(
                &q_buf,
                &k_buf,
                &v_buf,
                &log_gate_buf,
                &beta_buf,
                &state_dst_buf,
                8,
                4,
                2,
                1,
                1,
                4,
                4,
                4,
                4,
                8,
                8,
                1,
                2,
                2,
                1,
                1,
                false,
            )
            .unwrap();

            let actual_out = cuda.read_f32s(&state_dst_buf, 8).unwrap();
            let actual_all = cuda.read_f32s(&state_dst_buf, 8 + expected_state.len()).unwrap();
            let actual_state = &actual_all[8..];
            assert_close(
                &format!("gated_delta_output_step_{step}"),
                &actual_out,
                &expected,
                5.0e-2,
            );
            assert_close(
                &format!("gated_delta_state_step_{step}"),
                actual_state,
                &expected_state,
                1.0e-1,
            );
        }
    }

    #[test]
    fn cuda_qwen_attention_cache_matches_reference() {
        if !makepad_ggml::backend::cuda::is_available() {
            return;
        }
        let cuda = CudaRuntime::load().unwrap();
        let capacity = 4usize;
        let kv_head_count = 1usize;
        let q_head_count = 2usize;
        let q_heads_per_kv = 2usize;
        let head_dim = 2usize;
        let kv_width = kv_head_count * head_dim;
        let query = vec![1.0, 0.0, 0.0, 1.0];
        let key_tokens = [
            vec![1.0, 0.0],
            vec![0.0, 1.0],
            vec![1.0, 1.0],
        ];
        let value_tokens = [
            vec![5.0, 6.0],
            vec![7.0, 8.0],
            vec![9.0, 10.0],
        ];

        let key_cache = cuda
            .alloc_bytes(capacity * kv_width * std::mem::size_of::<u16>())
            .unwrap();
        let value_cache = cuda
            .alloc_bytes(capacity * kv_width * std::mem::size_of::<u16>())
            .unwrap();
        for (slot, (key, value)) in key_tokens.iter().zip(value_tokens.iter()).enumerate() {
            let key_buf = cuda.load_bytes(&f32_bytes(key)).unwrap();
            let value_buf = cuda.load_bytes(&f32_bytes(value)).unwrap();
            cuda.kv_append_f32(
                &key_buf,
                &value_buf,
                &key_cache,
                &value_cache,
                kv_head_count,
                head_dim,
                capacity,
                slot,
            )
            .unwrap();
        }

        let query_buf = cuda.load_bytes(&f32_bytes(&query)).unwrap();
        let logits_buf = cuda.alloc_f32(q_head_count * capacity).unwrap();
        let out_buf = cuda.alloc_f32(query.len()).unwrap();
        let kv_row_stride = capacity * head_dim;
        cuda.attention_logits_seq_f32(
            &query_buf,
            &key_cache,
            &logits_buf,
            q_head_count,
            q_heads_per_kv,
            head_dim,
            kv_row_stride,
            key_tokens.len(),
            0,
            capacity,
            capacity,
        )
        .unwrap();
        cuda.attention_softmax_weighted_sum_f32(
            &logits_buf,
            &value_cache,
            &out_buf,
            q_head_count,
            q_heads_per_kv,
            head_dim,
            kv_row_stride,
            key_tokens.len(),
            0,
            capacity,
            capacity,
            head_dim,
        )
        .unwrap();

        let key_cache_reference = key_tokens
            .iter()
            .flat_map(|row| round_bf16(row))
            .collect::<Vec<_>>();
        let value_cache_reference = value_tokens
            .iter()
            .flat_map(|row| round_bf16(row))
            .collect::<Vec<_>>();
        let expected = grouped_self_attention_step_f32(
            &query,
            &key_cache_reference,
            &value_cache_reference,
            q_head_count,
            kv_head_count,
            head_dim,
            key_tokens.len(),
        )
        .unwrap();
        let actual = cuda.read_f32s(&out_buf, query.len()).unwrap();
        assert_close("attention_cache", &actual, &expected, 1.0e-4);
    }

    #[test]
    fn cuda_qwen_attention_cache_device_u32_matches_reference() {
        if !makepad_ggml::backend::cuda::is_available() {
            return;
        }
        let cuda = CudaRuntime::load().unwrap();
        let capacity = 4usize;
        let kv_head_count = 1usize;
        let q_head_count = 2usize;
        let q_heads_per_kv = 2usize;
        let head_dim = 2usize;
        let kv_width = kv_head_count * head_dim;
        let query = vec![1.0, 0.0, 0.0, 1.0];
        let key_tokens = [
            vec![1.0, 0.0],
            vec![0.0, 1.0],
            vec![1.0, 1.0],
        ];
        let value_tokens = [
            vec![5.0, 6.0],
            vec![7.0, 8.0],
            vec![9.0, 10.0],
        ];

        let key_cache = cuda
            .alloc_bytes(capacity * kv_width * std::mem::size_of::<u16>())
            .unwrap();
        let value_cache = cuda
            .alloc_bytes(capacity * kv_width * std::mem::size_of::<u16>())
            .unwrap();
        for (slot, (key, value)) in key_tokens.iter().zip(value_tokens.iter()).enumerate() {
            let key_buf = cuda.load_bytes(&f32_bytes(key)).unwrap();
            let value_buf = cuda.load_bytes(&f32_bytes(value)).unwrap();
            let slot_buf = scalar_u32_buffer(&cuda, slot as u32);
            cuda.kv_append_f32_device_u32(
                &key_buf,
                &value_buf,
                &key_cache,
                &value_cache,
                kv_head_count,
                head_dim,
                capacity,
                &slot_buf,
            )
            .unwrap();
        }

        let query_buf = cuda.load_bytes(&f32_bytes(&query)).unwrap();
        let logits_buf = cuda.alloc_f32(q_head_count * capacity).unwrap();
        let out_buf = cuda.alloc_f32(query.len()).unwrap();
        let seq_len_buf = scalar_u32_buffer(&cuda, key_tokens.len() as u32);
        let start_slot_buf = scalar_u32_buffer(&cuda, 0);
        let kv_row_stride = capacity * head_dim;
        cuda.attention_logits_seq_f32_device_u32(
            &query_buf,
            &key_cache,
            &logits_buf,
            q_head_count,
            q_heads_per_kv,
            head_dim,
            kv_row_stride,
            &seq_len_buf,
            &start_slot_buf,
            capacity,
            capacity,
        )
        .unwrap();
        cuda.attention_softmax_weighted_sum_f32_device_u32(
            &logits_buf,
            &value_cache,
            &out_buf,
            q_head_count,
            q_heads_per_kv,
            head_dim,
            kv_row_stride,
            &seq_len_buf,
            &start_slot_buf,
            capacity,
            capacity,
            head_dim,
        )
        .unwrap();

        let key_cache_reference = key_tokens
            .iter()
            .flat_map(|row| round_bf16(row))
            .collect::<Vec<_>>();
        let value_cache_reference = value_tokens
            .iter()
            .flat_map(|row| round_bf16(row))
            .collect::<Vec<_>>();
        let expected = grouped_self_attention_step_f32(
            &query,
            &key_cache_reference,
            &value_cache_reference,
            q_head_count,
            kv_head_count,
            head_dim,
            key_tokens.len(),
        )
        .unwrap();
        let actual = cuda.read_f32s(&out_buf, query.len()).unwrap();
        assert_close("attention_cache_device_u32", &actual, &expected, 1.0e-4);
    }

    #[test]
    fn cuda_qwen_kv_append_device_u32_matches_reference_layout() {
        if !makepad_ggml::backend::cuda::is_available() {
            return;
        }
        let cuda = CudaRuntime::load().unwrap();
        let capacity = 4usize;
        let kv_head_count = 1usize;
        let head_dim = 2usize;
        let kv_width = kv_head_count * head_dim;
        let key_tokens = [vec![1.0, 0.0], vec![0.0, 1.0], vec![1.0, 1.0]];
        let value_tokens = [vec![5.0, 6.0], vec![7.0, 8.0], vec![9.0, 10.0]];

        let key_cache = cuda
            .alloc_bytes(capacity * kv_width * std::mem::size_of::<u16>())
            .unwrap();
        let value_cache = cuda
            .alloc_bytes(capacity * kv_width * std::mem::size_of::<u16>())
            .unwrap();
        for (slot, (key, value)) in key_tokens.iter().zip(value_tokens.iter()).enumerate() {
            let key_buf = cuda.load_bytes(&f32_bytes(key)).unwrap();
            let value_buf = cuda.load_bytes(&f32_bytes(value)).unwrap();
            let slot_buf = scalar_u32_buffer(&cuda, slot as u32);
            cuda.kv_append_f32_device_u32(
                &key_buf,
                &value_buf,
                &key_cache,
                &value_cache,
                kv_head_count,
                head_dim,
                capacity,
                &slot_buf,
            )
            .unwrap();
        }

        let actual_key = bf16_words_from_bytes(
            &cuda
                .read_bytes(&key_cache, capacity * kv_width * std::mem::size_of::<u16>())
                .unwrap(),
        );
        let actual_value = bf16_words_from_bytes(
            &cuda
                .read_bytes(&value_cache, capacity * kv_width * std::mem::size_of::<u16>())
                .unwrap(),
        );
        let expected_key = vec![
            qwen_f32_to_bf16_word(1.0),
            qwen_f32_to_bf16_word(0.0),
            qwen_f32_to_bf16_word(0.0),
            qwen_f32_to_bf16_word(1.0),
            qwen_f32_to_bf16_word(1.0),
            qwen_f32_to_bf16_word(1.0),
            0,
            0,
        ];
        let expected_value = vec![
            qwen_f32_to_bf16_word(5.0),
            qwen_f32_to_bf16_word(7.0),
            qwen_f32_to_bf16_word(9.0),
            0,
            qwen_f32_to_bf16_word(6.0),
            qwen_f32_to_bf16_word(8.0),
            qwen_f32_to_bf16_word(10.0),
            0,
        ];
        assert_eq!(actual_key, expected_key);
        assert_eq!(actual_value, expected_value);
    }

    #[test]
    fn cuda_qwen_attention_logits_device_u32_matches_reference() {
        if !makepad_ggml::backend::cuda::is_available() {
            return;
        }
        let cuda = CudaRuntime::load().unwrap();
        let capacity = 4usize;
        let kv_head_count = 1usize;
        let q_head_count = 2usize;
        let q_heads_per_kv = 2usize;
        let head_dim = 2usize;
        let kv_width = kv_head_count * head_dim;
        let query = vec![1.0, 0.0, 0.0, 1.0];
        let key_tokens = [vec![1.0, 0.0], vec![0.0, 1.0], vec![1.0, 1.0]];
        let value_tokens = [vec![5.0, 6.0], vec![7.0, 8.0], vec![9.0, 10.0]];

        let key_cache = cuda
            .alloc_bytes(capacity * kv_width * std::mem::size_of::<u16>())
            .unwrap();
        let value_cache = cuda
            .alloc_bytes(capacity * kv_width * std::mem::size_of::<u16>())
            .unwrap();
        for (slot, (key, value)) in key_tokens.iter().zip(value_tokens.iter()).enumerate() {
            let key_buf = cuda.load_bytes(&f32_bytes(key)).unwrap();
            let value_buf = cuda.load_bytes(&f32_bytes(value)).unwrap();
            let slot_buf = scalar_u32_buffer(&cuda, slot as u32);
            cuda.kv_append_f32_device_u32(
                &key_buf,
                &value_buf,
                &key_cache,
                &value_cache,
                kv_head_count,
                head_dim,
                capacity,
                &slot_buf,
            )
            .unwrap();
        }

        let query_buf = cuda.load_bytes(&f32_bytes(&query)).unwrap();
        let logits_buf = cuda.alloc_f32(q_head_count * capacity).unwrap();
        let seq_len_buf = scalar_u32_buffer(&cuda, key_tokens.len() as u32);
        let start_slot_buf = scalar_u32_buffer(&cuda, 0);
        cuda.attention_logits_seq_f32_device_u32(
            &query_buf,
            &key_cache,
            &logits_buf,
            q_head_count,
            q_heads_per_kv,
            head_dim,
            capacity * head_dim,
            &seq_len_buf,
            &start_slot_buf,
            capacity,
            capacity,
        )
        .unwrap();

        let actual = cuda.read_f32s(&logits_buf, q_head_count * capacity).unwrap();
        let expected = vec![
            qwen_bf16_round_to_f32(1.0),
            qwen_bf16_round_to_f32(0.0),
            qwen_bf16_round_to_f32(1.0),
            0.0,
            qwen_bf16_round_to_f32(0.0),
            qwen_bf16_round_to_f32(1.0),
            qwen_bf16_round_to_f32(1.0),
            0.0,
        ];
        assert_close("attention_logits_device_u32", &actual, &expected, 1.0e-5);
    }

    #[test]
    fn cuda_attention_softmax_weighted_sum_device_u32_matches_non_device() {
        if !makepad_ggml::backend::cuda::is_available() {
            return;
        }
        let cuda = CudaRuntime::load().unwrap();
        let capacity = 4usize;
        let q_head_count = 2usize;
        let q_heads_per_kv = 2usize;
        let head_dim = 2usize;
        let kv_row_stride = capacity * head_dim;
        let logits = [
            1.0f32, 0.0, 1.0, 0.0,
            0.0, 1.0, 1.0, 0.0,
        ];
        let value_cache_words = [
            qwen_f32_to_bf16_word(5.0),
            qwen_f32_to_bf16_word(7.0),
            qwen_f32_to_bf16_word(9.0),
            0,
            qwen_f32_to_bf16_word(6.0),
            qwen_f32_to_bf16_word(8.0),
            qwen_f32_to_bf16_word(10.0),
            0,
        ];
        let logits_non_device = cuda.load_bytes(&f32_bytes(&logits)).unwrap();
        let logits_device = cuda.load_bytes(&f32_bytes(&logits)).unwrap();
        let value_cache = cuda.load_bytes(&u16_bytes(&value_cache_words)).unwrap();
        let out_non_device = cuda.alloc_f32(q_head_count * head_dim).unwrap();
        let out_device = cuda.alloc_f32(q_head_count * head_dim).unwrap();
        let seq_len_buf = scalar_u32_buffer(&cuda, 3);
        let start_slot_buf = scalar_u32_buffer(&cuda, 0);

        cuda.attention_softmax_weighted_sum_f32(
            &logits_non_device,
            &value_cache,
            &out_non_device,
            q_head_count,
            q_heads_per_kv,
            head_dim,
            kv_row_stride,
            3,
            0,
            capacity,
            capacity,
            head_dim,
        )
        .unwrap();
        cuda.attention_softmax_weighted_sum_f32_device_u32(
            &logits_device,
            &value_cache,
            &out_device,
            q_head_count,
            q_heads_per_kv,
            head_dim,
            kv_row_stride,
            &seq_len_buf,
            &start_slot_buf,
            capacity,
            capacity,
            head_dim,
        )
        .unwrap();

        let expected = cuda.read_f32s(&out_non_device, q_head_count * head_dim).unwrap();
        let actual = cuda.read_f32s(&out_device, q_head_count * head_dim).unwrap();
        assert_close(
            "attention_softmax_weighted_sum_device_u32",
            &actual,
            &expected,
            1.0e-5,
        );
    }

    #[test]
    fn cuda_affine_get_row_matches_reference() {
        if !makepad_ggml::backend::cuda::is_available() {
            return;
        }
        let cuda = CudaRuntime::load().unwrap();
        let packed_row = vec![0x0101_0101u32; 16];
        let packed_weights = packed_row
            .iter()
            .copied()
            .chain(vec![0x0202_0202u32; 16])
            .collect::<Vec<_>>();
        let scales = vec![qwen_f32_to_bf16_word(1.0), qwen_f32_to_bf16_word(1.0)];
        let biases = vec![qwen_f32_to_bf16_word(0.0), qwen_f32_to_bf16_word(0.0)];
        let tensor = CudaAffineTensor {
            packed_weights: cuda.load_bytes(&u32_bytes(&packed_weights)).unwrap(),
            scales: cuda.load_bytes(&u16_bytes(&scales)).unwrap(),
            biases: cuda.load_bytes(&u16_bytes(&biases)).unwrap(),
            bits: 8,
            out_rows: 2,
            weight_words_per_row: 16,
            qparams_per_row: 1,
            plane_count: 1,
            weight_words_per_plane: 32,
            qparams_words_per_plane: 2,
        };
        let out_buf = cuda.alloc_f32(64).unwrap();
        tensor.get_row(&cuda, 1, &out_buf).unwrap();
        let actual = cuda.read_f32s(&out_buf, 64).unwrap();
        let expected_row = vec![0x0202_0202u32; 16];
        let expected = affine_dequantize_row_f32(
            &expected_row,
            &[scales[1]],
            &[biases[1]],
            64,
            8,
            4,
        )
        .unwrap();
        assert_close("affine_get_row", &actual, &expected, 1.0e-5);
    }

    #[test]
    fn cuda_affine_get_row_device_u32_matches_reference() {
        if !makepad_ggml::backend::cuda::is_available() {
            return;
        }
        let cuda = CudaRuntime::load().unwrap();
        let packed_row = vec![0x0101_0101u32; 16];
        let packed_weights = packed_row
            .iter()
            .copied()
            .chain(vec![0x0202_0202u32; 16])
            .collect::<Vec<_>>();
        let scales = vec![qwen_f32_to_bf16_word(1.0), qwen_f32_to_bf16_word(1.0)];
        let biases = vec![qwen_f32_to_bf16_word(0.0), qwen_f32_to_bf16_word(0.0)];
        let tensor = CudaAffineTensor {
            packed_weights: cuda.load_bytes(&u32_bytes(&packed_weights)).unwrap(),
            scales: cuda.load_bytes(&u16_bytes(&scales)).unwrap(),
            biases: cuda.load_bytes(&u16_bytes(&biases)).unwrap(),
            bits: 8,
            out_rows: 2,
            weight_words_per_row: 16,
            qparams_per_row: 1,
            plane_count: 1,
            weight_words_per_plane: 32,
            qparams_words_per_plane: 2,
        };
        let row_index = scalar_u32_buffer(&cuda, 1);
        let out_buf = cuda.alloc_f32(64).unwrap();
        tensor.get_row_device_u32(&cuda, &row_index, &out_buf).unwrap();
        let actual = cuda.read_f32s(&out_buf, 64).unwrap();
        let expected_row = vec![0x0202_0202u32; 16];
        let expected = affine_dequantize_row_f32(
            &expected_row,
            &[scales[1]],
            &[biases[1]],
            64,
            8,
            4,
        )
        .unwrap();
        assert_close("affine_get_row_device_u32", &actual, &expected, 1.0e-5);
    }

    #[test]
    fn cuda_affine_matvec_and_plane_match_reference() {
        if !makepad_ggml::backend::cuda::is_available() {
            return;
        }
        let cuda = CudaRuntime::load().unwrap();
        let input = (1..=64)
            .map(|value| qwen_f32_to_bf16_word(value as f32))
            .collect::<Vec<_>>();
        let input_buf = cuda.load_bytes(&u16_bytes(&input)).unwrap();

        let row_plane0 = vec![0x0101_0101u32; 16];
        let row_plane1 = vec![0x0202_0202u32; 16];
        let packed_weights = row_plane0
            .iter()
            .copied()
            .chain(row_plane0.iter().copied())
            .chain(row_plane1.iter().copied())
            .chain(row_plane1.iter().copied())
            .collect::<Vec<_>>();
        let scales = vec![
            qwen_f32_to_bf16_word(1.0),
            qwen_f32_to_bf16_word(1.0),
            qwen_f32_to_bf16_word(1.0),
            qwen_f32_to_bf16_word(1.0),
        ];
        let biases = vec![
            qwen_f32_to_bf16_word(0.0),
            qwen_f32_to_bf16_word(0.0),
            qwen_f32_to_bf16_word(0.0),
            qwen_f32_to_bf16_word(0.0),
        ];
        let tensor = CudaAffineTensor {
            packed_weights: cuda.load_bytes(&u32_bytes(&packed_weights)).unwrap(),
            scales: cuda.load_bytes(&u16_bytes(&scales)).unwrap(),
            biases: cuda.load_bytes(&u16_bytes(&biases)).unwrap(),
            bits: 8,
            out_rows: 2,
            weight_words_per_row: 16,
            qparams_per_row: 1,
            plane_count: 2,
            weight_words_per_plane: 32,
            qparams_words_per_plane: 2,
        };
        let out_buf = cuda.alloc_f32(2).unwrap();
        tensor.matvec_plane(&cuda, &input_buf, &out_buf, 0).unwrap();
        let actual_plane0 = cuda.read_f32s(&out_buf, 2).unwrap();
        tensor.matvec_plane(&cuda, &input_buf, &out_buf, 1).unwrap();
        let actual_plane1 = cuda.read_f32s(&out_buf, 2).unwrap();

        let expected_plane0 = affine_quantized_matmul_fallback(
            &input,
            &[0x0101_0101u32; 32],
            &[scales[0], scales[1]],
            &[biases[0], biases[1]],
            2,
            16,
            1,
            64,
            8,
        )
        .unwrap();
        let expected_plane1 = affine_quantized_matmul_fallback(
            &input,
            &[0x0202_0202u32; 32],
            &[scales[2], scales[3]],
            &[biases[2], biases[3]],
            2,
            16,
            1,
            64,
            8,
        )
        .unwrap();

        assert_close("affine_matvec_plane0", &actual_plane0, &expected_plane0, 1.0e-5);
        assert_close("affine_matvec_plane1", &actual_plane1, &expected_plane1, 1.0e-5);
    }

    #[test]
    fn cuda_affine_selected_plane_matches_reference() {
        if !makepad_ggml::backend::cuda::is_available() {
            return;
        }
        let cuda = CudaRuntime::load().unwrap();
        let input = (1..=64)
            .map(|value| qwen_f32_to_bf16_word(value as f32))
            .collect::<Vec<_>>();
        let input_buf = cuda.load_bytes(&u16_bytes(&input)).unwrap();

        let row_plane0 = vec![0x0101_0101u32; 16];
        let row_plane1 = vec![0x0202_0202u32; 16];
        let packed_weights = row_plane0
            .iter()
            .copied()
            .chain(row_plane0.iter().copied())
            .chain(row_plane1.iter().copied())
            .chain(row_plane1.iter().copied())
            .collect::<Vec<_>>();
        let scales = vec![
            qwen_f32_to_bf16_word(1.0),
            qwen_f32_to_bf16_word(1.0),
            qwen_f32_to_bf16_word(1.0),
            qwen_f32_to_bf16_word(1.0),
        ];
        let biases = vec![
            qwen_f32_to_bf16_word(0.0),
            qwen_f32_to_bf16_word(0.0),
            qwen_f32_to_bf16_word(0.0),
            qwen_f32_to_bf16_word(0.0),
        ];
        let tensor = CudaAffineTensor {
            packed_weights: cuda.load_bytes(&u32_bytes(&packed_weights)).unwrap(),
            scales: cuda.load_bytes(&u16_bytes(&scales)).unwrap(),
            biases: cuda.load_bytes(&u16_bytes(&biases)).unwrap(),
            bits: 8,
            out_rows: 2,
            weight_words_per_row: 16,
            qparams_per_row: 1,
            plane_count: 2,
            weight_words_per_plane: 32,
            qparams_words_per_plane: 2,
        };
        let plane_indices = cuda.load_bytes(&u32_bytes(&[1u32])).unwrap();
        let out_buf = cuda.alloc_f32(2).unwrap();
        tensor
            .matvec_plane_device_index(&cuda, &input_buf, &out_buf, &plane_indices, 0)
            .unwrap();
        let actual = cuda.read_f32s(&out_buf, 2).unwrap();
        let expected = affine_quantized_matmul_fallback(
            &input,
            &packed_weights[32..64],
            &scales[2..4],
            &biases[2..4],
            2,
            16,
            1,
            64,
            8,
        )
        .unwrap();
        assert_close("affine_selected_plane", &actual, &expected, 1.0e-5);
    }

    #[test]
    fn cuda_affine_selected_planes_match_reference() {
        if !makepad_ggml::backend::cuda::is_available() {
            return;
        }
        let cuda = CudaRuntime::load().unwrap();
        let input = (1..=64)
            .map(|value| qwen_f32_to_bf16_word(value as f32))
            .collect::<Vec<_>>();
        let input_buf = cuda.load_bytes(&u16_bytes(&input)).unwrap();

        let row_plane0 = vec![0x0101_0101u32; 16];
        let row_plane1 = vec![0x0202_0202u32; 16];
        let packed_weights = row_plane0
            .iter()
            .copied()
            .chain(row_plane0.iter().copied())
            .chain(row_plane1.iter().copied())
            .chain(row_plane1.iter().copied())
            .collect::<Vec<_>>();
        let scales = vec![
            qwen_f32_to_bf16_word(1.0),
            qwen_f32_to_bf16_word(1.0),
            qwen_f32_to_bf16_word(1.0),
            qwen_f32_to_bf16_word(1.0),
        ];
        let biases = vec![
            qwen_f32_to_bf16_word(0.0),
            qwen_f32_to_bf16_word(0.0),
            qwen_f32_to_bf16_word(0.0),
            qwen_f32_to_bf16_word(0.0),
        ];
        let tensor = CudaAffineTensor {
            packed_weights: cuda.load_bytes(&u32_bytes(&packed_weights)).unwrap(),
            scales: cuda.load_bytes(&u16_bytes(&scales)).unwrap(),
            biases: cuda.load_bytes(&u16_bytes(&biases)).unwrap(),
            bits: 8,
            out_rows: 2,
            weight_words_per_row: 16,
            qparams_per_row: 1,
            plane_count: 2,
            weight_words_per_plane: 32,
            qparams_words_per_plane: 2,
        };
        let plane_indices = cuda.load_bytes(&u32_bytes(&[1u32, 0u32])).unwrap();
        let out_buf = cuda.alloc_f32(4).unwrap();
        tensor
            .matvec_planes_device_indices(&cuda, &input_buf, &out_buf, &plane_indices, 2)
            .unwrap();
        let actual = cuda.read_f32s(&out_buf, 4).unwrap();
        let expected_plane1 = affine_quantized_matmul_fallback(
            &input,
            &packed_weights[32..64],
            &scales[2..4],
            &biases[2..4],
            2,
            16,
            1,
            64,
            8,
        )
        .unwrap();
        let expected_plane0 = affine_quantized_matmul_fallback(
            &input,
            &packed_weights[0..32],
            &scales[0..2],
            &biases[0..2],
            2,
            16,
            1,
            64,
            8,
        )
        .unwrap();
        let expected = expected_plane1
            .into_iter()
            .chain(expected_plane0)
            .collect::<Vec<_>>();
        assert_close("affine_selected_planes", &actual, &expected, 1.0e-5);
    }

    #[test]
    fn cuda_qwen_softmax_topk_routes_matches_reference() {
        if !makepad_ggml::backend::cuda::is_available() {
            return;
        }
        let cuda = CudaRuntime::load().unwrap();
        let logits = [1.0f32, 3.0, 2.0, -1.0];
        let logits_buf = cuda.load_bytes(&f32_bytes(&logits)).unwrap();
        let indices_buf = cuda.alloc_u32(2).unwrap();
        let weights_buf = cuda.alloc_f32(2).unwrap();
        cuda.qwen_softmax_topk_routes_f32(&logits_buf, &indices_buf, &weights_buf, logits.len(), 2)
            .unwrap();
        let actual_indices = cuda
            .read_bytes(&indices_buf, 2 * std::mem::size_of::<u32>())
            .unwrap()
            .chunks_exact(4)
            .map(|chunk| u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
            .collect::<Vec<_>>();
        let actual_weights = cuda.read_f32s(&weights_buf, 2).unwrap();
        let (_probabilities, expected_routes) = softmax_top_k_routes(&logits, 2).unwrap();
        let expected_indices = expected_routes
            .iter()
            .map(|route| route.expert_index)
            .collect::<Vec<_>>();
        let expected_weights = expected_routes
            .iter()
            .map(|route| route.weight)
            .collect::<Vec<_>>();
        assert_eq!(actual_indices, expected_indices);
        assert_close("qwen_softmax_topk_routes", &actual_weights, &expected_weights, 1.0e-6);
    }

    #[test]
    fn cuda_masked_argmax_device_u32_matches_reference() {
        if !makepad_ggml::backend::cuda::is_available() {
            return;
        }
        let cuda = CudaRuntime::load().unwrap();
        let logits = [1.0f32, 7.0, 3.0, 5.0, 6.0];
        let masked = [1u32, 4u32];
        let logits_buf = cuda.load_bytes(&f32_bytes(&logits)).unwrap();
        let masked_buf = cuda.load_bytes(&u32_bytes(&masked)).unwrap();
        let masked_count_buf = scalar_u32_buffer(&cuda, masked.len() as u32);
        let out_buf = cuda.alloc_u32(1).unwrap();
        cuda.masked_argmax_f32_device_u32(
            &logits_buf,
            &masked_buf,
            &masked_count_buf,
            &out_buf,
            logits.len(),
        )
        .unwrap();
        let actual = cuda.read_u32(&out_buf).unwrap();
        assert_eq!(actual, 3);
    }

    #[test]
    fn cuda_masked_argmax_device_u32_handles_zero_mask_count() {
        if !makepad_ggml::backend::cuda::is_available() {
            return;
        }
        let cuda = CudaRuntime::load().unwrap();
        let logits = [1.0f32, 7.0, 3.0, 5.0, 6.0];
        let masked_buf = cuda.alloc_u32(1).unwrap();
        let masked_count_buf = scalar_u32_buffer(&cuda, 0);
        let logits_buf = cuda.load_bytes(&f32_bytes(&logits)).unwrap();
        let out_buf = cuda.alloc_u32(1).unwrap();
        cuda.masked_argmax_f32_device_u32(
            &logits_buf,
            &masked_buf,
            &masked_count_buf,
            &out_buf,
            logits.len(),
        )
        .unwrap();
        let actual = cuda.read_u32(&out_buf).unwrap();
        assert_eq!(actual, 1);
    }

    #[test]
    fn cuda_real_token_embedding_row_matches_reference() {
        if !makepad_ggml::backend::cuda::is_available() {
            return;
        }
        let Some(model_dir) = real_qwen_model_dir() else {
            return;
        };
        let runtime_session = MlxQwen35MoeRuntimeSession::load(&model_dir).unwrap();
        let cuda_runtime = CudaQwenTextRuntime::load(runtime_session.as_ref()).unwrap();
        let token_id = 248045u32;

        let expected = runtime_session.token_embedding_f32(token_id).unwrap();
        let out_buf = cuda_runtime.cuda.alloc_f32(expected.len()).unwrap();
        cuda_runtime
            .token_embd
            .get_row(&cuda_runtime.cuda, token_id as usize, &out_buf)
            .unwrap();
        let actual = cuda_runtime
            .cuda
            .read_f32s(&out_buf, expected.len())
            .unwrap();
        assert_close("real_token_embedding_row", &actual, &expected, 1.0e-4);
    }

    #[test]
    fn cuda_real_layer0_recurrent_projections_match_reference() {
        if !makepad_ggml::backend::cuda::is_available() {
            return;
        }
        let Some(model_dir) = real_qwen_model_dir() else {
            return;
        };
        let runtime_session = MlxQwen35MoeRuntimeSession::load(&model_dir).unwrap();
        let cuda_runtime = CudaQwenTextRuntime::load(runtime_session.as_ref()).unwrap();
        let layer = &runtime_session.tensors.layers[0];
        let recurrent = layer.recurrent.as_ref().unwrap();
        let hidden = runtime_session.token_embedding_f32(248045).unwrap();
        let attn_input = runtime_session
            .rms_norm_weighted_f32(
                &hidden,
                &layer.attn_norm,
                runtime_session.weights.snapshot.config.text_config.rms_norm_eps,
            )
            .unwrap();
        let input_words = f32_to_bf16_words(&attn_input);
        let input_buf = cuda_runtime.cuda.load_bytes(&u16_bytes(&input_words)).unwrap();

        let expected_qkv =
            cpu_project_vector_bf16_words(runtime_session.as_ref(), &input_words, &recurrent.wqkv);
        let qkv_buf = cuda_runtime.cuda.alloc_f32(expected_qkv.len()).unwrap();
        let layer0 = match &cuda_runtime.layers[0] {
            CudaQwenLayer::Recurrent(layer) => layer,
            CudaQwenLayer::Attention(_) => panic!("expected recurrent layer 0"),
        };
        layer0
            .wqkv
            .matvec(&cuda_runtime.cuda, &input_buf, &qkv_buf)
            .unwrap();
        let actual_qkv = cuda_runtime
            .cuda
            .read_f32s(&qkv_buf, expected_qkv.len())
            .unwrap();
        assert_close("real_layer0_wqkv", &actual_qkv, &expected_qkv, 1.0e-4);

        let expected_z = cpu_project_vector_bf16_words(
            runtime_session.as_ref(),
            &input_words,
            &recurrent.wqkv_gate,
        );
        let z_buf = cuda_runtime.cuda.alloc_f32(expected_z.len()).unwrap();
        layer0
            .wqkv_gate
            .matvec(&cuda_runtime.cuda, &input_buf, &z_buf)
            .unwrap();
        let actual_z = cuda_runtime
            .cuda
            .read_f32s(&z_buf, expected_z.len())
            .unwrap();
        assert_close("real_layer0_wqkv_gate", &actual_z, &expected_z, 1.0e-4);

        let expected_beta = cpu_project_vector_bf16_words(
            runtime_session.as_ref(),
            &input_words,
            &recurrent.ssm_beta,
        );
        let beta_buf = cuda_runtime.cuda.alloc_f32(expected_beta.len()).unwrap();
        layer0
            .ssm_beta
            .matvec(&cuda_runtime.cuda, &input_buf, &beta_buf)
            .unwrap();
        let actual_beta = cuda_runtime
            .cuda
            .read_f32s(&beta_buf, expected_beta.len())
            .unwrap();
        assert_close("real_layer0_ssm_beta", &actual_beta, &expected_beta, 1.0e-4);

        let expected_alpha = cpu_project_vector_bf16_words(
            runtime_session.as_ref(),
            &input_words,
            &recurrent.ssm_alpha,
        );
        let alpha_buf = cuda_runtime.cuda.alloc_f32(expected_alpha.len()).unwrap();
        layer0
            .ssm_alpha
            .matvec(&cuda_runtime.cuda, &input_buf, &alpha_buf)
            .unwrap();
        let actual_alpha = cuda_runtime
            .cuda
            .read_f32s(&alpha_buf, expected_alpha.len())
            .unwrap();
        assert_close("real_layer0_ssm_alpha", &actual_alpha, &expected_alpha, 1.0e-4);
    }

    #[test]
    fn cuda_real_graph_backend_matches_non_graph_first_steps() {
        if !makepad_ggml::backend::cuda::is_available() {
            return;
        }
        let Some(model_dir) = real_qwen_model_dir() else {
            return;
        };
        let runtime_session = MlxQwen35MoeRuntimeSession::load(&model_dir).unwrap();
        let prompt_text = runtime_session
            .format_chat_prompt(
                &[QwenChatMessage::new(
                    QwenChatRole::User,
                    "Write a short haiku about rain.",
                )],
                false,
            )
            .unwrap();
        let prompt_token_ids = runtime_session.tokenize_prompt(&prompt_text).unwrap();
        let compare_steps = 8usize;
        let capacity_tokens = prompt_token_ids.len() + compare_steps + 1;
        let runtime = runtime_session.cuda_text_runtime().unwrap();
        let disallowed_token_ids = runtime_session.generation_disallowed_token_ids();
        let sampling = runtime_session.sampling_options(false);

        let (non_graph_session, graph_session, decode_graph) = {
            let runtime_guard = runtime.lock().unwrap();
            let non_graph_session = runtime_guard
                .new_decode_session(capacity_tokens, &disallowed_token_ids)
                .unwrap();
            let mut graph_session = runtime_guard
                .new_decode_session(capacity_tokens, &disallowed_token_ids)
                .unwrap();
            let decode_graph = runtime_guard.capture_decode_graph(&mut graph_session).unwrap();
            (non_graph_session, graph_session, decode_graph)
        };

        let mut non_graph = QwenCudaGenerationBackend {
            runtime: runtime.clone(),
            session: non_graph_session,
            prefill_graph: None,
            decode_graph: None,
            decode_chunk_graphs: Vec::new(),
            sampling,
            disallowed_token_ids: disallowed_token_ids.clone(),
            rng: QwenSamplingRng::new(0),
            debug_compare: None,
        };
        let mut graph = QwenCudaGenerationBackend {
            runtime,
            session: graph_session,
            prefill_graph: None,
            decode_graph: Some(decode_graph),
            decode_chunk_graphs: Vec::new(),
            sampling,
            disallowed_token_ids,
            rng: QwenSamplingRng::new(0),
            debug_compare: None,
        };

        let mut last_non_graph = 0u32;
        let mut last_graph = 0u32;
        for (position, &token_id) in prompt_token_ids.iter().enumerate() {
            last_non_graph = non_graph.eval_and_select(token_id, position).unwrap();
            last_graph = graph.eval_and_select(token_id, position).unwrap();
            if last_graph != last_non_graph {
                let (non_graph_logits, graph_logits) = {
                    let runtime = non_graph.runtime.lock().unwrap();
                    let non_graph_logits = runtime
                        .cuda
                        .read_f32s(&non_graph.session.workspace.logits, runtime.vocab_size)
                        .unwrap();
                    let graph_logits = runtime
                        .cuda
                        .read_f32s(&graph.session.workspace.logits, runtime.vocab_size)
                        .unwrap();
                    (non_graph_logits, graph_logits)
                };
                let non_graph_raw_top1 = argmax_index(&non_graph_logits);
                let graph_raw_top1 = argmax_index(&graph_logits);
                let non_graph_masked_top1 = select_top1_token_from_logits(
                    &non_graph_logits,
                    &non_graph.disallowed_token_ids,
                )
                .unwrap()
                .token_id;
                let graph_masked_top1 =
                    select_top1_token_from_logits(&graph_logits, &graph.disallowed_token_ids)
                        .unwrap()
                        .token_id;
                let graph_nan_count = graph_logits.iter().filter(|value| value.is_nan()).count();
                let non_graph_nan_count =
                    non_graph_logits.iter().filter(|value| value.is_nan()).count();
                panic!(
                    "graph/non-graph prefill mismatch at position {position}: graph_token={} non_graph_token={} graph_raw_top1={} non_graph_raw_top1={} graph_masked_top1={} non_graph_masked_top1={} graph_raw_top1_value={} non_graph_raw_top1_value={} graph_index0={} non_graph_index0={} graph_nan_count={} non_graph_nan_count={} logits_max_abs_diff={}",
                    last_graph,
                    last_non_graph,
                    graph_raw_top1,
                    non_graph_raw_top1,
                    graph_masked_top1,
                    non_graph_masked_top1,
                    graph_logits[graph_raw_top1],
                    non_graph_logits[non_graph_raw_top1],
                    graph_logits[0],
                    non_graph_logits[0],
                    graph_nan_count,
                    non_graph_nan_count,
                    max_abs_diff(&graph_logits, &non_graph_logits),
                );
            }
        }

        for step in 0..compare_steps {
            assert_eq!(
                last_graph, last_non_graph,
                "graph/non-graph decode mismatch before step {step}"
            );
            let decode_position = prompt_token_ids.len() + step;
            last_non_graph = non_graph.eval_and_select(last_non_graph, decode_position).unwrap();
            last_graph = graph.eval_and_select(last_graph, decode_position).unwrap();
        }
    }

    #[test]
    fn cuda_real_chunk_backend_matches_single_step_first_tokens() {
        if !makepad_ggml::backend::cuda::is_available() {
            return;
        }
        let Some(model_dir) = real_qwen_model_dir() else {
            return;
        };
        let runtime_session = MlxQwen35MoeRuntimeSession::load(&model_dir).unwrap();
        let prompt_text = runtime_session
            .format_chat_prompt(
                &[QwenChatMessage::new(
                    QwenChatRole::User,
                    "Write a short haiku about rain.",
                )],
                false,
            )
            .unwrap();
        let prompt_token_ids = runtime_session.tokenize_prompt(&prompt_text).unwrap();
        let generated_count = 8usize;
        let capacity_tokens = prompt_token_ids.len() + generated_count + 1;

        let mut single_step_backend =
            QwenCudaGenerationBackend::new(runtime_session.clone(), capacity_tokens, false)
                .unwrap();
        let mut chunk_backend =
            QwenCudaGenerationBackend::new(runtime_session, capacity_tokens, false).unwrap();

        let mut single_step_tokens = Vec::new();
        let mut next_single = single_step_backend.prefill_prompt(&prompt_token_ids).unwrap();
        single_step_tokens.push(next_single);
        for step in 1..generated_count {
            let position = prompt_token_ids.len() + step - 1;
            next_single = single_step_backend.eval_next_token(next_single, position).unwrap();
            single_step_tokens.push(next_single);
        }

        let first_chunk = chunk_backend.prefill_prompt(&prompt_token_ids).unwrap();
        let mut chunk_tokens = vec![first_chunk];
        let remaining = generated_count - 1;
        let start_position = prompt_token_ids.len();
        chunk_tokens.extend(
            chunk_backend
                .eval_token_chunk(first_chunk, start_position, remaining)
                .unwrap(),
        );

        assert_eq!(chunk_tokens, single_step_tokens);
    }

    #[test]
    fn cuda_graph_path_without_capture_matches_non_graph_first_step() {
        if !makepad_ggml::backend::cuda::is_available() {
            return;
        }
        let Some(model_dir) = real_qwen_model_dir() else {
            return;
        };
        let runtime_session = MlxQwen35MoeRuntimeSession::load(&model_dir).unwrap();
        let prompt_text = runtime_session
            .format_chat_prompt(
                &[QwenChatMessage::new(
                    QwenChatRole::User,
                    "Write a short haiku about rain.",
                )],
                false,
            )
            .unwrap();
        let prompt_token_ids = runtime_session.tokenize_prompt(&prompt_text).unwrap();
        let token_id = prompt_token_ids[0];
        let disallowed_token_ids = runtime_session.generation_disallowed_token_ids();
        let runtime = runtime_session.cuda_text_runtime().unwrap();
        let (non_graph_logits, graph_logits) = {
            let runtime_guard = runtime.lock().unwrap();
            let mut non_graph_session = runtime_guard
                .new_decode_session(4, &disallowed_token_ids)
                .unwrap();
            let mut graph_session = runtime_guard
                .new_decode_session(4, &disallowed_token_ids)
                .unwrap();
            runtime_guard
                .eval_token_logits(&mut non_graph_session, token_id, 0)
                .unwrap();
            let token_state = runtime_guard.alloc_graph_token_state().unwrap();
            runtime_guard
                .write_graph_token_state(&token_state, token_id, 0, disallowed_token_ids.len())
                .unwrap();
            runtime_guard
                .eval_token_logits_graph(&mut graph_session, &token_state)
                .unwrap();
            let non_graph_logits = runtime_guard
                .cuda
                .read_f32s(&non_graph_session.workspace.logits, runtime_guard.vocab_size)
                .unwrap();
            let graph_logits = runtime_guard
                .cuda
                .read_f32s(&graph_session.workspace.logits, runtime_guard.vocab_size)
                .unwrap();
            (non_graph_logits, graph_logits)
        };
        assert_eq!(
            graph_logits.iter().filter(|value| value.is_nan()).count(),
            0,
            "graph path without capture produced NaN logits"
        );
        assert_close("graph_path_without_capture", &graph_logits, &non_graph_logits, 1.0e-4);
    }

    #[test]
    #[ignore]
    fn cuda_real_first_step_states_match_reference() {
        if !makepad_ggml::backend::cuda::is_available() {
            return;
        }
        let Some(model_dir) = real_qwen_model_dir() else {
            return;
        };
        let runtime_session = MlxQwen35MoeRuntimeSession::load(&model_dir).unwrap();
        let cuda_runtime = CudaQwenTextRuntime::load(runtime_session.as_ref()).unwrap();
        let mut cuda_session = cuda_runtime.new_decode_session(1, &[]).unwrap();
        let mut reference_state = runtime_session.new_decode_state().unwrap();
        let token_id = 248045u32;

        runtime_session
            .eval_token_logits_reference_f32(token_id, 0, &mut reference_state)
            .unwrap();
        cuda_runtime
            .eval_token_logits(&mut cuda_session, token_id, 0)
            .unwrap();

        for (layer_index, (cuda_state, reference_layer)) in cuda_session
            .layer_states
            .iter()
            .zip(reference_state.layers.iter())
            .enumerate()
        {
            match (cuda_state, reference_layer) {
                (
                    CudaQwenLayerState::Attention(cuda_attn),
                    MlxQwen35MoeLayerDecodeState::Attention(reference_attn),
                ) => {
                    let key_words = bf16_words_from_bytes(
                        &cuda_runtime
                            .cuda
                            .read_bytes(
                                &cuda_attn.key_cache,
                                reference_attn.key_cache.len() * std::mem::size_of::<u16>(),
                            )
                            .unwrap(),
                    );
                    let value_words = bf16_words_from_bytes(
                        &cuda_runtime
                            .cuda
                            .read_bytes(
                                &cuda_attn.value_cache,
                                reference_attn.value_cache.len() * std::mem::size_of::<u16>(),
                            )
                            .unwrap(),
                    );
                    let actual_key = key_words
                        .into_iter()
                        .map(qwen_bf16_word_to_f32)
                        .collect::<Vec<_>>();
                    let actual_value = value_words
                        .into_iter()
                        .map(qwen_bf16_word_to_f32)
                        .collect::<Vec<_>>();
                    assert_close(
                        &format!("state_layer_{layer_index}_key_cache"),
                        &actual_key,
                        &round_bf16(&reference_attn.key_cache),
                        1.0e-4,
                    );
                    assert_close(
                        &format!("state_layer_{layer_index}_value_cache"),
                        &actual_value,
                        &round_bf16(&reference_attn.value_cache),
                        1.0e-4,
                    );
                }
                (
                    CudaQwenLayerState::Recurrent(cuda_recurrent),
                    MlxQwen35MoeLayerDecodeState::Recurrent(reference_recurrent),
                ) => {
                    let actual_conv = cuda_runtime
                        .cuda
                        .read_f32s(
                            &cuda_recurrent.conv_state,
                            reference_recurrent.conv_state.len(),
                        )
                        .unwrap();
                    assert_close(
                        &format!("state_layer_{layer_index}_conv_state"),
                        &actual_conv,
                        &reference_recurrent.conv_state,
                        1.0e-4,
                    );

                    let state_offset = cuda_runtime.recurrent_v_width;
                    let actual_gated = cuda_runtime
                        .cuda
                        .read_f32s(
                            &cuda_recurrent.gated_delta,
                            state_offset + reference_recurrent.ssm_state.len(),
                        )
                        .unwrap();
                    let actual_state = &actual_gated[state_offset..];
                    assert_close(
                        &format!("state_layer_{layer_index}_ssm_state"),
                        actual_state,
                        &reference_recurrent.ssm_state,
                        1.0e-4,
                    );
                }
                _ => panic!("state kind mismatch at layer {layer_index}"),
            }
        }
    }

    #[test]
    #[ignore]
    fn qwen_real_cuda_prefill_top1_matches_reference_first_steps() {
        if !makepad_ggml::backend::cuda::is_available() {
            return;
        }
        let Some(model_dir) = real_qwen_model_dir() else {
            return;
        };
        let runtime_session = MlxQwen35MoeRuntimeSession::load(&model_dir).unwrap();
        let prompt = runtime_session
            .tokenizer
            .encode("Write a short haiku about rain.")
            .unwrap();
        assert!(!prompt.is_empty());
        let steps = prompt.len().min(4);

        let mut reference_state = runtime_session.new_decode_state().unwrap();
        let cuda_runtime = CudaQwenTextRuntime::load(runtime_session.as_ref()).unwrap();
        let mut cuda_session = cuda_runtime.new_decode_session(steps + 1, &[]).unwrap();

        for (position, &token_id) in prompt.iter().take(steps).enumerate() {
            let reference_logits = runtime_session
                .eval_token_logits_reference_f32(token_id, position, &mut reference_state)
                .unwrap();
            cuda_runtime
                .eval_token_logits(&mut cuda_session, token_id, position)
                .unwrap();
            let cuda_logits = cuda_runtime
                .cuda
                .read_f32s(&cuda_session.workspace.logits, cuda_runtime.vocab_size)
                .unwrap();
            let reference_top1 = argmax_index(&reference_logits);
            let cuda_top1 = argmax_index(&cuda_logits);
            assert_eq!(
                cuda_top1,
                reference_top1,
                "prefill top1 mismatch at position {position}: cuda token {cuda_top1} ref token {reference_top1}"
            );
        }
    }

    #[test]
    #[ignore]
    fn cuda_real_layer6_moe_matches_reference_from_same_ffn_input() {
        if !makepad_ggml::backend::cuda::is_available() {
            return;
        }
        let Some(model_dir) = real_qwen_model_dir() else {
            return;
        };
        let runtime_session = MlxQwen35MoeRuntimeSession::load(&model_dir).unwrap();
        let cuda_runtime = CudaQwenTextRuntime::load(runtime_session.as_ref()).unwrap();
        let prompt = runtime_session
            .format_chat_prompt(&[QwenChatMessage::new(QwenChatRole::User, "Hello")], false)
            .unwrap();
        let prompt_token_ids = runtime_session.tokenize_prompt(&prompt).unwrap();
        assert!(prompt_token_ids.len() >= 2);

        let target_layer = 6usize;
        let position = 1usize;
        let mut decode_state = runtime_session.new_decode_state().unwrap();
        runtime_session
            .eval_token_logits_reference_f32(prompt_token_ids[0], 0, &mut decode_state)
            .unwrap();

        let mut hidden = runtime_session.token_embedding_f32(prompt_token_ids[position]).unwrap();
        for layer_index in 0..target_layer {
            runtime_session
                .apply_layer_decode_reference_f32(
                    layer_index,
                    position,
                    &mut hidden,
                    &mut decode_state,
                )
                .unwrap();
        }

        let layer = &runtime_session.tensors.layers[target_layer];
        let attn_input = runtime_session
            .rms_norm_weighted_f32(
                &hidden,
                &layer.attn_norm,
                runtime_session.weights.snapshot.config.text_config.rms_norm_eps,
            )
            .unwrap();
        let attn_out = match decode_state.layers.get_mut(target_layer).unwrap() {
            MlxQwen35MoeLayerDecodeState::Attention(state) => runtime_session
                .apply_attention_layer_decode_reference_f32(layer, &attn_input, position, state)
                .unwrap(),
            MlxQwen35MoeLayerDecodeState::Recurrent(state) => runtime_session
                .apply_recurrent_layer_decode_reference_f32(layer, &attn_input, state)
                .unwrap(),
        };
        add_residual_in_place(&mut hidden, &attn_out).unwrap();
        let ffn_input = runtime_session
            .rms_norm_weighted_f32(
                &hidden,
                &layer.post_attention_norm,
                runtime_session.weights.snapshot.config.text_config.rms_norm_eps,
            )
            .unwrap();
        let reference_ffn = runtime_session
            .apply_moe_ffn_reference_f32(target_layer as u32, &ffn_input)
            .unwrap();
        let exact_ffn = cuda_runtime
            .debug_eval_moe_from_ffn_input(target_layer, &ffn_input)
            .unwrap();

        eprintln!(
            "layer={target_layer} position={position} router_max_abs_diff={} output_max_abs_diff={} routed_output_max_abs_diff={} shared_output_max_abs_diff={} shared_gate_ref={} shared_gate_cuda={} ref_routes={:?} cuda_routes={:?}",
            max_abs_diff(&exact_ffn.router_logits, &reference_ffn.router_logits),
            max_abs_diff(&exact_ffn.output, &reference_ffn.output),
            max_abs_diff(&exact_ffn.routed_output, &reference_ffn.routed_output),
            max_abs_diff(&exact_ffn.shared_output, &reference_ffn.shared_output),
            reference_ffn.shared_gate,
            exact_ffn.shared_gate,
            reference_ffn
                .routed_experts
                .iter()
                .map(|route| (route.expert_index, route.weight))
                .collect::<Vec<_>>(),
            exact_ffn
                .routed_experts
                .iter()
                .map(|route| (route.expert_index, route.weight))
                .collect::<Vec<_>>(),
        );
        panic!("debug only");
    }
}
