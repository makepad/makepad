use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use crate::{ggml_row_size_for_type, TensorType};

use crate::error::{LlamaError, Result};
use crate::exec::{CompiledHybridDecode, ExecContextBuffers, ExecRuntime};
use crate::draft_vocab::DraftVocab;
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
    /// First arena row this graph's attention window starts at.
    ///
    /// Part of the KEY because two graphs of the same shape over different
    /// windows are different graphs: one reads slot 1's rows, the other slot
    /// 2's. Zero for everything except a slot's own prefill, so every graph
    /// that existed before keys, compiles and runs exactly as it did.
    attention_key_base: usize,
    attention_key_count: usize,
    /// Slots sharing this batch. 1 for every single-sequence path, which is
    /// every path that exists today — so a solo session keys, compiles and
    /// runs exactly the graph it always did.
    n_seqs: usize,
}

impl SessionGraphParams {
    fn new(n_tokens: usize, n_outputs: usize, attention_key_count: usize) -> Self {
        Self {
            kind: SessionGraphKind::Main,
            n_tokens,
            n_outputs,
            attention_key_base: 0,
            attention_key_count,
            n_seqs: 1,
        }
    }

    fn greedy(n_tokens: usize, attention_key_count: usize) -> Self {
        Self::new(n_tokens, 1, attention_key_count)
    }

    /// One token per slot, one logit row per slot. `n_seqs == 1` reduces to
    /// `Self::new(1, 1, ..)`, i.e. today's single-token decode key, so the solo
    /// lane shares the existing compiled graph instead of getting a twin.
    fn batched(n_seqs: usize, attention_key_count: usize) -> Self {
        Self {
            kind: SessionGraphKind::Main,
            n_tokens: n_seqs,
            n_outputs: n_seqs,
            attention_key_base: 0,
            attention_key_count,
            n_seqs,
        }
    }

    /// A verify batch spanning `n_seqs` lanes, `n_tokens / n_seqs` tokens each.
    ///
    /// At one lane this reduces to `Self::verify(n_tokens, ..)` exactly, so the
    /// solo speculative path keys and reuses the graph it always did rather
    /// than compiling a twin of it.
    fn verify_batched(n_seqs: usize, n_tokens: usize, attention_key_count: usize) -> Self {
        Self {
            kind: SessionGraphKind::MainVerify,
            n_tokens,
            n_outputs: n_tokens,
            attention_key_base: 0,
            attention_key_count,
            n_seqs,
        }
    }

    fn greedy_embeddings(n_tokens: usize, attention_key_count: usize) -> Self {
        let mut params = Self::greedy(n_tokens, attention_key_count);
        params.kind = SessionGraphKind::MainEmbeddings;
        params
    }

    fn mtp_draft(n_tokens: usize, attention_key_count: usize) -> Self {
        Self {
            kind: SessionGraphKind::MtpDraft,
            n_tokens,
            n_outputs: 1,
            attention_key_base: 0,
            attention_key_count,
            n_seqs: 1,
        }
    }

    /// One draft step for each of `width` lanes: `width` columns, `width`
    /// logit rows.
    ///
    /// `n_seqs` stays 1 and that is not an oversight. The draft head is a
    /// single ATTENTION layer with a dense FFN (`qwen35_mtp_decode_spec`) — no
    /// recurrent scan, so nothing in its graph needs the sequence count. Lane
    /// separation is carried entirely by the per-token write rows and the mask
    /// lower bounds, the same way a slot prefill carries it.
    ///
    /// At width 1 this is `Self::mtp_draft(1, ..)` exactly.
    fn mtp_draft_batched(width: usize, attention_key_count: usize) -> Self {
        Self {
            kind: SessionGraphKind::MtpDraft,
            n_tokens: width,
            n_outputs: width,
            attention_key_base: 0,
            attention_key_count,
            n_seqs: 1,
        }
    }

    fn token_generation(max_context: usize) -> Self {
        Self::greedy(1, max_context)
    }
}

