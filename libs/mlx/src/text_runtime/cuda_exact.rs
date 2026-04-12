use super::{
    bf16_round_to_f32, bf16_words_as_bytes, extract_gemma4_assistant_response_text,
    load_optional_scalar_f32, GemmaStopReason, GemmaTextBenchmarkOutput,
    GemmaTextGenerationOptions, GemmaTextRuntimeSession, GemmaTextSamplingOptions,
    TextLayerTensorNames, MlxIndexedSafetensors,
};
use crate::GemmaAttentionKind;
use makepad_ggml::backend::cuda::{CudaBuffer, CudaGraphExec, CudaRuntime};
use std::cmp::max;
use std::error::Error;
use std::mem::size_of;
use std::sync::Arc;
use std::time::{Duration, Instant};

const QK_Q8_1: usize = 32;
const QK_NVFP4: usize = 64;
const Q8_1_BLOCK_BYTES: usize = 36;
const CUDA_FINAL_TEXT_NORM_WEIGHT_NAME: &str = "language_model.model.norm.weight";
const CUDA_DISALLOWED_TOKEN_IDS_CAPACITY: usize = 64;
const CUDA_PREFILL_CHUNK_TOKENS: usize = 128;
const CUDA_SESSION_MIN_CAPACITY: usize = 1024;

pub(super) struct CudaNvfp4TextRuntime {
    session: Option<CudaNvfp4BenchmarkSession>,
}

impl CudaNvfp4TextRuntime {
    pub(super) fn new() -> Self {
        Self { session: None }
    }

    fn session_mut<'a>(
        &'a mut self,
        runtime: &Arc<GemmaTextRuntimeSession>,
        max_total_tokens: usize,
    ) -> Result<&'a mut CudaNvfp4BenchmarkSession, String> {
        let target_capacity = grow_cuda_session_capacity(runtime, max_total_tokens)?;
        let needs_rebuild = self
            .session
            .as_ref()
            .is_none_or(|session| session.max_total_tokens < target_capacity);
        if needs_rebuild {
            drop(self.session.take());
            self.session = Some(
                CudaNvfp4BenchmarkSession::load(runtime.clone(), target_capacity)
                    .map_err(|err| err.to_string())?,
            );
        }
        self.session
            .as_mut()
            .ok_or_else(|| "CUDA exact session did not initialize".to_string())
    }
}

fn cuda_exact_max_supported_tokens(runtime: &GemmaTextRuntimeSession) -> usize {
    let text = &runtime.weights.snapshot.config.text_config;
    if text
        .layer_types
        .iter()
        .any(|layer_type| layer_type == "sliding_attention")
    {
        text.sliding_window as usize
    } else {
        usize::MAX
    }
}

fn grow_cuda_session_capacity(
    runtime: &GemmaTextRuntimeSession,
    required_tokens: usize,
) -> Result<usize, String> {
    let max_supported = cuda_exact_max_supported_tokens(runtime);
    if required_tokens > max_supported {
        return Err(format!(
            "CUDA exact path supports up to {max_supported} total tokens for this model, requested {required_tokens}"
        ));
    }
    let min_capacity = required_tokens.max(CUDA_SESSION_MIN_CAPACITY);
    Ok(min_capacity
        .min(max_supported)
        .checked_next_power_of_two()
        .unwrap_or(min_capacity.min(max_supported)))
}

pub(super) fn supports_cuda_exact_greedy_generation(
    runtime: &Arc<GemmaTextRuntimeSession>,
    max_new_tokens: Option<usize>,
    sampling_options: &GemmaTextSamplingOptions,
) -> bool {
    runtime.has_cuda_exact_backend()
        && max_new_tokens.is_some_and(|limit| limit > 0)
        && (!sampling_options.do_sample || sampling_options.temperature <= 0.0)
        && supports_cuda_exact_greedy_model(runtime)
}

pub(super) fn supports_cuda_exact_greedy_model(runtime: &GemmaTextRuntimeSession) -> bool {
    supports_cuda_exact_greedy_weights(&runtime.weights)
}

pub(super) fn supports_cuda_exact_greedy_weights(weights: &MlxIndexedSafetensors) -> bool {
    let text = &weights.snapshot.config.text_config;
    weights.quantization_mode() == "nvfp4"
        && !text.enable_moe_block
        && text.hidden_size_per_layer_input == 0
        && text.num_kv_shared_layers == 0
}

pub(super) fn try_generate_cuda_nvfp4_greedy<F>(
    runtime: &Arc<GemmaTextRuntimeSession>,
    prompt_token_ids: Arc<[u32]>,
    max_new_tokens: Option<usize>,
    sampling_options: &GemmaTextSamplingOptions,
    mut on_generated_ids: F,
) -> Result<Option<(Arc<[u32]>, GemmaStopReason)>, String>
where
    F: FnMut(&[u32]) -> Result<(), String>,
{
    if !supports_cuda_exact_greedy_generation(runtime, max_new_tokens, sampling_options) {
        return Ok(None);
    }
    if prompt_token_ids.is_empty() {
        return Err("generation requires at least one prompt token".to_string());
    }

    let max_new_tokens = max_new_tokens.expect("checked by supports_cuda_exact_greedy_generation");
    let max_total_tokens = prompt_token_ids
        .len()
        .checked_add(max_new_tokens)
        .ok_or_else(|| "CUDA generation token budget overflow".to_string())?;
    let cuda_exact_backend = runtime.cuda_exact_backend()?;
    let mut backend = cuda_exact_backend
        .lock()
        .map_err(|_| "CUDA exact backend mutex poisoned".to_string())?;
    let session = backend.session_mut(runtime, max_total_tokens)?;
    let metrics = session
        .generate_greedy_with_callback(prompt_token_ids.as_ref(), max_new_tokens, |generated| {
            on_generated_ids(generated)
        })
        .map_err(|err| err.to_string())?;
    Ok(Some((
        Arc::<[u32]>::from(metrics.generated_token_ids),
        metrics.stop_reason,
    )))
}

