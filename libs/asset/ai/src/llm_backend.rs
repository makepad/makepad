//! The `llm` backend: local LLM prompt expansion (text domain).
//!
//! Role in the pipeline: the controlling AI (or a user) sends a TERSE asset
//! intent ("rusty pirate cannon"), and this backend expands it into a rich,
//! generation-ready prompt for a downstream domain — what a good Flux image
//! prompt, video shot prompt, or Trellis mesh prompt looks like lives in
//! per-domain system prompts that are plain editable text:
//!
//! - Defaults are embedded from `prompts/expand_{image,video,mesh,generic}.txt`.
//! - A file at `<cache_dir>/prompts/expand_<domain>.txt` overrides the
//!   embedded default at request time, so boxes can tune prompt style without
//!   a rebuild.
//!
//! Model: a dense Qwen3.5/3.6-family instruct GGUF on the in-repo
//! `makepad-llama` runtime (feature `llm`). The session lives on a dedicated
//! worker thread (`LlamaSession` is `!Send`) that stays alive across jobs —
//! together with the server worker keeping backend instances, that is the
//! warm-residency path: weights load once and stay resident.
//!
//! Everything except the actual token generation — prompt assembly, variant
//! fan-out, artifact shaping — compiles and tests WITHOUT the feature via a
//! stubbed generation fn, which is what CI runs.

use crate::backend::{CancelToken, ArtifactData, BackendCtx, ContentBackend, GenerateParams, ProgressSink};
use crate::error::AssetAiError;
use makepad_micro_serde::*;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// System prompts (editable data)
// ---------------------------------------------------------------------------

const PROMPT_IMAGE: &str = include_str!("../prompts/expand_image.txt");
const PROMPT_VIDEO: &str = include_str!("../prompts/expand_video.txt");
const PROMPT_MESH: &str = include_str!("../prompts/expand_mesh.txt");
const PROMPT_RIG: &str = include_str!("../prompts/expand_rig.txt");
const PROMPT_AUDIO: &str = include_str!("../prompts/expand_audio.txt");
const PROMPT_MUSIC: &str = include_str!("../prompts/expand_music.txt");
const PROMPT_GENERIC: &str = include_str!("../prompts/expand_generic.txt");

/// The next publishable streaming snapshot, or None to hold this round.
/// Streaming receivers (partial_text pollers, the chat broker's delta
/// slicer) rely on snapshots being PREFIX-STABLE: each publish only
/// appends. A chunk edge can split one character across byte-level BPE
/// tokens, so a full-sequence re-decode may end in U+FFFD this round and
/// re-decode as the real character next round — the unfinished tail is
/// trimmed BEFORE publishing (never published, so it can never wedge the
/// prefix check), and a decode that still diverges from what was already
/// published is held back until it heals.
fn next_stream_snapshot(prev: &str, decoded: &str) -> Option<String> {
    let trimmed = decoded.trim_end_matches('\u{fffd}');
    (trimmed.len() > prev.len() && trimmed.starts_with(prev)).then(|| trimmed.to_string())
}

// ---------------------------------------------------------------------------
// KV prefix cache (pure bookkeeping; the worker owns the session)
// ---------------------------------------------------------------------------

/// Which conversation the resident KV belongs to.
///
/// There is no session id on the `/generate` wire, so the identity is derived
/// from the prompt: a ChatML transcript for one conversation always opens with
/// the same system turn and the same FIRST user turn, and every later turn of
/// that conversation repeats them verbatim. Hashing the prompt through the end
/// of the first user turn therefore groups the turns of one conversation and
/// separates different ones.
///
/// It is a heuristic in exactly one direction: two genuinely different
/// conversations that open identically hash the same. That can only mislabel a
/// statistic — the reuse decision itself is still the literal text-prefix test,
/// which is sound whatever this returns.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct PrefixOwner {
    kind: String,
    opening: u64,
}

impl PrefixOwner {
    fn new(kind: &str, prompt_text: &str) -> Self {
        Self {
            kind: kind.to_string(),
            opening: fnv1a(conversation_opening(prompt_text).as_bytes()),
        }
    }

    fn short(&self) -> String {
        format!("{}/{:04x}", self.kind, self.opening & 0xffff)
    }
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    hash
}

/// The prompt through the end of its first `user` turn — the part every turn
/// of one conversation repeats. Falls back to the whole prompt when the
/// transcript has no complete user turn yet (single-shot expander jobs).
fn conversation_opening(prompt_text: &str) -> &str {
    const USER_OPEN: &str = "<|im_start|>user\n";
    const TURN_END: &str = "<|im_end|>";
    let Some(user_at) = prompt_text.find(USER_OPEN) else {
        return prompt_text;
    };
    let after = user_at + USER_OPEN.len();
    match prompt_text[after..].find(TURN_END) {
        Some(end) => &prompt_text[..after + end + TURN_END.len()],
        None => prompt_text,
    }
}

/// Why a job could not reuse the resident KV.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PrefixOutcome {
    /// The prompt extends the resident prefix: prefill only the delta.
    Hit,
    /// This conversation has been served before, and something else has taken
    /// the KV since. The whole re-prefill is waste that a second resident
    /// sequence would have avoided.
    Interleaved,
    /// First turn of this conversation (or a context-full restart). The
    /// prefill is real work, not waste.
    Cold,
}

/// The worker's KV prefix cache. **One entry, by construction** — the session
/// holds exactly one KV cache and one recurrent state, so exactly one
/// conversation's prefix can be resident.
///
/// N-way prefix caching is not reachable from here. Two conversations would
/// need two resident sequences (`LlamaSessionConfig::max_sequences > 1` plus a
/// slot table), and a partial rewind is not an escape hatch either: the 48
/// GatedDeltaNet layers carry a recurrent state that is only defined at the
/// position the session is standing on, so "rewind to the common prefix" has
/// no state to rewind to. Running two `LlamaSession`s instead is worse — each
/// owns its own device copy of the 16 GB arena. That is why this stayed a
/// one-entry cache and why the continuous-batching lane subsumes it.
///
/// So what this type adds is not more cache: it is knowing, and saying, what
/// the single entry costs. Every miss is classified and the interleave share
/// is accumulated, which turns "two chats re-prefill each other" from a
/// modelled ~2.6 s/turn into a measured number from production traffic.
#[derive(Default)]
pub(crate) struct PrefixCache {
    /// Prompt + reply + suffix the resident KV corresponds to.
    committed: String,
    /// Whose it is. `None` when the KV holds nothing usable.
    live: Option<PrefixOwner>,
    /// Conversations served since the worker started, most recent first, so a
    /// miss can be told apart from a cold start. Bounded: this is a classifier,
    /// not a cache.
    seen: std::collections::VecDeque<PrefixOwner>,
    stats: PrefixStats,
}

#[derive(Default, Debug, Clone, Copy)]
pub(crate) struct PrefixStats {
    pub hits: u64,
    pub cold: u64,
    pub interleaved: u64,
    /// Tokens re-prefilled purely because another conversation took the KV.
    pub interleaved_tokens: u64,
    pub interleaved_millis: u64,
}

impl PrefixCache {
    /// How many conversations to remember for miss classification. Larger than
    /// any plausible number of chats sharing one box, small enough to stay a
    /// linear scan.
    const SEEN_MAX: usize = 32;

    /// Classify this job against the resident prefix, WITHOUT changing the
    /// reuse rule: a hit is still "the new prompt literally extends the
    /// committed text", which is exactly the condition under which the
    /// resident KV is the prompt's own prefix.
    pub(crate) fn classify(&self, kind: &str, prompt_text: &str) -> (PrefixOutcome, PrefixOwner) {
        let owner = PrefixOwner::new(kind, prompt_text);
        // A different ChatML family must never extend the previous prefix even
        // if the bytes happen to line up: the systems differ, and a stale hit
        // is the expand->chat contamination path.
        let same_family = self.live.as_ref().is_some_and(|live| live.kind == kind);
        if same_family && !self.committed.is_empty() && prompt_text.starts_with(&self.committed) {
            return (PrefixOutcome::Hit, owner);
        }
        let outcome = if self.seen.contains(&owner) {
            PrefixOutcome::Interleaved
        } else {
            PrefixOutcome::Cold
        };
        (outcome, owner)
    }

    /// Record what the job actually cost. `tokens`/`elapsed` are the re-prefill
    /// the miss forced; on a hit they are the delta, which is real work.
    pub(crate) fn record(
        &mut self,
        outcome: PrefixOutcome,
        owner: &PrefixOwner,
        tokens: usize,
        elapsed: std::time::Duration,
    ) {
        match outcome {
            PrefixOutcome::Hit => self.stats.hits += 1,
            PrefixOutcome::Cold => self.stats.cold += 1,
            PrefixOutcome::Interleaved => {
                self.stats.interleaved += 1;
                self.stats.interleaved_tokens += tokens as u64;
                self.stats.interleaved_millis += elapsed.as_millis() as u64;
            }
        }
        self.seen.retain(|seen| seen != owner);
        self.seen.push_front(owner.clone());
        self.seen.truncate(Self::SEEN_MAX);
    }

    /// The KV now holds nothing usable (reset, or a failed prefill).
    pub(crate) fn invalidate(&mut self) {
        self.committed.clear();
        self.live = None;
    }

    /// The KV now corresponds exactly to `text`, for `owner`.
    pub(crate) fn commit(&mut self, owner: &PrefixOwner, text: String) {
        self.committed = text;
        self.live = Some(owner.clone());
    }

    pub(crate) fn committed(&self) -> &str {
        &self.committed
    }

    pub(crate) fn stats(&self) -> PrefixStats {
        self.stats
    }

    /// One honest line about what the single-entry cache is costing. Empty
    /// until an interleave has actually happened.
    pub(crate) fn waste_report(&self) -> String {
        let s = self.stats;
        if s.interleaved == 0 {
            return String::new();
        }
        format!(
            "interleave waste so far: {} turns, {} tok, {:.1}s re-prefilled because another \
             conversation held the only resident KV",
            s.interleaved,
            s.interleaved_tokens,
            s.interleaved_millis as f64 / 1000.0,
        )
    }
}

fn parse_prefill_counts(stage: &str) -> Option<(f64, f64)> {
    // "prefill 32/256 tok" or "kv reuse 8/8 tok"
    let mut nums = stage
        .split(|c: char| !c.is_ascii_digit())
        .filter_map(|p| p.parse::<f64>().ok());
    let done = nums.next()?;
    let total = nums.next()?;
    Some((done, total))
}

/// The embedded default system prompt for a target domain.
pub fn default_system_prompt(target_domain: &str) -> &'static str {
    match target_domain {
        "image" => PROMPT_IMAGE,
        "video" => PROMPT_VIDEO,
        "mesh" => PROMPT_MESH,
        // The character chain: the mesh will be auto-rigged + animated, so
        // the expansion must ask for a full-body A-pose humanoid, not the
        // mesh template's product-shot object.
        "rig" => PROMPT_RIG,
        "audio" => PROMPT_AUDIO,
        "music" => PROMPT_MUSIC,
        _ => PROMPT_GENERIC,
    }
}

/// The system prompt for a target domain, preferring an override file at
/// `<prompts_dir>/expand_<domain>.txt` when one exists.
pub fn system_prompt_for(target_domain: &str, prompts_dir: Option<&Path>) -> String {
    if let Some(dir) = prompts_dir {
        // Domain names come from the request; only look up sane ones.
        if target_domain
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            let path = dir.join(format!("expand_{target_domain}.txt"));
            if let Ok(text) = std::fs::read_to_string(&path) {
                if !text.trim().is_empty() {
                    return text;
                }
            }
        }
    }
    default_system_prompt(target_domain).to_string()
}

// ---------------------------------------------------------------------------
// Prompt assembly (pure, unit-tested)
// ---------------------------------------------------------------------------

/// One expansion request handed to the generator: a full chat-template prompt
/// plus sampling settings. The seed differs per variant.
#[derive(Clone, Debug)]
pub struct ExpandJob {
    pub prompt_text: String,
    pub max_tokens: u32,
    pub temperature: f32,
    pub seed: u64,
    /// Appended after the generated text in the worker's committed prefix.
    /// Expander jobs use `"\n"`; chat uses [`crate::protocol::CHAT_COMMIT_SUFFIX`].
    pub commit_suffix: String,
    /// KV-reuse domain: `"chat"` or `"expand:<target_domain>"`. Jobs of
    /// different kinds never continue each other's committed prefix — the
    /// ChatML systems differ, so a prefix match would be meaningless and a
    /// stale one is the expand→chat contamination path.
    pub kind: String,
}

/// The full Qwen ChatML prompt for one expansion. `think_prefill` is the
/// assistant-turn opener: Qwen3/3.6 close an empty think block so the
/// answer starts immediately; Qwen3.8 must leave `<think>` open.
pub fn build_prompt(system: &str, params: &GenerateParams) -> String {
    build_prompt_with_think(system, params, crate::protocol::CHAT_THINK_PREFILL)
}

pub fn build_prompt_with_think(
    system: &str,
    params: &GenerateParams,
    think_prefill: &str,
) -> String {
    let mut out = String::with_capacity(system.len() + params.prompt.len() + 256);
    out.push_str("<|im_start|>system\n");
    out.push_str(system.trim_end());
    out.push_str("<|im_end|>\n<|im_start|>user\n");
    out.push_str("Target domain: ");
    out.push_str(&params.target_domain);
    out.push('\n');
    if !params.identity_anchor.trim().is_empty() {
        out.push_str("Identity anchor (repeat this exact text verbatim in the answer): ");
        out.push_str(params.identity_anchor.trim());
        out.push('\n');
    }
    if !params.style.trim().is_empty() {
        out.push_str("Style direction: ");
        out.push_str(params.style.trim());
        out.push('\n');
    }
    out.push_str("Intent: ");
    out.push_str(params.prompt.trim());
    out.push_str("<|im_end|>\n<|im_start|>assistant\n");
    out.push_str(think_prefill);
    out
}

/// Strips artifacts a chat model may add around the expansion: a thinking
/// block (the prefill disables it, but be tolerant), surrounding quotes, and
/// whitespace.
pub fn clean_expansion(text: &str) -> String {
    let after_think = text.split("</think>").last().unwrap_or(text).trim();
    if text.contains("</think>") && !after_think.is_empty() {
        return strip_wrapping_quotes(after_think);
    }
    // Qwen3.8 often spends the whole decode inside <think> and drafts the
    // prompt in quotes. Prefer the last long quoted paragraph over the
    // reasoning dump so Flux/H3/Trellis get an actual prompt.
    if let Some(quoted) = last_quoted_paragraph(text) {
        return quoted;
    }
    strip_wrapping_quotes(after_think)
}

fn strip_wrapping_quotes(text: &str) -> String {
    text.strip_prefix('"')
        .and_then(|t| t.strip_suffix('"'))
        .unwrap_or(text)
        .trim()
        .to_string()
}

fn last_quoted_paragraph(text: &str) -> Option<String> {
    let mut best: Option<String> = None;
    let mut rest = text;
    while let Some(start) = rest.find('"') {
        let after = &rest[start + 1..];
        let Some(end) = after.find('"') else {
            break;
        };
        let chunk = after[..end].trim();
        if chunk.split_whitespace().count() >= 20 {
            best = Some(chunk.to_string());
        }
        rest = &after[end + 1..];
    }
    best
}

