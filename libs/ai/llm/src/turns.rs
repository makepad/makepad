//! Single-turn chat on a local model: one fresh `system + user` turn per
//! request, streamed, with the system prompt's state kept so a repeated
//! system prompt is prefilled once (a translator sends the same long system
//! message with every short text).
//!
//! The session is `!Send`, so it lives on a worker thread of its own; the
//! caller talks to it over channels and is woken through a hook. Turns are
//! answered newest-first: a request replaced while queued is skipped, and a
//! running one stops after its current decode chunk when cancelled.

use crate::{preprocess_rgb8, LlamaSession, LlamaSessionConfig, LlamaSessionState, LlamaStopReason, VisionTower};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::Instant;

pub type Wake = Arc<dyn Fn() + Send + Sync>;

#[derive(Clone, Debug)]
pub struct TurnConfig {
    pub model: PathBuf,
    /// The context one turn (system + user + reply) must fit in.
    pub max_context: u32,
    pub prefill_batch_size: usize,
    /// Maximum MTP draft depth; 0 keeps ordinary greedy decoding.
    pub spec_draft_max: usize,
    /// System prompts whose state is kept (least recently used dropped).
    pub kept_prefixes: usize,
    /// The vision tower (mmproj GGUF) for turns with an image; loaded at the
    /// first such turn.
    pub mmproj: Option<PathBuf>,
}

impl TurnConfig {
    pub fn new(model: PathBuf) -> Self {
        TurnConfig { model, max_context: 2048, prefill_batch_size: 128, spec_draft_max: 0, kept_prefixes: 2, mmproj: None }
    }
}

#[derive(Clone, Debug)]
pub struct TurnRequest {
    pub id: u64,
    pub system: String,
    pub user: String,
    /// A picture the user part refers to (placed before the user text).
    pub image: Option<TurnImage>,
    pub max_new_tokens: usize,
}

/// Interleaved RGB8 pixels. Each new size compiles a vision graph once
/// (two sizes are kept): send camera frames at one fixed size.
#[derive(Clone, Debug)]
pub struct TurnImage {
    pub rgb: Vec<u8>,
    pub width: usize,
    pub height: usize,
}

#[derive(Clone, Debug)]
pub struct TurnStats {
    /// Tokens prefilled for this turn (the user part, plus the system
    /// prompt when it was not kept).
    pub prefill_tokens: usize,
    pub prefill_ms: f64,
    pub tokens: usize,
    pub decode_ms: f64,
    pub prefix_reused: bool,
    /// Vision tower time of an image turn (incl. its first-use load).
    pub vision_ms: f64,
}

#[derive(Clone, Debug)]
pub enum TurnEvent {
    /// The model is resident: which device runs it and how long loading took.
    Ready { device: String, load_ms: f64 },
    LoadFailed { error: String },
    /// The reply so far (raw model text; think blocks are the caller's).
    Partial { id: u64, text: String },
    Done { id: u64, text: String, stats: TurnStats },
    /// A system prompt from `prepare` is kept; `prefill_ms` 0 when it already was.
    Prepared { prefill_ms: f64 },
    PrepareFailed { error: String },
    Failed { id: u64, error: String },
}

enum Job {
    Turn(TurnRequest),
    /// Prefill and keep a system prompt before its first turn.
    Prepare(String),
    /// Drop the vision tower (it loads again at the next image turn).
    ReleaseVision,
}