pub(super) fn try_benchmark_cuda_nvfp4_greedy(
    runtime: &Arc<GemmaTextRuntimeSession>,
    prompt_text: Arc<str>,
    formatted_prompt_text: Arc<str>,
    prompt_token_ids: Arc<[u32]>,
    options: &GemmaTextGenerationOptions,
    warmup_iters: usize,
    measured_iters: usize,
    load_started: Instant,
) -> Result<Option<GemmaTextBenchmarkOutput>, Box<dyn Error>> {
    if !runtime.has_cuda_exact_backend() || !supports_cuda_exact_greedy_model(runtime) {
        return Ok(None);
    }

    let max_total_tokens = prompt_token_ids
        .len()
        .checked_add(options.max_new_tokens)
        .ok_or("CUDA benchmark token budget overflow")?;
    let cuda_exact_backend = runtime.cuda_exact_backend().map_err(|err| err.to_string())?;
    let mut backend = cuda_exact_backend
        .lock()
        .map_err(|_| "CUDA exact backend mutex poisoned".to_string())?;
    let session = backend
        .session_mut(runtime, max_total_tokens)
        .map_err(|err| err.to_string())?;
    let load_duration = load_started.elapsed();

    for _ in 0..warmup_iters {
        let _ = session.generate_greedy(prompt_token_ids.as_ref(), options.max_new_tokens)?;
    }

    let started = Instant::now();
    let mut total_generated_tokens = 0usize;
    let mut time_to_first_token_elapsed = Duration::ZERO;
    let mut steady_state_elapsed = Duration::ZERO;
    let mut steady_state_generated_tokens = 0usize;
    let mut last_generated_token_ids = Arc::<[u32]>::from(Vec::<u32>::new());
    for _ in 0..measured_iters {
        let metrics = session.generate_greedy(prompt_token_ids.as_ref(), options.max_new_tokens)?;
        total_generated_tokens += metrics.generated_token_ids.len();
        time_to_first_token_elapsed += metrics.time_to_first_token_elapsed;
        steady_state_elapsed += metrics.steady_state_elapsed;
        steady_state_generated_tokens += metrics
            .generated_token_ids
            .len()
            .saturating_sub(usize::from(!metrics.generated_token_ids.is_empty()));
        last_generated_token_ids = Arc::<[u32]>::from(metrics.generated_token_ids);
    }
    let elapsed = started.elapsed();
    let last_generated_text = if last_generated_token_ids.is_empty() {
        Arc::<str>::from("")
    } else {
        let raw_text = runtime
            .tokenizer
            .decode(last_generated_token_ids.as_ref())
            .map_err(|err| err.to_string())?;
        Arc::<str>::from(extract_gemma4_assistant_response_text(
            &runtime.weights.snapshot.tokenizer_config,
            &raw_text,
        ))
    };
    let elapsed_secs = elapsed.as_secs_f64();
    let decode_tokens_per_second = if elapsed_secs > 0.0 {
        total_generated_tokens as f64 / elapsed_secs
    } else {
        0.0
    };
    let total_prompt_tokens = measured_iters
        .checked_mul(prompt_token_ids.len())
        .ok_or("benchmark prompt-token count overflow")?;
    let total_tokens_processed = total_prompt_tokens
        .checked_add(total_generated_tokens)
        .ok_or("benchmark total token count overflow")?;
    let total_tokens_per_second = if elapsed_secs > 0.0 {
        total_tokens_processed as f64 / elapsed_secs
    } else {
        0.0
    };
    let ttft_elapsed_secs = time_to_first_token_elapsed.as_secs_f64();
    let prompt_prefill_tokens_per_second = if ttft_elapsed_secs > 0.0 {
        total_prompt_tokens as f64 / ttft_elapsed_secs
    } else {
        0.0
    };
    let steady_state_elapsed_secs = steady_state_elapsed.as_secs_f64();
    let steady_state_decode_tokens_per_second = if steady_state_elapsed_secs > 0.0 {
        steady_state_generated_tokens as f64 / steady_state_elapsed_secs
    } else {
        0.0
    };

    Ok(Some(GemmaTextBenchmarkOutput {
        model_path: runtime.model_path.clone(),
        prompt_text,
        formatted_prompt_text,
        prompt_token_ids,
        max_new_tokens: options.max_new_tokens,
        warmup_iters,
        measured_iters,
        load_duration,
        elapsed,
        total_generated_tokens,
        time_to_first_token_elapsed,
        steady_state_elapsed,
        steady_state_generated_tokens,
        last_generated_token_ids,
        last_generated_text,
        metal_counters: Default::default(),
        prompt_prefill_tokens_per_second,
        steady_state_decode_tokens_per_second,
        decode_tokens_per_second,
        total_tokens_per_second,
    }))
}

struct GenerationMetrics {
    generated_token_ids: Vec<u32>,
    stop_reason: GemmaStopReason,
    time_to_first_token_elapsed: Duration,
    steady_state_elapsed: Duration,
}

struct CudaNvfp4KvCache {
    key: CudaBuffer,
    value: CudaBuffer,
    head_dim: usize,
    max_tokens: usize,
    stored_tokens: usize,
}

impl CudaNvfp4KvCache {
    fn new(cuda: &CudaRuntime, kv_head_count: usize, head_dim: usize, max_tokens: usize) -> Result<Self, String> {
        let row_stride = max_tokens
            .checked_mul(head_dim)
            .ok_or_else(|| "CUDA KV row stride overflow".to_string())?;
        let storage = kv_head_count
            .checked_mul(row_stride)
            .ok_or_else(|| "CUDA KV storage overflow".to_string())?;
        Ok(Self {
            key: cuda.alloc_f32(storage)?,
            value: cuda.alloc_f32(storage)?,
            head_dim,
            max_tokens,
            stored_tokens: 0,
        })
    }

    fn row_stride(&self) -> usize {
        self.max_tokens * self.head_dim
    }

    fn reset(&mut self) {
        self.stored_tokens = 0;
    }
}

struct CudaNvfp4Layer {
    head_dim: usize,
    q_head_count: usize,
    k_head_count: usize,
    q_heads_per_kv: usize,
    rotary_dim: usize,
    rope_base: f32,
    hidden_size: usize,
    intermediate_size: usize,
    q_out_len: usize,
    v_offset: usize,
    qkv_out_len: usize,
    layer_scalar: Option<f32>,
    input_norm_weight: CudaBuffer,
    q_norm_weight: CudaBuffer,
    k_norm_weight: CudaBuffer,
    post_attention_norm_weight: CudaBuffer,
    pre_feedforward_norm_weight: CudaBuffer,
    post_feedforward_norm_weight: CudaBuffer,
    qkv_weight: CudaBuffer,
    o_weight: CudaBuffer,
    mlp_gate_up_weight: CudaBuffer,
    mlp_down_weight: CudaBuffer,
    input_norm_out: CudaBuffer,
    qkv_out: CudaBuffer,
    q_rope: CudaBuffer,
    attn_out: CudaBuffer,
    o_proj_out: CudaBuffer,
    post_attention_norm_out: CudaBuffer,
    residual_out: CudaBuffer,
    pre_feedforward_norm_out: CudaBuffer,
    mlp_gate_up_out: CudaBuffer,
    geglu_out: CudaBuffer,
    mlp_down_out: CudaBuffer,
    post_feedforward_norm_out: CudaBuffer,
    kv_cache: CudaNvfp4KvCache,
}