#[derive(SerJson)]
struct VariantsJson {
    model: String,
    target_domain: String,
    intent: String,
    variants: Vec<String>,
}

// ---------------------------------------------------------------------------
// The backend
// ---------------------------------------------------------------------------

/// Pluggable generation: the real path runs a `LlamaSession` on a worker
/// thread; tests plug in a closure.
pub type GenerateFn = Box<dyn FnMut(&ExpandJob) -> Result<String, AssetAiError> + Send>;

enum Generator {
    /// Test/CI path: canned generation, no model files.
    Stub(GenerateFn),
    /// Real path: worker spawned in `ensure_loaded` once the GGUF is cached.
    #[cfg(feature = "llm")]
    Llama(Option<llama_worker::LlamaWorker>),
}

pub struct LlmBackend {
    model_id: String,
    generator: Generator,
    /// `<cache_dir>/prompts`, once `ensure_loaded` has seen the cache dir.
    prompts_dir: Option<PathBuf>,
    /// GGUF path resolved by `ensure_loaded` (real path only).
    #[cfg_attr(not(feature = "llm"), allow(dead_code))]
    gguf_path: Option<PathBuf>,
}

impl LlmBackend {
    /// Test/CI constructor: generation is the given closure, no files needed.
    pub fn with_stub(model_id: &str, generate: GenerateFn) -> Self {
        Self {
            model_id: model_id.to_string(),
            generator: Generator::Stub(generate),
            prompts_dir: None,
            gguf_path: None,
        }
    }

    /// Real constructor used by `create_backend`; the session spawns on the
    /// first `ensure_loaded` (which may download the GGUF).
    #[cfg(feature = "llm")]
    pub fn new_llama(model_id: &str) -> Self {
        Self {
            model_id: model_id.to_string(),
            generator: Generator::Llama(None),
            prompts_dir: None,
            gguf_path: None,
        }
    }

}

/// Runs one expansion; `on_token(k, max)` fires per decode chunk, `on_text`
/// receives prefix-stable full-text snapshots (real path only) and `cancel` is
/// checked between chunks.
///
/// Takes the generator rather than the backend so the caller can hold `&mut`
/// on one field instead of the whole struct.
fn expand_with(
    generator: &mut Generator,
    job: &ExpandJob,
    cancel: &CancelToken,
    on_token: &mut dyn FnMut(u32, u32),
    on_stage: &mut dyn FnMut(&str),
    on_text: &mut dyn FnMut(&str),
) -> Result<String, AssetAiError> {
    match generator {
        Generator::Stub(generate) => {
            cancel.check()?;
            let _ = (&on_token, &on_stage, &on_text);
            generate(job)
        }
        #[cfg(feature = "llm")]
        Generator::Llama(worker) => {
            let worker = worker.as_ref().ok_or_else(|| {
                AssetAiError::Backend("llm backend used before ensure_loaded".to_string())
            })?;
            expand_through(worker, job, cancel, on_token, on_stage, on_text, &mut |_| {})
        }
    }
}

/// The worker call itself, shared by the `&mut self` path and the concurrent
/// handle. `&LlamaWorker` is all it needs: the session lives on its own thread
/// and this only posts to it.
#[cfg(feature = "llm")]
fn expand_through(
    worker: &llama_worker::LlamaWorker,
    job: &ExpandJob,
    cancel: &CancelToken,
    on_token: &mut dyn FnMut(u32, u32),
    on_stage: &mut dyn FnMut(&str),
    on_text: &mut dyn FnMut(&str),
    on_serving: crate::backend::ServingSink,
) -> Result<String, AssetAiError> {
    worker
        .expand(job.clone(), cancel.clone(), on_token, on_stage, on_text, on_serving)
        .map_err(|e| {
            if e == "cancelled" {
                AssetAiError::Cancelled
            } else {
                AssetAiError::Backend(format!("llm: {e}"))
            }
        })
}

impl ContentBackend for LlmBackend {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn ensure_loaded(&mut self, ctx: &mut BackendCtx) -> Result<(), AssetAiError> {
        self.prompts_dir = Some(ctx.cache_dir.join("prompts"));
        match &mut self.generator {
            Generator::Stub(_) => Ok(()),
            #[cfg(feature = "llm")]
            Generator::Llama(worker) => {
                let paths = ctx.ensure_files()?;
                let gguf = paths
                    .iter()
                    .find(|p| p.extension().map_or(false, |e| e == "gguf"))
                    .cloned()
                    .ok_or_else(|| {
                        AssetAiError::Backend(format!(
                            "model {}: registry lists no .gguf file",
                            self.model_id
                        ))
                    })?;
                if worker.is_none() || self.gguf_path.as_deref() != Some(gguf.as_path()) {
                    let gb = std::fs::metadata(&gguf)
                        .map(|m| m.len() as f64 / 1e9)
                        .unwrap_or(0.0);
                    (ctx.progress)(&format!("load llm gguf ({gb:.1}GB)"), 0.1);
                    // Loads weights on the worker thread; blocks until the
                    // session reports in so load errors surface here.
                    *worker = Some(
                        llama_worker::LlamaWorker::spawn(
                            gguf.clone(),
                            self.model_id.clone(),
                            ctx.progress,
                        )
                            .map_err(|e| AssetAiError::Backend(format!("llm load: {e}")))?,
                    );
                    self.gguf_path = Some(gguf);
                    (ctx.progress)("llm session ready", 0.9);
                }
                Ok(())
            }
        }
    }

    fn is_resident(&self) -> bool {
        match &self.generator {
            #[cfg(feature = "llm")]
            Generator::Llama(worker) => worker.is_some(),
            _ => false,
        }
    }

    fn unload(&mut self) -> Result<(), AssetAiError> {
        match &mut self.generator {
            #[cfg(feature = "llm")]
            Generator::Llama(worker) => {
                // Dropping the worker closes the session thread; weights
                // unmap there. Next ensure_loaded respawns.
                *worker = None;
                self.gguf_path = None;
            }
            _ => {}
        }
        Ok(())
    }

    fn generate(
        &mut self,
        params: &GenerateParams,
        progress: ProgressSink,
        cancel: &CancelToken,
    ) -> Result<Vec<ArtifactData>, AssetAiError> {
        self.generate_streamed(params, progress, &mut |_| {}, cancel)
    }

    fn generate_streamed(
        &mut self,
        params: &GenerateParams,
        progress: ProgressSink,
        on_text: &mut dyn FnMut(&str),
        cancel: &CancelToken,
    ) -> Result<Vec<ArtifactData>, AssetAiError> {
        let model_id = self.model_id.clone();
        let prompts_dir = self.prompts_dir.clone();
        let generator = &mut self.generator;
        llm_generate_streamed(
            &model_id,
            prompts_dir.as_deref(),
            &mut |job, cancel, on_token, on_stage, on_text| {
                expand_with(generator, job, cancel, on_token, on_stage, on_text)
            },
            params,
            progress,
            on_text,
            cancel,
        )
    }

    /// A resident LLM can serve several turns at once — that is the whole
    /// point of the lane session — so it hands out a handle and lets the
    /// caller drop the backend registry lock before generating.
    ///
    /// Only when the worker is actually up. Before `ensure_loaded` there is
    /// nothing to talk to, and a handle that answers "used before
    /// ensure_loaded" to every turn would be worse than admitting there is no
    /// concurrent path yet.
    ///
    /// The stub generator deliberately gets none: it is an `FnMut` owned by
    /// one backend object, and pretending otherwise would make a test path
    /// claim a concurrency the real one has to honour.
    #[cfg(feature = "llm")]
    fn concurrent(&self) -> Option<Box<dyn crate::backend::ConcurrentBackend>> {
        match &self.generator {
            Generator::Llama(Some(worker)) => Some(Box::new(LlmConcurrent {
                model_id: self.model_id.clone(),
                prompts_dir: self.prompts_dir.clone(),
                worker: worker.clone(),
            })),
            _ => None,
        }
    }
}

/// One turn's view of a resident LLM: everything `llm_generate_streamed` needs
/// and nothing that has to be locked.
///
/// Cloned per caller rather than shared, because the channel to the session
/// thread is `Send` but not `Sync`.
#[cfg(feature = "llm")]
struct LlmConcurrent {
    model_id: String,
    prompts_dir: Option<PathBuf>,
    worker: llama_worker::LlamaWorker,
}

#[cfg(feature = "llm")]
impl crate::backend::ConcurrentBackend for LlmConcurrent {
    fn generate_streamed(
        &self,
        params: &GenerateParams,
        progress: ProgressSink,
        on_text: &mut dyn FnMut(&str),
        serving: crate::backend::ServingSink,
        cancel: &CancelToken,
    ) -> Result<Vec<ArtifactData>, AssetAiError> {
        let serving = std::cell::RefCell::new(serving);
        llm_generate_streamed(
            &self.model_id,
            self.prompts_dir.as_deref(),
            &mut |job, cancel, on_token, on_stage, on_text| {
                expand_through(
                    &self.worker,
                    job,
                    cancel,
                    on_token,
                    on_stage,
                    on_text,
                    &mut |update| (serving.borrow_mut())(update),
                )
            },
            params,
            progress,
            on_text,
            cancel,
        )
    }
}

/// The LLM generation body, free of `self`.
///
/// Extracted so the two entry points cannot drift: `LlmBackend` runs it
/// through `&mut self` (the stub generator needs that), and the concurrent
/// handle runs it through a cloned worker channel with no lock held. Both
/// build the same prompt, honour the same think-budget floors, apply the same
/// identity-anchor checks and emit the same artifacts, because it is the same
/// code — a second copy would be one refusal or one floor out of date within a
/// release.
#[allow(clippy::too_many_arguments)]
fn llm_generate_streamed(
    model_id: &str,
    prompts_dir: Option<&Path>,
    expand: &mut dyn FnMut(
        &ExpandJob,
        &CancelToken,
        &mut dyn FnMut(u32, u32),
        &mut dyn FnMut(&str),
        &mut dyn FnMut(&str),
    ) -> Result<String, AssetAiError>,
    params: &GenerateParams,
    progress: ProgressSink,
    on_text: &mut dyn FnMut(&str),
    cancel: &CancelToken,
) -> Result<Vec<ArtifactData>, AssetAiError> {
        let is_chat = params.target_domain == "chat";
        if params.prompt.trim().is_empty() {
            return Err(AssetAiError::Backend(if is_chat {
                "llm chat needs a non-empty prompt or chat_messages".to_string()
            } else {
                "llm expander needs a non-empty prompt (the terse asset intent)".to_string()
            }));
        }
        if !is_chat
            && !params.identity_anchor.trim().is_empty()
            && !params
                .prompt
                .to_lowercase()
                .contains(&params.identity_anchor.trim().to_lowercase())
        {
            return Err(AssetAiError::Params(
                "identity_anchor must occur in the terse prompt".to_string(),
            ));
        }
        let prompt_text = if is_chat {
            params.prompt.clone()
        } else {
            let system = format!(
                "Target domain: {}.\n\n{}",
                params.target_domain,
                system_prompt_for(&params.target_domain, prompts_dir)
            );
            let mut user = String::new();
            if !params.identity_anchor.trim().is_empty() {
                user.push_str("Identity anchor (repeat this exact text verbatim in the answer): ");
                user.push_str(params.identity_anchor.trim());
                user.push('\n');
            }
            if !params.style.trim().is_empty() {
                user.push_str("Style direction: ");
                user.push_str(params.style.trim());
                user.push('\n');
            }
            user.push_str("Intent: ");
            user.push_str(params.prompt.trim());
            // Qwen3.8 chat-shaped open think is the path that actually
            // decodes. The older expander ChatML + empty/seed </think>
            // prefill made 3.8 emit EOS on the first token.
            crate::protocol::assemble_chat_prompt_with_think(
                &system,
                &[crate::protocol::ChatMessageJson {
                    role: "user".to_string(),
                    text: user,
                }],
                crate::protocol::think_prefill_for_model(model_id),
            )
        };

        let variants = params.variants.max(1);
        let mut expansions = Vec::with_capacity(variants as usize);
        for index in 0..variants {
            cancel.check()?;
            // Per-variant progress band; token events subdivide it. The
            // prefill (batched, ~1-2s) is the gap before the first token.
            let base = index as f64 / variants as f64;
            let span = 0.93 / variants as f64;
            progress(
                &if variants > 1 {
                    format!("starting (variant {}/{variants})", index + 1)
                } else {
                    "starting".to_string()
                },
                0.02 + base * 0.93,
            );
            let job = ExpandJob {
                prompt_text: prompt_text.clone(),
                // Qwen3.8 leaves <think> open. A 220-token fleet request
                // can spend the whole budget inside the think block; after
                // clean_expansion strips it the expansion is empty and the
                // image/video/mesh pipeline has nothing to feed forward.
                // Chat gets the same treatment at a lower floor: a 32-token
                // chat budget is spent inside the open think and the reply
                // truncates to nothing.
                max_tokens: if is_chat {
                    if crate::protocol::model_uses_open_think(model_id) {
                        params.max_tokens.max(128)
                    } else {
                        params.max_tokens
                    }
                } else if crate::protocol::model_uses_open_think(model_id) {
                    params.max_tokens.max(512)
                } else {
                    params.max_tokens
                },
                // Greedy decode would make every variant identical; sampled
                // variants past the first get a floor temperature.
                temperature: if index == 0 {
                    params.temperature
                } else {
                    params.temperature.max(0.7)
                },
                seed: params.seed.wrapping_add(index as u64),
                commit_suffix: if is_chat {
                    crate::protocol::CHAT_COMMIT_SUFFIX.to_string()
                } else {
                    "\n".to_string()
                },
                kind: if is_chat {
                    "chat".to_string()
                } else {
                    format!("expand:{}", params.target_domain)
                },
            };
            let sink = std::cell::RefCell::new(&mut *progress);
            let mut on_token = |k: u32, max: u32| {
                let u = k as f64 / max.max(1) as f64;
                (sink.borrow_mut())(
                    &format!("decode {k}/{max}"),
                    0.08 + base * 0.93 + span * 0.88 * u,
                );
            };
            let mut on_stage = |stage: &str| {
                let frac = if let Some((done, total)) = parse_prefill_counts(stage) {
                    0.02 + base * 0.93 + span * 0.06 * (done / total.max(1.0))
                } else if stage.starts_with("prefill") {
                    0.03 + base * 0.93
                } else {
                    0.02 + base * 0.93
                };
                (sink.borrow_mut())(stage, frac);
            };
            // Text snapshots stream only for single-variant runs (the chat
            // lane is always one) — interleaved variants would fight over
            // one partial_text slot.
            let mut on_text_variant = |text: &str| {
                if variants == 1 {
                    on_text(text);
                }
            };
            let raw =
                expand(&job, cancel, &mut on_token, &mut on_stage, &mut on_text_variant)?;
            let text = if is_chat {
                raw
            } else {
                clean_expansion(&raw)
            };
            if text.is_empty() {
                return Err(AssetAiError::Backend(format!(
                    "llm produced an empty expansion (variant {index})"
                )));
            }
            if !params.identity_anchor.trim().is_empty()
                && !text
                    .to_lowercase()
                    .contains(&params.identity_anchor.trim().to_lowercase())
            {
                return Err(AssetAiError::Backend(format!(
                    "llm expansion dropped identity anchor {:?} (variant {index})",
                    params.identity_anchor.trim()
                )));
            }
            expansions.push(text);
        }
        progress("encode", 0.95);

        let mut artifacts = vec![ArtifactData {
            content_type: "text/plain; charset=utf-8",
            ext: "txt",
            bytes: expansions[0].clone().into_bytes(),
        }];
        if expansions.len() > 1 {
            let json = VariantsJson {
                model: model_id.to_string(),
                target_domain: params.target_domain.clone(),
                intent: params.prompt.clone(),
                variants: expansions,
            };
            artifacts.push(ArtifactData {
                content_type: "application/json",
                ext: "json",
                bytes: json.serialize_json().into_bytes(),
            });
        }
        progress("done", 1.0);
        Ok(artifacts)
}

