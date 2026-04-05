use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use makepad_ggml::backend::metal::{MetalBuffer, MetalRuntime};
use makepad_ggml::{ggml_row_size_for_type, TensorType};

use crate::error::{LlamaError, Result};
use crate::model::LlamaModel;
use crate::plan::ModelExecutionPlan;
use crate::runtime::{
    allocate_hybrid_shared_cache_tensors,
    compile_hybrid_decode_metal_with_shared_runtime_and_state_and_outputs_and_attention_key_count,
    create_metal_context_buffer_with_runtime, reserve_hybrid_decode_main_buffer_size,
    CompiledHybridDecodeMetal, HybridCacheLayout, HybridCacheShape, HybridCacheTypes,
    HybridDecodeBatchLayout, HybridDecodeRun, HybridDecodeSpec, HybridLayerSpec,
    HybridSharedCacheTensorIds, LogitsProbeInput,
};
use crate::vocab::LlamaVocab;
use crate::weights::LoadedGgufWeights;

const DEFAULT_EXTRA_ACTIVATION_BYTES: usize = 512 << 20;
const DEFAULT_PREFILL_BATCH_SIZE: usize = 1;
const GRAPH_RESERVE_RETRY_BYTES: usize = 64 << 20;
const MAX_GRAPH_RESERVE_RETRIES: usize = 4;

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

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct SessionGraphParams {
    n_tokens: usize,
    n_outputs: usize,
    attention_key_count: usize,
}

impl SessionGraphParams {
    fn new(n_tokens: usize, n_outputs: usize, attention_key_count: usize) -> Self {
        Self {
            n_tokens,
            n_outputs,
            attention_key_count,
        }
    }

    fn greedy(n_tokens: usize, attention_key_count: usize) -> Self {
        Self::new(n_tokens, 1, attention_key_count)
    }

    fn token_generation(max_context: usize) -> Self {
        Self::greedy(1, max_context)
    }
}

struct SessionGraphSet {
    shared_runtime: MetalRuntime,
    shared_cache: HybridSharedCacheTensorIds,
    shared_main_buffer: MetalBuffer,
    compiled_by_params: BTreeMap<SessionGraphParams, CompiledHybridDecodeMetal>,
}

impl SessionGraphSet {
    fn graph_for_mut(
        &mut self,
        params: SessionGraphParams,
    ) -> Option<&mut CompiledHybridDecodeMetal> {
        self.compiled_by_params.get_mut(&params)
    }

    fn has_graph(&self, params: SessionGraphParams) -> bool {
        self.compiled_by_params.contains_key(&params)
    }

    fn insert_graph(&mut self, params: SessionGraphParams, compiled: CompiledHybridDecodeMetal) {
        self.compiled_by_params.insert(params, compiled);
    }

