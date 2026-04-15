use crate::chat::extract_gemma4_assistant_response_text;
pub use crate::layer0_cached_case::{
    GemmaExactMetalBackendMode, GemmaExactMetalConfig, GemmaExactMetalKvCompressionMode,
};
use crate::multimodal::{prepare_image_prompt, GemmaVisionRuntime, PreparedImagePrompt};

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
    AutoChat,
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
            prompt_format: GemmaPromptFormat::AutoChat,
        }
    }
}

#[derive(Clone, Debug)]
pub struct GemmaTextSamplingOptions {
    pub do_sample: bool,
    pub temperature: f32,
    pub top_k: u32,
    pub top_p: f32,
    pub allow_thought: bool,
}

impl GemmaTextSamplingOptions {
    fn from_generation_config(config: &crate::MlxGenerationConfig) -> Self {
        Self {
            do_sample: config.do_sample,
            temperature: config.temperature,
            top_k: config.top_k,
            top_p: config.top_p,
            allow_thought: true,
        }
    }

    fn chat_from_generation_config(config: &crate::MlxGenerationConfig) -> Self {
        let mut options = Self::from_generation_config(config);
        options.allow_thought = false;
        options
    }

    pub(crate) fn greedy_variant(&self) -> Self {
        let mut options = self.clone();
        options.do_sample = false;
        options.temperature = 0.0;
        options.top_k = 1;
        options.top_p = 1.0;
        options
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
    pub metrics: GemmaTextGenerationMetrics,
}

#[derive(Clone, Debug)]
pub struct GemmaTextGenerationMetrics {
    pub elapsed: Duration,
    pub time_to_first_token_elapsed: Duration,
    pub steady_state_elapsed: Duration,
    pub steady_state_generated_tokens: usize,
    pub prompt_prefill_tokens_per_second: f64,
    pub steady_state_decode_tokens_per_second: f64,
    pub decode_tokens_per_second: f64,
}

#[derive(Clone, Debug)]
pub struct GemmaExactPrefillProbeOutput {
    pub model_path: PathBuf,
    pub prompt_text: Arc<str>,
    pub formatted_prompt_text: Arc<str>,
    pub prompt_token_ids: Arc<[u32]>,
    pub final_hidden_bf16_words: Arc<[u16]>,
    pub next_token: MlxGreedyToken,
    pub next_token_text: Arc<str>,
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
    vision_runtime: Arc<Mutex<Option<GemmaVisionRuntime>>>,
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
    let prompt_token_count = prompt_token_ids.len();
    let generated_token_count = generated_token_ids.len();
    let stop_reason = match snapshot
        .stop_reason
        .ok_or_else(|| "generation graph completed without a stop reason".to_string())?
    {
        ExactMetalGenerationStopReason::MaxNewTokens => GemmaStopReason::MaxNewTokens,
        ExactMetalGenerationStopReason::EosToken(token_id) => GemmaStopReason::EosToken(token_id),
    };
    build_generation_output_from_token_ids(
        runtime,
        prompt_text,
        formatted_prompt_text,
        prompt_token_ids,
        generated_token_ids,
        stop_reason,
        build_generation_metrics(
            Duration::ZERO,
            prompt_token_count,
            generated_token_count,
            Duration::ZERO,
        ),
    )
}

fn build_generation_metrics(
    elapsed: Duration,
    prompt_token_count: usize,
    generated_token_count: usize,
    time_to_first_token_elapsed: Duration,
) -> GemmaTextGenerationMetrics {
    let steady_state_generated_tokens = generated_token_count.saturating_sub(1);
    let steady_state_elapsed = elapsed.saturating_sub(time_to_first_token_elapsed);
    let ttft_secs = time_to_first_token_elapsed.as_secs_f64();
    let elapsed_secs = elapsed.as_secs_f64();
    let steady_secs = steady_state_elapsed.as_secs_f64();
    GemmaTextGenerationMetrics {
        elapsed,
        time_to_first_token_elapsed,
        steady_state_elapsed,
        steady_state_generated_tokens,
        prompt_prefill_tokens_per_second: if ttft_secs > 0.0 {
            prompt_token_count as f64 / ttft_secs
        } else {
            0.0
        },
        steady_state_decode_tokens_per_second: if steady_secs > 0.0 {
            steady_state_generated_tokens as f64 / steady_secs
        } else {
            0.0
        },
        decode_tokens_per_second: if elapsed_secs > 0.0 {
            generated_token_count as f64 / elapsed_secs
        } else {
            0.0
        },
    }
}

fn build_generation_output_from_token_ids(
    runtime: &Arc<GemmaTextRuntimeSession>,
    prompt_text: Arc<str>,
    formatted_prompt_text: Arc<str>,
    prompt_token_ids: Arc<[u32]>,
    generated_token_ids: Arc<[u32]>,
    stop_reason: GemmaStopReason,
    metrics: GemmaTextGenerationMetrics,
) -> Result<Arc<GemmaTextGenerationOutput>, String> {
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
        metrics,
    }))
}

#[derive(Clone, Debug)]
pub(crate) struct MlxTextSamplingRng {
    state: [u64; 312],
    index: usize,
}