struct CudaNvfp4TextIo {
    embed_weight: CudaBuffer,
    final_norm_weight: CudaBuffer,
    hidden_a: CudaBuffer,
    hidden_b: CudaBuffer,
    q8_scratch: CudaBuffer,
    final_norm_out: CudaBuffer,
    logits_out: CudaBuffer,
    argmax_out: CudaBuffer,
    disallowed_token_ids: CudaBuffer,
    hidden_size: usize,
    vocab_size: usize,
    embed_scale: f32,
    disallowed_token_capacity: usize,
}

struct CudaNvfp4PrefillBuffers {
    chunk_tokens: usize,
    hidden_a: CudaBuffer,
    hidden_b: CudaBuffer,
    input_norm_out: CudaBuffer,
    q8_scratch: CudaBuffer,
    qkv_out: CudaBuffer,
    q_rope: CudaBuffer,
    attn_out: CudaBuffer,
    o_proj_out: CudaBuffer,
    post_attention_norm_out: CudaBuffer,
    residual_out: CudaBuffer,
    pre_feedforward_norm_out: CudaBuffer,
    mlp_gate_up_out: CudaBuffer,
    geglu_out: CudaBuffer,
    mlp_down_out: CudaBuffer,
    post_feedforward_norm_out: CudaBuffer,
}

struct CudaNvfp4GraphTokenState {
    token_id: CudaBuffer,
    position: CudaBuffer,
    seq_len: CudaBuffer,
}

struct CudaNvfp4DecodeGraph {
    exec: CudaGraphExec,
    token_state: CudaNvfp4GraphTokenState,
    disallowed_count: CudaBuffer,
}

struct CudaNvfp4BenchmarkSession {
    runtime_session: Arc<GemmaTextRuntimeSession>,
    cuda: CudaRuntime,
    io: CudaNvfp4TextIo,
    prefill: CudaNvfp4PrefillBuffers,
    layers: Vec<CudaNvfp4Layer>,
    rms_norm_eps: f32,
    max_total_tokens: usize,
    decode_graph: Option<CudaNvfp4DecodeGraph>,
}