/// Per-lane context from `MAKEPAD_ASSET_AI_LLM_CONTEXT`, in tokens.
///
/// `None` when unset, so the caller keeps its own default rather than this
/// function inventing one.
pub fn configured_context_per_lane() -> Option<u32> {
    std::env::var("MAKEPAD_ASSET_AI_LLM_CONTEXT")
        .ok()
        .and_then(|v| v.trim().parse::<u32>().ok())
        .filter(|per_lane| *per_lane > 0)
}

/// Tokens held back from a prompt so there is room to answer it.
///
/// A prompt that exactly fills the context leaves nowhere for the reply, and
/// the failure lands at the first decode rather than at admission.
pub(crate) const DECODE_HEADROOM: usize = 512;

/// Drop the oldest non-system turn from a rendered ChatML prompt.
///
/// Returns `None` when there is nothing left to drop — the system block and
/// the trailing assistant opener are never candidates. The taught context IS
/// the assistant's instructions, so dropping it would change who the model is
/// rather than what it remembers.
///
/// Works on the rendered text because that is what the worker has. The format
/// is `<|im_start|>role\n...<|im_end|>\n` repeated, ending with an
/// `<|im_start|>assistant\n` opener with no terminator — so the blocks are
/// found by their delimiters and the opener is whatever follows the last one.
pub(crate) fn drop_oldest_turn(prompt: &str) -> Option<String> {
    const START: &str = "<|im_start|>";
    const END: &str = "<|im_end|>\n";
    // The system block is the first, and it stays.
    let first = prompt.find(START)?;
    let after_system = prompt[first..].find(END).map(|at| first + at + END.len())?;
    // The oldest droppable turn starts here.
    let rest = &prompt[after_system..];
    if !rest.starts_with(START) {
        return None;
    }
    let end = rest.find(END)? + END.len();
    // Never drop the trailing opener: it has no terminator, so if this block
    // ran to the end of the prompt there was no complete turn to remove.
    if after_system + end >= prompt.len() {
        return None;
    }
    let mut out = String::with_capacity(prompt.len() - end);
    out.push_str(&prompt[..after_system]);
    out.push_str(&rest[end..]);
    Some(out)
}

/// Chat lanes this box is configured for.
///
/// The service reads it to size the chat admission class and the number of
/// chat workers, so the store, the workers and the session all agree on one
/// number instead of three places computing it. Available without the `llm`
/// feature — a build with no LLM simply has one lane and no chat jobs.
pub fn configured_lane_count() -> usize {
    std::env::var("MAKEPAD_ASSET_AI_LLM_LANES")
        .ok()
        .and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(1)
        .clamp(1, 8)
}

/// Repetition mitigation for every turn this box serves, read once per
/// request from `MAKEPAD_ASSET_AI_LLM_{PRESENCE_PENALTY,FREQUENCY_PENALTY,
/// PENALTY_WINDOW}`.
///
/// The serving sampler had none at all, and a Q4 27B at long context on
/// numeric or tabular history enters a self-reinforcing loop that neither
/// temperature nor top-p can end — it runs to `max_tokens` emitting
/// `66.67.68|66|660 / 67.68.69|67|670 ...`, reproduced on .165 over two turns
/// at speculative acceptance 0.91-0.96 against 0.41-0.53 for ordinary prose.
/// Acceptance that high IS the loop: the draft head predicts a repeating
/// sequence almost perfectly.
///
/// The shape is frequency-dominant on purpose. A loop uses the same token ten
/// to thirty times inside a 64-token window and gets crushed; ordinary prose
/// reuses a common word two or three times and is barely touched. Presence
/// hits a word used once exactly as hard as one used twenty times, which is a
/// tax on normal English, so it defaults to 0 and stays a knob.
///
/// One helper rather than two literals, because the lane path and the solo
/// path must sample identically: a box whose answers change depending on
/// whether anyone else was talking is a bug nobody can reproduce.
fn sampling_penalties() -> (f32, f32, usize) {
    fn env_f32(key: &str, default: f32) -> f32 {
        // Garbage reads as the default rather than as zero: a typo must not
        // silently switch the mitigation off, which is the failure this whole
        // helper exists to prevent.
        std::env::var(key)
            .ok()
            .and_then(|v| v.trim().parse::<f32>().ok())
            .filter(|value| value.is_finite() && *value >= 0.0)
            .unwrap_or(default)
    }
    (
        env_f32("MAKEPAD_ASSET_AI_LLM_PRESENCE_PENALTY", 0.0),
        env_f32("MAKEPAD_ASSET_AI_LLM_FREQUENCY_PENALTY", 0.25),
        std::env::var("MAKEPAD_ASSET_AI_LLM_PENALTY_WINDOW")
            .ok()
            .and_then(|v| v.trim().parse::<usize>().ok())
            .unwrap_or(64),
    )
}

#[cfg(test)]
mod penalty_tests {
    use super::sampling_penalties;

    #[test]
    fn the_penalty_knobs_default_frequency_dominant() {
        // The defaults ARE the mitigation: a box that sets nothing must still
        // be protected from the loop, and it must be protected by the
        // frequency term rather than the presence one.
        //
        // Env is process-global and the suite runs threaded, so this reads the
        // defaults rather than setting anything — a `set_var` here would
        // change what every other test in this process sees.
        for key in [
            "MAKEPAD_ASSET_AI_LLM_PRESENCE_PENALTY",
            "MAKEPAD_ASSET_AI_LLM_FREQUENCY_PENALTY",
            "MAKEPAD_ASSET_AI_LLM_PENALTY_WINDOW",
        ] {
            if std::env::var(key).is_ok() {
                return;
            }
        }
        let (presence, frequency, window) = sampling_penalties();
        assert_eq!(presence, 0.0, "presence taxes ordinary prose; off by default");
        assert!(frequency > 0.0, "a box that sets nothing is still protected");
        assert!(frequency < 1.0, "and not so hard it rewrites normal English");
        assert!(window >= 32, "a window too short cannot see a loop at all");
    }
}

// ---------------------------------------------------------------------------
// Stall watchdog: no turn holds a lane or a queue slot without moving
// ---------------------------------------------------------------------------

/// What a turn is waiting on.
///
/// Which one it is decides what "progress" even means for it, and it is the
/// first thing a person reading a stall line has to know: a turn stuck in
/// prefill and a turn stuck in decode are two different faults on two
/// different code paths.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TurnPhase {
    /// Submitted, with no lane of its own yet — queued behind other turns, or
    /// on a lane the worker could not attribute. It holds a queue slot and
    /// nothing else, and what it is waiting for is the turns ahead of it
    /// moving, so the clock it is judged on is the BOX's rather than its own.
    Waiting,
    /// Ingesting its prompt on a known lane. Progress is tokens ingested.
    Prefill,
    /// Generating. Progress is tokens produced.
    Decode,
}

impl TurnPhase {
    /// How the phase reads in a log line and in the client's error.
    pub(crate) fn name(self) -> &'static str {
        match self {
            TurnPhase::Waiting => "waiting for a lane",
            TurnPhase::Prefill => "prefill",
            TurnPhase::Decode => "decode",
        }
    }
}

/// How long a turn may make no progress at all before the watchdog takes its
/// lane back, in seconds. `MAKEPAD_ASSET_AI_LLM_STALL_SECS` overrides it and
/// 0 turns the watchdog off.
///
/// 300 s, derived from the two prefill rates this box has actually been
/// measured at rather than from a feeling about how long is too long.
///
/// The finest progress signal a turn has is one prefill CHUNK — 512 tokens,
/// which is 1.13 s at the 454 tok/s the live box ingested a 34,074-token cold
/// prompt at (the 75-second turn that was escalated as a wedge and was not
/// one), and 2.77 s at 185 tok/s, the slowest prefill rate the box has ever
/// been measured at. A healthy turn is therefore never quiet for more than
/// about three seconds.
///
/// The budget is not three seconds, because the signal is not always that
/// fine. A turn the worker could not attribute to a lane says nothing at all
/// until its whole prompt is in, and a full 131,072-token lane at that same
/// 454 tok/s is 289 s of entirely legitimate silence. 300 s clears that worst
/// case, clears the incident four times over, and is still short enough that
/// an operator gets an answer instead of a frozen row.
pub(crate) const DEFAULT_STALL_SECS: u64 = 300;

/// The watchdog's budget, in one place so the decision and the log agree.
#[derive(Clone, Copy, Debug)]
pub(crate) struct StallPolicy {
    budget: std::time::Duration,
}

impl StallPolicy {
    pub(crate) fn seconds(secs: u64) -> Self {
        Self {
            budget: std::time::Duration::from_secs(secs),
        }
    }

    /// The configured budget. An unparseable value reads as the default
    /// rather than as "off": a typo must not quietly disarm the watchdog.
    pub(crate) fn from_env() -> Self {
        Self::seconds(
            std::env::var("MAKEPAD_ASSET_AI_LLM_STALL_SECS")
                .ok()
                .and_then(|v| v.trim().parse::<u64>().ok())
                .unwrap_or(DEFAULT_STALL_SECS),
        )
    }

    pub(crate) fn budget(&self) -> std::time::Duration {
        self.budget
    }

    pub(crate) fn is_off(&self) -> bool {
        self.budget.is_zero()
    }
}

/// A turn that has stopped moving, and the two facts every report of it needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Stalled {
    pub(crate) phase: TurnPhase,
    pub(crate) idle: std::time::Duration,
}

/// The watchdog's whole decision, over facts a test can hand it.
///
/// Separated from the worker loop on purpose: the loop it guards runs on a
/// GPU, and a rule that can only be exercised there is a rule nobody checks.
/// Everything the decision needs is here — which phase the turn is in, when it
/// last moved, what time it is now, and the budget.
pub(crate) fn stall_verdict(
    phase: TurnPhase,
    last_progress: std::time::Instant,
    now: std::time::Instant,
    policy: &StallPolicy,
) -> Option<Stalled> {
    if policy.is_off() {
        return None;
    }
    // `saturating_duration_since` rather than subtraction: a clock that
    // appears to run backwards must read as "no idle time", not panic inside
    // the loop that serves every conversation on the box.
    let idle = now.saturating_duration_since(last_progress);
    (idle >= policy.budget).then_some(Stalled { phase, idle })
}

#[cfg(test)]
mod stall_tests {
    use super::{stall_verdict, StallPolicy, TurnPhase, DEFAULT_STALL_SECS};
    use std::time::{Duration, Instant};

    /// The incident this watchdog was written after: a 34,074-token cold
    /// prefill that held its lane for 75 seconds, finished normally, and was
    /// escalated as a wedge because nothing about it moved. A watchdog that
    /// reaps THAT turn has replaced a confusing wait with a broken one.
    #[test]
    fn a_seventy_five_second_cold_prefill_is_not_a_stall() {
        let policy = StallPolicy::seconds(DEFAULT_STALL_SECS);
        let started = Instant::now();
        for phase in [TurnPhase::Waiting, TurnPhase::Prefill, TurnPhase::Decode] {
            assert_eq!(
                stall_verdict(phase, started, started + Duration::from_secs(75), &policy),
                None,
                "{phase:?}: the 34k-token prefill from the incident must survive"
            );
        }
        // And the worst legitimate case the budget was sized for: a full
        // 131,072-token lane at the same measured 454 tok/s, seen by a turn
        // whose only signal is the prefill finishing.
        let worst = Duration::from_secs_f64(131_072.0 / 454.0);
        assert!(
            worst < policy.budget(),
            "a full-lane cold prefill ({worst:?}) must fit inside the budget"
        );
        assert_eq!(
            stall_verdict(TurnPhase::Prefill, started, started + worst, &policy),
            None
        );
    }

    #[test]
    fn silence_past_the_budget_is_a_stall_and_names_its_phase() {
        let policy = StallPolicy::seconds(30);
        let started = Instant::now();
        // One tick under is still hope; the budget itself is not.
        assert_eq!(
            stall_verdict(
                TurnPhase::Decode,
                started,
                started + Duration::from_secs(29),
                &policy
            ),
            None
        );
        let verdict = stall_verdict(
            TurnPhase::Decode,
            started,
            started + Duration::from_secs(31),
            &policy,
        )
        .expect("31 s of silence on a 30 s budget is a stall");
        // The phase rides along because the log line and the client's error
        // both have to say which half of the turn stopped.
        assert_eq!(verdict.phase, TurnPhase::Decode);
        assert_eq!(verdict.phase.name(), "decode");
        assert!(verdict.idle >= Duration::from_secs(31));
        // A queued turn is judged the same way, on the box's clock: if
        // NOTHING anywhere has moved for the budget, the box is wedged and a
        // client waiting on it deserves an answer rather than a hang.
        assert!(stall_verdict(
            TurnPhase::Waiting,
            started,
            started + Duration::from_secs(31),
            &policy
        )
        .is_some());
    }

    #[test]
    fn a_zero_budget_disarms_the_watchdog_entirely() {
        // The escape hatch for a box doing something this file cannot predict
        // — a 300k-token ingest, a deliberately paused session. Off means off,
        // at any idle time, in any phase.
        let off = StallPolicy::seconds(0);
        let started = Instant::now();
        for phase in [TurnPhase::Waiting, TurnPhase::Prefill, TurnPhase::Decode] {
            assert_eq!(
                stall_verdict(phase, started, started + Duration::from_secs(86_400), &off),
                None
            );
        }
    }

    #[test]
    fn a_clock_that_appears_to_run_backwards_is_not_a_stall() {
        // `now` before `last_progress` is unreachable in the loop, but the
        // arithmetic that assumes it cannot happen is a panic on the thread
        // that serves every conversation on the box.
        let policy = StallPolicy::seconds(30);
        let now = Instant::now();
        assert_eq!(
            stall_verdict(TurnPhase::Prefill, now + Duration::from_secs(10), now, &policy),
            None
        );
    }

    #[test]
    fn the_default_budget_clears_the_measured_worst_case() {
        // Guards the constant itself: the numbers in its doc comment are the
        // reason it is 300 and not 30, and a later edit that trims it has to
        // fail here rather than on the box.
        let policy = StallPolicy::seconds(DEFAULT_STALL_SECS);
        assert!(policy.budget() >= Duration::from_secs(289));
        assert!(!policy.is_off());
    }
}