impl MlxTextSamplingRng {
    pub(crate) fn new(seed: u64) -> Self {
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

#[derive(Clone, Debug)]
struct SampledTokenCandidate {
    token_id: u32,
    logit: f32,
    scaled_logit: f32,
    prob: f64,
}

fn bf16_word_to_f32_local(word: u16) -> f32 {
    f32::from_bits((word as u32) << 16)
}

fn bf16_round_to_f32_local(value: f32) -> f32 {
    bf16_word_to_f32_local((value.to_bits() >> 16) as u16)
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

#[derive(Clone, Debug, Default)]
struct ChatSamplingConstraints {
    bos_token_id: Option<u32>,
    sot_token_id: Option<u32>,
    soc_token_id: Option<u32>,
    eoc_token_id: Option<u32>,
    stc_token_id: Option<u32>,
    etc_token_id: Option<u32>,
    std_token_id: Option<u32>,
    etd_token_id: Option<u32>,
    str_token_id: Option<u32>,
    etr_token_id: Option<u32>,
    boi_token_id: Option<u32>,
    eoi_token_id: Option<u32>,
    image_token_id: Option<u32>,
    boa_token_id: Option<u32>,
    eoa_token_id: Option<u32>,
    audio_token_id: Option<u32>,
    escape_token_id: Option<u32>,
    newline_token_id: Option<u32>,
}

impl ChatSamplingConstraints {
    fn from_runtime(runtime: &GemmaTextRuntimeSession) -> Self {
        let tokenizer = &runtime.tokenizer;
        let config = &runtime.weights.snapshot.tokenizer_config;
        Self {
            bos_token_id: tokenizer.token_to_id(&config.bos_token),
            sot_token_id: tokenizer.token_to_id(&config.sot_token),
            soc_token_id: tokenizer.token_to_id(&config.soc_token),
            eoc_token_id: tokenizer.token_to_id(&config.eoc_token),
            stc_token_id: tokenizer.token_to_id(&config.stc_token),
            etc_token_id: tokenizer.token_to_id(&config.etc_token),
            std_token_id: tokenizer.token_to_id(&config.std_token),
            etd_token_id: tokenizer.token_to_id(&config.etd_token),
            str_token_id: tokenizer.token_to_id(&config.str_token),
            etr_token_id: tokenizer.token_to_id(&config.etr_token),
            boi_token_id: tokenizer.token_to_id(&config.boi_token),
            eoi_token_id: tokenizer.token_to_id(&config.eoi_token),
            image_token_id: tokenizer.token_to_id(&config.image_token),
            boa_token_id: tokenizer.token_to_id(&config.boa_token),
            eoa_token_id: tokenizer.token_to_id(&config.eoa_token),
            audio_token_id: tokenizer.token_to_id(&config.audio_token),
            escape_token_id: tokenizer.token_to_id(&config.escape_token),
            newline_token_id: tokenizer.token_to_id("\n"),
        }
    }

    fn push_control_ids(&self, out: &mut Vec<u32>) {
        for token_id in [
            self.bos_token_id,
            self.sot_token_id,
            self.stc_token_id,
            self.etc_token_id,
            self.std_token_id,
            self.etd_token_id,
            self.str_token_id,
            self.etr_token_id,
            self.boi_token_id,
            self.eoi_token_id,
            self.image_token_id,
            self.boa_token_id,
            self.eoa_token_id,
            self.audio_token_id,
            self.escape_token_id,
        ]
        .into_iter()
        .flatten()
        {
            if !out.contains(&token_id) {
                out.push(token_id);
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ChatSamplingPhase {
    Start,
    InThought,
    BeforeContent,
    InContent,
}

#[derive(Clone, Debug)]
struct ChatSamplingState {
    phase: ChatSamplingPhase,
}

impl ChatSamplingState {
    fn new() -> Self {
        Self {
            phase: ChatSamplingPhase::Start,
        }
    }

    fn disallowed_token_ids(
        &self,
        constraints: &ChatSamplingConstraints,
        stop_tokens: &BTreeSet<u32>,
        sampling_options: &GemmaTextSamplingOptions,
    ) -> Vec<u32> {
        let mut out = Vec::with_capacity(16);
        constraints.push_control_ids(&mut out);
        match self.phase {
            ChatSamplingPhase::Start => {
                if !sampling_options.allow_thought {
                    if let Some(token_id) = constraints.soc_token_id {
                        out.push(token_id);
                    }
                }
                if let Some(token_id) = constraints.eoc_token_id {
                    out.push(token_id);
                }
                for &token_id in stop_tokens {
                    if !out.contains(&token_id) {
                        out.push(token_id);
                    }
                }
            }
            ChatSamplingPhase::InThought => {
                if let Some(token_id) = constraints.soc_token_id {
                    out.push(token_id);
                }
                for &token_id in stop_tokens {
                    if !out.contains(&token_id) {
                        out.push(token_id);
                    }
                }
            }
            ChatSamplingPhase::BeforeContent => {
                if !sampling_options.allow_thought {
                    if let Some(token_id) = constraints.soc_token_id {
                        out.push(token_id);
                    }
                }
                if let Some(token_id) = constraints.eoc_token_id {
                    out.push(token_id);
                }
                for &token_id in stop_tokens {
                    if !out.contains(&token_id) {
                        out.push(token_id);
                    }
                }
            }
            ChatSamplingPhase::InContent => {
                if let Some(token_id) = constraints.soc_token_id {
                    out.push(token_id);
                }
                if let Some(token_id) = constraints.eoc_token_id {
                    out.push(token_id);
                }
            }
        }
        out.sort_unstable();
        out.dedup();
        out
    }

    fn observe_token(&mut self, token_id: u32, constraints: &ChatSamplingConstraints) {
        match self.phase {
            ChatSamplingPhase::Start => {
                if Some(token_id) == constraints.soc_token_id {
                    self.phase = ChatSamplingPhase::InThought;
                } else if Some(token_id) == constraints.newline_token_id {
                    self.phase = ChatSamplingPhase::BeforeContent;
                } else {
                    self.phase = ChatSamplingPhase::InContent;
                }
            }
            ChatSamplingPhase::InThought => {
                if Some(token_id) == constraints.eoc_token_id {
                    self.phase = ChatSamplingPhase::BeforeContent;
                }
            }
            ChatSamplingPhase::BeforeContent => {
                if Some(token_id) != constraints.newline_token_id {
                    self.phase = ChatSamplingPhase::InContent;
                }
            }
            ChatSamplingPhase::InContent => {}
        }
    }
}

fn token_is_disallowed(disallowed_token_ids: &[u32], token_id: u32) -> bool {
    disallowed_token_ids.binary_search(&token_id).is_ok()
}

fn select_top1_token_from_logits(
    logits: &[f32],
    disallowed_token_ids: &[u32],
) -> Result<MlxGreedyToken, String> {
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

fn finalize_sampled_candidates(
    mut candidates: Vec<SampledTokenCandidate>,
    fallback_top1: impl FnOnce() -> Result<MlxGreedyToken, String>,
    sampling_options: &GemmaTextSamplingOptions,
    rng: &mut MlxTextSamplingRng,
) -> Result<MlxGreedyToken, String> {
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
        let truncated_sum = candidates
            .iter()
            .map(|candidate| candidate.prob)
            .sum::<f64>();
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
    sampling_options: &GemmaTextSamplingOptions,
    rng: &mut MlxTextSamplingRng,
) -> Result<MlxGreedyToken, String> {
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

pub(crate) fn sample_token_from_softcapped_bf16_bytes(
    logits_bytes: &[u8],
    softcap: Option<f32>,
    disallowed_token_ids: &[u32],
    sampling_options: &GemmaTextSamplingOptions,
    rng: &mut MlxTextSamplingRng,
) -> Result<MlxGreedyToken, String> {
    if logits_bytes.len() % size_of::<u16>() != 0 {
        return Err(format!(
            "softcapped logits byte length {} is not a multiple of {}",
            logits_bytes.len(),
            size_of::<u16>()
        ));
    }

    if !sampling_options.do_sample || sampling_options.temperature <= 0.0 {
        let mut logits = Vec::with_capacity(logits_bytes.len() / size_of::<u16>());
        for word_bytes in logits_bytes.chunks_exact(size_of::<u16>()) {
            let raw_logit =
                bf16_word_to_f32_local(u16::from_le_bytes([word_bytes[0], word_bytes[1]]));
            logits.push(if let Some(softcap) = softcap {
                bf16_round_to_f32_local((raw_logit / softcap).tanh() * softcap)
            } else {
                raw_logit
            });
        }
        return select_top1_token_from_logits(&logits, disallowed_token_ids);
    }

    let top_k = sampling_options.top_k as usize;
    let mut candidates =
        Vec::with_capacity(top_k.max((logits_bytes.len() / size_of::<u16>()).min(1024)));
    for (token_idx, word_bytes) in logits_bytes.chunks_exact(size_of::<u16>()).enumerate() {
        let token_id = token_idx as u32;
        if token_is_disallowed(disallowed_token_ids, token_id) {
            continue;
        }
        let raw_logit = bf16_word_to_f32_local(u16::from_le_bytes([word_bytes[0], word_bytes[1]]));
        push_sampled_candidate_top_k(
            &mut candidates,
            SampledTokenCandidate {
                token_id,
                logit: raw_logit,
                scaled_logit: raw_logit,
                prob: 0.0,
            },
            top_k,
        );
    }
    for candidate in &mut candidates {
        candidate.logit = if let Some(softcap) = softcap {
            bf16_round_to_f32_local((candidate.logit / softcap).tanh() * softcap)
        } else {
            candidate.logit
        };
        candidate.scaled_logit = candidate.logit / sampling_options.temperature;
    }
    finalize_sampled_candidates(
        candidates,
        || {
            let mut logits = Vec::with_capacity(logits_bytes.len() / size_of::<u16>());
            for word_bytes in logits_bytes.chunks_exact(size_of::<u16>()) {
                let raw_logit =
                    bf16_word_to_f32_local(u16::from_le_bytes([word_bytes[0], word_bytes[1]]));
                logits.push(if let Some(softcap) = softcap {
                    bf16_round_to_f32_local((raw_logit / softcap).tanh() * softcap)
                } else {
                    raw_logit
                });
            }
            select_top1_token_from_logits(&logits, disallowed_token_ids)
        },
        sampling_options,
        rng,
    )
}

fn generate_sampled_token_ids_with_exact_prefill<P, F>(
    runtime: &Arc<GemmaTextRuntimeSession>,
    prompt_token_ids: Arc<[u32]>,
    max_new_tokens: Option<usize>,
    sampling_options: &GemmaTextSamplingOptions,
    rng: &mut MlxTextSamplingRng,
    prefill: P,
    mut on_generated_ids: F,
) -> Result<(Arc<[u32]>, GemmaStopReason), String>
where
    P: FnOnce(
        &mut ExactMetalTextRuntimeSession,
        &[u32],
        usize,
        &[u32],
        &GemmaTextSamplingOptions,
        &mut MlxTextSamplingRng,
    ) -> Result<MlxGreedyToken, Box<dyn Error>>,
    F: FnMut(&[u32]) -> Result<(), String>,
{
    if prompt_token_ids.is_empty() {
        return Err("generation requires at least one prompt token".to_string());
    }

    let stop_tokens = &runtime.stop_tokens;
    let constraints = ChatSamplingConstraints::from_runtime(runtime);
    let mut sampling_state = ChatSamplingState::new();
    let exact_backend = runtime.exact_backend()?;
    let mut backend = exact_backend
        .lock()
        .map_err(|_| "exact backend mutex poisoned".to_string())?;

    let mut generated_token_ids = Vec::with_capacity(max_new_tokens.unwrap_or(32));
    let mut next_token = prefill(
        &mut backend,
        prompt_token_ids.as_ref(),
        0,
        &sampling_state.disallowed_token_ids(&constraints, stop_tokens, sampling_options),
        sampling_options,
        rng,
    )
    .map_err(|err| err.to_string())?;

    loop {
        generated_token_ids.push(next_token.token_id);
        sampling_state.observe_token(next_token.token_id, &constraints);
        on_generated_ids(&generated_token_ids)?;

        if stop_tokens.contains(&next_token.token_id) {
            return Ok((
                Arc::<[u32]>::from(generated_token_ids),
                GemmaStopReason::EosToken(next_token.token_id),
            ));
        }
        if max_new_tokens.is_some_and(|limit| generated_token_ids.len() >= limit) {
            return Ok((
                Arc::<[u32]>::from(generated_token_ids),
                GemmaStopReason::MaxNewTokens,
            ));
        }

        let position = prompt_token_ids
            .len()
            .checked_add(generated_token_ids.len())
            .and_then(|value| value.checked_sub(1))
            .ok_or_else(|| "generation cursor position overflow".to_string())?;
        next_token = backend
            .eval_token_sampled_from_token_id(
                next_token.token_id,
                position,
                &sampling_state.disallowed_token_ids(&constraints, stop_tokens, sampling_options),
                sampling_options,
                rng,
            )
            .map_err(|err| err.to_string())?;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TextGenerationBackend {
    MetalExact,
    CudaExactGreedy,
    Reference,
}

impl TextGenerationBackend {
    fn label(self) -> &'static str {
        match self {
            Self::MetalExact => "metal-exact",
            Self::CudaExactGreedy => "cuda-exact-greedy",
            Self::Reference => "reference",
        }
    }
}

fn select_text_generation_backend(
    runtime: &Arc<GemmaTextRuntimeSession>,
    max_new_tokens: Option<usize>,
    sampling_options: &GemmaTextSamplingOptions,
    has_prompt_embedding_rows: bool,
) -> TextGenerationBackend {
    if runtime.has_exact_backend() {
        TextGenerationBackend::MetalExact
    } else if !has_prompt_embedding_rows
        && runtime.has_cuda_exact_backend()
        && max_new_tokens.is_some_and(|limit| limit > 0)
        && (!sampling_options.do_sample || sampling_options.temperature <= 0.0)
    {
        TextGenerationBackend::CudaExactGreedy
    } else {
        TextGenerationBackend::Reference
    }
}

fn generate_sampled_token_ids<F>(
    runtime: &Arc<GemmaTextRuntimeSession>,
    prompt_token_ids: Arc<[u32]>,
    max_new_tokens: Option<usize>,
    sampling_options: &GemmaTextSamplingOptions,
    rng: &mut MlxTextSamplingRng,
    on_generated_ids: F,
) -> Result<(Arc<[u32]>, GemmaStopReason), String>
where
    F: FnMut(&[u32]) -> Result<(), String>,
{
    match select_text_generation_backend(runtime, max_new_tokens, sampling_options, false) {
        TextGenerationBackend::MetalExact => generate_sampled_token_ids_with_exact_prefill(
            runtime,
            prompt_token_ids,
            max_new_tokens,
            sampling_options,
            rng,
            |backend,
             prompt_token_ids,
             start_position,
             disallowed_token_ids,
             sampling_options,
             rng| {
                backend.prefill_prompt_sampled_from_token_ids(
                    prompt_token_ids,
                    start_position,
                    disallowed_token_ids,
                    sampling_options,
                    rng,
                )
            },
            on_generated_ids,
        ),
        TextGenerationBackend::CudaExactGreedy => cuda_exact::try_generate_cuda_nvfp4_greedy(
            runtime,
            prompt_token_ids,
            max_new_tokens,
            sampling_options,
            on_generated_ids,
        )?
        .ok_or_else(|| "CUDA exact backend selection unexpectedly fell back".to_string()),
        TextGenerationBackend::Reference => runtime.generate_sampled_token_ids_reference(
            prompt_token_ids,
            max_new_tokens,
            sampling_options,
            rng,
            on_generated_ids,
        ),
    }
}

fn generate_sampled_token_ids_from_embedding_rows<F>(
    runtime: &Arc<GemmaTextRuntimeSession>,
    prompt_token_ids: Arc<[u32]>,
    prompt_embedding_rows: Vec<Vec<u16>>,
    max_new_tokens: Option<usize>,
    sampling_options: &GemmaTextSamplingOptions,
    rng: &mut MlxTextSamplingRng,
    on_generated_ids: F,
) -> Result<(Arc<[u32]>, GemmaStopReason), String>
where
    F: FnMut(&[u32]) -> Result<(), String>,
{
    match select_text_generation_backend(runtime, max_new_tokens, sampling_options, true) {
        TextGenerationBackend::MetalExact => generate_sampled_token_ids_with_exact_prefill(
            runtime,
            prompt_token_ids,
            max_new_tokens,
            sampling_options,
            rng,
            |backend,
             _prompt_token_ids,
             start_position,
             disallowed_token_ids,
             sampling_options,
             rng| {
                backend.prefill_prompt_sampled_from_embedding_rows(
                    &prompt_embedding_rows,
                    start_position,
                    disallowed_token_ids,
                    sampling_options,
                    rng,
                )
            },
            on_generated_ids,
        ),
        TextGenerationBackend::CudaExactGreedy | TextGenerationBackend::Reference => runtime
            .generate_sampled_token_ids_from_embedding_rows_reference(
                prompt_token_ids,
                prompt_embedding_rows,
                max_new_tokens,
                sampling_options,
                rng,
                on_generated_ids,
            ),
    }
}

#[derive(Clone)]
struct GemmaTextRuntimeSession {
    model_path: PathBuf,
    #[cfg_attr(not(test), allow(dead_code))]
    backend_config: GemmaExactMetalConfig,
    weights: MlxIndexedSafetensors,
    tokenizer: MlxTokenizer,
    #[cfg_attr(not(test), allow(dead_code))]
    kv_layout: GemmaKvCacheLayout,
    stop_tokens: BTreeSet<u32>,
    exact_backend: Option<Arc<Mutex<ExactMetalTextRuntimeSession>>>,
    cuda_exact_backend: Option<Arc<Mutex<cuda_exact::CudaNvfp4TextRuntime>>>,
}

#[derive(Clone, Debug)]
struct TextProjectionNames {
    weight_name: String,
    scales_name: String,
    biases_name: String,
    norm_weight_name: Option<String>,
}

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
    per_layer_input_gate_weight_name: String,
    per_layer_input_gate_scales_name: String,
    per_layer_input_gate_biases_name: String,
    per_layer_projection_weight_name: String,
    per_layer_projection_scales_name: String,
    per_layer_projection_biases_name: String,
    post_per_layer_input_norm_weight_name: String,
    layer_scalar_name: String,
}

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
            per_layer_input_gate_weight_name: format!("{base}.per_layer_input_gate.weight"),
            per_layer_input_gate_scales_name: format!("{base}.per_layer_input_gate.scales"),
            per_layer_input_gate_biases_name: format!("{base}.per_layer_input_gate.biases"),
            per_layer_projection_weight_name: format!("{base}.per_layer_projection.weight"),
            per_layer_projection_scales_name: format!("{base}.per_layer_projection.scales"),
            per_layer_projection_biases_name: format!("{base}.per_layer_projection.biases"),
            post_per_layer_input_norm_weight_name: format!(
                "{base}.post_per_layer_input_norm.weight"
            ),
            layer_scalar_name: format!("{base}.layer_scalar"),
        }
    }
}

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
            let graph = runtime.start_generation_graph(
                prompt_token_ids.clone(),
                Some(self.options.max_new_tokens),
            )?;
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
            if runtime.has_exact_backend() {
                let snapshot = self.generation_graph()?.finish_snapshot()?;
                build_generation_output(
                    &runtime,
                    self.prompt_text.clone(),
                    formatted_prompt_text,
                    prompt_token_ids,
                    snapshot,
                )
            } else {
                let mut rng = MlxTextSamplingRng::new(0);
                let sampling_options = GemmaTextSamplingOptions::from_generation_config(
                    &runtime.weights.snapshot.generation_config,
                );
                let (generated_token_ids, stop_reason) = generate_sampled_token_ids(
                    &runtime,
                    prompt_token_ids.clone(),
                    Some(self.options.max_new_tokens),
                    &sampling_options,
                    &mut rng,
                    |_| Ok(()),
                )?;
                let prompt_token_count = prompt_token_ids.len();
                let generated_token_count = generated_token_ids.len();
                build_generation_output_from_token_ids(
                    &runtime,
                    self.prompt_text.clone(),
                    formatted_prompt_text,
                    prompt_token_ids,
                    generated_token_ids,
                    stop_reason,
                    build_generation_metrics(
                        Duration::ZERO,
                        prompt_token_count,
                        generated_token_count,
                        Duration::ZERO,
                    ),
                )
            }
        })
    }
}

impl GemmaTextModel {
    pub fn load(model_path: impl AsRef<Path>) -> Result<Self, Box<dyn Error>> {
        Self::load_with_backend_config(model_path, GemmaExactMetalConfig::default())
    }

    pub fn load_with_backend_config(
        model_path: impl AsRef<Path>,
        backend_config: GemmaExactMetalConfig,
    ) -> Result<Self, Box<dyn Error>> {
        Ok(Self {
            runtime: GemmaTextRuntimeSession::load_with_backend_config(
                model_path.as_ref(),
                backend_config,
            )
            .map_err(|err| err.to_string())?,
            vision_runtime: Arc::new(Mutex::new(None)),
        })
    }

    pub fn tokenizer_config(&self) -> &crate::MlxTokenizerConfig {
        &self.runtime.weights.snapshot.tokenizer_config
    }

    pub fn default_chat_prompt_format(&self) -> GemmaPromptFormat {
        self.runtime.default_chat_prompt_format()
    }

    pub fn chat_sampling_options(&self) -> GemmaTextSamplingOptions {
        GemmaTextSamplingOptions::chat_from_generation_config(
            &self.runtime.weights.snapshot.generation_config,
        )
    }

    pub fn default_sampling_options(&self) -> GemmaTextSamplingOptions {
        GemmaTextSamplingOptions::from_generation_config(
            &self.runtime.weights.snapshot.generation_config,
        )
    }

    pub fn generation_backend_label(
        &self,
        max_new_tokens: Option<usize>,
        sampling_options: &GemmaTextSamplingOptions,
    ) -> &'static str {
        select_text_generation_backend(&self.runtime, max_new_tokens, sampling_options, false)
            .label()
    }

    pub fn multimodal_generation_backend_label(
        &self,
        max_new_tokens: Option<usize>,
        sampling_options: &GemmaTextSamplingOptions,
    ) -> &'static str {
        select_text_generation_backend(&self.runtime, max_new_tokens, sampling_options, true)
            .label()
    }

