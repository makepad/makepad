use crate::{
    MlxDType, MlxGreedyToken, MlxQwen35MoeIndexedSafetensors, MlxQwen35MoeLayerKind,
    MlxQwen35MoeTensors, MlxTokenizer, MlxTokenizerConfig, Result,
};
use makepad_ggml::backend::{
    try_affine_quantized_matmul_bf16, try_matmul_nt_ggml_bytes, AffineQuantizedMatmulSpec,
};
use makepad_ggml::quant::GGML_TYPE_BF16;
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::mem::size_of;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QwenChatRole {
    System,
    User,
    Assistant,
}

impl QwenChatRole {
    pub fn as_prompt_label(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::User => "user",
            Self::Assistant => "assistant",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QwenChatMessage {
    pub role: QwenChatRole,
    pub content: Arc<str>,
}

impl QwenChatMessage {
    pub fn new(role: QwenChatRole, content: impl Into<String>) -> Self {
        Self {
            role,
            content: Arc::<str>::from(content.into()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MlxQwen35MoeHybridCacheTemplate {
    pub attention_layers: Vec<u32>,
    pub recurrent_layers: Vec<u32>,
    pub attention_k_width: u64,
    pub attention_v_width: u64,
    pub recurrent_r_width: u64,
    pub recurrent_s_width: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MlxModelLayerRole {
    Attention,
    Recurrent,
    Unknown,
}

impl MlxModelLayerRole {
    pub fn name(self) -> &'static str {
        match self {
            Self::Attention => "attention",
            Self::Recurrent => "recurrent",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MlxTensorInfo {
    pub canonical_name: String,
    pub actual_name: String,
    pub shard_path: PathBuf,
    pub dtype: MlxDType,
    pub shape: Vec<u64>,
    pub data_offsets: [u64; 2],
    pub size_bytes: u64,
}

impl MlxTensorInfo {
    fn from_indexed(
        weights: &MlxQwen35MoeIndexedSafetensors,
        canonical_name: &str,
    ) -> Result<Self> {
        let actual_name = weights.actual_tensor_name(canonical_name)?.to_owned();
        let header = weights.header_for_tensor(canonical_name)?;
        let entry = weights.tensor(canonical_name)?;
        Ok(Self {
            canonical_name: canonical_name.to_owned(),
            actual_name,
            shard_path: header.path.clone(),
            dtype: entry.dtype,
            shape: entry.shape.clone(),
            data_offsets: entry.data_offsets,
            size_bytes: entry.data_len_bytes(),
        })
    }
}

#[derive(Clone, Debug)]
pub struct MlxModelLayerInventory {
    pub index: u32,
    pub role: MlxModelLayerRole,
    pub tensors: BTreeMap<String, MlxTensorInfo>,
}

#[derive(Clone, Debug, Default)]
pub struct MlxModelTensorInventory {
    pub globals: BTreeMap<String, MlxTensorInfo>,
    pub layers: Vec<MlxModelLayerInventory>,
}

impl MlxModelTensorInventory {
    pub fn unique_tensor_count(&self) -> usize {
        self.unique_tensors().len()
    }

    pub fn total_tensor_bytes(&self) -> u64 {
        self.unique_tensors()
            .into_iter()
            .map(|tensor| tensor.size_bytes)
            .sum()
    }

    pub fn count_layers_with_role(&self, role: MlxModelLayerRole) -> usize {
        self.layers
            .iter()
            .filter(|layer| layer.role == role)
            .count()
    }

    pub fn unique_tensors(&self) -> Vec<MlxTensorInfo> {
        let mut tensors = BTreeMap::new();
        self.visit_tensors(|tensor| {
            tensors
                .entry(tensor.actual_name.clone())
                .or_insert_with(|| tensor.clone());
        });
        tensors.into_values().collect()
    }

    fn visit_tensors(&self, mut visit: impl FnMut(&MlxTensorInfo)) {
        for tensor in self.globals.values() {
            visit(tensor);
        }
        for layer in &self.layers {
            for tensor in layer.tensors.values() {
                visit(tensor);
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct MlxQwen35MoeTailProbePlan {
    pub output_norm: MlxTensorInfo,
    pub output: MlxTensorInfo,
}

#[derive(Clone, Debug)]
pub struct MlxQwen35MoeExecutionPlan {
    pub dims: MlxQwen35MoeDims,
    pub inventory: MlxModelTensorInventory,
    pub tail_probe: MlxQwen35MoeTailProbePlan,
    pub cache_template: MlxQwen35MoeHybridCacheTemplate,
}

impl MlxQwen35MoeExecutionPlan {
    pub fn layer_count(&self) -> usize {
        self.inventory.layers.len()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MlxQwen35MoeExpertRoute {
    pub expert_index: u32,
    pub logit: f32,
    pub probability: f32,
    pub weight: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MlxQwen35MoeFfnOutput {
    pub router_logits: Vec<f32>,
    pub router_probabilities: Vec<f32>,
    pub routed_experts: Vec<MlxQwen35MoeExpertRoute>,
    pub routed_output: Vec<f32>,
    pub shared_gate: f32,
    pub shared_output: Vec<f32>,
    pub output: Vec<f32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MlxQwen35MoeDims {
    pub vocab_size: u32,
    pub block_count: u32,
    pub embedding_length: u32,
    pub attention_head_count: u32,
    pub attention_head_count_kv: u32,
    pub attention_key_length: u32,
    pub attention_value_length: u32,
    pub expert_count: u32,
    pub expert_used_count: u32,
    pub ssm_conv_kernel: u32,
    pub ssm_state_size: u32,
    pub ssm_group_count: u32,
    pub ssm_time_step_rank: u32,
    pub ssm_inner_size: u32,
    pub full_attention_interval: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MlxQwen35MoeAttentionDecodeState {
    pub layer_index: u32,
    pub key_cache: Vec<f32>,
    pub value_cache: Vec<f32>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MlxQwen35MoeRecurrentDecodeState {
    pub layer_index: u32,
    pub conv_state: Vec<f32>,
    pub ssm_state: Vec<f32>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum MlxQwen35MoeLayerDecodeState {
    Attention(MlxQwen35MoeAttentionDecodeState),
    Recurrent(MlxQwen35MoeRecurrentDecodeState),
}

#[derive(Clone, Debug, PartialEq)]
pub struct MlxQwen35MoeDecodeState {
    pub token_count: usize,
    pub layers: Vec<MlxQwen35MoeLayerDecodeState>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MlxQwen35MoeStopReason {
    MaxNewTokens,
    EosToken(u32),
}

#[derive(Clone, Debug)]
pub struct MlxQwen35MoeGenerationMetrics {
    pub elapsed: Duration,
    pub time_to_first_token_elapsed: Duration,
    pub steady_state_elapsed: Duration,
    pub steady_state_generated_tokens: usize,
    pub prompt_prefill_tokens_per_second: f64,
    pub steady_state_decode_tokens_per_second: f64,
    pub decode_tokens_per_second: f64,
}

#[derive(Clone, Debug)]
pub struct MlxQwen35MoeGenerationOutput {
    pub prompt_token_ids: Arc<[u32]>,
    pub generated_token_ids: Arc<[u32]>,
    pub generated_text: Arc<str>,
    pub stop_reason: MlxQwen35MoeStopReason,
    pub metrics: MlxQwen35MoeGenerationMetrics,
}

impl MlxQwen35MoeDims {
    pub fn from_indexed(weights: &MlxQwen35MoeIndexedSafetensors) -> Result<Self> {
        let snapshot = &weights.snapshot;
        let cfg = &snapshot.config.text_config;
        let token_embd = weights.tensor("token_embd.weight")?;
        let vocab_size = token_embd.shape.first().copied().ok_or_else(|| {
            crate::MlxRtError::InvalidModelDir {
                path: snapshot.paths().model_safetensors_index_json.clone(),
                message: "token_embd.weight is missing vocab dimension".to_string(),
            }
        })?;
        let vocab_size =
            u32::try_from(vocab_size).map_err(|_| crate::MlxRtError::InvalidModelDir {
                path: snapshot.paths().model_safetensors_index_json.clone(),
                message: format!("vocab size {} does not fit in u32", vocab_size),
            })?;
        let ssm_inner_size = cfg
            .linear_num_value_heads
            .checked_mul(cfg.linear_value_head_dim)
            .ok_or_else(|| crate::MlxRtError::InvalidModelDir {
                path: snapshot.paths().config_json.clone(),
                message: "overflow computing qwen3_5_moe ssm_inner_size".to_string(),
            })?;
        Ok(Self {
            vocab_size,
            block_count: cfg.num_hidden_layers,
            embedding_length: cfg.hidden_size,
            attention_head_count: cfg.num_attention_heads,
            attention_head_count_kv: cfg.num_key_value_heads,
            attention_key_length: cfg.head_dim,
            attention_value_length: cfg.head_dim,
            expert_count: cfg.num_experts,
            expert_used_count: cfg.num_experts_per_tok,
            ssm_conv_kernel: cfg.linear_conv_kernel_dim,
            ssm_state_size: cfg.linear_key_head_dim,
            ssm_group_count: cfg.linear_num_key_heads,
            ssm_time_step_rank: cfg.linear_num_value_heads,
            ssm_inner_size,
            full_attention_interval: cfg.full_attention_interval,
        })
    }

    pub fn recurrent_conv_width(&self) -> Result<u64> {
        let conv_prefix = u64::from(self.ssm_conv_kernel.saturating_sub(1));
        let channels = u64::from(self.ssm_inner_size)
            .checked_add(
                2_u64
                    .checked_mul(u64::from(self.ssm_group_count))
                    .and_then(|v| v.checked_mul(u64::from(self.ssm_state_size)))
                    .ok_or_else(|| crate::MlxRtError::InvalidModelDir {
                        path: PathBuf::new(),
                        message: "overflow computing qwen3_5_moe conv channels".to_string(),
                    })?,
            )
            .ok_or_else(|| crate::MlxRtError::InvalidModelDir {
                path: PathBuf::new(),
                message: "overflow computing qwen3_5_moe conv channels".to_string(),
            })?;
        conv_prefix
            .checked_mul(channels)
            .ok_or_else(|| crate::MlxRtError::InvalidModelDir {
                path: PathBuf::new(),
                message: "overflow computing qwen3_5_moe conv width".to_string(),
            })
    }

    pub fn recurrent_state_width(&self) -> Result<u64> {
        u64::from(self.ssm_state_size)
            .checked_mul(u64::from(self.ssm_inner_size))
            .ok_or_else(|| crate::MlxRtError::InvalidModelDir {
                path: PathBuf::new(),
                message: "overflow computing qwen3_5_moe recurrent state width".to_string(),
            })
    }

    pub fn attention_k_width(&self) -> u64 {
        u64::from(self.attention_key_length) * u64::from(self.attention_head_count_kv)
    }

    pub fn attention_v_width(&self) -> u64 {
        u64::from(self.attention_value_length) * u64::from(self.attention_head_count_kv)
    }

    pub fn recurrent_value_head_dim(&self) -> Result<u32> {
        if self.ssm_time_step_rank == 0 {
            return Err(crate::MlxRtError::InvalidModelDir {
                path: PathBuf::new(),
                message: "qwen3_5_moe ssm_time_step_rank must be greater than zero".to_string(),
            });
        }
        if self.ssm_inner_size % self.ssm_time_step_rank != 0 {
            return Err(crate::MlxRtError::InvalidModelDir {
                path: PathBuf::new(),
                message: format!(
                    "qwen3_5_moe ssm_inner_size {} is not divisible by ssm_time_step_rank {}",
                    self.ssm_inner_size, self.ssm_time_step_rank
                ),
            });
        }
        Ok(self.ssm_inner_size / self.ssm_time_step_rank)
    }
}

pub fn format_qwen35moe_chat_prompt(
    tokenizer_config: &MlxTokenizerConfig,
    messages: &[QwenChatMessage],
) -> std::result::Result<String, Box<dyn Error>> {
    format_qwen35moe_chat_prompt_with_image(tokenizer_config, messages, false)
}

pub fn format_qwen35moe_chat_prompt_with_image(
    tokenizer_config: &MlxTokenizerConfig,
    messages: &[QwenChatMessage],
    include_image_on_last_user_turn: bool,
) -> std::result::Result<String, Box<dyn Error>> {
    if messages.is_empty() {
        return Err("chat prompt requires at least one message".into());
    }

    let mut prompt = String::new();
    if !tokenizer_config.bos_token.is_empty() {
        prompt.push_str(&tokenizer_config.bos_token);
    }
    for (index, message) in messages.iter().enumerate() {
        prompt.push_str("<|im_start|>");
        prompt.push_str(message.role.as_prompt_label());
        prompt.push('\n');
        if include_image_on_last_user_turn
            && index + 1 == messages.len()
            && message.role == QwenChatRole::User
        {
            prompt.push_str(&tokenizer_config.boi_token);
            prompt.push_str(&tokenizer_config.image_token);
            prompt.push_str(&tokenizer_config.eoi_token);
        }
        prompt.push_str(message.content.as_ref());
        prompt.push_str("<|im_end|>\n");
    }
    prompt.push_str("<|im_start|>assistant\n");
    Ok(prompt)
}

pub fn extract_qwen35moe_assistant_response_text(
    tokenizer_config: &MlxTokenizerConfig,
    raw_text: &str,
) -> String {
    let mut text = raw_text.trim_start().to_owned();
    if let Some(rest) = text.strip_prefix("<|im_start|>assistant\n") {
        text = rest.to_owned();
    }
    if !tokenizer_config.eos_token.is_empty() {
        if let Some(end) = text.find(&tokenizer_config.eos_token) {
            text.truncate(end);
        }
    }
    if let Some(end) = text.find("<|im_end|>") {
        text.truncate(end);
    }
    if let Some(end_think) = text.find("</think>") {
        let rest = &text[end_think + "</think>".len()..];
        text = rest.trim_start_matches('\n').to_owned();
    }
    text.trim().to_owned()
}

#[derive(Clone)]
pub struct MlxQwen35MoeRuntimeSession {
    pub model_path: PathBuf,
    pub weights: MlxQwen35MoeIndexedSafetensors,
    pub tokenizer: MlxTokenizer,
    pub tensors: MlxQwen35MoeTensors,
    pub dims: MlxQwen35MoeDims,
    pub stop_tokens: BTreeSet<u32>,
    pub cache_template: MlxQwen35MoeHybridCacheTemplate,
}

impl MlxQwen35MoeRuntimeSession {
    pub fn load(model_path: &Path) -> Result<Arc<Self>> {
        let model_root = qwen_model_root_dir(model_path)?;
        let weights = MlxQwen35MoeIndexedSafetensors::load(&model_root)?;
        let tokenizer = MlxTokenizer::load(&model_root)?;
        let tensors = MlxQwen35MoeTensors::from_indexed(&weights)?;
        let dims = MlxQwen35MoeDims::from_indexed(&weights)?;
        let stop_tokens = weights
            .snapshot
            .generation_config
            .eos_token_id
            .iter()
            .copied()
            .collect::<BTreeSet<_>>();
        let cache_template = MlxQwen35MoeHybridCacheTemplate::from_runtime_parts(&tensors, &dims)?;

        Ok(Arc::new(Self {
            model_path: model_path.to_path_buf(),
            weights,
            tokenizer,
            tensors,
            dims,
            stop_tokens,
            cache_template,
        }))
    }

    pub fn tokenizer_config(&self) -> &MlxTokenizerConfig {
        self.weights.snapshot.tokenizer_config()
    }

    pub fn format_chat_prompt(
        &self,
        messages: &[QwenChatMessage],
        include_image_on_last_user_turn: bool,
    ) -> std::result::Result<String, Box<dyn Error>> {
        format_qwen35moe_chat_prompt_with_image(
            self.tokenizer_config(),
            messages,
            include_image_on_last_user_turn,
        )
    }

    pub fn tokenize_prompt(&self, formatted_prompt: &str) -> Result<Arc<[u32]>> {
        let ids = self.tokenizer.encode(formatted_prompt).map_err(|err| {
            crate::MlxRtError::InvalidModelDir {
                path: self.weights.snapshot.paths().tokenizer_json.clone(),
                message: err.to_string(),
            }
        })?;
        if ids.is_empty() {
            return Err(crate::MlxRtError::InvalidModelDir {
                path: self.weights.snapshot.paths().tokenizer_json.clone(),
                message: "formatted prompt encoded to zero tokens".to_string(),
            });
        }
        Ok(Arc::<[u32]>::from(ids))
    }

    pub fn backend_label(&self) -> &'static str {
        if makepad_ggml::backend::cuda::is_available() {
            "qwen-cuda-reference"
        } else {
            "qwen-reference"
        }
    }

    pub fn execution_plan(&self) -> Result<MlxQwen35MoeExecutionPlan> {
        Ok(MlxQwen35MoeExecutionPlan {
            dims: self.dims.clone(),
            inventory: qwen35moe_inventory(&self.weights, &self.tensors)?,
            tail_probe: MlxQwen35MoeTailProbePlan {
                output_norm: MlxTensorInfo::from_indexed(
                    &self.weights,
                    &self.tensors.globals.output_norm,
                )?,
                output: MlxTensorInfo::from_indexed(&self.weights, &self.tensors.globals.output)?,
            },
            cache_template: self.cache_template.clone(),
        })
    }

    pub fn new_decode_state(&self) -> Result<MlxQwen35MoeDecodeState> {
        let mut layers = Vec::with_capacity(self.tensors.layers.len());
        let recurrent_state_width = self.dims.recurrent_state_width()? as usize;
        let recurrent_conv_width = self.dims.recurrent_conv_width()? as usize;
        let attention_k_width = self.dims.attention_k_width() as usize;
        let attention_v_width = self.dims.attention_v_width() as usize;
        for layer in &self.tensors.layers {
            let state = match layer.kind {
                MlxQwen35MoeLayerKind::Attention => {
                    MlxQwen35MoeLayerDecodeState::Attention(MlxQwen35MoeAttentionDecodeState {
                        layer_index: layer.index,
                        key_cache: Vec::new(),
                        value_cache: Vec::new(),
                    })
                }
                MlxQwen35MoeLayerKind::Recurrent => {
                    MlxQwen35MoeLayerDecodeState::Recurrent(MlxQwen35MoeRecurrentDecodeState {
                        layer_index: layer.index,
                        conv_state: vec![0.0; recurrent_conv_width],
                        ssm_state: vec![0.0; recurrent_state_width],
                    })
                }
            };
            match &state {
                MlxQwen35MoeLayerDecodeState::Attention(attn) => {
                    debug_assert_eq!(attn.key_cache.len() % attention_k_width.max(1), 0);
                    debug_assert_eq!(attn.value_cache.len() % attention_v_width.max(1), 0);
                }
                MlxQwen35MoeLayerDecodeState::Recurrent(recurrent) => {
                    debug_assert_eq!(recurrent.conv_state.len(), recurrent_conv_width);
                    debug_assert_eq!(recurrent.ssm_state.len(), recurrent_state_width);
                }
            }
            layers.push(state);
        }
        Ok(MlxQwen35MoeDecodeState {
            token_count: 0,
            layers,
        })
    }

    pub fn generate_preformatted_streaming<F>(
        &self,
        formatted_prompt_text: Arc<str>,
        max_new_tokens: Option<usize>,
        do_sample: bool,
        mut on_text_delta: F,
    ) -> std::result::Result<Arc<MlxQwen35MoeGenerationOutput>, Box<dyn Error>>
    where
        F: FnMut(&str) -> std::result::Result<(), Box<dyn Error>>,
    {
        let prompt_token_ids = self.tokenize_prompt(formatted_prompt_text.as_ref())?;
        let max_new_tokens = max_new_tokens.unwrap_or(512);
        let max_context_tokens = self.max_context_tokens();
        if prompt_token_ids.len().saturating_add(max_new_tokens) > max_context_tokens {
            return Err(format!(
                "Qwen prompt token budget {} + max_new_tokens {} exceeds context window {}",
                prompt_token_ids.len(),
                max_new_tokens,
                max_context_tokens,
            )
            .into());
        }
        if max_new_tokens == 0 {
            return Ok(Arc::new(MlxQwen35MoeGenerationOutput {
                prompt_token_ids,
                generated_token_ids: Arc::from([]),
                generated_text: Arc::from(""),
                stop_reason: MlxQwen35MoeStopReason::MaxNewTokens,
                metrics: MlxQwen35MoeGenerationMetrics {
                    elapsed: Duration::ZERO,
                    time_to_first_token_elapsed: Duration::ZERO,
                    steady_state_elapsed: Duration::ZERO,
                    steady_state_generated_tokens: 0,
                    prompt_prefill_tokens_per_second: 0.0,
                    steady_state_decode_tokens_per_second: 0.0,
                    decode_tokens_per_second: 0.0,
                },
            }));
        }

        let started = Instant::now();
        let mut decode_state = self.new_decode_state()?;
        let mut logits = Vec::new();
        for (position, &token_id) in prompt_token_ids.iter().enumerate() {
            logits = self.eval_token_logits_reference_f32(token_id, position, &mut decode_state)?;
        }

        let sampling = self.sampling_options(do_sample);
        let disallowed_token_ids = self.generation_disallowed_token_ids();
        let mut rng = QwenSamplingRng::new(0);
        let mut detokenizer = self.tokenizer.streaming_detokenizer(true);
        let skip_special_token_ids = self.tokenizer.special_token_ids().to_vec();
        let mut generated_token_ids = Vec::with_capacity(max_new_tokens);
        let mut time_to_first_token_elapsed = None;

        let stop_reason = loop {
            let next_token =
                sample_token_from_logits_f32(&logits, &disallowed_token_ids, &sampling, &mut rng)
                    .map_err(|err| self.invalid_runtime_error(err))?;
            generated_token_ids.push(next_token.token_id);
            if time_to_first_token_elapsed.is_none() {
                time_to_first_token_elapsed = Some(started.elapsed());
            }

            let delta = detokenizer.add_token(next_token.token_id, &skip_special_token_ids);
            if !delta.is_empty() {
                on_text_delta(&delta)?;
            }

            if self.stop_tokens.contains(&next_token.token_id) {
                break MlxQwen35MoeStopReason::EosToken(next_token.token_id);
            }
            if generated_token_ids.len() >= max_new_tokens {
                break MlxQwen35MoeStopReason::MaxNewTokens;
            }

            let position = prompt_token_ids.len() + generated_token_ids.len() - 1;
            logits =
                self.eval_token_logits_reference_f32(next_token.token_id, position, &mut decode_state)?;
        };

        let final_delta = detokenizer.finalize();
        if !final_delta.is_empty() {
            on_text_delta(&final_delta)?;
        }

        let elapsed = started.elapsed();
        let generated_token_count = generated_token_ids.len();
        let generated_text = extract_qwen35moe_assistant_response_text(
            self.tokenizer_config(),
            &self.tokenizer.decode(&generated_token_ids)?,
        );

        Ok(Arc::new(MlxQwen35MoeGenerationOutput {
            prompt_token_ids: prompt_token_ids.clone(),
            generated_token_ids: Arc::from(generated_token_ids),
            generated_text: Arc::from(generated_text),
            stop_reason,
            metrics: build_qwen_generation_metrics(
                elapsed,
                prompt_token_ids.len(),
                generated_token_count,
                time_to_first_token_elapsed.unwrap_or(elapsed),
            ),
        }))
    }

    fn max_context_tokens(&self) -> usize {
        self.weights.snapshot.config.text_config.max_position_embeddings as usize
    }

    fn sampling_options(&self, do_sample: bool) -> QwenSamplingOptions {
        let config = &self.weights.snapshot.generation_config;
        QwenSamplingOptions {
            do_sample,
            temperature: config.temperature,
            top_k: config.top_k,
            top_p: config.top_p,
        }
    }

    fn generation_disallowed_token_ids(&self) -> Vec<u32> {
        let mut token_ids = self.tokenizer.special_token_ids().to_vec();
        token_ids.retain(|token_id| !self.stop_tokens.contains(token_id));
        token_ids.sort_unstable();
        token_ids.dedup();
        token_ids
    }

    fn rope_sections4(&self) -> Result<[u32; 4]> {
        let sections = &self.weights.snapshot.config.text_config.rope_parameters.mrope_section;
        if sections.len() != 3 {
            return Err(self.invalid_runtime_error(format!(
                "expected 3 text mrope sections, got {:?}",
                sections
            )));
        }
        Ok([sections[0], sections[1], sections[2], 0])
    }

    fn attention_rotary_dim(&self) -> usize {
        ((self.weights.snapshot.config.text_config.head_dim as f32)
            * self.weights.snapshot.config.text_config.partial_rotary_factor)
            .round() as usize
    }

    fn eval_token_logits_reference_f32(
        &self,
        token_id: u32,
        position: usize,
        decode_state: &mut MlxQwen35MoeDecodeState,
    ) -> Result<Vec<f32>> {
        let mut hidden = self.token_embedding_f32(token_id)?;
        if hidden.len() != self.dims.embedding_length as usize {
            return Err(self.invalid_runtime_error(format!(
                "token embedding output length {} does not match hidden size {}",
                hidden.len(),
                self.dims.embedding_length
            )));
        }
        for layer_index in 0..self.tensors.layers.len() {
            self.apply_layer_decode_reference_f32(layer_index, position, &mut hidden, decode_state)?;
        }
        hidden = self.rms_norm_weighted_f32(
            &hidden,
            &self.tensors.globals.output_norm,
            self.weights.snapshot.config.text_config.rms_norm_eps,
        )?;
        let logits = self.output_logits_f32(&hidden)?;
        decode_state.token_count = position + 1;
        Ok(logits)
    }

    fn apply_layer_decode_reference_f32(
        &self,
        layer_index: usize,
        position: usize,
        hidden: &mut Vec<f32>,
        decode_state: &mut MlxQwen35MoeDecodeState,
    ) -> Result<()> {
        let layer = self
            .tensors
            .layers
            .get(layer_index)
            .ok_or_else(|| self.invalid_runtime_error(format!("layer {} out of range", layer_index)))?;
        let attn_input = self.rms_norm_weighted_f32(
            hidden,
            &layer.attn_norm,
            self.weights.snapshot.config.text_config.rms_norm_eps,
        )?;
        let attn_out = match decode_state.layers.get_mut(layer_index) {
            Some(MlxQwen35MoeLayerDecodeState::Attention(state)) => {
                self.apply_attention_layer_decode_reference_f32(layer, &attn_input, position, state)?
            }
            Some(MlxQwen35MoeLayerDecodeState::Recurrent(state)) => {
                self.apply_recurrent_layer_decode_reference_f32(layer, &attn_input, state)?
            }
            None => {
                return Err(self.invalid_runtime_error(format!(
                    "missing decode state for layer {}",
                    layer_index
                )))
            }
        };
        add_residual_in_place(hidden, &attn_out).map_err(|err| self.invalid_runtime_error(err))?;

        let ffn_input = self.rms_norm_weighted_f32(
            hidden,
            &layer.post_attention_norm,
            self.weights.snapshot.config.text_config.rms_norm_eps,
        )?;
        let ffn_out = self.apply_moe_ffn_reference_f32(layer.index, &ffn_input)?;
        add_residual_in_place(hidden, &ffn_out.output)
            .map_err(|err| self.invalid_runtime_error(err))?;
        Ok(())
    }

    fn apply_attention_layer_decode_reference_f32(
        &self,
        layer: &crate::MlxQwen35MoeLayerTensors,
        input: &[f32],
        position: usize,
        state: &mut MlxQwen35MoeAttentionDecodeState,
    ) -> Result<Vec<f32>> {
        let attention = layer.attention.as_ref().ok_or_else(|| {
            self.invalid_runtime_error(format!(
                "layer {} is missing full-attention tensors",
                layer.index
            ))
        })?;
        let query_gate = self.project_vector_f32(input, &attention.wq)?;
        let mut key = self.project_vector_f32(input, &attention.wk)?;
        let value = self.project_vector_f32(input, &attention.wv)?;
        let (mut query, gate) = split_interleaved_query_gate_heads(
            &query_gate,
            self.dims.attention_key_length as usize,
            self.dims.attention_head_count as usize,
        )
        .map_err(|message| {
            self.invalid_runtime_error(format!(
                "layer {} attention query/gate split failed: {}",
                layer.index, message
            ))
        })?;
        let q_norm_weights = self.vector_tensor_f32(&attention.attn_q_norm)?;
        let k_norm_weights = self.vector_tensor_f32(&attention.attn_k_norm)?;
        query = rms_norm_rows_shared_weight_f32(
            &query,
            &q_norm_weights,
            self.dims.attention_head_count as usize,
            self.dims.attention_key_length as usize,
            self.weights.snapshot.config.text_config.rms_norm_eps,
        )
        .map_err(|message| self.invalid_runtime_error(message))?;
        key = rms_norm_rows_shared_weight_f32(
            &key,
            &k_norm_weights,
            self.dims.attention_head_count_kv as usize,
            self.dims.attention_key_length as usize,
            self.weights.snapshot.config.text_config.rms_norm_eps,
        )
        .map_err(|message| self.invalid_runtime_error(message))?;

        let positions = qwen_text_mrope_positions(position as u32);
        let sections = self.rope_sections4()?;
        let rotary_dim = self.attention_rotary_dim();
        apply_qwen_mrope_rows_in_place(
            &mut query,
            self.dims.attention_head_count as usize,
            self.dims.attention_key_length as usize,
            rotary_dim,
            positions,
            sections,
            self.weights.snapshot.config.text_config.rope_parameters.rope_theta,
        )
        .map_err(|message| self.invalid_runtime_error(message))?;
        apply_qwen_mrope_rows_in_place(
            &mut key,
            self.dims.attention_head_count_kv as usize,
            self.dims.attention_key_length as usize,
            rotary_dim,
            positions,
            sections,
            self.weights.snapshot.config.text_config.rope_parameters.rope_theta,
        )
        .map_err(|message| self.invalid_runtime_error(message))?;

        state.key_cache.extend_from_slice(&key);
        state.value_cache.extend_from_slice(&value);
        let mut attn_out = grouped_self_attention_step_f32(
            &query,
            &state.key_cache,
            &state.value_cache,
            self.dims.attention_head_count as usize,
            self.dims.attention_head_count_kv as usize,
            self.dims.attention_key_length as usize,
            state.key_cache.len() / self.dims.attention_k_width() as usize,
        )
        .map_err(|message| {
            self.invalid_runtime_error(format!(
                "layer {} grouped attention failed: {}",
                layer.index, message
            ))
        })?;
        apply_sigmoid_gate_in_place(&mut attn_out, &gate)
            .map_err(|message| self.invalid_runtime_error(message))?;
        self.project_vector_f32(&attn_out, &attention.wo)
    }

    fn apply_recurrent_layer_decode_reference_f32(
        &self,
        layer: &crate::MlxQwen35MoeLayerTensors,
        input: &[f32],
        state: &mut MlxQwen35MoeRecurrentDecodeState,
    ) -> Result<Vec<f32>> {
        let recurrent = layer.recurrent.as_ref().ok_or_else(|| {
            self.invalid_runtime_error(format!(
                "layer {} is missing recurrent tensors",
                layer.index
            ))
        })?;
        let qkv = self.project_vector_f32(input, &recurrent.wqkv)?;
        let z = self.project_vector_f32(input, &recurrent.wqkv_gate)?;
        let beta_logits = self.project_vector_f32(input, &recurrent.ssm_beta)?;
        let alpha = self.project_vector_f32(input, &recurrent.ssm_alpha)?;
        let conv_kernel =
            self.conv1d_kernel_f32(&recurrent.ssm_conv1d, self.dims.ssm_conv_kernel as usize, qkv.len())?;
        let conv_out = apply_ssm_conv_with_state_f32(
            &qkv,
            &mut state.conv_state,
            &conv_kernel,
            self.dims.ssm_conv_kernel as usize,
        )
        .map_err(|message| self.invalid_runtime_error(message))?;
        let (mut q, mut k, v) = split_recurrent_qkv_projection(
            &conv_out,
            self.dims.ssm_state_size as usize,
            self.dims.ssm_group_count as usize,
            self.dims.recurrent_value_head_dim()? as usize,
            self.dims.ssm_time_step_rank as usize,
        )
        .map_err(|message| self.invalid_runtime_error(message))?;
        q = rms_norm_rows_no_scale_f32(
            &q,
            self.dims.ssm_group_count as usize,
            self.dims.ssm_state_size as usize,
            self.weights.snapshot.config.text_config.rms_norm_eps,
        )
        .map_err(|message| self.invalid_runtime_error(message))?;
        k = rms_norm_rows_no_scale_f32(
            &k,
            self.dims.ssm_group_count as usize,
            self.dims.ssm_state_size as usize,
            self.weights.snapshot.config.text_config.rms_norm_eps,
        )
        .map_err(|message| self.invalid_runtime_error(message))?;
        let inv_scale = (self.dims.ssm_state_size as f32).sqrt().recip();
        scale_in_place(&mut q, inv_scale * inv_scale);
        scale_in_place(&mut k, inv_scale);

        let dt_bias = self.vector_tensor_f32(&recurrent.ssm_dt)?;
        let a_log = self.vector_tensor_f32(&recurrent.ssm_a)?;
        let beta = beta_logits.into_iter().map(sigmoid_f32).collect::<Vec<_>>();
        let gate = compute_qwen_decay_gate(&a_log, &alpha, &dt_bias)
            .map_err(|message| self.invalid_runtime_error(message))?;
        let mut recurrent_out = gated_delta_net_step_f32(
            &q,
            &k,
            &v,
            &gate,
            &beta,
            &mut state.ssm_state,
            self.dims.ssm_state_size as usize,
            self.dims.ssm_group_count as usize,
            self.dims.recurrent_value_head_dim()? as usize,
            self.dims.ssm_time_step_rank as usize,
        )
        .map_err(|message| {
            self.invalid_runtime_error(format!(
                "layer {} gated delta failed: {}",
                layer.index, message
            ))
        })?;
        let ssm_norm_weights = self.vector_tensor_f32(&recurrent.ssm_norm)?;
        recurrent_out = rms_norm_rows_shared_weight_f32(
            &recurrent_out,
            &ssm_norm_weights,
            self.dims.ssm_time_step_rank as usize,
            self.dims.recurrent_value_head_dim()? as usize,
            self.weights.snapshot.config.text_config.rms_norm_eps,
        )
        .map_err(|message| self.invalid_runtime_error(message))?;
        apply_silu_gate_in_place(&mut recurrent_out, &z)
            .map_err(|message| self.invalid_runtime_error(message))?;
        self.project_vector_f32(&recurrent_out, &recurrent.ssm_out)
    }

    pub fn token_embedding_f32(&self, token_id: u32) -> Result<Vec<f32>> {
        let token_id = token_id as u64;
        let entry = self.weights.tensor(&self.tensors.globals.token_embd)?;
        if entry.shape.len() != 2 {
            return Err(self.invalid_runtime_error(format!(
                "token embedding tensor {} expected rank 2, got {:?}",
                self.tensors.globals.token_embd, entry.shape
            )));
        }
        if token_id >= entry.shape[0] {
            return Err(self.invalid_runtime_error(format!(
                "token id {} exceeds vocabulary rows {}",
                token_id, entry.shape[0]
            )));
        }
        self.read_rank2_row_f32(&self.tensors.globals.token_embd, token_id)
    }

    pub fn vector_tensor_f32(&self, name: &str) -> Result<Vec<f32>> {
        let entry = self.weights.tensor(name)?;
        if entry.dtype != MlxDType::BF16 {
            return Err(self.invalid_runtime_error(format!(
                "vector tensor {} expected BF16, got {:?}",
                name, entry.dtype
            )));
        }
        if entry.shape.len() != 1 {
            return Err(self.invalid_runtime_error(format!(
                "vector tensor {} expected rank 1, got {:?}",
                name, entry.shape
            )));
        }
        Ok(self
            .weights
            .read_bf16_tensor_words_cached(name)?
            .iter()
            .copied()
            .map(qwen_bf16_word_to_f32)
            .collect())
    }

    pub fn rms_norm_weighted_f32(
        &self,
        input: &[f32],
        weight_name: &str,
        eps: f32,
    ) -> Result<Vec<f32>> {
        let weights = self.vector_tensor_f32(weight_name)?;
        if input.len() != weights.len() {
            return Err(self.invalid_runtime_error(format!(
                "rms norm input length {} does not match weight {} length {}",
                input.len(),
                weight_name,
                weights.len()
            )));
        }
        Ok(rms_norm_weighted_f32(input, &weights, eps))
    }

    pub fn project_vector_f32(&self, input: &[f32], weight_name: &str) -> Result<Vec<f32>> {
        let input_words = input
            .iter()
            .copied()
            .map(qwen_f32_to_bf16_word)
            .collect::<Vec<_>>();
        self.project_vector_bf16_words(&input_words, weight_name)
    }

    pub fn project_vector_rank3_plane_f32(
        &self,
        input: &[f32],
        weight_name: &str,
        plane: u32,
    ) -> Result<Vec<f32>> {
        let input_words = input
            .iter()
            .copied()
            .map(qwen_f32_to_bf16_word)
            .collect::<Vec<_>>();
        self.project_vector_bf16_words_rank3_plane(&input_words, weight_name, plane)
    }

    pub fn output_logits_f32(&self, hidden: &[f32]) -> Result<Vec<f32>> {
        self.project_vector_f32(hidden, &self.tensors.globals.output)
    }

    pub fn apply_moe_ffn_reference_f32(
        &self,
        layer_index: u32,
        hidden: &[f32],
    ) -> Result<MlxQwen35MoeFfnOutput> {
        if hidden.len() != self.dims.embedding_length as usize {
            return Err(self.invalid_runtime_error(format!(
                "ffn input length {} does not match hidden size {}",
                hidden.len(),
                self.dims.embedding_length
            )));
        }
        let layer = self
            .tensors
            .layers
            .get(layer_index as usize)
            .ok_or_else(|| {
                self.invalid_runtime_error(format!("layer {} out of range", layer_index))
            })?;
        let router_logits = self.project_vector_f32(hidden, &layer.moe.ffn_gate_inp)?;
        let (router_probabilities, routed_experts) =
            softmax_top_k_routes(&router_logits, self.dims.expert_used_count as usize)
                .map_err(|message| self.invalid_runtime_error(message))?;

        let mut routed_output = vec![0.0f32; hidden.len()];
        for route in &routed_experts {
            let (gate, up) = if let Some(merged_name) = &layer.moe.ffn_gate_up_exps {
                let merged =
                    self.project_vector_rank3_plane_f32(hidden, merged_name, route.expert_index)?;
                split_gate_up_projection(&merged).map_err(|message| {
                    self.invalid_runtime_error(format!(
                        "layer {} expert {} merged gate/up split failed: {}",
                        layer_index, route.expert_index, message
                    ))
                })?
            } else {
                let gate_name = layer.moe.ffn_gate_exps.as_deref().ok_or_else(|| {
                    self.invalid_runtime_error(format!(
                        "layer {} is missing expert gate weights",
                        layer_index
                    ))
                })?;
                let up_name = layer.moe.ffn_up_exps.as_deref().ok_or_else(|| {
                    self.invalid_runtime_error(format!(
                        "layer {} is missing expert up weights",
                        layer_index
                    ))
                })?;
                (
                    self.project_vector_rank3_plane_f32(hidden, gate_name, route.expert_index)?,
                    self.project_vector_rank3_plane_f32(hidden, up_name, route.expert_index)?,
                )
            };
            let activated = swiglu_split_f32(&gate, &up).map_err(|message| {
                self.invalid_runtime_error(format!(
                    "layer {} expert {} swiglu failed: {}",
                    layer_index, route.expert_index, message
                ))
            })?;
            let down = self.project_vector_rank3_plane_f32(
                &activated,
                &layer.moe.ffn_down_exps,
                route.expert_index,
            )?;
            if down.len() != routed_output.len() {
                return Err(self.invalid_runtime_error(format!(
                    "layer {} expert {} down projection length {} does not match hidden size {}",
                    layer_index,
                    route.expert_index,
                    down.len(),
                    routed_output.len()
                )));
            }
            for (acc, value) in routed_output.iter_mut().zip(down.iter().copied()) {
                *acc = qwen_bf16_round_to_f32(*acc + qwen_bf16_round_to_f32(value * route.weight));
            }
        }

        let shared_gate = self.project_vector_f32(hidden, &layer.moe.ffn_gate_inp_shexp)?;
        if shared_gate.len() != 1 {
            return Err(self.invalid_runtime_error(format!(
                "layer {} shared expert gate expected 1 output, got {}",
                layer_index,
                shared_gate.len()
            )));
        }
        let shared_gate = sigmoid_f32(shared_gate[0]);
        let shared_gate_proj = self.project_vector_f32(hidden, &layer.moe.ffn_gate_shexp)?;
        let shared_up_proj = self.project_vector_f32(hidden, &layer.moe.ffn_up_shexp)?;
        let shared_activated =
            swiglu_split_f32(&shared_gate_proj, &shared_up_proj).map_err(|message| {
                self.invalid_runtime_error(format!(
                    "layer {} shared expert swiglu failed: {}",
                    layer_index, message
                ))
            })?;
        let mut shared_output =
            self.project_vector_f32(&shared_activated, &layer.moe.ffn_down_shexp)?;
        if shared_output.len() != hidden.len() {
            return Err(self.invalid_runtime_error(format!(
                "layer {} shared expert output length {} does not match hidden size {}",
                layer_index,
                shared_output.len(),
                hidden.len()
            )));
        }
        for value in &mut shared_output {
            *value = qwen_bf16_round_to_f32(*value * shared_gate);
        }

        let output = routed_output
            .iter()
            .copied()
            .zip(shared_output.iter().copied())
            .map(|(routed, shared)| qwen_bf16_round_to_f32(routed + shared))
            .collect::<Vec<_>>();

        Ok(MlxQwen35MoeFfnOutput {
            router_logits,
            router_probabilities,
            routed_experts,
            routed_output,
            shared_gate,
            shared_output,
            output,
        })
    }

    fn invalid_runtime_error(&self, message: impl Into<String>) -> crate::MlxRtError {
        crate::MlxRtError::InvalidModelDir {
            path: self.model_path.clone(),
            message: message.into(),
        }
    }

    fn actual_affine_qparam_names(&self, weight_name: &str) -> Result<(String, String)> {
        let actual_weight_name = self.weights.actual_tensor_name(weight_name)?;
        let (scales_name, biases_name) = actual_affine_qparam_names(actual_weight_name);
        let _ = self.weights.tensor(&scales_name)?;
        let _ = self.weights.tensor(&biases_name)?;
        Ok((scales_name, biases_name))
    }

    fn tensor_f32_flat(&self, name: &str) -> Result<Vec<f32>> {
        let entry = self.weights.tensor(name)?;
        if entry.dtype != MlxDType::BF16 {
            return Err(self.invalid_runtime_error(format!(
                "tensor {} expected BF16, got {:?}",
                name, entry.dtype
            )));
        }
        Ok(self
            .weights
            .read_bf16_tensor_words_cached(name)?
            .iter()
            .copied()
            .map(qwen_bf16_word_to_f32)
            .collect())
    }

    fn conv1d_kernel_f32(
        &self,
        name: &str,
        kernel_size: usize,
        channels: usize,
    ) -> Result<Vec<f32>> {
        let entry = self.weights.tensor(name)?;
        let flat = self.tensor_f32_flat(name)?;
        let expected = kernel_size
            .checked_mul(channels)
            .ok_or_else(|| self.invalid_runtime_error("conv1d kernel size overflow"))?;
        if flat.len() != expected {
            return Err(self.invalid_runtime_error(format!(
                "conv1d tensor {} flattened length {} does not match {}x{}",
                name,
                flat.len(),
                kernel_size,
                channels
            )));
        }
        match entry.shape.as_slice() {
            [shape0, shape1] if *shape0 as usize == kernel_size && *shape1 as usize == channels => {
                let mut out = vec![0.0f32; flat.len()];
                for channel in 0..channels {
                    for tap in 0..kernel_size {
                        out[channel * kernel_size + tap] = flat[tap * channels + channel];
                    }
                }
                Ok(out)
            }
            [shape0, shape1] if *shape0 as usize == channels && *shape1 as usize == kernel_size => {
                Ok(flat)
            }
            [shape0, shape1, shape2]
                if *shape0 as usize == channels
                    && *shape1 as usize == kernel_size
                    && *shape2 == 1 =>
            {
                Ok(flat)
            }
            [shape0, shape1, shape2]
                if *shape0 as usize == channels
                    && *shape1 == 1
                    && *shape2 as usize == kernel_size =>
            {
                let mut out = vec![0.0f32; flat.len()];
                for channel in 0..channels {
                    let src = &flat[channel * kernel_size..(channel + 1) * kernel_size];
                    out[channel * kernel_size..(channel + 1) * kernel_size].copy_from_slice(src);
                }
                Ok(out)
            }
            other => Err(self.invalid_runtime_error(format!(
                "unsupported conv1d tensor {} shape {:?} for {}x{} kernel",
                name, other, kernel_size, channels
            ))),
        }
    }

    fn read_rank2_row_f32(&self, weight_name: &str, row: u64) -> Result<Vec<f32>> {
        let entry = self.weights.tensor(weight_name)?;
        match entry.dtype {
            MlxDType::BF16 => {
                let cols = *entry.shape.get(1).ok_or_else(|| {
                    self.invalid_runtime_error(format!(
                        "tensor {} is missing rank-2 column dimension",
                        weight_name
                    ))
                })? as usize;
                let start = (row as usize)
                    .checked_mul(cols)
                    .ok_or_else(|| self.invalid_runtime_error("rank-2 row offset overflow"))?;
                let end = start
                    .checked_add(cols)
                    .ok_or_else(|| self.invalid_runtime_error("rank-2 row end overflow"))?;
                let words = self.weights.read_bf16_tensor_words_cached(weight_name)?;
                Ok(words[start..end]
                    .iter()
                    .copied()
                    .map(qwen_bf16_word_to_f32)
                    .collect())
            }
            MlxDType::U32 => {
                let quantization = self
                    .weights
                    .quantization_for_tensor(weight_name)?
                    .ok_or_else(|| {
                        self.invalid_runtime_error(format!(
                            "tensor {} is quantized but has no quantization config",
                            weight_name
                        ))
                    })?;
                if quantization.mode != "affine" {
                    return Err(self.invalid_runtime_error(format!(
                        "tensor {} uses unsupported quantization mode {}",
                        weight_name, quantization.mode
                    )));
                }
                let values_per_word = 32 / quantization.bits as u64;
                let (actual_scales_name, actual_biases_name) =
                    self.actual_affine_qparam_names(weight_name)?;
                let packed_entry = self.weights.tensor(weight_name)?;
                let qparam_entry = self.weights.tensor(&actual_scales_name)?;
                let packed_cols = *packed_entry.shape.get(1).ok_or_else(|| {
                    self.invalid_runtime_error(format!(
                        "tensor {} is missing rank-2 packed column dimension",
                        weight_name
                    ))
                })? as usize;
                let qparam_cols = *qparam_entry.shape.get(1).ok_or_else(|| {
                    self.invalid_runtime_error(format!(
                        "tensor {} is missing rank-2 qparam column dimension",
                        actual_scales_name
                    ))
                })? as usize;
                let packed_start = (row as usize)
                    .checked_mul(packed_cols)
                    .ok_or_else(|| self.invalid_runtime_error("packed row offset overflow"))?;
                let packed_end = packed_start
                    .checked_add(packed_cols)
                    .ok_or_else(|| self.invalid_runtime_error("packed row end overflow"))?;
                let qparam_start = (row as usize)
                    .checked_mul(qparam_cols)
                    .ok_or_else(|| self.invalid_runtime_error("qparam row offset overflow"))?;
                let qparam_end = qparam_start
                    .checked_add(qparam_cols)
                    .ok_or_else(|| self.invalid_runtime_error("qparam row end overflow"))?;
                let packed_words = self.weights.read_u32_tensor_words_cached(weight_name)?;
                let scale_words = self.weights.read_bf16_tensor_words_cached(&actual_scales_name)?;
                let bias_words = self.weights.read_bf16_tensor_words_cached(&actual_biases_name)?;
                let packed = &packed_words[packed_start..packed_end];
                let scales = &scale_words[qparam_start..qparam_end];
                let biases = &bias_words[qparam_start..qparam_end];
                affine_dequantize_row_f32(
                    packed,
                    scales,
                    biases,
                    quantization.group_size as u64,
                    quantization.bits,
                    values_per_word,
                )
                .map_err(|message| self.invalid_runtime_error(message))
            }
            other => Err(self.invalid_runtime_error(format!(
                "tensor {} expected BF16 or U32, got {:?}",
                weight_name, other
            ))),
        }
    }

    fn project_vector_bf16_words(
        &self,
        input_words: &[u16],
        weight_name: &str,
    ) -> Result<Vec<f32>> {
        let weight_entry = self.weights.tensor(weight_name)?;
        match weight_entry.dtype {
            MlxDType::BF16 => dense_bf16_matmul_t_f32(&self.weights, input_words, weight_name)
                .map_err(|message| self.invalid_runtime_error(message)),
            MlxDType::U32 => affine_quantized_matmul_t_f32(
                &self.weights,
                input_words,
                weight_name,
                self.weights
                    .quantization_for_tensor(weight_name)?
                    .ok_or_else(|| {
                        self.invalid_runtime_error(format!(
                            "tensor {} is quantized but has no quantization config",
                            weight_name
                        ))
                    })?,
            )
            .map_err(|message| self.invalid_runtime_error(message)),
            other => Err(self.invalid_runtime_error(format!(
                "tensor {} expected BF16 or U32, got {:?}",
                weight_name, other
            ))),
        }
    }

    fn project_vector_bf16_words_rank3_plane(
        &self,
        input_words: &[u16],
        weight_name: &str,
        plane: u32,
    ) -> Result<Vec<f32>> {
        let weight_entry = self.weights.tensor(weight_name)?;
        match weight_entry.dtype {
            MlxDType::BF16 => dense_bf16_matmul_t_f32_rank3_plane(
                &self.weights,
                input_words,
                weight_name,
                plane as u64,
            )
            .map_err(|message| self.invalid_runtime_error(message)),
            MlxDType::U32 => affine_quantized_matmul_t_f32_rank3_plane(
                &self.weights,
                input_words,
                weight_name,
                plane,
                self.weights
                    .quantization_for_tensor(weight_name)?
                    .ok_or_else(|| {
                        self.invalid_runtime_error(format!(
                            "tensor {} is quantized but has no quantization config",
                            weight_name
                        ))
                    })?,
            )
            .map_err(|message| self.invalid_runtime_error(message)),
            other => Err(self.invalid_runtime_error(format!(
                "rank-3 tensor {} expected BF16 or U32, got {:?}",
                weight_name, other
            ))),
        }
    }
}

impl MlxQwen35MoeHybridCacheTemplate {
    pub fn from_runtime_parts(
        tensors: &MlxQwen35MoeTensors,
        dims: &MlxQwen35MoeDims,
    ) -> Result<Self> {
        let attention_layers = tensors
            .layers
            .iter()
            .filter(|layer| layer.kind == MlxQwen35MoeLayerKind::Attention)
            .map(|layer| layer.index)
            .collect::<Vec<_>>();
        let recurrent_layers = tensors
            .layers
            .iter()
            .filter(|layer| layer.kind == MlxQwen35MoeLayerKind::Recurrent)
            .map(|layer| layer.index)
            .collect::<Vec<_>>();
        Ok(Self {
            attention_layers,
            recurrent_layers,
            attention_k_width: dims.attention_k_width(),
            attention_v_width: dims.attention_v_width(),
            recurrent_r_width: dims.recurrent_conv_width()?,
            recurrent_s_width: dims.recurrent_state_width()?,
        })
    }
}

fn qwen_model_root_dir(model_path: &Path) -> Result<PathBuf> {
    if model_path.is_dir() {
        return Ok(model_path.to_path_buf());
    }
    model_path
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| crate::MlxRtError::InvalidModelDir {
            path: model_path.to_path_buf(),
            message: format!(
                "model path {} has no parent directory",
                model_path.display()
            ),
        })
}

fn qwen35moe_inventory(
    weights: &MlxQwen35MoeIndexedSafetensors,
    tensors: &MlxQwen35MoeTensors,
) -> Result<MlxModelTensorInventory> {
    let mut globals = BTreeMap::new();
    insert_tensor(
        &mut globals,
        weights,
        "token_embd",
        &tensors.globals.token_embd,
    )?;
    insert_tensor(
        &mut globals,
        weights,
        "output_norm",
        &tensors.globals.output_norm,
    )?;
    insert_tensor(&mut globals, weights, "output", &tensors.globals.output)?;

    let mut layers = Vec::with_capacity(tensors.layers.len());
    for layer in &tensors.layers {
        let mut entries = BTreeMap::new();
        insert_tensor(&mut entries, weights, "attn_norm", &layer.attn_norm)?;
        insert_tensor(
            &mut entries,
            weights,
            "post_attention_norm",
            &layer.post_attention_norm,
        )?;

        if let Some(attention) = &layer.attention {
            insert_tensor(&mut entries, weights, "attn_q", &attention.wq)?;
            insert_tensor(&mut entries, weights, "attn_k", &attention.wk)?;
            insert_tensor(&mut entries, weights, "attn_v", &attention.wv)?;
            insert_tensor(&mut entries, weights, "attn_output", &attention.wo)?;
            insert_tensor(&mut entries, weights, "attn_q_norm", &attention.attn_q_norm)?;
            insert_tensor(&mut entries, weights, "attn_k_norm", &attention.attn_k_norm)?;
        }

        if let Some(recurrent) = &layer.recurrent {
            insert_tensor(&mut entries, weights, "attn_qkv", &recurrent.wqkv)?;
            insert_tensor(&mut entries, weights, "attn_gate", &recurrent.wqkv_gate)?;
            insert_tensor(&mut entries, weights, "ssm_conv1d", &recurrent.ssm_conv1d)?;
            insert_tensor(&mut entries, weights, "ssm_dt", &recurrent.ssm_dt)?;
            insert_tensor(&mut entries, weights, "ssm_a", &recurrent.ssm_a)?;
            insert_tensor(&mut entries, weights, "ssm_beta", &recurrent.ssm_beta)?;
            insert_tensor(&mut entries, weights, "ssm_alpha", &recurrent.ssm_alpha)?;
            insert_tensor(&mut entries, weights, "ssm_norm", &recurrent.ssm_norm)?;
            insert_tensor(&mut entries, weights, "ssm_out", &recurrent.ssm_out)?;
        }

        insert_tensor(
            &mut entries,
            weights,
            "ffn_gate_inp",
            &layer.moe.ffn_gate_inp,
        )?;
        insert_optional_tensor(
            &mut entries,
            weights,
            "ffn_gate_up_exps",
            &layer.moe.ffn_gate_up_exps,
        )?;
        insert_optional_tensor(
            &mut entries,
            weights,
            "ffn_gate_exps",
            &layer.moe.ffn_gate_exps,
        )?;
        insert_optional_tensor(&mut entries, weights, "ffn_up_exps", &layer.moe.ffn_up_exps)?;
        insert_tensor(
            &mut entries,
            weights,
            "ffn_down_exps",
            &layer.moe.ffn_down_exps,
        )?;
        insert_tensor(
            &mut entries,
            weights,
            "ffn_gate_inp_shexp",
            &layer.moe.ffn_gate_inp_shexp,
        )?;
        insert_tensor(
            &mut entries,
            weights,
            "ffn_gate_shexp",
            &layer.moe.ffn_gate_shexp,
        )?;
        insert_tensor(
            &mut entries,
            weights,
            "ffn_up_shexp",
            &layer.moe.ffn_up_shexp,
        )?;
        insert_tensor(
            &mut entries,
            weights,
            "ffn_down_shexp",
            &layer.moe.ffn_down_shexp,
        )?;

        layers.push(MlxModelLayerInventory {
            index: layer.index,
            role: qwen35moe_layer_role(layer.kind),
            tensors: entries,
        });
    }

    Ok(MlxModelTensorInventory { globals, layers })
}

fn qwen35moe_layer_role(kind: MlxQwen35MoeLayerKind) -> MlxModelLayerRole {
    match kind {
        MlxQwen35MoeLayerKind::Attention => MlxModelLayerRole::Attention,
        MlxQwen35MoeLayerKind::Recurrent => MlxModelLayerRole::Recurrent,
    }
}

fn insert_tensor(
    tensors: &mut BTreeMap<String, MlxTensorInfo>,
    weights: &MlxQwen35MoeIndexedSafetensors,
    key: &str,
    canonical_name: &str,
) -> Result<()> {
    tensors.insert(
        key.to_owned(),
        MlxTensorInfo::from_indexed(weights, canonical_name)?,
    );
    Ok(())
}

fn insert_optional_tensor(
    tensors: &mut BTreeMap<String, MlxTensorInfo>,
    weights: &MlxQwen35MoeIndexedSafetensors,
    key: &str,
    canonical_name: &Option<String>,
) -> Result<()> {
    if let Some(canonical_name) = canonical_name {
        insert_tensor(tensors, weights, key, canonical_name)?;
    }
    Ok(())
}

fn actual_affine_qparam_names(actual_weight_name: &str) -> (String, String) {
    if let Some(stem) = actual_weight_name.strip_suffix(".weight") {
        (format!("{stem}.scales"), format!("{stem}.biases"))
    } else {
        (
            format!("{actual_weight_name}.scales"),
            format!("{actual_weight_name}.biases"),
        )
    }
}

fn dense_bf16_matmul_t_f32(
    weights: &MlxQwen35MoeIndexedSafetensors,
    input_words: &[u16],
    weight_name: &str,
) -> std::result::Result<Vec<f32>, String> {
    let weight_entry = weights.tensor(weight_name).map_err(|err| err.to_string())?;
    if weight_entry.dtype != MlxDType::BF16 {
        return Err(format!(
            "tensor {} expected BF16, got {:?}",
            weight_name, weight_entry.dtype
        ));
    }
    if weight_entry.shape.len() != 2 {
        return Err(format!(
            "dense bf16 matmul expects rank-2 tensor {}, got {:?}",
            weight_name, weight_entry.shape
        ));
    }
    let rows = weight_entry.shape[0] as usize;
    let inner_dim = weight_entry.shape[1] as usize;
    if input_words.len() != inner_dim {
        return Err(format!(
            "activation length mismatch for {}: got {} expected {}",
            weight_name,
            input_words.len(),
            inner_dim
        ));
    }
    let weight_words = weights
        .read_bf16_tensor_words_cached(weight_name)
        .map_err(|err| err.to_string())?;
    let x = input_words
        .iter()
        .copied()
        .map(qwen_bf16_word_to_f32)
        .collect::<Vec<_>>();
    if let Some(out) = try_matmul_nt_ggml_bytes(
        &x,
        bf16_words_as_bytes(weight_words.as_slice()),
        GGML_TYPE_BF16,
        1,
        inner_dim,
        rows,
    ) {
        return Ok(out);
    }
    let mut out = Vec::with_capacity(rows);
    for row_idx in 0..rows {
        let row_start = row_idx * inner_dim;
        let row_end = row_start + inner_dim;
        let mut sum = 0.0f32;
        for (weight_word, x_value) in weight_words[row_start..row_end].iter().zip(x.iter()) {
            let product = qwen_bf16_round_to_f32(qwen_bf16_word_to_f32(*weight_word) * *x_value);
            sum = qwen_bf16_round_to_f32(sum + product);
        }
        out.push(sum);
    }
    Ok(out)
}

fn dense_bf16_matmul_t_f32_rank3_plane(
    weights: &MlxQwen35MoeIndexedSafetensors,
    input_words: &[u16],
    weight_name: &str,
    plane: u64,
) -> std::result::Result<Vec<f32>, String> {
    let weight_entry = weights.tensor(weight_name).map_err(|err| err.to_string())?;
    if weight_entry.dtype != MlxDType::BF16 {
        return Err(format!(
            "tensor {} expected BF16, got {:?}",
            weight_name, weight_entry.dtype
        ));
    }
    if weight_entry.shape.len() != 3 {
        return Err(format!(
            "dense rank-3 bf16 matmul expects tensor {}, got {:?}",
            weight_name, weight_entry.shape
        ));
    }
    if plane >= weight_entry.shape[0] {
        return Err(format!(
            "plane {} out of range for {} with {} planes",
            plane, weight_name, weight_entry.shape[0]
        ));
    }
    let rows = weight_entry.shape[1] as usize;
    let inner_dim = weight_entry.shape[2] as usize;
    if input_words.len() != inner_dim {
        return Err(format!(
            "activation length mismatch for {} plane {}: got {} expected {}",
            weight_name,
            plane,
            input_words.len(),
            inner_dim
        ));
    }
    let actual_weight_name = weights
        .actual_tensor_name(weight_name)
        .map_err(|err| err.to_string())?;
    let header = weights
        .header_for_tensor(weight_name)
        .map_err(|err| err.to_string())?;
    let weight_words = header
        .read_rank3_plane_bf16_words(actual_weight_name, plane)
        .map_err(|err| err.to_string())?;
    let x = input_words
        .iter()
        .copied()
        .map(qwen_bf16_word_to_f32)
        .collect::<Vec<_>>();
    if let Some(out) = try_matmul_nt_ggml_bytes(
        &x,
        bf16_words_as_bytes(weight_words.as_slice()),
        GGML_TYPE_BF16,
        1,
        inner_dim,
        rows,
    ) {
        return Ok(out);
    }
    let mut out = Vec::with_capacity(rows);
    for row_idx in 0..rows {
        let row_start = row_idx * inner_dim;
        let row_end = row_start + inner_dim;
        let mut sum = 0.0f32;
        for (weight_word, x_value) in weight_words[row_start..row_end].iter().zip(x.iter()) {
            let product = qwen_bf16_round_to_f32(qwen_bf16_word_to_f32(*weight_word) * *x_value);
            sum = qwen_bf16_round_to_f32(sum + product);
        }
        out.push(sum);
    }
    Ok(out)
}

fn affine_quantized_matmul_t_f32(
    weights: &MlxQwen35MoeIndexedSafetensors,
    input_words: &[u16],
    weight_name: &str,
    quantization: &crate::MlxQuantizationConfig,
) -> std::result::Result<Vec<f32>, String> {
    if quantization.mode != "affine" {
        return Err(format!(
            "tensor {} uses unsupported quantization mode {}",
            weight_name, quantization.mode
        ));
    }
    if quantization.bits == 0
        || quantization.bits > 8
        || (quantization.bits & (quantization.bits - 1)) != 0
    {
        return Err(format!(
            "tensor {} uses unsupported affine quantization bits {}",
            weight_name, quantization.bits
        ));
    }
    let weight_entry = weights.tensor(weight_name).map_err(|err| err.to_string())?;
    if weight_entry.dtype != MlxDType::U32 {
        return Err(format!(
            "tensor {} expected U32, got {:?}",
            weight_name, weight_entry.dtype
        ));
    }
    if weight_entry.shape.len() != 2 {
        return Err(format!(
            "quantized matmul expects rank-2 tensor {}, got {:?}",
            weight_name, weight_entry.shape
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
            "quantized qparams for {} expected BF16, got {:?} / {:?}",
            weight_name, scales_entry.dtype, biases_entry.dtype
        ));
    }
    if scales_entry.shape.len() != 2 || biases_entry.shape.len() != 2 {
        return Err(format!(
            "quantized qparams for {} expected rank-2 tensors, got {:?} / {:?}",
            weight_name, scales_entry.shape, biases_entry.shape
        ));
    }
    if scales_entry.shape != biases_entry.shape {
        return Err(format!(
            "scale/bias shape mismatch for {}: {:?} vs {:?}",
            weight_name, scales_entry.shape, biases_entry.shape
        ));
    }
    let values_per_word = 32 / quantization.bits as u64;
    let inner_dim = weight_entry.shape[1] * values_per_word;
    if input_words.len() as u64 != inner_dim {
        return Err(format!(
            "activation length mismatch for {}: got {} expected {}",
            weight_name,
            input_words.len(),
            inner_dim
        ));
    }
    if inner_dim != scales_entry.shape[1] * quantization.group_size as u64 {
        return Err(format!(
            "packed/qparam shape mismatch for {} with group_size={} bits={}",
            weight_name, quantization.group_size, quantization.bits
        ));
    }
    let root = weights.snapshot.paths().root_dir.to_string_lossy();
    let weight_key = format!("{root}:{actual_weight_name}");
    let scales_key = format!("{root}:{actual_scales_name}");
    let biases_key = format!("{root}:{actual_biases_name}");
    if let Some(result) = try_affine_quantized_matmul_bf16(
        AffineQuantizedMatmulSpec {
            input_bf16_words: input_words,
            out_rows: weight_entry.shape[0] as usize,
            weight_words_per_row: weight_entry.shape[1] as usize,
            qparams_per_row: scales_entry.shape[1] as usize,
            bits: quantization.bits,
            group_size: quantization.group_size as u64,
            cache_namespace: root.as_ref(),
        },
        &weight_key,
        &scales_key,
        &biases_key,
        || {
            weights
                .read_tensor_bytes(actual_weight_name)
                .map_err(|err| err.to_string())
        },
        || {
            weights
                .read_tensor_bytes(&actual_scales_name)
                .map_err(|err| err.to_string())
        },
        || {
            weights
                .read_tensor_bytes(&actual_biases_name)
                .map_err(|err| err.to_string())
        },
    ) {
        return result;
    }

    let packed_weights = weights
        .read_u32_tensor_words_cached(weight_name)
        .map_err(|err| err.to_string())?;
    let scales = weights
        .read_bf16_tensor_words_cached(&actual_scales_name)
        .map_err(|err| err.to_string())?;
    let biases = weights
        .read_bf16_tensor_words_cached(&actual_biases_name)
        .map_err(|err| err.to_string())?;
    affine_quantized_matmul_fallback(
        input_words,
        packed_weights.as_slice(),
        scales.as_slice(),
        biases.as_slice(),
        weight_entry.shape[0] as usize,
        weight_entry.shape[1] as usize,
        scales_entry.shape[1] as usize,
        quantization.group_size as u64,
        quantization.bits,
    )
}

fn affine_quantized_matmul_t_f32_rank3_plane(
    weights: &MlxQwen35MoeIndexedSafetensors,
    input_words: &[u16],
    weight_name: &str,
    plane: u32,
    quantization: &crate::MlxQuantizationConfig,
) -> std::result::Result<Vec<f32>, String> {
    if quantization.mode != "affine" {
        return Err(format!(
            "tensor {} uses unsupported quantization mode {}",
            weight_name, quantization.mode
        ));
    }
    if quantization.bits == 0
        || quantization.bits > 8
        || (quantization.bits & (quantization.bits - 1)) != 0
    {
        return Err(format!(
            "tensor {} uses unsupported affine quantization bits {}",
            weight_name, quantization.bits
        ));
    }
    let weight_entry = weights.tensor(weight_name).map_err(|err| err.to_string())?;
    if weight_entry.dtype != MlxDType::U32 {
        return Err(format!(
            "tensor {} expected U32, got {:?}",
            weight_name, weight_entry.dtype
        ));
    }
    if weight_entry.shape.len() != 3 {
        return Err(format!(
            "rank-3 quantized matmul expects tensor {}, got {:?}",
            weight_name, weight_entry.shape
        ));
    }
    if plane as u64 >= weight_entry.shape[0] {
        return Err(format!(
            "plane {} out of range for {} with {} planes",
            plane, weight_name, weight_entry.shape[0]
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
            "quantized rank-3 qparams for {} expected BF16, got {:?} / {:?}",
            weight_name, scales_entry.dtype, biases_entry.dtype
        ));
    }
    if scales_entry.shape.len() != 3 || biases_entry.shape.len() != 3 {
        return Err(format!(
            "quantized rank-3 qparams for {} expected rank-3 tensors, got {:?} / {:?}",
            weight_name, scales_entry.shape, biases_entry.shape
        ));
    }
    if scales_entry.shape != biases_entry.shape {
        return Err(format!(
            "rank-3 scale/bias shape mismatch for {}: {:?} vs {:?}",
            weight_name, scales_entry.shape, biases_entry.shape
        ));
    }
    let values_per_word = 32 / quantization.bits as u64;
    let inner_dim = weight_entry.shape[2] * values_per_word;
    if input_words.len() as u64 != inner_dim {
        return Err(format!(
            "activation length mismatch for {} plane {}: got {} expected {}",
            weight_name,
            plane,
            input_words.len(),
            inner_dim
        ));
    }
    if inner_dim != scales_entry.shape[2] * quantization.group_size as u64 {
        return Err(format!(
            "rank-3 packed/qparam shape mismatch for {} with group_size={} bits={}",
            weight_name, quantization.group_size, quantization.bits
        ));
    }
    let root = weights.snapshot.paths().root_dir.to_string_lossy();
    let weight_key = format!("{root}:{actual_weight_name}@{plane}");
    let scales_key = format!("{root}:{actual_scales_name}@{plane}");
    let biases_key = format!("{root}:{actual_biases_name}@{plane}");
    if let Some(result) = try_affine_quantized_matmul_bf16(
        AffineQuantizedMatmulSpec {
            input_bf16_words: input_words,
            out_rows: weight_entry.shape[1] as usize,
            weight_words_per_row: weight_entry.shape[2] as usize,
            qparams_per_row: scales_entry.shape[2] as usize,
            bits: quantization.bits,
            group_size: quantization.group_size as u64,
            cache_namespace: root.as_ref(),
        },
        &weight_key,
        &scales_key,
        &biases_key,
        || {
            let header = weights
                .header_for_tensor(weight_name)
                .map_err(|err| err.to_string())?;
            let words = header
                .read_rank3_plane_u32_words(actual_weight_name, plane as u64)
                .map_err(|err| err.to_string())?;
            Ok(words
                .iter()
                .flat_map(|word| word.to_le_bytes())
                .collect::<Vec<_>>())
        },
        || {
            let header = weights
                .header_for_tensor(&actual_scales_name)
                .map_err(|err| err.to_string())?;
            let words = header
                .read_rank3_plane_bf16_words(&actual_scales_name, plane as u64)
                .map_err(|err| err.to_string())?;
            Ok(bf16_words_as_bytes(&words).to_vec())
        },
        || {
            let header = weights
                .header_for_tensor(&actual_biases_name)
                .map_err(|err| err.to_string())?;
            let words = header
                .read_rank3_plane_bf16_words(&actual_biases_name, plane as u64)
                .map_err(|err| err.to_string())?;
            Ok(bf16_words_as_bytes(&words).to_vec())
        },
    ) {
        return result;
    }

    let actual_weight_name = weights
        .actual_tensor_name(weight_name)
        .map_err(|err| err.to_string())?;
    let weight_header = weights
        .header_for_tensor(weight_name)
        .map_err(|err| err.to_string())?;
    let scales_header = weights
        .header_for_tensor(&actual_scales_name)
        .map_err(|err| err.to_string())?;
    let biases_header = weights
        .header_for_tensor(&actual_biases_name)
        .map_err(|err| err.to_string())?;
    let packed_weights = weight_header
        .read_rank3_plane_u32_words(actual_weight_name, plane as u64)
        .map_err(|err| err.to_string())?;
    let scales = scales_header
        .read_rank3_plane_bf16_words(&actual_scales_name, plane as u64)
        .map_err(|err| err.to_string())?;
    let biases = biases_header
        .read_rank3_plane_bf16_words(&actual_biases_name, plane as u64)
        .map_err(|err| err.to_string())?;
    affine_quantized_matmul_fallback(
        input_words,
        &packed_weights,
        &scales,
        &biases,
        weight_entry.shape[1] as usize,
        weight_entry.shape[2] as usize,
        scales_entry.shape[2] as usize,
        quantization.group_size as u64,
        quantization.bits,
    )
}

fn affine_dequantize_row_f32(
    packed_weights: &[u32],
    scales: &[u16],
    biases: &[u16],
    group_size: u64,
    bits: u32,
    values_per_word: u64,
) -> std::result::Result<Vec<f32>, String> {
    if scales.len() != biases.len() {
        return Err(format!(
            "row scale/bias length mismatch: {} vs {}",
            scales.len(),
            biases.len()
        ));
    }
    let out_size = packed_weights.len() as u64 * values_per_word;
    if out_size != scales.len() as u64 * group_size {
        return Err(format!(
            "row packed/scales shape mismatch for group_size={} bits={}",
            group_size, bits
        ));
    }
    let words_per_group = group_size / values_per_word;
    if words_per_group == 0 || packed_weights.len() as u64 != scales.len() as u64 * words_per_group
    {
        return Err(format!("invalid words_per_group {}", words_per_group));
    }
    let mask = (1u32 << bits) - 1;
    let mut out = Vec::with_capacity(out_size as usize);
    for group_idx in 0..scales.len() {
        let scale = qwen_bf16_word_to_f32(scales[group_idx]);
        let bias = qwen_bf16_word_to_f32(biases[group_idx]);
        let group_start = group_idx * words_per_group as usize;
        let group_end = group_start + words_per_group as usize;
        for packed in &packed_weights[group_start..group_end] {
            let mut packed_word = *packed;
            for _ in 0..values_per_word as usize {
                let q = (packed_word & mask) as f32;
                out.push(qwen_bf16_round_to_f32(
                    qwen_bf16_round_to_f32(q * scale) + bias,
                ));
                if bits != 8 {
                    packed_word >>= bits;
                }
            }
        }
    }
    Ok(out)
}

fn affine_quantized_matmul_fallback(
    input_words: &[u16],
    packed_weights: &[u32],
    scales: &[u16],
    biases: &[u16],
    rows: usize,
    weight_words_per_row: usize,
    qparams_per_row: usize,
    group_size: u64,
    bits: u32,
) -> std::result::Result<Vec<f32>, String> {
    if scales.len() != biases.len() {
        return Err(format!(
            "scale/bias length mismatch: {} vs {}",
            scales.len(),
            biases.len()
        ));
    }
    let values_per_word = 32 / bits as u64;
    let words_per_group = group_size / values_per_word;
    if words_per_group == 0
        || weight_words_per_row as u64 != qparams_per_row as u64 * words_per_group
    {
        return Err(format!("invalid words_per_group {}", words_per_group));
    }
    let x = input_words
        .iter()
        .copied()
        .map(qwen_bf16_word_to_f32)
        .collect::<Vec<_>>();
    let pack_factor = values_per_word as usize;
    let mask = (1u32 << bits) - 1;
    let mut out = Vec::with_capacity(rows);
    for row in 0..rows {
        let weight_row_start = row * weight_words_per_row;
        let qparam_row_start = row * qparams_per_row;
        let mut total = 0.0f32;
        let mut x_index = 0usize;
        for group in 0..qparams_per_row {
            let scale = qwen_bf16_word_to_f32(scales[qparam_row_start + group]);
            let bias = qwen_bf16_word_to_f32(biases[qparam_row_start + group]);
            let group_start = weight_row_start + group * words_per_group as usize;
            let group_end = group_start + words_per_group as usize;
            let mut group_sum = 0.0f32;
            let mut group_accum = 0.0f32;
            for packed in &packed_weights[group_start..group_end] {
                let mut packed_word = *packed;
                for _ in 0..pack_factor {
                    let q = (packed_word & mask) as f32;
                    let xi = x[x_index];
                    group_sum += xi;
                    group_accum += xi * q;
                    x_index += 1;
                    if bits != 8 {
                        packed_word >>= bits;
                    }
                }
            }
            total += qwen_bf16_round_to_f32(scale * group_accum)
                + qwen_bf16_round_to_f32(bias * group_sum);
        }
        out.push(qwen_bf16_round_to_f32(total));
    }
    Ok(out)
}

fn build_qwen_generation_metrics(
    elapsed: Duration,
    prompt_token_count: usize,
    generated_token_count: usize,
    time_to_first_token_elapsed: Duration,
) -> MlxQwen35MoeGenerationMetrics {
    let steady_state_generated_tokens = generated_token_count.saturating_sub(1);
    let steady_state_elapsed = elapsed.saturating_sub(time_to_first_token_elapsed);
    MlxQwen35MoeGenerationMetrics {
        elapsed,
        time_to_first_token_elapsed,
        steady_state_elapsed,
        steady_state_generated_tokens,
        prompt_prefill_tokens_per_second: tokens_per_second(prompt_token_count, time_to_first_token_elapsed),
        steady_state_decode_tokens_per_second: tokens_per_second(
            steady_state_generated_tokens,
            steady_state_elapsed,
        ),
        decode_tokens_per_second: tokens_per_second(generated_token_count, elapsed),
    }
}

fn tokens_per_second(token_count: usize, elapsed: Duration) -> f64 {
    let seconds = elapsed.as_secs_f64();
    if token_count == 0 || seconds <= 0.0 {
        0.0
    } else {
        token_count as f64 / seconds
    }
}

#[derive(Clone, Copy, Debug)]
struct QwenSamplingOptions {
    do_sample: bool,
    temperature: f32,
    top_k: u32,
    top_p: f32,
}

#[derive(Clone, Debug)]
struct SampledTokenCandidate {
    token_id: u32,
    logit: f32,
    scaled_logit: f32,
    prob: f64,
}

#[derive(Clone, Debug)]
struct QwenSamplingRng {
    state: [u64; 312],
    index: usize,
}

impl QwenSamplingRng {
    fn new(seed: u64) -> Self {
        let mut state = [0u64; 312];
        state[0] = seed;
        for i in 1..state.len() {
            state[i] = 6_364_136_223_846_793_005u64
                .wrapping_mul(state[i - 1] ^ (state[i - 1] >> 62))
                .wrapping_add(i as u64);
        }
        Self {
            state,
            index: state.len(),
        }
    }

    fn twist(&mut self) {
        const NN: usize = 312;
        const MM: usize = 156;
        const MATRIX_A: u64 = 0xB502_6F5A_A966_19E9;
        const UM: u64 = 0xFFFF_FFFF_8000_0000;
        const LM: u64 = 0x7FFF_FFFF;

        for i in 0..(NN - MM) {
            let x = (self.state[i] & UM) | (self.state[i + 1] & LM);
            self.state[i] = self.state[i + MM] ^ (x >> 1) ^ if x & 1 != 0 { MATRIX_A } else { 0 };
        }
        for i in (NN - MM)..(NN - 1) {
            let x = (self.state[i] & UM) | (self.state[i + 1] & LM);
            self.state[i] =
                self.state[i + MM - NN] ^ (x >> 1) ^ if x & 1 != 0 { MATRIX_A } else { 0 };
        }
        let x = (self.state[NN - 1] & UM) | (self.state[0] & LM);
        self.state[NN - 1] = self.state[MM - 1] ^ (x >> 1) ^ if x & 1 != 0 { MATRIX_A } else { 0 };
        self.index = 0;
    }

    fn next_u64(&mut self) -> u64 {
        if self.index >= self.state.len() {
            self.twist();
        }
        let mut x = self.state[self.index];
        self.index += 1;
        x ^= (x >> 29) & 0x5555_5555_5555_5555;
        x ^= (x << 17) & 0x71D6_7FFF_EDA6_0000;
        x ^= (x << 37) & 0xFFF7_EEE0_0000_0000;
        x ^= x >> 43;
        x
    }

    fn next_f64(&mut self) -> f64 {
        ((self.next_u64() >> 11) as f64) * (1.0 / ((1u64 << 53) as f64))
    }
}

fn token_is_disallowed(disallowed_token_ids: &[u32], token_id: u32) -> bool {
    disallowed_token_ids.binary_search(&token_id).is_ok()
}

fn select_top1_token_from_logits(
    logits: &[f32],
    disallowed_token_ids: &[u32],
) -> std::result::Result<MlxGreedyToken, String> {
    let mut best: Option<MlxGreedyToken> = None;
    for (token_idx, &logit) in logits.iter().enumerate() {
        let token_id = token_idx as u32;
        if token_is_disallowed(disallowed_token_ids, token_id) {
            continue;
        }
        let better = match best {
            None => true,
            Some(current) => {
                logit > current.logit || (logit == current.logit && token_id < current.token_id)
            }
        };
        if better {
            best = Some(MlxGreedyToken { token_id, logit });
        }
    }
    best.ok_or_else(|| "no selectable token remained after suppression".to_string())
}

fn push_sampled_candidate_top_k(
    candidates: &mut Vec<SampledTokenCandidate>,
    candidate: SampledTokenCandidate,
    top_k: usize,
) {
    if top_k == 0 {
        candidates.push(candidate);
        return;
    }
    if candidates.len() < top_k {
        candidates.push(candidate);
        return;
    }
    let mut worst_index = 0usize;
    for candidate_index in 1..candidates.len() {
        let current = &candidates[candidate_index];
        let worst = &candidates[worst_index];
        let current_is_worse = current.scaled_logit < worst.scaled_logit
            || (current.scaled_logit == worst.scaled_logit && current.token_id > worst.token_id);
        if current_is_worse {
            worst_index = candidate_index;
        }
    }
    let worst = &candidates[worst_index];
    let candidate_is_better = candidate.scaled_logit > worst.scaled_logit
        || (candidate.scaled_logit == worst.scaled_logit && candidate.token_id < worst.token_id);
    if candidate_is_better {
        candidates[worst_index] = candidate;
    }
}

fn finalize_sampled_candidates(
    mut candidates: Vec<SampledTokenCandidate>,
    fallback_top1: impl FnOnce() -> std::result::Result<MlxGreedyToken, String>,
    sampling_options: &QwenSamplingOptions,
    rng: &mut QwenSamplingRng,
) -> std::result::Result<MlxGreedyToken, String> {
    if candidates.is_empty() {
        return Err("no selectable token remained after suppression".to_string());
    }
    candidates.sort_by(|lhs, rhs| {
        rhs.scaled_logit
            .partial_cmp(&lhs.scaled_logit)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| lhs.token_id.cmp(&rhs.token_id))
    });
    let max_scaled_logit = candidates[0].scaled_logit;
    let mut prob_sum = 0.0f64;
    for candidate in &mut candidates {
        candidate.prob = f64::from((candidate.scaled_logit - max_scaled_logit).exp());
        prob_sum += candidate.prob;
    }
    if prob_sum <= 0.0 || !prob_sum.is_finite() {
        return fallback_top1();
    }
    for candidate in &mut candidates {
        candidate.prob /= prob_sum;
    }
    if sampling_options.top_p < 1.0 {
        let mut cumulative = 0.0f64;
        let mut keep_count = 0usize;
        for candidate in &candidates {
            cumulative += candidate.prob;
            keep_count += 1;
            if cumulative >= sampling_options.top_p as f64 {
                break;
            }
        }
        candidates.truncate(keep_count.max(1));
        let truncated_sum = candidates.iter().map(|candidate| candidate.prob).sum::<f64>();
        for candidate in &mut candidates {
            candidate.prob /= truncated_sum;
        }
    }
    let threshold = rng.next_f64();
    let mut cumulative = 0.0f64;
    for candidate in &candidates {
        cumulative += candidate.prob;
        if threshold <= cumulative {
            return Ok(MlxGreedyToken {
                token_id: candidate.token_id,
                logit: candidate.logit,
            });
        }
    }
    let sampled = candidates
        .last()
        .ok_or_else(|| "sampling candidate list became empty".to_string())?;
    Ok(MlxGreedyToken {
        token_id: sampled.token_id,
        logit: sampled.logit,
    })
}

fn sample_token_from_logits_f32(
    logits: &[f32],
    disallowed_token_ids: &[u32],
    sampling_options: &QwenSamplingOptions,
    rng: &mut QwenSamplingRng,
) -> std::result::Result<MlxGreedyToken, String> {
    if !sampling_options.do_sample || sampling_options.temperature <= 0.0 {
        return select_top1_token_from_logits(logits, disallowed_token_ids);
    }
    let top_k = sampling_options.top_k as usize;
    let mut candidates = Vec::with_capacity(top_k.max(logits.len().min(1024)));
    for (token_idx, &logit) in logits.iter().enumerate() {
        let token_id = token_idx as u32;
        if token_is_disallowed(disallowed_token_ids, token_id) {
            continue;
        }
        push_sampled_candidate_top_k(
            &mut candidates,
            SampledTokenCandidate {
                token_id,
                logit,
                scaled_logit: logit / sampling_options.temperature,
                prob: 0.0,
            },
            top_k,
        );
    }
    finalize_sampled_candidates(
        candidates,
        || select_top1_token_from_logits(logits, disallowed_token_ids),
        sampling_options,
        rng,
    )
}

fn add_residual_in_place(dst: &mut [f32], residual: &[f32]) -> std::result::Result<(), String> {
    if dst.len() != residual.len() {
        return Err(format!(
            "residual length mismatch: {} vs {}",
            dst.len(),
            residual.len()
        ));
    }
    for (dst, residual) in dst.iter_mut().zip(residual.iter().copied()) {
        *dst = qwen_bf16_round_to_f32(*dst + residual);
    }
    Ok(())
}

fn scale_in_place(values: &mut [f32], scale: f32) {
    for value in values {
        *value = qwen_bf16_round_to_f32(*value * scale);
    }
}

fn apply_sigmoid_gate_in_place(
    values: &mut [f32],
    gate: &[f32],
) -> std::result::Result<(), String> {
    if values.len() != gate.len() {
        return Err(format!(
            "sigmoid gate length mismatch: {} vs {}",
            values.len(),
            gate.len()
        ));
    }
    for (value, gate) in values.iter_mut().zip(gate.iter().copied()) {
        *value = qwen_bf16_round_to_f32(*value * sigmoid_f32(gate));
    }
    Ok(())
}

fn apply_silu_gate_in_place(values: &mut [f32], gate: &[f32]) -> std::result::Result<(), String> {
    if values.len() != gate.len() {
        return Err(format!(
            "silu gate length mismatch: {} vs {}",
            values.len(),
            gate.len()
        ));
    }
    for (value, gate) in values.iter_mut().zip(gate.iter().copied()) {
        *value = qwen_bf16_round_to_f32(*value * silu_f32(gate));
    }
    Ok(())
}

fn rms_norm_rows_shared_weight_f32(
    input: &[f32],
    weights: &[f32],
    row_count: usize,
    row_width: usize,
    eps: f32,
) -> std::result::Result<Vec<f32>, String> {
    if weights.len() != row_width {
        return Err(format!(
            "rms norm weights length {} does not match row width {}",
            weights.len(),
            row_width
        ));
    }
    if input.len() != row_count * row_width {
        return Err(format!(
            "rms norm row input length {} does not match {}x{}",
            input.len(),
            row_count,
            row_width
        ));
    }
    let mut out = Vec::with_capacity(input.len());
    for row in 0..row_count {
        let start = row * row_width;
        let end = start + row_width;
        out.extend_from_slice(&rms_norm_weighted_f32(&input[start..end], weights, eps));
    }
    Ok(out)
}

fn rms_norm_rows_no_scale_f32(
    input: &[f32],
    row_count: usize,
    row_width: usize,
    eps: f32,
) -> std::result::Result<Vec<f32>, String> {
    if input.len() != row_count * row_width {
        return Err(format!(
            "rms norm row input length {} does not match {}x{}",
            input.len(),
            row_count,
            row_width
        ));
    }
    let mut out = Vec::with_capacity(input.len());
    for row in 0..row_count {
        let start = row * row_width;
        let end = start + row_width;
        let values = &input[start..end];
        let mut mean_square = 0.0f32;
        for value in values {
            mean_square += value * value;
        }
        mean_square /= row_width.max(1) as f32;
        let inv_rms = 1.0f32 / (mean_square + eps).sqrt();
        for value in values {
            out.push(qwen_bf16_round_to_f32(*value * inv_rms));
        }
    }
    Ok(out)
}

fn softplus_f32(value: f32) -> f32 {
    let out = if value > 20.0 {
        value
    } else if value < -20.0 {
        value.exp()
    } else {
        (1.0 + value.exp()).ln()
    };
    qwen_bf16_round_to_f32(out)
}

fn compute_qwen_decay_gate(
    a_log: &[f32],
    alpha: &[f32],
    dt_bias: &[f32],
) -> std::result::Result<Vec<f32>, String> {
    if a_log.len() != alpha.len() || alpha.len() != dt_bias.len() {
        return Err(format!(
            "qwen decay gate input mismatch: a_log {} alpha {} dt_bias {}",
            a_log.len(),
            alpha.len(),
            dt_bias.len()
        ));
    }
    Ok(a_log
        .iter()
        .copied()
        .zip(alpha.iter().copied())
        .zip(dt_bias.iter().copied())
        .map(|((a_log, alpha), dt_bias)| {
            qwen_bf16_round_to_f32((-(a_log.exp()) * softplus_f32(alpha + dt_bias)).exp())
        })
        .collect())
}

fn apply_ssm_conv_with_state_f32(
    current: &[f32],
    state: &mut [f32],
    kernel: &[f32],
    kernel_size: usize,
) -> std::result::Result<Vec<f32>, String> {
    if kernel_size == 0 {
        return Err("ssm conv kernel size must be non-zero".to_string());
    }
    let channels = current.len();
    let prefix = kernel_size.saturating_sub(1);
    if state.len() != prefix * channels {
        return Err(format!(
            "ssm conv state length {} does not match {}x{}",
            state.len(),
            prefix,
            channels
        ));
    }
    if kernel.len() != kernel_size * channels {
        return Err(format!(
            "ssm conv kernel length {} does not match {}x{}",
            kernel.len(),
            kernel_size,
            channels
        ));
    }
    let mut window = vec![0.0f32; kernel.len()];
    for channel in 0..channels {
        let window_base = channel * kernel_size;
        let state_base = channel * prefix;
        if prefix != 0 {
            window[window_base..window_base + prefix]
                .copy_from_slice(&state[state_base..state_base + prefix]);
        }
        window[window_base + prefix] = current[channel];
        if prefix != 0 {
            if prefix > 1 {
                state.copy_within(state_base + 1..state_base + prefix, state_base);
            }
            state[state_base + prefix - 1] = current[channel];
        }
    }
    let mut out = ssm_conv_step_f32(&window, kernel, kernel_size, channels)?;
    for value in &mut out {
        *value = silu_f32(*value);
    }
    Ok(out)
}

#[allow(dead_code)]
fn qwen_text_mrope_positions(position: u32) -> [u32; 4] {
    [position, position, position, 0]
}

#[allow(dead_code)]
fn split_interleaved_query_gate_heads(
    qg: &[f32],
    head_dim: usize,
    head_count: usize,
) -> std::result::Result<(Vec<f32>, Vec<f32>), String> {
    let expected = head_dim
        .checked_mul(head_count)
        .and_then(|value| value.checked_mul(2))
        .ok_or_else(|| "query/gate projection length overflow".to_string())?;
    if qg.len() != expected {
        return Err(format!(
            "query/gate projection length mismatch: got {} expected {}",
            qg.len(),
            expected
        ));
    }
    let mut query = Vec::with_capacity(head_dim * head_count);
    let mut gate = Vec::with_capacity(head_dim * head_count);
    for head in 0..head_count {
        let base = head * head_dim * 2;
        query.extend_from_slice(&qg[base..base + head_dim]);
        gate.extend_from_slice(&qg[base + head_dim..base + head_dim * 2]);
    }
    Ok((query, gate))
}

#[allow(dead_code)]
fn split_recurrent_qkv_projection(
    qkv: &[f32],
    head_k_dim: usize,
    num_k_heads: usize,
    head_v_dim: usize,
    num_v_heads: usize,
) -> std::result::Result<(Vec<f32>, Vec<f32>, Vec<f32>), String> {
    let q_width = head_k_dim
        .checked_mul(num_k_heads)
        .ok_or_else(|| "recurrent q width overflow".to_string())?;
    let v_width = head_v_dim
        .checked_mul(num_v_heads)
        .ok_or_else(|| "recurrent v width overflow".to_string())?;
    let expected = q_width
        .checked_mul(2)
        .and_then(|value| value.checked_add(v_width))
        .ok_or_else(|| "recurrent qkv width overflow".to_string())?;
    if qkv.len() != expected {
        return Err(format!(
            "recurrent qkv length mismatch: got {} expected {}",
            qkv.len(),
            expected
        ));
    }
    Ok((
        qkv[..q_width].to_vec(),
        qkv[q_width..q_width * 2].to_vec(),
        qkv[q_width * 2..].to_vec(),
    ))
}

#[allow(dead_code)]
fn apply_qwen_mrope_rows_in_place(
    rows: &mut [f32],
    head_count: usize,
    head_dim: usize,
    rotary_dim: usize,
    positions: [u32; 4],
    sections: [u32; 4],
    rope_theta: f32,
) -> std::result::Result<(), String> {
    if head_dim == 0 || rows.len() != head_count * head_dim {
        return Err(format!(
            "mrope rows length mismatch: got {} expected {}",
            rows.len(),
            head_count * head_dim
        ));
    }
    if rotary_dim == 0 {
        return Ok(());
    }
    if rotary_dim > head_dim || rotary_dim % 2 != 0 {
        return Err(format!(
            "invalid rotary dim {} for head dim {}",
            rotary_dim, head_dim
        ));
    }
    if rope_theta <= 0.0 || !rope_theta.is_finite() {
        return Err(format!("invalid rope theta {}", rope_theta));
    }
    let pair_count = rotary_dim / 2;
    let sect_dims = sections.iter().copied().sum::<u32>() as usize;
    if sect_dims == 0 {
        return Err("mrope sections are empty".to_string());
    }
    let section_h_start = sections[0] as usize;
    let section_w_start = section_h_start + sections[1] as usize;
    let section_e_start = section_w_start + sections[2] as usize;
    for head in 0..head_count {
        let row = &mut rows[head * head_dim..(head + 1) * head_dim];
        for pair_idx in 0..pair_count {
            let sector = pair_idx % sect_dims;
            let position = if sector % 3 == 1 && sector < 3 * sections[1] as usize {
                positions[1]
            } else if sector % 3 == 2 && sector < 3 * sections[2] as usize {
                positions[2]
            } else if sector % 3 == 0 && sector < 3 * sections[0] as usize {
                positions[0]
            } else if sector >= section_e_start {
                positions[3]
            } else if sector >= section_w_start {
                positions[2]
            } else if sector >= section_h_start {
                positions[1]
            } else {
                positions[0]
            };
            let theta =
                (position as f32) * rope_theta.powf(-(2.0 * pair_idx as f32) / rotary_dim as f32);
            let cos_theta = theta.cos();
            let sin_theta = theta.sin();
            let x0 = row[pair_idx];
            let x1 = row[pair_idx + pair_count];
            row[pair_idx] = x0 * cos_theta - x1 * sin_theta;
            row[pair_idx + pair_count] = x0 * sin_theta + x1 * cos_theta;
        }
    }
    Ok(())
}

#[allow(dead_code)]
fn grouped_self_attention_step_f32(
    query: &[f32],
    key_cache: &[f32],
    value_cache: &[f32],
    q_head_count: usize,
    kv_head_count: usize,
    head_dim: usize,
    token_count: usize,
) -> std::result::Result<Vec<f32>, String> {
    if q_head_count == 0 || kv_head_count == 0 || head_dim == 0 {
        return Err("attention shape components must be non-zero".to_string());
    }
    if q_head_count % kv_head_count != 0 {
        return Err(format!(
            "q heads {} must be divisible by kv heads {}",
            q_head_count, kv_head_count
        ));
    }
    let query_width = q_head_count
        .checked_mul(head_dim)
        .ok_or_else(|| "attention query width overflow".to_string())?;
    let kv_width = kv_head_count
        .checked_mul(head_dim)
        .ok_or_else(|| "attention kv width overflow".to_string())?;
    if query.len() != query_width {
        return Err(format!(
            "attention query length mismatch: got {} expected {}",
            query.len(),
            query_width
        ));
    }
    let expected_cache = token_count
        .checked_mul(kv_width)
        .ok_or_else(|| "attention cache width overflow".to_string())?;
    if key_cache.len() != expected_cache || value_cache.len() != expected_cache {
        return Err(format!(
            "attention cache length mismatch: key {} value {} expected {}",
            key_cache.len(),
            value_cache.len(),
            expected_cache
        ));
    }
    let q_heads_per_kv = q_head_count / kv_head_count;
    let scale = 1.0f32 / (head_dim as f32).sqrt();
    let mut out = vec![0.0f32; query_width];
    let mut logits = vec![0.0f32; token_count];
    for q_head in 0..q_head_count {
        let kv_head = q_head / q_heads_per_kv;
        let q_row = &query[q_head * head_dim..(q_head + 1) * head_dim];
        for token in 0..token_count {
            let key_base = token * kv_width + kv_head * head_dim;
            let key_row = &key_cache[key_base..key_base + head_dim];
            let mut dot = 0.0f32;
            for (q, k) in q_row.iter().zip(key_row.iter()) {
                dot += q * k;
            }
            logits[token] = dot * scale;
        }
        let max_logit = logits.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let mut exp_sum = 0.0f32;
        for logit in &mut logits {
            *logit = (*logit - max_logit).exp();
            exp_sum += *logit;
        }
        if !exp_sum.is_finite() || exp_sum <= 0.0 {
            return Err(format!("attention softmax sum is invalid: {}", exp_sum));
        }
        let out_row = &mut out[q_head * head_dim..(q_head + 1) * head_dim];
        for token in 0..token_count {
            let weight = logits[token] / exp_sum;
            let value_base = token * kv_width + kv_head * head_dim;
            let value_row = &value_cache[value_base..value_base + head_dim];
            for (dst, value) in out_row.iter_mut().zip(value_row.iter().copied()) {
                *dst += value * weight;
            }
        }
    }
    Ok(out)
}

#[allow(dead_code)]
fn ssm_conv_step_f32(
    window: &[f32],
    kernel: &[f32],
    d_conv: usize,
    channels: usize,
) -> std::result::Result<Vec<f32>, String> {
    let expected = d_conv
        .checked_mul(channels)
        .ok_or_else(|| "ssm_conv size overflow".to_string())?;
    if window.len() != expected || kernel.len() != expected {
        return Err(format!(
            "ssm_conv input/kernel mismatch: window {} kernel {} expected {}",
            window.len(),
            kernel.len(),
            expected
        ));
    }
    let mut out = vec![0.0f32; channels];
    for channel in 0..channels {
        let row_base = channel * d_conv;
        let mut sum = 0.0f32;
        for tap in 0..d_conv {
            sum += window[row_base + tap] * kernel[row_base + tap];
        }
        out[channel] = sum;
    }
    Ok(out)
}

#[allow(dead_code)]
fn gated_delta_net_step_f32(
    q: &[f32],
    k: &[f32],
    v: &[f32],
    g: &[f32],
    beta: &[f32],
    state: &mut [f32],
    head_k_dim: usize,
    num_k_heads: usize,
    head_v_dim: usize,
    num_v_heads: usize,
) -> std::result::Result<Vec<f32>, String> {
    let qk_expected = head_k_dim
        .checked_mul(num_k_heads)
        .ok_or_else(|| "gated_delta qk width overflow".to_string())?;
    let v_expected = head_v_dim
        .checked_mul(num_v_heads)
        .ok_or_else(|| "gated_delta v width overflow".to_string())?;
    let state_expected = head_k_dim
        .checked_mul(head_v_dim)
        .and_then(|value| value.checked_mul(num_v_heads))
        .ok_or_else(|| "gated_delta state width overflow".to_string())?;
    if q.len() != qk_expected || k.len() != qk_expected {
        return Err(format!(
            "gated_delta q/k length mismatch: q {} k {} expected {}",
            q.len(),
            k.len(),
            qk_expected
        ));
    }
    if v.len() != v_expected || beta.len() != num_v_heads || state.len() != state_expected {
        return Err(format!(
            "gated_delta v/beta/state mismatch: v {} beta {} state {} expected v {} beta {} state {}",
            v.len(),
            beta.len(),
            state.len(),
            v_expected,
            num_v_heads,
            state_expected
        ));
    }
    let kda = g.len() == num_v_heads * head_k_dim;
    if !(g.len() == num_v_heads || kda) {
        return Err(format!(
            "gated_delta gate length {} must be {} or {}",
            g.len(),
            num_v_heads,
            num_v_heads * head_k_dim
        ));
    }
    let mut out = vec![0.0f32; v_expected];
    let mut delta = vec![0.0f32; head_v_dim];
    let v_heads_per_k = num_v_heads
        .checked_div(num_k_heads)
        .ok_or_else(|| "invalid gated_delta head mapping".to_string())?;
    if v_heads_per_k == 0 || num_k_heads * v_heads_per_k != num_v_heads {
        return Err(format!(
            "num_v_heads {} must be divisible by num_k_heads {}",
            num_v_heads, num_k_heads
        ));
    }
    for head in 0..num_v_heads {
        let q_head = head / v_heads_per_k;
        let k_head = head / v_heads_per_k;
        let q_row = &q[q_head * head_k_dim..(q_head + 1) * head_k_dim];
        let k_row = &k[k_head * head_k_dim..(k_head + 1) * head_k_dim];
        let v_row = &v[head * head_v_dim..(head + 1) * head_v_dim];
        let state_base = head * head_v_dim * head_k_dim;
        if kda {
            let gate_row = &g[head * head_k_dim..(head + 1) * head_k_dim];
            for row in 0..head_v_dim {
                let row_base = state_base + row * head_k_dim;
                for col in 0..head_k_dim {
                    state[row_base + col] = qwen_bf16_round_to_f32(state[row_base + col] * gate_row[col]);
                }
            }
        } else {
            let gate_value = g[head];
            for index in 0..(head_v_dim * head_k_dim) {
                state[state_base + index] =
                    qwen_bf16_round_to_f32(state[state_base + index] * gate_value);
            }
        }
        for row in 0..head_v_dim {
            let row_base = state_base + row * head_k_dim;
            let mut sum = 0.0f32;
            for col in 0..head_k_dim {
                sum = qwen_bf16_round_to_f32(sum + qwen_bf16_round_to_f32(state[row_base + col] * k_row[col]));
            }
            delta[row] = qwen_bf16_round_to_f32((v_row[row] - sum) * beta[head]);
        }
        for row in 0..head_v_dim {
            let row_base = state_base + row * head_k_dim;
            for col in 0..head_k_dim {
                state[row_base + col] =
                    qwen_bf16_round_to_f32(state[row_base + col] + qwen_bf16_round_to_f32(k_row[col] * delta[row]));
            }
        }
        let out_row = &mut out[head * head_v_dim..(head + 1) * head_v_dim];
        for row in 0..head_v_dim {
            let row_base = state_base + row * head_k_dim;
            let mut sum = 0.0f32;
            for col in 0..head_k_dim {
                sum = qwen_bf16_round_to_f32(sum + qwen_bf16_round_to_f32(state[row_base + col] * q_row[col]));
            }
            out_row[row] = sum;
        }
    }
    Ok(out)
}

fn split_gate_up_projection(merged: &[f32]) -> std::result::Result<(Vec<f32>, Vec<f32>), String> {
    if merged.len() % 2 != 0 {
        return Err(format!(
            "merged gate/up projection length {} is not even",
            merged.len()
        ));
    }
    let half = merged.len() / 2;
    Ok((merged[..half].to_vec(), merged[half..].to_vec()))
}

fn swiglu_split_f32(gate: &[f32], up: &[f32]) -> std::result::Result<Vec<f32>, String> {
    if gate.len() != up.len() {
        return Err(format!(
            "swiglu gate/up length mismatch: {} vs {}",
            gate.len(),
            up.len()
        ));
    }
    Ok(gate
        .iter()
        .copied()
        .zip(up.iter().copied())
        .map(|(gate, up)| qwen_bf16_round_to_f32(silu_f32(gate) * up))
        .collect())
}

fn softmax_top_k_routes(
    logits: &[f32],
    top_k: usize,
) -> std::result::Result<(Vec<f32>, Vec<MlxQwen35MoeExpertRoute>), String> {
    if logits.is_empty() {
        return Err("router logits are empty".to_string());
    }
    if top_k == 0 || top_k > logits.len() {
        return Err(format!(
            "router top_k {} is invalid for {} logits",
            top_k,
            logits.len()
        ));
    }
    let max_logit = logits.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let exp_scores = logits
        .iter()
        .copied()
        .map(|value| (value - max_logit).exp())
        .collect::<Vec<_>>();
    let exp_sum = exp_scores.iter().copied().sum::<f32>();
    if !exp_sum.is_finite() || exp_sum <= 0.0 {
        return Err(format!("router softmax sum is invalid: {}", exp_sum));
    }
    let probabilities = exp_scores
        .iter()
        .copied()
        .map(|value| qwen_bf16_round_to_f32(value / exp_sum))
        .collect::<Vec<_>>();
    let mut expert_indices = (0..logits.len()).collect::<Vec<_>>();
    expert_indices.sort_by(|&lhs, &rhs| {
        probabilities[rhs]
            .total_cmp(&probabilities[lhs])
            .then_with(|| lhs.cmp(&rhs))
    });
    let selected = expert_indices.into_iter().take(top_k).collect::<Vec<_>>();
    let selected_sum = selected
        .iter()
        .copied()
        .map(|index| probabilities[index])
        .sum::<f32>();
    if !selected_sum.is_finite() || selected_sum <= 0.0 {
        return Err(format!(
            "router selected probability sum is invalid: {}",
            selected_sum
        ));
    }
    let routes = selected
        .into_iter()
        .map(|index| MlxQwen35MoeExpertRoute {
            expert_index: index as u32,
            logit: logits[index],
            probability: probabilities[index],
            weight: qwen_bf16_round_to_f32(probabilities[index] / selected_sum),
        })
        .collect::<Vec<_>>();
    Ok((probabilities, routes))
}

fn rms_norm_weighted_f32(input: &[f32], weights: &[f32], eps: f32) -> Vec<f32> {
    let mut mean_square = 0.0f32;
    for value in input {
        mean_square += value * value;
    }
    mean_square /= input.len().max(1) as f32;
    let inv_rms = 1.0f32 / (mean_square + eps).sqrt();
    input
        .iter()
        .copied()
        .zip(weights.iter().copied())
        .map(|(value, weight)| {
            qwen_bf16_round_to_f32(qwen_bf16_round_to_f32(value * inv_rms) * weight)
        })
        .collect()
}

fn silu_f32(value: f32) -> f32 {
    qwen_bf16_round_to_f32(value / (1.0 + (-value).exp()))
}

fn sigmoid_f32(value: f32) -> f32 {
    qwen_bf16_round_to_f32(1.0 / (1.0 + (-value).exp()))
}

fn qwen_bf16_word_to_f32(word: u16) -> f32 {
    f32::from_bits((word as u32) << 16)
}

fn qwen_f32_to_bf16_word(value: f32) -> u16 {
    (qwen_bf16_round_to_f32(value).to_bits() >> 16) as u16
}

fn qwen_bf16_round_to_f32(value: f32) -> f32 {
    let bits = value.to_bits();
    let lsb = (bits >> 16) & 1;
    let rounded = bits.wrapping_add(0x7FFF + lsb) & 0xFFFF0000;
    f32::from_bits(rounded)
}

fn bf16_words_as_bytes(words: &[u16]) -> &[u8] {
    #[cfg(target_endian = "little")]
    unsafe {
        std::slice::from_raw_parts(words.as_ptr().cast::<u8>(), words.len() * size_of::<u16>())
    }

    #[cfg(not(target_endian = "little"))]
    {
        unreachable!("bf16 byte reinterpreting currently assumes little-endian targets")
    }
}

#[cfg(test)]
mod tests {
    use super::{
        actual_affine_qparam_names, apply_qwen_mrope_rows_in_place, gated_delta_net_step_f32,
        grouped_self_attention_step_f32, qwen_bf16_round_to_f32, qwen_text_mrope_positions,
        softmax_top_k_routes, split_gate_up_projection, split_interleaved_query_gate_heads,
        split_recurrent_qkv_projection, ssm_conv_step_f32, swiglu_split_f32,
    };

    #[test]
    fn qwen_affine_qparam_names_handle_weight_suffix_and_suffixless_paths() {
        assert_eq!(
            actual_affine_qparam_names("language_model.model.layers.0.self_attn.q_proj.weight"),
            (
                "language_model.model.layers.0.self_attn.q_proj.scales".to_string(),
                "language_model.model.layers.0.self_attn.q_proj.biases".to_string()
            )
        );
        assert_eq!(
            actual_affine_qparam_names("model.language_model.layers.0.mlp.experts.gate_up_proj"),
            (
                "model.language_model.layers.0.mlp.experts.gate_up_proj.scales".to_string(),
                "model.language_model.layers.0.mlp.experts.gate_up_proj.biases".to_string()
            )
        );
    }

    #[test]
    fn qwen_softmax_top_k_routes_renormalize_selected_experts() {
        let (probabilities, routes) = softmax_top_k_routes(&[1.0, 3.0, 2.0], 2).unwrap();
        assert_eq!(routes.len(), 2);
        assert_eq!(routes[0].expert_index, 1);
        assert_eq!(routes[1].expert_index, 2);
        let route_weight_sum = routes.iter().map(|route| route.weight).sum::<f32>();
        assert!((route_weight_sum - 1.0).abs() < 0.0005);
        assert!(probabilities[1] > probabilities[2]);
        assert!(probabilities[2] > probabilities[0]);
    }

    #[test]
    fn qwen_swiglu_split_matches_expected_shape() {
        let out = swiglu_split_f32(&[1.0, -1.0], &[2.0, 4.0]).unwrap();
        assert_eq!(out.len(), 2);
        assert!((out[0] - qwen_bf16_round_to_f32(1.4628906)).abs() < 0.01);
        assert!((out[1] - qwen_bf16_round_to_f32(-1.0751953)).abs() < 0.02);
    }

    #[test]
    fn qwen_split_gate_up_projection_requires_even_length() {
        assert!(split_gate_up_projection(&[1.0, 2.0, 3.0]).is_err());
        let (gate, up) = split_gate_up_projection(&[1.0, 2.0, 3.0, 4.0]).unwrap();
        assert_eq!(gate, vec![1.0, 2.0]);
        assert_eq!(up, vec![3.0, 4.0]);
    }

    #[test]
    fn qwen_split_interleaved_query_gate_heads_preserves_head_local_order() {
        let (query, gate) =
            split_interleaved_query_gate_heads(&[1.0, 2.0, 10.0, 20.0, 3.0, 4.0, 30.0, 40.0], 2, 2)
                .unwrap();
        assert_eq!(query, vec![1.0, 2.0, 3.0, 4.0]);
        assert_eq!(gate, vec![10.0, 20.0, 30.0, 40.0]);
    }

    #[test]
    fn qwen_split_recurrent_qkv_projection_respects_qkqv_layout() {
        let (q, k, v) = split_recurrent_qkv_projection(
            &[1.0, 2.0, 10.0, 20.0, 100.0, 200.0, 300.0, 400.0],
            1,
            2,
            2,
            2,
        )
        .unwrap();
        assert_eq!(q, vec![1.0, 2.0]);
        assert_eq!(k, vec![10.0, 20.0]);
        assert_eq!(v, vec![100.0, 200.0, 300.0, 400.0]);
    }

    #[test]
    fn qwen_text_mrope_positions_match_llama_text_expansion() {
        assert_eq!(qwen_text_mrope_positions(7), [7, 7, 7, 0]);
    }

    #[test]
    fn qwen_mrope_keeps_position_zero_identity_and_preserves_tail() {
        let mut rows = vec![1.0, 2.0, 3.0, 4.0, 99.0, 100.0];
        apply_qwen_mrope_rows_in_place(
            &mut rows,
            1,
            6,
            4,
            [0, 0, 0, 0],
            [11, 11, 10, 0],
            10_000_000.0,
        )
        .unwrap();
        assert_eq!(rows, vec![1.0, 2.0, 3.0, 4.0, 99.0, 100.0]);
    }

    #[test]
    fn qwen_grouped_attention_decode_step_matches_manual_single_kv_head() {
        let out = grouped_self_attention_step_f32(
            &[1.0, 0.0, 0.0, 1.0],
            &[1.0, 0.0, 0.0, 1.0],
            &[5.0, 6.0, 7.0, 8.0],
            2,
            1,
            2,
            2,
        )
        .unwrap();
        assert_eq!(out.len(), 4);
        assert!(out[0] > 5.0 && out[0] < 7.0);
        assert!(out[1] > 6.0 && out[1] < 8.0);
        assert!(out[2] > 5.0 && out[2] < 7.0);
        assert!(out[3] > 6.0 && out[3] < 8.0);
    }

    #[test]
    fn qwen_ssm_conv_step_matches_manual_dot_per_channel() {
        let out = ssm_conv_step_f32(
            &[1.0, 2.0, 3.0, 4.0, 10.0, 20.0, 30.0, 40.0],
            &[0.5, 0.5, 1.0, 1.0, 1.0, 0.0, 0.5, 0.0],
            4,
            2,
        )
        .unwrap();
        assert_eq!(out, vec![8.5, 25.0]);
    }

    #[test]
    fn qwen_gated_delta_step_matches_single_head_manual_case() {
        let mut state = vec![0.0, 0.0, 0.0, 0.0];
        let out = gated_delta_net_step_f32(
            &[1.0, 0.0],
            &[1.0, 0.0],
            &[2.0, 4.0],
            &[1.0],
            &[0.5],
            &mut state,
            2,
            1,
            2,
            1,
        )
        .unwrap();
        assert_eq!(out.len(), 2);
        assert!((state[0] - 1.0).abs() < 1.0e-6);
        assert!((state[2] - 2.0).abs() < 1.0e-6);
        assert!((out[0] - 1.0).abs() < 1.0e-6);
        assert!((out[1] - 2.0).abs() < 1.0e-6);
    }
}