// ---------------------------------------------------------------------------
// Real generation: LlamaSession on a keep-alive worker thread (feature llm)
// ---------------------------------------------------------------------------

#[cfg(feature = "llm")]
mod llama_worker {
    use super::ExpandJob;
    use crate::backend::CancelToken;
    use makepad_ai_llm::{
        LlamaSamplerState, LlamaSamplingParams, LlamaSession, LlamaSessionConfig, LlamaStopReason,
    };
    use std::path::PathBuf;
    use std::sync::mpsc;

    /// Big enough for the chat lane's taught context + tool-round history
    /// (8192 was the wall the sandbox-LLM doom benchmark died on at 9064
    /// tokens); still far under the model's native 262k window so the KV
    /// cache stays modest. Long-context decode is safe since the FlashMma
    /// gate fix (the 16k cliff is gone fleet-wide).
    const MAX_CONTEXT: u32 = 32768;
    /// Batched prefill: measured 350-600 tok/s vs ~28 tok/s at batch 1 on
    /// the 9B (see libs/converse qwen_filter.rs).
    ///
    /// 512, not 64. A chunk's cost is dominated by terms that do not scale
    /// with the tokens in it — the attention kernel's pass over the key span,
    /// and a 27B FFN that is latency-bound below a few hundred columns — so 64
    /// was costing a factor of four and a half on a real prompt. Measured on
    /// .217, 4096 tokens into one lane: 786 tok/s at 64, 3575 at 512, 3800 at
    /// 1024. The last step is not worth twice the graph activations.
    ///
    /// A player feels this as the wait before the FIRST token of a fresh
    /// conversation: a 7,900-token taught context is 10 s at 786 tok/s and
    /// 2.2 s at 3575.
    const PREFILL_BATCH: usize = 512;

    /// Concurrent decode lanes. 1 keeps the single-lane worker, which is the
    /// path every existing deployment runs; >1 selects the batched worker.
    ///
    /// A box-level decision (VRAM and card), so it is an env knob rather than
    /// a per-request one: `MAKEPAD_ASSET_AI_LLM_LANES`.
    fn lane_count() -> usize {
        super::configured_lane_count()
    }

    /// Context each lane may hold. The arena is `lanes * this`.
    ///
    /// `MAKEPAD_ASSET_AI_LLM_CONTEXT` sets it directly, in tokens PER LANE —
    /// which is the unit people actually reason in ("64k per chat") and the
    /// unit `/health` advertises. Unset, it falls back to dividing the built-in
    /// budget, which is exactly what every box did before the knob existed.
    ///
    /// A knob rather than a constant because the right answer is per BOX: the
    /// same number that fits comfortably on a 96 GB card is an
    /// out-of-memory-at-load on a 32 GB one, and a hardcoded const has to be
    /// wrong for one of them.
    fn context_per_lane() -> u32 {
        super::configured_context_per_lane().unwrap_or_else(|| {
            context_for_lanes(MAX_CONTEXT, lane_count())
        })
    }

    /// Split a total context budget across lanes.
    ///
    /// Separated from the env lookup so the arithmetic is testable: getting it
    /// wrong either overruns VRAM (multiplying instead of dividing) or hands
    /// every conversation a uselessly short window, and both are quiet.
    fn context_for_lanes(total: u32, lanes: usize) -> u32 {
        (total / lanes.max(1) as u32).max(1)
    }

    #[cfg(test)]
    mod lane_config_tests {
        use super::{context_for_lanes, MAX_CONTEXT};

        #[test]
        fn lanes_divide_the_context_budget_they_do_not_multiply_it() {
            // The arena is lanes x per-lane, so per-lane must DIVIDE. The
            // opposite mistake allocates 4x the KV and fails at load, on the
            // box, after a swap.
            assert_eq!(context_for_lanes(32768, 1), 32768);
            assert_eq!(context_for_lanes(32768, 2), 16384);
            assert_eq!(context_for_lanes(32768, 4), 8192);
            for lanes in 1..=8 {
                let per = context_for_lanes(MAX_CONTEXT, lanes);
                assert!(
                    per * lanes as u32 <= MAX_CONTEXT,
                    "{lanes} lanes x {per} overruns the {MAX_CONTEXT} budget"
                );
            }
        }

        #[test]
        fn a_degenerate_lane_count_still_yields_a_usable_window() {
            assert_eq!(context_for_lanes(32768, 0), 32768, "zero lanes reads as one");
            assert!(context_for_lanes(4, 8) >= 1, "never a zero-length context");
        }
    }

    /// Say ONCE why a long conversation keeps prefilling cold.
    ///
    /// Once per process, not per turn: it is a property of how the box is
    /// configured, so repeating it every message would bury the turn lines that
    /// are actually per-turn facts.
    fn warn_open_think_is_never_warm() {
        use std::sync::Once;
        static SAID: Once = Once::new();
        if crate::protocol::chat_think_mode() == "brief" {
            return;
        }
        SAID.call_once(|| {
            eprintln!(
                "[llm-worker] NOTE: a returning conversation cannot be warm while thinking is \
                 open. The lane's cache holds <think> + the reasoning + the answer; a client \
                 stores the answer alone, so its next prompt stops extending the cache at the \
                 last token of the previous prompt and the whole history is re-ingested. \
                 MAKEPAD_ASSET_AI_CHAT_THINK=brief makes the rendered history match what was \
                 generated, and turns after the first become a delta."
            );
        });
    }

