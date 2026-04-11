use super::{
    bf16_round_to_f32, bf16_words_as_bytes,
    extract_gemma4_assistant_response_text, load_optional_scalar_f32, GemmaTextBenchmarkOutput,
    GemmaTextGenerationOptions, GemmaTextRuntimeSession, TextLayerTensorNames,
};
use crate::GemmaAttentionKind;
use makepad_ggml::backend::cuda::{is_available as cuda_is_available, CudaBuffer, CudaRuntime};
use std::cmp::max;
use std::error::Error;
use std::sync::Arc;
use std::time::{Duration, Instant};

const QK_Q8_1: usize = 32;
const Q8_1_BLOCK_BYTES: usize = 36;
const CUDA_FINAL_TEXT_NORM_WEIGHT_NAME: &str = "language_model.model.norm.weight";

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
    if !cuda_is_available() || !CudaNvfp4BenchmarkSession::supports(runtime) {
        return Ok(None);
    }

    let max_total_tokens = prompt_token_ids
        .len()
        .checked_add(options.max_new_tokens)
        .ok_or("CUDA benchmark token budget overflow")?;
    let mut session = CudaNvfp4BenchmarkSession::load(runtime.clone(), max_total_tokens)?;
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
    time_to_first_token_elapsed: Duration,
    steady_state_elapsed: Duration,
}

struct CudaNvfp4KvCache {
    key: CudaBuffer,
    value: CudaBuffer,
    kv_head_count: usize,
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
            kv_head_count,
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

    fn seq_len(&self) -> usize {
        self.stored_tokens
    }
}

struct CudaNvfp4Layer {
    attention_k_eq_v: bool,
    head_dim: usize,
    q_head_count: usize,
    k_head_count: usize,
    q_heads_per_kv: usize,
    rotary_dim: usize,
    rope_base: f32,
    hidden_size: usize,
    intermediate_size: usize,
    q_out_len: usize,
    k_out_len: usize,
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
    q_norm: CudaBuffer,
    k_norm: CudaBuffer,
    v_norm: CudaBuffer,
    q_rope: CudaBuffer,
    k_rope: CudaBuffer,
    attention_logits: CudaBuffer,
    attention_probs: CudaBuffer,
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
    hidden_size: usize,
    vocab_size: usize,
    embed_scale: f32,
}

struct CudaNvfp4BenchmarkSession {
    runtime_session: Arc<GemmaTextRuntimeSession>,
    cuda: CudaRuntime,
    io: CudaNvfp4TextIo,
    layers: Vec<CudaNvfp4Layer>,
    rms_norm_eps: f32,
    max_total_tokens: usize,
}

