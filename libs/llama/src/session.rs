use std::collections::BTreeMap;
use std::path::Path;

use makepad_ggml::backend::metal::MetalBuffer;
use makepad_ggml::TensorType;

use crate::error::{LlamaError, Result};
use crate::model::LlamaModel;
use crate::plan::ModelExecutionPlan;
use crate::qwen35moe_runtime::qwen35moe_hybrid_decode_spec;
use crate::runtime::{
    allocate_hybrid_shared_cache_tensors, compile_hybrid_prompt_processing_metal_with_shared_state,
    compile_hybrid_token_generation_metal_with_shared_state, create_metal_context_buffer,
    execute_hybrid_decode_graph_metal_cached_logits_only, CompiledHybridDecodeMetal,
    HybridCacheLayout, HybridCacheShape, HybridCacheTypes, HybridDecodeRun, HybridDecodeSpec,
    HybridSharedCacheTensorIds, LogitsProbeInput,
};
use crate::vocab::LlamaVocab;
use crate::weights::LoadedGgufWeights;

const DEFAULT_EXTRA_ACTIVATION_BYTES: usize = 512 << 20;
const DEFAULT_PREFILL_BATCH_SIZE: usize = 1;

#[derive(Clone, Copy, Debug)]
pub struct LlamaSessionConfig {
    pub max_context: Option<u32>,
    pub max_sequences: u32,
    pub prefill_batch_size: usize,
    pub attention_k_type: TensorType,
    pub attention_v_type: TensorType,
    pub recurrent_r_type: TensorType,
    pub recurrent_s_type: TensorType,
    pub extra_activation_bytes: usize,
}