/// Build the row-compacted draft LM head: gather the draft vocabulary's rows
/// out of the full `output.weight` into a new `[n_embd, draft_vocab]` tensor of
/// the same quantised type.
///
/// Rows of a ggml matrix are contiguous byte runs, so this is a gather of
/// fixed-size slices and the kept rows stay **bit-identical** to the full
/// head's — a draft logit for a kept token is exactly the logit the full head
/// would have produced. Must run before the context buffers are created: the
/// resident prefix of the arena is what gets uploaded.
fn build_restricted_draft_head(
    weights: &mut LoadedGgufWeights,
    model: &LlamaModel,
    vocab: &DraftVocab,
) -> Result<()> {
    let tensors = model.qwen35_tensors()?;
    let source_name = tensors
        .mtp
        .as_ref()
        .and_then(|mtp| mtp.shared_head.as_ref())
        .unwrap_or(&tensors.globals.output)
        .name
        .clone();
    let source_id = weights.require_tensor_id(&source_name)?;
    let source = weights
        .ctx
        .tensor(source_id)
        .ok_or_else(|| LlamaError::format("draft head source tensor is invalid"))?;
    let n_embd = source.ne[0];
    let rows = source.ne[1];
    let ty = source.desc.ty;
    if u32::try_from(rows).unwrap_or(u32::MAX) != vocab.vocab_size {
        return Err(LlamaError::format(format!(
            "draft vocabulary was built for {} rows, '{source_name}' has {rows}",
            vocab.vocab_size
        )));
    }
    let row_bytes = ggml_row_size_for_type(ty, n_embd).map_err(LlamaError::format)?;

    // Gather host-side first: with a mapped gguf the source lives in the
    // read-only region and the destination in the dirty one, so they cannot be
    // borrowed at once.
    let mut gathered = Vec::with_capacity(row_bytes * vocab.len());
    {
        let source_bytes = weights
            .ctx
            .tensor_data(source_id)
            .map_err(LlamaError::format)?;
        for &token in &vocab.ids {
            let start = (token as usize)
                .checked_mul(row_bytes)
                .ok_or_else(|| LlamaError::format("draft head row offset overflow"))?;
            let end = start
                .checked_add(row_bytes)
                .ok_or_else(|| LlamaError::format("draft head row offset overflow"))?;
            let row = source_bytes.get(start..end).ok_or_else(|| {
                LlamaError::format(format!("draft head row {token} is outside '{source_name}'"))
            })?;
            gathered.extend_from_slice(row);
        }
    }

    let head = weights
        .ctx
        .new_named_tensor(
            crate::qwen35_runtime::MTP_DRAFT_HEAD_TENSOR,
            ty,
            2,
            &[n_embd, vocab.len() as i64],
            crate::BufferUsage::Weights,
        )
        .map_err(LlamaError::format)?;
    weights
        .ctx
        .write_tensor_data(head, &gathered)
        .map_err(LlamaError::format)?;
    weights
        .tensor_ids
        .insert(crate::qwen35_runtime::MTP_DRAFT_HEAD_TENSOR.to_string(), head);
    Ok(())
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

/// Rows the hidden-carry tensor needs for `lanes` lanes.
///
/// Each lane owns a contiguous block of `carry_ring + 2` rows: its ring, then
/// its scratch row, then its never-written zero row. Lane 0's block starts at
/// row 0, so a single-lane session addresses exactly the rows it always did.
///
/// The ring is indexed `position % carry_ring`, so a SHARED ring silently
/// aliases whenever two lanes sit at congruent positions — at ring 2048, every
/// 2048 tokens. That is a wrong-hidden-state feeding the draft head, which
/// surfaces as the model getting subtly worse rather than as an error. Per-lane
/// blocks make the collision unrepresentable.
fn carry_rows_total(carry_ring: usize, lanes: usize) -> usize {
    (carry_ring + 2) * lanes.max(1)
}

fn carry_ring_bytes(hidden_size: u32, carry_ring: usize, lanes: usize) -> Result<usize> {
    ggml_row_size_for_type(TensorType::F32, i64::from(hidden_size))
        .map_err(LlamaError::format)?
        .checked_mul(carry_rows_total(carry_ring, lanes))
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

/// Byte range of rows `first .. first + rows` of a 2-D state tensor.
///
/// Row-addressed because that is how `get_rows`/`set_rows` address these
/// caches: dimension 1 is the row and `nb[1]` is its stride. Every bound is
/// checked — a range computed one row wide of where it should be would clear
/// the neighbouring slot's state, which is the exact failure this is meant to
/// prevent, arrived at from the other side.
fn state_row_range(
    ctx: &crate::Context,
    tensor_id: crate::TensorId,
    first: usize,
    rows: usize,
) -> Result<(usize, usize)> {
    let tensor = ctx
        .tensor(tensor_id)
        .ok_or_else(|| LlamaError::format(format!("invalid state tensor id {tensor_id}")))?;
    let total = usize::try_from(tensor.ne[1]).unwrap_or(0);
    if tensor.ne[2] != 1 || tensor.ne[3] != 1 {
        return Err(LlamaError::format(format!(
            "state tensor {tensor_id} is not row-addressed: shape {:?}",
            tensor.ne
        )));
    }
    if first + rows > total {
        return Err(LlamaError::format(format!(
            "rows {first}..{} are outside the {total}-row state tensor {tensor_id}",
            first + rows
        )));
    }
    let stride = tensor.nb[1];
    let offset = tensor
        .data_offset
        .ok_or_else(|| {
            LlamaError::format(format!("state tensor {tensor_id} has no allocated data"))
        })?
        .checked_add(first * stride)
        .ok_or_else(|| LlamaError::format("overflow computing a state row offset"))?;
    Ok((offset, rows * stride))
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
    /// Present when the draft head's rows were restricted to a sidecar
    /// vocabulary; maps the head's dense output index back to a token id.
    draft_vocab: Option<DraftVocab>,
    config: LlamaSessionConfig,
    max_context: usize,
    /// Rows in the shared attention arena: `max_sequences * max_context`.
    /// Equals `max_context` for a single-slot session, which is every session
    /// that exists today.
    attention_arena_rows: usize,
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

    /// Speculative draft depth actually ACTIVE, which is 0 whenever the draft
    /// head did not load — `spec_draft_max` is only a request, and a gguf
    /// without a nextn block silently declines it.
    ///
    /// Exposed so a test can refuse to pass vacuously: a gate that thinks it
    /// exercised speculation, on a model that has none, certifies nothing.
    pub fn speculation_depth(&self) -> usize {
        self.mtp.map(|mtp| mtp.draft_max).unwrap_or(0)
    }

    /// Tokens that end a turn: this model's end-of-sequence and padding ids.
    ///
    /// Exposed because a scheduler that owns lanes has to know when a lane's
    /// turn is over, and it has no vocabulary of its own to ask. The
    /// single-stream loop consults [`Self::stop_reason_for`] internally; a
    /// caller driving decode itself must consult this.
    pub fn stop_tokens(&self) -> Vec<i32> {
        let mut tokens = Vec::with_capacity(2);
        if let Some(eos) = self.vocab.eos_token_id() {
            tokens.push(eos);
        }
        if let Some(pad) = self.vocab.padding_token_id() {
            if !tokens.contains(&pad) {
                tokens.push(pad);
            }
        }
        tokens
    }

    /// Slots this session was built for. 1 for every single-stream session.
    pub fn slot_count(&self) -> usize {
        self.config.max_sequences as usize
    }

    /// Rows in the shared attention arena, `slot_count * max_context`.
    pub fn attention_arena_rows(&self) -> usize {
        self.attention_arena_rows
    }

    /// Bytes the attention K/V caches occupy for this session's geometry.
    ///
    /// Reported rather than estimated because per-token KV cost is a property
    /// of the model's attention shape — how many layers actually have
    /// attention, their head dims, the cache types — and a hybrid model's is
    /// nothing like a full-attention model's. Sizing a box from a guess is how
    /// a context knob turns into an out-of-memory at load, on the box, after a
    /// swap.
    pub fn attention_cache_bytes(&self) -> Result<usize> {
        // NOT `attention_cache_bytes_from_spec`: that one REFUSES a spec
        // containing recurrent layers, because it is the fallback for sizing a
        // whole cache without a template. Every hybrid model has recurrent
        // layers, so asking it here returned an error that a caller would
        // reasonably turn into 0 — and a zero-byte KV report is worse than no
        // report, because it reads as a fact.
        let mut total = 0usize;
        let mut seen = BTreeSet::new();
        for layer in &self.spec.layers {
            let HybridLayerSpec::Attention { decode, .. } = layer else {
                continue;
            };
            if !seen.insert(decode.cache_layer_index) {
                continue;
            }
            let rows = u64::from(decode.cache.max_context)
                .checked_mul(u64::from(decode.cache.max_sequences))
                .ok_or_else(|| LlamaError::format("attention cache rows overflow"))?;
            for (width, ty) in [
                (
                    u64::from(decode.block.k_head_dim) * u64::from(decode.block.kv_head_count),
                    decode.cache.k_type,
                ),
                (
                    u64::from(decode.block.v_head_dim) * u64::from(decode.block.kv_head_count),
                    decode.cache.v_type,
                ),
            ] {
                let elements = width
                    .checked_mul(rows)
                    .ok_or_else(|| LlamaError::format("attention cache elements overflow"))?;
                let bytes = ggml_row_size_for_type(
                    ty,
                    i64::try_from(elements)
                        .map_err(|_| LlamaError::format("attention elements do not fit in i64"))?,
                )
                .map_err(LlamaError::format)?;
                total = total
                    .checked_add(bytes)
                    .ok_or_else(|| LlamaError::format("attention cache bytes overflow"))?;
            }
        }
        Ok(total)
    }

    /// Bytes one more token of context costs, per lane.
    ///
    /// The number to multiply when answering "what would 128k cost?".
    pub fn attention_cache_bytes_per_token(&self) -> Result<usize> {
        Ok(self.attention_cache_bytes()? / self.attention_arena_rows.max(1))
    }

    /// A slot table matching this session's cache geometry exactly.
    ///
    /// Built here rather than by the caller so the table's bases and the
    /// caches they index can never disagree — a slot whose `kv_base` did not
    /// match its cache region would read another conversation's history and
    /// produce fluent, wrong text.
    pub fn new_slot_table(&self) -> Result<crate::slots::SlotTable> {
        let draft_max = self.mtp.map(|_| self.config.spec_draft_max).unwrap_or(0);
        let state_rows_per_slot = if draft_max > 0 { draft_max + 2 } else { 1 };
        crate::slots::SlotTable::new(
            self.slot_count(),
            self.max_context,
            state_rows_per_slot,
            draft_max,
        )
    }

    /// Zero one slot's recurrent state and hidden-carry block.
    ///
    /// The counterpart to admission, and NOT the same thing as
    /// [`reset`](Self::reset): that one clears the whole device state and
    /// therefore every other conversation's caches too.
    ///
    /// A slot's stale ATTENTION rows are harmless — they sit above the new
    /// fill and the mask's lower/upper span excludes them exactly, which is why
    /// admission can hand out a slot without touching the KV. The recurrent
    /// state is the opposite: the delta-net scan RESUMES from the row it is
    /// given, so a fresh conversation handed a used slot starts its scan from
    /// the previous occupant's running state. Nothing errors. The reply is
    /// fluent and is conditioned on a conversation this client never had.
    ///
    /// Cheap: the recurrent rows of one slot plus its carry block, not the
    /// whole arena.
    pub fn clear_slot_state(&mut self, lane: usize) -> Result<()> {
        if lane >= self.slot_count() {
            return Err(LlamaError::format(format!(
                "slot {lane} is outside the session's {} slots",
                self.slot_count()
            )));
        }
        let rows_per_slot = self.state_rows_per_slot();
        let first = lane * rows_per_slot;
        let mut ranges = Vec::new();
        for ids in self.graphs.shared_cache.recurrent.values() {
            for tensor in [ids.r_cache, ids.s_cache] {
                ranges.push(state_row_range(&self.weights.ctx, tensor, first, rows_per_slot)?);
            }
        }
        if let (Some(carry), Some(mtp)) = (self.graphs.hidden_carry, self.mtp) {
            // Ring, scratch row and the never-written zero row: a lane's whole
            // block, because the zero row is load-bearing precisely by being
            // zero and the previous occupant's scratch row is not this lane's.
            let block = mtp.carry_ring + 2;
            ranges.push(state_row_range(
                &self.weights.ctx,
                carry,
                lane * block,
                block,
            )?);
        }
        ranges.sort_unstable();
        ranges.dedup();
        self.graphs.shared_runtime.clear_state_ranges(
            &mut self.weights.ctx,
            &self.graphs.shared_buffers,
            &ranges,
        )
    }

    /// Prefill one slot's region with a chunk of tokens.
    ///
    /// A prefill chunk is ONE sequence, so this runs at `n_seqs == 1` — the
    /// slot only shows up as a nonzero `kv_base` on the cache writes and a
    /// nonzero lower bound on the mask. That means every slot's prefill uses
    /// the same compiled graphs as today's single-stream prefill, and works on
    /// Metal as well as CUDA.
    ///
    /// `start` is the within-slot position of the first token. Returns the
    /// logits of the chunk's last token.
    pub fn prefill_slot_chunk(
        &mut self,
        lane: usize,
        kv_base: usize,
        state_row: usize,
        start: usize,
        tokens: &[i32],
    ) -> Result<Vec<f32>> {
        if tokens.is_empty() {
            return Err(LlamaError::format("a prefill chunk needs at least one token"));
        }
        let batch = tokens.len();
        let positions: Vec<i32> = (start..start + batch)
            .map(|position| {
                i32::try_from(position)
                    .map_err(|_| LlamaError::format("slot position does not fit in i32"))
            })
            .collect::<Result<Vec<_>>>()?;
        let write_indices: Vec<i32> = (start..start + batch)
            .map(|position| {
                i32::try_from(kv_base + position)
                    .map_err(|_| LlamaError::format("slot cache row does not fit in i32"))
            })
            .collect::<Result<Vec<_>>>()?;
        let base = i32::try_from(kv_base)
            .map_err(|_| LlamaError::format("slot kv_base does not fit in i32"))?;
        // The window is the SLOT's own rows, not the arena from row 0.
        //
        // A prefill graph costs `n_tokens x attention_key_count` in mask bytes
        // and in the attention kernel's pass over the key span, and under
        // slot-major addressing a lane's absolute span is its BASE plus its
        // fill. Keying off the absolute span made lane N of a 128k-per-lane box
        // pay for hundreds of thousands of rows belonging to other
        // conversations — measured at 2.2x on lane 1 of a 2x64k session, and it
        // grows with the base. Anchoring the window at `kv_base` makes a slot
        // prefill cost exactly what the same prompt costs on lane 0, which is
        // what it always should have cost.
        //
        // The bucketing is unchanged, only what it is applied to: the span
        // WITHIN the slot. So slot 0 (base 0) keys the same graphs it always
        // did and is byte-identical by construction.
        let key_span = start + batch;
        let window = if std::env::var_os("MAKEPAD_LLAMA_PER_LEN_GRAPHS").is_some() {
            key_span
        } else {
            key_span.next_multiple_of(GRAPH_KEY_BUCKET)
        }
        .min(self.attention_arena_rows.saturating_sub(kv_base).max(1));
        let attention_key_count = window;
        let mut graph_params = SessionGraphParams::greedy(batch, attention_key_count);
        graph_params.attention_key_base = kv_base;
        self.ensure_compiled_graph(graph_params)?;
        let state_row_i32 = i32::try_from(state_row)
            .map_err(|_| LlamaError::format("slot state row does not fit in i32"))?;
        let dump_row = self
            .mtp
            .map(|mtp| self.carry_dump_row(&mtp, lane))
            .unwrap_or(0);
        let run = {
            let compiled = self
                .graphs
                .graph_for_mut(graph_params)
                .ok_or_else(|| LlamaError::format("compiled prefill graph was not cached"))?;
            let output_ids = [i32::try_from(batch - 1)
                .map_err(|_| LlamaError::format("slot output id does not fit in i32"))?];
            let mut layout = HybridDecodeBatchLayout::from_contiguous_positions_and_outputs(
                &positions,
                attention_key_count,
                &output_ids,
            )?;
            layout.attention_write_indices = write_indices;
            // No lower bounds, at ANY base. The window starts at this slot's
            // first row, so there is nothing below it to exclude — the mask is
            // the ordinary causal one and every slot takes the mask builder's
            // single-sequence path, which is what makes slot k byte-identical
            // to slot 0 rather than merely equivalent to it.
            let _ = base;
            layout.attention_key_base = kv_base;
            layout.recurrent_state_rows = vec![state_row_i32];
            if compiled.decode().input_recurrent_state_rows.is_none() {
                layout.recurrent_state_rows.clear();
            }
            // With a draft head loaded the graph obliges every column to write
            // a hidden-carry row. This lane is not speculating, so it dumps
            // into its OWN block's scratch row: nothing reads it while the
            // lane is not drafting, and it cannot alias another lane's ring.
            if compiled.decode().input_hidden_write_rows.is_some() {
                layout.hidden_write_rows = vec![dump_row; batch];
            }
            compiled.execute_logits_only_with_layout(LogitsProbeInput::TokenIds(tokens), &layout)?
        };
        let mut rows = split_run_logits(run, 1)?;
        rows.pop()
            .ok_or_else(|| LlamaError::format("prefill chunk produced no logits"))
    }

    /// Run one multi-slot decode step: one token per active slot, one logit row
    /// per active slot, in plan order.
    ///
    /// `tokens[i]` is the token slot `plan.slots[i]` decodes. The caller owns
    /// sampling (one `LlamaSamplerState` per slot) and tells the slot table
    /// what was produced.
    pub fn step_slots(
        &mut self,
        plan: &crate::slots::StepPlan,
        tokens: &[i32],
    ) -> Result<Vec<Vec<f32>>> {
        if plan.slots.is_empty() {
            return Err(LlamaError::format("a decode step needs at least one slot"));
        }
        let width = plan.slots.len();
        if tokens.len() != width {
            return Err(LlamaError::format(format!(
                "step plan has {} slots but {} tokens were supplied",
                width,
                tokens.len()
            )));
        }
        if width > self.slot_count() {
            return Err(LlamaError::format(format!(
                "step plan has {} slots but the session was built for {}",
                width,
                self.slot_count()
            )));
        }
        let layout = plan.to_batch_layout()?;
        // Bucketed exactly as the single-stream path buckets, but against the
        // ARENA rather than one slot's context, because the key span is an
        // absolute row count across slots.
        let attention_key_count = if std::env::var_os("MAKEPAD_LLAMA_PER_LEN_GRAPHS").is_some() {
            layout.attention_key_count
        } else {
            layout
                .attention_key_count
                .next_multiple_of(GRAPH_KEY_BUCKET)
                .min(self.attention_arena_rows)
        };
        let graph_params = SessionGraphParams::batched(width, attention_key_count);
        self.ensure_compiled_graph(graph_params)?;
        let mut layout = layout;
        layout.attention_key_count = attention_key_count;
        // Same obligation as prefill: with a draft head loaded every column
        // must write a hidden row. Phase 1 does not speculate inside a batch,
        // so each lane dumps into its own block and no ring is touched.
        let dump_rows: Vec<i32> = match self.mtp {
            Some(mtp) => plan
                .slots
                .iter()
                .map(|step| self.carry_dump_row(&mtp, step.slot))
                .collect(),
            None => Vec::new(),
        };
        let run = {
            let compiled = self
                .graphs
                .graph_for_mut(graph_params)
                .ok_or_else(|| LlamaError::format("compiled batched graph was not cached"))?;
            if compiled.decode().input_hidden_write_rows.is_some() {
                layout.hidden_write_rows = dump_rows;
            }
            compiled.execute_logits_only_with_layout(LogitsProbeInput::TokenIds(tokens), &layout)?
        };
        split_run_logits(run, width)
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
        // `max_context` is PER SLOT. Slots share one flat attention arena of
        // `slots * per_slot_context` rows, and the slot index is folded into
        // the CONTEXT dimension rather than becoming a sequence dimension —
        // so `n_seq_max` stays 1 and every downstream shape is unchanged.
        // That is what keeps `set_rows` at ne[2] == 1, keeps the flash op at
        // q.ne[3] == 1, and keeps the mask 2-D: slot membership lives in the
        // mask VALUES, so no CUDA kernel has to learn about slots.
        let attention_arena_rows = u32::try_from(config.max_sequences)
            .ok()
            .and_then(|slots| max_context.checked_mul(slots))
            .ok_or_else(|| {
                LlamaError::format("session attention arena overflows: slots x max_context")
            })?;
        let cache_shape = HybridCacheShape {
            n_ctx_seq: attention_arena_rows,
            n_seq_max: 1,
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
        // Each slot owns a contiguous block of recurrent rows: the live row
        // plus one checkpoint per verify-batch position when speculating.
        // Recurrent rows are per SLOT, and the slot count is `max_sequences`.
        // The attention cache's `n_seq_max` is 1 because the slot index folds
        // into the context dimension, so it must NOT be reused here — doing so
        // gave every slot row 0 and therefore one shared recurrent state.
        //
        // With speculation each slot owns `draft_max + 2` rows: the live row
        // plus one checkpoint per verify-batch position.
        let rows_per_slot = if draft_max > 0 { draft_max + 2 } else { 1 };
        let recurrent_rows = (config.max_sequences as usize)
            .checked_mul(rows_per_slot)
            .and_then(|rows| u32::try_from(rows).ok())
            .ok_or_else(|| LlamaError::format("recurrent row count is too large"))?;
        for spec in [&mut spec, &mut spec_embeddings] {
            for layer in spec.layers.iter_mut() {
                if let HybridLayerSpec::Recurrent { decode, .. } = layer {
                    decode.cache.max_sequences = recurrent_rows;
                }
            }
        }
        // MKLLM_MTP_FULL_DRAFT_VOCAB=1 keeps the full 248320-row draft head
        // (the A/B for the restricted-head win).
        let draft_vocab = if draft_max > 0 && std::env::var_os("MKLLM_MTP_FULL_DRAFT_VOCAB").is_none()
        {
            DraftVocab::load_for_model(
                &model.gguf.path,
                u32::try_from(vocab.len())
                    .map_err(|_| LlamaError::format("vocabulary size does not fit in u32"))?,
            )?
        } else {
            None
        };
        if let Some(loaded) = draft_vocab.as_ref() {
            eprintln!(
                "llama: mtp draft head restricted to {} of {} tokens ({:.1}% corpus coverage)",
                loaded.len(),
                loaded.vocab_size,
                loaded.coverage() * 100.0
            );
        }
        let mut spec_mtp = if draft_max > 0 {
            model.mtp_decode_spec(
                cache_shape.n_ctx_seq,
                cache_shape.n_seq_max,
                config.attention_k_type,
                config.attention_v_type,
                draft_vocab.is_some(),
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
                .checked_add(carry_ring_bytes(
                    model.embedding_length()?,
                    carry_ring,
                    config.max_sequences as usize,
                )?)
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
            draft_vocab.as_ref(),
            carry_ring,
            config.max_sequences as usize,
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
            draft_vocab,
            config,
            max_context: max_context_usize,
            attention_arena_rows: usize::try_from(attention_arena_rows).map_err(|_| {
                LlamaError::format("session attention arena does not fit in usize")
            })?,
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
        self.carry_rows_for_lane_positions(0, start, count)
    }

    /// Ring rows for `count` tokens of `lane` starting at its within-lane
    /// position `start`. Lane 0's base is 0, so this is byte-identical to the
    /// single-sequence arithmetic it replaces.
    fn carry_rows_for_lane_positions(&self, lane: usize, start: usize, count: usize) -> Vec<i32> {
        let Some(mtp) = self.mtp.as_ref() else {
            return Vec::new();
        };
        let base = self.carry_lane_base(mtp, lane);
        (start..start + count)
            .map(|index| base + (index % mtp.carry_ring) as i32)
            .collect()
    }

    /// First row of `lane`'s carry block.
    fn carry_lane_base(&self, mtp: &MtpRuntime, lane: usize) -> i32 {
        (lane * (mtp.carry_ring + 2)) as i32
    }

    fn carry_scratch_row(&self, mtp: &MtpRuntime) -> i32 {
        self.carry_scratch_row_for(mtp, 0)
    }

    fn carry_scratch_row_for(&self, mtp: &MtpRuntime, lane: usize) -> i32 {
        self.carry_lane_base(mtp, lane) + mtp.carry_ring as i32
    }

    fn carry_zero_row(&self, mtp: &MtpRuntime) -> i32 {
        self.carry_zero_row_for(mtp, 0)
    }

    fn carry_zero_row_for(&self, mtp: &MtpRuntime, lane: usize) -> i32 {
        self.carry_lane_base(mtp, lane) + mtp.carry_ring as i32 + 1
    }

    /// Where a lane that is NOT speculating dumps the hidden row the graph
    /// obliges every batch column to write.
    ///
    /// It is that lane's own scratch row, which nothing else reads while the
    /// lane is not drafting. Deliberately NOT the shared scratch row: session
    ///.rs reads that as the draft chain's `h_{-1}` source, so a foreign write
    /// there corrupts another lane's drafts. And not the zero row, which is
    /// load-bearing precisely by staying zero.
    fn carry_dump_row(&self, mtp: &MtpRuntime, lane: usize) -> i32 {
        self.carry_scratch_row_for(mtp, lane)
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
                params.attention_key_base,
                params.attention_key_count,
                params.n_seqs,
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
                        self.draft_vocab.as_ref(),
                        self.mtp.map(|mtp| mtp.carry_ring).unwrap_or(0),
                        self.config.max_sequences as usize,
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

/// The normalised residual `max(0, p - q)` a rejected draft is replaced from,
/// with the proposal `q` given sparsely as `(token, probability)` over real
/// token ids — the draft head may cover only part of the vocabulary.
/// Together with `speculative_acceptance` this makes the emitted token exactly
/// `p`-distributed (Leviathan et al. / Chen et al. speculative sampling).
/// Degenerate case: when `q == p` everywhere the residual is empty, and the
/// only way to get there is `p(x)/q(x) == 1` for the drafted token, i.e. the
/// draft was already accepted — so falling back to `p` is unreachable in
/// practice and merely keeps the function total.
fn speculative_residual(target: &[f32], draft: &[(u32, f32)]) -> Vec<f32> {
    let mut residual: Vec<f32> = target.to_vec();
    for &(token, q) in draft {
        if let Some(slot) = residual.get_mut(token as usize) {
            *slot = (*slot - q).max(0.0);
        }
    }
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
#[derive(PartialEq, Clone, Copy, Debug)]
pub struct LlamaSamplingParams {
    pub temperature: f32,
    pub top_p: f32,
    /// 0 disables top-k.
    pub top_k: usize,
    pub seed: u64,
    /// Subtracted from the logit of every token that appears AT ALL in the
    /// recent window. Flat: seen once costs the same as seen twenty times.
    pub presence_penalty: f32,
    /// Subtracted once per OCCURRENCE in the recent window, so a token the
    /// generation keeps returning to gets pushed down further each time.
    pub frequency_penalty: f32,
    /// How many of the most recently sampled tokens the two penalties look
    /// at. 0 disables both, which is the default and reproduces every
    /// pre-penalty run exactly.
    pub penalty_last_n: usize,
}

impl Default for LlamaSamplingParams {
    fn default() -> Self {
        Self {
            temperature: 0.7,
            top_p: 0.9,
            top_k: 0,
            seed: 7,
            presence_penalty: 0.0,
            frequency_penalty: 0.0,
            penalty_last_n: 0,
        }
    }
}

impl LlamaSamplingParams {
    fn is_greedy(&self) -> bool {
        self.temperature <= 0.0
    }

    /// Whether the penalties can change any logit. Both a zero window and two
    /// zero strengths mean "not configured", and the sampler then skips the
    /// whole path rather than copying a 248320-entry row to subtract nothing
    /// from it.
    fn penalises(&self) -> bool {
        self.penalty_last_n > 0
            && (self.presence_penalty != 0.0 || self.frequency_penalty != 0.0)
    }
}

/// Sampler RNG state for ONE generation.
///
/// The RNG stream must be seeded once per generation and then carried across
/// every call that continues that generation. Callers that decode in chunks —
/// the asset-ai chat worker decodes 24 tokens at a time so it can check
/// cancellation and publish a partial-text snapshot — used to call
/// [`LlamaSession::continue_sampled`] once per chunk, which re-seeded from
/// `params.seed` every time: token 0 and token 24 drew the SAME uniform, and
/// with the same logits (a real occurrence in a repetitive tail) they sampled
/// the same token. The stream was 24 draws long, replayed, for the whole reply.
///
/// So the state is explicit and caller-owned rather than derived from `seed`
/// inside the call. It is deliberately **not** `Copy`: forking the stream must
/// be a visible `.clone()`, never an accidental move-out. One state per
/// generation — and, once slots interleave, one state per slot.
#[derive(Clone, Debug)]
pub struct LlamaSamplerState {
    rng: Xorshift64,
    /// The tokens this generation has committed, most recent last, trimmed to
    /// the penalty window. It lives here rather than being re-derived from the
    /// session's token vector for the same reason the RNG does: a lane's
    /// generation is chunked across many calls and interleaved with three other
    /// lanes, and "the tokens I have produced" must follow the conversation,
    /// not the slot it happens to be sitting in. Empty when no penalty is
    /// configured — nothing is recorded that nothing will read.
    recent: Vec<i32>,
}

impl LlamaSamplerState {
    /// Start a fresh stream. Same seed, same stream — reproducibility is
    /// unchanged, it is only the re-seeding that is gone.
    pub fn new(seed: u64) -> Self {
        Self {
            rng: Xorshift64::new(seed),
            recent: Vec::new(),
        }
    }

    /// The window the penalties read.
    fn recent(&self) -> &[i32] {
        &self.recent
    }

    /// Record a committed token, keeping at most `penalty_last_n` of them.
    ///
    /// Called for tokens the generation actually KEPT. A speculative round
    /// drafts tokens it then throws away, and a rejected draft was never part
    /// of the reply — penalising against it would make the penalty depend on
    /// how the drafter guessed, which is the one thing speculation is not
    /// allowed to change.
    fn remember(&mut self, token: i32, params: LlamaSamplingParams) {
        if !params.penalises() {
            return;
        }
        self.recent.push(token);
        let over = self.recent.len().saturating_sub(params.penalty_last_n);
        if over > 0 {
            self.recent.drain(..over);
        }
    }

    /// Draw one token from a raw logits row, advancing this stream.
    ///
    /// The batched worker holds one row per lane and one state per lane, so it
    /// samples directly rather than through the single-sequence generate loop.
    /// Semantics match [`LlamaSession::continue_sampled_with`] exactly —
    /// greedy when the temperature is non-positive, otherwise
    /// temperature/top-k/top-p then a draw — so a lane sampled here and a
    /// sequence sampled there make the same choices from the same logits and
    /// the same stream position.
    pub fn sample_logits(
        &mut self,
        logits: &[f32],
        params: LlamaSamplingParams,
    ) -> Result<i32> {
        let token = if params.is_greedy() {
            // Greedy is penalised too. A temperature-zero loop is the worst
            // kind — nothing random can ever break it — so the one path that
            // most needs the penalty is not the one to leave out.
            match penalty_window(params, self.recent()) {
                Some(penalties) => argmax_penalized(logits, &penalties)?,
                None => argmax_token_id(logits)?,
            }
        } else {
            let probs = sampling_probabilities(logits, params, self.recent())?;
            sample_from(&probs, &mut self.rng)?
        };
        self.remember(token, params);
        Ok(token)
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
fn sampling_probabilities(
    logits: &[f32],
    params: LlamaSamplingParams,
    recent: &[i32],
) -> Result<Vec<f32>> {
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
    // The penalties fold in here as a rescale rather than as a second pass over
    // a copied logit row: `exp((l - p - max) / T) == exp((l - max) / T) *
    // exp(-p / T)`, exactly, so touching the handful of penalised entries is
    // the same arithmetic as penalising the whole row would have been. Using
    // the UNPENALISED max as the stability offset stays valid because a penalty
    // only ever subtracts — nothing can exceed the offset and overflow.
    if let Some(penalties) = penalty_window(params, recent) {
        for (index, penalty) in penalties {
            if let Some(value) = probs.get_mut(index) {
                *value *= (-penalty / temperature).exp();
            }
        }
    }
    let sum: f32 = probs.iter().sum();
    if !(sum > 0.0) {
        return Err(LlamaError::format("logit softmax underflowed to zero"));
    }
    for value in probs.iter_mut() {
        *value /= sum;
    }

    // Rank once, then apply top-k and top-p on the same ordering. Sorting the
    // whole vocabulary is not affordable here: it is 248320 entries on
    // Qwen3.8 and the speculative path ranks `n_draft + n_verify` rows per
    // round, which costs more than the forward passes themselves. A nucleus
    // wide enough to matter is tiny, so partition to a candidate cap first
    // (O(V)) and sort only that; if the nucleus genuinely needs more than the
    // cap, fall back to the exact full ranking.
    const CANDIDATE_CAP: usize = 1024;
    let mut order: Vec<u32> = (0..probs.len() as u32).collect();
    let by_prob = |probs: &[f32], a: &u32, b: &u32| probs[*b as usize].total_cmp(&probs[*a as usize]);
    let mut keep = if params.top_k > 0 {
        params.top_k.min(probs.len())
    } else {
        probs.len()
    };
    let top_p = params.top_p.clamp(0.0, 1.0);
    let mut capped = false;
    if top_p < 1.0 {
        let cap = keep.min(CANDIDATE_CAP);
        if cap < keep {
            capped = true;
            keep = cap;
        }
    }
    if keep < probs.len() {
        order.select_nth_unstable_by(keep - 1, |a, b| by_prob(&probs, a, b));
        order.truncate(keep);
    }
    order.sort_unstable_by(|a, b| by_prob(&probs, a, b));
    if capped && order.iter().map(|token| probs[*token as usize]).sum::<f32>() < top_p {
        // The nucleus is wider than the cap — redo exactly.
        order = (0..probs.len() as u32).collect();
        order.sort_unstable_by(|a, b| by_prob(&probs, a, b));
    }

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

/// Push the tokens a generation keeps returning to back down the logit row.
///
/// This is the ONLY thing standing between a thinking model at long context
/// and a reply that runs to the token cap. A 27B at Q4 given a conversation
/// full of ids and measurements will, often enough to matter, stop reading its
/// context and start CONTINUING A PATTERN instead — the counting babble a
/// player sees as `1.1.1.1..4.23.23.4.234.24`. The loop is self-reinforcing:
/// every repetition makes the next repetition likelier, so once it starts,
/// temperature and nucleus sampling cannot end it, and the box's own telemetry
/// shows why — a looping turn verifies at acceptance 0.96 against 0.44-0.68 for
/// ordinary prose, because a loop is the easiest thing in the world to predict.
/// Qwen's own guidance for quantised 3.x is a presence penalty for exactly this
/// symptom.
///
/// Both penalties are the OpenAI shape — flat for presence, per-occurrence for
/// frequency — subtracted from the raw logit before temperature, so the
/// strength means the same thing whatever the temperature is.
///
/// It never copies the logit row. `sampling_probabilities` already allocates
/// two vocabulary-sized vectors per call, and the runbook records that ranking
/// this vocabulary "costs more than the forward passes themselves" — so a third
/// 1 MB copy per sampled token per lane is not a cost this path can absorb for
/// a feature that is about to be on by default. At most `penalty_last_n`
/// entries change, so the caller is handed those and applies them where it is
/// already touching the data.
///
/// Returns `None` when nothing is configured, which is the whole feature
/// switched off with no work done at all.
fn penalty_window(params: LlamaSamplingParams, recent: &[i32]) -> Option<BTreeMap<usize, f32>> {
    if !params.penalises() || recent.is_empty() {
        return None;
    }
    let window = recent.len().min(params.penalty_last_n);
    let mut counts: BTreeMap<usize, f32> = BTreeMap::new();
    for &token in &recent[recent.len() - window..] {
        // An id that is not a vocabulary index is ignored rather than fatal.
        // The window is fed from committed token ids, and taking a lane down
        // mid-conversation over one odd value would be a far worse failure
        // than declining to penalise it.
        let Ok(index) = usize::try_from(token) else {
            continue;
        };
        *counts.entry(index).or_insert(0.0) += 1.0;
    }
    let mut out: BTreeMap<usize, f32> = BTreeMap::new();
    for (index, count) in counts {
        let penalty = params.presence_penalty + params.frequency_penalty * count;
        if penalty != 0.0 {
            out.insert(index, penalty);
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// Greedy's answer under the same penalties, without materialising a penalised
/// row. The penalties only ever LOWER a logit, so the winner is either the best
/// entry outside the window or the best penalised entry inside it, and both are
/// found in one pass plus a walk of at most `penalty_last_n` entries.
fn argmax_penalized(logits: &[f32], penalties: &BTreeMap<usize, f32>) -> Result<i32> {
    let mut best: Option<(usize, f32)> = None;
    for (index, &logit) in logits.iter().enumerate() {
        if logit.is_nan() {
            continue;
        }
        let value = logit - penalties.get(&index).copied().unwrap_or(0.0);
        if best.is_none_or(|(_, seen)| value > seen) {
            best = Some((index, value));
        }
    }
    let (index, _) = best.ok_or_else(|| LlamaError::format("logits contain no finite value"))?;
    i32::try_from(index).map_err(|_| LlamaError::format("sampled token does not fit in i32"))
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

    /// True when the draft head reads a restricted `.draftvocab` LM head.
    ///
    /// It is a ~3.3x cheaper draft forward at identical acceptance, which is
    /// the term that decides whether a deeper batched round pays — so the
    /// allocator has to be told which head it is costing, not assume the
    /// expensive one.
    pub fn has_restricted_draft_head(&self) -> bool {
        self.draft_vocab.is_some()
    }

    /// Offset inside slot 0's recurrent block that the session's own
    /// single-sequence state currently lives at.
    ///
    /// The session-native speculative path MOVES this every round — it commits
    /// by choosing which checkpoint plane the next scan resumes from. Slot 0's
    /// copy of the same fact does not move on its own, so anything that hands a
    /// solo lane over to the batched path has to carry this across; see
    /// [`crate::LaneExecutor`]. Without it the batched step resumes lane 0 from
    /// a checkpoint belonging to a different number of committed tokens, and
    /// the output is fluent and wrong.
    pub fn live_state_offset(&self) -> usize {
        usize::try_from(self.live_state_row()).unwrap_or(0)
    }

    /// Tokens of the session's own history the DRAFT head's KV holds.
    ///
    /// Lags the model's fill until a catch-up runs. Carried across the same
    /// handover as [`live_state_offset`](Self::live_state_offset): a slot that
    /// under-reports it makes the draft head re-ingest the whole conversation,
    /// and one that over-reports it leaves the draft head conditioned on tokens
    /// it never read.
    pub fn draft_head_fill(&self) -> usize {
        self.mtp.map(|mtp| mtp.mtp_filled).unwrap_or(0)
    }

    /// Sample `max_new_tokens` tokens as ONE self-contained generation: the
    /// RNG stream starts at `params.seed` and ends with the call.
    ///
    /// Decoding a reply in several chunks is therefore **not** a loop over this
    /// method — that restarts the stream at every chunk boundary. Seed a
    /// [`LlamaSamplerState`] once and loop over
    /// [`continue_sampled_with`](Self::continue_sampled_with) instead.
    pub fn continue_sampled(
        &mut self,
        max_new_tokens: usize,
        params: LlamaSamplingParams,
    ) -> Result<LlamaGeneration> {
        let mut state = LlamaSamplerState::new(params.seed);
        self.continue_sampled_with(max_new_tokens, params, &mut state)
    }

    /// Sample `max_new_tokens` tokens, continuing the caller's RNG stream.
    ///
    /// With speculation on this uses proper speculative rejection sampling
    /// (accept draft `x` with probability `min(1, p(x)/q(x))`, otherwise draw
    /// from the normalised residual `max(0, p - q)`), so the output
    /// distribution is exactly the one the non-speculative sampler would
    /// produce.
    ///
    /// `params.seed` is ignored here — the stream lives in `state`, which the
    /// caller seeds once per generation. Chunking a generation across several
    /// calls with the same `state` yields exactly the token sequence one big
    /// call would have produced.
    pub fn continue_sampled_with(
        &mut self,
        max_new_tokens: usize,
        params: LlamaSamplingParams,
        state: &mut LlamaSamplerState,
    ) -> Result<LlamaGeneration> {
        if params.is_greedy() {
            return self.continue_greedy(max_new_tokens);
        }
        if self.mtp.is_some() {
            return self.continue_sampled_speculative(max_new_tokens, params, state);
        }

        let mut token_ids = Vec::with_capacity(max_new_tokens);
        let mut stop_reason = LlamaStopReason::MaxNewTokens;
        for _ in 0..max_new_tokens {
            let logits = self.last_logits().ok_or_else(|| {
                LlamaError::format("session has no logits yet; append context tokens first")
            })?;
            let probs = sampling_probabilities(logits, params, state.recent())?;
            let next_token = sample_from(&probs, &mut state.rng)?;
            if let Some(reason) = self.stop_reason_for(next_token) {
                stop_reason = reason;
                break;
            }
            self.append_token(next_token)?;
            state.remember(next_token, params);
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
        state: &mut LlamaSamplerState,
    ) -> Result<LlamaGeneration> {
        let mut token_ids = Vec::with_capacity(max_new_tokens);
        let mut stop_reason = LlamaStopReason::MaxNewTokens;
        while token_ids.len() < max_new_tokens {
            let remaining = max_new_tokens - token_ids.len();
            if let Some(reason) =
                self.speculative_round_sampled(&mut token_ids, remaining, params, state)?
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
        state: &mut LlamaSamplerState,
    ) -> Result<Option<LlamaStopReason>> {
        let mtp = self
            .mtp
            .ok_or_else(|| LlamaError::format("speculative round without an MTP head"))?;
        self.flush_mtp_prefill(true)?;

        let first = {
            let logits = self.last_logits().ok_or_else(|| {
                LlamaError::format("session has no logits yet; append context tokens first")
            })?;
            let probs = sampling_probabilities(logits, params, state.recent())?;
            sample_from(&probs, &mut state.rng)?
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
        let mut draft_probs: Vec<Vec<(u32, f32)>> = Vec::with_capacity(draft_max);
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
            let (drafted, proposal) = self.draft_proposal(&logits, params, &mut state.rng)?;
            token = drafted;
            drafts.push(token);
            draft_probs.push(proposal);
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

        // The penalty window advances WITHIN the round. The target row at
        // position `i` is the distribution for the token that follows `first`
        // and `drafts[..i]`, so those tokens belong in the window that shapes
        // it — exactly as they would if the round had been i+1 separate decode
        // steps. Speculation is not allowed to change the distribution it
        // samples from, and the part of that distribution the penalty owns is
        // no exception.
        let mut window: Vec<i32> = state.recent().to_vec();
        window.push(first);

        let mut accepted = 0usize;
        let mut bonus = None;
        for index in 0..drafts.len() {
            let target = sampling_probabilities(
                &run.logits[index * vocab_size..(index + 1) * vocab_size],
                params,
                &window,
            )?;
            let drafted = drafts[index] as usize;
            let q = draft_probs[index]
                .iter()
                .find(|(token, _)| *token as usize == drafted)
                .map(|(_, probability)| *probability)
                .unwrap_or(0.0);
            let p = target[drafted];
            let ratio = speculative_acceptance(p, q);
            if state.rng.next_f32() < ratio {
                accepted += 1;
                window.push(drafts[index]);
                continue;
            }
            // Rejected: draw the replacement from the normalised residual so
            // the overall distribution stays exactly `p`.
            let residual = speculative_residual(&target, &draft_probs[index]);
            bonus = Some(sample_from(&residual, &mut state.rng)?);
            break;
        }
        let bonus = match bonus {
            Some(token) => token,
            None => {
                let target = sampling_probabilities(
                    &run.logits[accepted * vocab_size..(accepted + 1) * vocab_size],
                    params,
                    &window,
                )?;
                sample_from(&target, &mut state.rng)?
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
        // Only what the round KEPT enters the window. Drafts that were
        // rejected, and the bonus token that has not been committed yet, were
        // never part of the reply.
        for &token in committed {
            state.remember(token, params);
        }
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
        let drafted = argmax_token_id(&logits)?;
        self.draft_token(drafted)
    }

    /// Map a draft head output index to a real token id. With a restricted
    /// head the two differ; without one they are the same number.
    fn draft_token(&self, draft_id: i32) -> Result<i32> {
        match self.draft_vocab.as_ref() {
            Some(vocab) => {
                let index = usize::try_from(draft_id).map_err(|_| {
                    LlamaError::format("draft head index does not fit in usize")
                })?;
                vocab.real_token(index)
            }
            None => Ok(draft_id),
        }
    }

    /// The draft proposal as a sparse distribution over REAL token ids. Only
    /// the nucleus is non-zero, so this stays small whether or not the head is
    /// restricted, and the rejection-sampling residual can subtract it from a
    /// full-vocabulary target without materialising a second full vector.
    /// The draft's `q` is deliberately NOT penalised, and that is a
    /// correctness statement rather than an omission. Rejection sampling emits
    /// exactly the target `p` for ANY proposal distribution `q` — `q` only
    /// decides how often a draft survives. Penalising here would mean mapping
    /// the recent REAL token ids back through a restricted draft vocabulary,
    /// which is a second place for that mapping to be wrong, in exchange for
    /// nothing the output distribution can see. What it costs is acceptance,
    /// and only while a penalty is actively biting: the drafter proposes the
    /// token the loop wants, the penalised target rejects it, and the round
    /// commits the replacement. That is the loop being broken, priced
    /// correctly.
    fn draft_proposal(
        &self,
        logits: &[f32],
        params: LlamaSamplingParams,
        rng: &mut Xorshift64,
    ) -> Result<(i32, Vec<(u32, f32)>)> {
        let probs = sampling_probabilities(logits, params, &[])?;
        let drafted = sample_from(&probs, rng)?;
        let mut sparse = Vec::new();
        for (index, &probability) in probs.iter().enumerate() {
            if probability > 0.0 {
                let token = self.draft_token(index as i32)?;
                sparse.push((token as u32, probability));
            }
        }
        Ok((self.draft_token(drafted)?, sparse))
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
        let batch_size = tokens.len();
        if batch_size == 0 {
            return Err(LlamaError::format("a verify batch needs at least one token"));
        }
        self.ensure_capacity(batch_size)?;
        // ONE verify implementation, driven here as a single lane.
        //
        // The single-stream path used to build its own layout beside the
        // batched one. Two copies of a layout this fiddly — positions, write
        // rows, checkpoint rows, hidden rows — is two chances to disagree, and
        // the one that matters most is the one nobody can test on a Mac:
        // recurrent checkpoints do not exist on Metal, so a divergence here
        // only ever shows up on a box.
        //
        // Driven as a lane at base 0, this reduces to what it built by hand:
        // `kv_base` and `state_base` are 0 so positions equal write rows, the
        // lower-bound vector stays empty and takes the mask builder's
        // single-sequence path, `checkpointed_state_rows` yields
        // `[resume, 0..n-1]`, and the hidden rows are lane 0's ring. Same
        // graph key, same layout, same bytes.
        let tokens_owned = self.token_ids.clone();
        let lane = SpecLane {
            lane: 0,
            kv_base: 0,
            state_base: 0,
            live_state_offset: usize::try_from(mtp.state_row)
                .map_err(|_| LlamaError::format("committed checkpoint row is negative"))?,
            fill: self.token_ids.len(),
            mtp_filled: mtp.mtp_filled,
            tokens: &tokens_owned,
            first: tokens[0],
        };
        self.run_slot_verify_batch(&[lane], tokens, batch_size)
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

/// One lane's facts for a batched speculative round.
///
/// Every one of them arrives as an argument, exactly as it does for
/// [`LlamaSession::prefill_slot_chunk`] and [`LlamaSession::step_slots`]. The
/// session keeps its single-sequence state for the solo path and learns
/// nothing about lanes; the durable per-lane facts live where `fill` and the
/// cache bases already live, in the slot table.
///
/// That split is the point. A lane's history, its draft-head fill and its
/// resume row are exactly the things two conversations must not share, and the
/// four bugs this lane has already caught were all one owner short.
#[derive(Clone, Copy, Debug)]
pub struct SpecLane<'a> {
    /// Slot index. Addresses this lane's hidden-carry block, so it must be the
    /// slot's own index and not its position in the batch.
    pub lane: usize,
    /// First absolute attention-cache row the lane owns.
    pub kv_base: usize,
    /// First recurrent-state row the lane owns.
    pub state_base: usize,
    /// Offset INSIDE the lane's recurrent block that the scan resumes from —
    /// the checkpoint its last round committed at.
    pub live_state_offset: usize,
    /// Tokens the lane's attention KV already holds; the within-lane position
    /// its next token takes.
    pub fill: usize,
    /// Tokens the DRAFT head's KV holds for this lane. Lags `fill` until the
    /// catch-up runs, and is per lane for the same reason the ring is.
    pub mtp_filled: usize,
    /// The lane's token history, `fill` long. Only `mtp_filled..fill` is read,
    /// but the slice is the lane's own and is never another lane's.
    pub tokens: &'a [i32],
    /// The token this round starts from: sampled from the lane's own last
    /// logits, by the lane's own stream.
    pub first: i32,
}

/// What a batched speculative round produced for one lane.
#[derive(Clone, Debug, Default)]
pub struct SpecRoundOutcome {
    /// Tokens committed this round, in order. At least one.
    pub committed: Vec<i32>,
    /// The token the lane starts its NEXT round from. Already drawn from this
    /// round's logits by this lane's stream, so nothing has to be re-sampled.
    pub next: i32,
    /// Offset inside the lane's recurrent block the next round resumes from.
    pub live_state_offset: usize,
    /// Draft-head KV fill after the round.
    pub mtp_filled: usize,
    /// Draft tokens proposed and accepted, for the acceptance EMA the
    /// allocator spends columns on.
    pub drafted: usize,
    pub accepted: usize,
    /// Set when a committed token, or the bonus, is a stop token.
    pub stop_reason: Option<LlamaStopReason>,
}

impl LlamaSession {
    /// One speculative round for every lane in `lanes`, at a UNIFORM draft
    /// depth, in one verify batch.
    ///
    /// Uniform because the recurrent scan reshapes the batch as
    /// `[w, n_tokens / n_seqs, n_seqs]`, so every lane in one batch must
    /// contribute the same token count. Per-lane depths would need padded
    /// columns, and a padded column costs exactly the column it is trying to
    /// save — so the allocator picks ONE depth for the step rather than one per
    /// lane. That is a real narrowing of the design's allocator and it is
    /// recorded as such, not slipped in.
    ///
    /// `params[i]` and `samplers[i]` belong to `lanes[i]`. Both are the
    /// caller's: sampling settings ride the request, and an RNG stream that
    /// two lanes shared would make one chat's output depend on when the other
    /// one drew.
    ///
    /// `attention_key_count` is the arena span the graph must cover, from the
    /// slot table — one past the highest occupied absolute row.
    pub fn speculative_round_slots(
        &mut self,
        lanes: &[SpecLane<'_>],
        depth: usize,
        params: &[LlamaSamplingParams],
        samplers: &mut [LlamaSamplerState],
    ) -> Result<Vec<SpecRoundOutcome>> {
        let mtp = self
            .mtp
            .ok_or_else(|| LlamaError::format("batched speculative round without an MTP head"))?;
        let width = lanes.len();
        if width == 0 {
            return Err(LlamaError::format(
                "a batched speculative round needs at least one lane",
            ));
        }
        if params.len() != width || samplers.len() != width {
            return Err(LlamaError::format(format!(
                "batched speculative round has {} lanes but {} sampling params and {} streams",
                width,
                params.len(),
                samplers.len()
            )));
        }
        if depth > mtp.draft_max {
            return Err(LlamaError::format(format!(
                "batched speculative round asked for depth {} past the session's {}",
                depth, mtp.draft_max
            )));
        }
        for lane in lanes {
            if lane.tokens.len() != lane.fill {
                return Err(LlamaError::format(format!(
                    "lane {} holds {} tokens but its cache fill is {}",
                    lane.lane,
                    lane.tokens.len(),
                    lane.fill
                )));
            }
            if lane.mtp_filled > lane.fill {
                return Err(LlamaError::format(format!(
                    "lane {} has a draft head ahead of the model: {} > {}",
                    lane.lane, lane.mtp_filled, lane.fill
                )));
            }
            if lane.live_state_offset >= self.state_rows_per_slot() {
                return Err(LlamaError::format(format!(
                    "lane {} resumes from offset {} past its {}-row block",
                    lane.lane,
                    lane.live_state_offset,
                    self.state_rows_per_slot()
                )));
            }
            if lane.fill + depth + 1 > self.max_context {
                return Err(LlamaError::format(format!(
                    "lane {} would run past its {}-token context",
                    lane.lane, self.max_context
                )));
            }
        }

        // The draft head lags the model until this runs, and it lags PER LANE.
        let mut mtp_filled: Vec<usize> = Vec::with_capacity(width);
        for lane in lanes {
            mtp_filled.push(self.catch_up_slot_draft_head(lane)?);
        }

        // Draft: `depth` forwards, each one column per lane, so the draft head
        // reads its ~46 MB restricted LM head once per DEPTH rather than once
        // per lane per depth.
        let mut drafts: Vec<Vec<i32>> = vec![Vec::with_capacity(depth); width];
        let mut draft_probs: Vec<Vec<Vec<(u32, f32)>>> = vec![Vec::with_capacity(depth); width];
        let mut fed: Vec<i32> = lanes.iter().map(|lane| lane.first).collect();
        for step in 0..depth {
            let read_rows: Vec<i32> = lanes
                .iter()
                .map(|lane| {
                    if step > 0 {
                        // The draft chain's own hidden, from this lane's
                        // scratch row. Its own, or one lane would draft on
                        // another's hidden state.
                        self.carry_scratch_row_for(&mtp, lane.lane)
                    } else if lane.fill == 0 {
                        self.carry_zero_row_for(&mtp, lane.lane)
                    } else {
                        self.carry_lane_base(&mtp, lane.lane)
                            + ((lane.fill - 1) % mtp.carry_ring) as i32
                    }
                })
                .collect();
            let started = std::time::Instant::now();
            let rows = self.run_slot_draft_step(lanes, &fed, step, &read_rows)?;
            if let Some(mtp) = self.mtp.as_mut() {
                mtp.draft_nanos += started.elapsed().as_nanos() as u64;
            }
            for (index, row) in rows.iter().enumerate() {
                let (drafted, proposal) =
                    self.draft_proposal(row, params[index], &mut samplers[index].rng)?;
                fed[index] = drafted;
                drafts[index].push(drafted);
                draft_probs[index].push(proposal);
            }
        }

        // Verify: ONE batch across every lane. Lane `i`'s tokens are
        // `batch[i * per_lane .. (i+1) * per_lane]` — sequence-major, which is
        // the order the recurrent scan reshapes.
        let per_lane = depth + 1;
        let mut batch: Vec<i32> = Vec::with_capacity(width * per_lane);
        for (index, lane) in lanes.iter().enumerate() {
            batch.push(lane.first);
            batch.extend_from_slice(&drafts[index]);
        }
        let started = std::time::Instant::now();
        let run = self.run_slot_verify_batch(lanes, &batch, per_lane)?;
        if let Some(mtp) = self.mtp.as_mut() {
            mtp.verify_nanos += started.elapsed().as_nanos() as u64;
        }
        let rows = split_run_logits(run, width * per_lane)?;

        let mut outcomes = Vec::with_capacity(width);
        for (index, lane) in lanes.iter().enumerate() {
            let rows = &rows[index * per_lane..(index + 1) * per_lane];
            outcomes.push(self.commit_slot_round(
                lane,
                &batch[index * per_lane..(index + 1) * per_lane],
                &drafts[index],
                &draft_probs[index],
                rows,
                params[index],
                &mut samplers[index],
            )?);
        }
        if let Some(mtp) = self.mtp.as_mut() {
            mtp.rounds += 1;
            for outcome in &outcomes {
                mtp.drafted += outcome.drafted as u64;
                mtp.accepted += outcome.accepted as u64;
            }
        }
        Ok(outcomes)
    }

    /// Recurrent rows one slot owns: the live row plus one checkpoint per
    /// verify-batch position when speculating, else 1.
    fn state_rows_per_slot(&self) -> usize {
        match self.mtp {
            Some(mtp) if mtp.draft_max > 0 => mtp.draft_max + 2,
            _ => 1,
        }
    }

    /// Accept or reject one lane's drafts and decide what it committed.
    ///
    /// Byte-for-byte the accept test the single-stream round runs; the only
    /// difference is that every piece of state it reads is this lane's.
    #[allow(clippy::too_many_arguments)]
    fn commit_slot_round(
        &self,
        lane: &SpecLane<'_>,
        batch: &[i32],
        drafts: &[i32],
        draft_probs: &[Vec<(u32, f32)>],
        rows: &[Vec<f32>],
        params: LlamaSamplingParams,
        sampler: &mut LlamaSamplerState,
    ) -> Result<SpecRoundOutcome> {
        // This lane's window, advanced within the round exactly as the
        // single-stream path advances its own — `batch[0]` is the token the
        // round starts from, and each accepted draft joins the window before
        // the next target row is shaped.
        let mut window: Vec<i32> = sampler.recent().to_vec();
        if let Some(&first) = batch.first() {
            window.push(first);
        }
        let mut accepted = 0usize;
        let mut bonus = None;
        for index in 0..drafts.len() {
            let target = sampling_probabilities(&rows[index], params, &window)?;
            let drafted = drafts[index] as usize;
            let q = draft_probs[index]
                .iter()
                .find(|(token, _)| *token as usize == drafted)
                .map(|(_, probability)| *probability)
                .unwrap_or(0.0);
            let p = target.get(drafted).copied().unwrap_or(0.0);
            if sampler.rng.next_f32() < speculative_acceptance(p, q) {
                accepted += 1;
                window.push(drafts[index]);
                continue;
            }
            let residual = speculative_residual(&target, &draft_probs[index]);
            bonus = Some(sample_from(&residual, &mut sampler.rng)?);
            break;
        }
        let bonus = match bonus {
            Some(token) => token,
            None => {
                let target = sampling_probabilities(&rows[accepted], params, &window)?;
                sample_from(&target, &mut sampler.rng)?
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
        if stop_reason.is_none() {
            stop_reason = self.stop_reason_for(bonus);
        }

        let committed = batch[..=commit].to_vec();
        // Only what this lane KEPT enters its window; a rejected draft was
        // never part of this conversation's reply.
        for &token in &committed {
            sampler.remember(token, params);
        }
        let start = lane.fill;
        // The draft head ingested exactly the positions it DRAFTED from:
        // `start .. start + drafts.len()`. Without KV reuse only the first of
        // those is trustworthy — it is `first`, which is always committed —
        // and beyond it the draft chain's own tokens may have been rejected.
        //
        // Bounded by `drafts.len()` rather than assumed to be one. At depth 0
        // the draft head ingested NOTHING, and claiming a position it never
        // saw would leave a hole its next catch-up skips over: the draft head
        // would then be conditioned on a token it never read, and the only
        // symptom is worse proposals.
        let ingested = draft_head_fill_after(drafts.len(), committed.len(), reuse_draft_kv());
        Ok(SpecRoundOutcome {
            live_state_offset: commit,
            mtp_filled: start + ingested,
            drafted: drafts.len(),
            accepted,
            committed,
            next: bonus,
            stop_reason,
        })
    }

    /// Bring one lane's draft-head KV up to its model KV, in chunks.
    ///
    /// Returns the lane's new draft fill. Per lane and never shared: the draft
    /// head's cache lives in the same arena as the model's, at the same
    /// `kv_base`, so a lane catching up against another lane's fill would
    /// ingest its neighbour's tokens.
    fn catch_up_slot_draft_head(&mut self, lane: &SpecLane<'_>) -> Result<usize> {
        let Some(mtp) = self.mtp else {
            return Ok(lane.fill);
        };
        let mut filled = lane.mtp_filled;
        if filled >= lane.fill {
            return Ok(filled);
        }
        let started = std::time::Instant::now();
        while filled < lane.fill {
            let chunk = MTP_PREFILL_CHUNK.min(mtp.carry_ring.saturating_sub(1)).max(1);
            let end = (filled + chunk).min(lane.fill);
            self.run_slot_draft_prefill_chunk(lane, filled, end)?;
            filled = end;
        }
        if let Some(mtp) = self.mtp.as_mut() {
            mtp.catchup_nanos += started.elapsed().as_nanos() as u64;
        }
        Ok(filled)
    }

    /// One draft-head catch-up chunk for a lane: its tokens `start..end`.
    fn run_slot_draft_prefill_chunk(
        &mut self,
        lane: &SpecLane<'_>,
        start: usize,
        end: usize,
    ) -> Result<()> {
        let mtp = self
            .mtp
            .ok_or_else(|| LlamaError::format("draft catch-up without an MTP head"))?;
        let batch = end - start;
        if batch == 0 {
            return Ok(());
        }
        let positions: Vec<i32> = (start..end)
            .map(|position| {
                i32::try_from(position)
                    .map_err(|_| LlamaError::format("draft catch-up position does not fit in i32"))
            })
            .collect::<Result<Vec<_>>>()?;
        let write_indices: Vec<i32> = (start..end)
            .map(|position| {
                i32::try_from(lane.kv_base + position).map_err(|_| {
                    LlamaError::format("draft catch-up cache row does not fit in i32")
                })
            })
            .collect::<Result<Vec<_>>>()?;
        // Row `i` consumes the model's hidden at `i - 1`; the lane's first
        // token has none and reads its own never-written zero row.
        let zero_row = self.carry_zero_row_for(&mtp, lane.lane);
        let base = self.carry_lane_base(&mtp, lane.lane);
        let read_rows: Vec<i32> = (start..end)
            .map(|index| {
                if index == 0 {
                    zero_row
                } else {
                    base + ((index - 1) % mtp.carry_ring) as i32
                }
            })
            .collect();
        let scratch_row = self.carry_scratch_row_for(&mtp, lane.lane);
        let key_count = self.slot_key_count(lane.kv_base + end)?;
        let graph_params = SessionGraphParams::mtp_draft(batch, key_count);
        self.ensure_compiled_graph(graph_params)?;
        let lower = i32::try_from(lane.kv_base)
            .map_err(|_| LlamaError::format("lane kv_base does not fit in i32"))?;
        let compiled = self
            .graphs
            .graph_for_mut(graph_params)
            .ok_or_else(|| LlamaError::format("compiled draft catch-up graph was not cached"))?;
        let mut layout = HybridDecodeBatchLayout::from_contiguous_positions_and_outputs(
            &positions,
            key_count,
            &[(batch - 1) as i32],
        )?;
        layout.attention_write_indices = write_indices;
        if lower != 0 {
            layout.attention_key_lower_bounds = vec![lower; batch];
        }
        layout.recurrent_state_rows.clear();
        layout.hidden_read_rows = read_rows;
        layout.hidden_write_rows = vec![scratch_row; batch];
        compiled.execute_logits_only_with_layout(
            LogitsProbeInput::TokenIds(&lane.tokens[start..end]),
            &layout,
        )?;
        Ok(())
    }

    /// One draft step across every lane: `lanes.len()` columns, one logit row
    /// each.
    ///
    /// `fed[i]` is the token lane `i` drafts from, `step` is how far into the
    /// draft chain it is, and `read_rows[i]` is the hidden row it consumes.
    fn run_slot_draft_step(
        &mut self,
        lanes: &[SpecLane<'_>],
        fed: &[i32],
        step: usize,
        read_rows: &[i32],
    ) -> Result<Vec<Vec<f32>>> {
        let mtp = self
            .mtp
            .ok_or_else(|| LlamaError::format("draft step without an MTP head"))?;
        let width = lanes.len();
        let positions: Vec<i32> = lanes
            .iter()
            .map(|lane| {
                i32::try_from(lane.fill + step)
                    .map_err(|_| LlamaError::format("draft position does not fit in i32"))
            })
            .collect::<Result<Vec<_>>>()?;
        let write_indices: Vec<i32> = lanes
            .iter()
            .map(|lane| {
                i32::try_from(lane.kv_base + lane.fill + step)
                    .map_err(|_| LlamaError::format("draft cache row does not fit in i32"))
            })
            .collect::<Result<Vec<_>>>()?;
        let span = lanes
            .iter()
            .map(|lane| lane.kv_base + lane.fill + step + 1)
            .max()
            .unwrap_or(1);
        let key_count = self.slot_key_count(span)?;
        let graph_params = SessionGraphParams::mtp_draft_batched(width, key_count);
        self.ensure_compiled_graph(graph_params)?;
        let scratch_rows: Vec<i32> = lanes
            .iter()
            .map(|lane| self.carry_scratch_row_for(&mtp, lane.lane))
            .collect();
        let lower_bounds = slot_lower_bounds(lanes)?;
        let output_ids: Vec<i32> = (0..width)
            .map(|index| {
                i32::try_from(index)
                    .map_err(|_| LlamaError::format("draft output id does not fit in i32"))
            })
            .collect::<Result<Vec<_>>>()?;
        let compiled = self
            .graphs
            .graph_for_mut(graph_params)
            .ok_or_else(|| LlamaError::format("compiled draft graph was not cached"))?;
        let mut layout = HybridDecodeBatchLayout::from_contiguous_positions_and_outputs(
            &positions,
            key_count,
            &output_ids,
        )?;
        layout.attention_write_indices = write_indices;
        layout.attention_key_lower_bounds = lower_bounds;
        layout.recurrent_state_rows.clear();
        layout.hidden_read_rows = read_rows.to_vec();
        layout.hidden_write_rows = scratch_rows;
        let run =
            compiled.execute_logits_only_with_layout(LogitsProbeInput::TokenIds(fed), &layout)?;
        split_run_logits(run, width)
    }

    /// The verify forward across every lane: `per_lane` positions each, logits
    /// for all of them.
    ///
    /// `batch` is sequence-major — lane `i`'s tokens at
    /// `i * per_lane .. (i+1) * per_lane` — because that is how the recurrent
    /// scan reshapes a batch. The state-row vector is NOT in that order; see
    /// [`checkpointed_state_rows`].
    fn run_slot_verify_batch(
        &mut self,
        lanes: &[SpecLane<'_>],
        batch: &[i32],
        per_lane: usize,
    ) -> Result<HybridDecodeRun> {
        // Guard only: a verify graph exists only alongside a draft head, and
        // the hidden rows below are the draft head's to read.
        self.mtp
            .ok_or_else(|| LlamaError::format("verify batch without an MTP head"))?;
        let width = lanes.len();
        let n_tokens = width * per_lane;
        if batch.len() != n_tokens {
            return Err(LlamaError::format(format!(
                "verify batch has {} tokens for {} lanes of {}",
                batch.len(),
                width,
                per_lane
            )));
        }
        let mut positions = Vec::with_capacity(n_tokens);
        let mut write_indices = Vec::with_capacity(n_tokens);
        let mut hidden_write_rows = Vec::with_capacity(n_tokens);
        for lane in lanes {
            for offset in 0..per_lane {
                positions.push(i32::try_from(lane.fill + offset).map_err(|_| {
                    LlamaError::format("verify position does not fit in i32")
                })?);
                write_indices.push(
                    i32::try_from(lane.kv_base + lane.fill + offset).map_err(|_| {
                        LlamaError::format("verify cache row does not fit in i32")
                    })?,
                );
            }
            hidden_write_rows.extend(self.carry_rows_for_lane_positions(
                lane.lane,
                lane.fill,
                per_lane,
            ));
        }
        let resume: Vec<i32> = lanes
            .iter()
            .map(|lane| {
                i32::try_from(lane.state_base + lane.live_state_offset)
                    .map_err(|_| LlamaError::format("lane resume row does not fit in i32"))
            })
            .collect::<Result<Vec<_>>>()?;
        let bases: Vec<i32> = lanes
            .iter()
            .map(|lane| {
                i32::try_from(lane.state_base)
                    .map_err(|_| LlamaError::format("lane state base does not fit in i32"))
            })
            .collect::<Result<Vec<_>>>()?;
        let state_rows = crate::runtime::checkpointed_state_rows(&resume, &bases, per_lane)?;
        // Two different key counts, and they are not interchangeable.
        //
        // The GRAPH is compiled at a bucketed width so a long generation does
        // not compile a new graph per token. The LAYOUT carries the EXACT
        // occupancy, because that is what the mask builder is told: keys past
        // it are -inf'd at full graph width. Handing the layout the bucketed
        // number instead would be describing cache rows that are not there.
        // The single-stream path has always passed the exact count here, and
        // this is that behaviour kept rather than re-derived.
        let span = usize::try_from(write_indices.iter().copied().max().unwrap_or(0) + 1)
            .map_err(|_| LlamaError::format("verify span does not fit in usize"))?;
        let key_count = self.slot_key_count(span)?;
        let graph_params = SessionGraphParams::verify_batched(width, n_tokens, key_count);
        self.ensure_compiled_graph(graph_params)?;
        let output_ids: Vec<i32> = (0..n_tokens)
            .map(|index| {
                i32::try_from(index)
                    .map_err(|_| LlamaError::format("verify output id does not fit in i32"))
            })
            .collect::<Result<Vec<_>>>()?;
        let lower_bounds = slot_lower_bounds_per_token(lanes, per_lane)?;
        let compiled = self
            .graphs
            .graph_for_mut(graph_params)
            .ok_or_else(|| LlamaError::format("compiled verify graph was not cached"))?;
        let mut layout = HybridDecodeBatchLayout::from_contiguous_positions_and_outputs(
            &positions,
            span,
            &output_ids,
        )?;
        layout.attention_write_indices = write_indices;
        layout.attention_key_lower_bounds = lower_bounds;
        layout.recurrent_state_rows = state_rows;
        layout.hidden_write_rows = hidden_write_rows;
        compiled.execute_logits_only_with_layout(LogitsProbeInput::TokenIds(batch), &layout)
    }

    /// Bucket an absolute arena span into a compiled graph's key count.
    ///
    /// Against the ARENA, not one slot's context: the span is an absolute row
    /// count across slots. At one slot the arena IS the context, so a solo
    /// session buckets exactly as it does today.
    fn slot_key_count(&self, span: usize) -> Result<usize> {
        if span == 0 {
            return Err(LlamaError::format("a batch must span at least one key"));
        }
        if span > self.attention_arena_rows {
            return Err(LlamaError::format(format!(
                "batch spans {} cache rows past the {}-row arena",
                span, self.attention_arena_rows
            )));
        }
        Ok(if std::env::var_os("MAKEPAD_LLAMA_PER_LEN_GRAPHS").is_some() {
            span
        } else {
            span.next_multiple_of(GRAPH_KEY_BUCKET)
                .min(self.attention_arena_rows)
        })
    }
}

/// How far the draft head's KV is trustworthy after a round, as a count of
/// positions past the round's start.
///
/// The draft head ingested exactly the positions it DRAFTED from — `drafted`
/// of them. Without KV reuse only the first is trustworthy: that one is
/// `first`, which is always committed, while the draft chain's own tokens may
/// have been rejected.
///
/// Bounded by `drafted` and not assumed to be one. At depth 0 the draft head
/// ingested NOTHING, and claiming a position it never saw leaves a hole its
/// next catch-up skips straight over — after which the draft head is
/// conditioned on a token it never read, and the only symptom is proposals
/// quietly getting worse.
fn draft_head_fill_after(drafted: usize, committed: usize, reuse: bool) -> usize {
    drafted.min(if reuse { committed } else { 1 })
}

/// One lower bound per lane, or empty when every lane is at row 0.
///
/// Empty is not an optimisation: an all-zero vector and an absent one take
/// DIFFERENT paths through the mask builder, and only the absent one is the
/// single-sequence path a solo lane must stay on.
fn slot_lower_bounds(lanes: &[SpecLane<'_>]) -> Result<Vec<i32>> {
    if lanes.iter().all(|lane| lane.kv_base == 0) {
        return Ok(Vec::new());
    }
    lanes
        .iter()
        .map(|lane| {
            i32::try_from(lane.kv_base)
                .map_err(|_| LlamaError::format("lane kv_base does not fit in i32"))
        })
        .collect()
}

/// The same bounds, repeated for every token a lane contributes.
fn slot_lower_bounds_per_token(lanes: &[SpecLane<'_>], per_lane: usize) -> Result<Vec<i32>> {
    let bounds = slot_lower_bounds(lanes)?;
    if bounds.is_empty() {
        return Ok(Vec::new());
    }
    let mut out = Vec::with_capacity(bounds.len() * per_lane);
    for bound in bounds {
        out.extend(std::iter::repeat(bound).take(per_lane));
    }
    Ok(out)
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
    draft_vocab: Option<&DraftVocab>,
    carry_ring: usize,
    // Lanes the carry tensor must hold blocks for. 1 for a single-sequence
    // session, which reproduces the previous allocation exactly.
    carry_lanes: usize,
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
                        i64::try_from(carry_rows_total(carry_ring, carry_lanes)).map_err(
                            |_| LlamaError::format("mtp carry ring does not fit in i64"),
                        )?,
                    ],
                    crate::BufferUsage::State,
                )
                .map_err(LlamaError::format)?;
            weights
                .tensor_ids
                .insert(carry_spec.tensor_name.clone(), carry);
            hidden_carry = Some(carry);
            if let Some(vocab) = draft_vocab {
                build_restricted_draft_head(&mut weights, model, vocab)?;
            }
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
                0,
                session_attention_key_count(spec)?,
                1,
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

/// Split a multi-row decode run into one logits vector per output row.
fn split_run_logits(run: HybridDecodeRun, rows: usize) -> Result<Vec<Vec<f32>>> {
    let vocab = run.vocab_size;
    if vocab == 0 {
        return Err(LlamaError::format("decode run reported a zero vocabulary"));
    }
    if run.logits.len() < rows * vocab {
        return Err(LlamaError::format(format!(
            "batched decode produced {} logits, need {} for {} slots",
            run.logits.len(),
            rows * vocab,
            rows
        )));
    }
    Ok(run
        .logits
        .chunks(vocab)
        .take(rows)
        .map(|row| row.to_vec())
        .collect())
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
        argmax_penalized, argmax_token_id, draft_head_fill_after, mtp_carry_ring, penalty_window,
        sample_from, sampling_probabilities, slot_lower_bounds, slot_lower_bounds_per_token,
        speculative_acceptance, speculative_residual, LlamaSamplerState, LlamaSamplingParams,
        SpecLane, Xorshift64,
    };

    fn params(temperature: f32, top_p: f32, top_k: usize) -> LlamaSamplingParams {
        LlamaSamplingParams {
            temperature,
            top_p,
            top_k,
            seed: 7,
            ..Default::default()
        }
    }

    #[test]
    fn sampling_probabilities_normalise_and_respect_top_k() {
        let logits = [1.0f32, 2.0, 3.0, 4.0];
        let probs = sampling_probabilities(&logits, params(1.0, 1.0, 2), &[]).unwrap();
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
        let probs = sampling_probabilities(&logits, params(1.0, 0.9, 0), &[]).unwrap();
        assert_eq!(probs[1], 1.0);
        assert!(probs.iter().enumerate().all(|(i, p)| i == 1 || *p == 0.0));
    }

    #[test]
    fn low_temperature_collapses_onto_the_argmax() {
        let logits = [0.0f32, 1.0, 0.5];
        let probs = sampling_probabilities(&logits, params(0.01, 1.0, 0), &[]).unwrap();
        assert!(probs[1] > 0.999, "{probs:?}");
    }

    /// The penalties are OFF unless asked for, and "off" has to mean
    /// bit-identical, not merely similar: every measurement, every gate and
    /// every byte-exactness comparison on this fleet predates them.
    #[test]
    fn an_unconfigured_penalty_changes_nothing() {
        let logits = [1.0f32, 2.0, 3.0, 4.0];
        let plain = sampling_probabilities(&logits, params(1.0, 1.0, 0), &[]).unwrap();
        let with_history =
            sampling_probabilities(&logits, params(1.0, 1.0, 0), &[3, 3, 3, 2]).unwrap();
        assert_eq!(plain, with_history, "no window configured, so no penalty");

        // Configured strength but a zero window is still off, and so is a
        // window with zero strength. Both halves have to be present.
        let no_window = LlamaSamplingParams {
            presence_penalty: 2.0,
            frequency_penalty: 2.0,
            penalty_last_n: 0,
            ..params(1.0, 1.0, 0)
        };
        assert_eq!(
            sampling_probabilities(&logits, no_window, &[3, 3, 3]).unwrap(),
            plain
        );
        let no_strength = LlamaSamplingParams {
            presence_penalty: 0.0,
            frequency_penalty: 0.0,
            penalty_last_n: 64,
            ..params(1.0, 1.0, 0)
        };
        assert_eq!(
            sampling_probabilities(&logits, no_strength, &[3, 3, 3]).unwrap(),
            plain
        );
    }

    /// The whole point: a token the generation keeps returning to loses mass,
    /// and the one it has not used gains it.
    #[test]
    fn a_repeated_token_loses_its_grip() {
        let logits = [0.0f32, 0.0, 0.0, 3.0];
        let penalised = LlamaSamplingParams {
            presence_penalty: 1.0,
            frequency_penalty: 0.5,
            penalty_last_n: 64,
            ..params(1.0, 1.0, 0)
        };
        let plain = sampling_probabilities(&logits, penalised, &[]).unwrap();
        assert!(plain[3] > 0.85, "token 3 dominates before any penalty: {plain:?}");

        // Token 3 five times over: presence once, frequency five times.
        let looped = [3i32, 3, 3, 3, 3];
        let after = sampling_probabilities(&logits, penalised, &looped).unwrap();
        assert!(after[3] < plain[3], "the loop's token must lose mass");
        assert!(after[0] > plain[0], "the mass has to go somewhere");
        // 3.0 - (1.0 + 0.5 * 5) = -0.5, now BELOW the untouched entries, so
        // the loop is not merely discouraged, it is outvoted.
        assert!(after[0] > after[3], "{after:?}");
        assert!((after.iter().sum::<f32>() - 1.0).abs() < 1e-5);
    }

    /// Frequency is per occurrence and presence is flat, which is the whole
    /// reason to carry both: one of them notices a token used twenty times.
    #[test]
    fn frequency_counts_and_presence_does_not() {
        let logits = [0.0f32, 5.0];
        let presence_only = LlamaSamplingParams {
            presence_penalty: 1.0,
            frequency_penalty: 0.0,
            penalty_last_n: 64,
            ..params(1.0, 1.0, 0)
        };
        let once = sampling_probabilities(&logits, presence_only, &[1]).unwrap();
        let twenty = sampling_probabilities(&logits, presence_only, &[1; 20]).unwrap();
        assert_eq!(once, twenty, "presence is flat by definition");

        let frequency_only = LlamaSamplingParams {
            presence_penalty: 0.0,
            frequency_penalty: 0.25,
            penalty_last_n: 64,
            ..params(1.0, 1.0, 0)
        };
        let once = sampling_probabilities(&logits, frequency_only, &[1]).unwrap();
        let twenty = sampling_probabilities(&logits, frequency_only, &[1; 20]).unwrap();
        assert!(twenty[1] < once[1], "frequency has to keep counting");
    }

    /// The window is the last N tokens, not the whole reply. A model that
    /// used a word once at the top of a long answer must be free to use it
    /// again at the bottom — otherwise the penalty stops being a loop-breaker
    /// and starts being a vocabulary ban.
    #[test]
    fn the_window_forgets() {
        // Token 1 is the one under test; token 2 is filler, so the filler's
        // own penalty cannot be mistaken for token 1's.
        let logits = [0.0f32, 5.0, 0.0];
        let short_window = LlamaSamplingParams {
            presence_penalty: 6.0,
            frequency_penalty: 0.0,
            penalty_last_n: 3,
            ..params(1.0, 1.0, 0)
        };
        let plain = sampling_probabilities(&logits, short_window, &[]).unwrap();

        // Token 1 is old news: three newer tokens have pushed it out.
        let stale = [1i32, 2, 2, 2];
        let after = sampling_probabilities(&logits, short_window, &stale).unwrap();
        assert!(
            after[1] >= plain[1],
            "token 1 left the window and must not be penalised: {after:?}"
        );

        // Inside the window it is penalised, so the test is not vacuous.
        let fresh = [2i32, 2, 1];
        let inside = sampling_probabilities(&logits, short_window, &fresh).unwrap();
        assert!(inside[1] < plain[1] * 0.5, "{inside:?}");
    }

    /// The rescale IS the penalty, to floating-point tolerance.
    ///
    /// `sampling_probabilities` never builds a penalised logit row — it folds
    /// the penalty in as `exp(-p / T)` on the handful of affected entries,
    /// because copying a 248320-entry row per sampled token per lane is a cost
    /// this path cannot absorb. That identity is the one thing holding the
    /// optimisation up, so it is asserted against the obvious implementation
    /// rather than trusted.
    #[test]
    fn the_rescale_equals_penalising_the_row() {
        let logits = [0.5f32, 3.0, -1.0, 2.25, 0.0, 4.5];
        let recent = [1i32, 3, 1, 5, 1];
        for temperature in [0.2f32, 0.7, 1.0, 1.8] {
            let penalised = LlamaSamplingParams {
                presence_penalty: 0.75,
                frequency_penalty: 0.4,
                penalty_last_n: 8,
                ..params(temperature, 1.0, 0)
            };
            // The obvious implementation: subtract from the row, then sample.
            let mut by_hand = logits;
            for (index, penalty) in penalty_window(penalised, &recent).unwrap() {
                by_hand[index] -= penalty;
            }
            let expected =
                sampling_probabilities(&by_hand, params(temperature, 1.0, 0), &[]).unwrap();
            let actual = sampling_probabilities(&logits, penalised, &recent).unwrap();
            for (index, (a, b)) in actual.iter().zip(expected.iter()).enumerate() {
                assert!(
                    (a - b).abs() < 1e-5,
                    "T={temperature} entry {index}: rescale {a} vs penalised row {b}"
                );
            }
        }
    }

    /// Greedy takes the same answer the penalised row would have given,
    /// including the case that matters: the raw argmax is a token the loop has
    /// been repeating, so the winner has to CHANGE.
    #[test]
    fn greedy_argmax_agrees_with_the_penalised_row() {
        let logits = [0.0f32, 1.0, 5.0, 4.0];
        let penalties = penalty_window(
            LlamaSamplingParams {
                presence_penalty: 1.0,
                frequency_penalty: 0.5,
                penalty_last_n: 8,
                ..params(1.0, 1.0, 0)
            },
            &[2i32, 2, 2],
        )
        .unwrap();
        // Token 2 is the raw argmax at 5.0 but has been used three times:
        // 5.0 - (1.0 + 1.5) = 2.5, which loses to token 3's untouched 4.0.
        assert_eq!(argmax_token_id(&logits).unwrap(), 2);
        assert_eq!(argmax_penalized(&logits, &penalties).unwrap(), 3);

        let mut by_hand = logits;
        for (index, penalty) in &penalties {
            by_hand[*index] -= penalty;
        }
        assert_eq!(
            argmax_penalized(&logits, &penalties).unwrap(),
            argmax_token_id(&by_hand).unwrap()
        );
    }

    /// A token id that is not a vocabulary index must not panic the sampler.
    /// The window is fed from committed token ids, and a draft vocabulary or a
    /// malformed gguf is exactly the kind of thing that puts a stray value in
    /// one — an out-of-range id is a reason to ignore that entry, never a
    /// reason to take a lane down mid-conversation.
    #[test]
    fn an_out_of_range_token_in_the_window_is_ignored() {
        let logits = [0.0f32, 1.0, 2.0];
        let penalised = LlamaSamplingParams {
            presence_penalty: 1.0,
            frequency_penalty: 0.0,
            penalty_last_n: 8,
            ..params(1.0, 1.0, 0)
        };
        let plain = sampling_probabilities(&logits, penalised, &[]).unwrap();
        let junk = sampling_probabilities(&logits, penalised, &[-1, 9999, i32::MIN]).unwrap();
        assert_eq!(plain, junk);
    }

    /// The chunked-decode regression, at the level where it actually happened:
    /// a caller that decodes a reply `CHUNK` tokens at a time.
    ///
    /// Re-seeding per chunk (what `continue_sampled` does per call, and what
    /// the chat worker used to do 24 tokens at a time) makes the draw sequence
    /// PERIODIC — chunk 2 replays chunk 1 exactly. Carrying one
    /// `LlamaSamplerState` across the chunks reproduces the single-shot stream
    /// instead. Both halves are asserted: the fix must be equivalent to one
    /// big call, and the old behaviour must be visibly broken so this test
    /// fails if anyone re-seeds again.
    #[test]
    fn a_chunked_generation_carries_one_rng_stream() {
        // A distribution wide enough that a repeated uniform repeats a token.
        let probs: Vec<f32> = (0..64).map(|_| 1.0 / 64.0).collect();
        const CHUNK: usize = 24;
        const TOTAL: usize = 96;

        let single: Vec<i32> = {
            let mut state = LlamaSamplerState::new(7);
            (0..TOTAL)
                .map(|_| sample_from(&probs, &mut state.rng).unwrap())
                .collect()
        };

        let chunked_carried: Vec<i32> = {
            let mut state = LlamaSamplerState::new(7);
            let mut out = Vec::new();
            while out.len() < TOTAL {
                let want = CHUNK.min(TOTAL - out.len());
                for _ in 0..want {
                    out.push(sample_from(&probs, &mut state.rng).unwrap());
                }
            }
            out
        };
        assert_eq!(
            chunked_carried, single,
            "carrying the sampler state across chunks must equal one call"
        );

        let chunked_reseeded: Vec<i32> = {
            let mut out = Vec::new();
            while out.len() < TOTAL {
                // The bug: a fresh state per chunk, from the same seed.
                let mut state = LlamaSamplerState::new(7);
                let want = CHUNK.min(TOTAL - out.len());
                for _ in 0..want {
                    out.push(sample_from(&probs, &mut state.rng).unwrap());
                }
            }
            out
        };
        assert_eq!(
            &chunked_reseeded[..CHUNK],
            &chunked_reseeded[CHUNK..2 * CHUNK],
            "re-seeding per chunk repeats the whole chunk verbatim"
        );
        assert_ne!(
            chunked_reseeded, single,
            "re-seeding per chunk must not be mistaken for the correct stream"
        );
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
            let sparse: Vec<(u32, f32)> = draft
                .iter()
                .enumerate()
                .map(|(token, probability)| (token as u32, *probability))
                .collect();
            let residual = speculative_residual(&target, &sparse);
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

    /// A draft head that can only propose PART of the vocabulary must still
    /// leave the emitted distribution equal to the target — that is what makes
    /// the restricted draft head free of quality cost.
    #[test]
    fn a_partial_draft_vocabulary_still_preserves_the_target() {
        let target = [0.5f32, 0.3, 0.15, 0.05];
        // The draft head covers only tokens 0 and 1; 2 and 3 are unreachable
        // to it and must arrive entirely through the residual.
        for draft in [
            vec![(0u32, 0.5f32), (1, 0.5)],
            vec![(0u32, 1.0f32)],
            vec![(1u32, 0.9f32), (0, 0.1)],
        ] {
            let residual = speculative_residual(&target, &draft);
            let accept_total: f32 = draft
                .iter()
                .map(|(token, q)| q * speculative_acceptance(target[*token as usize], *q))
                .sum();
            for token in 0..4 {
                let proposed = draft
                    .iter()
                    .find(|(id, _)| *id as usize == token)
                    .map(|(_, q)| *q)
                    .unwrap_or(0.0);
                let emitted = proposed * speculative_acceptance(target[token], proposed)
                    + (1.0 - accept_total) * residual[token];
                assert!(
                    (emitted - target[token]).abs() < 1e-5,
                    "draft {draft:?} token {token}: emitted {emitted} vs {}",
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
    /// The lane facts a batched speculative round is driven by, at base 0 —
    /// which is what the single-stream verify now delegates as.
    fn solo_lane(tokens: &[i32], resume: usize) -> SpecLane<'_> {
        SpecLane {
            lane: 0,
            kv_base: 0,
            state_base: 0,
            live_state_offset: resume,
            fill: tokens.len(),
            mtp_filled: tokens.len(),
            tokens,
            first: 1,
        }
    }

    #[test]
    fn a_solo_lane_asks_the_mask_builder_for_its_single_sequence_path() {
        // Empty is not an optimisation. An all-zero lower-bound vector and an
        // ABSENT one take different paths through the mask builder, and only
        // the absent one is the path a single-stream decode has always taken.
        // A solo lane that started emitting zeros would still be correct and
        // would stop being byte-identical, which is the one promise this lane
        // makes about solo.
        let tokens = [7, 8, 9];
        let lane = solo_lane(&tokens, 0);
        assert!(slot_lower_bounds(&[lane]).expect("bounds").is_empty());
        assert!(slot_lower_bounds_per_token(&[lane], 4)
            .expect("bounds")
            .is_empty());
    }

    #[test]
    fn a_lane_above_zero_bounds_every_token_it_contributes() {
        let a = [1, 2];
        let b = [3, 4];
        let lanes = [
            solo_lane(&a, 0),
            SpecLane {
                lane: 1,
                kv_base: 8192,
                state_base: 5,
                ..solo_lane(&b, 0)
            },
        ];
        assert_eq!(slot_lower_bounds(&lanes).expect("bounds"), vec![0, 8192]);
        // Per token, in the batch's own sequence-major order: lane 0's tokens
        // then lane 1's. A bound that drifted out of step with the tokens
        // would let a lane read below its own base, into whatever the
        // neighbour beneath it is holding.
        assert_eq!(
            slot_lower_bounds_per_token(&lanes, 3).expect("bounds"),
            vec![0, 0, 0, 8192, 8192, 8192]
        );
    }

    #[test]
    fn the_draft_head_never_claims_a_position_it_did_not_ingest() {
        // Depth 0: nothing was drafted, so nothing was ingested. Claiming one
        // leaves a hole the next catch-up skips, after which the draft head is
        // conditioned on a token it never read — and the only symptom is
        // proposals quietly getting worse.
        assert_eq!(draft_head_fill_after(0, 1, false), 0);
        assert_eq!(draft_head_fill_after(0, 1, true), 0);
        // The shipped default: only `first` is trustworthy, because the draft
        // chain's own tokens may have been rejected.
        assert_eq!(draft_head_fill_after(3, 4, false), 1);
        assert_eq!(draft_head_fill_after(1, 2, false), 1);
        // With reuse the accepted prefix is trustworthy, but never past what
        // was actually drafted.
        assert_eq!(draft_head_fill_after(3, 2, true), 2);
        assert_eq!(draft_head_fill_after(2, 4, true), 2);
    }

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

#[cfg(test)]
mod nucleus_tests {
    use super::{sampling_probabilities, LlamaSamplingParams};

    fn params(top_p: f32) -> LlamaSamplingParams {
        LlamaSamplingParams {
            temperature: 1.0,
            top_p,
            top_k: 0,
            seed: 7,
            ..Default::default()
        }
    }

    /// The candidate cap must not change the answer: a nucleus wider than the
    /// cap has to fall back to the exact ranking. A near-uniform vocabulary
    /// larger than CANDIDATE_CAP with top_p 0.99 is exactly that case.
    #[test]
    fn wide_nucleus_falls_back_to_the_exact_ranking() {
        let vocab = 4096;
        let logits: Vec<f32> = (0..vocab).map(|i| (i % 7) as f32 * 1e-3).collect();
        let probs = sampling_probabilities(&logits, params(0.99), &[]).unwrap();
        let kept = probs.iter().filter(|p| **p > 0.0).count();
        assert!(kept > 1024, "kept {kept}, expected the full nucleus");
        assert!((probs.iter().sum::<f32>() - 1.0).abs() < 1e-4);
    }

    #[test]
    fn narrow_nucleus_matches_the_full_sort() {
        let mut logits = vec![0.0f32; 4096];
        logits[10] = 24.0;
        logits[20] = 23.0;
        logits[30] = 22.0;
        let probs = sampling_probabilities(&logits, params(0.9), &[]).unwrap();
        // The three peaks carry all the mass, so the nucleus is just them and
        // it fits well inside the candidate cap.
        assert!(probs[10] > 0.0 && probs[20] > 0.0);
        assert_eq!(probs[0], 0.0);
        assert_eq!(probs.iter().filter(|p| **p > 0.0).count(), 2);
        assert!((probs.iter().sum::<f32>() - 1.0).abs() < 1e-5);
    }
}