    fn evict_graphs_except(&mut self, keep: SessionGraphParams) {
        self.compiled_by_params.retain(|params, _| *params == keep);
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
            prompt_batch_capacity(self.config.prefill_batch_size, self.max_context),
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
        let spec = model.hybrid_decode_spec(
            cache_shape.n_ctx_seq,
            cache_shape.n_seq_max,
            config.attention_k_type,
            config.attention_v_type,
            config.recurrent_r_type,
            config.recurrent_s_type,
        )?;
        let cache_bytes = if let Some(template) = plan.hybrid_cache.as_ref() {
            HybridCacheLayout::new(template.materialize(cache_shape, cache_types))?.total_bytes
        } else {
            attention_cache_bytes_from_spec(&spec)?
        };
        let context_extra_bytes = cache_bytes
            .checked_add(config.extra_activation_bytes)
            .ok_or_else(|| LlamaError::format("overflow computing session activation bytes"))?;
        let max_context_usize = usize::try_from(max_context)
            .map_err(|_| LlamaError::format("session max_context does not fit in usize"))?;
        let (weights, graphs) = build_runtime_state(
            &model,
            &plan,
            &spec,
            context_extra_bytes,
            prompt_batch_capacity(config.prefill_batch_size, max_context_usize),
        )?;

        Ok(Self {
            model,
            vocab,
            plan,
            spec,
            config,
            max_context: max_context_usize,
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
        let graph_params = SessionGraphParams::greedy(batch_size, cache_tokens);
        self.ensure_compiled_graph(graph_params)?;
        let run = {
            let compiled = self
                .graphs
                .graph_for_mut(graph_params)
                .ok_or_else(|| LlamaError::format("compiled graph params were not cached"))?;
            let output_ids = [i32::try_from(batch_size - 1)
                .map_err(|_| LlamaError::format("session output id does not fit in i32"))?];
            let mut layout = HybridDecodeBatchLayout::from_contiguous_positions_and_outputs(
                &positions,
                cache_tokens,
                &output_ids,
            )?;
            if compiled.decode().input_recurrent_state_rows.is_none() {
                layout.recurrent_state_rows.clear();
            }
            compiled
                .execute_logits_only_with_layout(LogitsProbeInput::TokenIds(token_ids), &layout)?
        };
        self.token_ids.extend_from_slice(token_ids);
        self.last_run = Some(collapse_last_token_run(run)?);
        Ok(())
    }

    fn ensure_compiled_graph(&mut self, params: SessionGraphParams) -> Result<()> {
        if self.graphs.has_graph(params) {
            return Ok(());
        }
        for attempt in 0..=MAX_GRAPH_RESERVE_RETRIES {
            self.graphs.evict_graphs_except(params);
            match compile_hybrid_decode_metal_with_shared_runtime_and_state_and_outputs_and_attention_key_count(
                &mut self.weights,
                &self.spec,
                &self.graphs.shared_runtime,
                &self.graphs.shared_cache,
                &self.graphs.shared_main_buffer,
                params.n_tokens,
                params.n_outputs,
                params.attention_key_count,
            ) {
                Ok(compiled) => {
                    self.graphs.insert_graph(params, compiled);
                    return Ok(());
                }
                Err(err)
                    if attempt < MAX_GRAPH_RESERVE_RETRIES
                        && should_retry_graph_reserve(&err)
                        && self.token_ids.is_empty()
                        && self.last_run.is_none() =>
                {
                    self.context_extra_bytes = self
                        .context_extra_bytes
                        .checked_add(GRAPH_RESERVE_RETRY_BYTES)
                        .ok_or_else(|| {
                            LlamaError::format("overflow growing session activation reserve")
                        })?;
                    let (weights, graphs) = build_runtime_state(
                        &self.model,
                        &self.plan,
                        &self.spec,
                        self.context_extra_bytes,
                        prompt_batch_capacity(self.config.prefill_batch_size, self.max_context),
                    )?;
                    self.weights = weights;
                    self.graphs = graphs;
                }
                Err(err) => return Err(err),
            }
        }
        Err(LlamaError::format(
            "session graph reserve retry loop exhausted unexpectedly",
        ))
    }
}

fn resolve_max_context(model: &LlamaModel, config: LlamaSessionConfig) -> Result<u32> {
    let max_context = config.max_context.unwrap_or(model.context_length()?);
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
    prompt_batch_capacity: usize,
) -> Result<(LoadedGgufWeights, SessionGraphSet)> {
    let mut extra_bytes = context_extra_bytes;
    for attempt in 0..=MAX_GRAPH_RESERVE_RETRIES {
        let mut weights = plan
            .full_weights
            .allocate_and_load_with_extra(&model.gguf, extra_bytes)?;
        let shared_runtime = MetalRuntime::new().map_err(LlamaError::unsupported)?;
        let shared_cache =
            allocate_hybrid_shared_cache_tensors(&mut weights.ctx, &weights.tensor_ids, spec)?;
        let prompt_batch_capacity = prompt_batch_capacity.max(1);
        let mut required_main_buffer_size = reserve_hybrid_decode_main_buffer_size(
            &weights,
            spec,
            Some(&shared_cache),
            1,
            1,
            shared_runtime.features(),
        )?;
        if prompt_batch_capacity > 1 {
            required_main_buffer_size =
                required_main_buffer_size.max(reserve_hybrid_decode_main_buffer_size(
                    &weights,
                    spec,
                    Some(&shared_cache),
                    prompt_batch_capacity,
                    1,
                    shared_runtime.features(),
                )?);
        }
        if required_main_buffer_size > weights.ctx.mem_size() {
            if attempt < MAX_GRAPH_RESERVE_RETRIES {
                extra_bytes = extra_bytes
                    .checked_add(required_main_buffer_size - weights.ctx.mem_size())
                    .ok_or_else(|| {
                        LlamaError::format("overflow growing session activation reserve")
                    })?;
                continue;
            }
            return Err(LlamaError::format(format!(
                "shared Metal main buffer reserve is too small: got {}, need at least {}",
                weights.ctx.mem_size(),
                required_main_buffer_size
            )));
        }
        let shared_main_buffer =
            create_metal_context_buffer_with_runtime(&shared_runtime, &weights.ctx)?;
        let mut compiled_by_params = BTreeMap::new();
        let build_result = (|| {
            let token_generation =
                compile_hybrid_decode_metal_with_shared_runtime_and_state_and_outputs_and_attention_key_count(
                    &mut weights,
                    spec,
                    &shared_runtime,
                    &shared_cache,
                    &shared_main_buffer,
                    1,
                    1,
                    session_attention_key_count(spec)?,
                )?;
            compiled_by_params.insert(
                SessionGraphParams::token_generation(session_attention_key_count(spec)?),
                token_generation,
            );
            Ok::<(), LlamaError>(())
        })();
        match build_result {
            Ok(()) => {
                return Ok((
                    weights,
                    SessionGraphSet {
                        shared_runtime,
                        shared_cache,
                        shared_main_buffer,
                        compiled_by_params,
                    },
                ));
            }
            Err(err) if attempt < MAX_GRAPH_RESERVE_RETRIES && should_retry_graph_reserve(&err) => {
                extra_bytes = extra_bytes
                    .checked_add(GRAPH_RESERVE_RETRY_BYTES)
                    .ok_or_else(|| {
                        LlamaError::format("overflow growing session activation reserve")
                    })?;
            }
            Err(err) => return Err(err),
        }
    }
    Err(LlamaError::format(
        "session graph reserve retry loop exhausted unexpectedly",
    ))
}

fn should_retry_graph_reserve(err: &LlamaError) -> bool {
    match err {
        LlamaError::Format(msg) => {
            msg.contains("context out of memory allocating")
                || msg.contains("shared Metal main buffer is too small")
        }
        LlamaError::Io(_) | LlamaError::Unsupported(_) => false,
    }
}

fn prompt_batch_capacity(prefill_batch_size: usize, max_context: usize) -> usize {
    prefill_batch_size.max(1).min(max_context.max(1))
}

fn attention_cache_bytes_from_spec(spec: &HybridDecodeSpec) -> Result<usize> {
    let mut total = 0usize;
    let mut seen_attention_layers = BTreeSet::new();
    for layer in &spec.layers {
        match layer {
            HybridLayerSpec::Attention { decode, .. } => {
                if !seen_attention_layers.insert(decode.cache_layer_index) {
                    continue;
                }
                let k_width = u64::from(decode.block.k_head_dim)
                    .checked_mul(u64::from(decode.block.kv_head_count))
                    .ok_or_else(|| LlamaError::format("overflow computing attention K width"))?;
                let v_width = u64::from(decode.block.v_head_dim)
                    .checked_mul(u64::from(decode.block.kv_head_count))
                    .ok_or_else(|| LlamaError::format("overflow computing attention V width"))?;
                let k_elements = k_width
                    .checked_mul(u64::from(decode.cache.max_context))
                    .and_then(|v| v.checked_mul(u64::from(decode.cache.max_sequences)))
                    .ok_or_else(|| {
                        LlamaError::format("overflow computing attention K cache elements")
                    })?;
                let v_elements = v_width
                    .checked_mul(u64::from(decode.cache.max_context))
                    .and_then(|v| v.checked_mul(u64::from(decode.cache.max_sequences)))
                    .ok_or_else(|| {
                        LlamaError::format("overflow computing attention V cache elements")
                    })?;
                let k_bytes = ggml_row_size_for_type(
                    decode.cache.k_type,
                    i64::try_from(k_elements).map_err(|_| {
                        LlamaError::format("attention K elements do not fit in i64")
                    })?,
                )
                .map_err(LlamaError::format)?;
                let v_bytes = ggml_row_size_for_type(
                    decode.cache.v_type,
                    i64::try_from(v_elements).map_err(|_| {
                        LlamaError::format("attention V elements do not fit in i64")
                    })?,
                )
                .map_err(LlamaError::format)?;
                total = total
                    .checked_add(k_bytes)
                    .and_then(|v| v.checked_add(v_bytes))
                    .ok_or_else(|| {
                        LlamaError::format("overflow computing attention cache bytes")
                    })?;
            }
            HybridLayerSpec::Recurrent { .. } => {
                return Err(LlamaError::unsupported(
                    "session cache sizing without a hybrid_cache template is not implemented for recurrent layers"
                        .to_string(),
                ));
            }
        }
    }
    Ok(total)
}

fn session_attention_key_count(spec: &HybridDecodeSpec) -> Result<usize> {
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