    /// One session, N conversations, jobs joining and leaving at chunk
    /// boundaries.
    ///
    /// The scheduler decides, the executor performs, and this only moves work
    /// in and events out. Solo turns route to the speculative path inside the
    /// executor, so a lone client keeps today's speed.
    fn run_lane_worker(
        session: makepad_ai_llm::LlamaSession,
        model_id: String,
        rx: mpsc::Receiver<WorkerMsg>,
    ) {
        use super::{StallPolicy, TurnPhase};
        use makepad_ai_llm::{
            LaneEvent, LaneExecutor, LaneOutcome, LaneRequest, LaneScheduler, SlotPhase,
        };
        use std::time::Instant;

        struct JobLane {
            events: mpsc::Sender<WorkerEvent>,
            cancel: CancelToken,
            token_ids: Vec<i32>,
            streamed: String,
            max_tokens: usize,
            /// Prompt tokens this turn handed the scheduler. The denominator
            /// of its prefill meter, and the only place that number exists —
            /// the scheduler reports an ingest ONCE, when the whole prompt is
            /// in, which on a cold 34k-token prompt is 75 seconds after the
            /// question was asked.
            prompt_tokens: usize,
            /// The slot this turn landed on, when the worker could tell.
            ///
            /// The scheduler picks the slot and there is no event for
            /// "admitted", so the only way to learn it from outside is to
            /// submit one request and admit it on its own: the lane that
            /// lights up is that request's. When a backlog makes that
            /// ambiguous this stays `None`, the turn reports its phase without
            /// a token count, and the watchdog judges it on the box's clock —
            /// an honest gap rather than a guessed lane, because the watchdog
            /// CANCELS what it judges and cancelling a stranger's turn is
            /// worse than any stall.
            lane: Option<usize>,
            phase: TurnPhase,
            /// Tokens of this turn's prompt already ingested, as last seen.
            ingested: usize,
            /// The last stage string published for this turn. The sink takes
            /// the job-store mutex and wakes everything waiting on it, so a
            /// number that has not changed must not be republished.
            stage: String,
            /// When this turn last moved. What the watchdog measures.
            last_progress: Instant,
            /// How the lane's prompt was ingested: (tokens, resumed). The
            /// scheduler's verdict, not a guess — this is what tells a client
            /// its conversation stayed warm.
            warm: Option<(usize, bool)>,
            /// Tokens generated before `</think>` closed, i.e. tokens the user
            /// never sees.
            ///
            /// Counted because it is the difference between the rate the box
            /// achieves and the rate a person perceives, and nothing upstream
            /// can tell them apart: a turn that decodes at 96 tok/s of which
            /// two thirds are reasoning reads as ~30 tok/s from the client's
            /// seat, with a wait before ANY text appears. Without this number
            /// that looks exactly like a slow box.
            think_tokens: Option<usize>,
        }


        let lanes = lane_count();
        let table = match session.new_slot_table() {
            Ok(table) => table,
            Err(err) => {
                eprintln!("[llm-worker] lane table: {err:?}");
                return;
            }
        };
        let queue_max = std::env::var("MAKEPAD_ASSET_AI_LLM_QUEUE")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(8);
        // The scheduler chunks prefill with the same batch the session uses,
        // so the two cannot drift into a graph neither of them sized for.
        let scheduler = LaneScheduler::new(table, queue_max).with_prefill_chunk(PREFILL_BATCH);
        let advert_model = model_id.clone();
        let mut exec = LaneExecutor::new(session, scheduler, LlamaSamplingParams::default())
            .on_counts(move |counts| {
                let _ = &advert_model;
                crate::lane_advert::set_live(
                    counts.slots_claimed as u64,
                    counts.lanes_active as u64,
                );
            });

        let mut jobs: std::collections::HashMap<u64, JobLane> = std::collections::HashMap::new();
        let mut next_job: u64 = 1;
        // Read once for the life of the worker, so every turn on this box
        // samples the same way and a mid-session env change cannot make two
        // conversations disagree about what the model is.
        let (presence_penalty, frequency_penalty, penalty_last_n) = super::sampling_penalties();
        let stall = StallPolicy::from_env();
        // The box's own clock, for turns that have no lane of their own to be
        // judged on. It is fed by ANY movement anywhere — a chunk ingested on
        // any lane, a token produced on any lane, a lane retiring — because
        // that is exactly what a queued turn is waiting for. If this stops,
        // nothing on the box is moving and everything in flight is stuck
        // together, which is when a queued turn deserves an answer instead of
        // a wait that will never end.
        let mut box_progress = Instant::now();
        // Per-slot ingest position at the last boundary. The one signal that
        // separates a prefill that is working from one that has stopped, and
        // the scheduler emits no event for it.
        let mut ingested_at: Vec<usize> = vec![0; lanes];
        // Speculation counters at the last turn boundary, so each turn can
        // report its OWN acceptance rather than a running average since boot.
        //
        // Worth a line because acceptance is the whole economics of
        // speculation and it is invisible from outside: at the measured
        // 4-column verify cost, acceptance 0.9 is the difference between ~120
        // tok/s and ~70, and nothing above the session can tell which one the
        // box is getting. A rate that looks "a bit slow" and a draft head
        // feeding on the wrong hidden state look identical from the client.
        let mut spec_mark = exec.session().speculative_stats();
        // The geometry this box actually allocated, in the units a sizing
        // decision is made in. Reported rather than derived, because per-token
        // KV cost is a property of the MODEL's attention shape — how many
        // layers actually have attention, their head dims, the cache types —
        // and a hybrid model's is nothing like a full-attention model's.
        // Sizing a card from a guess is how a context knob becomes an
        // out-of-memory at load, on the box, after a swap.
        // Reported as "unknown" rather than 0 when the session cannot answer.
        // A zero-byte KV report reads as a fact, and sizing a card from it is
        // exactly the mistake this line exists to prevent.
        let geometry = match (
            exec.session().attention_cache_bytes(),
            exec.session().attention_cache_bytes_per_token(),
        ) {
            (Ok(bytes), Ok(per_token)) if bytes > 0 => format!(
                "attention KV {:.2} GiB ({per_token} B/token/lane)",
                bytes as f64 / (1024.0 * 1024.0 * 1024.0)
            ),
            _ => "attention KV unknown".to_string(),
        };
        eprintln!(
            "[llm-worker] batched worker: {lanes} lanes x {} tok, queue {queue_max}, \
             speculation depth {}, {geometry}, stall watchdog {}",
            context_per_lane(),
            exec.session().speculation_depth(),
            if stall.is_off() {
                "OFF".to_string()
            } else {
                format!("{}s", stall.budget().as_secs())
            },
        );

        loop {
            // Take new work. Block only when there is genuinely nothing to do,
            // so an idle box costs no CPU and a busy one never stalls.
            //
            // DRAIN FIRST, SUBMIT AFTER. The prefix hit below is only valid
            // for a turn that will decode ALONE, and a second message arriving
            // later in the same drain would take that away from a turn already
            // submitted with only its delta — which would then prefill the
            // delta at position 0 of a slot holding none of the history, and
            // answer a question it never saw. Knowing the whole drain before
            // deciding makes that unrepresentable.
            let was_idle = exec.is_idle();
            let mut arrivals: Vec<(ExpandJob, CancelToken, mpsc::Sender<WorkerEvent>)> = Vec::new();
            loop {
                let msg = if exec.is_idle() && arrivals.is_empty() {
                    match rx.recv() {
                        Ok(msg) => Some(msg),
                        Err(_) => return,
                    }
                } else {
                    match rx.try_recv() {
                        Ok(msg) => Some(msg),
                        Err(mpsc::TryRecvError::Empty) => None,
                        Err(mpsc::TryRecvError::Disconnected) => return,
                    }
                };
                let Some(WorkerMsg::Expand(job, cancel, events)) = msg else {
                    break;
                };
                arrivals.push((job, cancel, events));
            }
            // An idle box has no clock to run. The loop blocks in `recv` while
            // nothing is in flight, so without this the mark would still say
            // "last movement" from before a quiet night and the first turn of
            // the morning would read as stalled the instant it arrived.
            if was_idle {
                box_progress = Instant::now();
            }
            // A turn is alone only if the box had nothing and this drain
            // brought exactly one thing. Then `next_step` admits that one job
            // into slot 0 with an empty queue, which is precisely the
            // condition the executor tests for the session-native path.
            let alone = was_idle && arrivals.len() == 1;
            'arrival: for (job, cancel, events) in arrivals {
                {
                let tokens = match exec.session().vocab().tokenize(&job.prompt_text, true, true) {
                    Ok(mut tokens) => {
                        // ChatML already ends the turn; a gguf with
                        // add_eos_token would make the first decode EOS.
                        if tokens.last().copied() == exec.session().vocab().eos_token_id() {
                            tokens.pop();
                        }
                        tokens
                    }
                    Err(err) => {
                        let _ = events.send(WorkerEvent::Done(Err(format!("tokenize: {err:?}"))));
                        continue;
                    }
                };
                let id = next_job;
                next_job += 1;
                // COMPACT rather than overflow. A conversation that outgrows its
                // lane used to reach the first decode and come back as
                // "gguf format error: session context overflow" — a message
                // from four layers down, in a game chat, about a number the
                // player never chose. Dropping the oldest turns is what a chat
                // client would do, and the taught context is never a
                // candidate: it is who the assistant IS, not what it recalls.
                let budget = (context_per_lane() as usize).saturating_sub(super::DECODE_HEADROOM);
                let mut prompt_text = job.prompt_text.clone();
                let mut tokens = tokens;
                let mut dropped = 0usize;
                while tokens.len() > budget {
                    let Some(shorter) = super::drop_oldest_turn(&prompt_text) else {
                        break;
                    };
                    prompt_text = shorter;
                    dropped += 1;
                    tokens = match exec.session().vocab().tokenize(&prompt_text, true, true) {
                        Ok(mut t) => {
                            if t.last().copied() == exec.session().vocab().eos_token_id() {
                                t.pop();
                            }
                            t
                        }
                        Err(err) => {
                            let _ = events
                                .send(WorkerEvent::Done(Err(format!("tokenize: {err:?}"))));
                            continue 'arrival;
                        }
                    };
                }
                if tokens.len() > budget {
                    // Only reachable when the system block alone does not fit,
                    // which is a configuration problem and says so.
                    let _ = events.send(WorkerEvent::Done(Err(format!(
                        "this conversation's system prompt alone needs {} tokens, more than the \
                         {budget} a lane can hold (raise MAKEPAD_ASSET_AI_LLM_CONTEXT or lower \
                         MAKEPAD_ASSET_AI_LLM_LANES)",
                        tokens.len()
                    ))));
                    continue;
                }
                if dropped > 0 {
                    eprintln!(
                        "[llm-worker] turn {id}: compacted {dropped} oldest turn(s) to fit \
                         {} tokens in a {budget}-token budget",
                        tokens.len()
                    );
                }
                // The SCHEDULER owns prefix reuse now: it holds each lane's own
                // token history and matches against it. The worker's job is to
                // hand over the FULL prompt and get out of the way — sending a
                // delta here would be a second opinion about a lane's contents,
                // and the two disagreeing means ingesting a delta at position 0
                // of a lane that holds none of the history.
                let prompt_tokens = tokens.len();
                let request = LaneRequest {
                    job: id,
                    session: job.kind.clone(),
                    prompt_tokens: tokens,
                    // Never a veto from here: the scheduler decides whether a
                    // lane can be resumed, and it is the only thing that knows.
                    reset_first: false,
                    max_new: job.max_tokens.max(1) as usize,
                    sampling: LlamaSamplingParams {
                        temperature: job.temperature.max(0.0),
                        top_p: 0.95,
                        top_k: 0,
                        seed: job.seed,
                        presence_penalty,
                        frequency_penalty,
                        penalty_last_n,
                    },
                };
                // The queue this refuses on is the SESSION's, not the job
                // store's — the store already refused at admission with a 409
                // long before a turn could reach here, so this is the last
                // honest answer rather than the first. Say the numbers and say
                // to retry: a client that cannot tell "full right now" from
                // "broken" retries neither.
                if let Err(refused) = exec.scheduler().submit(request) {
                    let _ = events.send(WorkerEvent::Done(Err(format!(
                        "busy: all {lanes} lanes are serving and {queue_max} more turns are \
                         already waiting, so this turn was never started - retry it"
                    ))));
                    let _ = refused;
                    continue;
                }
                jobs.insert(
                    id,
                    JobLane {
                        events,
                        cancel,
                        token_ids: Vec::new(),
                        streamed: String::new(),
                        max_tokens: job.max_tokens.max(1) as usize,
                        prompt_tokens,
                        lane: None,
                        phase: TurnPhase::Waiting,
                        ingested: 0,
                        stage: String::new(),
                        last_progress: Instant::now(),
                        warm: None,
                        think_tokens: None,
                        // Only a turn that went in ALONE can leave the
                        // session's own state describing it. One that joined a
                        // batch decodes through a slot and leaves that state
                        // where it was, so it must not claim it.
                    },
                );
                }
            }

            // Cancellations are honoured at the boundary, never mid-step.
            let cancelled: Vec<u64> = jobs
                .iter()
                .filter(|(_, lane)| lane.cancel.is_cancelled())
                .map(|(id, _)| *id)
                .collect();
            for id in cancelled {
                exec.scheduler().cancel(id);
            }

            // Which lanes were taken before this step, so the ones the step
            // admits can be told apart from the ones it did not.
            let claimed_before: Vec<bool> = (0..lanes)
                .map(|index| exec.scheduler().is_lane_claimed(index))
                .collect();

            let events = match exec.step() {
                Ok(events) => events,
                Err(err) => {
                    // A step failure is not one job's problem: the batch is
                    // shared, so every lane in flight hears about it rather
                    // than hanging on a reply that will never come.
                    eprintln!("[llm-worker] step: {err}");
                    for (_, lane) in jobs.drain() {
                        let _ = lane.events.send(WorkerEvent::Done(Err(err.clone())));
                    }
                    continue;
                }
            };

            // Learn which lane each turn landed on.
            //
            // The scheduler picks the slot and emits no "admitted" event — only
            // `Prefilled`, which on a cold 34k-token prompt arrives 75 seconds
            // after the turn started, long after the meter needed to move. So
            // the mapping is read off the step instead: admission happens at
            // the start of a step and retirement at its end, so a lane cannot
            // be freed and re-taken inside one, and a lane that lit up across
            // this step took the work the step admitted.
            //
            // One lane lighting up while one turn is waiting is one fact. Two
            // of either is a guess, and this must not guess: the watchdog
            // CANCELS what it judges, and cancelling a stranger's turn is worse
            // than any stall. Read before the events below are applied, because
            // a prompt short enough to fit one chunk is admitted and prefilled
            // in the same step, and would otherwise have left the candidate set
            // before anyone looked at it.
            {
                let lit: Vec<usize> = (0..lanes)
                    .filter(|index| {
                        !claimed_before[*index] && exec.scheduler().is_lane_claimed(*index)
                    })
                    .collect();
                // Candidates are turns that have not been admitted yet, which
                // is what `Waiting` means — a turn that was admitted without
                // being attributed leaves the set the moment its prompt is in,
                // so one unattributable turn does not poison the mapping for
                // every turn after it.
                let waiting: Vec<u64> = jobs
                    .iter()
                    .filter(|(_, lane)| lane.phase == TurnPhase::Waiting)
                    .map(|(id, _)| *id)
                    .collect();
                if let ([index], [id]) = (lit.as_slice(), waiting.as_slice()) {
                    if let Some(lane) = jobs.get_mut(id) {
                        lane.lane = Some(*index);
                        lane.phase = TurnPhase::Prefill;
                        lane.last_progress = Instant::now();
                    }
                }
            }

            // Collect first, publish once. A step on the solo speculative path
            // returns a whole 24-token CHUNK at once, and a text snapshot is
            // the entire reply so far: publishing per token detokenises the
            // whole sequence 24 times per chunk, allocates 24 growing strings,
            // and takes the job-store mutex 24 times — each of which
            // `notify_all`s every worker waiting on it. One waiter today; one
            // per chat lane once the dispatcher lands, which is a thundering
            // herd per token. The single-lane worker has always published once
            // per chunk; this is the lane worker catching up to it.
            let mut touched: Vec<u64> = Vec::new();
            if !events.is_empty() {
                box_progress = Instant::now();
            }
            for event in events {
                match event {
                    LaneEvent::Prefilled { job, ingested, resumed } => {
                        eprintln!(
                            "[llm-worker] turn {job}: prefill {} {ingested} tok",
                            if resumed { "RESUMED, ingested only" } else { "cold, ingested" }
                        );
                        // A big COLD ingest on a box that thinks out loud is
                        // not a bug in the matcher, and the next person to read
                        // this log should not have to derive that again. The
                        // KV holds the reasoning; the client stores the reply
                        // without it; so the prompt cannot extend what the lane
                        // holds, and the divergence sits at the last token of
                        // the previous prompt — which is why the whole
                        // conversation is re-ingested rather than a tail of it.
                        if !resumed && ingested > 1024 {
                            warn_open_think_is_never_warm();
                        }
                        if let Some(lane) = jobs.get_mut(&job) {
                            lane.warm = Some((ingested, resumed));
                            // The prompt is in, so this turn is judged on the
                            // tokens it produces from here on.
                            lane.phase = TurnPhase::Decode;
                            lane.ingested = ingested;
                            lane.last_progress = Instant::now();
                            let _ = lane.events.send(WorkerEvent::Serving(
                                crate::backend::ServingUpdate::Prefill {
                                    tokens: ingested,
                                    resumed,
                                },
                            ));
                        }
                    }
                    LaneEvent::Token { job, token, produced } => {
                        let Some(lane) = jobs.get_mut(&job) else { continue };
                        lane.last_progress = Instant::now();
                        lane.token_ids.push(token);
                        let _ =
                            lane.events
                                .send(WorkerEvent::Token(produced as u32, lane.max_tokens as u32));
                        if !touched.contains(&job) {
                            touched.push(job);
                        }
                    }
                    LaneEvent::Finished { job, outcome, .. } => {
                        touched.retain(|id| *id != job);
                        let Some(lane) = jobs.remove(&job) else { continue };
                        let result = match outcome {
                            LaneOutcome::Cancelled => Err("cancelled".to_string()),
                            LaneOutcome::Complete => exec
                                .session()
                                .vocab()
                                .decode_tokens(&lane.token_ids)
                                .map_err(|e| format!("detokenize: {e:?}")),
                        };
                        // The final counts, once. The per-step updates stop at
                        // the last step, and a reply that ends in the same
                        // chunk its think block closed in would leave a client
                        // showing "0 visible" for a turn that had several.
                        // Sent before Done, which the receiver returns on, so
                        // channel order is what makes this arrive at all.
                        {
                            let total = lane.token_ids.len();
                            let think = lane.think_tokens.unwrap_or(0);
                            let _ = lane.events.send(WorkerEvent::Serving(
                                crate::backend::ServingUpdate::Think {
                                    think,
                                    visible: Some(total.saturating_sub(think)),
                                },
                            ));
                        }
                        let total = lane.token_ids.len();
                        if total == 0 {
                            // A turn that produced NOTHING is the one thing this
                            // log absolutely has to carry, and until now it was
                            // the one thing it dropped: the line below only
                            // printed when a think block had closed, so an empty
                            // reply left no trace at all and the client's
                            // "llm produced an empty expansion" had nothing on
                            // the box to match it against.
                            //
                            // It means the model sampled its end-of-turn token as
                            // the FIRST token after the prompt — it read the turn
                            // as already finished. A closed think block in the
                            // generation prefill is the known way to provoke
                            // that, so the mode is named here rather than left to
                            // be guessed.
                            eprintln!(
                                "[llm-worker] turn {job}: EMPTY REPLY - the model ended the turn \
                                 on its first token (think mode: {}). The client will report this \
                                 as an empty expansion.",
                                crate::protocol::chat_think_mode(),
                            );
                        } else if let Some(think) = lane.think_tokens {
                            eprintln!(
                                "[llm-worker] turn {job}: {total} tokens generated, {think} of \
                                 them inside <think> ({:.0}% never shown to the user), \
                                 {} visible",
                                think as f64 / total.max(1) as f64 * 100.0,
                                total.saturating_sub(think),
                            );
                        } else {
                            // No think block at all (brief mode, or a model that
                            // does not reason). Still say what came out: a turn
                            // with no line at all is indistinguishable from a
                            // turn that never happened.
                            eprintln!(
                                "[llm-worker] turn {job}: {total} tokens generated, all visible"
                            );
                        }
                        // The session's KV now holds prompt + reply + suffix,
                        // so the NEXT turn of this conversation is a delta
                        // prefill instead of the whole history again. Only for
                        // a turn that ran alone and completed: a cancelled or
                        // failed turn leaves the KV somewhere the committed
                        // text does not describe, and claiming it would hand
                        // the next turn a prefix that is not there.
                        {
                            let now = exec.session().speculative_stats();
                            if let (Some(now), Some(then)) = (now, spec_mark) {
                                let rounds = now.rounds.saturating_sub(then.rounds);
                                let drafted = now.drafted.saturating_sub(then.drafted);
                                let accepted = now.accepted.saturating_sub(then.accepted);
                                if rounds > 0 {
                                    let ms = |a: u64, b: u64| {
                                        a.saturating_sub(b) as f64 / rounds as f64 / 1e6
                                    };
                                    eprintln!(
                                        "[llm-worker] turn {job}: {rounds} rounds, {:.2} \
                                         tok/round, acceptance {:.2}, per round draft {:.1} ms \
                                         + verify {:.1} ms + catch-up {:.1} ms",
                                        (accepted + rounds) as f64 / rounds as f64,
                                        if drafted > 0 {
                                            accepted as f64 / drafted as f64
                                        } else {
                                            0.0
                                        },
                                        ms(now.draft_nanos, then.draft_nanos),
                                        ms(now.verify_nanos, then.verify_nanos),
                                        ms(now.catchup_nanos, then.catchup_nanos),
                                    );
                                }
                            }
                            spec_mark = now;
                        }
                        let _ = lane.events.send(WorkerEvent::Done(result));
                    }
                }
            }
            for job in touched {
                let Some(lane) = jobs.get_mut(&job) else { continue };
                // Decode the WHOLE sequence: byte-level BPE can split a
                // character across a chunk edge, so per-chunk decodes do not
                // concatenate cleanly.
                let Ok(decoded) = exec.session().vocab().decode_tokens(&lane.token_ids) else {
                    continue;
                };
                if lane.think_tokens.is_none() && decoded.contains("</think>") {
                    lane.think_tokens = Some(lane.token_ids.len());
                }
                // Reported every step, open block or not: a client showing
                // "thinking, N" needs N to move, and it needs to learn the
                // moment it stops.
                // A reply with NO think block at all is all visible. Reporting
                // the total as `think` with no `visible` would tell a client
                // the user saw none of it, and it would show "thinking" for a
                // turn that was answering the whole time.
                let opened = decoded.contains("<think>");
                let update = match (opened, lane.think_tokens) {
                    (_, Some(think)) => crate::backend::ServingUpdate::Think {
                        think,
                        visible: Some(lane.token_ids.len().saturating_sub(think)),
                    },
                    (true, None) => crate::backend::ServingUpdate::Think {
                        think: lane.token_ids.len(),
                        visible: None,
                    },
                    (false, None) => crate::backend::ServingUpdate::Think {
                        think: 0,
                        visible: Some(lane.token_ids.len()),
                    },
                };
                let _ = lane.events.send(WorkerEvent::Serving(update));
                if let Some(snapshot) = super::next_stream_snapshot(&lane.streamed, &decoded) {
                    lane.streamed = snapshot.clone();
                    let _ = lane.events.send(WorkerEvent::Text(snapshot));
                }
            }

            // What a turn ingesting its prompt looks like from outside.
            //
            // Read off the slot table, because that is the only thing that
            // moves during a prefill: the scheduler ingests in chunks and
            // reports ONCE, at the end, so a 34,074-token cold prompt used to
            // leave its job sitting on `starting`, at 2%, for 75 seconds. That
            // is indistinguishable from a hang, it was escalated as one, and
            // it was not one. `fill - cursor` is this TURN's ingest — the
            // cursor is where the slot stood when the turn took it, so a
            // resumed lane counts its delta and a cold one counts everything.
            let queued = exec.scheduler().queue_depth();
            // Box-wide first, and attribution-independent: whether a chunk
            // landed ANYWHERE is a different question from which turn it
            // belonged to, and the watchdog below needs the first answer even
            // when the second one is unknown.
            let mut ingest_moved = false;
            for index in 0..lanes {
                let fill = match exec.scheduler().slot(index).map(|s| (s.phase(), s.fill())) {
                    Some((SlotPhase::Prefilling { .. }, fill)) => fill,
                    _ => 0,
                };
                if fill > ingested_at[index] {
                    ingest_moved = true;
                }
                ingested_at[index] = fill;
            }
            for (_, lane) in jobs.iter_mut() {
                let ingesting = lane.lane.and_then(|index| {
                    exec.scheduler().slot(index).and_then(|slot| match slot.phase() {
                        SlotPhase::Prefilling { cursor } => {
                            Some((slot.fill().saturating_sub(cursor), cursor))
                        }
                        _ => None,
                    })
                });
                let stage = match ingesting {
                    Some((done, cursor)) => {
                        if done > lane.ingested {
                            lane.ingested = done;
                        }
                        // NOT "decode k/n", and it must never be mistaken for
                        // it: `libs/asset/chat/src/qwen.rs::parse_decode_tokens`
                        // reads a generated-token count off any stage that
                        // starts with `decode`, and a client meter is wired to
                        // it. This starts with `prefill`, so that parser sees
                        // `None` and the meter stays a decode meter.
                        format!(
                            "prefill {done}/{} tok",
                            lane.prompt_tokens.saturating_sub(cursor).max(done)
                        )
                    }
                    // No lane of its own to read. Say what is true anyway: with
                    // an empty scheduler queue every submitted turn is on a
                    // lane, so this one is ingesting and only its numerator is
                    // missing; with turns queued, name the queue. "Queued, not
                    // stuck" is the first thing anyone asks about a slow turn.
                    None if lane.phase == TurnPhase::Waiting => {
                        if queued == 0 {
                            "prefill".to_string()
                        } else {
                            format!("queued behind {queued} turn(s)")
                        }
                    }
                    None => continue,
                };
                if stage != lane.stage {
                    lane.stage = stage.clone();
                    let _ = lane.events.send(WorkerEvent::Stage(stage));
                }
            }
            // A prefill blocks every other lane BY DESIGN: `next_step` serves
            // any lane that still needs its prompt before it plans a decode
            // step at all, so a 34k-token ingest on lane 2 stops lanes 0 and 1
            // dead for its whole duration. A turn starved by that is not
            // stalled — it is waiting on work that is visibly happening — and
            // reaping it would kill the conversation that was behaving. So a
            // chunk landing anywhere feeds every turn's clock, and only a box
            // where NOTHING ingests and nothing decodes runs the clock down.
            if ingest_moved {
                let now = Instant::now();
                box_progress = now;
                for (_, lane) in jobs.iter_mut() {
                    lane.last_progress = now;
                }
            }

            // The watchdog. A turn that is not moving is holding a lane the
            // rest of the box needs, and the only thing worse than a stall is
            // a stall nobody is told about.
            let now = Instant::now();
            let stalled: Vec<(u64, super::Stalled)> = jobs
                .iter()
                .filter_map(|(id, lane)| {
                    // A turn with no lane of its own is judged on the box's
                    // clock: what it is waiting for is the turns ahead of it
                    // moving, and punishing it for their stall would kill the
                    // one turn that was behaving.
                    let clock = match lane.phase {
                        TurnPhase::Waiting => box_progress,
                        _ => lane.last_progress,
                    };
                    super::stall_verdict(lane.phase, clock, now, &stall)
                        .map(|verdict| (*id, verdict))
                })
                .collect();
            for (id, verdict) in stalled {
                let Some(lane) = jobs.remove(&id) else { continue };
                let on_lane = match lane.lane {
                    Some(index) => format!("lane {index}"),
                    None => "no lane (queued)".to_string(),
                };
                let progress = match lane.phase {
                    TurnPhase::Decode => {
                        format!("{} tokens generated", lane.token_ids.len())
                    }
                    _ => format!("{}/{} tok ingested", lane.ingested, lane.prompt_tokens),
                };
                // ONE line, and it names everything the next question needs:
                // which turn, which lane, which phase, how long, and how far it
                // had got. Written for somebody reading a box log at speed
                // while a fleet view shows a frozen row.
                eprintln!(
                    "[llm-worker] turn {id}: STALLED in {} on {on_lane} - no progress for \
                     {:.1}s ({progress}). Cancelling the turn and freeing the lane. Raise \
                     MAKEPAD_ASSET_AI_LLM_STALL_SECS (now {}s) if this box legitimately \
                     needs longer.",
                    verdict.phase.name(),
                    verdict.idle.as_secs_f64(),
                    stall.budget().as_secs(),
                );
                // Cancel FIRST, then answer. `cancel` marks the lane done in
                // whatever phase it is in — including a turn that never got
                // past its first prefill chunk — and `step` reaps every done
                // lane at the end of every step, so the slot is back before
                // the next turn is admitted.
                exec.scheduler().cancel(id);
                // A failure, not a cancellation: nobody asked for this and the
                // client has to be able to tell the two apart. Its `Finished`
                // event arrives a step later for a job that is no longer in the
                // map, and is ignored there.
                let _ = lane.events.send(WorkerEvent::Done(Err(format!(
                    "the box stopped making progress on this turn: nothing moved for {:.0}s \
                     during {} ({progress}). The lane has been freed - retry the turn.",
                    verdict.idle.as_secs_f64(),
                    verdict.phase.name(),
                ))));
            }
        }
    }

    /// Streamed back to the blocked caller while a job runs on the session
    /// thread: per-token progress, then exactly one Done.
    enum WorkerEvent {
        Stage(String),
        Token(u32, u32),
        /// Chat serving facts as they become known — warmth at prefill, the
        /// think split as the reply grows.
        Serving(crate::backend::ServingUpdate),
        /// Full assistant text so far — a monotonically growing,
        /// prefix-stable snapshot (never a delta), emitted once per decode
        /// chunk. Receivers replace, not append.
        Text(String),
        Done(Result<String, String>),
    }

    enum WorkerMsg {
        Expand(ExpandJob, CancelToken, mpsc::Sender<WorkerEvent>),
    }

    /// Owns the dedicated session thread. `LlamaSession` is `!Send`, so the
    /// session is built and used only on that thread; this handle is Send.
    /// The thread — and the resident weights — live for as long as the
    /// backend instance, which the server worker keeps across jobs.
    /// Clone is a second handle to the SAME resident session, not a second
    /// session: the struct is one channel to the thread that owns the weights.
    /// That is what lets several turns be in flight at once.
    #[derive(Clone)]
    pub struct LlamaWorker {
        tx: mpsc::Sender<WorkerMsg>,
    }

    impl LlamaWorker {
        pub fn spawn(
            gguf: PathBuf,
            model_id: String,
            progress: &mut dyn FnMut(&str, f64),
        ) -> Result<Self, String> {
            enum BootEvt {
                Progress(String, f64),
                Ready(Result<(), String>),
            }
            let (tx, rx) = mpsc::channel::<WorkerMsg>();
            let (boot_tx, boot_rx) = mpsc::channel::<BootEvt>();
            std::thread::Builder::new()
                .name("llm-expander".to_string())
                .spawn(move || {
                    let config = LlamaSessionConfig {
                        max_context: Some(context_per_lane()),
                        max_sequences: lane_count() as u32,
                        prefill_batch_size: PREFILL_BATCH,
                        // MTP speculative decoding (nextn draft head). 3 is
                        // the measured sweet spot on served chat (Blackwell
                        // A/B: n3 106.3 vs n5 101.3 tok/s full head, 121.6
                        // vs 119.8 restricted) and shortens the losing
                        // chains on low-acceptance expander prose; 8 crosses
                        // the MMVQ column cliff (see qwen38-mtp campaign).
                        // Models without an MTP block load exactly as before
                        // (draft head only loads when the gguf carries one),
                        // and the .draftvocab sidecar is picked up when it
                        // sits beside the gguf (full head otherwise).
                        spec_draft_max: 3,
                        ..LlamaSessionConfig::default()
                    };
                    let mut session = match LlamaSession::load_with_progress(
                        &gguf,
                        config,
                        &mut |stage, frac| {
                            let _ = boot_tx.send(BootEvt::Progress(stage.to_string(), frac));
                        },
                    ) {
                        Ok(session) => {
                            let _ = boot_tx.send(BootEvt::Ready(Ok(())));
                            session
                        }
                        Err(err) => {
                            let _ = boot_tx.send(BootEvt::Ready(Err(format!("{err:?}"))));
                            return;
                        }
                    };
                    // Advertise this box's decode capacity now the weights are
                    // resident. One lane today: `max_sequences` defaults to 1,
                    // so anything larger would be advertising capacity that
                    // does not exist. The number moves when the batched worker
                    // lands; the CONTRACT does not, which is the point of
                    // shipping the protocol once rather than in stages.
                    crate::lane_advert::publish(crate::lane_advert::LaneFacts::idle(
                        model_id.clone(),
                        lane_count() as u64,
                        u64::from(context_per_lane()),
                    ));
                    if lane_count() > 1 {
                        run_lane_worker(session, model_id.clone(), rx);
                    } else {
                        let mut prefix = super::PrefixCache::default();
                        while let Ok(WorkerMsg::Expand(job, cancel, events)) = rx.recv() {
                            // A turn occupies its lane for its duration.
                            // Claimed survives the turn — the conversation's KV
                            // stays resident and its next turn comes back here.
                            crate::lane_advert::lane_entered();
                            let result =
                                run_expand(&mut session, &mut prefix, &job, &cancel, &events);
                            crate::lane_advert::lane_left();
                            let _ = events.send(WorkerEvent::Done(result));
                        }
                    }
                    crate::lane_advert::clear();
                    // Sender dropped -> backend dropped: session unloads here.
                })
                .map_err(|e| format!("spawn llm worker: {e}"))?;
            loop {
                match boot_rx.recv() {
                    Ok(BootEvt::Progress(stage, frac)) => progress(&stage, frac),
                    Ok(BootEvt::Ready(result)) => {
                        result?;
                        break;
                    }
                    Err(_) => return Err("llm worker died during load".to_string()),
                }
            }
            Ok(Self { tx })
        }

        /// Blocks until the expansion finishes, forwarding per-token events
        /// into `on_token`. Cancellation: the shared token is checked on the
        /// session thread between generated tokens; the run then reports
        /// Err("cancelled").
        #[allow(clippy::too_many_arguments)]
        pub fn expand(
            &self,
            job: ExpandJob,
            cancel: CancelToken,
            on_token: &mut dyn FnMut(u32, u32),
            on_stage: &mut dyn FnMut(&str),
            on_text: &mut dyn FnMut(&str),
            on_serving: crate::backend::ServingSink,
        ) -> Result<String, String> {
            let (event_tx, event_rx) = mpsc::channel();
            self.tx
                .send(WorkerMsg::Expand(job, cancel, event_tx))
                .map_err(|_| "llm worker thread is gone".to_string())?;
            loop {
                match event_rx.recv() {
                    Ok(WorkerEvent::Stage(name)) => on_stage(&name),
                    Ok(WorkerEvent::Token(k, max)) => on_token(k, max),
                    Ok(WorkerEvent::Text(text)) => on_text(&text),
                    Ok(WorkerEvent::Serving(update)) => on_serving(update),
                    Ok(WorkerEvent::Done(result)) => return result,
                    Err(_) => return Err("llm worker dropped the reply".to_string()),
                }
            }
        }
    }

    fn run_expand(
        session: &mut LlamaSession,
        prefix: &mut super::PrefixCache,
        job: &ExpandJob,
        cancel: &CancelToken,
        events: &mpsc::Sender<WorkerEvent>,
    ) -> Result<String, String> {
        use super::PrefixOutcome;
        let cancelled = || "cancelled".to_string();
        // Chat turns send a prompt that extends the previous prompt+reply.
        // Reuse the live KV (weights already resident) and prefill only the
        // suffix. Independent expand jobs with a different prompt reset.
        const DECODE_RESERVE: usize = 128;
        let tokenize_prompt = |session: &LlamaSession, text: &str, add_special: bool| {
            let mut tokens = session
                .vocab()
                .tokenize(text, add_special, true)
                .map_err(|e| format!("tokenize: {e:?}"))?;
            // ChatML already ends the turn. A GGUF with add_eos_token=true
            // would append EOS here and the first decode token is then EOS
            // (empty expansion).
            if tokens.last().copied() == session.vocab().eos_token_id() {
                tokens.pop();
            }
            Ok::<Vec<i32>, String>(tokens)
        };
        let (outcome, owner) = prefix.classify(&job.kind, &job.prompt_text);
        let committed_len = prefix.committed().len();
        eprintln!(
            "[llm-worker] job start: session={} prefix={outcome:?} committed={committed_len}B \
             prompt={}B max_tokens={} temp={} suffix={:?}",
            owner.short(),
            job.prompt_text.len(),
            job.max_tokens,
            job.temperature,
            job.commit_suffix,
        );
        // Everything below charges its prefill to `outcome`: on a hit only the
        // delta is prefilled (real work), on an interleaved miss the entire
        // history is re-prefilled purely because another conversation took the
        // one resident KV (waste).
        let started = std::time::Instant::now();
        let mut prefilled = 0usize;
        // A context-full restart is a cold prefill however we got here: the
        // history no longer fits, so no slot count would have saved it.
        let mut outcome = outcome;
        if outcome == PrefixOutcome::Hit {
            let delta = job.prompt_text[committed_len..].to_string();
            if !delta.is_empty() {
                let extra = tokenize_prompt(session, &delta, false)?;
                if session.remaining_context() < extra.len() + DECODE_RESERVE {
                    session.reset().map_err(|e| format!("reset: {e:?}"))?;
                    prefix.invalidate();
                    outcome = PrefixOutcome::Cold;
                    let tokens = tokenize_prompt(session, &job.prompt_text, true)?;
                    prefilled = tokens.len();
                    let _ = events.send(WorkerEvent::Stage(format!(
                        "prefill {} tok (context full)",
                        tokens.len()
                    )));
                    session
                        .append_tokens_with_progress(&tokens, &mut |done, total| {
                            let _ = events.send(WorkerEvent::Stage(format!(
                                "prefill {done}/{total} tok (context full)"
                            )));
                        })
                        .map_err(|e| format!("prefill: {e:?}"))?;
                } else {
                    prefilled = extra.len();
                    session
                        .append_tokens_with_progress(&extra, &mut |done, total| {
                            let _ = events.send(WorkerEvent::Stage(format!(
                                "kv reuse {done}/{total} tok"
                            )));
                        })
                        .map_err(|e| format!("prefill-continue: {e:?}"))?;
                }
            } else {
                let _ = events.send(WorkerEvent::Stage("kv reuse".into()));
            }
        } else {
            session.reset().map_err(|e| format!("reset: {e:?}"))?;
            prefix.invalidate();
            let tokens = tokenize_prompt(session, &job.prompt_text, true)?;
            prefilled = tokens.len();
            // Say WHY in the stage string: a user watching a turn take three
            // seconds should see that it is a session switch, not the model.
            let why = if outcome == PrefixOutcome::Interleaved {
                " (session switch)"
            } else {
                ""
            };
            session
                .append_tokens_with_progress(&tokens, &mut |done, total| {
                    let _ = events
                        .send(WorkerEvent::Stage(format!("prefill {done}/{total} tok{why}")));
                })
                .map_err(|e| format!("prefill: {e:?}"))?;
        }
        prefix.record(outcome, &owner, prefilled, started.elapsed());
        if outcome == PrefixOutcome::Interleaved {
            eprintln!(
                "[llm-worker] prefix MISS (interleave): {} re-prefilled {prefilled} tok in \
                 {:.2}s because another conversation held the resident KV; {}",
                owner.short(),
                started.elapsed().as_secs_f64(),
                prefix.waste_report(),
            );
        }
        if cancel.is_cancelled() {
            return Err(cancelled());
        }

        // Chunked decode through the session's OWN generation loop.
        // `continue_sampled` dispatches to greedy at temperature 0 and runs
        // MTP speculative decoding (exact Leviathan/Chen rejection sampling)
        // whenever the model carries a draft head. The per-token
        // sample-from-raw-logits loop that used to live here silently
        // BYPASSED speculation: spec_draft_max was configured, the draft
        // head loaded, and the chat lane still decoded one token at a time
        // (measured 52.5 tok/s on the 27B — the non-speculative rate).
        // Chunks give the job protocol its three boundaries — cancel check,
        // progress event, partial-text snapshot — every ~4 speculative
        // rounds.
        let max = job.max_tokens.max(1) as usize;
        // The SAME penalties the lane worker applies. A box whose answers
        // depend on whether it happened to be configured for lanes is a
        // difference nobody can reproduce from the outside.
        let (presence_penalty, frequency_penalty, penalty_last_n) = super::sampling_penalties();
        let sampling = LlamaSamplingParams {
            temperature: job.temperature.max(0.0),
            top_p: 0.95,
            top_k: 0,
            seed: job.seed,
            presence_penalty,
            frequency_penalty,
            penalty_last_n,
        };
        const CHUNK: usize = 24;
        // ONE RNG stream for the whole reply. `continue_sampled` seeds from
        // `params.seed` per call, so the chunk loop below used to restart the
        // sampler every 24 tokens: token 24 drew the same uniform as token 0,
        // token 25 the same as token 1, and so on for the whole generation. On
        // a repetitive tail (where consecutive logit rows are near-identical)
        // that is a 24-token loop the sampler cannot escape. Seeding once and
        // carrying the state makes a chunked decode produce exactly what one
        // unchunked call would.
        let mut sampler = LlamaSamplerState::new(job.seed);
        let mut token_ids: Vec<i32> = Vec::new();
        let mut streamed = String::new();
        loop {
            if cancel.is_cancelled() {
                return Err(cancelled());
            }
            let want = (max - token_ids.len()).min(CHUNK);
            if want == 0 {
                break;
            }
            let generated = session
                .continue_sampled_with(want, sampling, &mut sampler)
                .map_err(|e| format!("decode: {e:?}"))?;
            token_ids.extend_from_slice(&generated.token_ids);
            let _ = events.send(WorkerEvent::Token(token_ids.len() as u32, max as u32));
            // Partial-text snapshots: decode the FULL sequence each chunk —
            // byte-level BPE can split one character across a chunk edge,
            // so per-chunk decodes do not concatenate cleanly — and only
            // publish while the previous snapshot is still a prefix; a
            // trailing incomplete character heals on the next chunk.
            if let Ok(decoded) = session.vocab().decode_tokens(&token_ids) {
                if let Some(snapshot) = super::next_stream_snapshot(&streamed, &decoded) {
                    streamed = snapshot.clone();
                    let _ = events.send(WorkerEvent::Text(snapshot));
                }
            }
            if generated.stop_reason != LlamaStopReason::MaxNewTokens {
                break;
            }
            if generated.token_ids.len() < want {
                // Short without a stop reason: the session cannot make
                // progress (context full); looping again would spin.
                break;
            }
        }
        let text = session
            .vocab()
            .decode_tokens(&token_ids)
            .map_err(|e| format!("detokenize: {e:?}"))?;
        let stats = prefix.stats();
        eprintln!(
            "[llm-worker] job end: decoded={} tok first={:?} eos={:?} session_tokens={} \
             prefix={}hit/{}cold/{}interleave",
            token_ids.len(),
            token_ids.first(),
            session.vocab().eos_token_id(),
            session.token_count(),
            stats.hits,
            stats.cold,
            stats.interleaved,
        );
        let mut committed = String::with_capacity(
            job.prompt_text.len() + text.len() + job.commit_suffix.len(),
        );
        committed.push_str(&job.prompt_text);
        committed.push_str(&text);
        committed.push_str(&job.commit_suffix);
        prefix.commit(&owner, committed);
        Ok(text)
    }

}