    pub fn tokenize_formatted_prompt(&self, formatted_prompt: &str) -> Result<Arc<[u32]>, String> {
        self.runtime.tokenize_prompt(formatted_prompt)
    }

    pub fn cuda_exact_supported_total_tokens(&self) -> Option<usize> {
        self.runtime.has_cuda_exact_backend().then(|| {
            crate::text_runtime::cuda_exact::cuda_exact_max_supported_tokens(&self.runtime)
        })
    }

    pub fn prewarm_greedy_backend(&self, max_new_tokens: Option<usize>) -> Result<(), String> {
        let sampling_options = self.chat_sampling_options().greedy_variant();
        crate::text_runtime::cuda_exact::prewarm_cuda_nvfp4_greedy(
            &self.runtime,
            max_new_tokens,
            &sampling_options,
        )
    }

    pub(crate) fn decode_token_ids(&self, token_ids: &[u32]) -> Result<String, String> {
        self.runtime
            .tokenizer
            .decode(token_ids)
            .map_err(|err| err.to_string())
    }

    pub(crate) fn generate_pretokenized_cuda_exact_greedy_with_callback<F>(
        &self,
        prompt_text: Arc<str>,
        formatted_prompt_text: Arc<str>,
        prompt_token_ids: Arc<[u32]>,
        prompt_prefill_token_count: usize,
        max_new_tokens: Option<usize>,
        sampling_options: &GemmaTextSamplingOptions,
        on_generated_ids: F,
    ) -> Result<Option<Arc<GemmaTextGenerationOutput>>, Box<dyn Error>>
    where
        F: FnMut(&[u32]) -> Result<(), String>,
    {
        let Some(metrics) =
            crate::text_runtime::cuda_exact::try_generate_cuda_nvfp4_greedy_incremental(
                &self.runtime,
                prompt_token_ids.clone(),
                max_new_tokens,
                sampling_options,
                on_generated_ids,
            )
            .map_err(|err| err.to_string())?
        else {
            return Ok(None);
        };
        let generated_token_ids = Arc::<[u32]>::from(metrics.generated_token_ids);
        let elapsed = metrics
            .time_to_first_token_elapsed
            .checked_add(metrics.steady_state_elapsed)
            .unwrap_or(metrics.time_to_first_token_elapsed + metrics.steady_state_elapsed);
        build_generation_output_from_token_ids(
            &self.runtime,
            prompt_text,
            formatted_prompt_text,
            prompt_token_ids,
            generated_token_ids.clone(),
            metrics.stop_reason,
            build_generation_metrics(
                elapsed,
                prompt_prefill_token_count,
                generated_token_ids.len(),
                metrics.time_to_first_token_elapsed,
            ),
        )
        .map(Some)
        .map_err(|err| err.into())
    }

