#[cfg(test)]
use crate::layer0_cached_case::ExactMetalGenerationCursor;
use crate::layer0_cached_case::{
    run_layer_sequence_from_inputs, CachedLayerInputs, ExactMetalGenerationGraph,
    ExactMetalGenerationStopReason, ExactMetalTextRuntimeSession, Layer0CachedArtifacts,
    Layer0CachedPlan, Layer0CachedStage,
};
use crate::GemmaKvCacheLayout;
#[cfg(test)]
use crate::{GemmaKvCacheSet, KvTensor, KvTensorShape};
use makepad_ggml::backend::metal::MetalRuntimeCounters;
#[cfg(test)]
use makepad_mlx_rt_core::{MlxDType, MlxGemmaMoeExpertOutput, MlxRouterTopKOutput};
use makepad_mlx_rt_core::{MlxGreedyToken, MlxIndexedSafetensors, MlxTokenizer};
use std::collections::BTreeSet;
use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct GemmaTextStepOutput {
    pub model_path: PathBuf,
    pub prompt_text: Option<String>,
    pub prompt_token_ids: Vec<u32>,
    pub layers: Vec<Layer0CachedArtifacts>,
    pub final_hidden_bf16_words: Vec<u16>,
    pub final_norm_bf16_words: Vec<u16>,
    pub next_token: MlxGreedyToken,
    pub next_token_text: String,
}

pub fn run_two_token_prompt(
    model_path: PathBuf,
    prompt_text: impl Into<String>,
) -> Result<GemmaTextStepOutput, Box<dyn Error>> {
    let prompt_text = prompt_text.into();
    let runtime = GemmaTextRuntimeSession::load(&model_path).map_err(|err| err.to_string())?;
    let prompt_token_ids = runtime
        .tokenizer
        .encode(&prompt_text)
        .map_err(|err| err.to_string())?;
    if prompt_token_ids.len() != 2 {
        return Err(format!(
            "two-token prompt path expected exactly 2 token ids, got {} for {:?}",
            prompt_token_ids.len(),
            prompt_text
        )
        .into());
    }
    run_two_token_ids_with_loaded(runtime, Some(prompt_text), prompt_token_ids, 0, 1)
}

pub fn run_two_token_ids(
    model_path: PathBuf,
    prompt_token_ids: [u32; 2],
    prefill_position: i32,
    decode_position: i32,
) -> Result<GemmaTextStepOutput, Box<dyn Error>> {
    let runtime = GemmaTextRuntimeSession::load(&model_path).map_err(|err| err.to_string())?;
    run_two_token_ids_with_loaded(
        runtime,
        None,
        prompt_token_ids.to_vec(),
        prefill_position,
        decode_position,
    )
}

