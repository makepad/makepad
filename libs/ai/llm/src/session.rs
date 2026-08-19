use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::{ggml_row_size_for_type, TensorType};

use crate::error::{LlamaError, Result};
use crate::exec::{CompiledHybridDecode, ExecContextBuffers, ExecRuntime};
use crate::model::LlamaModel;
use crate::plan::ModelExecutionPlan;
use crate::runtime::{
    allocate_hybrid_shared_cache_tensors, HybridCacheLayout, HybridCacheShape, HybridCacheTypes,
    HybridDecodeBatchLayout, HybridDecodeRun, HybridDecodeSpec, HybridLayerSpec,
    HybridSharedCacheTensorIds, LogitsProbeInput, ProbeInputKind,
};
use crate::vocab::LlamaVocab;
use crate::weights::LoadedGgufWeights;

const DEFAULT_EXTRA_ACTIVATION_BYTES: usize = 512 << 20;
// Was 1 while batched prefill produced garbage — root cause was the non-flat
// unary metal dispatch dropping its ne0-chunk grid factor (fixed 2026-07-28,
// see makepad-ggml `executes_unary_large_rows_on_metal_when_available`).
// Batched prefill now verifies against sequential on qwen35 4B/9B up to 64.
const DEFAULT_PREFILL_BATCH_SIZE: usize = 32;
const GRAPH_RESERVE_RETRY_BYTES: usize = 64 << 20;
const MAX_GRAPH_RESERVE_RETRIES: usize = 4;
/// Attention-graph key widths round up to this bucket, bounding the number
/// of compiled graphs per batch shape at max_context / bucket.
/// llama.cpp `llama-kv-cache.cpp:1116-1126` pads live `n_kv` to
/// `max(n_pad, 256)` so `K.ne[1] % FATTN_KQ_STRIDE == 0` and
/// `fattn.cu:402` can pick VEC on Ada decode (`GQA>4`, `KV<8192`).
const GRAPH_KEY_BUCKET: usize = 256;

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
    /// Maximum MTP draft tokens per speculative step. 0 disables speculative
    /// decoding entirely, and then the nextn block is not even loaded.
    pub spec_draft_max: usize,
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
            spec_draft_max: 0,
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

/// Which of the session's decode specs a compiled graph was built from.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum SessionGraphKind {
    /// The main network, token-id input.
    Main,
    /// The main network with precomputed embeddings (image spans).
    MainEmbeddings,
    /// The main network with per-token recurrent state checkpoints, used for
    /// speculative verification batches.
    MainVerify,
    /// The `blk.N` multi-token-prediction draft head.
    MtpDraft,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct SessionGraphParams {
    kind: SessionGraphKind,
    n_tokens: usize,
    n_outputs: usize,
    attention_key_count: usize,
}

impl SessionGraphParams {
    fn new(n_tokens: usize, n_outputs: usize, attention_key_count: usize) -> Self {
        Self {
            kind: SessionGraphKind::Main,
            n_tokens,
            n_outputs,
            attention_key_count,
        }
    }

    fn greedy(n_tokens: usize, attention_key_count: usize) -> Self {
        Self::new(n_tokens, 1, attention_key_count)
    }

    fn greedy_embeddings(n_tokens: usize, attention_key_count: usize) -> Self {
        let mut params = Self::greedy(n_tokens, attention_key_count);
        params.kind = SessionGraphKind::MainEmbeddings;
        params
    }

    fn verify(n_tokens: usize, attention_key_count: usize) -> Self {
        Self {
            kind: SessionGraphKind::MainVerify,
            n_tokens,
            n_outputs: n_tokens,
            attention_key_count,
        }
    }

    fn mtp_draft(n_tokens: usize, attention_key_count: usize) -> Self {
        Self {
            kind: SessionGraphKind::MtpDraft,
            n_tokens,
            n_outputs: 1,
            attention_key_count,
        }
    }

    fn token_generation(max_context: usize) -> Self {
        Self::greedy(1, max_context)
    }
}

/// Rows in the hidden-carry ring. Sized well above a prefill chunk so the MTP
/// prefill hook can cover many main chunks per draft-graph call: that graph's
/// LM head reads ~1 GB of weights per call regardless of batch size, so fewer
/// and larger calls are far cheaper. One extra row (index `carry_ring`) is the
/// draft chain's scratch row.
const MTP_CARRY_RING_TARGET: usize = 2048;

fn mtp_carry_ring(prefill_batch_size: usize, draft_max: usize, max_context: usize) -> usize {
    // A verify batch of `draft_max + 1` positions writes rows
    // `base+1 ..= base+draft_max+1`, so the ring must never wrap onto `base`.
    let min_rows = prefill_batch_size.max(draft_max + 2);
    MTP_CARRY_RING_TARGET
        .min(max_context.max(min_rows))
        .max(min_rows)
}

fn carry_ring_bytes(hidden_size: u32, carry_ring: usize) -> Result<usize> {
    ggml_row_size_for_type(TensorType::F32, i64::from(hidden_size))
        .map_err(LlamaError::format)?
        .checked_mul(carry_ring + 2)
        .ok_or_else(|| LlamaError::format("overflow sizing the mtp hidden carry"))
}

/// Live speculative-decoding state.
#[derive(Clone, Copy, Debug)]
struct MtpRuntime {
    draft_max: usize,
    /// Recurrent r/s cache row holding the committed state.
    state_row: i32,
    /// Hidden-carry ring length. The carry tensor has `carry_ring + 2` rows:
    /// the ring, then the draft chain's scratch row, then a row that is never
    /// written so it stays zero (the `h_{-1}` the first token needs).
    carry_ring: usize,
    /// Tokens whose MTP KV has been filled (the draft head lags the main
    /// model until the prefill hook catches it up).
    mtp_filled: usize,
    drafted: u64,
    accepted: u64,
    rounds: u64,
    draft_nanos: u64,
    verify_nanos: u64,
    catchup_nanos: u64,
}

/// Counters for a speculative run: how many draft tokens were proposed, how
/// many were accepted, and how many verify batches ran.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SpeculativeStats {
    pub drafted: u64,
    pub accepted: u64,
    pub rounds: u64,
    /// Wall time inside the draft-head forwards, the verify forward, and the
    /// draft-head KV catch-up, in nanoseconds.
    pub draft_nanos: u64,
    pub verify_nanos: u64,
    pub catchup_nanos: u64,
}

impl SpeculativeStats {
    /// Draft acceptance rate, llama.cpp's `n_accept / n_drafted`.
    pub fn acceptance(&self) -> f64 {
        if self.drafted == 0 {
            0.0
        } else {
            self.accepted as f64 / self.drafted as f64
        }
    }

    /// Mean tokens committed per verify forward (1.0 == no speedup).
    pub fn tokens_per_round(&self) -> f64 {
        if self.rounds == 0 {
            0.0
        } else {
            (self.accepted + self.rounds) as f64 / self.rounds as f64
        }
    }
}

struct SessionGraphSet {
    shared_runtime: ExecRuntime,
    shared_cache: HybridSharedCacheTensorIds,
    shared_buffers: ExecContextBuffers,
    compiled_by_params: BTreeMap<SessionGraphParams, CompiledHybridDecode>,
    /// `[hidden_size, rows]` F32 State tensor holding post-final-norm hidden
    /// rows between the main graph and the MTP draft graph.
    hidden_carry: Option<crate::TensorId>,
}

impl SessionGraphSet {
    fn graph_for_mut(&mut self, params: SessionGraphParams) -> Option<&mut CompiledHybridDecode> {
        self.compiled_by_params.get_mut(&params)
    }

    fn has_graph(&self, params: SessionGraphParams) -> bool {
        self.compiled_by_params.contains_key(&params)
    }

    fn insert_graph(&mut self, params: SessionGraphParams, compiled: CompiledHybridDecode) {
        self.compiled_by_params.insert(params, compiled);
    }

    fn evict_graphs_except(&mut self, keep: SessionGraphParams) {
        self.compiled_by_params.retain(|params, _| *params == keep);
    }
}

fn shared_cache_ranges(
    ctx: &crate::Context,
    cache: &HybridSharedCacheTensorIds,
    hidden_carry: Option<crate::TensorId>,
) -> Result<Vec<(usize, usize)>> {
    let mut tensor_ids = BTreeSet::new();
    for ids in cache.attention.values() {
        tensor_ids.insert(ids.k_cache);
        tensor_ids.insert(ids.v_cache);
    }
    for ids in cache.recurrent.values() {
        tensor_ids.insert(ids.r_cache);
        tensor_ids.insert(ids.s_cache);
    }
    if let Some(carry) = hidden_carry {
        tensor_ids.insert(carry);
    }

    let mut ranges = Vec::with_capacity(tensor_ids.len());
    for tensor_id in tensor_ids {
        let tensor = ctx
            .tensor(tensor_id)
            .ok_or_else(|| LlamaError::format(format!("invalid cache tensor id {tensor_id}")))?;
        let offset = tensor.data_offset.ok_or_else(|| {
            LlamaError::format(format!("cache tensor {tensor_id} has no allocated data"))
        })?;
        ranges.push((offset, tensor.nbytes()));
    }
    ranges.sort_unstable();
    ranges.dedup();
    Ok(ranges)
}