    fn prepare_image_prompt(
        &self,
        image_path: &Path,
        prompt_text: &str,
        prompt_format: GemmaPromptFormat,
    ) -> Result<PreparedImagePrompt, Box<dyn Error>> {
        let prompt_with_image = format!(
            "{} {}",
            self.runtime.weights.snapshot.tokenizer_config.image_token, prompt_text
        );
        let formatted_prompt_text = self
            .runtime
            .format_prompt_text(&prompt_with_image, prompt_format);
        let mut vision_runtime = self
            .vision_runtime
            .lock()
            .map_err(|_| "vision runtime mutex poisoned".to_string())?;
        if vision_runtime.is_none() {
            *vision_runtime = Some(GemmaVisionRuntime::load(&self.runtime.weights));
        }
        prepare_image_prompt(
            &self.runtime.weights,
            &self.runtime.tokenizer,
            vision_runtime
                .as_mut()
                .ok_or_else(|| "vision runtime did not initialize".to_string())?,
            &formatted_prompt_text,
            image_path,
        )
        .map_err(|err| err.into())
    }

    fn prepare_preformatted_image_prompt(
        &self,
        image_path: &Path,
        formatted_prompt_text: &str,
    ) -> Result<PreparedImagePrompt, Box<dyn Error>> {
        let mut vision_runtime = self
            .vision_runtime
            .lock()
            .map_err(|_| "vision runtime mutex poisoned".to_string())?;
        if vision_runtime.is_none() {
            *vision_runtime = Some(GemmaVisionRuntime::load(&self.runtime.weights));
        }
        prepare_image_prompt(
            &self.runtime.weights,
            &self.runtime.tokenizer,
            vision_runtime
                .as_mut()
                .ok_or_else(|| "vision runtime did not initialize".to_string())?,
            formatted_prompt_text,
            image_path,
        )
        .map_err(|err| err.into())
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
        let mut rng = MlxTextSamplingRng::new(0);
        self.generate_from_formatted_arcs_with_rng(
            prompt_text,
            formatted_prompt_text,
            Some(options.max_new_tokens),
            &GemmaTextSamplingOptions::from_generation_config(
                &self.runtime.weights.snapshot.generation_config,
            ),
            &mut rng,
        )
    }