// ---------------------------------------------------------------------------
// Tests (stubbed generation — this is what CI exercises)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod compaction_tests {
    use super::{drop_oldest_turn, DECODE_HEADROOM};

    fn prompt(turns: &[(&str, &str)]) -> String {
        let mut out = String::from("<|im_start|>system\nTAUGHT<|im_end|>\n");
        for (role, text) in turns {
            out.push_str("<|im_start|>");
            out.push_str(role);
            out.push('\n');
            out.push_str(text);
            out.push_str("<|im_end|>\n");
        }
        out.push_str("<|im_start|>assistant\n<think>\n");
        out
    }

    #[test]
    fn compaction_drops_the_oldest_turn_and_keeps_the_taught_context() {
        // The system block is who the assistant IS, not what it remembers.
        // Dropping it to save room would change the character rather than its
        // memory, which is the one thing compaction must never do.
        let full = prompt(&[("user", "one"), ("assistant", "two"), ("user", "three")]);
        let once = drop_oldest_turn(&full).expect("a turn to drop");
        assert!(once.starts_with("<|im_start|>system\nTAUGHT<|im_end|>\n"));
        assert!(!once.contains("one"), "the oldest turn is gone");
        assert!(once.contains("two") && once.contains("three"));
        assert!(once.ends_with("<|im_start|>assistant\n<think>\n"));

        let twice = drop_oldest_turn(&once).expect("another turn to drop");
        assert!(!twice.contains("two"));
        assert!(twice.contains("three"));
    }

    #[test]
    fn compaction_stops_rather_than_eating_the_system_block() {
        // Nothing left but the taught context and the opener: there is no
        // turn to drop, and saying so lets the caller report a configuration
        // problem instead of looping.
        let bare = prompt(&[]);
        assert!(drop_oldest_turn(&bare).is_none());
        // One turn left, then none.
        let one = prompt(&[("user", "only")]);
        let dropped = drop_oldest_turn(&one).expect("one to drop");
        assert!(!dropped.contains("only"));
        assert!(drop_oldest_turn(&dropped).is_none());
    }

    #[test]
    fn compaction_never_eats_the_trailing_assistant_opener() {
        // The opener has no `<|im_end|>`, so a naive block walk would treat it
        // as the next droppable turn and hand the model a prompt with nothing
        // to continue.
        for turns in 0..4 {
            let built: Vec<(&str, &str)> = (0..turns).map(|_| ("user", "x")).collect();
            let mut text = prompt(&built);
            while let Some(next) = drop_oldest_turn(&text) {
                text = next;
            }
            assert!(
                text.ends_with("<|im_start|>assistant\n<think>\n"),
                "the opener must survive every drop"
            );
        }
    }

    #[test]
    fn the_decode_headroom_is_not_zero() {
        // A prompt that exactly fills the context leaves nowhere for the
        // reply, and the failure then lands at the first decode rather than at
        // admission where it can be handled.
        assert!(DECODE_HEADROOM >= 128);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::GenerateParams;
    use crate::protocol::GenerateRequestJson;

    fn params(prompt: &str, target_domain: &str) -> GenerateParams {
        let request = GenerateRequestJson {
            model: "qwen-test".to_string(),
            prompt: Some(prompt.to_string()),
            target_domain: Some(target_domain.to_string()),
            seed: Some(7),
            ..GenerateRequestJson::default()
        };
        GenerateParams::from_request(&request).unwrap()
    }

    #[test]
    fn system_prompts_pick_domain() {
        assert!(default_system_prompt("image").contains("Flux"));
        assert!(default_system_prompt("video").contains("shot"));
        assert!(default_system_prompt("mesh").contains("3D"));
        let rig = default_system_prompt("rig").to_lowercase();
        assert!(rig.contains("identity fidelity"));
        // The rig pose is allowed to separate these forms, never erase them.
        // This is deliberately character-agnostic, but covers the identity
        // regression found with a literal `yoshi` prompt.
        for defining_form in ["tails", "saddles", "shoes or boots", "gloves"] {
            assert!(rig.contains(defining_form), "missing {defining_form:?}");
        }
        // A narrow A-pose led to hand/hip fusion, while an extreme T-pose
        // repeatedly produced shoulder stretch under locomotion. Keep the
        // presentation in the generalized, reconstruction-safe middle.
        for pose_rule in [
            "relaxed wide a-pose",
            "35-45 degrees below shoulder height",
            "not a horizontal t-pose",
            "hands touching hips",
        ] {
            assert!(rig.contains(pose_rule), "missing {pose_rule:?}");
        }
        assert!(default_system_prompt("audio").contains("sound"));
        let music = default_system_prompt("music");
        assert!(music.contains("Lyrics:"));
        assert!(music.contains("[Verse]"));
        assert!(music.contains("[Chorus]"));
        assert!(music.contains("explicitly instrumental"));
        // Unknown domains fall back to the generic expander.
        assert_eq!(default_system_prompt("weird"), PROMPT_GENERIC);
    }

    #[test]
    fn prompt_override_file_wins() {
        let dir = std::env::temp_dir().join(format!(
            "ai-content-prompt-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("expand_image.txt"), "OVERRIDDEN IMAGE PROMPT").unwrap();
        assert_eq!(
            system_prompt_for("image", Some(&dir)),
            "OVERRIDDEN IMAGE PROMPT"
        );
        // No override file for video -> embedded default.
        assert_eq!(system_prompt_for("video", Some(&dir)), PROMPT_VIDEO);
        // Path-traversal-looking domains never touch the filesystem.
        assert_eq!(system_prompt_for("../evil", Some(&dir)), PROMPT_GENERIC);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn qwen38_leaves_think_open() {
        assert!(crate::protocol::model_uses_open_think("qwen3.8-27b"));
        assert!(!crate::protocol::model_uses_open_think("qwen3.6-27b"));
        assert_eq!(
            crate::protocol::think_prefill_for_model("qwen3.8-27b"),
            crate::protocol::CHAT_THINK_PREFILL_OPEN
        );
        let prompt = build_prompt_with_think(
            "SYS",
            &params("a pretty elf", "video"),
            crate::protocol::think_prefill_for_model("qwen3.8-27b"),
        );
        assert!(prompt.ends_with("<|im_start|>assistant\n<think>\n"));
        assert!(!prompt.contains("</think>"));
        let expand = crate::protocol::assemble_chat_prompt_with_think(
            "Target domain: video.\n\nSYS",
            &[crate::protocol::ChatMessageJson {
                role: "user".to_string(),
                text: "Intent: a pretty elf".to_string(),
            }],
            crate::protocol::think_prefill_for_model("qwen3.8-27b"),
        );
        assert!(expand.ends_with("<|im_start|>assistant\n<think>\n"));
        assert!(expand.contains("Intent: a pretty elf"));
    }

    #[test]
    fn chatml_prompt_shape() {
        let mut p = params("rusty pirate cannon", "mesh");
        p.style = "low-poly stylized".to_string();
        let prompt = build_prompt(default_system_prompt("mesh"), &p);
        assert!(prompt.starts_with("<|im_start|>system\n"));
        assert!(prompt.contains("<|im_end|>\n<|im_start|>user\nTarget domain: mesh\n"));
        assert!(prompt.contains("Style direction: low-poly stylized\n"));
        assert!(prompt.contains("Intent: rusty pirate cannon"));
        // Non-thinking assistant prefill closes the prompt on Qwen3/3.6.
        assert!(prompt.ends_with("<|im_start|>assistant\n<think>\n\n</think>\n\n"));
        let qwen38 = {
            let mut backend = LlmBackend::with_stub("qwen3.8-27b", Box::new(|_| Ok("x".into())));
            let mut sink = |_: &str, _: f64| {};
            let _ = backend.generate(&params("x", "video"), &mut sink, &CancelToken::new());
            crate::protocol::assemble_chat_prompt_with_think(
                &format!("Target domain: video.\n\n{}", default_system_prompt("video")),
                &[crate::protocol::ChatMessageJson {
                    role: "user".to_string(),
                    text: "Intent: x".to_string(),
                }],
                crate::protocol::think_prefill_for_model("qwen3.8-27b"),
            )
        };
        assert!(
            qwen38.ends_with("<|im_start|>assistant\n<think>\n"),
            "Qwen3.8 expander must leave <think> open, got {qwen38:?}"
        );
        // No style line when style is empty.
        let plain = build_prompt(default_system_prompt("mesh"), &params("x", "mesh"));
        assert!(!plain.contains("Style direction"));

        let mut anchored = params("yoshi", "rig");
        anchored.identity_anchor = "yoshi".to_string();
        let prompt = build_prompt(default_system_prompt("rig"), &anchored);
        assert!(prompt.contains(
            "Identity anchor (repeat this exact text verbatim in the answer): yoshi\n"
        ));
    }

    #[test]
    fn clean_expansion_strips_wrappers() {
        assert_eq!(clean_expansion("  a fine prompt \n"), "a fine prompt");
        assert_eq!(
            clean_expansion("<think>\nhmm\n</think>\n\nthe prompt"),
            "the prompt"
        );
        assert_eq!(clean_expansion("\"quoted prompt\""), "quoted prompt");
        let think_draft = "Let me draft:\n\n\"A red fox stands alert on moss, russet coat catching late afternoon light through the canopy, white-tipped tail curled, amber eyes watching the treeline.\"\n\nToo short, retry:\n\n\"A red fox stands alert on a mossy forest floor, its russet coat catching late afternoon light filtering through the canopy, white-tipped tail curling behind it, amber eyes fixed just beyond the frame.\"";
        assert!(clean_expansion(think_draft).starts_with("A red fox stands alert on a mossy forest floor"));
    }

    #[test]
    fn qwen38_expander_floors_token_budget() {
        let seen = std::sync::Arc::new(std::sync::Mutex::new(0u32));
        let seen_job = seen.clone();
        let mut backend = LlmBackend::with_stub(
            "qwen3.8-27b",
            Box::new(move |job: &ExpandJob| {
                *seen_job.lock().unwrap() = job.max_tokens;
                Ok("<think>\nplan\n</think>\n\na red fox in morning light".into())
            }),
        );
        let mut p = params("a red fox", "image");
        p.max_tokens = 64;
        let mut sink = |_: &str, _: f64| {};
        let artifacts = backend
            .generate(&p, &mut sink, &CancelToken::new())
            .unwrap();
        assert_eq!(*seen.lock().unwrap(), 512);
        assert_eq!(
            String::from_utf8(artifacts[0].bytes.clone()).unwrap(),
            "a red fox in morning light"
        );
    }

    #[test]
    fn stub_generation_single_variant() {
        let mut backend = LlmBackend::with_stub(
            "qwen-test",
            Box::new(|job: &ExpandJob| {
                // The stub sees the assembled ChatML prompt.
                assert!(job.prompt_text.contains("Intent: a red fox"));
                Ok(format!("expanded[a red fox seed={}]", job.seed))
            }),
        );
        let mut sink = |_: &str, _: f64| {};
        let artifacts = backend
            .generate(&params("a red fox", "image"), &mut sink, &CancelToken::new())
            .unwrap();
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0].content_type, "text/plain; charset=utf-8");
        assert_eq!(artifacts[0].ext, "txt");
        assert_eq!(
            String::from_utf8(artifacts[0].bytes.clone()).unwrap(),
            "expanded[a red fox seed=7]"
        );
    }

    #[test]
    fn stub_generation_variants_json() {
        let mut backend = LlmBackend::with_stub(
            "qwen-test",
            Box::new(|job: &ExpandJob| Ok(format!("variant-{}", job.seed))),
        );
        let mut p = params("castle", "image");
        p.variants = 3;
        p.temperature = 0.0;
        let mut sink = |_: &str, _: f64| {};
        let artifacts = backend.generate(&p, &mut sink, &CancelToken::new()).unwrap();
        assert_eq!(artifacts.len(), 2);
        assert_eq!(artifacts[1].content_type, "application/json");
        let json = String::from_utf8(artifacts[1].bytes.clone()).unwrap();
        assert!(json.contains("variant-7"));
        assert!(json.contains("variant-8"));
        assert!(json.contains("variant-9"));
        assert!(json.contains("\"target_domain\":\"image\""));
    }

    #[test]
    fn chat_request_skips_expander_and_is_prefix_stable() {
        use crate::protocol::{assemble_chat_prompt, ChatMessageJson, CHAT_COMMIT_SUFFIX};

        let turn1 = vec![ChatMessageJson {
            role: "user".into(),
            text: "hi".into(),
        }];
        let p1 = assemble_chat_prompt("SYS", &turn1);
        assert!(p1.contains("<|im_start|>user\nhi<|im_end|>"));
        assert!(!p1.contains("Intent:"));
        assert!(p1.ends_with("<|im_start|>assistant\n<think>\n\n</think>\n\n"));

        let turn2 = vec![
            ChatMessageJson { role: "user".into(), text: "hi".into() },
            ChatMessageJson { role: "assistant".into(), text: "hello".into() },
            ChatMessageJson { role: "user".into(), text: "second message".into() },
        ];
        let p2 = assemble_chat_prompt("SYS", &turn2);
        let committed = format!("{p1}hello{CHAT_COMMIT_SUFFIX}");
        assert!(
            p2.starts_with(&committed),
            "turn2 must extend committed KV prefix\ncommitted={committed:?}\np2={p2:?}"
        );

        let request = GenerateRequestJson {
            model: "qwen-test".to_string(),
            prompt: Some("hi".into()),
            domain: Some("chat".into()),
            chat_system: Some("SYS".into()),
            chat_messages: Some(turn1),
            seed: Some(7),
            ..GenerateRequestJson::default()
        };
        let parsed = GenerateParams::from_request(&request).unwrap();
        assert_eq!(parsed.target_domain, "chat");
        assert!(parsed.prompt.starts_with("<|im_start|>system\nSYS"));

        let mut backend = LlmBackend::with_stub(
            "qwen-test",
            Box::new(|job: &ExpandJob| {
                assert!(!job.prompt_text.contains("Intent:"));
                assert!(job.prompt_text.contains("<|im_start|>user\nhi"));
                assert_eq!(job.commit_suffix, CHAT_COMMIT_SUFFIX);
                Ok("hello".into())
            }),
        );
        let mut sink = |_: &str, _: f64| {};
        let artifacts = backend
            .generate(&parsed, &mut sink, &CancelToken::new())
            .unwrap();
        assert_eq!(
            String::from_utf8(artifacts[0].bytes.clone()).unwrap(),
            "hello"
        );
    }

    #[test]
    fn variant_temperatures_floor_after_first() {
        // Greedy first variant, sampled (>= 0.7) after — otherwise all
        // variants would be identical.
        let mut seen: Vec<f32> = Vec::new();
        let seen_ptr = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let inner = seen_ptr.clone();
        let mut backend = LlmBackend::with_stub(
            "qwen-test",
            Box::new(move |job: &ExpandJob| {
                inner.lock().unwrap().push(job.temperature);
                Ok(format!("v{}", job.seed))
            }),
        );
        let mut p = params("castle", "image");
        p.variants = 2;
        p.temperature = 0.0;
        let mut sink = |_: &str, _: f64| {};
        backend.generate(&p, &mut sink, &CancelToken::new()).unwrap();
        seen.extend(seen_ptr.lock().unwrap().iter().copied());
        assert_eq!(seen, vec![0.0, 0.7]);
    }

    #[test]
    fn pre_raised_cancel_unwinds_before_expansion() {
        let mut backend = LlmBackend::with_stub(
            "qwen-test",
            Box::new(|_: &ExpandJob| panic!("expansion must not run for a cancelled job")),
        );
        let cancel = CancelToken::new();
        cancel.cancel();
        let mut sink = |_: &str, _: f64| {};
        assert!(matches!(
            backend.generate(&params("a red fox", "image"), &mut sink, &cancel),
            Err(AssetAiError::Cancelled)
        ));
    }

    #[test]
    fn empty_intent_rejected() {
        let mut backend =
            LlmBackend::with_stub("qwen-test", Box::new(|_: &ExpandJob| Ok("x".to_string())));
        let mut sink = |_: &str, _: f64| {};
        assert!(backend.generate(&params("  ", "image"), &mut sink, &CancelToken::new()).is_err());
    }

    #[test]
    fn identity_anchor_is_checked_before_and_after_real_model_stage() {
        let mut called = false;
        let mut backend = LlmBackend::with_stub(
            "qwen-test",
            Box::new(|_: &ExpandJob| Ok("A generic green dinosaur in an A-pose".to_string())),
        );
        let mut p = params("yoshi", "rig");
        p.identity_anchor = "yoshi".to_string();
        let mut sink = |_: &str, _: f64| {
            called = true;
        };
        let error = match backend.generate(&p, &mut sink, &CancelToken::new()) {
            Ok(_) => panic!("identity-dropping expansion must fail"),
            Err(error) => error,
        };
        assert!(format!("{error}").contains("dropped identity anchor"));
        assert!(called, "the request reached the model stage; this is not a template check");

        let mut backend = LlmBackend::with_stub(
            "qwen-test",
            Box::new(|_: &ExpandJob| Ok("Yoshi, a green dinosaur, in an A-pose".to_string())),
        );
        assert!(backend
            .generate(&p, &mut |_, _| {}, &CancelToken::new())
            .is_ok());

        let mut mismatched = params("mario", "rig");
        mismatched.identity_anchor = "yoshi".to_string();
        let error = match backend.generate(&mismatched, &mut |_, _| {}, &CancelToken::new()) {
            Ok(_) => panic!("mismatched identity anchor must fail"),
            Err(error) => error,
        };
        assert!(format!("{error}").contains("must occur in the terse prompt"));
    }

    #[test]
    fn stream_snapshots_are_prefix_stable() {
        // Plain growth publishes the whole decode.
        assert_eq!(next_stream_snapshot("", "Hel").as_deref(), Some("Hel"));
        assert_eq!(
            next_stream_snapshot("Hel", "Hello wor").as_deref(),
            Some("Hello wor")
        );
        // No growth: hold.
        assert_eq!(next_stream_snapshot("Hello", "Hello"), None);
        // A chunk edge split a character: the decode ends in U+FFFD this
        // round. The unfinished tail is trimmed off the publish, so the
        // bad character can never enter the published stream...
        assert_eq!(
            next_stream_snapshot("", "caf\u{fffd}").as_deref(),
            Some("caf")
        );
        // ...trimming down to no growth holds the round entirely...
        assert_eq!(next_stream_snapshot("caf", "caf\u{fffd}"), None);
        // ...and the healed re-decode appends cleanly next round.
        assert_eq!(
            next_stream_snapshot("caf", "caf\u{e9} au lait").as_deref(),
            Some("caf\u{e9} au lait")
        );
        // A decode whose tail rewrote what was already published is held
        // back rather than corrupting receivers' byte offsets.
        assert_eq!(next_stream_snapshot("cafX", "caf\u{e9} au lait"), None);
    }

    #[test]
    fn generate_streamed_defaults_to_generate_without_text() {
        // The stub generator produces no incremental text; the streamed
        // entry point must still generate and must not touch the sink.
        let mut backend = LlmBackend::with_stub(
            "qwen-test",
            Box::new(|_: &ExpandJob| Ok("a quiet reply".to_string())),
        );
        let mut streamed: Vec<String> = Vec::new();
        let artifacts = backend
            .generate_streamed(
                &params("hello", "chat"),
                &mut |_, _| {},
                &mut |text| streamed.push(text.to_string()),
                &CancelToken::new(),
            )
            .expect("stub chat generates");
        assert_eq!(artifacts.len(), 1);
        assert_eq!(
            String::from_utf8(artifacts[0].bytes.clone()).unwrap(),
            "a quiet reply"
        );
        assert!(
            streamed.is_empty(),
            "stub path streams nothing; only the llama worker emits snapshots"
        );
    }

    // -----------------------------------------------------------------------
    // KV prefix cache
    // -----------------------------------------------------------------------

    fn chat_turns(system: &str, turns: &[(&str, &str)]) -> Vec<String> {
        // Prefix-stable ChatML, exactly as `assemble_chat_prompt` builds it:
        // prompt_n + reply + CHAT_COMMIT_SUFFIX is a prefix of prompt_{n+1}.
        let mut out = Vec::new();
        let mut base = format!("<|im_start|>system\n{system}<|im_end|>\n");
        for (user, reply) in turns {
            base.push_str(&format!("<|im_start|>user\n{user}<|im_end|>\n"));
            out.push(format!("{base}<|im_start|>assistant\n"));
            base.push_str(&format!("<|im_start|>assistant\n{reply}<|im_end|>\n"));
        }
        out
    }

    #[test]
    fn a_conversation_keeps_one_identity_across_its_turns() {
        let turns = chat_turns("SYS", &[("hello", "hi"), ("more", "sure"), ("again", "ok")]);
        let first = PrefixOwner::new("chat", &turns[0]);
        for turn in &turns[1..] {
            assert_eq!(PrefixOwner::new("chat", turn), first, "{turn:?}");
        }
        // A different opening user turn is a different conversation...
        let other = chat_turns("SYS", &[("goodbye", "bye")]);
        assert_ne!(PrefixOwner::new("chat", &other[0]), first);
        // ...and so is the same text under a different ChatML family.
        assert_ne!(PrefixOwner::new("expand:image", &turns[0]), first);
    }

    /// Turns of ONE conversation hit; that is the case the cache was built for
    /// and it must not regress.
    #[test]
    fn consecutive_turns_of_one_conversation_reuse_the_kv() {
        let turns = chat_turns("SYS", &[("hello", "hi"), ("more", "sure")]);
        let mut cache = PrefixCache::default();

        let (outcome, owner) = cache.classify("chat", &turns[0]);
        assert_eq!(outcome, PrefixOutcome::Cold);
        cache.record(outcome, &owner, 40, std::time::Duration::ZERO);
        cache.commit(&owner, format!("{}hi<|im_end|>\n", turns[0]));

        let (outcome, _) = cache.classify("chat", &turns[1]);
        assert_eq!(outcome, PrefixOutcome::Hit);
        assert_eq!(cache.stats().interleaved, 0);
    }

    /// The §2 finding, as a test: two chats alternating on one worker miss on
    /// every alternation, and the miss is attributed to the interleave rather
    /// than counted as an ordinary cold prefill.
    #[test]
    fn two_interleaved_conversations_miss_on_every_alternation() {
        let a = chat_turns("SYS", &[("a1", "ra1"), ("a2", "ra2"), ("a3", "ra3")]);
        let b = chat_turns("SYS", &[("b1", "rb1"), ("b2", "rb2"), ("b3", "rb3")]);
        let mut cache = PrefixCache::default();

        let mut serve = |cache: &mut PrefixCache, prompt: &str, reply: &str| {
            let (outcome, owner) = cache.classify("chat", prompt);
            cache.record(outcome, &owner, 4000, std::time::Duration::from_millis(1300));
            cache.commit(&owner, format!("{prompt}{reply}<|im_end|>\n"));
            outcome
        };

        // a1, b1: both cold — nothing was evicted, these are first turns.
        assert_eq!(serve(&mut cache, &a[0], "ra1"), PrefixOutcome::Cold);
        assert_eq!(serve(&mut cache, &b[0], "rb1"), PrefixOutcome::Cold);
        // From here every turn re-prefills a history that WAS resident.
        for (prompt, reply) in [
            (&a[1], "ra2"),
            (&b[1], "rb2"),
            (&a[2], "ra3"),
            (&b[2], "rb3"),
        ] {
            assert_eq!(serve(&mut cache, prompt, reply), PrefixOutcome::Interleaved);
        }
        let stats = cache.stats();
        assert_eq!(stats.hits, 0, "alternation never hits");
        assert_eq!(stats.cold, 2);
        assert_eq!(stats.interleaved, 4);
        assert_eq!(stats.interleaved_tokens, 16_000);
        assert_eq!(stats.interleaved_millis, 5_200);
        assert!(cache.waste_report().contains("4 turns"), "{}", cache.waste_report());

        // The SAME six turns served one conversation at a time: 2 cold
        // prefills and 4 hits. Identical work, different order.
        let mut serial = PrefixCache::default();
        for (turns, replies) in [(&a, ["ra1", "ra2", "ra3"]), (&b, ["rb1", "rb2", "rb3"])] {
            for (prompt, reply) in turns.iter().zip(replies) {
                assert_eq!(serve(&mut serial, prompt, reply) == PrefixOutcome::Hit, prompt != &turns[0]);
            }
        }
        assert_eq!(serial.stats().hits, 4);
        assert_eq!(serial.stats().interleaved, 0);
        assert_eq!(serial.stats().cold, 2);
    }

    /// A different ChatML family must never extend the resident prefix, even
    /// when the bytes happen to line up (the expand->chat contamination path).
    #[test]
    fn a_different_chatml_family_never_extends_the_prefix() {
        let mut cache = PrefixCache::default();
        let owner = PrefixOwner::new("expand:image", "<|im_start|>user\nx<|im_end|>\n");
        cache.commit(&owner, "<|im_start|>user\nx<|im_end|>\n".to_string());
        let (outcome, _) = cache.classify("chat", "<|im_start|>user\nx<|im_end|>\nmore");
        assert_eq!(outcome, PrefixOutcome::Cold);
    }

    #[test]
    fn a_prompt_without_a_complete_user_turn_still_has_an_identity() {
        // Single-shot expander prompts may not carry a closed user turn.
        let owner = PrefixOwner::new("expand:mesh", "Intent: a rusty cannon");
        assert_eq!(owner, PrefixOwner::new("expand:mesh", "Intent: a rusty cannon"));
        assert_ne!(owner, PrefixOwner::new("expand:mesh", "Intent: a shiny cannon"));
    }
}