fn run_two_token_ids_with_loaded(
    runtime: Arc<GemmaTextRuntimeSession>,
    prompt_text: Option<String>,
    prompt_token_ids: Vec<u32>,
    prefill_position: i32,
    decode_position: i32,
) -> Result<GemmaTextStepOutput, Box<dyn Error>> {
    if prompt_token_ids.len() != 2 {
        return Err(format!(
            "two-token step expected exactly 2 token ids, got {}",
            prompt_token_ids.len()
        )
        .into());
    }

    let prefill_input_words = runtime
        .weights
        .embed_token_bf16_words(prompt_token_ids[0])?;
    let decode_input_words = runtime
        .weights
        .embed_token_bf16_words(prompt_token_ids[1])?;
    let layer_indices = (0..runtime
        .weights
        .snapshot
        .config
        .text_config
        .num_hidden_layers as usize)
        .collect::<Vec<_>>();
    let mut plan = Layer0CachedPlan::new();
    plan.require_stage(Layer0CachedStage::PostFfnResidual);
    let layers = run_layer_sequence_from_inputs(
        runtime.model_path.clone(),
        &layer_indices,
        CachedLayerInputs {
            prefill_input_words,
            decode_input_words,
            prefill_rope_offset: prefill_position,
            decode_rope_offset: decode_position,
            validate_against_oracle: false,
        },
        plan,
    )?;
    let final_hidden_bf16_words = layers
        .last()
        .ok_or("two-token step produced no layer outputs")?
        .bf16_words_for_stage(Layer0CachedStage::PostFfnResidual)
        .ok_or("missing final hidden state from last text layer")?;
    let final_norm_bf16_words = runtime
        .weights
        .final_text_norm_bf16_words(&final_hidden_bf16_words)?;
    let next_token = runtime
        .weights
        .tied_text_logits_top1_f32(&final_norm_bf16_words)?;
    let next_token_text = runtime.tokenizer.decode(&[next_token.token_id])?;

    Ok(GemmaTextStepOutput {
        model_path: runtime.model_path.clone(),
        prompt_text,
        prompt_token_ids,
        layers,
        final_hidden_bf16_words,
        final_norm_bf16_words,
        next_token,
        next_token_text,
    })
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GemmaPromptFormat {
    RawBos,
    #[default]
    Gemma4UserTurn,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GemmaStopReason {
    MaxNewTokens,
    EosToken(u32),
}

#[derive(Clone, Debug)]
pub struct GemmaTextGenerationOptions {
    pub max_new_tokens: usize,
    pub prompt_format: GemmaPromptFormat,
}

impl Default for GemmaTextGenerationOptions {
    fn default() -> Self {
        Self {
            max_new_tokens: 32,
            prompt_format: GemmaPromptFormat::Gemma4UserTurn,
        }
    }
}

#[derive(Clone, Debug)]
pub struct GemmaTextGenerationOutput {
    pub model_path: PathBuf,
    pub prompt_text: Arc<str>,
    pub formatted_prompt_text: Arc<str>,
    pub prompt_token_ids: Arc<[u32]>,
    pub generated_token_ids: Arc<[u32]>,
    pub generated_text: Arc<str>,
    pub stop_reason: GemmaStopReason,
}

#[derive(Clone, Debug)]
pub struct GemmaTextBenchmarkOutput {
    pub model_path: PathBuf,
    pub prompt_text: Arc<str>,
    pub formatted_prompt_text: Arc<str>,
    pub prompt_token_ids: Arc<[u32]>,
    pub max_new_tokens: usize,
    pub warmup_iters: usize,
    pub measured_iters: usize,
    pub load_duration: Duration,
    pub elapsed: Duration,
    pub total_generated_tokens: usize,
    pub time_to_first_token_elapsed: Duration,
    pub steady_state_elapsed: Duration,
    pub steady_state_generated_tokens: usize,
    pub last_generated_token_ids: Arc<[u32]>,
    pub last_generated_text: Arc<str>,
    pub metal_counters: MetalRuntimeCounters,
    pub prompt_prefill_tokens_per_second: f64,
    pub steady_state_decode_tokens_per_second: f64,
    pub decode_tokens_per_second: f64,
    pub total_tokens_per_second: f64,
}

#[derive(Clone)]
pub struct GemmaLazyTextPlan {
    inner: Arc<GemmaLazyTextPlanInner>,
}

struct LazyTextNode<T> {
    value: OnceLock<Result<T, String>>,
}

impl<T> Default for LazyTextNode<T> {
    fn default() -> Self {
        Self {
            value: OnceLock::new(),
        }
    }
}

impl<T: Clone> LazyTextNode<T> {
    fn eval<F>(&self, f: F) -> Result<T, String>
    where
        F: FnOnce() -> Result<T, String>,
    {
        self.value.get_or_init(f).clone()
    }
}

struct GemmaLazyTextPlanInner {
    model_path: PathBuf,
    prompt_text: Arc<str>,
    options: GemmaTextGenerationOptions,
    runtime: LazyTextNode<Arc<GemmaTextRuntimeSession>>,
    formatted_prompt_text: LazyTextNode<Arc<str>>,
    prompt_token_ids: LazyTextNode<Arc<[u32]>>,
    generation_graph: LazyTextNode<Arc<ExactMetalGenerationGraph>>,
    generation: LazyTextNode<Arc<GemmaTextGenerationOutput>>,
}

#[derive(Clone)]
struct GemmaTextRuntimeSession {
    model_path: PathBuf,
    weights: MlxIndexedSafetensors,
    tokenizer: MlxTokenizer,
    kv_layout: GemmaKvCacheLayout,
    stop_tokens: BTreeSet<u32>,
    exact_backend: Arc<Mutex<ExactMetalTextRuntimeSession>>,
}

#[cfg(test)]
#[derive(Clone, Debug)]
struct TextProjectionNames {
    weight_name: String,
    scales_name: String,
    biases_name: String,
    norm_weight_name: Option<String>,
}

#[cfg(test)]
impl TextProjectionNames {
    fn new(base: &str, prefix: &str, norm_weight_name: Option<String>) -> Self {
        Self {
            weight_name: format!("{base}.self_attn.{prefix}.weight"),
            scales_name: format!("{base}.self_attn.{prefix}.scales"),
            biases_name: format!("{base}.self_attn.{prefix}.biases"),
            norm_weight_name,
        }
    }
}

#[cfg(test)]
#[derive(Clone, Debug)]
struct TextLayerTensorNames {
    input_norm_weight_name: String,
    q: TextProjectionNames,
    k: TextProjectionNames,
    v: TextProjectionNames,
    o: TextProjectionNames,
    post_attention_norm_weight_name: String,
    pre_feedforward_norm_weight_name: String,
    pre_feedforward_norm2_weight_name: String,
    post_feedforward_norm_weight_name: String,
    post_feedforward_norm1_weight_name: String,
    post_feedforward_norm2_weight_name: String,
    mlp_gate_weight_name: String,
    mlp_gate_scales_name: String,
    mlp_gate_biases_name: String,
    mlp_up_weight_name: String,
    mlp_up_scales_name: String,
    mlp_up_biases_name: String,
    mlp_down_weight_name: String,
    mlp_down_scales_name: String,
    mlp_down_biases_name: String,
    router_scale_name: String,
    router_per_expert_scale_name: String,
    router_proj_weight_name: String,
    router_proj_scales_name: String,
    router_proj_biases_name: String,
    expert_gate_weight_name: String,
    expert_gate_scales_name: String,
    expert_gate_biases_name: String,
    expert_up_weight_name: String,
    expert_up_scales_name: String,
    expert_up_biases_name: String,
    expert_down_weight_name: String,
    expert_down_scales_name: String,
    expert_down_biases_name: String,
    layer_scalar_name: String,
}

#[cfg(test)]
impl TextLayerTensorNames {
    fn for_layer(layer_idx: usize, attention_k_eq_v: bool) -> Self {
        let base = format!("language_model.model.layers.{layer_idx}");
        let q = TextProjectionNames::new(
            &base,
            "q_proj",
            Some(format!("{base}.self_attn.q_norm.weight")),
        );
        let k = TextProjectionNames::new(
            &base,
            "k_proj",
            Some(format!("{base}.self_attn.k_norm.weight")),
        );
        let v = if attention_k_eq_v {
            TextProjectionNames {
                weight_name: k.weight_name.clone(),
                scales_name: k.scales_name.clone(),
                biases_name: k.biases_name.clone(),
                norm_weight_name: None,
            }
        } else {
            TextProjectionNames::new(&base, "v_proj", None)
        };
        let o = TextProjectionNames::new(&base, "o_proj", None);
        Self {
            input_norm_weight_name: format!("{base}.input_layernorm.weight"),
            q,
            k,
            v,
            o,
            post_attention_norm_weight_name: format!("{base}.post_attention_layernorm.weight"),
            pre_feedforward_norm_weight_name: format!("{base}.pre_feedforward_layernorm.weight"),
            pre_feedforward_norm2_weight_name: format!("{base}.pre_feedforward_layernorm_2.weight"),
            post_feedforward_norm_weight_name: format!("{base}.post_feedforward_layernorm.weight"),
            post_feedforward_norm1_weight_name: format!(
                "{base}.post_feedforward_layernorm_1.weight"
            ),
            post_feedforward_norm2_weight_name: format!(
                "{base}.post_feedforward_layernorm_2.weight"
            ),
            mlp_gate_weight_name: format!("{base}.mlp.gate_proj.weight"),
            mlp_gate_scales_name: format!("{base}.mlp.gate_proj.scales"),
            mlp_gate_biases_name: format!("{base}.mlp.gate_proj.biases"),
            mlp_up_weight_name: format!("{base}.mlp.up_proj.weight"),
            mlp_up_scales_name: format!("{base}.mlp.up_proj.scales"),
            mlp_up_biases_name: format!("{base}.mlp.up_proj.biases"),
            mlp_down_weight_name: format!("{base}.mlp.down_proj.weight"),
            mlp_down_scales_name: format!("{base}.mlp.down_proj.scales"),
            mlp_down_biases_name: format!("{base}.mlp.down_proj.biases"),
            router_scale_name: format!("{base}.router.scale"),
            router_per_expert_scale_name: format!("{base}.router.per_expert_scale"),
            router_proj_weight_name: format!("{base}.router.proj.weight"),
            router_proj_scales_name: format!("{base}.router.proj.scales"),
            router_proj_biases_name: format!("{base}.router.proj.biases"),
            expert_gate_weight_name: format!("{base}.experts.switch_glu.gate_proj.weight"),
            expert_gate_scales_name: format!("{base}.experts.switch_glu.gate_proj.scales"),
            expert_gate_biases_name: format!("{base}.experts.switch_glu.gate_proj.biases"),
            expert_up_weight_name: format!("{base}.experts.switch_glu.up_proj.weight"),
            expert_up_scales_name: format!("{base}.experts.switch_glu.up_proj.scales"),
            expert_up_biases_name: format!("{base}.experts.switch_glu.up_proj.biases"),
            expert_down_weight_name: format!("{base}.experts.switch_glu.down_proj.weight"),
            expert_down_scales_name: format!("{base}.experts.switch_glu.down_proj.scales"),
            expert_down_biases_name: format!("{base}.experts.switch_glu.down_proj.biases"),
            layer_scalar_name: format!("{base}.layer_scalar"),
        }
    }
}

#[cfg(test)]
#[derive(Clone, Copy, Debug)]
struct RopeSpec {
    head_dim: usize,
    rotary_dim: usize,
    base: f32,
}

impl GemmaLazyTextPlan {
    pub fn from_prompt(
        model_path: PathBuf,
        prompt_text: impl Into<String>,
        options: GemmaTextGenerationOptions,
    ) -> Self {
        Self {
            inner: Arc::new(GemmaLazyTextPlanInner {
                model_path,
                prompt_text: Arc::<str>::from(prompt_text.into()),
                options,
                runtime: LazyTextNode::default(),
                formatted_prompt_text: LazyTextNode::default(),
                prompt_token_ids: LazyTextNode::default(),
                generation_graph: LazyTextNode::default(),
                generation: LazyTextNode::default(),
            }),
        }
    }

    pub fn eval_formatted_prompt_text(&self) -> Result<Arc<str>, Box<dyn Error>> {
        self.inner.formatted_prompt_text().map_err(|err| err.into())
    }

    pub fn eval_prompt_token_ids(&self) -> Result<Arc<[u32]>, Box<dyn Error>> {
        self.inner.prompt_token_ids().map_err(|err| err.into())
    }

    pub fn eval_generate(&self) -> Result<Arc<GemmaTextGenerationOutput>, Box<dyn Error>> {
        self.inner.generation().map_err(|err| err.into())
    }

    pub fn eval_generated_token_ids_up_to(
        &self,
        max_count: usize,
    ) -> Result<Arc<[u32]>, Box<dyn Error>> {
        self.inner
            .generated_token_ids_up_to(max_count)
            .map_err(|err| err.into())
    }
}

impl GemmaLazyTextPlanInner {
    fn runtime(&self) -> Result<Arc<GemmaTextRuntimeSession>, String> {
        self.runtime
            .eval(|| GemmaTextRuntimeSession::load(&self.model_path))
    }

    fn formatted_prompt_text(&self) -> Result<Arc<str>, String> {
        self.formatted_prompt_text.eval(|| {
            let runtime = self.runtime()?;
            Ok(Arc::<str>::from(runtime.format_prompt_text(
                &self.prompt_text,
                self.options.prompt_format,
            )))
        })
    }

    fn prompt_token_ids(&self) -> Result<Arc<[u32]>, String> {
        self.prompt_token_ids.eval(|| {
            let runtime = self.runtime()?;
            let formatted_prompt = self.formatted_prompt_text()?;
            runtime.tokenize_prompt(formatted_prompt.as_ref())
        })
    }

    fn generation_graph(&self) -> Result<Arc<ExactMetalGenerationGraph>, String> {
        self.generation_graph.eval(|| {
            let runtime = self.runtime()?;
            let prompt_token_ids = self.prompt_token_ids()?;
            let graph = runtime
                .start_generation_graph(prompt_token_ids.clone(), self.options.max_new_tokens)?;
            Ok(Arc::new(graph))
        })
    }

    fn generated_token_ids_up_to(&self, max_count: usize) -> Result<Arc<[u32]>, String> {
        self.generation_graph()?
            .generated_token_ids_up_to(max_count)
    }

    fn generation(&self) -> Result<Arc<GemmaTextGenerationOutput>, String> {
        self.generation.eval(|| {
            let runtime = self.runtime()?;
            let formatted_prompt_text = self.formatted_prompt_text()?;
            let prompt_token_ids = self.prompt_token_ids()?;
            let snapshot = self.generation_graph()?.finish_snapshot()?;
            let generated_token_ids = snapshot.generated_token_ids.clone();
            let stop_reason = match snapshot
                .stop_reason
                .ok_or_else(|| "generation graph completed without a stop reason".to_string())?
            {
                ExactMetalGenerationStopReason::MaxNewTokens => GemmaStopReason::MaxNewTokens,
                ExactMetalGenerationStopReason::EosToken(token_id) => {
                    GemmaStopReason::EosToken(token_id)
                }
            };
            let generated_text = if generated_token_ids.is_empty() {
                Arc::<str>::from("")
            } else {
                Arc::<str>::from(
                    runtime
                        .tokenizer
                        .decode(generated_token_ids.as_ref())
                        .map_err(|err| err.to_string())?,
                )
            };
            Ok(Arc::new(GemmaTextGenerationOutput {
                model_path: runtime.model_path.clone(),
                prompt_text: self.prompt_text.clone(),
                formatted_prompt_text,
                prompt_token_ids,
                generated_token_ids,
                generated_text,
                stop_reason,
            }))
        })
    }
}

pub fn lazy_text_plan(
    model_path: PathBuf,
    prompt_text: impl Into<String>,
    options: GemmaTextGenerationOptions,
) -> GemmaLazyTextPlan {
    GemmaLazyTextPlan::from_prompt(model_path, prompt_text, options)
}

pub fn generate_text(
    model_path: PathBuf,
    prompt_text: impl Into<String>,
    options: GemmaTextGenerationOptions,
) -> Result<Arc<GemmaTextGenerationOutput>, Box<dyn Error>> {
    lazy_text_plan(model_path, prompt_text, options).eval_generate()
}

pub fn benchmark_text_generation(
    model_path: PathBuf,
    prompt_text: impl Into<String>,
    options: GemmaTextGenerationOptions,
    warmup_iters: usize,
    measured_iters: usize,
) -> Result<GemmaTextBenchmarkOutput, Box<dyn Error>> {
    if measured_iters == 0 {
        return Err("benchmark requires at least one measured iteration".into());
    }

    let prompt_text = Arc::<str>::from(prompt_text.into());
    let load_started = Instant::now();
    let runtime = GemmaTextRuntimeSession::load(&model_path).map_err(|err| err.to_string())?;
    let load_duration = load_started.elapsed();
    let formatted_prompt_text =
        Arc::<str>::from(runtime.format_prompt_text(prompt_text.as_ref(), options.prompt_format));
    let prompt_token_ids = runtime.tokenize_prompt(formatted_prompt_text.as_ref())?;

    for _ in 0..warmup_iters {
        runtime
            .start_generation_graph(prompt_token_ids.clone(), options.max_new_tokens)?
            .finish_snapshot()?;
    }
    runtime
        .exact_backend
        .lock()
        .map_err(|_| "exact backend mutex poisoned".to_string())?
        .reset_runtime_counters();

    let started = Instant::now();
    let mut total_generated_tokens = 0usize;
    let mut time_to_first_token_elapsed = Duration::ZERO;
    let mut steady_state_elapsed = Duration::ZERO;
    let mut steady_state_generated_tokens = 0usize;
    let mut last_generated_token_ids = Arc::<[u32]>::from(Vec::<u32>::new());
    for _ in 0..measured_iters {
        let ttft_started = Instant::now();
        let graph =
            runtime.start_generation_graph(prompt_token_ids.clone(), options.max_new_tokens)?;
        let first_generated_token_ids = graph.generated_token_ids_up_to(1)?;
        time_to_first_token_elapsed += ttft_started.elapsed();

        let steady_started = Instant::now();
        let snapshot = graph.finish_snapshot()?;
        steady_state_elapsed += steady_started.elapsed();
        total_generated_tokens += snapshot.generated_token_ids.len();
        steady_state_generated_tokens += snapshot
            .generated_token_ids
            .len()
            .saturating_sub(first_generated_token_ids.len());
        last_generated_token_ids = snapshot.generated_token_ids.clone();
    }
    let elapsed = started.elapsed();
    let last_generated_text = if last_generated_token_ids.is_empty() {
        Arc::<str>::from("")
    } else {
        Arc::<str>::from(
            runtime
                .tokenizer
                .decode(last_generated_token_ids.as_ref())
                .map_err(|err| err.to_string())?,
        )
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
    let metal_counters = runtime
        .exact_backend
        .lock()
        .map_err(|_| "exact backend mutex poisoned".to_string())?
        .runtime_counters();

    Ok(GemmaTextBenchmarkOutput {
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
        metal_counters,
        prompt_prefill_tokens_per_second,
        steady_state_decode_tokens_per_second,
        decode_tokens_per_second,
        total_tokens_per_second,
    })
}

impl GemmaTextRuntimeSession {
    fn load(model_path: &Path) -> Result<Arc<Self>, String> {
        let model_root = model_root_dir(model_path).map_err(|err| err.to_string())?;
        let weights = MlxIndexedSafetensors::load(&model_root).map_err(|err| err.to_string())?;
        let tokenizer =
            MlxTokenizer::from_snapshot(&weights.snapshot).map_err(|err| err.to_string())?;
        let config = &weights.snapshot.config.text_config;
        if config.hidden_size_per_layer_input != 0 {
            return Err("text runtime does not yet support per-layer input embeddings".to_string());
        }
        if config.num_kv_shared_layers != 0 {
            return Err("text runtime does not yet support KV-shared Gemma variants".to_string());
        }
        let kv_layout =
            GemmaKvCacheLayout::from_text_config(config, 1).map_err(|err| err.to_string())?;
        let stop_tokens = weights
            .snapshot
            .generation_config
            .eos_token_id
            .iter()
            .copied()
            .collect::<BTreeSet<_>>();
        Ok(Arc::new(Self {
            model_path: model_path.to_path_buf(),
            weights,
            tokenizer,
            kv_layout,
            stop_tokens,
            exact_backend: Arc::new(Mutex::new(
                ExactMetalTextRuntimeSession::load(model_path.to_path_buf())
                    .map_err(|err| err.to_string())?,
            )),
        }))
    }

    fn format_prompt_text(&self, prompt_text: &str, prompt_format: GemmaPromptFormat) -> String {
        match prompt_format {
            GemmaPromptFormat::RawBos => format!(
                "{}{}",
                self.weights.snapshot.tokenizer_config.bos_token, prompt_text
            ),
            GemmaPromptFormat::Gemma4UserTurn => format!(
                "{}{}user\n{}{}\n{}model\n",
                self.weights.snapshot.tokenizer_config.bos_token,
                self.weights.snapshot.tokenizer_config.sot_token,
                prompt_text,
                self.weights.snapshot.tokenizer_config.eot_token,
                self.weights.snapshot.tokenizer_config.sot_token,
            ),
        }
    }

    fn tokenize_prompt(&self, formatted_prompt: &str) -> Result<Arc<[u32]>, String> {
        let ids = self
            .tokenizer
            .encode(formatted_prompt)
            .map_err(|err| err.to_string())?;
        if ids.is_empty() {
            return Err("formatted prompt encoded to zero tokens".to_string());
        }
        Ok(Arc::<[u32]>::from(ids))
    }

    #[cfg(test)]
    fn start_generation_cursor(
        self: &Arc<Self>,
        prompt_token_ids: Arc<[u32]>,
        max_new_tokens: usize,
    ) -> Result<ExactMetalGenerationCursor, String> {
        ExactMetalTextRuntimeSession::generation_cursor(
            self.exact_backend.clone(),
            prompt_token_ids,
            self.stop_tokens.clone(),
            max_new_tokens,
        )
        .map_err(|err| err.to_string())
    }

    fn start_generation_graph(
        self: &Arc<Self>,
        prompt_token_ids: Arc<[u32]>,
        max_new_tokens: usize,
    ) -> Result<ExactMetalGenerationGraph, String> {
        ExactMetalTextRuntimeSession::generation_graph(
            self.exact_backend.clone(),
            prompt_token_ids,
            self.stop_tokens.clone(),
            max_new_tokens,
        )
        .map_err(|err| err.to_string())
    }
}

#[cfg(test)]
impl GemmaTextRuntimeSession {
    fn greedy_token_from_hidden(&self, hidden_words: &[u16]) -> Result<MlxGreedyToken, String> {
        let final_norm_words = self
            .weights
            .final_text_norm_bf16_words(hidden_words)
            .map_err(|err| err.to_string())?;
        self.weights
            .tied_text_logits_top1_f32(&final_norm_words)
            .map_err(|err| err.to_string())
    }
}

#[cfg(test)]
impl GemmaTextRuntimeSession {
    fn eval_token_hidden_state(
        &self,
        token_id: u32,
        position: usize,
        caches: &mut GemmaKvCacheSet<f32>,
    ) -> Result<Vec<u16>, String> {
        let mut hidden_words = self
            .weights
            .embed_token_bf16_words(token_id)
            .map_err(|err| err.to_string())?;
        for layer_idx in 0..self.weights.snapshot.config.text_config.num_hidden_layers as usize {
            hidden_words =
                self.eval_layer_hidden_state(layer_idx, &hidden_words, position, caches)?;
        }
        Ok(hidden_words)
    }
    fn eval_layer_hidden_state(
        &self,
        layer_idx: usize,
        input_words: &[u16],
        position: usize,
        caches: &mut GemmaKvCacheSet<f32>,
    ) -> Result<Vec<u16>, String> {
        let config = &self.weights.snapshot.config.text_config;
        let layer_type = config
            .layer_types
            .get(layer_idx)
            .ok_or_else(|| format!("missing text layer type for layer {layer_idx}"))?;
        let attention_k_eq_v = config.attention_k_eq_v && layer_type == "full_attention";
        let head_dim = if layer_type == "full_attention" {
            config.global_head_dim as usize
        } else {
            config.head_dim as usize
        };
        let k_head_count = if attention_k_eq_v && layer_type == "full_attention" {
            config.num_global_key_value_heads as usize
        } else {
            config.num_key_value_heads as usize
        };
        let q_head_count = config.num_attention_heads as usize;
        let v_head_count = k_head_count;
        if k_head_count == 0 || q_head_count == 0 || q_head_count % k_head_count != 0 {
            return Err(format!(
                "invalid Gemma attention head layout for layer {layer_idx}: q={q_head_count} kv={k_head_count}"
            ));
        }
        let q_heads_per_kv = q_head_count / k_head_count;
        let rope_params = if layer_type == "full_attention" {
            &config.rope_parameters.full_attention
        } else {
            &config.rope_parameters.sliding_attention
        };
        let rope_rotary_dim = if let Some(partial_factor) = rope_params.partial_rotary_factor {
            let rotary_dim = (head_dim as f32 * partial_factor).round() as usize;
            if rotary_dim == 0 || rotary_dim > head_dim || rotary_dim % 2 != 0 {
                return Err(format!(
                    "invalid rope rotary dim {} for layer {} head_dim {} factor {}",
                    rotary_dim, layer_idx, head_dim, partial_factor
                ));
            }
            rotary_dim
        } else {
            head_dim
        };
        let rope = RopeSpec {
            head_dim,
            rotary_dim: rope_rotary_dim,
            base: rope_params.rope_theta,
        };
        let names = TextLayerTensorNames::for_layer(layer_idx, attention_k_eq_v);

        let input_norm =
            rms_norm_weighted_tensor(&self.weights, input_words, &names.input_norm_weight_name)?;
        let input_norm_words = f32s_to_bf16_words(&input_norm);

        let q_raw = quantized_matmul_tensor(
            &self.weights,
            &input_norm_words,
            &names.q.weight_name,
            &names.q.scales_name,
            &names.q.biases_name,
        )?;
        let q_norm_weight_name = names
            .q
            .norm_weight_name
            .as_deref()
            .ok_or_else(|| format!("missing q norm weight name for layer {layer_idx}"))?;
        let q_norm_weights = self
            .weights
            .read_bf16_tensor_words(q_norm_weight_name)
            .map_err(|err| err.to_string())?;
        let mut q_norm =
            rms_norm_rows_weighted_f32(&q_raw, q_head_count, head_dim, &q_norm_weights)?;
        apply_rope_rows_in_place(&mut q_norm, q_head_count, rope, position)?;

        let k_raw = quantized_matmul_tensor(
            &self.weights,
            &input_norm_words,
            &names.k.weight_name,
            &names.k.scales_name,
            &names.k.biases_name,
        )?;
        let k_norm_weight_name = names
            .k
            .norm_weight_name
            .as_deref()
            .ok_or_else(|| format!("missing k norm weight name for layer {layer_idx}"))?;
        let k_norm_weights = self
            .weights
            .read_bf16_tensor_words(k_norm_weight_name)
            .map_err(|err| err.to_string())?;
        let mut k_norm =
            rms_norm_rows_weighted_f32(&k_raw, k_head_count, head_dim, &k_norm_weights)?;
        apply_rope_rows_in_place(&mut k_norm, k_head_count, rope, position)?;

        let v_raw = if attention_k_eq_v {
            k_raw
        } else {
            quantized_matmul_tensor(
                &self.weights,
                &input_norm_words,
                &names.v.weight_name,
                &names.v.scales_name,
                &names.v.biases_name,
            )?
        };
        let v_norm =
            rms_norm_rows_no_scale_f32(&v_raw, v_head_count, head_dim, config.rms_norm_eps)?;

        let k_tensor =
            single_token_tensor(k_head_count, head_dim, k_norm).map_err(|err| err.to_string())?;
        let v_tensor =
            single_token_tensor(v_head_count, head_dim, v_norm).map_err(|err| err.to_string())?;
        let layer_cache = caches
            .cache_for_layer_mut(layer_idx)
            .map_err(|err| err.to_string())?;
        layer_cache
            .update_and_fetch(k_tensor.view(), v_tensor.view())
            .map_err(|err| err.to_string())?;

        let attention_out = compute_attention_output_f32(
            &q_norm,
            layer_cache,
            q_head_count,
            q_heads_per_kv,
            head_dim,
        )
        .map_err(|err| err.to_string())?;
        let attention_out_words = f32s_to_bf16_words(&attention_out);
        let attention_oproj = quantized_matmul_tensor(
            &self.weights,
            &attention_out_words,
            &names.o.weight_name,
            &names.o.scales_name,
            &names.o.biases_name,
        )?;
        let attention_oproj_words = f32s_to_bf16_words(&attention_oproj);
        let post_attention_norm = rms_norm_weighted_tensor(
            &self.weights,
            &attention_oproj_words,
            &names.post_attention_norm_weight_name,
        )?;
        let post_attention_residual = add_bf16_and_f32(input_words, &post_attention_norm)?;
        let post_attention_residual_words = f32s_to_bf16_words(&post_attention_residual);

        let pre_feedforward_norm = rms_norm_weighted_tensor(
            &self.weights,
            &post_attention_residual_words,
            &names.pre_feedforward_norm_weight_name,
        )?;
        let pre_feedforward_norm_words = f32s_to_bf16_words(&pre_feedforward_norm);
        let dense_gate = quantized_matmul_tensor(
            &self.weights,
            &pre_feedforward_norm_words,
            &names.mlp_gate_weight_name,
            &names.mlp_gate_scales_name,
            &names.mlp_gate_biases_name,
        )?;
        let dense_up = quantized_matmul_tensor(
            &self.weights,
            &pre_feedforward_norm_words,
            &names.mlp_up_weight_name,
            &names.mlp_up_scales_name,
            &names.mlp_up_biases_name,
        )?;
        let dense_geglu = geglu_f32(&dense_gate, &dense_up)?;
        let dense_geglu_words = f32s_to_bf16_words(&dense_geglu);
        let dense_down = quantized_matmul_tensor(
            &self.weights,
            &dense_geglu_words,
            &names.mlp_down_weight_name,
            &names.mlp_down_scales_name,
            &names.mlp_down_biases_name,
        )?;

        let feedforward_out = if config.enable_moe_block {
            let dense_down_words = f32s_to_bf16_words(&dense_down);
            let dense_branch = rms_norm_weighted_tensor(
                &self.weights,
                &dense_down_words,
                &names.post_feedforward_norm1_weight_name,
            )?;
            let router = gemma_router_topk_from_residual_bf16(
                &self.weights,
                &post_attention_residual_words,
                &names.router_scale_name,
                &names.router_per_expert_scale_name,
                &names.router_proj_weight_name,
                &names.router_proj_scales_name,
                &names.router_proj_biases_name,
                config.rms_norm_eps,
                config.top_k_experts as usize,
            )?;
            let moe = gemma_moe_expert_block_from_residual_bf16(
                &self.weights,
                &post_attention_residual_words,
                &names.pre_feedforward_norm2_weight_name,
                &names.expert_gate_weight_name,
                &names.expert_gate_scales_name,
                &names.expert_gate_biases_name,
                &names.expert_up_weight_name,
                &names.expert_up_scales_name,
                &names.expert_up_biases_name,
                &names.expert_down_weight_name,
                &names.expert_down_scales_name,
                &names.expert_down_biases_name,
                &router.top_k_indices,
                &router.top_k_weights,
            )?;
            let moe_out_words = f32s_to_bf16_words(&moe.expert_out);
            let moe_branch = rms_norm_weighted_tensor(
                &self.weights,
                &moe_out_words,
                &names.post_feedforward_norm2_weight_name,
            )?;
            let merged = add_f32(&dense_branch, &moe_branch)?;
            let merged_words = f32s_to_bf16_words(&merged);
            rms_norm_weighted_tensor(
                &self.weights,
                &merged_words,
                &names.post_feedforward_norm_weight_name,
            )?
        } else {
            let dense_down_words = f32s_to_bf16_words(&dense_down);
            rms_norm_weighted_tensor(
                &self.weights,
                &dense_down_words,
                &names.post_feedforward_norm_weight_name,
            )?
        };

        let mut output = add_f32(&post_attention_residual, &feedforward_out)?;
        if let Some(layer_scalar) =
            load_optional_scalar_f32(&self.weights, &names.layer_scalar_name)?
        {
            scale_in_place(&mut output, layer_scalar);
        }
        Ok(f32s_to_bf16_words(&output))
    }
}

#[cfg(test)]
fn rms_norm_weighted_tensor(
    weights: &MlxIndexedSafetensors,
    input_words: &[u16],
    weight_name: &str,
) -> Result<Vec<f32>, String> {
    weights
        .header_for_tensor(weight_name)
        .map_err(|err| err.to_string())?
        .rms_norm_weighted_f32(
            input_words,
            weight_name,
            weights.snapshot.config.text_config.rms_norm_eps,
        )
        .map_err(|err| err.to_string())
}

#[cfg(test)]
fn quantized_matmul_tensor(
    weights: &MlxIndexedSafetensors,
    input_words: &[u16],
    weight_name: &str,
    scales_name: &str,
    biases_name: &str,
) -> Result<Vec<f32>, String> {
    let bits = weights.snapshot.config.quantization.bits;
    let group_size = weights.snapshot.config.quantization.group_size as u64;
    if bits == 0 || bits > 8 || (bits & (bits - 1)) != 0 {
        return Err(format!("unsupported affine quantized matmul bits {bits}"));
    }

    let weight_entry = weights.tensor(weight_name).map_err(|err| err.to_string())?;
    let scales_entry = weights.tensor(scales_name).map_err(|err| err.to_string())?;
    let biases_entry = weights.tensor(biases_name).map_err(|err| err.to_string())?;
    if weight_entry.dtype != MlxDType::U32 {
        return Err(format!(
            "tensor {weight_name} expected U32, got {:?}",
            weight_entry.dtype
        ));
    }
    if scales_entry.dtype != MlxDType::BF16 || biases_entry.dtype != MlxDType::BF16 {
        return Err(format!(
            "tensors {scales_name} / {biases_name} expected BF16, got {:?} / {:?}",
            scales_entry.dtype, biases_entry.dtype
        ));
    }
    if weight_entry.shape.len() != 2
        || scales_entry.shape.len() != 2
        || biases_entry.shape.len() != 2
    {
        return Err(format!(
            "quantized matmul expects rank-2 tensors, got {:?} {:?} {:?}",
            weight_entry.shape, scales_entry.shape, biases_entry.shape
        ));
    }
    if scales_entry.shape != biases_entry.shape {
        return Err(format!(
            "scale/bias shape mismatch: {:?} vs {:?}",
            scales_entry.shape, biases_entry.shape
        ));
    }
    if weight_entry.shape[0] != scales_entry.shape[0] {
        return Err(format!(
            "weight/scales outer shape mismatch: {:?} vs {:?}",
            weight_entry.shape, scales_entry.shape
        ));
    }

    let values_per_word = 32 / bits as u64;
    let inner_dim = weight_entry.shape[1] * values_per_word;
    if inner_dim != scales_entry.shape[1] * group_size {
        return Err(format!(
            "packed/scales shape mismatch for group_size={group_size} bits={bits}"
        ));
    }
    if input_words.len() as u64 != inner_dim {
        return Err(format!(
            "activation length mismatch: got {} expected {inner_dim}",
            input_words.len()
        ));
    }
    let words_per_group = group_size / values_per_word;
    if words_per_group == 0 || weight_entry.shape[1] != scales_entry.shape[1] * words_per_group {
        return Err(format!("invalid words_per_group {words_per_group}"));
    }

    let packed_weights = weights
        .header_for_tensor(weight_name)
        .map_err(|err| err.to_string())?
        .read_u32_tensor_words(weight_name)
        .map_err(|err| err.to_string())?;
    let scales = weights
        .header_for_tensor(scales_name)
        .map_err(|err| err.to_string())?
        .read_bf16_tensor_words(scales_name)
        .map_err(|err| err.to_string())?;
    let biases = weights
        .header_for_tensor(biases_name)
        .map_err(|err| err.to_string())?
        .read_bf16_tensor_words(biases_name)
        .map_err(|err| err.to_string())?;
    let x = input_words
        .iter()
        .copied()
        .map(bf16_word_to_f32)
        .collect::<Vec<_>>();
    let rows = weight_entry.shape[0] as usize;
    let weight_stride = weight_entry.shape[1] as usize;
    let groups_per_row = scales_entry.shape[1] as usize;
    let pack_factor = values_per_word as usize;
    let mask = (1u32 << bits) - 1;
    let mut out = Vec::with_capacity(rows);

    for row in 0..rows {
        let weight_row_start = row * weight_stride;
        let qparam_row_start = row * groups_per_row;
        let mut total = 0.0f32;
        let mut x_index = 0usize;
        for group in 0..groups_per_row {
            let scale = bf16_word_to_f32(scales[qparam_row_start + group]);
            let bias = bf16_word_to_f32(biases[qparam_row_start + group]);
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
            total += bf16_round_to_f32(scale * group_accum) + bf16_round_to_f32(bias * group_sum);
        }
        out.push(bf16_round_to_f32(total));
    }

    Ok(out)
}

#[cfg(test)]
fn quantized_matmul_rank3_plane_tensor(
    weights: &MlxIndexedSafetensors,
    input_words: &[u16],
    weight_name: &str,
    scales_name: &str,
    biases_name: &str,
    plane: u64,
) -> Result<Vec<f32>, String> {
    let bits = weights.snapshot.config.quantization.bits;
    let group_size = weights.snapshot.config.quantization.group_size as u64;
    if bits == 0 || bits > 8 || (bits & (bits - 1)) != 0 {
        return Err(format!("unsupported affine quantized matmul bits {bits}"));
    }

    let weight_entry = weights.tensor(weight_name).map_err(|err| err.to_string())?;
    let scales_entry = weights.tensor(scales_name).map_err(|err| err.to_string())?;
    let biases_entry = weights.tensor(biases_name).map_err(|err| err.to_string())?;
    if weight_entry.dtype != MlxDType::U32 {
        return Err(format!(
            "tensor {weight_name} expected U32, got {:?}",
            weight_entry.dtype
        ));
    }
    if scales_entry.dtype != MlxDType::BF16 || biases_entry.dtype != MlxDType::BF16 {
        return Err(format!(
            "tensors {scales_name} / {biases_name} expected BF16, got {:?} / {:?}",
            scales_entry.dtype, biases_entry.dtype
        ));
    }
    if weight_entry.shape.len() != 3
        || scales_entry.shape.len() != 3
        || biases_entry.shape.len() != 3
    {
        return Err(format!(
            "rank-3 affine quantized matmul expects rank-3 tensors, got {:?} {:?} {:?}",
            weight_entry.shape, scales_entry.shape, biases_entry.shape
        ));
    }
    if scales_entry.shape != biases_entry.shape {
        return Err(format!(
            "scale/bias shape mismatch: {:?} vs {:?}",
            scales_entry.shape, biases_entry.shape
        ));
    }
    if weight_entry.shape[0] != scales_entry.shape[0]
        || weight_entry.shape[1] != scales_entry.shape[1]
    {
        return Err(format!(
            "weight/scales outer shape mismatch: {:?} vs {:?}",
            weight_entry.shape, scales_entry.shape
        ));
    }
    if plane >= weight_entry.shape[0] {
        return Err(format!(
            "plane {plane} out of range for tensor {weight_name} with {} planes",
            weight_entry.shape[0]
        ));
    }

    let values_per_word = 32 / bits as u64;
    let inner_dim = weight_entry.shape[2] * values_per_word;
    if inner_dim != scales_entry.shape[2] * group_size {
        return Err(format!(
            "packed/scales plane shape mismatch for group_size={group_size} bits={bits}"
        ));
    }
    if input_words.len() as u64 != inner_dim {
        return Err(format!(
            "activation length mismatch: got {} expected {inner_dim}",
            input_words.len()
        ));
    }
    let words_per_group = group_size / values_per_word;
    if words_per_group == 0 || weight_entry.shape[2] != scales_entry.shape[2] * words_per_group {
        return Err(format!("invalid words_per_group {words_per_group}"));
    }

    let packed_weights = weights
        .header_for_tensor(weight_name)
        .map_err(|err| err.to_string())?
        .read_rank3_plane_u32_words(weight_name, plane)
        .map_err(|err| err.to_string())?;
    let scales = weights
        .header_for_tensor(scales_name)
        .map_err(|err| err.to_string())?
        .read_rank3_plane_bf16_words(scales_name, plane)
        .map_err(|err| err.to_string())?;
    let biases = weights
        .header_for_tensor(biases_name)
        .map_err(|err| err.to_string())?
        .read_rank3_plane_bf16_words(biases_name, plane)
        .map_err(|err| err.to_string())?;
    let x = input_words
        .iter()
        .copied()
        .map(bf16_word_to_f32)
        .collect::<Vec<_>>();
    let rows = weight_entry.shape[1] as usize;
    let weight_stride = weight_entry.shape[2] as usize;
    let groups_per_row = scales_entry.shape[2] as usize;
    let pack_factor = values_per_word as usize;
    let mask = (1u32 << bits) - 1;
    let mut out = Vec::with_capacity(rows);

    for row in 0..rows {
        let weight_row_start = row * weight_stride;
        let qparam_row_start = row * groups_per_row;
        let mut sum = 0.0f32;
        let mut x_index = 0usize;
        for group in 0..groups_per_row {
            let scale = bf16_word_to_f32(scales[qparam_row_start + group]);
            let bias = bf16_word_to_f32(biases[qparam_row_start + group]);
            let group_start = weight_row_start + group * words_per_group as usize;
            let group_end = group_start + words_per_group as usize;
            for packed in &packed_weights[group_start..group_end] {
                let mut packed_word = *packed;
                for _ in 0..pack_factor {
                    let q = (packed_word & mask) as f32;
                    let deq_mul = bf16_round_to_f32(scale * q);
                    let deq = bf16_round_to_f32(deq_mul + bias);
                    let prod = bf16_round_to_f32(x[x_index] * deq);
                    sum = bf16_round_to_f32(sum + prod);
                    x_index += 1;
                    if bits != 8 {
                        packed_word >>= bits;
                    }
                }
            }
        }
        out.push(sum);
    }

    Ok(out)
}

#[cfg(test)]
fn gemma_router_topk_from_residual_bf16(
    weights: &MlxIndexedSafetensors,
    residual_bf16_words: &[u16],
    router_scale_name: &str,
    per_expert_scale_name: &str,
    proj_weight_name: &str,
    proj_scales_name: &str,
    proj_biases_name: &str,
    eps: f32,
    top_k: usize,
) -> Result<MlxRouterTopKOutput, String> {
    if top_k == 0 {
        return Err("router top_k must be greater than zero".to_string());
    }

    let hidden = residual_bf16_words.len();
    let router_scale_words = weights
        .read_bf16_tensor_words(router_scale_name)
        .map_err(|err| err.to_string())?;
    if router_scale_words.len() != hidden {
        return Err(format!(
            "router scale length mismatch: got {} expected {}",
            router_scale_words.len(),
            hidden
        ));
    }
    let per_expert_scale_words = weights
        .read_bf16_tensor_words(per_expert_scale_name)
        .map_err(|err| err.to_string())?;
    if top_k > per_expert_scale_words.len() {
        return Err(format!(
            "router top_k {top_k} exceeds num_experts {}",
            per_expert_scale_words.len()
        ));
    }

    let residual = residual_bf16_words
        .iter()
        .copied()
        .map(bf16_word_to_f32)
        .collect::<Vec<_>>();
    let mut mean_square = 0.0f32;
    for value in &residual {
        mean_square += value * value;
    }
    mean_square /= hidden as f32;
    let inv_rms = 1.0f32 / (mean_square + eps).sqrt();

    let root_size = bf16_round_to_f32((hidden as f32).powf(-0.5));
    let mut router_scaled = Vec::with_capacity(hidden);
    let mut router_scaled_words = Vec::with_capacity(hidden);
    for (index, value) in residual.iter().copied().enumerate() {
        let normed = bf16_round_to_f32(value * inv_rms);
        let scaled_root = bf16_round_to_f32(normed * root_size);
        let scaled = bf16_round_to_f32(scaled_root * bf16_word_to_f32(router_scale_words[index]));
        router_scaled.push(scaled);
        router_scaled_words.push(f32_to_bf16_word(scaled));
    }

    let expert_scores = quantized_matmul_tensor(
        weights,
        &router_scaled_words,
        proj_weight_name,
        proj_scales_name,
        proj_biases_name,
    )?;
    if expert_scores.is_empty() {
        return Err("router projection produced no scores".to_string());
    }
    if top_k > expert_scores.len() {
        return Err(format!(
            "router top_k {top_k} exceeds expert_scores length {}",
            expert_scores.len()
        ));
    }

    let max_score = expert_scores
        .iter()
        .copied()
        .fold(f32::NEG_INFINITY, f32::max);
    let exp_scores = expert_scores
        .iter()
        .copied()
        .map(|value| (value - max_score).exp())
        .collect::<Vec<_>>();
    let exp_sum = exp_scores.iter().copied().sum::<f32>();
    let router_probs = exp_scores
        .iter()
        .copied()
        .map(|value| bf16_round_to_f32(value / exp_sum))
        .collect::<Vec<_>>();

    let mut indices = (0..expert_scores.len()).collect::<Vec<_>>();
    indices.sort_by(|&lhs, &rhs| {
        expert_scores[rhs]
            .total_cmp(&expert_scores[lhs])
            .then_with(|| lhs.cmp(&rhs))
    });
    let top_k_indices = indices
        .into_iter()
        .take(top_k)
        .map(|index| index as u32)
        .collect::<Vec<_>>();

    let mut top_k_weights = top_k_indices
        .iter()
        .copied()
        .map(|index| router_probs[index as usize])
        .collect::<Vec<_>>();
    let mut top_k_sum = 0.0f32;
    for weight in &top_k_weights {
        top_k_sum = bf16_round_to_f32(top_k_sum + *weight);
    }
    for (slot, weight) in top_k_weights.iter_mut().enumerate() {
        let normalized = bf16_round_to_f32(*weight / top_k_sum);
        let expert_scale = bf16_word_to_f32(per_expert_scale_words[top_k_indices[slot] as usize]);
        *weight = bf16_round_to_f32(normalized * expert_scale);
    }

    Ok(MlxRouterTopKOutput {
        router_scaled,
        expert_scores,
        router_probs,
        top_k_indices,
        top_k_weights,
    })
}

#[cfg(test)]
fn gemma_moe_expert_block_from_residual_bf16(
    weights: &MlxIndexedSafetensors,
    residual_bf16_words: &[u16],
    pre_feedforward_norm2_weight_name: &str,
    expert_gate_weight_name: &str,
    expert_gate_scales_name: &str,
    expert_gate_biases_name: &str,
    expert_up_weight_name: &str,
    expert_up_scales_name: &str,
    expert_up_biases_name: &str,
    expert_down_weight_name: &str,
    expert_down_scales_name: &str,
    expert_down_biases_name: &str,
    top_k_indices: &[u32],
    top_k_weights: &[f32],
) -> Result<MlxGemmaMoeExpertOutput, String> {
    if top_k_indices.is_empty() {
        return Err("moe expert path needs at least one routed expert".to_string());
    }
    if top_k_indices.len() != top_k_weights.len() {
        return Err(format!(
            "top_k index/weight length mismatch: {} vs {}",
            top_k_indices.len(),
            top_k_weights.len()
        ));
    }

    let pre_feedforward_norm2 = rms_norm_weighted_tensor(
        weights,
        residual_bf16_words,
        pre_feedforward_norm2_weight_name,
    )?;
    let pre_feedforward_norm2_words = pre_feedforward_norm2
        .iter()
        .copied()
        .map(f32_to_bf16_word)
        .collect::<Vec<_>>();

    let mut gate_proj = Vec::new();
    let mut up_proj = Vec::new();
    let mut geglu = Vec::new();
    let mut down_proj = Vec::new();
    let hidden = residual_bf16_words.len();

    for &expert_index in top_k_indices {
        let gate_row = quantized_matmul_rank3_plane_tensor(
            weights,
            &pre_feedforward_norm2_words,
            expert_gate_weight_name,
            expert_gate_scales_name,
            expert_gate_biases_name,
            expert_index as u64,
        )?;
        let up_row = quantized_matmul_rank3_plane_tensor(
            weights,
            &pre_feedforward_norm2_words,
            expert_up_weight_name,
            expert_up_scales_name,
            expert_up_biases_name,
            expert_index as u64,
        )?;
        if gate_row.len() != up_row.len() {
            return Err(format!(
                "expert {expert_index} gate/up output length mismatch: {} vs {}",
                gate_row.len(),
                up_row.len()
            ));
        }

        let mut geglu_row = Vec::with_capacity(gate_row.len());
        for (&gate, &up) in gate_row.iter().zip(up_row.iter()) {
            geglu_row.push(bf16_round_to_f32(gelu_approx_f32(gate) * up));
        }
        let geglu_words = geglu_row
            .iter()
            .copied()
            .map(f32_to_bf16_word)
            .collect::<Vec<_>>();
        let down_row = quantized_matmul_rank3_plane_tensor(
            weights,
            &geglu_words,
            expert_down_weight_name,
            expert_down_scales_name,
            expert_down_biases_name,
            expert_index as u64,
        )?;
        if down_row.len() != hidden {
            return Err(format!(
                "expert {expert_index} down projection length mismatch: got {} expected {hidden}",
                down_row.len()
            ));
        }

        gate_proj.extend_from_slice(&gate_row);
        up_proj.extend_from_slice(&up_row);
        geglu.extend_from_slice(&geglu_row);
        down_proj.extend_from_slice(&down_row);
    }

    let mut expert_out = vec![0.0f32; hidden];
    for (expert_slot, &weight) in top_k_weights.iter().enumerate() {
        for hidden_index in 0..hidden {
            let weighted =
                bf16_round_to_f32(down_proj[expert_slot * hidden + hidden_index] * weight);
            expert_out[hidden_index] = bf16_round_to_f32(expert_out[hidden_index] + weighted);
        }
    }

    Ok(MlxGemmaMoeExpertOutput {
        pre_feedforward_norm2,
        gate_proj,
        up_proj,
        geglu,
        down_proj,
        expert_out,
    })
}

#[cfg(test)]
fn load_optional_scalar_f32(
    weights: &MlxIndexedSafetensors,
    tensor_name: &str,
) -> Result<Option<f32>, String> {
    let tensor = match weights.tensor(tensor_name) {
        Ok(tensor) => tensor,
        Err(_) => return Ok(None),
    };
    if tensor.shape.iter().product::<u64>() != 1 {
        return Err(format!("layer scalar tensor {} is not scalar", tensor_name));
    }
    let words = weights
        .read_bf16_tensor_words(tensor_name)
        .map_err(|err| err.to_string())?;
    let word = words
        .first()
        .copied()
        .ok_or_else(|| format!("layer scalar tensor {} is empty", tensor_name))?;
    Ok(Some(bf16_word_to_f32(word)))
}

#[cfg(test)]
fn rms_norm_rows_weighted_f32(
    input: &[f32],
    row_count: usize,
    row_len: usize,
    weight_words: &[u16],
) -> Result<Vec<f32>, String> {
    if input.len() != row_count * row_len {
        return Err(format!(
            "row RMS input length mismatch: got {} expected {}",
            input.len(),
            row_count * row_len
        ));
    }
    if weight_words.len() != row_len {
        return Err(format!(
            "row RMS weight length mismatch: got {} expected {}",
            weight_words.len(),
            row_len
        ));
    }
    let weights = weight_words
        .iter()
        .copied()
        .map(bf16_word_to_f32)
        .collect::<Vec<_>>();
    let mut out = Vec::with_capacity(input.len());
    for row_idx in 0..row_count {
        let row = &input[row_idx * row_len..(row_idx + 1) * row_len];
        let inv_rms = inv_rms_f32(row, 1e-6f32);
        for (index, value) in row.iter().copied().enumerate() {
            let normalized = bf16_round_to_f32(value * inv_rms);
            out.push(bf16_round_to_f32(normalized * weights[index]));
        }
    }
    Ok(out)
}

#[cfg(test)]
fn rms_norm_rows_no_scale_f32(
    input: &[f32],
    row_count: usize,
    row_len: usize,
    eps: f32,
) -> Result<Vec<f32>, String> {
    if input.len() != row_count * row_len {
        return Err(format!(
            "row RMS no-scale input length mismatch: got {} expected {}",
            input.len(),
            row_count * row_len
        ));
    }
    let mut out = Vec::with_capacity(input.len());
    for row_idx in 0..row_count {
        let row = &input[row_idx * row_len..(row_idx + 1) * row_len];
        let inv_rms = inv_rms_f32(row, eps);
        for value in row {
            out.push(bf16_round_to_f32(*value * inv_rms));
        }
    }
    Ok(out)
}

#[cfg(test)]
fn apply_rope_rows_in_place(
    values: &mut [f32],
    row_count: usize,
    rope: RopeSpec,
    position: usize,
) -> Result<(), String> {
    if values.len() != row_count * rope.head_dim {
        return Err(format!(
            "rope row length mismatch: got {} expected {}",
            values.len(),
            row_count * rope.head_dim
        ));
    }
    if rope.rotary_dim == 0 || rope.rotary_dim > rope.head_dim || rope.rotary_dim % 2 != 0 {
        return Err(format!(
            "invalid rope rotary dimension {} for head_dim {}",
            rope.rotary_dim, rope.head_dim
        ));
    }
    let half = rope.head_dim / 2;
    let rotary_pairs = rope.rotary_dim / 2;
    if rotary_pairs > half {
        return Err(format!(
            "rope rotary pair count {} exceeds half-head {}",
            rotary_pairs, half
        ));
    }
    for row_idx in 0..row_count {
        let row = &mut values[row_idx * rope.head_dim..(row_idx + 1) * rope.head_dim];
        for pair_idx in 0..rotary_pairs {
            let exponent = (2.0f32 * pair_idx as f32) / rope.head_dim as f32;
            let inv_freq = rope.base.powf(-exponent);
            let theta = position as f32 * inv_freq;
            let cos_theta = theta.cos();
            let sin_theta = theta.sin();
            let left = row[pair_idx];
            let right = row[half + pair_idx];
            row[pair_idx] = bf16_round_to_f32(left * cos_theta - right * sin_theta);
            row[half + pair_idx] = bf16_round_to_f32(left * sin_theta + right * cos_theta);
        }
    }
    Ok(())
}

#[cfg(test)]
fn compute_attention_output_f32(
    q_values: &[f32],
    cache: &crate::GemmaKvCache<f32>,
    q_head_count: usize,
    q_heads_per_kv: usize,
    head_dim: usize,
) -> Result<Vec<f32>, Box<dyn Error>> {
    let cache_state = cache.fetch()?;
    let seq_len = cache_state.stored_tokens();
    let mut out = Vec::with_capacity(q_head_count * head_dim);

    for q_head in 0..q_head_count {
        let q_row = &q_values[q_head * head_dim..(q_head + 1) * head_dim];
        let kv_head = q_head / q_heads_per_kv;

        let mut scores = Vec::with_capacity(seq_len);
        for token in 0..seq_len {
            let k_row = cache_state.keys.row(0, kv_head, token)?;
            let mut sum = 0.0f32;
            for dim in 0..head_dim {
                sum += q_row[dim] * k_row[dim];
            }
            scores.push(bf16_round_to_f32(sum));
        }

        let max_score = scores.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let exp_scores = scores
            .iter()
            .copied()
            .map(|score| (score - max_score).exp())
            .collect::<Vec<_>>();
        let exp_sum = exp_scores.iter().copied().sum::<f32>();
        let probs = exp_scores
            .iter()
            .copied()
            .map(|score| bf16_round_to_f32(score / exp_sum))
            .collect::<Vec<_>>();

        for dim in 0..head_dim {
            let mut acc = 0.0f32;
            for (token, prob) in probs.iter().enumerate() {
                let v_row = cache_state.values.row(0, kv_head, token)?;
                acc = bf16_round_to_f32(acc + bf16_round_to_f32(*prob * v_row[dim]));
            }
            out.push(acc);
        }
    }

    Ok(out)
}

#[cfg(test)]
fn single_token_tensor(
    head_count: usize,
    head_dim: usize,
    values: Vec<f32>,
) -> crate::Result<KvTensor<f32>> {
    KvTensor::from_vec(
        KvTensorShape {
            batch_size: 1,
            kv_head_count: head_count,
            seq_len: 1,
            head_dim,
        },
        values,
    )
}

#[cfg(test)]
fn geglu_f32(gate: &[f32], up: &[f32]) -> Result<Vec<f32>, String> {
    if gate.len() != up.len() {
        return Err(format!(
            "GeGLU input length mismatch: gate={} up={}",
            gate.len(),
            up.len()
        ));
    }
    let mut out = Vec::with_capacity(gate.len());
    for (&gate_value, &up_value) in gate.iter().zip(up.iter()) {
        out.push(bf16_round_to_f32(gelu_approx_f32(gate_value) * up_value));
    }
    Ok(out)
}

#[cfg(test)]
fn gelu_approx_f32(value: f32) -> f32 {
    let squared = bf16_round_to_f32(value * value);
    let cubic = bf16_round_to_f32(squared * value);
    let poly = bf16_round_to_f32(value + bf16_round_to_f32(0.044_715f32 * cubic));
    let tanh_input = bf16_round_to_f32(0.797_884_6f32 * poly);
    let tanh_value = bf16_round_to_f32(tanh_input.tanh());
    let half = bf16_round_to_f32(0.5f32 * value);
    bf16_round_to_f32(half * bf16_round_to_f32(1.0f32 + tanh_value))
}

#[cfg(test)]
fn add_bf16_and_f32(left_words: &[u16], right: &[f32]) -> Result<Vec<f32>, String> {
    if left_words.len() != right.len() {
        return Err(format!(
            "add input length mismatch: left={} right={}",
            left_words.len(),
            right.len()
        ));
    }
    Ok(left_words
        .iter()
        .copied()
        .zip(right.iter().copied())
        .map(|(left, right)| bf16_round_to_f32(bf16_word_to_f32(left) + right))
        .collect())
}

#[cfg(test)]
fn add_f32(left: &[f32], right: &[f32]) -> Result<Vec<f32>, String> {
    if left.len() != right.len() {
        return Err(format!(
            "add input length mismatch: left={} right={}",
            left.len(),
            right.len()
        ));
    }
    Ok(left
        .iter()
        .copied()
        .zip(right.iter().copied())
        .map(|(left, right)| bf16_round_to_f32(left + right))
        .collect())
}

#[cfg(test)]
fn scale_in_place(values: &mut [f32], scale: f32) {
    for value in values {
        *value = bf16_round_to_f32(*value * scale);
    }
}

#[cfg(test)]
fn inv_rms_f32(values: &[f32], eps: f32) -> f32 {
    let mean_square = values
        .iter()
        .copied()
        .map(|value| value * value)
        .sum::<f32>()
        / values.len() as f32;
    1.0f32 / (mean_square + eps).sqrt()
}

#[cfg(test)]
fn f32s_to_bf16_words(values: &[f32]) -> Vec<u16> {
    values.iter().copied().map(f32_to_bf16_word).collect()
}

#[cfg(test)]
fn bf16_word_to_f32(word: u16) -> f32 {
    f32::from_bits((word as u32) << 16)
}

#[cfg(test)]
fn f32_to_bf16_word(value: f32) -> u16 {
    (bf16_round_to_f32(value).to_bits() >> 16) as u16
}

#[cfg(test)]
fn bf16_round_to_f32(value: f32) -> f32 {
    let bits = value.to_bits();
    let lsb = (bits >> 16) & 1;
    f32::from_bits(bits.wrapping_add(0x7FFF + lsb) & 0xFFFF_0000)
}

fn model_root_dir(model_path: &Path) -> Result<PathBuf, Box<dyn Error>> {
    if model_path.is_dir() {
        return Ok(model_path.to_path_buf());
    }
    model_path.parent().map(Path::to_path_buf).ok_or_else(|| {
        format!(
            "model path {} has no parent directory",
            model_path.display()
        )
        .into()
    })
}

#[cfg(test)]
mod tests {
    use super::{
        apply_rope_rows_in_place, compute_attention_output_f32, lazy_text_plan,
        quantized_matmul_tensor, rms_norm_rows_no_scale_f32, rms_norm_rows_weighted_f32,
        rms_norm_weighted_tensor, run_two_token_ids, run_two_token_prompt, single_token_tensor,
        GemmaPromptFormat, GemmaStopReason, GemmaTextGenerationOptions, GemmaTextRuntimeSession,
        GemmaTextStepOutput, RopeSpec, TextLayerTensorNames,
    };
    use crate::GemmaKvCacheSet;
    use makepad_mlx_rt_core::fnv1a64_u32_words;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::Arc;

    fn default_model_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../local/models/gemma-4-26b-mlx/model-00001-of-00003.safetensors")
    }

    fn assert_valid_step(output: &GemmaTextStepOutput) {
        assert_eq!(output.layers.len(), 30);
        assert_eq!(output.final_hidden_bf16_words.len(), 2_816);
        assert_eq!(output.final_norm_bf16_words.len(), 2_816);
        assert!(output.next_token.logit.is_finite());
        assert!(!output.next_token_text.is_empty());
    }

    #[test]
    #[ignore]
    fn two_token_prompt_executes_end_to_end() {
        let output = run_two_token_prompt(default_model_path(), "say hi").unwrap();
        assert_eq!(output.prompt_token_ids, vec![30_468, 5_631]);
        let layer_hashes = output
            .layers
            .iter()
            .map(|layer| {
                format!(
                    "0x{:016X}",
                    fnv1a64_u32_words(
                        layer
                            .layer_output_bits()
                            .expect("missing layer output bits in two-token step"),
                    )
                )
            })
            .collect::<Vec<_>>();
        let final_hidden_bits = output
            .final_hidden_bf16_words
            .iter()
            .map(|word| (*word as u32) << 16)
            .collect::<Vec<_>>();
        let final_norm_bits = output
            .final_norm_bf16_words
            .iter()
            .map(|word| (*word as u32) << 16)
            .collect::<Vec<_>>();
        println!("layer_post_ffn_residual_hashes={:?}", layer_hashes);
        println!(
            "final_hidden_fnv1a64=0x{:016X} final_norm_fnv1a64=0x{:016X}",
            fnv1a64_u32_words(&final_hidden_bits),
            fnv1a64_u32_words(&final_norm_bits)
        );
        println!(
            "next_token_id={} next_token_logit={} next_token_text={:?}",
            output.next_token.token_id, output.next_token.logit, output.next_token_text
        );
        assert_valid_step(&output);
    }

    #[test]
    #[ignore]
    fn two_token_prompt_writes_final_hidden_f32_for_mlx_oracle() {
        let output = run_two_token_prompt(default_model_path(), "say hi").unwrap();
        let dump_path = std::env::temp_dir().join("gemma_two_token_final_hidden_f32.bin");
        let mut bytes = Vec::with_capacity(output.final_hidden_bf16_words.len() * 4);
        for word in &output.final_hidden_bf16_words {
            let value = f32::from_bits((*word as u32) << 16);
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        fs::write(&dump_path, bytes).unwrap();
        let final_hidden_bits = output
            .final_hidden_bf16_words
            .iter()
            .map(|word| (*word as u32) << 16)
            .collect::<Vec<_>>();
        println!("final_hidden_dump_path={}", dump_path.display());
        println!(
            "final_hidden_fnv1a64=0x{:016X}",
            fnv1a64_u32_words(&final_hidden_bits)
        );
        assert_eq!(output.final_hidden_bf16_words.len(), 2_816);
    }

    #[test]
    #[ignore]
    fn host_runtime_reports_against_exact_raw_two_token_path() {
        let exact = run_two_token_prompt(default_model_path(), "say hi").unwrap();
        let runtime = GemmaTextRuntimeSession::load(&default_model_path()).unwrap();
        let runtime = Arc::unwrap_or_clone(runtime);
        let mut caches = GemmaKvCacheSet::<f32>::new(runtime.kv_layout.clone()).unwrap();
        let num_layers = runtime
            .weights
            .snapshot
            .config
            .text_config
            .num_hidden_layers as usize;

        let mut prefill_hidden = runtime
            .weights
            .embed_token_bf16_words(exact.prompt_token_ids[0])
            .unwrap();
        for layer_idx in 0..num_layers {
            prefill_hidden = runtime
                .eval_layer_hidden_state(layer_idx, &prefill_hidden, 0, &mut caches)
                .unwrap();
        }

        let mut host_hidden = runtime
            .weights
            .embed_token_bf16_words(exact.prompt_token_ids[1])
            .unwrap();
        let mut host_layer_hashes = Vec::with_capacity(num_layers);
        for layer_idx in 0..num_layers {
            host_hidden = runtime
                .eval_layer_hidden_state(layer_idx, &host_hidden, 1, &mut caches)
                .unwrap();
            let layer_bits = host_hidden
                .iter()
                .map(|word| (*word as u32) << 16)
                .collect::<Vec<_>>();
            host_layer_hashes.push(format!("0x{:016X}", fnv1a64_u32_words(&layer_bits)));
        }
        let host_hidden_bits = host_hidden
            .iter()
            .map(|word| (*word as u32) << 16)
            .collect::<Vec<_>>();
        let exact_hidden_bits = exact
            .final_hidden_bf16_words
            .iter()
            .map(|word| (*word as u32) << 16)
            .collect::<Vec<_>>();
        let exact_layer_hashes = exact
            .layers
            .iter()
            .map(|layer| {
                format!(
                    "0x{:016X}",
                    fnv1a64_u32_words(layer.layer_output_bits().unwrap())
                )
            })
            .collect::<Vec<_>>();
        let host_next = runtime.greedy_token_from_hidden(&host_hidden).unwrap();
        let first_mismatch = exact_layer_hashes
            .iter()
            .zip(host_layer_hashes.iter())
            .position(|(exact_hash, host_hash)| exact_hash != host_hash);
        println!("exact_layer_hashes={exact_layer_hashes:?}");
        println!("host_layer_hashes={host_layer_hashes:?}");
        println!("first_mismatch_layer={first_mismatch:?}");
        println!(
            "exact_final_hidden_fnv1a64=0x{:016X} host_final_hidden_fnv1a64=0x{:016X}",
            fnv1a64_u32_words(&exact_hidden_bits),
            fnv1a64_u32_words(&host_hidden_bits),
        );
        println!(
            "exact_next_token_id={} host_next_token_id={} host_next_token_logit={}",
            exact.next_token.token_id, host_next.token_id, host_next.logit
        );
        assert_eq!(host_hidden.len(), exact.final_hidden_bf16_words.len());
    }

    #[test]
    #[ignore]
    fn host_layer0_reports_against_exact_cached_stage_hashes() {
        let exact = run_two_token_prompt(default_model_path(), "say hi").unwrap();
        let exact_layer0 = &exact.layers[0];
        let runtime = GemmaTextRuntimeSession::load(&default_model_path()).unwrap();
        let runtime = Arc::unwrap_or_clone(runtime);
        let config = &runtime.weights.snapshot.config.text_config;
        let layer_idx = 0usize;
        let layer_type = config.layer_types.get(layer_idx).unwrap();
        let attention_k_eq_v = config.attention_k_eq_v && layer_type == "full_attention";
        let head_dim = if layer_type == "full_attention" {
            config.global_head_dim as usize
        } else {
            config.head_dim as usize
        };
        let k_head_count = if attention_k_eq_v && layer_type == "full_attention" {
            config.num_global_key_value_heads as usize
        } else {
            config.num_key_value_heads as usize
        };
        let q_head_count = config.num_attention_heads as usize;
        let v_head_count = k_head_count;
        let q_heads_per_kv = q_head_count / k_head_count;
        let rope_params = if layer_type == "full_attention" {
            &config.rope_parameters.full_attention
        } else {
            &config.rope_parameters.sliding_attention
        };
        let rope_rotary_dim = if let Some(partial_factor) = rope_params.partial_rotary_factor {
            (head_dim as f32 * partial_factor).round() as usize
        } else {
            head_dim
        };
        let rope = RopeSpec {
            head_dim,
            rotary_dim: rope_rotary_dim,
            base: rope_params.rope_theta,
        };
        let names = TextLayerTensorNames::for_layer(layer_idx, attention_k_eq_v);
        let q_norm_weights = runtime
            .weights
            .read_bf16_tensor_words(names.q.norm_weight_name.as_deref().unwrap())
            .unwrap();
        let k_norm_weights = runtime
            .weights
            .read_bf16_tensor_words(names.k.norm_weight_name.as_deref().unwrap())
            .unwrap();

        let hash_f32 = |values: &[f32]| {
            let bits = values.iter().copied().map(f32::to_bits).collect::<Vec<_>>();
            fnv1a64_u32_words(&bits)
        };
        let hash_opt_bits =
            |bits: Option<&[u32]>| bits.map(fnv1a64_u32_words).expect("missing exact bits");

        let mut caches = GemmaKvCacheSet::<f32>::new(runtime.kv_layout.clone()).unwrap();
        let mut prefill_hidden = runtime
            .weights
            .embed_token_bf16_words(exact.prompt_token_ids[0])
            .unwrap();
        let prefill_input_norm = rms_norm_weighted_tensor(
            &runtime.weights,
            &prefill_hidden,
            &names.input_norm_weight_name,
        )
        .unwrap();
        let prefill_input_norm_words = prefill_input_norm
            .iter()
            .copied()
            .map(super::f32_to_bf16_word)
            .collect::<Vec<_>>();
        let prefill_k_raw = quantized_matmul_tensor(
            &runtime.weights,
            &prefill_input_norm_words,
            &names.k.weight_name,
            &names.k.scales_name,
            &names.k.biases_name,
        )
        .unwrap();
        let mut prefill_k =
            rms_norm_rows_weighted_f32(&prefill_k_raw, k_head_count, head_dim, &k_norm_weights)
                .unwrap();
        apply_rope_rows_in_place(&mut prefill_k, k_head_count, rope, 0).unwrap();
        let prefill_v_raw = if attention_k_eq_v {
            prefill_k_raw
        } else {
            quantized_matmul_tensor(
                &runtime.weights,
                &prefill_input_norm_words,
                &names.v.weight_name,
                &names.v.scales_name,
                &names.v.biases_name,
            )
            .unwrap()
        };
        let prefill_v =
            rms_norm_rows_no_scale_f32(&prefill_v_raw, v_head_count, head_dim, config.rms_norm_eps)
                .unwrap();
        let layer_cache = caches.cache_for_layer_mut(layer_idx).unwrap();
        layer_cache
            .update_and_fetch(
                single_token_tensor(k_head_count, head_dim, prefill_k.clone())
                    .unwrap()
                    .view(),
                single_token_tensor(v_head_count, head_dim, prefill_v.clone())
                    .unwrap()
                    .view(),
            )
            .unwrap();
        prefill_hidden = runtime
            .eval_layer_hidden_state(layer_idx, &prefill_hidden, 0, &mut caches)
            .unwrap();

        let decode_hidden = runtime
            .weights
            .embed_token_bf16_words(exact.prompt_token_ids[1])
            .unwrap();
        let decode_input_norm = rms_norm_weighted_tensor(
            &runtime.weights,
            &decode_hidden,
            &names.input_norm_weight_name,
        )
        .unwrap();
        let decode_input_norm_words = decode_input_norm
            .iter()
            .copied()
            .map(super::f32_to_bf16_word)
            .collect::<Vec<_>>();
        let decode_q_raw = quantized_matmul_tensor(
            &runtime.weights,
            &decode_input_norm_words,
            &names.q.weight_name,
            &names.q.scales_name,
            &names.q.biases_name,
        )
        .unwrap();
        let mut decode_q =
            rms_norm_rows_weighted_f32(&decode_q_raw, q_head_count, head_dim, &q_norm_weights)
                .unwrap();
        apply_rope_rows_in_place(&mut decode_q, q_head_count, rope, 1).unwrap();
        let decode_k_raw = quantized_matmul_tensor(
            &runtime.weights,
            &decode_input_norm_words,
            &names.k.weight_name,
            &names.k.scales_name,
            &names.k.biases_name,
        )
        .unwrap();
        let mut decode_k =
            rms_norm_rows_weighted_f32(&decode_k_raw, k_head_count, head_dim, &k_norm_weights)
                .unwrap();
        apply_rope_rows_in_place(&mut decode_k, k_head_count, rope, 1).unwrap();
        let decode_v_raw = if attention_k_eq_v {
            decode_k_raw
        } else {
            quantized_matmul_tensor(
                &runtime.weights,
                &decode_input_norm_words,
                &names.v.weight_name,
                &names.v.scales_name,
                &names.v.biases_name,
            )
            .unwrap()
        };
        let decode_v =
            rms_norm_rows_no_scale_f32(&decode_v_raw, v_head_count, head_dim, config.rms_norm_eps)
                .unwrap();
        let layer_cache = caches.cache_for_layer_mut(layer_idx).unwrap();
        layer_cache
            .update_and_fetch(
                single_token_tensor(k_head_count, head_dim, decode_k.clone())
                    .unwrap()
                    .view(),
                single_token_tensor(v_head_count, head_dim, decode_v.clone())
                    .unwrap()
                    .view(),
            )
            .unwrap();
        let attention_out = compute_attention_output_f32(
            &decode_q,
            layer_cache,
            q_head_count,
            q_heads_per_kv,
            head_dim,
        )
        .unwrap();
        let attention_out_words = attention_out
            .iter()
            .copied()
            .map(super::f32_to_bf16_word)
            .collect::<Vec<_>>();
        let attention_oproj = quantized_matmul_tensor(
            &runtime.weights,
            &attention_out_words,
            &names.o.weight_name,
            &names.o.scales_name,
            &names.o.biases_name,
        )
        .unwrap();

        println!(
            "prefill_input_norm host=0x{:016X} exact=0x{:016X}",
            hash_f32(&prefill_input_norm),
            fnv1a64_u32_words(&exact_layer0.prefill_input_norm_bits),
        );
        println!(
            "prefill_k host=0x{:016X} exact=0x{:016X}",
            hash_f32(&prefill_k),
            fnv1a64_u32_words(&exact_layer0.prefill_k_bits),
        );
        println!(
            "prefill_v_proj host=0x{:016X} exact=0x{:016X}",
            hash_f32(&prefill_v_raw),
            fnv1a64_u32_words(&exact_layer0.prefill_v_proj_bits),
        );
        println!(
            "prefill_v host=0x{:016X} exact=0x{:016X}",
            hash_f32(&prefill_v),
            fnv1a64_u32_words(&exact_layer0.prefill_v_bits),
        );
        println!(
            "decode_input_norm host=0x{:016X} exact=0x{:016X}",
            hash_f32(&decode_input_norm),
            fnv1a64_u32_words(&exact_layer0.decode_input_norm_bits),
        );
        println!(
            "decode_q host=0x{:016X} exact=0x{:016X}",
            hash_f32(&decode_q),
            fnv1a64_u32_words(&exact_layer0.decode_q_bits),
        );
        println!(
            "decode_k host=0x{:016X} exact=0x{:016X}",
            hash_f32(&decode_k),
            fnv1a64_u32_words(&exact_layer0.decode_k_bits),
        );
        println!(
            "decode_v_proj host=0x{:016X} exact=0x{:016X}",
            hash_f32(&decode_v_raw),
            fnv1a64_u32_words(&exact_layer0.decode_v_proj_bits),
        );
        println!(
            "decode_v host=0x{:016X} exact=0x{:016X}",
            hash_f32(&decode_v),
            fnv1a64_u32_words(&exact_layer0.decode_v_bits),
        );
        println!(
            "attention_out host=0x{:016X} exact=0x{:016X}",
            hash_f32(&attention_out),
            fnv1a64_u32_words(&exact_layer0.attention_out_bits),
        );
        println!(
            "attention_oproj host=0x{:016X} exact=0x{:016X}",
            hash_f32(&attention_oproj),
            hash_opt_bits(exact_layer0.attention_oproj_bits.as_deref()),
        );
        assert_eq!(decode_q.len(), exact_layer0.decode_q_bits.len());
    }

    #[test]
    fn lazy_plan_formats_gemma4_user_turn_prompt() {
        let plan = lazy_text_plan(
            default_model_path(),
            "say hi",
            GemmaTextGenerationOptions {
                max_new_tokens: 4,
                prompt_format: GemmaPromptFormat::Gemma4UserTurn,
            },
        );
        let formatted = plan.eval_formatted_prompt_text().unwrap();
        assert!(formatted.starts_with("<bos><|turn>user\n"));
        assert!(formatted.contains("say hi"));
        assert!(formatted.ends_with("<|turn>model\n"));
    }

    #[test]
    #[ignore]
    fn lazy_plan_materializes_generation_prefixes_incrementally() {
        let plan = lazy_text_plan(
            default_model_path(),
            "say hi",
            GemmaTextGenerationOptions {
                max_new_tokens: 4,
                prompt_format: GemmaPromptFormat::Gemma4UserTurn,
            },
        );
        let prefix1 = plan.eval_generated_token_ids_up_to(1).unwrap();
        let prefix2 = plan.eval_generated_token_ids_up_to(2).unwrap();
        let output = plan.eval_generate().unwrap();

        assert!(prefix1.len() <= 1);
        assert!(prefix2.len() <= 2);
        assert_eq!(&*prefix1, &prefix2[..prefix1.len()]);
        assert_eq!(&*prefix2, &output.generated_token_ids[..prefix2.len()]);
    }

    #[test]
    #[ignore]
    fn exact_cursor_matches_two_token_exact_path() {
        let runtime = GemmaTextRuntimeSession::load(&default_model_path()).unwrap();
        let exact = run_two_token_ids(default_model_path(), [30_468, 5_631], 0, 1).unwrap();
        let mut cursor = runtime
            .start_generation_cursor(Arc::<[u32]>::from(exact.prompt_token_ids.clone()), 1)
            .unwrap();
        cursor.ensure_generated(1).unwrap();
        assert_eq!(cursor.generated_token_ids(), [exact.next_token.token_id]);
    }

    #[test]
    #[ignore]
    fn generation_cursor_defers_prompt_prefill_until_demanded() {
        let runtime = GemmaTextRuntimeSession::load(&default_model_path()).unwrap();
        let prompt_token_ids = Arc::<[u32]>::from(vec![30_468, 5_631]);
        let mut cursor = runtime
            .start_generation_cursor(prompt_token_ids.clone(), 1)
            .unwrap();
        assert_eq!(cursor.processed_prompt_tokens(), 0);
        assert_eq!(cursor.position(), 0);
        assert!(!cursor.has_pending_next());

        cursor.ensure_generated(0).unwrap();
        assert_eq!(cursor.processed_prompt_tokens(), 0);
        assert_eq!(cursor.position(), 0);
        assert!(!cursor.has_pending_next());

        cursor.ensure_generated(1).unwrap();
        assert_eq!(cursor.processed_prompt_tokens(), prompt_token_ids.len());
        assert_eq!(cursor.generated_token_ids(), [11]);
    }

    #[test]
    #[ignore]
    fn generation_graph_materializes_prefix_nodes_incrementally() {
        let runtime = GemmaTextRuntimeSession::load(&default_model_path()).unwrap();
        let prompt_token_ids = Arc::<[u32]>::from(vec![30_468, 5_631]);
        let graph = runtime
            .start_generation_graph(prompt_token_ids.clone(), 2)
            .unwrap();

        let prefix1 = graph.generated_token_ids_up_to(1).unwrap();
        let prefix2 = graph.generated_token_ids_up_to(2).unwrap();
        let final_snapshot = graph.finish_snapshot().unwrap();

        assert!(prefix1.len() <= 1);
        assert!(prefix2.len() <= 2);
        assert_eq!(&*prefix1, &prefix2[..prefix1.len()]);
        assert_eq!(&*prefix2, final_snapshot.generated_token_ids.as_ref());
        assert_eq!(
            final_snapshot.processed_prompt_tokens,
            prompt_token_ids.len()
        );
        assert!(final_snapshot.position >= prompt_token_ids.len());
        assert!(
            final_snapshot.has_pending_next
                || final_snapshot.stop_reason.is_some()
                || !final_snapshot.generated_token_ids.is_empty()
        );
    }

    #[test]
    #[ignore]
    fn exact_backend_head_matches_two_token_exact_hidden() {
        let runtime = GemmaTextRuntimeSession::load(&default_model_path()).unwrap();
        let exact = run_two_token_ids(default_model_path(), [30_468, 5_631], 0, 1).unwrap();
        let mut backend = runtime.exact_backend.lock().unwrap();
        let next = backend
            .greedy_token_from_hidden_words(&exact.final_hidden_bf16_words)
            .unwrap();
        println!(
            "exact_hidden_next_token_id={} exact_hidden_next_token_logit={}",
            next.token_id, next.logit
        );
        assert_eq!(next.token_id, exact.next_token.token_id);
    }

    #[test]
    #[ignore]
    fn exact_backend_device_head_matches_shared_logits_scan() {
        let runtime = GemmaTextRuntimeSession::load(&default_model_path()).unwrap();
        let exact = run_two_token_ids(default_model_path(), [30_468, 5_631], 0, 1).unwrap();
        let mut backend = runtime.exact_backend.lock().unwrap();
        let (device, shared) = backend
            .compare_greedy_token_paths_from_hidden_words(&exact.final_hidden_bf16_words)
            .unwrap();
        println!(
            "device_token_id={} device_logit={} shared_token_id={} shared_logit={}",
            device.token_id, device.logit, shared.token_id, shared.logit
        );
        assert_eq!(device.token_id, shared.token_id);
        assert_eq!(device.logit, shared.logit);
    }

    #[test]
    #[ignore]
    fn lazy_plan_generates_text_end_to_end() {
        let plan = lazy_text_plan(
            default_model_path(),
            "say hi",
            GemmaTextGenerationOptions {
                max_new_tokens: 8,
                prompt_format: GemmaPromptFormat::Gemma4UserTurn,
            },
        );
        let output = plan.eval_generate().unwrap();
        println!(
            "prompt_ids={:?} generated_ids={:?} generated_text={:?} stop_reason={:?}",
            output.prompt_token_ids,
            output.generated_token_ids,
            output.generated_text,
            output.stop_reason,
        );
        assert!(!output.prompt_token_ids.is_empty());
        assert!(matches!(
            output.stop_reason,
            GemmaStopReason::MaxNewTokens | GemmaStopReason::EosToken(_)
        ));
    }

    #[test]
    #[ignore]
    fn formatted_say_hi_greedy_prefix_matches_local_mlx() {
        let output = lazy_text_plan(
            default_model_path(),
            "say hi",
            GemmaTextGenerationOptions {
                max_new_tokens: 8,
                prompt_format: GemmaPromptFormat::Gemma4UserTurn,
            },
        )
        .eval_generate()
        .unwrap();

        let expected_prompt_token_ids =
            [2, 105, 2364, 107, 30_468, 5_631, 106, 107, 105, 4_368, 107];
        let expected_generated_token_ids = [11, 40, 20, 2, 2, 3, 2, 11];

        println!(
            "prompt_ids={:?} generated_ids={:?} generated_text={:?} stop_reason={:?}",
            output.prompt_token_ids,
            output.generated_token_ids,
            output.generated_text,
            output.stop_reason,
        );

        assert_eq!(output.prompt_token_ids.as_ref(), expected_prompt_token_ids);
        assert_eq!(
            output.generated_token_ids.as_ref(),
            expected_generated_token_ids
        );
        assert_eq!(output.stop_reason, GemmaStopReason::MaxNewTokens);
    }

    #[test]
    #[ignore]
    fn poem_prompt_32_token_prefix_matches_local_mlx() {
        let output = lazy_text_plan(
            default_model_path(),
            "Write a poem in exactly 10 lines about unified memory, lazy evaluation, and metal at midnight. Keep each line concise.",
            GemmaTextGenerationOptions {
                max_new_tokens: 32,
                prompt_format: GemmaPromptFormat::Gemma4UserTurn,
            },
        )
        .eval_generate()
        .unwrap();

        let expected_prompt_token_ids = [
            2, 105, 2364, 107, 6974, 496, 27355, 528, 7121, 236743, 236770, 236771, 4463, 1003,
            44623, 6571, 236764, 31770, 12207, 236764, 532, 6211, 657, 38735, 236761, 17608, 1546,
            1757, 63510, 236761, 106, 107, 105, 4368, 107,
        ];
        let expected_generated_token_ids = [
            4, 2, 2, 4, 2, 4, 4, 4, 2, 13, 36, 40, 20, 2, 4, 2, 13, 2, 9, 17, 16, 20, 20, 34, 2, 2,
            11, 2, 5, 2, 9, 2,
        ];

        println!(
            "prompt_ids={:?} generated_ids={:?} generated_text={:?} stop_reason={:?}",
            output.prompt_token_ids,
            output.generated_token_ids,
            output.generated_text,
            output.stop_reason,
        );

        assert_eq!(output.prompt_token_ids.as_ref(), expected_prompt_token_ids);
        assert_eq!(
            output.generated_token_ids.as_ref(),
            expected_generated_token_ids
        );
        assert_eq!(output.stop_reason, GemmaStopReason::MaxNewTokens);
    }
}
