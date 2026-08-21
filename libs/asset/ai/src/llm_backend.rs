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

    /// Runs one expansion; `on_token(k, max)` fires per decode chunk,
    /// `on_text` receives prefix-stable full-text snapshots (real path
    /// only) and `cancel` is checked between chunks.
    fn expand(
        &mut self,
        job: &ExpandJob,
        cancel: &CancelToken,
        on_token: &mut dyn FnMut(u32, u32),
        on_stage: &mut dyn FnMut(&str),
        on_text: &mut dyn FnMut(&str),
    ) -> Result<String, AssetAiError> {
        match &mut self.generator {
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
                worker
                    .expand(job.clone(), cancel.clone(), on_token, on_stage, on_text)
                    .map_err(|e| {
                        if e == "cancelled" {
                            AssetAiError::Cancelled
                        } else {
                            AssetAiError::Backend(format!("llm: {e}"))
                        }
                    })
            }
        }
    }
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
                system_prompt_for(&params.target_domain, self.prompts_dir.as_deref())
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
                crate::protocol::think_prefill_for_model(&self.model_id),
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
                    if crate::protocol::model_uses_open_think(&self.model_id) {
                        params.max_tokens.max(128)
                    } else {
                        params.max_tokens
                    }
                } else if crate::protocol::model_uses_open_think(&self.model_id) {
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
                self.expand(&job, cancel, &mut on_token, &mut on_stage, &mut on_text_variant)?;
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
                model: self.model_id.clone(),
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
    const PREFILL_BATCH: usize = 64;

    /// Streamed back to the blocked caller while a job runs on the session
    /// thread: per-token progress, then exactly one Done.
    enum WorkerEvent {
        Stage(String),
        Token(u32, u32),
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
                        max_context: Some(MAX_CONTEXT),
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
                        1,
                        u64::from(MAX_CONTEXT),
                    ));
                    let mut prefix = super::PrefixCache::default();
                    while let Ok(WorkerMsg::Expand(job, cancel, events)) = rx.recv() {
                        // A turn occupies its lane for its duration. Claimed
                        // survives the turn — the conversation's KV stays
                        // resident and its next turn should come back here.
                        crate::lane_advert::lane_entered();
                        let result = run_expand(&mut session, &mut prefix, &job, &cancel, &events);
                        crate::lane_advert::lane_left();
                        let _ = events.send(WorkerEvent::Done(result));
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
        pub fn expand(
            &self,
            job: ExpandJob,
            cancel: CancelToken,
            on_token: &mut dyn FnMut(u32, u32),
            on_stage: &mut dyn FnMut(&str),
            on_text: &mut dyn FnMut(&str),
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
        let sampling = LlamaSamplingParams {
            temperature: job.temperature.max(0.0),
            top_p: 0.95,
            top_k: 0,
            seed: job.seed,
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