impl CudaNvfp4BenchmarkSession {
    fn load(runtime_session: Arc<GemmaTextRuntimeSession>, max_total_tokens: usize) -> Result<Self, Box<dyn Error>> {
        if max_total_tokens == 0 {
            return Err("CUDA benchmark requires at least one token".into());
        }
        let text = &runtime_session.weights.snapshot.config.text_config;
        let rms_norm_eps = text.rms_norm_eps;
        let hidden_size = text.hidden_size as usize;
        let intermediate_size = text.intermediate_size as usize;
        let vocab_size = text.vocab_size as usize;
        let cuda = CudaRuntime::load()?;

        let embed_weight = cuda.load_bytes(
                &runtime_session
                    .weights
                    .repack_nvfp4_tensor_to_ggml_bytes(
                        super::EMBED_TOKENS_WEIGHT_NAME,
                        super::EMBED_TOKENS_SCALES_NAME,
                )
                .map_err(|err| err.to_string())?,
        )?;
        let final_norm_weight = cuda.load_bytes(
            bf16_words_as_bytes(
                &runtime_session
                    .weights
                    .read_bf16_tensor_words(CUDA_FINAL_TEXT_NORM_WEIGHT_NAME)
                    .map_err(|err| err.to_string())?,
            ),
        )?;
        let q8_scratch_len = max(
            intermediate_size,
            max(hidden_size, (text.num_attention_heads as usize) * (text.global_head_dim as usize)),
        );
        let q8_scratch_bytes = q8_scratch_len
            .checked_div(QK_Q8_1)
            .ok_or("CUDA q8 scratch block count underflow")?
            .checked_mul(Q8_1_BLOCK_BYTES)
            .ok_or("CUDA q8 scratch byte size overflow")?;
        let io = CudaNvfp4TextIo {
            embed_weight,
            final_norm_weight,
            hidden_a: cuda.alloc_f32(hidden_size)?,
            hidden_b: cuda.alloc_f32(hidden_size)?,
            q8_scratch: cuda.alloc_bytes(q8_scratch_bytes)?,
            final_norm_out: cuda.alloc_f32(hidden_size)?,
            logits_out: cuda.alloc_f32(vocab_size)?,
            argmax_out: cuda.alloc_u32(1)?,
            disallowed_token_ids: cuda.alloc_u32(CUDA_DISALLOWED_TOKEN_IDS_CAPACITY)?,
            hidden_size,
            vocab_size,
            embed_scale: bf16_round_to_f32((hidden_size as f32).sqrt()),
            disallowed_token_capacity: CUDA_DISALLOWED_TOKEN_IDS_CAPACITY,
        };

        let mut max_q_out_len = 0usize;
        let mut max_qkv_out_len = 0usize;
        let mut layers = Vec::with_capacity(text.num_hidden_layers as usize);
        for layer_idx in 0..text.num_hidden_layers as usize {
            let layer_type = text
                .layer_types
                .get(layer_idx)
                .ok_or_else(|| format!("missing layer type for layer {layer_idx}"))?;
            let attention = match layer_type.as_str() {
                "full_attention" => GemmaAttentionKind::Full,
                "sliding_attention" => GemmaAttentionKind::Sliding,
                other => return Err(format!("unsupported attention kind {other}").into()),
            };
            if attention == GemmaAttentionKind::Sliding && max_total_tokens > text.sliding_window as usize {
                return Err(format!(
                    "CUDA benchmark token budget {} exceeds sliding window {}",
                    max_total_tokens, text.sliding_window
                )
                .into());
            }
            let attention_k_eq_v = text.attention_k_eq_v && attention == GemmaAttentionKind::Full;
            let names = TextLayerTensorNames::for_layer(layer_idx, attention_k_eq_v);
            let head_dim = if attention == GemmaAttentionKind::Full {
                text.global_head_dim as usize
            } else {
                text.head_dim as usize
            };
            let q_head_count = text.num_attention_heads as usize;
            let k_head_count = if attention_k_eq_v && attention == GemmaAttentionKind::Full {
                text.num_global_key_value_heads_or_default() as usize
            } else {
                text.num_key_value_heads as usize
            };
            let q_heads_per_kv = q_head_count / k_head_count;
            let q_out_len = q_head_count * head_dim;
            let k_out_len = k_head_count * head_dim;
            let qkv_pairs = if attention_k_eq_v {
                vec![
                    (&names.q.weight_name as &str, &names.q.scales_name as &str),
                    (&names.k.weight_name as &str, &names.k.scales_name as &str),
                ]
            } else {
                vec![
                    (&names.q.weight_name as &str, &names.q.scales_name as &str),
                    (&names.k.weight_name as &str, &names.k.scales_name as &str),
                    (&names.v.weight_name as &str, &names.v.scales_name as &str),
                ]
            };
            let qkv_weight = cuda.load_bytes(
                &runtime_session
                    .weights
                    .repack_nvfp4_tensors_to_ggml_bytes(&qkv_pairs)
                    .map_err(|err| err.to_string())?,
            )?;
            let o_weight = cuda.load_bytes(
                &runtime_session
                    .weights
                    .repack_nvfp4_tensor_to_ggml_bytes(&names.o.weight_name, &names.o.scales_name)
                    .map_err(|err| err.to_string())?,
            )?;
            let mlp_gate_up_weight = cuda.load_bytes(
                &runtime_session
                    .weights
                    .repack_nvfp4_tensors_to_ggml_bytes(&[
                        (&names.mlp_gate_weight_name, &names.mlp_gate_scales_name),
                        (&names.mlp_up_weight_name, &names.mlp_up_scales_name),
                    ])
                    .map_err(|err| err.to_string())?,
            )?;
            let mlp_down_weight = cuda.load_bytes(
                &runtime_session
                    .weights
                    .repack_nvfp4_tensor_to_ggml_bytes(
                        &names.mlp_down_weight_name,
                        &names.mlp_down_scales_name,
                    )
                    .map_err(|err| err.to_string())?,
            )?;
            let rope_params = if attention == GemmaAttentionKind::Full {
                &text.rope_parameters.full_attention
            } else {
                &text.rope_parameters.sliding_attention
            };
            let rotary_dim = if let Some(partial_factor) = rope_params.partial_rotary_factor {
                (head_dim as f32 * partial_factor).round() as usize
            } else {
                head_dim
            };
            let kv_cache = CudaNvfp4KvCache::new(&cuda, k_head_count, head_dim, max_total_tokens)?;
            let v_offset = if attention_k_eq_v {
                q_out_len
            } else {
                q_out_len + k_out_len
            };
            let qkv_out_len = q_out_len + k_out_len + if attention_k_eq_v { 0 } else { k_out_len };
            max_q_out_len = max(max_q_out_len, q_out_len);
            max_qkv_out_len = max(max_qkv_out_len, qkv_out_len);
            layers.push(CudaNvfp4Layer {
                head_dim,
                q_head_count,
                k_head_count,
                q_heads_per_kv,
                rotary_dim,
                rope_base: rope_params.rope_theta,
                hidden_size,
                intermediate_size,
                q_out_len,
                v_offset,
                qkv_out_len,
                layer_scalar: load_optional_scalar_f32(&runtime_session.weights, &names.layer_scalar_name)?,
                input_norm_weight: cuda.load_bytes(bf16_words_as_bytes(
                    &runtime_session
                        .weights
                        .read_bf16_tensor_words(&names.input_norm_weight_name)
                        .map_err(|err| err.to_string())?,
                ))?,
                q_norm_weight: cuda.load_bytes(bf16_words_as_bytes(
                    &runtime_session
                        .weights
                        .read_bf16_tensor_words(
                            names.q
                                .norm_weight_name
                                .as_deref()
                                .ok_or_else(|| format!("missing q norm weight for layer {layer_idx}"))?,
                        )
                        .map_err(|err| err.to_string())?,
                ))?,
                k_norm_weight: cuda.load_bytes(bf16_words_as_bytes(
                    &runtime_session
                        .weights
                        .read_bf16_tensor_words(
                            names.k
                                .norm_weight_name
                                .as_deref()
                                .ok_or_else(|| format!("missing k norm weight for layer {layer_idx}"))?,
                        )
                        .map_err(|err| err.to_string())?,
                ))?,
                post_attention_norm_weight: cuda.load_bytes(bf16_words_as_bytes(
                    &runtime_session
                        .weights
                        .read_bf16_tensor_words(&names.post_attention_norm_weight_name)
                        .map_err(|err| err.to_string())?,
                ))?,
                pre_feedforward_norm_weight: cuda.load_bytes(bf16_words_as_bytes(
                    &runtime_session
                        .weights
                        .read_bf16_tensor_words(&names.pre_feedforward_norm_weight_name)
                        .map_err(|err| err.to_string())?,
                ))?,
                post_feedforward_norm_weight: cuda.load_bytes(bf16_words_as_bytes(
                    &runtime_session
                        .weights
                        .read_bf16_tensor_words(&names.post_feedforward_norm_weight_name)
                        .map_err(|err| err.to_string())?,
                ))?,
                qkv_weight,
                o_weight,
                mlp_gate_up_weight,
                mlp_down_weight,
                input_norm_out: cuda.alloc_f32(hidden_size)?,
                qkv_out: cuda.alloc_f32(qkv_out_len)?,
                q_rope: cuda.alloc_f32(q_out_len)?,
                attn_out: cuda.alloc_f32(q_out_len)?,
                o_proj_out: cuda.alloc_f32(hidden_size)?,
                post_attention_norm_out: cuda.alloc_f32(hidden_size)?,
                residual_out: cuda.alloc_f32(hidden_size)?,
                pre_feedforward_norm_out: cuda.alloc_f32(hidden_size)?,
                mlp_gate_up_out: cuda.alloc_f32(intermediate_size * 2)?,
                geglu_out: cuda.alloc_f32(intermediate_size)?,
                mlp_down_out: cuda.alloc_f32(hidden_size)?,
                post_feedforward_norm_out: cuda.alloc_f32(hidden_size)?,
                kv_cache,
            });
        }

        let prefill_chunk_tokens = max_total_tokens.min(CUDA_PREFILL_CHUNK_TOKENS);
        let prefill_q8_scratch_len = prefill_chunk_tokens
            .checked_mul(max(intermediate_size, max(hidden_size, max_q_out_len)))
            .ok_or("CUDA prefill q8 scratch size overflow")?;
        let prefill_q8_scratch_bytes = prefill_q8_scratch_len
            .checked_div(QK_Q8_1)
            .ok_or("CUDA prefill q8 scratch block count underflow")?
            .checked_mul(Q8_1_BLOCK_BYTES)
            .ok_or("CUDA prefill q8 scratch byte size overflow")?;
        let prefill = CudaNvfp4PrefillBuffers {
            chunk_tokens: prefill_chunk_tokens,
            hidden_a: cuda.alloc_f32(prefill_chunk_tokens * hidden_size)?,
            hidden_b: cuda.alloc_f32(prefill_chunk_tokens * hidden_size)?,
            input_norm_out: cuda.alloc_f32(prefill_chunk_tokens * hidden_size)?,
            q8_scratch: cuda.alloc_bytes(prefill_q8_scratch_bytes)?,
            qkv_out: cuda.alloc_f32(prefill_chunk_tokens * max_qkv_out_len)?,
            q_rope: cuda.alloc_f32(prefill_chunk_tokens * max_q_out_len)?,
            attn_out: cuda.alloc_f32(prefill_chunk_tokens * max_q_out_len)?,
            o_proj_out: cuda.alloc_f32(prefill_chunk_tokens * hidden_size)?,
            post_attention_norm_out: cuda.alloc_f32(prefill_chunk_tokens * hidden_size)?,
            residual_out: cuda.alloc_f32(prefill_chunk_tokens * hidden_size)?,
            pre_feedforward_norm_out: cuda.alloc_f32(prefill_chunk_tokens * hidden_size)?,
            mlp_gate_up_out: cuda.alloc_f32(prefill_chunk_tokens * intermediate_size * 2)?,
            geglu_out: cuda.alloc_f32(prefill_chunk_tokens * intermediate_size)?,
            mlp_down_out: cuda.alloc_f32(prefill_chunk_tokens * hidden_size)?,
            post_feedforward_norm_out: cuda.alloc_f32(prefill_chunk_tokens * hidden_size)?,
        };

        let mut session = Self {
            runtime_session,
            cuda,
            io,
            prefill,
            layers,
            rms_norm_eps,
            max_total_tokens,
            decode_graph: None,
        };
        session.decode_graph = Some(session.capture_decode_graph()?);
        session.eval_next_token_graph(2, 0, &[])?;
        session.reset();
        Ok(session)
    }