pub struct LlamaSession {
    model: LlamaModel,
    vocab: LlamaVocab,
    plan: ModelExecutionPlan,
    spec: HybridDecodeSpec,
    spec_embeddings: HybridDecodeSpec,
    /// Main network with per-token recurrent checkpoints (speculative verify).
    spec_verify: Option<HybridDecodeSpec>,
    /// The MTP draft head.
    spec_mtp: Option<HybridDecodeSpec>,
    mtp: Option<MtpRuntime>,
    config: LlamaSessionConfig,
    max_context: usize,
    context_extra_bytes: usize,
    weights: LoadedGgufWeights,
    graphs: SessionGraphSet,
    token_ids: Vec<i32>,
    /// Next M-RoPE position. Tracks token count for pure text; falls behind it
    /// after an image span, whose n_pos is max(tokens_w, tokens_h) rather than
    /// its token count.
    rope_pos_next: i64,
    last_run: Option<HybridDecodeRun>,
}

impl LlamaSession {
    pub fn load(path: impl AsRef<Path>, config: LlamaSessionConfig) -> Result<Self> {
        Self::load_with_progress(path, config, &mut |_, _| {})
    }

    pub fn load_with_progress(
        path: impl AsRef<Path>,
        config: LlamaSessionConfig,
        progress: &mut dyn FnMut(&str, f64),
    ) -> Result<Self> {
        progress("load llm parse", 0.11);
        Self::from_owned_model_with_progress(LlamaModel::load(path)?, config, progress)
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
        let ranges = shared_cache_ranges(
            &self.weights.ctx,
            &self.graphs.shared_cache,
            self.graphs.hidden_carry,
        )?;
        self.graphs.shared_runtime.clear_state_ranges(
            &mut self.weights.ctx,
            &self.graphs.shared_buffers,
            &ranges,
        )?;
        self.token_ids.clear();
        self.rope_pos_next = 0;
        self.last_run = None;
        if let Some(mtp) = self.mtp.as_mut() {
            mtp.state_row = 0;
            mtp.mtp_filled = 0;
        }
        if std::env::var("MAKEPAD_LLM_RESET_VERIFY").is_ok() {
            self.verify_state_cleared()?;
        }
        Ok(())
    }

    /// Debug: fingerprint the read-only (weights) region of the live device
    /// buffer by sampling `samples` evenly spaced 1 MiB windows. Two calls
    /// returning different values mean something WROTE INTO THE WEIGHTS
    /// between them (an out-of-bounds kernel store) — state that no cache
    /// clear can ever undo.
    pub fn debug_weights_fingerprint(&self, samples: usize) -> Result<u64> {
        const WINDOW: usize = 1 << 20;
        let ctx = &self.weights.ctx;
        let ids: Vec<_> = self.weights.tensor_ids.values().copied().collect();
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        let step = (ids.len() / samples.max(1)).max(1);
        for id in ids.iter().step_by(step) {
            let Some(tensor) = ctx.tensor(*id) else { continue };
            let Some(offset) = tensor.data_offset else { continue };
            let len = tensor.nbytes().min(WINDOW);
            if len == 0 {
                continue;
            }
            let bytes = self.graphs.shared_runtime.read_state_range(
                &self.graphs.shared_buffers,
                offset,
                len,
            )?;
            for chunk in bytes.chunks_exact(8) {
                hash ^= u64::from_le_bytes(chunk.try_into().unwrap());
                hash = hash.wrapping_mul(0x1000_0000_01b3);
            }
        }
        Ok(hash)
    }

    /// Debug: reset, then additionally zero EVERY allocated tensor that does
    /// not overlap a GGUF weight tensor. If a normal `reset()` leaves the
    /// next run nondeterministic but this restores determinism, some kernel
    /// reads a non-weight buffer it did not write this run.
    pub fn debug_scorched_reset(&mut self) -> Result<usize> {
        self.reset()?;
        let ctx = &self.weights.ctx;
        let mut weight_ranges: Vec<(usize, usize)> = Vec::new();
        for id in self.weights.tensor_ids.values() {
            if let Some(tensor) = ctx.tensor(*id) {
                if let Some(offset) = tensor.data_offset {
                    weight_ranges.push((offset, offset + tensor.nbytes()));
                }
            }
        }
        weight_ranges.sort_unstable();
        let overlaps_weight = |start: usize, end: usize| {
            let idx = weight_ranges.partition_point(|(_, we)| *we <= start);
            weight_ranges
                .get(idx)
                .map(|(ws, _)| *ws < end)
                .unwrap_or(false)
        };
        let mut ranges: Vec<(usize, usize)> = Vec::new();
        for tensor in ctx.tensors() {
            let Some(offset) = tensor.data_offset else { continue };
            let len = tensor.nbytes();
            if len == 0 || overlaps_weight(offset, offset + len) {
                continue;
            }
            ranges.push((offset, len));
        }
        ranges.sort_unstable();
        ranges.dedup();
        let count = ranges.len();
        self.graphs.shared_runtime.clear_state_ranges(
            &mut self.weights.ctx,
            &self.graphs.shared_buffers,
            &ranges,
        )?;
        Ok(count)
    }

    /// Debug: hash every allocated context tensor's live device bytes
    /// (capped at 1 MiB each). Diffing two snapshots taken at points that
    /// should be identical (e.g. right after two different `reset()` calls)
    /// names exactly which buffers carry state across the reset.
    pub fn debug_state_snapshot(&self) -> Result<Vec<(String, usize, usize, u64)>> {
        const WINDOW: usize = 1 << 20;
        let ctx = &self.weights.ctx;
        let mut out = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        for (index, tensor) in ctx.tensors().iter().enumerate() {
            let Some(offset) = tensor.data_offset else { continue };
            let len = tensor.nbytes().min(WINDOW);
            if len == 0 || !seen.insert((offset, len)) {
                continue;
            }
            let bytes = self.graphs.shared_runtime.read_state_range(
                &self.graphs.shared_buffers,
                offset,
                len,
            )?;
            let mut hash = 0xcbf2_9ce4_8422_2325u64;
            for chunk in bytes.chunks_exact(8) {
                hash ^= u64::from_le_bytes(chunk.try_into().unwrap());
                hash = hash.wrapping_mul(0x1000_0000_01b3);
            }
            let name = tensor
                .name()
                .map(str::to_owned)
                .unwrap_or_else(|| format!("tensor#{index}"));
            out.push((name, offset, len, hash));
        }
        Ok(out)
    }

    /// Debug (MAKEPAD_LLM_RESET_VERIFY=1): read every shared cache tensor
    /// back from the live device buffer and report any that still holds
    /// nonzero bytes after `reset`. Catches a clear that silently missed the
    /// device (wrong offsets, wrong buffer, stale stream).
    fn verify_state_cleared(&self) -> Result<()> {
        let ctx = &self.weights.ctx;
        let mut named: Vec<(String, usize, usize)> = Vec::new();
        for (layer, ids) in &self.graphs.shared_cache.attention {
            for (tag, id) in [("k", ids.k_cache), ("v", ids.v_cache)] {
                let Some(tensor) = ctx.tensor(id) else { continue };
                let Some(offset) = tensor.data_offset else { continue };
                named.push((format!("attn{layer}.{tag}"), offset, tensor.nbytes()));
            }
        }
        for (layer, ids) in &self.graphs.shared_cache.recurrent {
            for (tag, id) in [("r", ids.r_cache), ("s", ids.s_cache)] {
                let Some(tensor) = ctx.tensor(id) else { continue };
                let Some(offset) = tensor.data_offset else { continue };
                named.push((format!("recur{layer}.{tag}"), offset, tensor.nbytes()));
            }
        }
        let mut dirty = 0usize;
        for (name, offset, len) in &named {
            let bytes = self.graphs.shared_runtime.read_state_range(
                &self.graphs.shared_buffers,
                *offset,
                *len,
            )?;
            let nonzero = bytes.iter().filter(|b| **b != 0).count();
            if nonzero > 0 {
                dirty += 1;
                eprintln!(
                    "[llm reset-verify] {name} STILL DIRTY: {nonzero}/{len} nonzero bytes at ctx offset {offset}"
                );
            }
        }
        eprintln!(
            "[llm reset-verify] {} cache tensors checked, {dirty} dirty",
            named.len()
        );
        Ok(())
    }

    pub fn append_token(&mut self, token_id: i32) -> Result<()> {
        self.append_tokens(std::slice::from_ref(&token_id))
    }

    pub fn append_tokens(&mut self, token_ids: &[i32]) -> Result<()> {
        self.append_tokens_with_progress(token_ids, &mut |_, _| {})
    }

    pub fn append_tokens_with_progress(
        &mut self,
        token_ids: &[i32],
        progress: &mut dyn FnMut(usize, usize),
    ) -> Result<()> {
        if token_ids.is_empty() {
            return Ok(());
        }
        self.ensure_capacity(token_ids.len())?;
        let prefill_batch_size = self.config.prefill_batch_size.max(1);
        let mut offset = 0;
        progress(0, token_ids.len());
        while offset < token_ids.len() {
            let batch_size = (token_ids.len() - offset).min(prefill_batch_size);
            self.append_token_batch(&token_ids[offset..offset + batch_size])?;
            offset += batch_size;
            progress(offset, token_ids.len());
        }
        Ok(())
    }