pub struct TurnEngine {
    /// None once dropped: the worker sees the channel close and ends.
    requests: Option<Sender<Job>>,
    events: Receiver<TurnEvent>,
    latest: Arc<AtomicU64>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl TurnEngine {
    /// Loads the model on a worker thread; `Ready` or `LoadFailed` follows.
    pub fn start(config: TurnConfig, wake: Option<Wake>) -> TurnEngine {
        let (requests, request_rx) = channel::<Job>();
        let (event_tx, events) = channel::<TurnEvent>();
        let latest = Arc::new(AtomicU64::new(0));
        let worker_latest = latest.clone();
        let worker = std::thread::Builder::new()
            .name("llm-turns".into())
            .spawn(move || worker(config, request_rx, event_tx, worker_latest, wake))
            .expect("spawn llm-turns worker");
        TurnEngine { requests: Some(requests), events, latest, worker: Some(worker) }
    }

    /// Answer `request`; whatever ran before is abandoned.
    pub fn submit(&self, request: TurnRequest) {
        self.latest.store(request.id, Ordering::SeqCst);
        if let Some(requests) = &self.requests {
            let _ = requests.send(Job::Turn(request));
        }
    }

    /// Frees the vision tower's memory (~0.7 GB on the phone) when the app
    /// leaves the camera; the next image turn loads it again (~1 s from the
    /// page cache).
    pub fn release_vision(&self) {
        if let Some(requests) = &self.requests {
            let _ = requests.send(Job::ReleaseVision);
        }
    }

    /// Prefill `system` now (e.g. right after `Ready`, while the user is
    /// still speaking) so its first turn starts at the user part. A
    /// `Prepared` event follows.
    pub fn prepare(&self, system: String) {
        if let Some(requests) = &self.requests {
            let _ = requests.send(Job::Prepare(system));
        }
    }

    /// Abandon the running turn (it stops at its next token).
    pub fn cancel(&self, next_id: u64) {
        self.latest.store(next_id, Ordering::SeqCst);
    }

    pub fn poll(&self) -> Vec<TurnEvent> {
        self.events.try_iter().collect()
    }
}

/// Stops the running turn and waits for the worker to release the model:
/// GPU objects must not outlive the engine into process exit (a driver
/// tearing down under a still-running thread crashes).
impl Drop for TurnEngine {
    fn drop(&mut self) {
        self.latest.store(u64::MAX, Ordering::SeqCst);
        self.requests = None;
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

/// The Qwen chat rendering of a single turn: system part, then user part
/// with the assistant header and an empty think block (no reasoning).
fn system_part(system: &str) -> String {
    format!("<|im_start|>system\n{system}<|im_end|>\n")
}

fn user_part(user: &str) -> String {
    format!("<|im_start|>user\n{user}<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n")
}

/// An image turn's user part around the image embeddings.
const IMAGE_USER_START: &str = "<|im_start|>user\n<|vision_start|>";

fn image_user_end(user: &str) -> String {
    format!("<|vision_end|>{user}<|im_end|>\n<|im_start|>assistant\n<think>\n\n</think>\n\n")
}

struct Worker {
    session: LlamaSession,
    /// The state of the empty session, to start a new system prompt from.
    empty: LlamaSessionState,
    /// Kept system prompts, most recently used last.
    kept: VecDeque<(String, LlamaSessionState)>,
    kept_max: usize,
    mmproj: Option<PathBuf>,
    tower: Option<VisionTower>,
}

fn worker(config: TurnConfig, requests: Receiver<Job>, events: Sender<TurnEvent>, latest: Arc<AtomicU64>, wake: Option<Wake>) {
    let send = |e: TurnEvent| {
        let _ = events.send(e);
        if let Some(w) = &wake {
            w();
        }
    };
    let t0 = Instant::now();
    let session_config = LlamaSessionConfig {
        max_context: Some(config.max_context),
        prefill_batch_size: config.prefill_batch_size,
        // No slack on top of what the graphs measure: the default 512 MB is
        // half a gigabyte of a phone's memory budget (the reserve pass
        // grows the arena to exactly what the decode and prefill graphs need).
        extra_activation_bytes: 0,
        spec_draft_max: config.spec_draft_max,
        ..LlamaSessionConfig::default()
    };
    let session = match LlamaSession::load(&config.model, session_config) {
        Ok(s) => s,
        Err(e) => return send(TurnEvent::LoadFailed { error: format!("{e:?}") }),
    };
    let empty = match session.save_state() {
        Ok(s) => s,
        Err(e) => return send(TurnEvent::LoadFailed { error: format!("{e:?}") }),
    };
    send(TurnEvent::Ready { device: session.device_name(), load_ms: t0.elapsed().as_secs_f64() * 1e3 });
    let mut w = Worker { session, empty, kept: VecDeque::new(), kept_max: config.kept_prefixes.max(1), mmproj: config.mmproj.clone(), tower: None };
    while let Ok(job) = requests.recv() {
        // Prepares run in order; of the queued turns only the newest counts.
        let mut turn = None;
        for job in std::iter::once(job).chain(requests.try_iter()) {
            match job {
                Job::Prepare(system) => {
                    let t0 = Instant::now();
                    match w.system(&system) {
                        Ok(_) => send(TurnEvent::Prepared { prefill_ms: t0.elapsed().as_secs_f64() * 1e3 }),
                        Err(error) => send(TurnEvent::PrepareFailed { error }),
                    }
                }
                Job::ReleaseVision => w.tower = None,
                Job::Turn(request) => turn = Some(request),
            }
        }
        let Some(request) = turn else { continue };
        if latest.load(Ordering::SeqCst) != request.id {
            continue;
        }
        let id = request.id;
        match w.turn(&request, &latest, &send) {
            Ok(Some((text, stats))) => send(TurnEvent::Done { id, text, stats }),
            Ok(None) => {}
            Err(error) => send(TurnEvent::Failed { id, error }),
        }
    }
}

impl Worker {
    /// Puts the session at the end of `system`'s part: restores its kept
    /// state, or prefills it from empty and keeps it. Returns (reused,
    /// tokens prefilled).
    fn system(&mut self, system: &str) -> Result<(bool, usize), String> {
        let err = |e: crate::LlamaError| format!("{e:?}");
        if let Some(at) = self.kept.iter().position(|(sys, _)| sys == system) {
            let entry = self.kept.remove(at).unwrap();
            self.session.restore_state(&entry.1).map_err(err)?;
            self.kept.push_back(entry);
            return Ok((true, 0));
        }
        self.session.restore_state(&self.empty).map_err(err)?;
        let tokens = self.session.vocab().tokenize(&system_part(system), true, true).map_err(err)?;
        self.session.append_tokens(&tokens).map_err(err)?;
        let state = self.session.save_state().map_err(err)?;
        if self.kept.len() >= self.kept_max {
            self.kept.pop_front();
        }
        self.kept.push_back((system.to_string(), state));
        Ok((false, tokens.len()))
    }

    /// Image embeddings and their token grid (width, height); loads the
    /// vision tower at first use.
    fn embed(&mut self, image: &TurnImage) -> Result<(Vec<f32>, usize, usize), String> {
        let err = |e: crate::LlamaError| format!("{e:?}");
        if self.tower.is_none() {
            let path = self.mmproj.as_ref().ok_or("image turns need TurnConfig::mmproj")?;
            self.tower = Some(VisionTower::load(&path.to_string_lossy()).map_err(err)?);
        }
        let tower = self.tower.as_mut().unwrap();
        let prepared = preprocess_rgb8(&image.rgb, image.width, image.height, &tower.config).map_err(err)?;
        let embeddings = tower.encode(&prepared).map_err(err)?;
        Ok((embeddings, prepared.tokens_w(), prepared.tokens_h()))
    }

    /// Runs one turn; None when it was cancelled.
    fn turn(&mut self, request: &TurnRequest, latest: &AtomicU64, send: &dyn Fn(TurnEvent)) -> Result<Option<(String, TurnStats)>, String> {
        let err = |e: crate::LlamaError| format!("{e:?}");
        let vocab_tokens = |s: &LlamaSession, text: &str, bos: bool| s.vocab().tokenize(text, bos, true).map_err(err);
        // The image first: its embeddings do not depend on the session.
        let mut vision_ms = 0.0;
        let image = match &request.image {
            Some(image) => {
                let t = Instant::now();
                let embedded = self.embed(image)?;
                vision_ms = t.elapsed().as_secs_f64() * 1e3;
                Some(embedded)
            }
            None => None,
        };
        let t_prefill = Instant::now();
        let (reused, mut prefill_tokens) = self.system(&request.system)?;
        let (tokens, tail) = match &image {
            Some(_) => (vocab_tokens(&self.session, IMAGE_USER_START, false)?, vocab_tokens(&self.session, &image_user_end(&request.user), false)?),
            None => (vocab_tokens(&self.session, &user_part(&request.user), false)?, Vec::new()),
        };
        // The session already holds the system part.
        let user_tokens = tokens.len() + image.as_ref().map_or(0, |(_, w, h)| w * h) + tail.len();
        prefill_tokens += user_tokens;
        if self.session.token_count() + user_tokens + request.max_new_tokens > self.session.max_context() {
            return Err(format!(
                "the input is too long for the on-device context ({} tokens available)",
                self.session.max_context().saturating_sub(self.session.token_count() + request.max_new_tokens)
            ));
        }
        self.session.append_tokens(&tokens).map_err(err)?;
        if let Some((embeddings, w, h)) = &image {
            self.session.append_image_embeddings(embeddings, *w, *h).map_err(err)?;
            self.session.append_tokens(&tail).map_err(err)?;
        }
        let prefill_ms = t_prefill.elapsed().as_secs_f64() * 1e3;
        let t_decode = Instant::now();
        let mut generated = Vec::new();
        let mut text = String::new();
        while generated.len() < request.max_new_tokens {
            if latest.load(Ordering::SeqCst) != request.id {
                return Ok(None);
            }
            if self.session.speculative_enabled() {
                // Stream at most one draft span at a time, checking for a
                // replaced request between chunks just as between tokens.
                let chunk = self.session.speculation_depth().saturating_add(1)
                    .min(request.max_new_tokens - generated.len());
                let reply = self.session.continue_greedy(chunk).map_err(err)?;
                generated.extend(reply.token_ids);
                text = self.session.vocab().decode_tokens(&generated).map_err(err)?;
                if latest.load(Ordering::SeqCst) != request.id {
                    return Ok(None);
                }
                if !text.ends_with('\u{fffd}') {
                    send(TurnEvent::Partial { id: request.id, text: text.clone() });
                }
                if reply.stop_reason != LlamaStopReason::MaxNewTokens {
                    break;
                }
                continue;
            }
            match self.session.next_greedy_token().map_err(err)? {
                Some(token) => {
                    generated.push(token);
                    text = self.session.vocab().decode_tokens(&generated).map_err(err)?;
                    // A character split over tokens decodes as U+FFFD until
                    // its last byte arrives: no partial until then.
                    if !text.ends_with('\u{fffd}') {
                        send(TurnEvent::Partial { id: request.id, text: text.clone() });
                    }
                }
                None => break,
            }
        }
        let stats = TurnStats {
            prefill_tokens,
            prefill_ms,
            tokens: generated.len(),
            decode_ms: t_decode.elapsed().as_secs_f64() * 1e3,
            prefix_reused: reused,
            vision_ms,
        };
        Ok(Some((text, stats)))
    }
}