impl Default for LlamaSessionConfig {
    fn default() -> Self {
        Self {
            max_context: None,
            max_sequences: 1,
            prefill_batch_size: DEFAULT_PREFILL_BATCH_SIZE,
            attention_k_type: TensorType::F16,
            attention_v_type: TensorType::F16,
            recurrent_r_type: TensorType::F32,
            recurrent_s_type: TensorType::F32,
            extra_activation_bytes: DEFAULT_EXTRA_ACTIVATION_BYTES,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LlamaStopReason {
    MaxNewTokens,
    EndOfSequence,
    PaddingToken,
}

#[derive(Clone, Debug)]
pub struct LlamaGeneration {
    pub token_ids: Vec<i32>,
    pub text: String,
    pub stop_reason: LlamaStopReason,
}

struct SessionGraphSet {
    shared_cache: HybridSharedCacheTensorIds,
    shared_main_buffer: MetalBuffer,
    token_generation: CompiledHybridDecodeMetal,
    prompt_processing_by_batch: BTreeMap<usize, CompiledHybridDecodeMetal>,
}

impl SessionGraphSet {
    fn graph_for_batch(&self, batch_size: usize) -> Option<&CompiledHybridDecodeMetal> {
        if batch_size == 1 {
            Some(&self.token_generation)
        } else {
            self.prompt_processing_by_batch.get(&batch_size)
        }
    }

    fn has_batch(&self, batch_size: usize) -> bool {
        batch_size == 1 || self.prompt_processing_by_batch.contains_key(&batch_size)
    }

    fn insert_prompt_processing_graph(
        &mut self,
        batch_size: usize,
        compiled: CompiledHybridDecodeMetal,
    ) {
        if batch_size == 1 {
            self.token_generation = compiled;
        } else {
            self.prompt_processing_by_batch.insert(batch_size, compiled);
        }
    }
}

pub struct LlamaSession {
    model: LlamaModel,
    vocab: LlamaVocab,
    plan: ModelExecutionPlan,
    spec: HybridDecodeSpec,
    config: LlamaSessionConfig,
    max_context: usize,
    context_extra_bytes: usize,
    weights: LoadedGgufWeights,
    graphs: SessionGraphSet,
    token_ids: Vec<i32>,
    last_run: Option<HybridDecodeRun>,
}

impl LlamaSession {
    pub fn load(path: impl AsRef<Path>, config: LlamaSessionConfig) -> Result<Self> {
        Self::from_owned_model(LlamaModel::load(path)?, config)
    }

    pub fn from_model(model: &LlamaModel, config: LlamaSessionConfig) -> Result<Self> {
        Self::from_owned_model(model.clone(), config)
    }

    pub fn model(&self) -> &LlamaModel {
        &self.model
    }

    pub fn vocab(&self) -> &LlamaVocab {
        &self.vocab
    }

    pub fn config(&self) -> &LlamaSessionConfig {
        &self.config
    }

    pub fn token_ids(&self) -> &[i32] {
        &self.token_ids
    }

    pub fn token_count(&self) -> usize {
        self.token_ids.len()
    }

    pub fn max_context(&self) -> usize {
        self.max_context
    }

    pub fn remaining_context(&self) -> usize {
        self.max_context.saturating_sub(self.token_ids.len())
    }

    pub fn last_run(&self) -> Option<&HybridDecodeRun> {
        self.last_run.as_ref()
    }

    pub fn last_logits(&self) -> Option<&[f32]> {
        self.last_run.as_ref().map(|run| run.logits.as_slice())
    }

    pub fn reset(&mut self) -> Result<()> {
        let (weights, graphs) = build_runtime_state(
            &self.model,
            &self.plan,
            &self.spec,
            self.context_extra_bytes,
        )?;
        self.weights = weights;
        self.graphs = graphs;
        self.token_ids.clear();
        self.last_run = None;
        Ok(())
    }

    pub fn append_token(&mut self, token_id: i32) -> Result<()> {
        self.append_tokens(std::slice::from_ref(&token_id))
    }

    pub fn append_tokens(&mut self, token_ids: &[i32]) -> Result<()> {
        if token_ids.is_empty() {
            return Ok(());
        }
        self.ensure_capacity(token_ids.len())?;
        let prefill_batch_size = self.config.prefill_batch_size.max(1);
        let mut offset = 0;
        while offset < token_ids.len() {
            let batch_size = (token_ids.len() - offset).min(prefill_batch_size);
            self.append_token_batch(&token_ids[offset..offset + batch_size])?;
            offset += batch_size;
        }
        Ok(())
    }

    pub fn next_greedy_token(&mut self) -> Result<Option<i32>> {
        let next_token = self.greedy_candidate()?;
        if self.stop_reason_for(next_token).is_some() {
            return Ok(None);
        }
        self.append_token(next_token)?;
        Ok(Some(next_token))
    }

    pub fn continue_greedy(&mut self, max_new_tokens: usize) -> Result<LlamaGeneration> {
        let mut token_ids = Vec::with_capacity(max_new_tokens);
        let mut stop_reason = LlamaStopReason::MaxNewTokens;

        for _ in 0..max_new_tokens {
            let next_token = self.greedy_candidate()?;
            if let Some(reason) = self.stop_reason_for(next_token) {
                stop_reason = reason;
                break;
            }
            self.append_token(next_token)?;
            token_ids.push(next_token);
        }

        Ok(LlamaGeneration {
            text: self.vocab.decode_tokens(&token_ids)?,
            token_ids,
            stop_reason,
        })
    }

    fn from_owned_model(model: LlamaModel, config: LlamaSessionConfig) -> Result<Self> {
        model.validate_layout()?;
        if config.max_sequences == 0 {
            return Err(LlamaError::format(
                "session max_sequences must be at least 1",
            ));
        }

        let vocab = LlamaVocab::from_model(&model)?;
        let plan = model.execution_plan()?;
        let max_context = resolve_max_context(&model, config)?;
        let cache_shape = HybridCacheShape {
            n_ctx_seq: max_context,
            n_seq_max: config.max_sequences,
        };
        let cache_types = HybridCacheTypes {
            attention_k_type: config.attention_k_type,
            attention_v_type: config.attention_v_type,
            recurrent_r_type: config.recurrent_r_type,
            recurrent_s_type: config.recurrent_s_type,
        };
        let cache_bytes = plan
            .hybrid_cache
            .as_ref()
            .map(|template| HybridCacheLayout::new(template.materialize(cache_shape, cache_types)))
            .transpose()?
            .map_or(0, |layout| layout.total_bytes);
        let context_extra_bytes = cache_bytes
            .checked_add(config.extra_activation_bytes)
            .ok_or_else(|| LlamaError::format("overflow computing session activation bytes"))?;
        let spec = qwen35moe_hybrid_decode_spec(
            &model,
            cache_shape.n_ctx_seq,
            cache_shape.n_seq_max,
            config.attention_k_type,
            config.attention_v_type,
            config.recurrent_r_type,
            config.recurrent_s_type,
        )?;
        let (weights, graphs) = build_runtime_state(&model, &plan, &spec, context_extra_bytes)?;

        Ok(Self {
            model,
            vocab,
            plan,
            spec,
            config,
            max_context: usize::try_from(max_context)
                .map_err(|_| LlamaError::format("session max_context does not fit in usize"))?,
            context_extra_bytes,
            weights,
            graphs,
            token_ids: Vec::new(),
            last_run: None,
        })
    }

    fn ensure_capacity(&self, additional_tokens: usize) -> Result<()> {
        let total = self
            .token_ids
            .len()
            .checked_add(additional_tokens)
            .ok_or_else(|| LlamaError::format("overflow computing total session tokens"))?;
        if total > self.max_context {
            return Err(LlamaError::format(format!(
                "session context overflow: need {} tokens, max_context is {}",
                total, self.max_context
            )));
        }
        Ok(())
    }

    fn greedy_candidate(&self) -> Result<i32> {
        argmax_token_id(self.last_logits().ok_or_else(|| {
            LlamaError::format("session has no logits yet; append context tokens before continuing")
        })?)
    }

    fn stop_reason_for(&self, token_id: i32) -> Option<LlamaStopReason> {
        if Some(token_id) == self.vocab.eos_token_id() {
            Some(LlamaStopReason::EndOfSequence)
        } else if Some(token_id) == self.vocab.padding_token_id() {
            Some(LlamaStopReason::PaddingToken)
        } else {
            None
        }
    }

    fn append_token_batch(&mut self, token_ids: &[i32]) -> Result<()> {
        let batch_size = token_ids.len();
        if batch_size > 1 {
            return Err(LlamaError::unsupported(
                "multi-token prompt processing is not wired to shared cache state yet; use prefill_batch_size=1 until the prompt-processing graph family shares the same KV/recurrent tensors as token generation".to_string(),
            ));
        }
        self.ensure_compiled_batch(batch_size)?;
        let start = self.token_ids.len();
        let positions = (start..start + batch_size)
            .map(|position| {
                i32::try_from(position)
                    .map_err(|_| LlamaError::format("token position does not fit in i32"))
            })
            .collect::<Result<Vec<_>>>()?;
        let cache_tokens = start
            .checked_add(batch_size)
            .ok_or_else(|| LlamaError::format("overflow computing session cache length"))?;
        let run = {
            let compiled = self
                .graphs
                .graph_for_batch(batch_size)
                .ok_or_else(|| LlamaError::format("compiled batch graph was not cached"))?;
            execute_hybrid_decode_graph_metal_cached_logits_only(
                compiled,
                &mut self.weights,
                LogitsProbeInput::TokenIds(token_ids),
                &positions,
                cache_tokens,
            )?
        };
        self.token_ids.extend_from_slice(token_ids);
        self.last_run = Some(collapse_last_token_run(run)?);
        Ok(())
    }

    fn ensure_compiled_batch(&mut self, batch_size: usize) -> Result<()> {
        if self.graphs.has_batch(batch_size) {
            return Ok(());
        }
        if batch_size > 1 {
            return Err(LlamaError::unsupported(
                "prompt-processing graphs for batch_size>1 are not enabled until they share cache tensors with token-generation graphs".to_string(),
            ));
        }
        let compiled = compile_hybrid_prompt_processing_metal_with_shared_state(
            &mut self.weights,
            &self.spec,
            &self.graphs.shared_cache,
            &self.graphs.shared_main_buffer,
            batch_size,
        )?;
        self.graphs
            .insert_prompt_processing_graph(batch_size, compiled);
        Ok(())
    }
}

fn resolve_max_context(model: &LlamaModel, config: LlamaSessionConfig) -> Result<u32> {
    let max_context = config
        .max_context
        .unwrap_or(model.require_qwen35moe()?.context_length);
    if max_context == 0 {
        return Err(LlamaError::format("session max_context must be at least 1"));
    }
    Ok(max_context)
}

fn build_runtime_state(
    model: &LlamaModel,
    plan: &ModelExecutionPlan,
    spec: &HybridDecodeSpec,
    context_extra_bytes: usize,
) -> Result<(LoadedGgufWeights, SessionGraphSet)> {
    let mut weights = plan
        .full_weights
        .allocate_and_load_with_extra(&model.gguf, context_extra_bytes)?;
    let shared_cache =
        allocate_hybrid_shared_cache_tensors(&mut weights.ctx, &weights.tensor_ids, spec)?;
    let shared_main_buffer = create_metal_context_buffer(&weights.ctx)?;
    let token_generation = compile_hybrid_token_generation_metal_with_shared_state(
        &mut weights,
        spec,
        &shared_cache,
        &shared_main_buffer,
    )?;
    Ok((
        weights,
        SessionGraphSet {
            shared_cache,
            shared_main_buffer,
            token_generation,
            prompt_processing_by_batch: BTreeMap::new(),
        },
    ))
}

fn argmax_token_id(logits: &[f32]) -> Result<i32> {
    let (index, _) = logits
        .iter()
        .copied()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(&b.1).then_with(|| a.0.cmp(&b.0)))
        .ok_or_else(|| LlamaError::format("logit vector was empty"))?;
    i32::try_from(index).map_err(|_| LlamaError::format("argmax index does not fit in i32"))
}

fn collapse_last_token_run(run: HybridDecodeRun) -> Result<HybridDecodeRun> {
    if run.n_tokens <= 1 {
        return Ok(run);
    }

    if run.hidden_size > 0 && run.hidden.len() < run.hidden_size {
        return Err(LlamaError::format(format!(
            "hybrid decode hidden length mismatch: got {}, need at least {}",
            run.hidden.len(),
            run.hidden_size
        )));
    }

    if run.logits.len() < run.vocab_size {
        return Err(LlamaError::format(format!(
            "hybrid decode logits length mismatch: got {}, need at least {}",
            run.logits.len(),
            run.vocab_size
        )));
    }

    let inferred_tokens = if run.vocab_size > 0 && run.logits.len() % run.vocab_size == 0 {
        run.logits.len() / run.vocab_size
    } else if run.hidden_size > 0 && run.hidden.len() % run.hidden_size == 0 {
        run.hidden.len() / run.hidden_size
    } else {
        run.n_tokens
    };
    let logits_start = run.logits.len() - run.vocab_size;
    let hidden = if run.hidden_size > 0 {
        let hidden_start = run.hidden.len() - run.hidden_size;
        run.hidden[hidden_start..].to_vec()
    } else {
        Vec::new()
    };
    let selected_experts = run
        .selected_experts
        .into_iter()
        .map(|(layer_index, experts)| {
            let per_token = experts.len().checked_div(inferred_tokens).unwrap_or(0);
            let experts = if per_token == 0 || per_token * inferred_tokens != experts.len() {
                experts
            } else {
                experts[experts.len() - per_token..].to_vec()
            };
            (layer_index, experts)
        })
        .collect();

    Ok(HybridDecodeRun {
        hidden,
        logits: run.logits[logits_start..].to_vec(),
        n_tokens: 1,
        hidden_size: run.hidden_size,
        vocab_size: run.vocab_size,
        selected_experts,
    })
}