    /// Debug: per-cache-tensor stats over the written region (KV rows
    /// 0..cache_tokens, full recurrent state): (abs-sum, max-abs, nan count).
    /// Used to diff the state handed to decode across prefill batchings.
    pub fn debug_cache_fingerprints(&self) -> Vec<(String, f64, f32, usize)> {
        fn stats(bytes: &[u8], ty: TensorType) -> (f64, f32, usize) {
            let mut sum = 0.0f64;
            let mut max = 0.0f32;
            let mut nans = 0usize;
            match ty {
                TensorType::F32 => {
                    for chunk in bytes.chunks_exact(4) {
                        let v = f32::from_le_bytes(chunk.try_into().unwrap());
                        if v.is_nan() {
                            nans += 1;
                        } else {
                            sum += v.abs() as f64;
                            max = max.max(v.abs());
                        }
                    }
                }
                _ => {
                    for chunk in bytes.chunks_exact(2) {
                        let v = makepad_ai_cuda::quant::f16_to_f32(u16::from_le_bytes(
                            chunk.try_into().unwrap(),
                        ));
                        if v.is_nan() {
                            nans += 1;
                        } else {
                            sum += v.abs() as f64;
                            max = max.max(v.abs());
                        }
                    }
                }
            }
            (sum, max, nans)
        }
        let cache_tokens = self.token_ids.len();
        let ctx = &self.weights.ctx;
        let mut out = Vec::new();
        for (layer, ids) in &self.graphs.shared_cache.attention {
            for (tag, id) in [("k", ids.k_cache), ("v", ids.v_cache)] {
                let Some(tensor) = ctx.tensor(id) else { continue };
                let Some(offset) = tensor.data_offset else { continue };
                let len = tensor.nb[1].saturating_mul(cache_tokens);
                let Ok(bytes) = ctx.data_at(offset, len) else { continue };
                let (sum, max, nans) = stats(bytes, tensor.desc.ty);
                out.push((format!("attn{}.{}", layer, tag), sum, max, nans));
            }
        }
        for (layer, ids) in &self.graphs.shared_cache.recurrent {
            for (tag, id) in [("r", ids.r_cache), ("s", ids.s_cache)] {
                let Some(tensor) = ctx.tensor(id) else { continue };
                let Ok(bytes) = ctx.tensor_data(id) else { continue };
                let (sum, max, nans) = stats(bytes, tensor.desc.ty);
                out.push((format!("recur{}.{}", layer, tag), sum, max, nans));
            }
        }
        out
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
        if self.mtp.is_some() {
            return self.continue_greedy_speculative(max_new_tokens);
        }
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
        Self::from_owned_model_with_progress(model, config, &mut |_, _| {})
    }

    fn from_owned_model_with_progress(
        mut model: LlamaModel,
        config: LlamaSessionConfig,
        progress: &mut dyn FnMut(&str, f64),
    ) -> Result<Self> {
        // The nextn block only enters the tensor inventory (and therefore
        // VRAM) when speculative decoding will actually use it.
        model.set_load_mtp(config.spec_draft_max > 0 && model.has_mtp_block());
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
        let mut spec = model.hybrid_decode_spec(
            cache_shape.n_ctx_seq,
            cache_shape.n_seq_max,
            config.attention_k_type,
            config.attention_v_type,
            config.recurrent_r_type,
            config.recurrent_s_type,
        )?;
        // Debug escape hatch: truncate the network to the first N blocks so a
        // wrong-output bug can be bisected to the block where it starts.
        if let Ok(max_blocks) = std::env::var("MAKEPAD_LLAMA_MAX_BLOCKS") {
            if let Ok(max_blocks) = max_blocks.parse::<usize>() {
                eprintln!(
                    "session: truncating {} layers to {max_blocks} (MAKEPAD_LLAMA_MAX_BLOCKS)",
                    spec.layers.len()
                );
                spec.layers.truncate(max_blocks);
            }
        }
        // Same graph with precomputed-embedding input for image spans; both
        // specs share the cache tensors, so batches can alternate freely.
        let mut spec_embeddings = spec.clone();
        spec_embeddings.input = ProbeInputKind::Embeddings {
            hidden_size: model.embedding_length()?,
            input_type: TensorType::F32,
        };

        let draft_max = if model.load_mtp() {
            config.spec_draft_max
        } else {
            0
        };
        // The verify batch checkpoints the recurrent state after every token,
        // so the r/s caches need one row per batch position plus the live row.
        let recurrent_rows = if draft_max > 0 {
            let rows = u32::try_from(draft_max + 2)
                .map_err(|_| LlamaError::format("spec_draft_max is too large"))?;
            for layer in spec.layers.iter_mut() {
                if let HybridLayerSpec::Recurrent { decode, .. } = layer {
                    decode.cache.max_sequences = rows;
                }
            }
            for layer in spec_embeddings.layers.iter_mut() {
                if let HybridLayerSpec::Recurrent { decode, .. } = layer {
                    decode.cache.max_sequences = rows;
                }
            }
            rows
        } else {
            cache_shape.n_seq_max
        };
        let mut spec_mtp = if draft_max > 0 {
            model.mtp_decode_spec(
                cache_shape.n_ctx_seq,
                cache_shape.n_seq_max,
                config.attention_k_type,
                config.attention_v_type,
            )?
        } else {
            None
        };
        if spec_mtp.is_none() {
            // No draft head: drop the hidden carry the main spec asked for.
            spec.hidden_carry = None;
            spec_embeddings.hidden_carry = None;
        }
        let carry_ring = if spec_mtp.is_some() {
            mtp_carry_ring(config.prefill_batch_size, draft_max, max_context as usize)
        } else {
            0
        };
        let spec_verify = spec_mtp.as_ref().map(|_| {
            let mut verify = spec.clone();
            verify.recurrent_checkpoints = true;
            verify
        });
        if let Some(mtp) = spec_mtp.as_mut() {
            debug_assert!(mtp.hidden_carry.is_some());
        }

        let mut cache_bytes = if let Some(template) = plan.hybrid_cache.as_ref() {
            HybridCacheLayout::new(template.materialize(cache_shape, cache_types))?.total_bytes
        } else {
            attention_cache_bytes_from_spec(&spec)?
        };
        // The template sizes every cache at `config.max_sequences`; the extra
        // recurrent checkpoint rows and the hidden carry are on top of that.
        if let (Some(template), true) = (plan.hybrid_cache.as_ref(), draft_max > 0) {
            let extra_rows = u64::from(recurrent_rows.saturating_sub(cache_shape.n_seq_max));
            let per_row = ggml_row_size_for_type(
                config.recurrent_r_type,
                i64::try_from(template.recurrent_r_width).map_err(|_| {
                    LlamaError::format("recurrent r width does not fit in i64")
                })?,
            )
            .map_err(LlamaError::format)?
                + ggml_row_size_for_type(
                    config.recurrent_s_type,
                    i64::try_from(template.recurrent_s_width).map_err(|_| {
                        LlamaError::format("recurrent s width does not fit in i64")
                    })?,
                )
                .map_err(LlamaError::format)?;
            cache_bytes = cache_bytes
                .checked_add(
                    per_row
                        .checked_mul(template.recurrent_layers.len())
                        .and_then(|v| usize::try_from(extra_rows).ok().and_then(|r| v.checked_mul(r)))
                        .ok_or_else(|| {
                            LlamaError::format("overflow sizing recurrent checkpoint rows")
                        })?,
                )
                .ok_or_else(|| LlamaError::format("overflow sizing recurrent checkpoint rows"))?;
            cache_bytes = cache_bytes
                .checked_add(carry_ring_bytes(model.embedding_length()?, carry_ring)?)
                .ok_or_else(|| LlamaError::format("overflow sizing the mtp hidden carry"))?;
        }
        let context_extra_bytes = cache_bytes
            .checked_add(config.extra_activation_bytes)
            .ok_or_else(|| LlamaError::format("overflow computing session activation bytes"))?;
        let max_context_usize = usize::try_from(max_context)
            .map_err(|_| LlamaError::format("session max_context does not fit in usize"))?;
        let (weights, graphs) = build_runtime_state(
            &model,
            &plan,
            &spec,
            spec_mtp.as_ref(),
            carry_ring,
            context_extra_bytes,
            prompt_batch_capacity(config.prefill_batch_size, max_context_usize),
            progress,
        )?;

        let mtp = spec_mtp.as_ref().map(|_| MtpRuntime {
            draft_max,
            state_row: 0,
            carry_ring,
            mtp_filled: 0,
            drafted: 0,
            accepted: 0,
            rounds: 0,
            draft_nanos: 0,
            verify_nanos: 0,
            catchup_nanos: 0,
        });

        Ok(Self {
            model,
            vocab,
            plan,
            spec,
            spec_embeddings,
            spec_verify,
            spec_mtp,
            mtp,
            config,
            max_context: max_context_usize,
            context_extra_bytes,
            weights,
            graphs,
            token_ids: Vec::new(),
            rope_pos_next: 0,
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
        // After an image span, rope positions run behind the linear sequence
        // index — text continues from the image's pos_0 + max(w, h).
        let rope_positions = if self.rope_pos_next != start as i64 {
            let base = self.rope_pos_next;
            let mut planes = vec![0i32; batch_size * 4];
            for i in 0..batch_size {
                let p = i32::try_from(base + i as i64)
                    .map_err(|_| LlamaError::format("rope position does not fit in i32"))?;
                planes[i] = p;
                planes[batch_size + i] = p;
                planes[2 * batch_size + i] = p;
                // fourth component stays 0 (unused section)
            }
            Some(planes)
        } else {
            None
        };
        // Key compiled graphs by a BUCKETED key count, not the exact cache
        // length: per-length keys meant a NEW Metal graph compiled and
        // cached for EVERY generated token (unbounded growth — the 30GB
        // footprint on long voice sessions). The graph key width only needs
        // to COVER the cache — masks are written at full graph width so the
        // unwritten tail is -inf'd exactly (keying by max_context wasted
        // key_count-cache_tokens of attention work per token; keying wider
        // than the mask CANNOT be fixed by view reconfigure — the flash op
        // reads permute nodes with build-time dims, which silently corrupted
        // attention). MAKEPAD_LLAMA_PER_LEN_GRAPHS=1 restores per-length
        // keying (A/B escape hatch).
        let attention_key_count = if std::env::var_os("MAKEPAD_LLAMA_PER_LEN_GRAPHS").is_some() {
            cache_tokens
        } else {
            cache_tokens
                .next_multiple_of(GRAPH_KEY_BUCKET)
                .min(self.max_context())
        };
        let graph_params = SessionGraphParams::greedy(batch_size, attention_key_count);
        self.ensure_compiled_graph(graph_params)?;
        let state_row = self.live_state_row();
        let hidden_write_rows = self.carry_rows_for_positions(start, batch_size);
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
            layout.rope_positions = rope_positions;
            layout.recurrent_state_rows = vec![state_row];
            layout.hidden_write_rows = hidden_write_rows;
            if compiled.decode().input_recurrent_state_rows.is_none() {
                layout.recurrent_state_rows.clear();
            }
            compiled
                .execute_logits_only_with_layout(LogitsProbeInput::TokenIds(token_ids), &layout)?
        };
        self.token_ids.extend_from_slice(token_ids);
        self.rope_pos_next += batch_size as i64;
        self.last_run = Some(collapse_last_token_run(run)?);
        self.flush_mtp_prefill(false)?;
        Ok(())
    }

    /// Recurrent-cache row the non-speculative graphs read and write.
    fn live_state_row(&self) -> i32 {
        self.mtp.map(|mtp| mtp.state_row).unwrap_or(0)
    }

    /// Hidden-carry ring rows for `count` tokens starting at sequence index
    /// `start`. Row of sequence index `i` is `i % carry_ring`, which makes the
    /// row of any position derivable without extra bookkeeping.
    fn carry_rows_for_positions(&self, start: usize, count: usize) -> Vec<i32> {
        let Some(mtp) = self.mtp.as_ref() else {
            return Vec::new();
        };
        (start..start + count)
            .map(|index| (index % mtp.carry_ring) as i32)
            .collect()
    }

    fn carry_scratch_row(&self, mtp: &MtpRuntime) -> i32 {
        mtp.carry_ring as i32
    }

    fn carry_zero_row(&self, mtp: &MtpRuntime) -> i32 {
        mtp.carry_ring as i32 + 1
    }

    /// Append an image span: precomputed vision embeddings for a grid of
    /// `tokens_w` x `tokens_h` merged tokens (row-major), as produced by
    /// `VisionTower::encode`. Occupies `tokens_w * tokens_h` sequence slots
    /// but advances the rope position by only `max(tokens_w, tokens_h)`,
    /// with Qwen-VL 2D positions `[pos0, pos0+y, pos0+x, 0]` per token.
    /// Callers surround this with the `<|vision_start|>` / `<|vision_end|>`
    /// text tokens via `append_tokens`.
    pub fn append_image_embeddings(
        &mut self,
        embeddings: &[f32],
        tokens_w: usize,
        tokens_h: usize,
    ) -> Result<()> {
        let n_tokens = tokens_w * tokens_h;
        if n_tokens == 0 {
            return Ok(());
        }
        let hidden = usize::try_from(self.model.embedding_length()?)
            .map_err(|_| LlamaError::format("embedding length does not fit in usize"))?;
        if embeddings.len() != n_tokens * hidden {
            return Err(LlamaError::format(format!(
                "image embeddings length {} does not match {}x{} tokens x {} hidden",
                embeddings.len(),
                tokens_w,
                tokens_h,
                hidden
            )));
        }
        self.ensure_capacity(n_tokens)?;
        let pad_token = self.vocab.token_id("<|image_pad|>").unwrap_or(-1);
        let pos0 = self.rope_pos_next;

        let prefill_batch_size = self.config.prefill_batch_size.max(1);
        let mut offset = 0usize;
        while offset < n_tokens {
            let batch_size = (n_tokens - offset).min(prefill_batch_size);
            let start = self.token_ids.len();
            let positions = (start..start + batch_size)
                .map(|position| {
                    i32::try_from(position)
                        .map_err(|_| LlamaError::format("token position does not fit in i32"))
                })
                .collect::<Result<Vec<_>>>()?;
            let cache_tokens = start + batch_size;

            let mut planes = vec![0i32; batch_size * 4];
            for i in 0..batch_size {
                let token_index = offset + i;
                let y = (token_index / tokens_w) as i64;
                let x = (token_index % tokens_w) as i64;
                let clamp = |v: i64| {
                    i32::try_from(v)
                        .map_err(|_| LlamaError::format("rope position does not fit in i32"))
                };
                planes[i] = clamp(pos0)?;
                planes[batch_size + i] = clamp(pos0 + y)?;
                planes[2 * batch_size + i] = clamp(pos0 + x)?;
                // fourth component stays 0 (unused section)
            }

            let graph_params = SessionGraphParams::greedy_embeddings(batch_size, cache_tokens);
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
                layout.rope_positions = Some(planes);
                if compiled.decode().input_recurrent_state_rows.is_none() {
                    layout.recurrent_state_rows.clear();
                }
                compiled.execute_logits_only_with_layout(
                    LogitsProbeInput::EmbeddingsF32 {
                        data: &embeddings[offset * hidden..(offset + batch_size) * hidden],
                        n_tokens: batch_size,
                    },
                    &layout,
                )?
            };
            self.token_ids
                .extend(std::iter::repeat(pad_token).take(batch_size));
            self.last_run = Some(collapse_last_token_run(run)?);
            offset += batch_size;
        }
        self.rope_pos_next = pos0 + tokens_w.max(tokens_h) as i64;
        Ok(())
    }