    fn reset(&mut self) {
        for layer in &mut self.layers {
            layer.kv_cache.reset();
        }
    }

    fn alloc_graph_token_state(&self) -> Result<CudaNvfp4GraphTokenState, Box<dyn Error>> {
        let token_id = self.cuda.alloc_u32(1)?;
        let position = self.cuda.alloc_u32(1)?;
        let seq_len = self.cuda.alloc_u32(1)?;
        self.cuda.write_u32(&token_id, 0)?;
        self.cuda.write_u32(&position, 0)?;
        self.cuda.write_u32(&seq_len, 1)?;
        Ok(CudaNvfp4GraphTokenState {
            token_id,
            position,
            seq_len,
        })
    }

    fn write_graph_token_state(
        &self,
        token_state: &CudaNvfp4GraphTokenState,
        token_id: u32,
        position: usize,
    ) -> Result<(), Box<dyn Error>> {
        if token_id as usize >= self.io.vocab_size {
            return Err(format!("token id {} exceeds vocab {}", token_id, self.io.vocab_size).into());
        }
        if position >= self.max_total_tokens {
            return Err(format!(
                "token position {} exceeds CUDA session capacity {}",
                position, self.max_total_tokens
            )
            .into());
        }
        self.cuda.write_u32(&token_state.token_id, token_id)?;
        self.cuda
            .write_u32(&token_state.position, position as u32)?;
        self.cuda.write_u32(
            &token_state.seq_len,
            position
                .checked_add(1)
                .ok_or("token sequence length overflow")? as u32,
        )?;
        Ok(())
    }

    fn capture_decode_graph(&mut self) -> Result<CudaNvfp4DecodeGraph, Box<dyn Error>> {
        let token_state = self.alloc_graph_token_state()?;
        let disallowed_count = self.cuda.alloc_u32(1)?;
        self.cuda.write_u32(&disallowed_count, 0)?;
        self.reset();
        self.cuda.begin_capture()?;
        let hidden_is_a = self.eval_token_hidden_from_token_id_graph(
            &token_state.token_id,
            &token_state.position,
            &token_state.seq_len,
        )?;
        self.greedy_token_from_hidden_with_disallowed_graph(hidden_is_a, &disallowed_count)?;
        let exec = self.cuda.end_capture()?.instantiate().map_err(|err| err.to_string())?;
        self.reset();
        Ok(CudaNvfp4DecodeGraph {
            exec,
            token_state,
            disallowed_count,
        })
    }

    fn increment_kv_caches(&mut self) {
        for layer in &mut self.layers {
            layer.kv_cache.stored_tokens += 1;
        }
    }

    fn eval_next_token_graph(
        &mut self,
        token_id: u32,
        position: usize,
        disallowed_token_ids: &[u32],
    ) -> Result<(), Box<dyn Error>> {
        if disallowed_token_ids.len() > self.io.disallowed_token_capacity {
            return Err(format!(
                "CUDA disallowed token set {} exceeds capacity {}",
                disallowed_token_ids.len(),
                self.io.disallowed_token_capacity
            )
            .into());
        }

        let decode_graph = self
            .decode_graph
            .as_ref()
            .ok_or("CUDA decode graph did not initialize")?;
        self.write_graph_token_state(&decode_graph.token_state, token_id, position)?;
        if !disallowed_token_ids.is_empty() {
            let disallowed_bytes = unsafe {
                std::slice::from_raw_parts(
                    disallowed_token_ids.as_ptr().cast::<u8>(),
                    disallowed_token_ids.len() * size_of::<u32>(),
                )
            };
            self.cuda
                .write_bytes(&self.io.disallowed_token_ids, disallowed_bytes)?;
        }
        self.cuda
            .write_u32(&decode_graph.disallowed_count, disallowed_token_ids.len() as u32)?;
        self.cuda.launch_graph(&decode_graph.exec)?;
        self.increment_kv_caches();
        Ok(())
    }

    fn generate_next_token_graph(
        &mut self,
        token_id: u32,
        position: usize,
        disallowed_token_ids: &[u32],
    ) -> Result<u32, Box<dyn Error>> {
        self.eval_next_token_graph(token_id, position, disallowed_token_ids)?;
        let next_token = self.cuda.read_u32(&self.io.argmax_out)?;
        if next_token == u32::MAX {
            return Err("no selectable token remained after suppression".into());
        }
        Ok(next_token)
    }

    fn load_prefill_embeddings(&self, token_ids: &[u32]) -> Result<(), Box<dyn Error>> {
        for (row_idx, &token_id) in token_ids.iter().enumerate() {
            if token_id as usize >= self.io.vocab_size {
                return Err(format!("token id {} exceeds vocab {}", token_id, self.io.vocab_size).into());
            }
            self.cuda.nvfp4_get_row_f32_offset(
                &self.io.embed_weight,
                &self.prefill.hidden_a,
                row_idx * self.io.hidden_size,
                self.io.hidden_size,
                token_id as usize,
            )?;
        }
        self.cuda.scale_f32_inplace(
            &self.prefill.hidden_a,
            self.io.embed_scale,
            token_ids
                .len()
                .checked_mul(self.io.hidden_size)
                .ok_or("CUDA prefill embedding scale length overflow")?,
        )?;
        Ok(())
    }

