use crate::chat::extract_gemma4_assistant_response_text;
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
pub struct GemmaTextModel {
    runtime: Arc<GemmaTextRuntimeSession>,
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

fn build_generation_output(
    runtime: &Arc<GemmaTextRuntimeSession>,
    prompt_text: Arc<str>,
    formatted_prompt_text: Arc<str>,
    prompt_token_ids: Arc<[u32]>,
    snapshot: Arc<crate::layer0_cached_case::ExactMetalGenerationSnapshot>,
) -> Result<Arc<GemmaTextGenerationOutput>, String> {
    let generated_token_ids = snapshot.generated_token_ids.clone();
    let stop_reason = match snapshot
        .stop_reason
        .ok_or_else(|| "generation graph completed without a stop reason".to_string())?
    {
        ExactMetalGenerationStopReason::MaxNewTokens => GemmaStopReason::MaxNewTokens,
        ExactMetalGenerationStopReason::EosToken(token_id) => GemmaStopReason::EosToken(token_id),
    };
    let generated_text = if generated_token_ids.is_empty() {
        Arc::<str>::from("")
    } else {
        let raw_text = runtime
            .tokenizer
            .decode(generated_token_ids.as_ref())
            .map_err(|err| err.to_string())?;
        Arc::<str>::from(extract_gemma4_assistant_response_text(
            &runtime.weights.snapshot.tokenizer_config,
            &raw_text,
        ))
    };
    Ok(Arc::new(GemmaTextGenerationOutput {
        model_path: runtime.model_path.clone(),
        prompt_text,
        formatted_prompt_text,
        prompt_token_ids,
        generated_token_ids,
        generated_text,
        stop_reason,
    }))
}

#[derive(Clone)]
struct GemmaTextRuntimeSession {
    model_path: PathBuf,
    weights: MlxIndexedSafetensors,
    tokenizer: MlxTokenizer,
    #[cfg_attr(not(test), allow(dead_code))]
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
            .start_generation_graph(prompt_token_ids.clone(), Some(self.options.max_new_tokens))?;
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
            build_generation_output(
                &runtime,
                self.prompt_text.clone(),
                formatted_prompt_text,
                prompt_token_ids,
                snapshot,
            )
        })
    }
}

impl GemmaTextModel {
    pub fn load(model_path: impl AsRef<Path>) -> Result<Self, Box<dyn Error>> {
        Ok(Self {
            runtime: GemmaTextRuntimeSession::load(model_path.as_ref())
                .map_err(|err| err.to_string())?,
        })
    }

    pub fn tokenizer_config(&self) -> &crate::MlxTokenizerConfig {
        &self.runtime.weights.snapshot.tokenizer_config
    }

    pub fn generate(
        &self,
        prompt_text: impl Into<String>,
        options: GemmaTextGenerationOptions,
    ) -> Result<Arc<GemmaTextGenerationOutput>, Box<dyn Error>> {
        let prompt_text = Arc::<str>::from(prompt_text.into());
        let formatted_prompt_text = Arc::<str>::from(
            self.runtime
                .format_prompt_text(prompt_text.as_ref(), options.prompt_format),
        );
        self.generate_from_formatted_arcs(
            prompt_text,
            formatted_prompt_text,
            Some(options.max_new_tokens),
        )
    }

    pub fn generate_preformatted(
        &self,
        formatted_prompt_text: impl Into<String>,
        max_new_tokens: Option<usize>,
    ) -> Result<Arc<GemmaTextGenerationOutput>, Box<dyn Error>> {
        let formatted_prompt_text = Arc::<str>::from(formatted_prompt_text.into());
        self.generate_from_formatted_arcs(
            formatted_prompt_text.clone(),
            formatted_prompt_text,
            max_new_tokens,
        )
    }

    pub fn stream_generate_preformatted<F>(
        &self,
        formatted_prompt_text: impl Into<String>,
        max_new_tokens: Option<usize>,
        mut on_text_delta: F,
    ) -> Result<Arc<GemmaTextGenerationOutput>, Box<dyn Error>>
    where
        F: FnMut(&str) -> Result<(), Box<dyn Error>>,
    {
        let formatted_prompt_text = Arc::<str>::from(formatted_prompt_text.into());
        let prompt_text = formatted_prompt_text.clone();
        let prompt_token_ids = self
            .runtime
            .tokenize_prompt(formatted_prompt_text.as_ref())
            .map_err(|err| err.to_string())?;
        let graph = self
            .runtime
            .start_generation_graph(prompt_token_ids.clone(), max_new_tokens)
            .map_err(|err| err.to_string())?;

        let mut streamed_text = String::new();
        let mut count = 1usize;
        loop {
            let generated_token_ids = graph
                .generated_token_ids_up_to(count)
                .map_err(|err| err.to_string())?;
            let partial_text = self.decode_generated_text(generated_token_ids.as_ref())?;
            if let Some(delta) = partial_text.strip_prefix(&streamed_text) {
                if !delta.is_empty() {
                    on_text_delta(delta)?;
                    streamed_text.push_str(delta);
                }
            }
            if generated_token_ids.len() < count {
                break;
            }
            if max_new_tokens == Some(count) {
                break;
            }
            count = count
                .checked_add(1)
                .ok_or_else(|| "generation token count overflow".to_string())?;
        }

        let snapshot = graph.finish_snapshot().map_err(|err| err.to_string())?;
        let output = build_generation_output(
            &self.runtime,
            prompt_text,
            formatted_prompt_text,
            prompt_token_ids,
            snapshot,
        )?;
        if let Some(delta) = output.generated_text.as_ref().strip_prefix(&streamed_text) {
            if !delta.is_empty() {
                on_text_delta(delta)?;
            }
        }
        Ok(output)
    }

    fn generate_from_formatted_arcs(
        &self,
        prompt_text: Arc<str>,
        formatted_prompt_text: Arc<str>,
        max_new_tokens: Option<usize>,
    ) -> Result<Arc<GemmaTextGenerationOutput>, Box<dyn Error>> {
        let prompt_token_ids = self
            .runtime
            .tokenize_prompt(formatted_prompt_text.as_ref())
            .map_err(|err| err.to_string())?;
        let graph = self
            .runtime
            .start_generation_graph(prompt_token_ids.clone(), max_new_tokens)
            .map_err(|err| err.to_string())?;
        let snapshot = graph.finish_snapshot().map_err(|err| err.to_string())?;
        build_generation_output(
            &self.runtime,
            prompt_text,
            formatted_prompt_text,
            prompt_token_ids,
            snapshot,
        )
        .map_err(|err| err.into())
    }

    fn decode_generated_text(
        &self,
        generated_token_ids: &[u32],
    ) -> Result<String, Box<dyn Error>> {
        if generated_token_ids.is_empty() {
            return Ok(String::new());
        }
        let raw_text = self
            .runtime
            .tokenizer
            .decode(generated_token_ids)
            .map_err(|err| err.to_string())?;
        Ok(extract_gemma4_assistant_response_text(
            &self.runtime.weights.snapshot.tokenizer_config,
            &raw_text,
        ))
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
            .start_generation_graph(prompt_token_ids.clone(), Some(options.max_new_tokens))?
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
        let graph = runtime.start_generation_graph(
            prompt_token_ids.clone(),
            Some(options.max_new_tokens),
        )?;
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