    fn ensure_compiled_graph(&mut self, params: SessionGraphParams) -> Result<()> {
        if self.graphs.has_graph(params) {
            return Ok(());
        }
        for attempt in 0..=MAX_GRAPH_RESERVE_RETRIES {
            // Cached graphs don't reserve buffer space of their own (plans
            // share the main buffer), so keep them: prefill steps revisit the
            // same (n_tokens, key_count) params across prompts, and evicting
            // here forced a recompile of every step on every prompt. Eviction
            // happens only when a compile actually fails to reserve, below.
            if attempt > 0 {
                self.graphs.evict_graphs_except(params);
            }
            let spec = match params.kind {
                SessionGraphKind::Main => &self.spec,
                SessionGraphKind::MainEmbeddings => &self.spec_embeddings,
                SessionGraphKind::MainVerify => self.spec_verify.as_ref().ok_or_else(|| {
                    LlamaError::format("speculative verify graph requested without an MTP spec")
                })?,
                SessionGraphKind::MtpDraft => self.spec_mtp.as_ref().ok_or_else(|| {
                    LlamaError::format("MTP draft graph requested without an MTP spec")
                })?,
            };
            match self.graphs.shared_runtime.compile_hybrid_decode(
                &mut self.weights,
                spec,
                &self.graphs.shared_cache,
                &self.graphs.shared_buffers,
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
                        self.spec_mtp.as_ref(),
                        self.mtp.map(|mtp| mtp.carry_ring).unwrap_or(0),
                        self.context_extra_bytes,
                        prompt_batch_capacity(self.config.prefill_batch_size, self.max_context),
                        &mut |_, _| {},
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

/// Probability of accepting a draft token: `min(1, p/q)`. A draft token the
/// target assigns zero mass to is always rejected.
fn speculative_acceptance(target: f32, draft: f32) -> f32 {
    if draft > 0.0 {
        (target / draft).min(1.0)
    } else {
        0.0
    }
}

/// The normalised residual `max(0, p - q)` a rejected draft is replaced from.
/// Together with `speculative_acceptance` this makes the emitted token exactly
/// `p`-distributed (Leviathan et al. / Chen et al. speculative sampling).
/// Degenerate case: when `q == p` everywhere the residual is empty, and the
/// only way to get there is `p(x)/q(x) == 1` for the drafted token, i.e. the
/// draft was already accepted — so falling back to `p` is unreachable in
/// practice and merely keeps the function total.
fn speculative_residual(target: &[f32], draft: &[f32]) -> Vec<f32> {
    let mut residual: Vec<f32> = target
        .iter()
        .zip(draft.iter())
        .map(|(p, q)| (p - q).max(0.0))
        .collect();
    let total: f32 = residual.iter().sum();
    if total > 0.0 {
        for value in residual.iter_mut() {
            *value /= total;
        }
        residual
    } else {
        target.to_vec()
    }
}

/// A degenerate logit vector whose argmax and softmax both concentrate on
/// `token`; used to hand an already-sampled token to the next round without a
/// second forward.
fn one_hot_logits(token: i32, vocab_size: usize) -> Result<Vec<f32>> {
    let index = usize::try_from(token)
        .ok()
        .filter(|index| *index < vocab_size)
        .ok_or_else(|| LlamaError::format(format!("token {token} is outside the vocabulary")))?;
    let mut logits = vec![f32::NEG_INFINITY; vocab_size];
    logits[index] = 0.0;
    Ok(logits)
}

/// Sampling knobs. Defaults match the fleet's chat settings.
#[derive(Clone, Copy, Debug)]
pub struct LlamaSamplingParams {
    pub temperature: f32,
    pub top_p: f32,
    /// 0 disables top-k.
    pub top_k: usize,
    pub seed: u64,
}

impl Default for LlamaSamplingParams {
    fn default() -> Self {
        Self {
            temperature: 0.7,
            top_p: 0.9,
            top_k: 0,
            seed: 7,
        }
    }
}

impl LlamaSamplingParams {
    fn is_greedy(&self) -> bool {
        self.temperature <= 0.0
    }
}

/// Deterministic per-session RNG so a seed reproduces a run exactly.
#[derive(Clone, Copy, Debug)]
struct Xorshift64(u64);

impl Xorshift64 {
    fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15).max(1))
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn next_f32(&mut self) -> f32 {
        // 24 mantissa bits -> [0, 1)
        (self.next_u64() >> 40) as f32 / (1u32 << 24) as f32
    }
}

/// Turn logits into the probability vector the sampler actually draws from:
/// temperature, then top-k, then top-p, then renormalise. Entries outside the
/// kept set are exactly zero, which is what makes the speculative residual
/// `max(0, p - q)` well defined over the whole vocabulary.
fn sampling_probabilities(logits: &[f32], params: LlamaSamplingParams) -> Result<Vec<f32>> {
    if logits.is_empty() {
        return Err(LlamaError::format("cannot sample from empty logits"));
    }
    let temperature = params.temperature.max(1e-6);
    let max_logit = logits.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    if !max_logit.is_finite() {
        return Err(LlamaError::format("logits contain no finite value"));
    }
    let mut probs: Vec<f32> = logits
        .iter()
        .map(|logit| ((logit - max_logit) / temperature).exp())
        .collect();
    let sum: f32 = probs.iter().sum();
    if !(sum > 0.0) {
        return Err(LlamaError::format("logit softmax underflowed to zero"));
    }
    for value in probs.iter_mut() {
        *value /= sum;
    }

    // Rank once, then apply top-k and top-p on the same ordering.
    let mut order: Vec<u32> = (0..probs.len() as u32).collect();
    let keep = if params.top_k > 0 {
        params.top_k.min(probs.len())
    } else {
        probs.len()
    };
    if keep < probs.len() {
        order.select_nth_unstable_by(keep - 1, |a, b| {
            probs[*b as usize].total_cmp(&probs[*a as usize])
        });
        order.truncate(keep);
    }
    order.sort_unstable_by(|a, b| probs[*b as usize].total_cmp(&probs[*a as usize]));

    let top_p = params.top_p.clamp(0.0, 1.0);
    let mut cumulative = 0.0f32;
    let mut cut = order.len();
    if top_p < 1.0 {
        for (index, token) in order.iter().enumerate() {
            cumulative += probs[*token as usize];
            if cumulative >= top_p {
                cut = index + 1;
                break;
            }
        }
    }
    let kept: BTreeSet<u32> = order[..cut.min(order.len())].iter().copied().collect();
    let mut total = 0.0f32;
    for (token, value) in probs.iter_mut().enumerate() {
        if kept.contains(&(token as u32)) {
            total += *value;
        } else {
            *value = 0.0;
        }
    }
    if !(total > 0.0) {
        return Err(LlamaError::format("sampling kept no probability mass"));
    }
    for value in probs.iter_mut() {
        *value /= total;
    }
    Ok(probs)
}

/// Inverse-CDF draw from a normalised probability vector.
fn sample_from(probs: &[f32], rng: &mut Xorshift64) -> Result<i32> {
    let target = rng.next_f32();
    let mut cumulative = 0.0f32;
    let mut last = None;
    for (token, &value) in probs.iter().enumerate() {
        if value <= 0.0 {
            continue;
        }
        last = Some(token);
        cumulative += value;
        if cumulative >= target {
            return i32::try_from(token)
                .map_err(|_| LlamaError::format("sampled token does not fit in i32"));
        }
    }
    // Rounding can leave the cumulative just short of 1.0.
    let token = last.ok_or_else(|| LlamaError::format("probability vector is all zeros"))?;
    i32::try_from(token).map_err(|_| LlamaError::format("sampled token does not fit in i32"))
}

/// MTP speculative decoding: draft with the `blk.N` nextn head, verify the
/// whole draft in one batched forward, keep the longest matching prefix.
///
/// The hard part of a hybrid model is undoing a rejected draft: the 48
/// GatedDeltaNet layers carry a recurrent state that the verify batch has
/// already advanced past the rejection point. The verify graph therefore runs
/// its recurrent scan one token at a time and parks the state after every
/// token in its own cache row, so "roll back to the accepted position" is a
/// change of the row the next step resumes from — no second forward, no
/// state copy. Attention needs nothing: its writes are index-addressed, so
/// rejected rows are simply overwritten and masked out meanwhile.
impl LlamaSession {
    /// Draft/accept counters, or `None` when speculative decoding is off.
    pub fn speculative_stats(&self) -> Option<SpeculativeStats> {
        self.mtp.map(|mtp| SpeculativeStats {
            drafted: mtp.drafted,
            accepted: mtp.accepted,
            rounds: mtp.rounds,
            draft_nanos: mtp.draft_nanos,
            verify_nanos: mtp.verify_nanos,
            catchup_nanos: mtp.catchup_nanos,
        })
    }

    /// True when this session runs speculative decoding.
    pub fn speculative_enabled(&self) -> bool {
        self.mtp.is_some()
    }

    /// Sample `max_new_tokens` tokens. With speculation on this uses proper
    /// speculative rejection sampling (accept draft `x` with probability
    /// `min(1, p(x)/q(x))`, otherwise draw from the normalised residual
    /// `max(0, p - q)`), so the output distribution is exactly the one the
    /// non-speculative sampler would produce.
    pub fn continue_sampled(
        &mut self,
        max_new_tokens: usize,
        params: LlamaSamplingParams,
    ) -> Result<LlamaGeneration> {
        if params.is_greedy() {
            return self.continue_greedy(max_new_tokens);
        }
        let mut rng = Xorshift64::new(params.seed);
        if self.mtp.is_some() {
            return self.continue_sampled_speculative(max_new_tokens, params, &mut rng);
        }

        let mut token_ids = Vec::with_capacity(max_new_tokens);
        let mut stop_reason = LlamaStopReason::MaxNewTokens;
        for _ in 0..max_new_tokens {
            let logits = self.last_logits().ok_or_else(|| {
                LlamaError::format("session has no logits yet; append context tokens first")
            })?;
            let probs = sampling_probabilities(logits, params)?;
            let next_token = sample_from(&probs, &mut rng)?;
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

    fn continue_sampled_speculative(
        &mut self,
        max_new_tokens: usize,
        params: LlamaSamplingParams,
        rng: &mut Xorshift64,
    ) -> Result<LlamaGeneration> {
        let mut token_ids = Vec::with_capacity(max_new_tokens);
        let mut stop_reason = LlamaStopReason::MaxNewTokens;
        while token_ids.len() < max_new_tokens {
            let remaining = max_new_tokens - token_ids.len();
            if let Some(reason) =
                self.speculative_round_sampled(&mut token_ids, remaining, params, rng)?
            {
                stop_reason = reason;
                break;
            }
        }
        Ok(LlamaGeneration {
            text: self.vocab.decode_tokens(&token_ids)?,
            token_ids,
            stop_reason,
        })
    }

    /// One speculative round under sampling. Same state machinery as the
    /// greedy round; only the accept test differs.
    fn speculative_round_sampled(
        &mut self,
        out: &mut Vec<i32>,
        remaining: usize,
        params: LlamaSamplingParams,
        rng: &mut Xorshift64,
    ) -> Result<Option<LlamaStopReason>> {
        let mtp = self
            .mtp
            .ok_or_else(|| LlamaError::format("speculative round without an MTP head"))?;
        self.flush_mtp_prefill(true)?;

        let first = {
            let logits = self.last_logits().ok_or_else(|| {
                LlamaError::format("session has no logits yet; append context tokens first")
            })?;
            let probs = sampling_probabilities(logits, params)?;
            sample_from(&probs, rng)?
        };
        if let Some(reason) = self.stop_reason_for(first) {
            return Ok(Some(reason));
        }

        let start = self.token_ids.len();
        let draft_max = mtp
            .draft_max
            .min(remaining.saturating_sub(1))
            .min(self.max_context.saturating_sub(start + 1));

        // Draft under the SAME sampling transform as the target, so the
        // acceptance ratio `p/q` is the standard one and the residual is a
        // valid distribution.
        let mut drafts = Vec::with_capacity(draft_max);
        let mut draft_probs: Vec<Vec<f32>> = Vec::with_capacity(draft_max);
        let mut token = first;
        for step in 0..draft_max {
            let position = start + step;
            let read_row = if step == 0 {
                if start == 0 {
                    self.carry_zero_row(&mtp)
                } else {
                    ((start - 1) % mtp.carry_ring) as i32
                }
            } else {
                self.carry_scratch_row(&mtp)
            };
            let started = std::time::Instant::now();
            let logits = self.run_mtp_draft_logits(token, position, read_row)?;
            if let Some(mtp) = self.mtp.as_mut() {
                mtp.draft_nanos += started.elapsed().as_nanos() as u64;
            }
            let probs = sampling_probabilities(&logits, params)?;
            token = sample_from(&probs, rng)?;
            drafts.push(token);
            draft_probs.push(probs);
        }

        let mut batch = Vec::with_capacity(drafts.len() + 1);
        batch.push(first);
        batch.extend_from_slice(&drafts);
        let started = std::time::Instant::now();
        let run = self.run_verify_batch(&batch)?;
        if let Some(mtp) = self.mtp.as_mut() {
            mtp.verify_nanos += started.elapsed().as_nanos() as u64;
        }

        let vocab_size = run.vocab_size;
        if vocab_size == 0 || run.logits.len() != vocab_size * batch.len() {
            return Err(LlamaError::format(format!(
                "speculative verify produced {} logits for {} rows of vocab {}",
                run.logits.len(),
                batch.len(),
                vocab_size
            )));
        }

        let mut accepted = 0usize;
        let mut bonus = None;
        for index in 0..drafts.len() {
            let target = sampling_probabilities(
                &run.logits[index * vocab_size..(index + 1) * vocab_size],
                params,
            )?;
            let drafted = drafts[index] as usize;
            let q = draft_probs[index][drafted];
            let p = target[drafted];
            let ratio = speculative_acceptance(p, q);
            if rng.next_f32() < ratio {
                accepted += 1;
                continue;
            }
            // Rejected: draw the replacement from the normalised residual so
            // the overall distribution stays exactly `p`.
            let residual = speculative_residual(&target, &draft_probs[index]);
            bonus = Some(sample_from(&residual, rng)?);
            break;
        }
        let bonus = match bonus {
            Some(token) => token,
            None => {
                let target = sampling_probabilities(
                    &run.logits[accepted * vocab_size..(accepted + 1) * vocab_size],
                    params,
                )?;
                sample_from(&target, rng)?
            }
        };

        let mut commit = accepted;
        let mut stop_reason = None;
        for (index, &drafted) in drafts[..accepted].iter().enumerate() {
            if let Some(reason) = self.stop_reason_for(drafted) {
                commit = index;
                stop_reason = Some(reason);
                break;
            }
        }

        let committed = &batch[..=commit];
        self.token_ids.extend_from_slice(committed);
        self.rope_pos_next += committed.len() as i64;
        out.extend_from_slice(committed);
        // The next round samples `first` from these logits; when the whole
        // draft was consumed the bonus token is already drawn, so feed it as a
        // one-hot to keep the two paths on one code path.
        self.last_run = Some(HybridDecodeRun {
            hidden: Vec::new(),
            logits: one_hot_logits(bonus, vocab_size)?,
            n_tokens: 1,
            hidden_size: run.hidden_size,
            vocab_size,
            selected_experts: Vec::new(),
        });

        if let Some(mtp) = self.mtp.as_mut() {
            mtp.state_row = i32::try_from(commit)
                .map_err(|_| LlamaError::format("speculative checkpoint row does not fit in i32"))?;
            mtp.drafted += drafts.len() as u64;
            mtp.accepted += accepted as u64;
            mtp.rounds += 1;
            mtp.mtp_filled = if reuse_draft_kv() {
                start + committed.len()
            } else {
                (start + 1).min(start + committed.len())
            };
        }

        if stop_reason.is_none() {
            if let Some(reason) = self.stop_reason_for(bonus) {
                return Ok(Some(reason));
            }
        }
        Ok(stop_reason)
    }

    fn continue_greedy_speculative(&mut self, max_new_tokens: usize) -> Result<LlamaGeneration> {
        let mut token_ids = Vec::with_capacity(max_new_tokens);
        let mut stop_reason = LlamaStopReason::MaxNewTokens;

        while token_ids.len() < max_new_tokens {
            let remaining = max_new_tokens - token_ids.len();
            if let Some(reason) = self.speculative_round(&mut token_ids, remaining)? {
                stop_reason = reason;
                break;
            }
        }

        Ok(LlamaGeneration {
            text: self.vocab.decode_tokens(&token_ids)?,
            token_ids,
            stop_reason,
        })
    }

    /// One speculative round. Appends the committed tokens to `out` and
    /// returns a stop reason when generation should end.
    fn speculative_round(
        &mut self,
        out: &mut Vec<i32>,
        remaining: usize,
    ) -> Result<Option<LlamaStopReason>> {
        let mtp = self
            .mtp
            .ok_or_else(|| LlamaError::format("speculative round without an MTP head"))?;
        self.flush_mtp_prefill(true)?;

        let first = self.greedy_candidate()?;
        if let Some(reason) = self.stop_reason_for(first) {
            return Ok(Some(reason));
        }

        let start = self.token_ids.len();
        // Never draft past the context wall or past what the caller still
        // wants; `+1` accounts for the bonus token the verify always yields.
        let draft_max = mtp
            .draft_max
            .min(remaining.saturating_sub(1))
            .min(self.max_context.saturating_sub(start + 1));

        let mut drafts = Vec::with_capacity(draft_max);
        let mut token = first;
        for step in 0..draft_max {
            let position = start + step;
            let read_row = if step == 0 {
                if start == 0 {
                    self.carry_zero_row(&mtp)
                } else {
                    ((start - 1) % mtp.carry_ring) as i32
                }
            } else {
                self.carry_scratch_row(&mtp)
            };
            let started = std::time::Instant::now();
            token = self.run_mtp_draft_step(token, position, read_row)?;
            if let Some(mtp) = self.mtp.as_mut() {
                mtp.draft_nanos += started.elapsed().as_nanos() as u64;
            }
            drafts.push(token);
        }

        let mut batch = Vec::with_capacity(drafts.len() + 1);
        batch.push(first);
        batch.extend_from_slice(&drafts);
        let started = std::time::Instant::now();
        let run = self.run_verify_batch(&batch)?;
        if let Some(mtp) = self.mtp.as_mut() {
            mtp.verify_nanos += started.elapsed().as_nanos() as u64;
        }

        let vocab_size = run.vocab_size;
        if vocab_size == 0 || run.logits.len() != vocab_size * batch.len() {
            return Err(LlamaError::format(format!(
                "speculative verify produced {} logits for {} rows of vocab {}",
                run.logits.len(),
                batch.len(),
                vocab_size
            )));
        }
        let row = |index: usize| &run.logits[index * vocab_size..(index + 1) * vocab_size];

        let mut accepted = 0usize;
        let mut next_token = argmax_token_id(row(0))?;
        while accepted < drafts.len() && drafts[accepted] == next_token {
            accepted += 1;
            next_token = argmax_token_id(row(accepted))?;
        }

        // A stop token inside the accepted run truncates the commit; the
        // caches are left advanced but nothing reads past `token_ids`.
        let mut commit = accepted;
        let mut stop_reason = None;
        for (index, &drafted) in drafts[..accepted].iter().enumerate() {
            if let Some(reason) = self.stop_reason_for(drafted) {
                commit = index;
                stop_reason = Some(reason);
                break;
            }
        }

        let committed = &batch[..=commit];
        self.token_ids.extend_from_slice(committed);
        self.rope_pos_next += committed.len() as i64;
        out.extend_from_slice(committed);
        self.last_run = Some(HybridDecodeRun {
            hidden: Vec::new(),
            logits: row(commit).to_vec(),
            n_tokens: 1,
            hidden_size: run.hidden_size,
            vocab_size,
            selected_experts: Vec::new(),
        });

        if let Some(mtp) = self.mtp.as_mut() {
            mtp.state_row = i32::try_from(commit)
                .map_err(|_| LlamaError::format("speculative checkpoint row does not fit in i32"))?;
            mtp.drafted += drafts.len() as u64;
            mtp.accepted += accepted as u64;
            mtp.rounds += 1;
            // The draft head already wrote KV for every drafted position, but
            // only position `start` used the main model's hidden state; the
            // rest chained off the draft head's own output. Re-running the
            // accepted tail through the prefill hook restores llama.cpp's
            // exact conditioning. MKLLM_MTP_REUSE_DRAFT_KV=1 keeps the
            // approximate rows and skips that catch-up decode.
            mtp.mtp_filled = if reuse_draft_kv() {
                start + committed.len()
            } else {
                (start + 1).min(start + committed.len())
            };
        }

        Ok(stop_reason)
    }

    /// One autoregressive draft step through the nextn block, greedy.
    fn run_mtp_draft_step(&mut self, token: i32, position: usize, read_row: i32) -> Result<i32> {
        let logits = self.run_mtp_draft_logits(token, position, read_row)?;
        argmax_token_id(&logits)
    }

    /// One autoregressive draft step, returning the draft head's logits.
    fn run_mtp_draft_logits(
        &mut self,
        token: i32,
        position: usize,
        read_row: i32,
    ) -> Result<Vec<f32>> {
        let mtp = self
            .mtp
            .ok_or_else(|| LlamaError::format("draft step without an MTP head"))?;
        let cache_tokens = position + 1;
        let params = SessionGraphParams::mtp_draft(1, self.graph_key_count(cache_tokens));
        self.ensure_compiled_graph(params)?;
        let scratch_row = self.carry_scratch_row(&mtp);
        let compiled = self
            .graphs
            .graph_for_mut(params)
            .ok_or_else(|| LlamaError::format("compiled mtp graph params were not cached"))?;
        let positions = [i32::try_from(position)
            .map_err(|_| LlamaError::format("draft position does not fit in i32"))?];
        let mut layout =
            HybridDecodeBatchLayout::from_contiguous_positions_and_outputs(
                &positions,
                cache_tokens,
                &[0],
            )?;
        layout.recurrent_state_rows.clear();
        layout.hidden_read_rows = vec![read_row];
        layout.hidden_write_rows = vec![scratch_row];
        let run =
            compiled.execute_logits_only_with_layout(LogitsProbeInput::TokenIds(&[token]), &layout)?;
        Ok(run.logits)
    }


    /// The verify forward: `tokens.len()` positions, logits for every one.
    fn run_verify_batch(&mut self, tokens: &[i32]) -> Result<HybridDecodeRun> {
        let mtp = self
            .mtp
            .ok_or_else(|| LlamaError::format("verify batch without an MTP head"))?;
        let start = self.token_ids.len();
        let batch_size = tokens.len();
        self.ensure_capacity(batch_size)?;
        let cache_tokens = start + batch_size;
        let params =
            SessionGraphParams::verify(batch_size, self.graph_key_count(cache_tokens));
        self.ensure_compiled_graph(params)?;

        let positions = (start..start + batch_size)
            .map(|position| {
                i32::try_from(position)
                    .map_err(|_| LlamaError::format("verify position does not fit in i32"))
            })
            .collect::<Result<Vec<_>>>()?;
        let output_ids = (0..batch_size)
            .map(|index| {
                i32::try_from(index)
                    .map_err(|_| LlamaError::format("verify output id does not fit in i32"))
            })
            .collect::<Result<Vec<_>>>()?;
        // `[resume_row, w_0 .. w_n]`: the scan resumes from the committed row
        // and parks a checkpoint after every token.
        let mut state_rows = Vec::with_capacity(batch_size + 1);
        state_rows.push(mtp.state_row);
        for index in 0..batch_size {
            state_rows.push(index as i32);
        }
        let hidden_write_rows = self.carry_rows_for_positions(start, batch_size);

        let compiled = self
            .graphs
            .graph_for_mut(params)
            .ok_or_else(|| LlamaError::format("compiled verify graph params were not cached"))?;
        let mut layout = HybridDecodeBatchLayout::from_contiguous_positions_and_outputs(
            &positions,
            cache_tokens,
            &output_ids,
        )?;
        layout.recurrent_state_rows = state_rows;
        layout.hidden_write_rows = hidden_write_rows;
        compiled.execute_logits_only_with_layout(LogitsProbeInput::TokenIds(tokens), &layout)
    }

    /// Run the draft head over the tokens the main model has already consumed
    /// but the draft head has not, so its KV cache carries the same context.
    /// llama.cpp does this after every ubatch; batching it is much cheaper
    /// because the draft graph's LM head reads ~1 GB of weights per call
    /// whatever the batch size.
    fn flush_mtp_prefill(&mut self, force: bool) -> Result<()> {
        let Some(mtp) = self.mtp else {
            return Ok(());
        };
        let pending = self.token_ids.len().saturating_sub(mtp.mtp_filled);
        if pending == 0 {
            return Ok(());
        }
        // Carry rows are a ring: letting the main model run a full ring ahead
        // would overwrite hidden rows the draft head has not read yet.
        if !force && pending + 1 < mtp.carry_ring {
            return Ok(());
        }
        let started = std::time::Instant::now();
        while self
            .mtp
            .map(|mtp| self.token_ids.len() > mtp.mtp_filled)
            .unwrap_or(false)
        {
            self.run_mtp_prefill_chunk()?;
        }
        if let Some(mtp) = self.mtp.as_mut() {
            mtp.catchup_nanos += started.elapsed().as_nanos() as u64;
        }
        Ok(())
    }

    fn run_mtp_prefill_chunk(&mut self) -> Result<()> {
        let mtp = self
            .mtp
            .ok_or_else(|| LlamaError::format("mtp prefill without an MTP head"))?;
        let start = mtp.mtp_filled;
        let chunk = MTP_PREFILL_CHUNK.min(mtp.carry_ring.saturating_sub(1)).max(1);
        let end = (start + chunk).min(self.token_ids.len());
        let batch_size = end - start;
        let tokens = self.token_ids[start..end].to_vec();
        let positions = (start..end)
            .map(|position| {
                i32::try_from(position)
                    .map_err(|_| LlamaError::format("mtp prefill position does not fit in i32"))
            })
            .collect::<Result<Vec<_>>>()?;
        // Row `i` consumes the main model's hidden at `i - 1`; the first token
        // of the sequence has none, so it reads the never-written zero row
        // (llama.cpp starts from a zeroed `pending_h`).
        let zero_row = self.carry_zero_row(&mtp);
        let read_rows = (start..end)
            .map(|index| {
                if index == 0 {
                    zero_row
                } else {
                    ((index - 1) % mtp.carry_ring) as i32
                }
            })
            .collect::<Vec<_>>();
        let scratch_row = self.carry_scratch_row(&mtp);
        let params = SessionGraphParams::mtp_draft(batch_size, self.graph_key_count(end));
        self.ensure_compiled_graph(params)?;
        {
            let compiled = self
                .graphs
                .graph_for_mut(params)
                .ok_or_else(|| LlamaError::format("compiled mtp graph params were not cached"))?;
            let mut layout = HybridDecodeBatchLayout::from_contiguous_positions_and_outputs(
                &positions,
                end,
                &[(batch_size - 1) as i32],
            )?;
            layout.recurrent_state_rows.clear();
            layout.hidden_read_rows = read_rows;
            layout.hidden_write_rows = vec![scratch_row; batch_size];
            compiled
                .execute_logits_only_with_layout(LogitsProbeInput::TokenIds(&tokens), &layout)?;
        }
        if let Some(mtp) = self.mtp.as_mut() {
            mtp.mtp_filled = end;
        }
        Ok(())
    }

    fn graph_key_count(&self, cache_tokens: usize) -> usize {
        if std::env::var_os("MAKEPAD_LLAMA_PER_LEN_GRAPHS").is_some() {
            cache_tokens
        } else {
            cache_tokens
                .next_multiple_of(GRAPH_KEY_BUCKET)
                .min(self.max_context())
        }
    }
}

/// Tokens per MTP catch-up call. Large enough that a long prompt costs only a
/// handful of the draft head's ~1 GB LM-head reads.
const MTP_PREFILL_CHUNK: usize = 512;

fn reuse_draft_kv() -> bool {
    std::env::var_os("MKLLM_MTP_REUSE_DRAFT_KV").is_some()
}

fn resolve_max_context(model: &LlamaModel, config: LlamaSessionConfig) -> Result<u32> {
    let max_context = config.max_context.unwrap_or(model.context_length()?);
    if max_context == 0 {
        return Err(LlamaError::format("session max_context must be at least 1"));
    }
    // Official llama.cpp pads n_ctx to a multiple of 256 under flash attention
    // (GGML_PAD in llama-context.cpp), so n_kv is always ncpsg-aligned and
    // flash-ext never takes its kvpad path — that unaligned path page-faults
    // on M4 at n_q >= 20. The graph-width clamps below (.min(max_context))
    // are the only source of unaligned widths, so padding the session
    // capacity here protects every caller, not just run_bench's pre-pad.
    max_context
        .checked_next_multiple_of(GRAPH_KEY_BUCKET as u32)
        .ok_or_else(|| LlamaError::format("session max_context overflows when padded"))
}

#[allow(clippy::too_many_arguments)]
fn build_runtime_state(
    model: &LlamaModel,
    plan: &ModelExecutionPlan,
    spec: &HybridDecodeSpec,
    spec_mtp: Option<&HybridDecodeSpec>,
    carry_ring: usize,
    context_extra_bytes: usize,
    prompt_batch_capacity: usize,
    progress: &mut dyn FnMut(&str, f64),
) -> Result<(LoadedGgufWeights, SessionGraphSet)> {
    // Weights come from a read-only mmap of the gguf (clean file-backed
    // pages, lazy page-in) unless mapping is unavailable or disabled;
    // the fallback is EXACTLY the previous owned-arena path.
    // MAKEPAD_LLAMA_NO_MMAP=1 forces the owned-arena path (A/B).
    let use_mmap = std::env::var_os("MAKEPAD_LLAMA_NO_MMAP").is_none();
    let mut extra_bytes = context_extra_bytes;
    for attempt in 0..=MAX_GRAPH_RESERVE_RETRIES {
        // Retries re-run this: remapping the same file is cheap, and only
        // a real mapping failure may take the owned-arena fallback.
        progress("load llm mmap", 0.13);
        let mapped_weights = if use_mmap {
            plan.full_weights.map_and_load(&model.gguf, extra_bytes)
        } else {
            None
        };
        let mut weights = match mapped_weights {
            Some(weights) => weights,
            None => plan
                .full_weights
                .allocate_and_load_with_extra(&model.gguf, extra_bytes)?,
        };
        let shared_runtime = ExecRuntime::new()?;
        let mut shared_cache =
            allocate_hybrid_shared_cache_tensors(&mut weights.ctx, &weights.tensor_ids, spec)?;
        // The draft head is one more attention layer with its own KV cache,
        // plus the hidden-carry tensor both graphs address by name.
        let mut hidden_carry = None;
        if let Some(mtp) = spec_mtp {
            let mtp_cache =
                allocate_hybrid_shared_cache_tensors(&mut weights.ctx, &weights.tensor_ids, mtp)?;
            shared_cache.attention.extend(mtp_cache.attention);
            shared_cache.recurrent.extend(mtp_cache.recurrent);
            let carry_spec = mtp.hidden_carry.as_ref().ok_or_else(|| {
                LlamaError::format("mtp decode spec is missing its hidden carry")
            })?;
            let carry = weights
                .ctx
                .new_named_tensor(
                    carry_spec.tensor_name.clone(),
                    TensorType::F32,
                    2,
                    &[
                        i64::from(carry_spec.hidden_size),
                        i64::try_from(carry_ring + 2).map_err(|_| {
                            LlamaError::format("mtp carry ring does not fit in i64")
                        })?,
                    ],
                    crate::BufferUsage::State,
                )
                .map_err(LlamaError::format)?;
            weights
                .tensor_ids
                .insert(carry_spec.tensor_name.clone(), carry);
            hidden_carry = Some(carry);
        }
        let prompt_batch_capacity = prompt_batch_capacity.max(1);
        let mut required_main_buffer_size = shared_runtime
            .reserve_hybrid_decode_main_buffer_size(&weights, spec, Some(&shared_cache), 1, 1)?;
        if prompt_batch_capacity > 1 {
            required_main_buffer_size = required_main_buffer_size.max(
                shared_runtime.reserve_hybrid_decode_main_buffer_size(
                    &weights,
                    spec,
                    Some(&shared_cache),
                    prompt_batch_capacity,
                    1,
                )?,
            );
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
        progress("load llm upload", 0.18);
        let weight_bytes = weights.ctx.ro_split().max(weights.ctx.used_mem());
        let gb = weight_bytes as f64 / 1e9;
        let shared_buffers = shared_runtime.create_context_buffers_with_progress(
            &weights.ctx,
            &mut |done, total| {
                let denom = if total == 0 { weight_bytes.max(1) } else { total };
                let frac = 0.18 + 0.62 * (done as f64 / denom as f64).clamp(0.0, 1.0);
                progress(
                    &format!("load llm gguf ({:.1}/{:.1}GB)", done as f64 / 1e9, gb.max(0.1)),
                    frac,
                );
            },
        )?;
        progress("load llm compile", 0.85);
        let mut compiled_by_params = BTreeMap::new();
        let build_result = (|| {
            let token_generation = shared_runtime.compile_hybrid_decode(
                &mut weights,
                spec,
                &shared_cache,
                &shared_buffers,
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
                        shared_buffers,
                        compiled_by_params,
                        hidden_carry,
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
            let max_context = usize::try_from(decode.cache.max_context).map_err(|_| {
                LlamaError::format(format!(
                    "attention max_context {} does not fit in usize",
                    decode.cache.max_context
                ))
            })?;
            // Precompile the first 256-wide decode graph (official min pad),
            // not the 8192 allocation. Wider buckets compile on demand.
            return Ok(GRAPH_KEY_BUCKET.min(max_context).max(1));
        }
    }
    Ok(1)
}

/// Greedy pick: first index of the maximum, NaN-safe and tie-broken to the
/// lower index (llama.cpp's ordering).
///
/// This runs once per generated token over a 248320-entry vocabulary and is
/// several ms with an `Iterator::max_by(total_cmp)` — comparable to a whole
/// decode step on a 5090, and the speculative path pays it once per verify
/// row. A plain loop over `f32` with a NaN guard is ~20x faster and picks the
/// same token.
fn argmax_token_id(logits: &[f32]) -> Result<i32> {
    if logits.is_empty() {
        return Err(LlamaError::format("logit vector was empty"));
    }
    let mut best_index = 0usize;
    let mut best = f32::NEG_INFINITY;
    let mut saw_finite = false;
    for (index, &value) in logits.iter().enumerate() {
        // `>` skips NaN, so the first finite maximum wins and ties keep the
        // lower index, exactly like `max_by(total_cmp)` did.
        if value > best {
            best = value;
            best_index = index;
            saw_finite = true;
        }
    }
    if !saw_finite {
        // All-NaN (or all -inf): fall back to the total order so behaviour
        // matches the old comparator instead of silently returning 0.
        best_index = logits
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1).then_with(|| b.0.cmp(&a.0)))
            .map(|(index, _)| index)
            .unwrap_or(0);
    }
    i32::try_from(best_index).map_err(|_| LlamaError::format("argmax index does not fit in i32"))
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

#[cfg(test)]
mod tests {
    use super::{
        mtp_carry_ring, sample_from, sampling_probabilities, speculative_acceptance,
        speculative_residual, LlamaSamplingParams, Xorshift64,
    };

    fn params(temperature: f32, top_p: f32, top_k: usize) -> LlamaSamplingParams {
        LlamaSamplingParams {
            temperature,
            top_p,
            top_k,
            seed: 7,
        }
    }

    #[test]
    fn sampling_probabilities_normalise_and_respect_top_k() {
        let logits = [1.0f32, 2.0, 3.0, 4.0];
        let probs = sampling_probabilities(&logits, params(1.0, 1.0, 2)).unwrap();
        assert_eq!(probs.len(), 4);
        assert!(probs[0] == 0.0 && probs[1] == 0.0, "{probs:?}");
        assert!((probs.iter().sum::<f32>() - 1.0).abs() < 1e-5, "{probs:?}");
        // Ratio between the kept entries survives the renormalisation.
        let ratio = probs[3] / probs[2];
        assert!((ratio - 1.0f32.exp()).abs() < 1e-4, "{ratio}");
    }

    #[test]
    fn top_p_keeps_the_smallest_prefix_reaching_the_mass() {
        let logits = [0.0f32, 10.0, 0.0, 0.0];
        let probs = sampling_probabilities(&logits, params(1.0, 0.9, 0)).unwrap();
        assert_eq!(probs[1], 1.0);
        assert!(probs.iter().enumerate().all(|(i, p)| i == 1 || *p == 0.0));
    }

    #[test]
    fn low_temperature_collapses_onto_the_argmax() {
        let logits = [0.0f32, 1.0, 0.5];
        let probs = sampling_probabilities(&logits, params(0.01, 1.0, 0)).unwrap();
        assert!(probs[1] > 0.999, "{probs:?}");
    }

    #[test]
    fn sample_from_follows_the_distribution() {
        let probs = [0.25f32, 0.0, 0.75];
        let mut rng = Xorshift64::new(11);
        let mut counts = [0u32; 3];
        for _ in 0..20_000 {
            counts[sample_from(&probs, &mut rng).unwrap() as usize] += 1;
        }
        assert_eq!(counts[1], 0);
        let share = counts[2] as f32 / 20_000.0;
        assert!((share - 0.75).abs() < 0.02, "{counts:?}");
    }

    /// The whole point of rejection sampling: the emitted token distribution
    /// must be the TARGET distribution, whatever the draft proposes.
    #[test]
    fn speculative_rejection_sampling_preserves_the_target_distribution() {
        let target = [0.5f32, 0.3, 0.15, 0.05];
        for draft in [
            [0.25f32, 0.25, 0.25, 0.25],
            [0.9, 0.05, 0.03, 0.02],
            [0.05, 0.05, 0.05, 0.85],
            [0.5, 0.3, 0.15, 0.05],
        ] {
            let residual = speculative_residual(&target, &draft);
            let accept_total: f32 = (0..4)
                .map(|token| draft[token] * speculative_acceptance(target[token], draft[token]))
                .sum();
            for token in 0..4 {
                let emitted = draft[token] * speculative_acceptance(target[token], draft[token])
                    + (1.0 - accept_total) * residual[token];
                assert!(
                    (emitted - target[token]).abs() < 1e-5,
                    "draft {draft:?} token {token}: emitted {emitted} vs target {}",
                    target[token]
                );
            }
        }
    }

    #[test]
    fn xorshift_is_seed_deterministic() {
        let mut a = Xorshift64::new(3);
        let mut b = Xorshift64::new(3);
        let mut c = Xorshift64::new(4);
        let first: Vec<u64> = (0..8).map(|_| a.next_u64()).collect();
        let second: Vec<u64> = (0..8).map(|_| b.next_u64()).collect();
        let other: Vec<u64> = (0..8).map(|_| c.next_u64()).collect();
        assert_eq!(first, second);
        assert_ne!(first, other);
    }

    /// The verify batch writes `draft_max + 1` carry rows starting one past the
    /// last committed row, so the ring must never wrap back onto that row.
    #[test]
    fn carry_ring_always_clears_a_verify_batch() {
        for prefill in [1usize, 32, 256, 4096] {
            for draft_max in [1usize, 2, 4, 8] {
                let ring = mtp_carry_ring(prefill, draft_max, 262144);
                assert!(ring >= draft_max + 2, "ring {ring} draft_max {draft_max}");
                assert!(ring >= prefill, "ring {ring} prefill {prefill}");
            }
        }
        // A tiny context cannot force a ring smaller than one verify batch.
        assert!(mtp_carry_ring(1, 4, 4) >= 6);
    }
}

#[cfg(test)]
mod argmax_tests {
    use super::argmax_token_id;

    #[test]
    fn argmax_picks_the_first_maximum() {
        assert_eq!(argmax_token_id(&[0.0, 3.0, 1.0, 3.0]).unwrap(), 1);
        assert_eq!(argmax_token_id(&[-1.0, -2.0, -0.5]).unwrap(), 2);
        assert_eq!(argmax_token_id(&[5.0]).unwrap(), 0);
    }

    #[test]
    fn argmax_skips_nan_but_still_answers_when_all_nan() {
        assert_eq!(argmax_token_id(&[f32::NAN, 1.0, f32::NAN]).unwrap(), 1);
        let all_nan = [f32::NAN, f32::NAN];
        assert!(argmax_token_id(&all_nan).is_ok());
    }

    #[test]
    fn argmax_rejects_empty() {
        assert!(argmax_token_id(&[]).is_err());
    }

    #[test]
    fn argmax_handles_negative_infinity_rows() {
        // one_hot_logits produces exactly this shape.
        let mut logits = vec![f32::NEG_INFINITY; 8];
        logits[5] = 0.0;
        assert_eq!(argmax_token_id(&logits).unwrap(), 5);
    }
}