    fn prefill_prompt_hidden_batched(
        &mut self,
        prompt_token_ids: &[u32],
    ) -> Result<(bool, usize), Box<dyn Error>> {
        if prompt_token_ids.is_empty() {
            return Err("CUDA prefill requires at least one prompt token".into());
        }
        let chunk_capacity = self.prefill.chunk_tokens;
        if chunk_capacity == 0 {
            return Err("CUDA prefill chunk capacity is zero".into());
        }

        let mut chunk_start = 0usize;
        let mut final_hidden_is_a = true;
        let mut final_row_offset_elems = 0usize;
        while chunk_start < prompt_token_ids.len() {
            let chunk_len = (prompt_token_ids.len() - chunk_start).min(chunk_capacity);
            self.load_prefill_embeddings(&prompt_token_ids[chunk_start..chunk_start + chunk_len])?;

            let prefill = &self.prefill;
            let mut input_is_a = true;
            for layer in &mut self.layers {
                let (input_hidden, output_hidden) = if input_is_a {
                    (&prefill.hidden_a, &prefill.hidden_b)
                } else {
                    (&prefill.hidden_b, &prefill.hidden_a)
                };
                Self::eval_layer_prefill_chunk(
                    &self.cuda,
                    prefill,
                    layer,
                    input_hidden,
                    output_hidden,
                    chunk_start,
                    chunk_len,
                    self.rms_norm_eps,
                )?;
                input_is_a = !input_is_a;
            }
            final_hidden_is_a = input_is_a;
            final_row_offset_elems = chunk_len
                .checked_sub(1)
                .ok_or("CUDA prefill final row underflow")?
                .checked_mul(self.io.hidden_size)
                .ok_or("CUDA prefill final row offset overflow")?;
            chunk_start += chunk_len;
        }
        Ok((final_hidden_is_a, final_row_offset_elems))
    }

    fn greedy_token_from_prefill_hidden(
        &mut self,
        hidden_is_a: bool,
        hidden_offset_elems: usize,
    ) -> Result<u32, Box<dyn Error>> {
        self.greedy_token_from_prefill_hidden_with_disallowed(hidden_is_a, hidden_offset_elems, &[])
    }

    fn greedy_token_from_prefill_hidden_with_disallowed(
        &mut self,
        hidden_is_a: bool,
        hidden_offset_elems: usize,
        disallowed_token_ids: &[u32],
    ) -> Result<u32, Box<dyn Error>> {
        if disallowed_token_ids.len() > self.io.disallowed_token_capacity {
            return Err(format!(
                "CUDA disallowed token set {} exceeds capacity {}",
                disallowed_token_ids.len(),
                self.io.disallowed_token_capacity
            )
            .into());
        }
        let hidden = if hidden_is_a {
            &self.prefill.hidden_a
        } else {
            &self.prefill.hidden_b
        };
        self.cuda.rms_norm_row_weighted_f32_input_offset(
            hidden,
            hidden_offset_elems,
            &self.io.final_norm_weight,
            &self.io.final_norm_out,
            self.io.hidden_size,
            self.rms_norm_eps,
        )?;
        self.cuda.quantize_nvfp4_f32(
            &self.io.final_norm_out,
            1.0,
            &self.io.q8_scratch,
            self.io.hidden_size,
        )?;
        self.cuda.nvfp4_nvfp4_matvec(
            &self.io.q8_scratch,
            &self.io.embed_weight,
            1.0,
            &self.io.logits_out,
            self.io.hidden_size / QK_NVFP4,
            self.io.vocab_size,
        )?;
        if disallowed_token_ids.is_empty() {
            self.cuda
                .argmax_f32(&self.io.logits_out, &self.io.argmax_out, self.io.vocab_size)?;
        } else {
            let disallowed_bytes = unsafe {
                std::slice::from_raw_parts(
                    disallowed_token_ids.as_ptr().cast::<u8>(),
                    disallowed_token_ids.len() * size_of::<u32>(),
                )
            };
            self.cuda
                .write_bytes(&self.io.disallowed_token_ids, disallowed_bytes)?;
            self.cuda.masked_argmax_f32(
                &self.io.logits_out,
                &self.io.disallowed_token_ids,
                disallowed_token_ids.len(),
                &self.io.argmax_out,
                self.io.vocab_size,
            )?;
        }
        let token_id = self.cuda.read_u32(&self.io.argmax_out)?;
        if token_id == u32::MAX {
            return Err("no selectable token remained after suppression".into());
        }
        Ok(token_id)
    }

    fn generate_greedy(&mut self, prompt_token_ids: &[u32], max_new_tokens: usize) -> Result<GenerationMetrics, Box<dyn Error>> {
        self.generate_greedy_with_callback(prompt_token_ids, max_new_tokens, |_| Ok(()))
    }

    fn generate_greedy_with_callback<F>(
        &mut self,
        prompt_token_ids: &[u32],
        max_new_tokens: usize,
        mut on_generated_ids: F,
    ) -> Result<GenerationMetrics, Box<dyn Error>>
    where
        F: FnMut(&[u32]) -> Result<(), String>,
    {
        if prompt_token_ids.is_empty() {
            return Err("generation requires at least one prompt token".into());
        }
        if prompt_token_ids.len() + max_new_tokens > self.max_total_tokens {
            return Err("benchmark token budget exceeds CUDA session capacity".into());
        }
        self.reset();

        let ttft_started = Instant::now();
        let first_token_id = if prompt_token_ids.len() == 1 {
            self.generate_next_token_graph(prompt_token_ids[0], 0, &[])?
        } else {
            let (hidden_is_a, hidden_offset_elems) =
                self.prefill_prompt_hidden_batched(prompt_token_ids)?;
            self.greedy_token_from_prefill_hidden(hidden_is_a, hidden_offset_elems)?
        };
        let time_to_first_token_elapsed = ttft_started.elapsed();

        let mut generated = Vec::with_capacity(max_new_tokens);
        if self.runtime_session.stop_tokens.contains(&first_token_id) {
            return Ok(GenerationMetrics {
                generated_token_ids: generated,
                stop_reason: GemmaStopReason::EosToken(first_token_id),
                time_to_first_token_elapsed,
                steady_state_elapsed: Duration::ZERO,
            });
        }

        generated.push(first_token_id);
        on_generated_ids(&generated)
            .map_err(std::io::Error::other)?;

        let steady_started = Instant::now();
        let stop_reason = loop {
            if generated.len() >= max_new_tokens {
                break GemmaStopReason::MaxNewTokens;
            }
            let input_token = *generated
                .last()
                .ok_or("missing last generated token for CUDA decode")?;
            let position = prompt_token_ids.len() + generated.len() - 1;
            let next_token = self.generate_next_token_graph(input_token, position, &[])?;
            if self.runtime_session.stop_tokens.contains(&next_token) {
                break GemmaStopReason::EosToken(next_token);
            }
            generated.push(next_token);
            on_generated_ids(&generated)
                .map_err(std::io::Error::other)?;
        };

        Ok(GenerationMetrics {
            generated_token_ids: generated,
            stop_reason,
            time_to_first_token_elapsed,
            steady_state_elapsed: steady_started.elapsed(),
        })
    }