    pub fn generate_multimodal(
        &self,
        image_path: impl AsRef<Path>,
        prompt_text: impl Into<String>,
        options: GemmaTextGenerationOptions,
    ) -> Result<Arc<GemmaTextGenerationOutput>, Box<dyn Error>> {
        let prompt_text = Arc::<str>::from(prompt_text.into());
        let prepared = self.prepare_image_prompt(
            image_path.as_ref(),
            prompt_text.as_ref(),
            options.prompt_format,
        )?;
        let formatted_prompt_text = Arc::<str>::from(prepared.formatted_prompt_text.clone());
        let prompt_token_ids = Arc::<[u32]>::from(prepared.prompt_token_ids);
        let prompt_embedding_rows = prepared.prompt_embedding_rows;
        let mut rng = MlxTextSamplingRng::new(0);
        let started = Instant::now();
        let mut time_to_first_token_elapsed = None;
        let (generated_token_ids, stop_reason) = generate_sampled_token_ids_from_embedding_rows(
            &self.runtime,
            prompt_token_ids.clone(),
            prompt_embedding_rows,
            Some(options.max_new_tokens),
            &GemmaTextSamplingOptions::from_generation_config(
                &self.runtime.weights.snapshot.generation_config,
            ),
            &mut rng,
            |generated_token_ids| {
                if time_to_first_token_elapsed.is_none() && !generated_token_ids.is_empty() {
                    time_to_first_token_elapsed = Some(started.elapsed());
                }
                Ok(())
            },
        )?;
        let elapsed = started.elapsed();
        let prompt_token_count = prompt_token_ids.len();
        let generated_token_count = generated_token_ids.len();
        build_generation_output_from_token_ids(
            &self.runtime,
            prompt_text,
            formatted_prompt_text,
            prompt_token_ids,
            generated_token_ids,
            stop_reason,
            build_generation_metrics(
                elapsed,
                prompt_token_count,
                generated_token_count,
                time_to_first_token_elapsed.unwrap_or(elapsed),
            ),
        )
        .map_err(|err| err.into())
    }

    pub fn stream_generate_multimodal<F>(
        &self,
        image_path: impl AsRef<Path>,
        prompt_text: impl Into<String>,
        options: GemmaTextGenerationOptions,
        mut on_text_delta: F,
    ) -> Result<Arc<GemmaTextGenerationOutput>, Box<dyn Error>>
    where
        F: FnMut(&str) -> Result<(), Box<dyn Error>>,
    {
        let prompt_text = Arc::<str>::from(prompt_text.into());
        let prepared = self.prepare_image_prompt(
            image_path.as_ref(),
            prompt_text.as_ref(),
            options.prompt_format,
        )?;
        let formatted_prompt_text = Arc::<str>::from(prepared.formatted_prompt_text.clone());
        let prompt_token_ids = Arc::<[u32]>::from(prepared.prompt_token_ids);
        let prompt_embedding_rows = prepared.prompt_embedding_rows;
        let sampling_options = GemmaTextSamplingOptions::from_generation_config(
            &self.runtime.weights.snapshot.generation_config,
        );
        let mut rng = MlxTextSamplingRng::new(0);
        let mut detokenizer = self.runtime.tokenizer.streaming_detokenizer(true);
        let skip_special_token_ids = self.runtime.tokenizer.special_token_ids().to_vec();
        let started = Instant::now();
        let mut time_to_first_token_elapsed = None;
        let (generated_token_ids, stop_reason) = generate_sampled_token_ids_from_embedding_rows(
            &self.runtime,
            prompt_token_ids.clone(),
            prompt_embedding_rows,
            Some(options.max_new_tokens),
            &sampling_options,
            &mut rng,
            |generated_token_ids| {
                if time_to_first_token_elapsed.is_none() && !generated_token_ids.is_empty() {
                    time_to_first_token_elapsed = Some(started.elapsed());
                }
                let Some(&token_id) = generated_token_ids.last() else {
                    return Ok(());
                };
                let delta = detokenizer.add_token(token_id, &skip_special_token_ids);
                if !delta.is_empty() {
                    on_text_delta(&delta).map_err(|err| err.to_string())?;
                }
                Ok(())
            },
        )?;
        let final_delta = detokenizer.finalize();
        if !final_delta.is_empty() {
            on_text_delta(&final_delta)?;
        }
        let elapsed = started.elapsed();
        let prompt_token_count = prompt_token_ids.len();
        let generated_token_count = generated_token_ids.len();
        build_generation_output_from_token_ids(
            &self.runtime,
            prompt_text,
            formatted_prompt_text,
            prompt_token_ids,
            generated_token_ids,
            stop_reason,
            build_generation_metrics(
                elapsed,
                prompt_token_count,
                generated_token_count,
                time_to_first_token_elapsed.unwrap_or(elapsed),
            ),
        )
        .map_err(|err| err.into())
    }

