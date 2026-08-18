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
    let text = text.split("</think>").last().unwrap_or(text).trim();
    let text = text
        .strip_prefix('"')
        .and_then(|t| t.strip_suffix('"'))
        .unwrap_or(text);
    text.trim().to_string()
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

    /// Runs one expansion; `on_token(k, max)` fires per generated token
    /// (real path only) and `cancel` is checked between tokens.
    fn expand(
        &mut self,
        job: &ExpandJob,
        cancel: &CancelToken,
        on_token: &mut dyn FnMut(u32, u32),
        on_stage: &mut dyn FnMut(&str),
    ) -> Result<String, AssetAiError> {
        match &mut self.generator {
            Generator::Stub(generate) => {
                cancel.check()?;
                let _ = (&on_token, &on_stage);
                generate(job)
            }
            #[cfg(feature = "llm")]
            Generator::Llama(worker) => {
                let worker = worker.as_ref().ok_or_else(|| {
                    AssetAiError::Backend("llm backend used before ensure_loaded".to_string())
                })?;
                worker
                    .expand(job.clone(), cancel.clone(), on_token, on_stage)
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
                        llama_worker::LlamaWorker::spawn(gguf.clone(), ctx.progress)
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
            let mut system = system_prompt_for(&params.target_domain, self.prompts_dir.as_deref());
            if crate::protocol::model_uses_open_think(&self.model_id) {
                system = format!(
                    "{}\n\n{system}",
                    crate::protocol::QWEN38_LOW_EFFORT
                );
            }
            build_prompt_with_think(
                &system,
                params,
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
                max_tokens: if is_chat {
                    params.max_tokens
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
            let raw = self.expand(&job, cancel, &mut on_token, &mut on_stage)?;
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
    use makepad_ai_llm::{LlamaSession, LlamaSessionConfig};
    use std::path::PathBuf;
    use std::sync::mpsc;

    /// Prompt + expansion both fit comfortably; keeps the KV cache small
    /// instead of sized for the model's native 262k window.
    const MAX_CONTEXT: u32 = 8192;
    /// Batched prefill: measured 350-600 tok/s vs ~28 tok/s at batch 1 on
    /// the 9B (see libs/converse qwen_filter.rs).
    const PREFILL_BATCH: usize = 64;

    /// Streamed back to the blocked caller while a job runs on the session
    /// thread: per-token progress, then exactly one Done.
    enum WorkerEvent {
        Stage(String),
        Token(u32, u32),
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
                    let mut committed = String::new();
                    while let Ok(WorkerMsg::Expand(job, cancel, events)) = rx.recv() {
                        let result =
                            run_expand(&mut session, &mut committed, &job, &cancel, &events);
                        let _ = events.send(WorkerEvent::Done(result));
                    }
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
        ) -> Result<String, String> {
            let (event_tx, event_rx) = mpsc::channel();
            self.tx
                .send(WorkerMsg::Expand(job, cancel, event_tx))
                .map_err(|_| "llm worker thread is gone".to_string())?;
            loop {
                match event_rx.recv() {
                    Ok(WorkerEvent::Stage(name)) => on_stage(&name),
                    Ok(WorkerEvent::Token(k, max)) => on_token(k, max),
                    Ok(WorkerEvent::Done(result)) => return result,
                    Err(_) => return Err("llm worker dropped the reply".to_string()),
                }
            }
        }
    }

    fn run_expand(
        session: &mut LlamaSession,
        committed: &mut String,
        job: &ExpandJob,
        cancel: &CancelToken,
        events: &mpsc::Sender<WorkerEvent>,
    ) -> Result<String, String> {
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
        let reuse = !committed.is_empty() && job.prompt_text.starts_with(committed.as_str());
        if reuse {
            let delta = &job.prompt_text[committed.len()..];
            if !delta.is_empty() {
                let extra = tokenize_prompt(session, delta, false)?;
                if session.remaining_context() < extra.len() + DECODE_RESERVE {
                    session.reset().map_err(|e| format!("reset: {e:?}"))?;
                    committed.clear();
                    let tokens = tokenize_prompt(session, &job.prompt_text, true)?;
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
            committed.clear();
            let tokens = tokenize_prompt(session, &job.prompt_text, true)?;
            session
                .append_tokens_with_progress(&tokens, &mut |done, total| {
                    let _ = events.send(WorkerEvent::Stage(format!("prefill {done}/{total} tok")));
                })
                .map_err(|e| format!("prefill: {e:?}"))?;
        }
        if cancel.is_cancelled() {
            return Err(cancelled());
        }

        // One decode loop for both modes: greedy uses the session's own
        // per-token step (same stop conditions as continue_greedy), sampled
        // mode picks from raw logits — either way the loop yields per-token
        // progress events and a cancel boundary between tokens.
        let max = job.max_tokens.max(1);
        let sampled = job.temperature > 0.0;
        let eos = session.vocab().eos_token_id();
        let pad = session.vocab().padding_token_id();
        let mut rng = Xorshift64::new(job.seed);
        let mut token_ids: Vec<i32> = Vec::new();
        for k in 0..max {
            if cancel.is_cancelled() {
                return Err(cancelled());
            }
            if sampled {
                let next = {
                    let logits = session
                        .last_logits()
                        .ok_or_else(|| "no logits after prefill".to_string())?;
                    sample_top_p(logits, job.temperature, 0.95, &mut rng)
                };
                if Some(next) == eos || Some(next) == pad {
                    break;
                }
                session
                    .append_token(next)
                    .map_err(|e| format!("decode: {e:?}"))?;
                token_ids.push(next);
            } else {
                match session
                    .next_greedy_token()
                    .map_err(|e| format!("decode: {e:?}"))?
                {
                    Some(next) => token_ids.push(next),
                    None => break,
                }
            }
            let _ = events.send(WorkerEvent::Token(k + 1, max));
        }
        let text = session
            .vocab()
            .decode_tokens(&token_ids)
            .map_err(|e| format!("detokenize: {e:?}"))?;
        committed.clear();
        committed.push_str(&job.prompt_text);
        committed.push_str(&text);
        committed.push_str(&job.commit_suffix);
        Ok(text)
    }

    struct Xorshift64 {
        state: u64,
    }

    impl Xorshift64 {
        fn new(seed: u64) -> Self {
            Self {
                // splitmix-style scramble; never zero.
                state: seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1,
            }
        }

        fn next_f64(&mut self) -> f64 {
            let mut x = self.state;
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            self.state = x;
            (x >> 11) as f64 / (1u64 << 53) as f64
        }
    }

    /// Temperature + top-p (nucleus) sampling over raw logits.
    fn sample_top_p(logits: &[f32], temperature: f32, top_p: f64, rng: &mut Xorshift64) -> i32 {
        let inv_temp = 1.0 / temperature.max(1e-4);
        let max_logit = logits.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let mut candidates: Vec<(i32, f64)> = logits
            .iter()
            .enumerate()
            .map(|(id, &logit)| {
                (
                    id as i32,
                    (((logit - max_logit) * inv_temp) as f64).exp(),
                )
            })
            .collect();
        candidates.sort_unstable_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        let total: f64 = candidates.iter().map(|(_, w)| w).sum();
        if total <= 0.0 {
            return candidates.first().map(|(id, _)| *id).unwrap_or(0);
        }
        // Nucleus: keep the smallest prefix with cumulative mass >= top_p.
        let mut kept = 0;
        let mut mass = 0.0;
        for (index, (_, weight)) in candidates.iter().enumerate() {
            mass += weight / total;
            kept = index + 1;
            if mass >= top_p {
                break;
            }
        }
        candidates.truncate(kept.max(1));
        let kept_total: f64 = candidates.iter().map(|(_, w)| w).sum();
        let mut pick = rng.next_f64() * kept_total;
        for (id, weight) in &candidates {
            pick -= weight;
            if pick <= 0.0 {
                return *id;
            }
        }
        candidates.last().map(|(id, _)| *id).unwrap_or(0)
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
            build_prompt_with_think(
                default_system_prompt("video"),
                &params("x", "video"),
                crate::protocol::think_prefill_for_model("qwen3.8-27b"),
            )
        };
        assert!(
            qwen38.ends_with("<|im_start|>assistant\n<think>\n"),
            "Qwen3.8 must leave <think> open, got {qwen38:?}"
        );
        assert!(!qwen38.contains("</think>"));
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
}