    fn eval_token_hidden_from_token_id_graph(
        &mut self,
        token_id: &CudaBuffer,
        position: &CudaBuffer,
        seq_len: &CudaBuffer,
    ) -> Result<bool, Box<dyn Error>> {
        self.cuda.nvfp4_get_row_f32_device_u32(
            &self.io.embed_weight,
            &self.io.hidden_a,
            self.io.hidden_size,
            token_id,
        )?;
        self.cuda
            .scale_f32_inplace(&self.io.hidden_a, self.io.embed_scale, self.io.hidden_size)?;

        let mut input_is_a = true;
        for layer in &mut self.layers {
            let (input, output) = if input_is_a {
                (&self.io.hidden_a, &self.io.hidden_b)
            } else {
                (&self.io.hidden_b, &self.io.hidden_a)
            };
            Self::eval_layer_graph(
                &self.cuda,
                layer,
                &self.io.q8_scratch,
                input,
                output,
                position,
                seq_len,
                self.rms_norm_eps,
            )?;
            input_is_a = !input_is_a;
        }
        Ok(input_is_a)
    }

    fn eval_layer_graph(
        cuda: &CudaRuntime,
        layer: &mut CudaNvfp4Layer,
        q8_scratch: &CudaBuffer,
        input_hidden: &CudaBuffer,
        output_hidden: &CudaBuffer,
        position: &CudaBuffer,
        seq_len: &CudaBuffer,
        eps: f32,
    ) -> Result<(), Box<dyn Error>> {
        cuda.rms_norm_row_weighted_f32(
            input_hidden,
            &layer.input_norm_weight,
            &layer.input_norm_out,
            layer.hidden_size,
            eps,
        )?;
        cuda.quantize_nvfp4_f32(&layer.input_norm_out, 1.0, q8_scratch, layer.hidden_size)?;
        cuda.nvfp4_nvfp4_matvec(
            q8_scratch,
            &layer.qkv_weight,
            1.0,
            &layer.qkv_out,
            layer.hidden_size / QK_NVFP4,
            layer.qkv_out_len,
        )?;
        cuda.qkv_norm_rope_cache_f32_device_u32(
            &layer.qkv_out,
            &layer.q_norm_weight,
            &layer.k_norm_weight,
            &layer.q_rope,
            &layer.kv_cache.key,
            &layer.kv_cache.value,
            layer.q_head_count,
            layer.k_head_count,
            layer.head_dim,
            0,
            layer.q_out_len,
            layer.v_offset,
            layer.rotary_dim,
            layer.rope_base,
            position,
            eps,
            layer.kv_cache.max_tokens,
        )?;
        cuda.attention_seq_softmax_weighted_sum_f32_device_u32(
            &layer.q_rope,
            &layer.kv_cache.key,
            &layer.kv_cache.value,
            &layer.attn_out,
            layer.q_head_count,
            layer.q_heads_per_kv,
            layer.head_dim,
            layer.kv_cache.row_stride(),
            seq_len,
            layer.kv_cache.max_tokens,
            layer.head_dim,
        )?;
        cuda.quantize_nvfp4_f32(&layer.attn_out, 1.0, q8_scratch, layer.q_out_len)?;
        cuda.nvfp4_nvfp4_matvec(
            q8_scratch,
            &layer.o_weight,
            1.0,
            &layer.o_proj_out,
            layer.q_out_len / QK_NVFP4,
            layer.hidden_size,
        )?;
        cuda.rms_norm_row_weighted_f32(
            &layer.o_proj_out,
            &layer.post_attention_norm_weight,
            &layer.post_attention_norm_out,
            layer.hidden_size,
            eps,
        )?;
        cuda.add_f32(
            input_hidden,
            &layer.post_attention_norm_out,
            &layer.residual_out,
            layer.hidden_size,
        )?;
        cuda.rms_norm_row_weighted_f32(
            &layer.residual_out,
            &layer.pre_feedforward_norm_weight,
            &layer.pre_feedforward_norm_out,
            layer.hidden_size,
            eps,
        )?;
        cuda.quantize_nvfp4_f32(
            &layer.pre_feedforward_norm_out,
            1.0,
            q8_scratch,
            layer.hidden_size,
        )?;
        cuda.nvfp4_nvfp4_matvec(
            q8_scratch,
            &layer.mlp_gate_up_weight,
            1.0,
            &layer.mlp_gate_up_out,
            layer.hidden_size / QK_NVFP4,
            layer.intermediate_size * 2,
        )?;
        cuda.geglu_split_f32(
            &layer.mlp_gate_up_out,
            &layer.geglu_out,
            layer.intermediate_size,
            layer.intermediate_size,
        )?;
        cuda.quantize_nvfp4_f32(&layer.geglu_out, 1.0, q8_scratch, layer.intermediate_size)?;
        cuda.nvfp4_nvfp4_matvec(
            q8_scratch,
            &layer.mlp_down_weight,
            1.0,
            &layer.mlp_down_out,
            layer.intermediate_size / QK_NVFP4,
            layer.hidden_size,
        )?;
        cuda.rms_norm_row_weighted_f32(
            &layer.mlp_down_out,
            &layer.post_feedforward_norm_weight,
            &layer.post_feedforward_norm_out,
            layer.hidden_size,
            eps,
        )?;
        cuda.add_f32(
            &layer.residual_out,
            &layer.post_feedforward_norm_out,
            output_hidden,
            layer.hidden_size,
        )?;
        if let Some(scale) = layer.layer_scalar {
            cuda.scale_f32_inplace(output_hidden, scale, layer.hidden_size)?;
        }
        Ok(())
    }