    pub fn generate_preformatted(
        &self,
        formatted_prompt_text: impl Into<String>,
        max_new_tokens: Option<usize>,
    ) -> Result<Arc<GemmaTextGenerationOutput>, Box<dyn Error>> {
        let mut rng = MlxTextSamplingRng::new(0);
        self.generate_preformatted_with_rng(formatted_prompt_text, max_new_tokens, &mut rng)
    }

    pub(crate) fn generate_preformatted_with_rng(
        &self,
        formatted_prompt_text: impl Into<String>,
        max_new_tokens: Option<usize>,
        rng: &mut MlxTextSamplingRng,
    ) -> Result<Arc<GemmaTextGenerationOutput>, Box<dyn Error>> {
        self.generate_preformatted_with_rng_and_sampling(
            formatted_prompt_text,
            max_new_tokens,
            &GemmaTextSamplingOptions::from_generation_config(
                &self.runtime.weights.snapshot.generation_config,
            ),
            rng,
        )
    }

    pub(crate) fn generate_preformatted_with_rng_and_sampling(
        &self,
        formatted_prompt_text: impl Into<String>,
        max_new_tokens: Option<usize>,
        sampling_options: &GemmaTextSamplingOptions,
        rng: &mut MlxTextSamplingRng,
    ) -> Result<Arc<GemmaTextGenerationOutput>, Box<dyn Error>> {
        let formatted_prompt_text = Arc::<str>::from(formatted_prompt_text.into());
        self.generate_from_formatted_arcs_with_rng(
            formatted_prompt_text.clone(),
            formatted_prompt_text,
            max_new_tokens,
            sampling_options,
            rng,
        )
    }

    pub(crate) fn generate_preformatted_multimodal_with_rng_and_sampling(
        &self,
        image_path: impl AsRef<Path>,
        formatted_prompt_text: impl Into<String>,
        max_new_tokens: Option<usize>,
        sampling_options: &GemmaTextSamplingOptions,
        rng: &mut MlxTextSamplingRng,
    ) -> Result<Arc<GemmaTextGenerationOutput>, Box<dyn Error>> {
        let formatted_prompt_text = Arc::<str>::from(formatted_prompt_text.into());
        let prompt_text = formatted_prompt_text.clone();
        let prepared = self.prepare_preformatted_image_prompt(
            image_path.as_ref(),
            formatted_prompt_text.as_ref(),
        )?;
        let prompt_token_ids = Arc::<[u32]>::from(prepared.prompt_token_ids);
        let prompt_embedding_rows = prepared.prompt_embedding_rows;
        let started = Instant::now();
        let mut time_to_first_token_elapsed = None;
        let (generated_token_ids, stop_reason) = generate_sampled_token_ids_from_embedding_rows(
            &self.runtime,
            prompt_token_ids.clone(),
            prompt_embedding_rows,
            max_new_tokens,
            sampling_options,
            rng,
            |generated_token_ids| {
                if time_to_first_token_elapsed.is_none() && !generated_token_ids.is_empty() {
                    time_to_first_token_elapsed = Some(started.elapsed());
                }
                Ok(())
            },
        )?;
        let elapsed = started.elapsed();
        let prompt_token_count = prompt_token_ids.len();
        let generated_token_count = generated_token_ids.len();
        build_generation_output_from_token_ids(
            &self.runtime,
            prompt_text,
            formatted_prompt_text,
            prompt_token_ids,
            generated_token_ids,
            stop_reason,
            build_generation_metrics(
                elapsed,
                prompt_token_count,
                generated_token_count,
                time_to_first_token_elapsed.unwrap_or(elapsed),
            ),
        )
        .map_err(|err| err.into())
    }

    pub fn stream_generate_preformatted<F>(
        &self,
        formatted_prompt_text: impl Into<String>,
        max_new_tokens: Option<usize>,
        on_text_delta: F,
    ) -> Result<Arc<GemmaTextGenerationOutput>, Box<dyn Error>>
    where
        F: FnMut(&str) -> Result<(), Box<dyn Error>>,
    {
        let mut rng = MlxTextSamplingRng::new(0);
        self.stream_generate_preformatted_with_rng(
            formatted_prompt_text,
            max_new_tokens,
            &mut rng,
            on_text_delta,
        )
    }

    pub(crate) fn stream_generate_preformatted_with_rng<F>(
        &self,
        formatted_prompt_text: impl Into<String>,
        max_new_tokens: Option<usize>,
        rng: &mut MlxTextSamplingRng,
        on_text_delta: F,
    ) -> Result<Arc<GemmaTextGenerationOutput>, Box<dyn Error>>
    where
        F: FnMut(&str) -> Result<(), Box<dyn Error>>,
    {
        self.stream_generate_preformatted_with_rng_and_sampling(
            formatted_prompt_text,
            max_new_tokens,
            &GemmaTextSamplingOptions::from_generation_config(
                &self.runtime.weights.snapshot.generation_config,
            ),
            rng,
            on_text_delta,
        )
    }