impl CudaNvfp4BenchmarkSession {
    fn supports(runtime: &GemmaTextRuntimeSession) -> bool {
        let text = &runtime.weights.snapshot.config.text_config;
        runtime.weights.quantization_mode() == "nvfp4"
            && !text.enable_moe_block
            && text.hidden_size_per_layer_input == 0
            && text.num_kv_shared_layers == 0
    }

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
            hidden_size,
            vocab_size,
            embed_scale: bf16_round_to_f32((hidden_size as f32).sqrt()),
        };

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
            let attention_row_stride = max_total_tokens;
            let qkv_out_len = q_out_len + k_out_len + if attention_k_eq_v { 0 } else { k_out_len };
            layers.push(CudaNvfp4Layer {
                attention_k_eq_v,
                head_dim,
                q_head_count,
                k_head_count,
                q_heads_per_kv,
                rotary_dim,
                rope_base: rope_params.rope_theta,
                hidden_size,
                intermediate_size,
                q_out_len,
                k_out_len,
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
                q_norm: cuda.alloc_f32(q_out_len)?,
                k_norm: cuda.alloc_f32(k_out_len)?,
                v_norm: cuda.alloc_f32(k_out_len)?,
                q_rope: cuda.alloc_f32(q_out_len)?,
                k_rope: cuda.alloc_f32(k_out_len)?,
                attention_logits: cuda.alloc_f32(q_head_count * attention_row_stride)?,
                attention_probs: cuda.alloc_f32(q_head_count * attention_row_stride)?,
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

        Ok(Self {
            runtime_session,
            cuda,
            io,
            layers,
            rms_norm_eps,
            max_total_tokens,
        })
    }

    fn reset(&mut self) {
        for layer in &mut self.layers {
            layer.kv_cache.reset();
        }
    }

    fn generate_greedy(&mut self, prompt_token_ids: &[u32], max_new_tokens: usize) -> Result<GenerationMetrics, Box<dyn Error>> {
        if prompt_token_ids.is_empty() {
            return Err("generation requires at least one prompt token".into());
        }
        if prompt_token_ids.len() + max_new_tokens > self.max_total_tokens {
            return Err("benchmark token budget exceeds CUDA session capacity".into());
        }
        self.reset();

        let ttft_started = Instant::now();
        let mut final_hidden_is_a = true;
        for (position, &token_id) in prompt_token_ids.iter().enumerate() {
            final_hidden_is_a = self.eval_token_hidden_from_token_id(token_id, position)?;
        }
        let first_token_id = self.greedy_token_from_hidden(final_hidden_is_a)?;
        let time_to_first_token_elapsed = ttft_started.elapsed();

        let mut generated = Vec::with_capacity(max_new_tokens);
        if !self.runtime_session.stop_tokens.contains(&first_token_id) {
            generated.push(first_token_id);
        }
        let steady_started = Instant::now();
        while !generated.is_empty() && generated.len() < max_new_tokens {
            let input_token = *generated
                .last()
                .ok_or("missing last generated token for CUDA decode")?;
            let position = prompt_token_ids.len() + generated.len() - 1;
            final_hidden_is_a = self.eval_token_hidden_from_token_id(input_token, position)?;
            let next_token = self.greedy_token_from_hidden(final_hidden_is_a)?;
            if self.runtime_session.stop_tokens.contains(&next_token) {
                break;
            }
            generated.push(next_token);
        }
        Ok(GenerationMetrics {
            generated_token_ids: generated,
            time_to_first_token_elapsed,
            steady_state_elapsed: steady_started.elapsed(),
        })
    }

    fn eval_token_hidden_from_token_id(&mut self, token_id: u32, position: usize) -> Result<bool, Box<dyn Error>> {
        if token_id as usize >= self.io.vocab_size {
            return Err(format!("token id {} exceeds vocab {}", token_id, self.io.vocab_size).into());
        }
        self.cuda.nvfp4_get_row_f32(&self.io.embed_weight, &self.io.hidden_a, self.io.hidden_size, token_id as usize)?;
        self.cuda.scale_f32_inplace(&self.io.hidden_a, self.io.embed_scale, self.io.hidden_size)?;

        let mut input_is_a = true;
        for layer in &mut self.layers {
            let (input, output) = if input_is_a {
                (&self.io.hidden_a, &self.io.hidden_b)
            } else {
                (&self.io.hidden_b, &self.io.hidden_a)
            };
            Self::eval_layer(
                &self.cuda,
                layer,
                &self.io.q8_scratch,
                input,
                output,
                position,
                self.rms_norm_eps,
            )?;
            input_is_a = !input_is_a;
        }
        Ok(input_is_a)
    }

    fn eval_layer(
        cuda: &CudaRuntime,
        layer: &mut CudaNvfp4Layer,
        q8_scratch: &CudaBuffer,
        input_hidden: &CudaBuffer,
        output_hidden: &CudaBuffer,
        position: usize,
        eps: f32,
    ) -> Result<(), Box<dyn Error>> {
        cuda.rms_norm_row_weighted_f32(
            input_hidden,
            &layer.input_norm_weight,
            &layer.input_norm_out,
            layer.hidden_size,
            eps,
        )?;
        cuda.quantize_q8_1_f32(&layer.input_norm_out, q8_scratch, layer.hidden_size)?;
        cuda.nvfp4_q8_1_matvec(
            q8_scratch,
            &layer.qkv_weight,
            &layer.qkv_out,
            layer.hidden_size / QK_Q8_1,
            layer.qkv_out_len,
        )?;
        cuda.rms_norm_rows_weighted_f32_offset(
            &layer.qkv_out,
            0,
            &layer.q_norm_weight,
            &layer.q_norm,
            0,
            layer.q_head_count,
            layer.head_dim,
            layer.head_dim,
            eps,
        )?;
        cuda.rms_norm_rows_weighted_f32_offset(
            &layer.qkv_out,
            layer.q_out_len,
            &layer.k_norm_weight,
            &layer.k_norm,
            0,
            layer.k_head_count,
            layer.head_dim,
            layer.head_dim,
            eps,
        )?;
        let v_offset = if layer.attention_k_eq_v {
            layer.q_out_len
        } else {
            layer.q_out_len + layer.k_out_len
        };
        cuda.rms_norm_rows_no_scale_f32_offset(
            &layer.qkv_out,
            v_offset,
            &layer.v_norm,
            0,
            layer.k_head_count,
            layer.head_dim,
            layer.head_dim,
            eps,
        )?;
        cuda.rope_rows_f32(
            &layer.q_norm,
            &layer.q_rope,
            layer.q_head_count,
            layer.head_dim,
            layer.head_dim,
            layer.rotary_dim,
            layer.rope_base,
            position,
        )?;
        cuda.rope_rows_f32(
            &layer.k_norm,
            &layer.k_rope,
            layer.k_head_count,
            layer.head_dim,
            layer.head_dim,
            layer.rotary_dim,
            layer.rope_base,
            position,
        )?;
        let slot = layer.kv_cache.stored_tokens;
        cuda.kv_append_f32(
            &layer.k_rope,
            &layer.v_norm,
            &layer.kv_cache.key,
            &layer.kv_cache.value,
            layer.kv_cache.kv_head_count,
            layer.kv_cache.head_dim,
            layer.kv_cache.max_tokens,
            slot,
        )?;
        layer.kv_cache.stored_tokens += 1;
        let seq_len = layer.kv_cache.seq_len();
        cuda.attention_logits_seq_f32(
            &layer.q_rope,
            &layer.kv_cache.key,
            &layer.attention_logits,
            layer.q_head_count,
            layer.q_heads_per_kv,
            layer.head_dim,
            layer.kv_cache.row_stride(),
            seq_len,
            0,
            layer.kv_cache.max_tokens,
            layer.kv_cache.max_tokens,
        )?;
        cuda.softmax_rows_f32(
            &layer.attention_logits,
            &layer.attention_probs,
            layer.q_head_count,
            layer.kv_cache.max_tokens,
            seq_len,
        )?;
        cuda.attention_weighted_sum_f32(
            &layer.attention_probs,
            &layer.kv_cache.value,
            &layer.attn_out,
            layer.q_head_count,
            layer.q_heads_per_kv,
            layer.head_dim,
            layer.kv_cache.row_stride(),
            seq_len,
            0,
            layer.kv_cache.max_tokens,
            layer.kv_cache.max_tokens,
            layer.head_dim,
        )?;
        cuda.quantize_q8_1_f32(&layer.attn_out, q8_scratch, layer.q_out_len)?;
        cuda.nvfp4_q8_1_matvec(
            q8_scratch,
            &layer.o_weight,
            &layer.o_proj_out,
            layer.q_out_len / QK_Q8_1,
            layer.hidden_size,
        )?;
        cuda.rms_norm_row_weighted_f32(
            &layer.o_proj_out,
            &layer.post_attention_norm_weight,
            &layer.post_attention_norm_out,
            layer.hidden_size,
            eps,
        )?;
        cuda.add_f32(input_hidden, &layer.post_attention_norm_out, &layer.residual_out, layer.hidden_size)?;
        cuda.rms_norm_row_weighted_f32(
            &layer.residual_out,
            &layer.pre_feedforward_norm_weight,
            &layer.pre_feedforward_norm_out,
            layer.hidden_size,
            eps,
        )?;
        cuda.quantize_q8_1_f32(&layer.pre_feedforward_norm_out, q8_scratch, layer.hidden_size)?;
        cuda.nvfp4_q8_1_matvec(
            q8_scratch,
            &layer.mlp_gate_up_weight,
            &layer.mlp_gate_up_out,
            layer.hidden_size / QK_Q8_1,
            layer.intermediate_size * 2,
        )?;
        cuda.geglu_split_f32(
            &layer.mlp_gate_up_out,
            &layer.geglu_out,
            layer.intermediate_size,
            layer.intermediate_size,
        )?;
        cuda.quantize_q8_1_f32(&layer.geglu_out, q8_scratch, layer.intermediate_size)?;
        cuda.nvfp4_q8_1_matvec(
            q8_scratch,
            &layer.mlp_down_weight,
            &layer.mlp_down_out,
            layer.intermediate_size / QK_Q8_1,
            layer.hidden_size,
        )?;
        cuda.rms_norm_row_weighted_f32(
            &layer.mlp_down_out,
            &layer.post_feedforward_norm_weight,
            &layer.post_feedforward_norm_out,
            layer.hidden_size,
            eps,
        )?;
        cuda.add_f32(&layer.residual_out, &layer.post_feedforward_norm_out, output_hidden, layer.hidden_size)?;
        if let Some(scale) = layer.layer_scalar {
            cuda.scale_f32_inplace(output_hidden, scale, layer.hidden_size)?;
        }
        Ok(())
    }

    fn greedy_token_from_hidden(&mut self, hidden_is_a: bool) -> Result<u32, Box<dyn Error>> {
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
        self.cuda.quantize_q8_1_f32(&self.io.final_norm_out, &self.io.q8_scratch, self.io.hidden_size)?;
        self.cuda.nvfp4_q8_1_matvec(
            &self.io.q8_scratch,
            &self.io.embed_weight,
            &self.io.logits_out,
            self.io.hidden_size / QK_Q8_1,
            self.io.vocab_size,
        )?;
        self.cuda.argmax_f32(&self.io.logits_out, &self.io.argmax_out, self.io.vocab_size)?;
        Ok(self.cuda.read_u32(&self.io.argmax_out)?)
    }
}