    fn eval_layer_prefill_chunk(
        cuda: &CudaRuntime,
        prefill: &CudaNvfp4PrefillBuffers,
        layer: &mut CudaNvfp4Layer,
        input_hidden: &CudaBuffer,
        output_hidden: &CudaBuffer,
        chunk_start_position: usize,
        chunk_len: usize,
        eps: f32,
    ) -> Result<(), Box<dyn Error>> {
        if chunk_len == 0 {
            return Ok(());
        }

        let hidden_elems = chunk_len
            .checked_mul(layer.hidden_size)
            .ok_or("CUDA prefill hidden length overflow")?;
        cuda.rms_norm_rows_weighted_f32(
            input_hidden,
            &layer.input_norm_weight,
            &prefill.input_norm_out,
            chunk_len,
            layer.hidden_size,
            layer.hidden_size,
            eps,
        )?;
        cuda.quantize_q8_1_mmq_f32(
            &prefill.input_norm_out,
            &prefill.q8_scratch,
            layer.hidden_size,
            chunk_len,
        )?;
        cuda.nvfp4_q8_1_mmq_matmul_batched(
            &prefill.q8_scratch,
            &layer.qkv_weight,
            &prefill.qkv_out,
            layer.hidden_size,
            layer.qkv_out_len,
            chunk_len,
        )?;
        let chunk_start_slot = layer.kv_cache.stored_tokens;
        cuda.qkv_norm_rope_cache_rows_f32(
            &prefill.qkv_out,
            &layer.q_norm_weight,
            &layer.k_norm_weight,
            &prefill.q_rope,
            &layer.kv_cache.key,
            &layer.kv_cache.value,
            layer.q_head_count,
            layer.k_head_count,
            layer.head_dim,
            layer.qkv_out_len,
            layer.q_out_len,
            0,
            layer.q_out_len,
            layer.v_offset,
            layer.rotary_dim,
            layer.rope_base,
            chunk_start_position,
            eps,
            layer.kv_cache.max_tokens,
            chunk_start_slot,
            chunk_len,
        )?;
        cuda.attention_seq_softmax_weighted_sum_rows_f32(
            &prefill.q_rope,
            &layer.kv_cache.key,
            &layer.kv_cache.value,
            &prefill.attn_out,
            chunk_len,
            layer.q_head_count,
            layer.q_heads_per_kv,
            layer.head_dim,
            layer.kv_cache.row_stride(),
            layer.q_out_len,
            layer.q_out_len,
            chunk_start_slot,
            layer.kv_cache.max_tokens,
        )?;
        layer.kv_cache.stored_tokens += chunk_len;

        cuda.quantize_q8_1_mmq_f32(
            &prefill.attn_out,
            &prefill.q8_scratch,
            layer.q_out_len,
            chunk_len,
        )?;
        cuda.nvfp4_q8_1_mmq_matmul_batched(
            &prefill.q8_scratch,
            &layer.o_weight,
            &prefill.o_proj_out,
            layer.q_out_len,
            layer.hidden_size,
            chunk_len,
        )?;
        cuda.rms_norm_rows_weighted_f32(
            &prefill.o_proj_out,
            &layer.post_attention_norm_weight,
            &prefill.post_attention_norm_out,
            chunk_len,
            layer.hidden_size,
            layer.hidden_size,
            eps,
        )?;
        cuda.add_f32(
            input_hidden,
            &prefill.post_attention_norm_out,
            &prefill.residual_out,
            hidden_elems,
        )?;
        cuda.rms_norm_rows_weighted_f32(
            &prefill.residual_out,
            &layer.pre_feedforward_norm_weight,
            &prefill.pre_feedforward_norm_out,
            chunk_len,
            layer.hidden_size,
            layer.hidden_size,
            eps,
        )?;
        cuda.quantize_q8_1_mmq_f32(
            &prefill.pre_feedforward_norm_out,
            &prefill.q8_scratch,
            layer.hidden_size,
            chunk_len,
        )?;
        cuda.nvfp4_q8_1_mmq_matmul_batched(
            &prefill.q8_scratch,
            &layer.mlp_gate_up_weight,
            &prefill.mlp_gate_up_out,
            layer.hidden_size,
            layer.intermediate_size * 2,
            chunk_len,
        )?;
        cuda.geglu_split_f32_rows(
            &prefill.mlp_gate_up_out,
            &prefill.geglu_out,
            chunk_len,
            layer.intermediate_size * 2,
            layer.intermediate_size,
            layer.intermediate_size,
        )?;
        cuda.quantize_q8_1_mmq_f32(
            &prefill.geglu_out,
            &prefill.q8_scratch,
            layer.intermediate_size,
            chunk_len,
        )?;
        cuda.nvfp4_q8_1_mmq_matmul_batched(
            &prefill.q8_scratch,
            &layer.mlp_down_weight,
            &prefill.mlp_down_out,
            layer.intermediate_size,
            layer.hidden_size,
            chunk_len,
        )?;
        cuda.rms_norm_rows_weighted_f32(
            &prefill.mlp_down_out,
            &layer.post_feedforward_norm_weight,
            &prefill.post_feedforward_norm_out,
            chunk_len,
            layer.hidden_size,
            layer.hidden_size,
            eps,
        )?;
        cuda.add_f32(
            &prefill.residual_out,
            &prefill.post_feedforward_norm_out,
            output_hidden,
            hidden_elems,
        )?;
        if let Some(scale) = layer.layer_scalar {
            cuda.scale_f32_inplace(output_hidden, scale, hidden_elems)?;
        }
        Ok(())
    }

    fn greedy_token_from_hidden_with_disallowed_graph(
        &mut self,
        hidden_is_a: bool,
        disallowed_count: &CudaBuffer,
    ) -> Result<(), Box<dyn Error>> {
        let hidden = if hidden_is_a {
            &self.io.hidden_a
        } else {
            &self.io.hidden_b
        };
        self.cuda.rms_norm_row_weighted_f32(
            hidden,
            &self.io.final_norm_weight,
            &self.io.final_norm_out,
            self.io.hidden_size,
            self.rms_norm_eps,
        )?;
        self.cuda
            .quantize_nvfp4_f32(&self.io.final_norm_out, 1.0, &self.io.q8_scratch, self.io.hidden_size)?;
        self.cuda.nvfp4_nvfp4_matvec(
            &self.io.q8_scratch,
            &self.io.embed_weight,
            1.0,
            &self.io.logits_out,
            self.io.hidden_size / QK_NVFP4,
            self.io.vocab_size,
        )?;
        self.cuda.masked_argmax_f32_device_u32(
            &self.io.logits_out,
            &self.io.disallowed_token_ids,
            disallowed_count,
            &self.io.argmax_out,
            self.io.vocab_size,
        )?;
        Ok(())
    }
}