    pub(crate) fn stream_generate_preformatted_with_rng_and_sampling<F>(
        &self,
        formatted_prompt_text: impl Into<String>,
        max_new_tokens: Option<usize>,
        sampling_options: &GemmaTextSamplingOptions,
        rng: &mut MlxTextSamplingRng,
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
        let started = Instant::now();
        let mut time_to_first_token_elapsed = None;
        let (generated_token_ids, stop_reason) = if sampling_options.allow_thought {
            let mut streamed_text = String::new();
            generate_sampled_token_ids(
                &self.runtime,
                prompt_token_ids.clone(),
                max_new_tokens,
                sampling_options,
                rng,
                |generated_token_ids| {
                    if time_to_first_token_elapsed.is_none() && !generated_token_ids.is_empty() {
                        time_to_first_token_elapsed = Some(started.elapsed());
                    }
                    let partial_text = self
                        .decode_generated_text(generated_token_ids)
                        .map_err(|err| err.to_string())?;
                    if let Some(delta) = partial_text.strip_prefix(&streamed_text) {
                        if !delta.is_empty() {
                            on_text_delta(delta).map_err(|err| err.to_string())?;
                            streamed_text.push_str(delta);
                        }
                    }
                    Ok(())
                },
            )?
        } else {
            let mut detokenizer = self.runtime.tokenizer.streaming_detokenizer(true);
            let skip_special_token_ids = self.runtime.tokenizer.special_token_ids().to_vec();
            let (generated_token_ids, stop_reason) = generate_sampled_token_ids(
                &self.runtime,
                prompt_token_ids.clone(),
                max_new_tokens,
                sampling_options,
                rng,
                |generated_token_ids| {
                    if time_to_first_token_elapsed.is_none() && !generated_token_ids.is_empty() {
                        time_to_first_token_elapsed = Some(started.elapsed());
                    }
                    let Some(&token_id) = generated_token_ids.last() else {
                        return Ok(());
                    };
                    let delta = detokenizer.add_token(token_id, &skip_special_token_ids);
                    if !delta.is_empty() {
                        on_text_delta(&delta).map_err(|err| err.to_string())?;
                    }
                    Ok(())
                },
            )?;
            let final_delta = detokenizer.finalize();
            if !final_delta.is_empty() {
                on_text_delta(&final_delta).map_err(|err| err.to_string())?;
            }
            (generated_token_ids, stop_reason)
        };
        let elapsed = started.elapsed();

        let prompt_token_count = prompt_token_ids.len();
        let generated_token_count = generated_token_ids.len();
        let output = build_generation_output_from_token_ids(
            &self.runtime,
            prompt_text,
            formatted_prompt_text,
            prompt_token_ids,
            generated_token_ids,
            stop_reason,
            build_generation_metrics(
                elapsed,
                prompt_token_count,
                generated_token_count,
                time_to_first_token_elapsed.unwrap_or(elapsed),
            ),
        )?;
        Ok(output)
    }

    pub(crate) fn stream_generate_preformatted_multimodal_with_rng_and_sampling<F>(
        &self,
        image_path: impl AsRef<Path>,
        formatted_prompt_text: impl Into<String>,
        max_new_tokens: Option<usize>,
        sampling_options: &GemmaTextSamplingOptions,
        rng: &mut MlxTextSamplingRng,
        mut on_text_delta: F,
    ) -> Result<Arc<GemmaTextGenerationOutput>, Box<dyn Error>>
    where
        F: FnMut(&str) -> Result<(), Box<dyn Error>>,
    {
        let formatted_prompt_text = Arc::<str>::from(formatted_prompt_text.into());
        let prompt_text = formatted_prompt_text.clone();
        let prepared = self.prepare_preformatted_image_prompt(
            image_path.as_ref(),
            formatted_prompt_text.as_ref(),
        )?;
        let prompt_token_ids = Arc::<[u32]>::from(prepared.prompt_token_ids);
        let prompt_embedding_rows = prepared.prompt_embedding_rows;
        let mut detokenizer = self.runtime.tokenizer.streaming_detokenizer(true);
        let skip_special_token_ids = self.runtime.tokenizer.special_token_ids().to_vec();
        let started = Instant::now();
        let mut time_to_first_token_elapsed = None;
        let (generated_token_ids, stop_reason) = generate_sampled_token_ids_from_embedding_rows(
            &self.runtime,
            prompt_token_ids.clone(),
            prompt_embedding_rows,
            max_new_tokens,
            sampling_options,
            rng,
            |generated_token_ids| {
                if time_to_first_token_elapsed.is_none() && !generated_token_ids.is_empty() {
                    time_to_first_token_elapsed = Some(started.elapsed());
                }
                let Some(&token_id) = generated_token_ids.last() else {
                    return Ok(());
                };
                let delta = detokenizer.add_token(token_id, &skip_special_token_ids);
                if !delta.is_empty() {
                    on_text_delta(&delta).map_err(|err| err.to_string())?;
                }
                Ok(())
            },
        )?;
        let final_delta = detokenizer.finalize();
        if !final_delta.is_empty() {
            on_text_delta(&final_delta)?;
        }
        let elapsed = started.elapsed();
        let prompt_token_count = prompt_token_ids.len();
        let generated_token_count = generated_token_ids.len();
        build_generation_output_from_token_ids(
            &self.runtime,
            prompt_text,
            formatted_prompt_text,
            prompt_token_ids,
            generated_token_ids,
            stop_reason,
            build_generation_metrics(
                elapsed,
                prompt_token_count,
                generated_token_count,
                time_to_first_token_elapsed.unwrap_or(elapsed),
            ),
        )
        .map_err(|err| err.into())
    }

    fn generate_from_formatted_arcs_with_rng(
        &self,
        prompt_text: Arc<str>,
        formatted_prompt_text: Arc<str>,
        max_new_tokens: Option<usize>,
        sampling_options: &GemmaTextSamplingOptions,
        rng: &mut MlxTextSamplingRng,
    ) -> Result<Arc<GemmaTextGenerationOutput>, Box<dyn Error>> {
        let prompt_token_ids = self
            .runtime
            .tokenize_prompt(formatted_prompt_text.as_ref())
            .map_err(|err| err.to_string())?;
        let started = Instant::now();
        let mut time_to_first_token_elapsed = None;
        let (generated_token_ids, stop_reason) = generate_sampled_token_ids(
            &self.runtime,
            prompt_token_ids.clone(),
            max_new_tokens,
            sampling_options,
            rng,
            |generated_token_ids| {
                if time_to_first_token_elapsed.is_none() && !generated_token_ids.is_empty() {
                    time_to_first_token_elapsed = Some(started.elapsed());
                }
                Ok(())
            },
        )?;
        let elapsed = started.elapsed();
        let prompt_token_count = prompt_token_ids.len();
        let generated_token_count = generated_token_ids.len();
        build_generation_output_from_token_ids(
            &self.runtime,
            prompt_text,
            formatted_prompt_text,
            prompt_token_ids,
            generated_token_ids,
            stop_reason,
            build_generation_metrics(
                elapsed,
                prompt_token_count,
                generated_token_count,
                time_to_first_token_elapsed.unwrap_or(elapsed),
            ),
        )
        .map_err(|err| err.into())
    }

    fn decode_generated_text(&self, generated_token_ids: &[u32]) -> Result<String, Box<dyn Error>> {
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
    generate_text_with_backend_config(
        model_path,
        prompt_text,
        options,
        GemmaExactMetalConfig::default(),
    )
}

pub fn generate_text_with_backend_config(
    model_path: PathBuf,
    prompt_text: impl Into<String>,
    options: GemmaTextGenerationOptions,
    backend_config: GemmaExactMetalConfig,
) -> Result<Arc<GemmaTextGenerationOutput>, Box<dyn Error>> {
    GemmaTextModel::load_with_backend_config(model_path, backend_config)?
        .generate(prompt_text, options)
}

pub fn generate_multimodal_text(
    model_path: PathBuf,
    image_path: impl AsRef<Path>,
    prompt_text: impl Into<String>,
    options: GemmaTextGenerationOptions,
) -> Result<Arc<GemmaTextGenerationOutput>, Box<dyn Error>> {
    generate_multimodal_text_with_backend_config(
        model_path,
        image_path,
        prompt_text,
        options,
        GemmaExactMetalConfig::default(),
    )
}

pub fn generate_multimodal_text_with_backend_config(
    model_path: PathBuf,
    image_path: impl AsRef<Path>,
    prompt_text: impl Into<String>,
    options: GemmaTextGenerationOptions,
    backend_config: GemmaExactMetalConfig,
) -> Result<Arc<GemmaTextGenerationOutput>, Box<dyn Error>> {
    GemmaTextModel::load_with_backend_config(model_path, backend_config)?.generate_multimodal(
        image_path,
        prompt_text,
        options,
    )
}

pub fn probe_exact_prefill_with_backend_config(
    model_path: PathBuf,
    prompt_text: impl Into<String>,
    prompt_format: GemmaPromptFormat,
    backend_config: GemmaExactMetalConfig,
) -> Result<GemmaExactPrefillProbeOutput, Box<dyn Error>> {
    let prompt_text = Arc::<str>::from(prompt_text.into());
    let runtime = GemmaTextRuntimeSession::load_with_backend_config(&model_path, backend_config)
        .map_err(|err| err.to_string())?;
    let formatted_prompt_text =
        Arc::<str>::from(runtime.format_prompt_text(prompt_text.as_ref(), prompt_format));
    let prompt_token_ids = runtime
        .tokenize_prompt(formatted_prompt_text.as_ref())
        .map_err(|err| err.to_string())?;
    let exact_backend = runtime
        .exact_backend
        .as_ref()
        .cloned()
        .ok_or("exact metal backend unavailable for prefill probe")?;
    let mut backend = exact_backend
        .lock()
        .map_err(|_| "exact backend mutex poisoned".to_string())?;
    let final_hidden_bf16_words = Arc::<[u16]>::from(
        backend
            .prefill_prompt_hidden_words_from_token_ids(prompt_token_ids.as_ref(), 0)
            .map_err(|err| err.to_string())?,
    );
    let next_token = backend
        .greedy_token_from_hidden_words(final_hidden_bf16_words.as_ref())
        .map_err(|err| err.to_string())?;
    let next_token_text = Arc::<str>::from(
        runtime
            .tokenizer
            .decode(&[next_token.token_id])
            .map_err(|err| err.to_string())?,
    );

    Ok(GemmaExactPrefillProbeOutput {
        model_path: runtime.model_path.clone(),
        prompt_text,
        formatted_prompt_text,
        prompt_token_ids,
        final_hidden_bf16_words,
        next_token,
        next_token_text,
    })
}

pub fn benchmark_text_generation(
    model_path: PathBuf,
    prompt_text: impl Into<String>,
    options: GemmaTextGenerationOptions,
    greedy: bool,
    warmup_iters: usize,
    measured_iters: usize,
) -> Result<GemmaTextBenchmarkOutput, Box<dyn Error>> {
    benchmark_text_generation_with_backend_config(
        model_path,
        prompt_text,
        options,
        greedy,
        warmup_iters,
        measured_iters,
        GemmaExactMetalConfig::default(),
    )
}

pub fn benchmark_text_generation_with_backend_config(
    model_path: PathBuf,
    prompt_text: impl Into<String>,
    options: GemmaTextGenerationOptions,
    greedy: bool,
    warmup_iters: usize,
    measured_iters: usize,
    backend_config: GemmaExactMetalConfig,
) -> Result<GemmaTextBenchmarkOutput, Box<dyn Error>> {
    if measured_iters == 0 {
        return Err("benchmark requires at least one measured iteration".into());
    }

    let prompt_text = Arc::<str>::from(prompt_text.into());
    let load_started = Instant::now();
    let runtime = GemmaTextRuntimeSession::load_with_backend_config(&model_path, backend_config)
        .map_err(|err| err.to_string())?;
    let formatted_prompt_text =
        Arc::<str>::from(runtime.format_prompt_text(prompt_text.as_ref(), options.prompt_format));
    let prompt_token_ids = runtime.tokenize_prompt(formatted_prompt_text.as_ref())?;
    if greedy {
        if let Some(output) = crate::text_runtime::cuda_exact::try_benchmark_cuda_nvfp4_greedy(
            &runtime,
            prompt_text.clone(),
            formatted_prompt_text.clone(),
            prompt_token_ids.clone(),
            &options,
            warmup_iters,
            measured_iters,
            load_started,
        )? {
            return Ok(output);
        }
    }
    let exact_backend = if runtime.has_exact_backend() {
        Some(runtime.exact_backend()?)
    } else {
        None
    };
    let load_duration = load_started.elapsed();
    let sampling_options = if greedy {
        GemmaTextSamplingOptions::from_generation_config(
            &runtime.weights.snapshot.generation_config,
        )
        .greedy_variant()
    } else {
        GemmaTextSamplingOptions::from_generation_config(
            &runtime.weights.snapshot.generation_config,
        )
    };

    if runtime.has_exact_backend() {
        for _ in 0..warmup_iters {
            runtime
                .start_generation_graph(prompt_token_ids.clone(), Some(options.max_new_tokens))?
                .finish_snapshot()?;
        }
        if let Some(exact_backend) = &exact_backend {
            exact_backend
                .lock()
                .map_err(|_| "exact backend mutex poisoned".to_string())?
                .reset_runtime_counters();
        }
    } else {
        for _ in 0..warmup_iters {
            let mut rng = MlxTextSamplingRng::new(0);
            let _ = generate_sampled_token_ids(
                &runtime,
                prompt_token_ids.clone(),
                Some(options.max_new_tokens),
                &sampling_options,
                &mut rng,
                |_| Ok(()),
            )?;
        }
    }

    let started = Instant::now();
    let mut total_generated_tokens = 0usize;
    let mut time_to_first_token_elapsed = Duration::ZERO;
    let mut steady_state_elapsed = Duration::ZERO;
    let mut steady_state_generated_tokens = 0usize;
    let mut last_generated_token_ids = Arc::<[u32]>::from(Vec::<u32>::new());
    if runtime.has_exact_backend() {
        for _ in 0..measured_iters {
            let ttft_started = Instant::now();
            let graph = runtime
                .start_generation_graph(prompt_token_ids.clone(), Some(options.max_new_tokens))?;
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
    } else {
        for _ in 0..measured_iters {
            let iter_started = Instant::now();
            let mut first_token_elapsed = None::<Duration>;
            let mut rng = MlxTextSamplingRng::new(0);
            let (generated_token_ids, _stop_reason) = generate_sampled_token_ids(
                &runtime,
                prompt_token_ids.clone(),
                Some(options.max_new_tokens),
                &sampling_options,
                &mut rng,
                |ids| {
                    if first_token_elapsed.is_none() && !ids.is_empty() {
                        first_token_elapsed = Some(iter_started.elapsed());
                    }
                    Ok(())
                },
            )?;
            let iter_elapsed = iter_started.elapsed();
            let ttft_elapsed = first_token_elapsed.unwrap_or(iter_elapsed);
            let first_generated_count = usize::from(!generated_token_ids.is_empty());
            time_to_first_token_elapsed += ttft_elapsed;
            steady_state_elapsed += iter_elapsed.saturating_sub(ttft_elapsed);
            total_generated_tokens += generated_token_ids.len();
            steady_state_generated_tokens += generated_token_ids
                .len()
                .saturating_sub(first_generated_count);
            last_generated_token_ids = generated_token_ids;
        }
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
    let metal_counters = if let Some(exact_backend) = &exact_backend {
        exact_backend
            .lock()
            .map_err(|_| "exact backend mutex poisoned".to_string())?
            .runtime_counters()
    } else {
        MetalRuntimeCounters::default()
    };

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
