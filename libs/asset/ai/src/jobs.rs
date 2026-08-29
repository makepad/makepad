//! Async job model: generations take seconds to minutes, one GPU runs one
//! job at a time. `JobStore` is the pure state machine (unit-testable without
//! threads); `SharedJobs` wraps it in Mutex+Condvar for the single worker
//! thread the server spawns.

use crate::backend::{CancelToken, GenerateParams, LiveParams};
use crate::error::AssetAiError;
use crate::protocol::{
    ArtifactRefJson, JobStatusJson, LiveStatusJson, JOB_STATE_CANCELLED, JOB_STATE_DONE,
    JOB_STATE_ERROR, JOB_STATE_LIVE, JOB_STATE_QUEUED, JOB_STATE_RUNNING,
};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Default cap on jobs WAITING in the FIFO (the running job is not counted).
/// A durable node must refuse unbounded backlog explicitly rather than accept
/// hours of queued GPU work; override per box with `MAKEPAD_ASSET_AI_MAX_QUEUE`.
pub const DEFAULT_QUEUE_LIMIT: usize = 32;
/// Finished (done/error/cancelled) job records retained for late pollers.
/// Beyond this the oldest-finished records are evicted together with their
/// artifact files, bounding a long-lived node's memory and disk.
pub const FINISHED_RETAIN: usize = 512;
/// Bounded per-job stage log: enough to reconstruct what a job did without
/// letting a 10-minute denoise grow an unbounded line list.
const LOG_CAP: usize = 64;
/// Same-phase ticks are logged at most this often; phase CHANGES always log.
const LOG_MIN_INTERVAL_MS: u64 = 5000;

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum QueuePolicy {
    /// Wait behind earlier jobs (default).
    #[default]
    Queue,
    /// Fail with `AssetAiError::Busy` when any job is queued or running.
    Reject,
}

impl QueuePolicy {
    pub fn parse(text: Option<&str>) -> Result<Self, AssetAiError> {
        match text {
            None | Some("queue") => Ok(QueuePolicy::Queue),
            Some("reject") => Ok(QueuePolicy::Reject),
            Some(other) => Err(AssetAiError::Backend(format!(
                "unknown queue_policy {other:?} (expected \"queue\" or \"reject\")"
            ))),
        }
    }
}

#[derive(Clone, Debug)]
pub enum JobState {
    Queued,
    Running { stage: String, progress: f64 },
    /// A live/realtime session running (see `JOB_STATE_LIVE`). `stage` is
    /// informational ("live"); `frames_in`/`frames_out`/`fps` are the same
    /// counters `crate::realtime::RealtimeSession` tracks, mirrored here at
    /// up to 10 Hz so a poller that never opens the websocket still sees
    /// the session is alive.
    Live {
        stage: String,
        frames_in: u64,
        frames_out: u64,
        fps: f64,
    },
    Done { artifacts: Vec<ArtifactRefJson> },
    Error { message: String },
    /// Cancelled — either dropped from the queue, or the running worker
    /// noticed the raised cancel flag and unwound.
    Cancelled,
}

/// What a job carries into the worker: an ordinary one-shot generation, or a
/// live session's initial config. `JobStore` only needs to tell the two
/// apart (`take_next` picks the right initial `JobState`; `is_live` lets
/// `server::worker_loop` dispatch to `execute_job` vs `execute_live_job`) —
/// everything else is opaque to it.
pub enum JobParams {
    Generate(GenerateParams),
    Live(LiveParams),
}

impl JobParams {
    pub fn model(&self) -> &str {
        match self {
            JobParams::Generate(params) => &params.model,
            JobParams::Live(params) => &params.model,
        }
    }

    pub fn is_live(&self) -> bool {
        matches!(self, JobParams::Live(_))
    }
}

/// Result of a cancel request (`POST /job/<id>/cancel`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CancelOutcome {
    /// Was queued: dropped immediately, state is now `cancelled`.
    Cancelled,
    /// Was running: the cancel flag is raised; the worker unwinds at the
    /// next step/tile boundary and the job then reports `cancelled`.
    Cancelling,
    /// Already finished (done/error/cancelled) — nothing to cancel.
    NotCancellable,
    Unknown,
}

pub struct JobRecord {
    pub id: String,
    /// Kept separately because finished status needs only the model id. The
    /// full request can contain a large binary input and bearer tickets, so
    /// the worker takes it out exactly once instead of cloning and retaining
    /// it for the finished-job lifetime.
    pub model: String,
    pub params: Option<JobParams>,
    pub state: JobState,
    /// Shared cancel flag: raised by POST /job/<id>/cancel, checked by the
    /// backend between steps/tiles/load components.
    pub cancel: CancelToken,
    /// Lifecycle timestamps (unix ms) surfaced on `/job/<id>`.
    pub queued_ms: u64,
    pub started_ms: Option<u64>,
    pub finished_ms: Option<u64>,
    /// Assistant text for chat/LLM jobs; surfaced as `JobStatusJson.partial_text`.
    pub partial_text: Option<String>,
    /// The completed text answer, set once when the job's text artifact is
    /// persisted; surfaced as `JobStatusJson.text`.
    pub text: Option<String>,
    /// Chat serving facts (warmth, think/visible split), surfaced as
    /// `JobStatusJson.serving`. Set by the backend as the turn runs.
    pub serving: Option<crate::protocol::ServingStatusJson>,
    /// Bounded stage-transition log, oldest first (see [`LOG_CAP`]).
    log: Vec<String>,
    /// Non-numeric prefix of the last logged stage (throttling key).
    log_phase: String,
    log_at_ms: u64,
}

impl JobRecord {
    /// Appends a log line when the stage's phase changed (its text up to the
    /// first digit — "denoise 3/50" and "denoise 4/50" are one phase) or the
    /// same phase has been ticking for [`LOG_MIN_INTERVAL_MS`].
    fn log_stage(&mut self, stage: &str, progress: f64) {
        let phase: String = stage.chars().take_while(|c| !c.is_ascii_digit()).collect();
        let now = now_ms();
        if phase == self.log_phase && now.saturating_sub(self.log_at_ms) < LOG_MIN_INTERVAL_MS {
            return;
        }
        self.log_phase = phase;
        self.log_at_ms = now;
        let since = now.saturating_sub(self.started_ms.unwrap_or(self.queued_ms));
        self.log.push(format!(
            "t+{:.1}s {stage} {:.0}%",
            since as f64 / 1000.0,
            progress * 100.0
        ));
        if self.log.len() > LOG_CAP {
            self.log.remove(0);
        }
    }
}

/// A finished job evicted by retention, with the artifact ids the server
/// must drop from its map (and delete from disk).
#[derive(Debug, PartialEq, Eq)]
pub struct EvictedJob {
    pub job_id: String,
    pub artifact_ids: Vec<String>,
}

/// Which admission class a job belongs to.
///
/// The box has one GPU and two very different kinds of work on it. A heavy
/// generation owns the device for tens of seconds; a chat turn is a handful of
/// milliseconds per token against a model that is already resident and that
/// serves several conversations from one copy of its weights.
///
/// Running them under one "one GPU, one job" rule makes the second kind wait
/// for the first, which on a shared box means a chat turn can sit behind a
/// video generation — and it means the lane machinery underneath chat can
/// never see more than one conversation, so the lanes do nothing at all.
///
/// The class is decided by the CALLER, from the registry, because the store
/// has no registry and should not grow one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JobClass {
    /// Served by the lane worker: many turns share one resident model.
    Chat,
    /// Everything else. One at a time, as before.
    Heavy,
}

pub struct JobStore {
    next_id: u64,
    jobs: HashMap<String, JobRecord>,
    queue: VecDeque<String>,
    class: HashMap<String, JobClass>,
    /// The one heavy job that owns the device.
    running_heavy: Option<String>,
    /// Chat turns in flight, bounded by [`Self::chat_slots`].
    running_chat: Vec<String>,
    /// Chat turns admitted at once. **1 is exactly today's behaviour** for the
    /// chat class itself; the lane count is what makes it worth raising.
    chat_slots: usize,
    /// Finished job ids in finish order (retention ring).
    finished: VecDeque<String>,
    queue_limit: usize,
}

impl Default for JobStore {
    fn default() -> Self {
        Self {
            next_id: 0,
            jobs: HashMap::new(),
            queue: VecDeque::new(),
            class: HashMap::new(),
            running_heavy: None,
            running_chat: Vec::new(),
            chat_slots: 1,
            finished: VecDeque::new(),
            queue_limit: DEFAULT_QUEUE_LIMIT,
        }
    }
}

impl JobStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replaces the queued-job bound (`MAKEPAD_ASSET_AI_MAX_QUEUE` on the service).
    /// A zero limit is refused into 1 — a queue that can hold nothing would
    /// make every submit fail.
    pub fn set_queue_limit(&mut self, limit: usize) {
        self.queue_limit = limit.max(1);
    }

    pub fn queue_limit(&self) -> usize {
        self.queue_limit
    }

    /// Chat turns that may run at once. Set from the box's lane count, so the
    /// store admits exactly as many conversations as the session can decode in
    /// one batch — no more, because a turn with no lane to sit in would queue
    /// inside the worker where the job protocol cannot see it.
    ///
    /// A zero is refused into 1: a class that can admit nothing would strand
    /// every chat turn forever.
    pub fn set_chat_slots(&mut self, slots: usize) {
        self.chat_slots = slots.max(1);
    }

    pub fn chat_slots(&self) -> usize {
        self.chat_slots
    }

    pub fn is_busy(&self) -> bool {
        self.running_heavy.is_some() || !self.running_chat.is_empty() || !self.queue.is_empty()
    }

    /// Jobs queued plus the running ones — the `/health` `jobs_pending` value
    /// fleet schedulers use as an affinity tiebreak.
    pub fn pending_count(&self) -> u64 {
        self.queue.len() as u64
            + self.running_heavy.is_some() as u64
            + self.running_chat.len() as u64
    }

    pub fn submit(
        &mut self,
        params: JobParams,
        policy: QueuePolicy,
    ) -> Result<String, AssetAiError> {
        self.submit_as(params, policy, JobClass::Heavy)
    }

    /// Submit into a named admission class.
    ///
    /// `QueuePolicy::Reject` still means "refuse if the box is doing anything
    /// at all". It is a caller's explicit request not to wait, and softening
    /// it per class would change what an existing caller asked for.
    pub fn submit_as(
        &mut self,
        params: JobParams,
        policy: QueuePolicy,
        class: JobClass,
    ) -> Result<String, AssetAiError> {
        if policy == QueuePolicy::Reject && self.is_busy() {
            return Err(AssetAiError::Busy);
        }
        if self.queue.len() >= self.queue_limit {
            return Err(AssetAiError::QueueFull(self.queue_limit));
        }
        self.next_id += 1;
        let id = format!("job-{}", self.next_id);
        let model = params.model().to_string();
        self.jobs.insert(
            id.clone(),
            JobRecord {
                id: id.clone(),
                model,
                params: Some(params),
                state: JobState::Queued,
                cancel: CancelToken::new(),
                queued_ms: now_ms(),
                started_ms: None,
                finished_ms: None,
                partial_text: None,
                text: None,
                serving: None,
                log: Vec::new(),
                log_phase: String::new(),
                log_at_ms: 0,
            },
        );
        self.class.insert(id.clone(), class);
        self.queue.push_back(id.clone());
        Ok(id)
    }

    /// Models with a job RUNNING on them right now, in either class.
    ///
    /// Eviction reads this and refuses. With one admission class it could not
    /// happen — the only running job was the one asking to load. With two, a
    /// heavy job admitting itself can otherwise tear down the model a chat
    /// turn is decoding through, and the turn dies with "llm worker dropped
    /// the reply" for a reason that has nothing to do with it.
    pub fn running_models(&self) -> Vec<String> {
        self.running_heavy
            .iter()
            .chain(self.running_chat.iter())
            .filter_map(|id| self.jobs.get(id).map(|job| job.model.clone()))
            .collect()
    }

    /// The admission class a job was submitted into.
    pub fn class_of(&self, id: &str) -> JobClass {
        self.class.get(id).copied().unwrap_or(JobClass::Heavy)
    }

    /// Pops the next queued HEAVY job and marks it running. Returns `None`
    /// while a heavy job is already running (one GPU, one heavy job) or no
    /// heavy job is queued.
    pub fn take_next(&mut self) -> Option<String> {
        self.take_next_of(JobClass::Heavy)
    }

    /// Pops the next queued job of `class` and marks it running.
    ///
    /// Each class holds its own device budget, so a chat turn is never behind
    /// a running video generation and a video generation is never behind a
    /// chat turn. FIFO is preserved WITHIN a class; across classes there is no
    /// single order to preserve, because they are no longer competing for one
    /// slot.
    pub fn take_next_of(&mut self, class: JobClass) -> Option<String> {
        match class {
            JobClass::Heavy => {
                if self.running_heavy.is_some() {
                    return None;
                }
            }
            JobClass::Chat => {
                if self.running_chat.len() >= self.chat_slots {
                    return None;
                }
            }
        }
        let at = self
            .queue
            .iter()
            .position(|id| self.class_of(id) == class)?;
        let id = self.queue.remove(at)?;
        if let Some(job) = self.jobs.get_mut(&id) {
            let is_live = job.params.as_ref().map(JobParams::is_live).unwrap_or(false);
            job.state = if is_live {
                JobState::Live {
                    stage: "starting".to_string(),
                    frames_in: 0,
                    frames_out: 0,
                    fps: 0.0,
                }
            } else {
                JobState::Running {
                    stage: "starting".to_string(),
                    progress: 0.0,
                }
            };
            job.started_ms = Some(now_ms());
        }
        match class {
            JobClass::Heavy => self.running_heavy = Some(id.clone()),
            JobClass::Chat => self.running_chat.push(id.clone()),
        }
        Some(id)
    }

    /// Release whichever class slot `id` occupies. Called from every terminal
    /// transition, so a job cannot finish and keep its slot.
    fn release(&mut self, id: &str) {
        if self.running_heavy.as_deref() == Some(id) {
            self.running_heavy = None;
        }
        self.running_chat.retain(|running| running != id);
    }

    /// Moves the potentially large/sensitive request into the worker. A job
    /// has one execution attempt; retaining or cloning these bytes after it
    /// starts only wastes memory and keeps expired transfer tickets alive.
    pub fn take_params(&mut self, id: &str) -> Option<JobParams> {
        self.jobs.get_mut(id)?.params.take()
    }

    /// Peeks whether a (still-queued-or-just-taken) job is a live session —
    /// `server::worker_loop` uses this right after `wait_take_next` to pick
    /// `execute_job` vs `execute_live_job`, before `take_params` moves the
    /// params out.
    pub fn is_live(&self, id: &str) -> bool {
        self.jobs
            .get(id)
            .and_then(|job| job.params.as_ref())
            .map(JobParams::is_live)
            .unwrap_or(false)
    }

    /// The model ONE job runs on. `running_models` answers for the whole box
    /// and so mixes the admission classes together; a worker that needs to
    /// know what IT is about to run — to remember whose device caches it will
    /// be filling — must ask about its own job and nothing else.
    pub fn model_of(&self, id: &str) -> Option<String> {
        self.jobs.get(id).map(|job| job.model.clone())
    }

    /// Cancels a job. Queued: dropped from the FIFO immediately. Running:
    /// raises the job's shared cancel flag — the backend checks it between
    /// steps/tiles/load components and unwinds within seconds; the worker
    /// then marks the job `cancelled` and discards partial artifacts.
    /// Finished jobs keep their result (NotCancellable).
    pub fn cancel(&mut self, id: &str) -> CancelOutcome {
        if let Some(at) = self.queue.iter().position(|q| q == id) {
            self.queue.remove(at);
            if let Some(job) = self.jobs.get_mut(id) {
                job.state = JobState::Cancelled;
                job.finished_ms = Some(now_ms());
                job.params = None;
            }
            self.finished.push_back(id.to_string());
            return CancelOutcome::Cancelled;
        }
        match self.jobs.get(id) {
            None => CancelOutcome::Unknown,
            Some(job) => match job.state {
                JobState::Running { .. } | JobState::Live { .. } => {
                    job.cancel.cancel();
                    CancelOutcome::Cancelling
                }
                _ => CancelOutcome::NotCancellable,
            },
        }
    }

    /// The running job's shared cancel flag (for the worker to pass into the
    /// backend).
    pub fn cancel_token(&self, id: &str) -> Option<CancelToken> {
        self.jobs.get(id).map(|job| job.cancel.clone())
    }

    /// Worker saw AssetAiError::Cancelled: the job unwound mid-run.
    pub fn cancelled(&mut self, id: &str) {
        if let Some(job) = self.jobs.get_mut(id) {
            job.state = JobState::Cancelled;
            job.finished_ms = Some(now_ms());
        }
        self.finished.push_back(id.to_string());
        self.release(id);
    }

    pub fn set_partial_text(&mut self, id: &str, text: String) {
        if let Some(job) = self.jobs.get_mut(id) {
            job.partial_text = Some(text);
        }
    }

    /// The finished answer (see `JobStatusJson::text`). Called once, when the
    /// worker persists a text artifact — never from the streaming path, so a
    /// reader that only trusts `text` never sees a half-written reply.
    pub fn set_text(&mut self, id: &str, text: String) {
        if let Some(job) = self.jobs.get_mut(id) {
            job.text = Some(text);
        }
    }

    /// Merge chat serving facts. Fields arrive at different moments — warmth
    /// at prefill, the think split as the reply grows — so each update fills
    /// only what it knows and leaves the rest alone. Overwriting wholesale
    /// would blank the warmth the moment the first token lands.
    pub fn update_serving(
        &mut self,
        id: &str,
        update: impl FnOnce(&mut crate::protocol::ServingStatusJson),
    ) {
        if let Some(job) = self.jobs.get_mut(id) {
            let mut serving = job.serving.take().unwrap_or(
                crate::protocol::ServingStatusJson {
                    prefix_ingested: None,
                    prefix_resumed: None,
                    think_tokens: None,
                    visible_tokens: None,
                    gen_tokens: None,
                },
            );
            update(&mut serving);
            job.serving = Some(serving);
        }
    }

    pub fn set_progress(&mut self, id: &str, stage: &str, progress: f64) {
        if let Some(job) = self.jobs.get_mut(id) {
            if matches!(job.state, JobState::Running { .. }) {
                job.state = JobState::Running {
                    stage: stage.to_string(),
                    progress: progress.clamp(0.0, 1.0),
                };
                job.log_stage(stage, progress.clamp(0.0, 1.0));
            }
        }
    }

    /// Live-session counters, updated by `crate::realtime::run_live` through
    /// `server::execute_live_job`'s progress closure (throttled to ~10 Hz
    /// there — this only writes when the job is still `Live`, so a stray
    /// late update after cancel/finish is silently ignored, same convention
    /// as `set_progress`).
    pub fn set_live_progress(&mut self, id: &str, stage: &str, frames_in: u64, frames_out: u64, fps: f64) {
        if let Some(job) = self.jobs.get_mut(id) {
            if matches!(job.state, JobState::Live { .. }) {
                job.state = JobState::Live {
                    stage: stage.to_string(),
                    frames_in,
                    frames_out,
                    fps,
                };
                job.log_stage(stage, 0.0);
            }
        }
    }

    pub fn finish(&mut self, id: &str, artifacts: Vec<ArtifactRefJson>) {
        if let Some(job) = self.jobs.get_mut(id) {
            job.state = JobState::Done { artifacts };
            job.finished_ms = Some(now_ms());
        }
        self.finished.push_back(id.to_string());
        self.release(id);
    }

    pub fn fail(&mut self, id: &str, message: String) {
        if let Some(job) = self.jobs.get_mut(id) {
            job.state = JobState::Error { message };
            job.finished_ms = Some(now_ms());
        }
        self.finished.push_back(id.to_string());
        self.release(id);
    }

    /// Retention: keeps the newest [`FINISHED_RETAIN`] finished records and
    /// evicts the rest oldest-first, returning what was dropped so the
    /// server can also delete the artifacts from its map and disk. Late
    /// polls of an evicted id get an honest 404, never a stale answer.
    pub fn evict_expired_finished(&mut self) -> Vec<EvictedJob> {
        let mut evicted = Vec::new();
        while self.finished.len() > FINISHED_RETAIN {
            let Some(id) = self.finished.pop_front() else {
                break;
            };
            let Some(job) = self.jobs.remove(&id) else {
                continue;
            };
            let artifact_ids = match job.state {
                JobState::Done { artifacts } => {
                    artifacts.into_iter().map(|artifact| artifact.id).collect()
                }
                _ => Vec::new(),
            };
            evicted.push(EvictedJob {
                job_id: id,
                artifact_ids,
            });
        }
        evicted
    }

    /// What the box is busy with while a queued job waits: queue position
    /// plus the running job's live stage, so a queued client can tell a
    /// working box from a hung one ("2 ahead; box: denoise 32/100").
    ///
    /// "Ahead" counts only jobs of the SAME class, because those are the only
    /// ones this job is actually waiting for. Counting a chat turn as being
    /// ahead of a video generation would be a number that never goes down for
    /// the reason it claims.
    fn queued_detail(&self, id: &str) -> String {
        let class = self.class_of(id);
        let ahead = self
            .queue
            .iter()
            .take_while(|q| *q != id)
            .filter(|q| self.class_of(q) == class)
            .count();
        let busy = self
            .running_heavy
            .iter()
            .chain(self.running_chat.iter())
            .find_map(|r| {
                self.jobs.get(r).and_then(|job| match &job.state {
                    JobState::Running { stage, progress } => {
                        Some(format!("{stage} {:.0}%", progress * 100.0))
                    }
                    JobState::Live { stage, frames_out, .. } => {
                        Some(format!("{stage} (live session, {frames_out} frames out)"))
                    }
                    _ => None,
                })
            });
        match busy {
            Some(busy) => format!("{ahead} ahead; box: {busy}"),
            None => format!("{ahead} ahead"),
        }
    }

    /// Running jobs first, then the queue in FIFO order — the live picture
    /// `GET /jobs` serves (finished jobs stay reachable by id only).
    pub fn active_status_json(&self) -> Vec<JobStatusJson> {
        self.running_heavy
            .iter()
            .chain(self.running_chat.iter())
            .chain(self.queue.iter())
            .filter_map(|id| self.status_json(id))
            .collect()
    }

    pub fn status_json(&self, id: &str) -> Option<JobStatusJson> {
        let job = self.jobs.get(id)?;
        let mut status = JobStatusJson {
            job_id: job.id.clone(),
            state: String::new(),
            stage: None,
            progress: None,
            artifacts: Vec::new(),
            error: None,
            model: Some(job.model.clone()),
            queued_ms: Some(job.queued_ms),
            started_ms: job.started_ms,
            finished_ms: job.finished_ms,
            log: if job.log.is_empty() {
                None
            } else {
                Some(job.log.clone())
            },
            partial_text: job.partial_text.clone(),
            live: None,
            serving: job.serving.clone(),
            text: job.text.clone(),
        };
        match &job.state {
            JobState::Queued => {
                status.state = JOB_STATE_QUEUED.to_string();
                status.stage = Some(self.queued_detail(id));
            }
            JobState::Running { stage, progress } => {
                status.state = JOB_STATE_RUNNING.to_string();
                status.stage = Some(stage.clone());
                status.progress = Some(*progress);
            }
            JobState::Live { stage, frames_in, frames_out, fps } => {
                status.state = JOB_STATE_LIVE.to_string();
                status.stage = Some(stage.clone());
                status.live = Some(LiveStatusJson {
                    frames_in: *frames_in,
                    frames_out: *frames_out,
                    fps: *fps,
                });
            }
            JobState::Done { artifacts } => {
                status.state = JOB_STATE_DONE.to_string();
                status.progress = Some(1.0);
                status.artifacts = artifacts.clone();
            }
            JobState::Error { message } => {
                status.state = JOB_STATE_ERROR.to_string();
                status.error = Some(message.clone());
            }
            JobState::Cancelled => {
                status.state = JOB_STATE_CANCELLED.to_string();
            }
        }
        Some(status)
    }
}

/// Mutex+Condvar wrapper shared between the HTTP request loop (submit,
/// status) and the worker thread (take, progress, finish).
#[derive(Clone, Default)]
pub struct SharedJobs {
    inner: Arc<(Mutex<JobStore>, Condvar)>,
}

impl SharedJobs {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with<R>(&self, f: impl FnOnce(&mut JobStore) -> R) -> R {
        let mut store = self.inner.0.lock().unwrap();
        let result = f(&mut store);
        self.inner.1.notify_all();
        result
    }

    pub fn submit(
        &self,
        params: JobParams,
        policy: QueuePolicy,
    ) -> Result<String, AssetAiError> {
        self.submit_as(params, policy, JobClass::Heavy)
    }

    pub fn submit_as(
        &self,
        params: JobParams,
        policy: QueuePolicy,
        class: JobClass,
    ) -> Result<String, AssetAiError> {
        self.with(|store| store.submit_as(params, policy, class))
    }

    /// Blocks (with a timeout so the worker stays responsive to process
    /// shutdown) until a HEAVY job can start.
    pub fn wait_take_next(&self, timeout: Duration) -> Option<String> {
        self.wait_take_next_of(JobClass::Heavy, timeout)
    }

    /// Blocks (with a timeout) until a job of `class` can start.
    ///
    /// One waiter per class, so a worker never wakes for work it cannot take.
    /// `notify_all` in `with` is what makes that safe: every terminal
    /// transition wakes every class, and each re-checks its own budget.
    pub fn wait_take_next_of(&self, class: JobClass, timeout: Duration) -> Option<String> {
        let mut store = self.inner.0.lock().unwrap();
        if let Some(id) = store.take_next_of(class) {
            return Some(id);
        }
        let (mut store, _timed_out) = self.inner.1.wait_timeout(store, timeout).unwrap();
        store.take_next_of(class)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn params(model: &str) -> JobParams {
        JobParams::Generate(generate_params(model))
    }

    fn live_params(model: &str) -> JobParams {
        JobParams::Live(LiveParams {
            model: model.to_string(),
            config: crate::backend::LiveConfig::default(),
            loop_mode: crate::backend::LoopMode::Feed,
            input_encoding: crate::backend::OutputEncoding::Raw,
            output_encoding: crate::backend::OutputEncoding::Raw,
            max_fps: 0.0,
            idle_timeout_s: 30,
        })
    }

    pub(crate) fn generate_params(model: &str) -> GenerateParams {
        GenerateParams {
            model: model.to_string(),
            prompt: "p".to_string(),
            negative_prompt: String::new(),
            width: Some(64),
            height: Some(64),
            seed: 1,
            steps: Some(1),
            guidance: Some(1.0),
            delay_ms: 0,
            inputs: Vec::new(),
            strength: None,
            pull_only: false,
            input_bytes: Vec::new(),
            input_content_type: String::new(),
            frames: None,
            codec: String::new(),
            audio: None,
            interpolate: None,
            upscale: None,
            flow_map: false,
            target_domain: "image".to_string(),
            identity_anchor: String::new(),
            style: String::new(),
            max_tokens: 512,
            temperature: 0.7,
            variants: 1,
            text: String::new(),
            voice: String::new(),
            speed: 1.0,
            emotion: None,
            seconds: None,
            lyrics: String::new(),
            remesh_resolution: None,
            texture: None,
            decimation_target: None,
            texture_size: None,
            gaussians: None,
            motion_mode: None,
            canny_low: None,
            canny_high: None,
            loras: Vec::new(),
            peer_sources: Vec::new(),
            peer_tickets: Vec::new(),
        }
    }

    /// The whole point of the split: a chat turn is not stuck behind a video.
    #[test]
    fn a_chat_turn_does_not_wait_for_a_running_heavy_job() {
        let mut store = JobStore::new();
        let heavy = store
            .submit_as(params("h3"), QueuePolicy::Queue, JobClass::Heavy)
            .unwrap();
        let chat = store
            .submit_as(params("qwen"), QueuePolicy::Queue, JobClass::Chat)
            .unwrap();

        assert_eq!(store.take_next_of(JobClass::Heavy).as_deref(), Some(&*heavy));
        assert_eq!(
            store.take_next_of(JobClass::Chat).as_deref(),
            Some(&*chat),
            "a chat turn must start while the heavy job is still running"
        );
        assert_eq!(store.status_json(&heavy).unwrap().state, "running");
        assert_eq!(store.status_json(&chat).unwrap().state, "running");
    }

    /// And the converse, which is just as important on a shared box: a queue
    /// full of chat must not starve the heavy class.
    #[test]
    fn a_heavy_job_does_not_wait_for_running_chat() {
        let mut store = JobStore::new();
        store.set_chat_slots(2);
        for _ in 0..2 {
            let id = store
                .submit_as(params("qwen"), QueuePolicy::Queue, JobClass::Chat)
                .unwrap();
            assert_eq!(store.take_next_of(JobClass::Chat).as_deref(), Some(&*id));
        }
        let heavy = store
            .submit_as(params("h3"), QueuePolicy::Queue, JobClass::Heavy)
            .unwrap();
        assert_eq!(store.take_next_of(JobClass::Heavy).as_deref(), Some(&*heavy));
    }

    #[test]
    fn chat_admits_up_to_its_slot_count_and_no_further() {
        let mut store = JobStore::new();
        store.set_chat_slots(3);
        let ids: Vec<String> = (0..5)
            .map(|_| {
                store
                    .submit_as(params("qwen"), QueuePolicy::Queue, JobClass::Chat)
                    .unwrap()
            })
            .collect();
        for expected in ids.iter().take(3) {
            assert_eq!(
                store.take_next_of(JobClass::Chat).as_deref(),
                Some(&**expected),
                "chat is FIFO within its own class"
            );
        }
        assert!(
            store.take_next_of(JobClass::Chat).is_none(),
            "the fourth turn waits: a turn with no lane to sit in would queue \
             inside the worker where the job protocol cannot see it"
        );
        store.finish(&ids[0], Vec::new());
        assert_eq!(
            store.take_next_of(JobClass::Chat).as_deref(),
            Some(&*ids[3]),
            "a finished turn frees its slot for the next one"
        );
    }

    #[test]
    fn a_slot_count_of_zero_would_strand_every_turn_so_it_is_refused() {
        let mut store = JobStore::new();
        store.set_chat_slots(0);
        assert_eq!(store.chat_slots(), 1);
    }

    #[test]
    fn every_terminal_transition_frees_the_slot_it_held() {
        for (label, terminate) in [
            ("finish", 0usize),
            ("fail", 1),
            ("cancelled", 2),
        ] {
            let mut store = JobStore::new();
            let chat = store
                .submit_as(params("qwen"), QueuePolicy::Queue, JobClass::Chat)
                .unwrap();
            store.take_next_of(JobClass::Chat).unwrap();
            let heavy = store
                .submit_as(params("h3"), QueuePolicy::Queue, JobClass::Heavy)
                .unwrap();
            store.take_next_of(JobClass::Heavy).unwrap();
            match terminate {
                0 => {
                    store.finish(&chat, Vec::new());
                    store.finish(&heavy, Vec::new());
                }
                1 => {
                    store.fail(&chat, "boom".to_string());
                    store.fail(&heavy, "boom".to_string());
                }
                _ => {
                    store.cancelled(&chat);
                    store.cancelled(&heavy);
                }
            }
            let next_chat = store
                .submit_as(params("qwen"), QueuePolicy::Queue, JobClass::Chat)
                .unwrap();
            let next_heavy = store
                .submit_as(params("h3"), QueuePolicy::Queue, JobClass::Heavy)
                .unwrap();
            assert_eq!(
                store.take_next_of(JobClass::Chat).as_deref(),
                Some(&*next_chat),
                "{label} must free the chat slot"
            );
            assert_eq!(
                store.take_next_of(JobClass::Heavy).as_deref(),
                Some(&*next_heavy),
                "{label} must free the heavy slot"
            );
        }
    }

    /// `pending_count` feeds `/health`'s affinity tiebreak. Missing a class
    /// would advertise a busy box as free.
    #[test]
    fn pending_counts_both_classes() {
        let mut store = JobStore::new();
        store.set_chat_slots(2);
        let heavy = store
            .submit_as(params("h3"), QueuePolicy::Queue, JobClass::Heavy)
            .unwrap();
        let chat = store
            .submit_as(params("qwen"), QueuePolicy::Queue, JobClass::Chat)
            .unwrap();
        assert_eq!(store.pending_count(), 2);
        store.take_next_of(JobClass::Heavy).unwrap();
        store.take_next_of(JobClass::Chat).unwrap();
        assert_eq!(store.pending_count(), 2, "running still counts as pending");
        assert!(store.is_busy());
        store.finish(&heavy, Vec::new());
        store.finish(&chat, Vec::new());
        assert_eq!(store.pending_count(), 0);
        assert!(!store.is_busy());
    }

    /// Default construction must behave exactly as it did before the split:
    /// one class, one job.
    #[test]
    fn the_default_store_is_still_one_job_at_a_time() {
        let mut store = JobStore::new();
        let a = store.submit(params("m"), QueuePolicy::Queue).unwrap();
        let b = store.submit(params("m"), QueuePolicy::Queue).unwrap();
        assert_eq!(store.take_next().as_deref(), Some(&*a));
        assert!(store.take_next().is_none());
        assert_eq!(store.class_of(&b), JobClass::Heavy);
    }

    #[test]
    fn fifo_lifecycle() {
        let mut store = JobStore::new();
        let a = store.submit(params("m"), QueuePolicy::Queue).unwrap();
        let b = store.submit(params("m"), QueuePolicy::Queue).unwrap();
        assert_ne!(a, b);
        assert_eq!(store.status_json(&a).unwrap().state, "queued");

        // FIFO: a runs first, and nothing else can start while it runs.
        let running = store.take_next().unwrap();
        assert_eq!(running, a);
        assert!(store.take_next().is_none());
        assert_eq!(store.status_json(&a).unwrap().state, "running");

        store.set_progress(&a, "render", 0.5);
        let status = store.status_json(&a).unwrap();
        assert_eq!(status.stage.as_deref(), Some("render"));
        assert_eq!(status.progress, Some(0.5));

        store.finish(&a, vec![]);
        assert_eq!(store.status_json(&a).unwrap().state, "done");

        // Now b can start.
        assert_eq!(store.take_next().unwrap(), b);
        store.fail(&b, "boom".to_string());
        let status = store.status_json(&b).unwrap();
        assert_eq!(status.state, "error");
        assert_eq!(status.error.as_deref(), Some("boom"));
        assert!(!store.is_busy());
    }

    #[test]
    fn reject_policy() {
        let mut store = JobStore::new();
        let a = store.submit(params("m"), QueuePolicy::Reject).unwrap();
        // Busy while queued.
        assert_eq!(
            store.submit(params("m"), QueuePolicy::Reject),
            Err(AssetAiError::Busy)
        );
        let running = store.take_next().unwrap();
        assert_eq!(running, a);
        // Busy while running.
        assert_eq!(
            store.submit(params("m"), QueuePolicy::Reject),
            Err(AssetAiError::Busy)
        );
        // Queue policy still allowed while busy.
        assert!(store.submit(params("m"), QueuePolicy::Queue).is_ok());
        store.finish(&a, vec![]);
        // Still busy: one queued.
        assert_eq!(
            store.submit(params("m"), QueuePolicy::Reject),
            Err(AssetAiError::Busy)
        );
    }

    #[test]
    fn progress_ignored_when_not_running() {
        let mut store = JobStore::new();
        let a = store.submit(params("m"), QueuePolicy::Queue).unwrap();
        store.set_progress(&a, "x", 0.5);
        assert_eq!(store.status_json(&a).unwrap().state, "queued");
        assert!(store.status_json("job-999").is_none());
    }

    #[test]
    fn cancel_queued_drops_running_flags() {
        let mut store = JobStore::new();
        let a = store.submit(params("m"), QueuePolicy::Queue).unwrap();
        let b = store.submit(params("m"), QueuePolicy::Queue).unwrap();
        // a starts running; b sits in the queue.
        assert_eq!(store.take_next(), Some(a.clone()));
        // Queued: dropped immediately, state "cancelled".
        assert_eq!(store.cancel(&b), CancelOutcome::Cancelled);
        assert_eq!(store.status_json(&b).unwrap().state, "cancelled");
        assert_eq!(store.status_json(&b).unwrap().error, None);
        // Running: the shared flag is raised; the worker unwinds later.
        let token = store.cancel_token(&a).unwrap();
        assert!(!token.is_cancelled());
        assert_eq!(store.cancel(&a), CancelOutcome::Cancelling);
        assert!(token.is_cancelled());
        assert!(token.check().is_err());
        // Unknown id.
        assert_eq!(store.cancel("job-999"), CancelOutcome::Unknown);
        // The cancelled queued job never reaches the worker.
        assert_eq!(store.pending_count(), 1); // just the running one
        // Worker notices the flag and reports the unwind.
        store.cancelled(&a);
        assert_eq!(store.status_json(&a).unwrap().state, "cancelled");
        // Finished jobs are not cancellable.
        assert_eq!(store.cancel(&a), CancelOutcome::NotCancellable);
        assert_eq!(store.take_next(), None);
    }

    #[test]
    fn queue_policy_parse() {
        assert_eq!(QueuePolicy::parse(None).unwrap(), QueuePolicy::Queue);
        assert_eq!(
            QueuePolicy::parse(Some("queue")).unwrap(),
            QueuePolicy::Queue
        );
        assert_eq!(
            QueuePolicy::parse(Some("reject")).unwrap(),
            QueuePolicy::Reject
        );
        assert!(QueuePolicy::parse(Some("nope")).is_err());
    }

    #[test]
    fn bounded_queue_refuses_overflow_explicitly() {
        let mut store = JobStore::new();
        store.set_queue_limit(2);
        assert_eq!(store.queue_limit(), 2);
        let a = store.submit(params("m"), QueuePolicy::Queue).unwrap();
        let _b = store.submit(params("m"), QueuePolicy::Queue).unwrap();
        // Third queued submit hits the bound with the explicit error.
        assert_eq!(
            store.submit(params("m"), QueuePolicy::Queue),
            Err(AssetAiError::QueueFull(2))
        );
        // The RUNNING job does not consume queue capacity: once `a` starts,
        // a slot frees up.
        assert_eq!(store.take_next(), Some(a));
        assert!(store.submit(params("m"), QueuePolicy::Queue).is_ok());
        assert_eq!(
            store.submit(params("m"), QueuePolicy::Queue),
            Err(AssetAiError::QueueFull(2))
        );
        // Zero is refused into a 1-slot queue, never a dead one.
        store.set_queue_limit(0);
        assert_eq!(store.queue_limit(), 1);
    }

    #[test]
    fn finished_retention_evicts_oldest_with_artifact_ids() {
        use crate::protocol::ArtifactRefJson;
        let mut store = JobStore::new();
        store.set_queue_limit(FINISHED_RETAIN + 8);
        let mut ids = Vec::new();
        for _ in 0..FINISHED_RETAIN + 3 {
            let id = store.submit(params("m"), QueuePolicy::Queue).unwrap();
            assert_eq!(store.take_next().as_ref(), Some(&id));
            let artifact = ArtifactRefJson {
                id: format!("{id}-0"),
                url: format!("/artifact/{id}-0"),
                content_type: "image/png".to_string(),
                sha256: None,
                byte_len: None,
            };
            store.finish(&id, vec![artifact]);
            ids.push(id);
        }
        let evicted = store.evict_expired_finished();
        assert_eq!(evicted.len(), 3);
        for (evicted, expected) in evicted.iter().zip(&ids[..3]) {
            assert_eq!(&evicted.job_id, expected);
            assert_eq!(evicted.artifact_ids, vec![format!("{expected}-0")]);
            // Evicted records answer honestly: gone is gone.
            assert!(store.status_json(expected).is_none());
        }
        // The newest jobs survive.
        assert!(store.status_json(ids.last().unwrap()).is_some());
        // Idempotent: nothing more to evict.
        assert!(store.evict_expired_finished().is_empty());
    }

    #[test]
    fn job_metadata_and_log_capture_lifecycle() {
        let mut store = JobStore::new();
        let id = store.submit(params("m"), QueuePolicy::Queue).unwrap();
        let queued = store.status_json(&id).unwrap();
        assert_eq!(queued.model.as_deref(), Some("m"));
        assert!(queued.queued_ms.is_some());
        assert!(queued.started_ms.is_none());
        assert!(queued.log.is_none());

        store.take_next().unwrap();
        // Phase changes always log; same-phase ticks are throttled.
        store.set_progress(&id, "load unet 1/23GB", 0.1);
        store.set_progress(&id, "load unet 2/23GB", 0.2);
        store.set_progress(&id, "denoise 1/50", 0.5);
        store.set_progress(&id, "denoise 2/50", 0.6);
        let running = store.status_json(&id).unwrap();
        let log = running.log.expect("log after progress");
        assert_eq!(log.len(), 2, "one line per phase, got: {log:?}");
        assert!(log[0].contains("load unet"));
        assert!(log[1].contains("denoise"));

        store.finish(&id, vec![]);
        let done = store.status_json(&id).unwrap();
        let started = done.started_ms.expect("started_ms");
        let finished = done.finished_ms.expect("finished_ms");
        assert!(done.queued_ms.unwrap() <= started && started <= finished);
    }

    #[test]
    fn live_session_queued_to_live_to_done() {
        let mut store = JobStore::new();
        let id = store.submit(live_params("testpattern"), QueuePolicy::Queue).unwrap();
        assert_eq!(store.status_json(&id).unwrap().state, "queued");

        assert_eq!(store.take_next().as_deref(), Some(id.as_str()));
        let live = store.status_json(&id).unwrap();
        assert_eq!(live.state, "live");
        let counters = live.live.expect("live counters present");
        assert_eq!((counters.frames_in, counters.frames_out), (0, 0));

        // A second job stays queued behind the live session — the box is
        // dedicated to the live feed until it stops.
        let queued = store.submit(params("testpattern"), QueuePolicy::Queue).unwrap();
        assert_eq!(store.status_json(&queued).unwrap().state, "queued");
        assert!(store.take_next().is_none());

        store.set_live_progress(&id, "live", 12, 11, 30.5);
        let live = store.status_json(&id).unwrap();
        let counters = live.live.expect("live counters present");
        assert_eq!((counters.frames_in, counters.frames_out), (12, 11));
        assert_eq!(counters.fps, 30.5);
        // Ordinary set_progress must not touch a Live job's state.
        store.set_progress(&id, "denoise", 0.5);
        assert_eq!(store.status_json(&id).unwrap().state, "live");

        // "stop"/idle-timeout ends a live session as an ordinary finish
        // (done, no artifacts) — the worker calls this exactly like any
        // other job, just with an empty artifact list.
        store.finish(&id, vec![]);
        assert_eq!(store.status_json(&id).unwrap().state, "done");
        // The worker slot is free again: the queued job can now start.
        assert_eq!(store.pending_count(), 1);
        assert_eq!(store.take_next().as_deref(), Some(queued.as_str()));
    }

    #[test]
    fn live_session_cancel_raises_flag_like_running() {
        let mut store = JobStore::new();
        let id = store.submit(live_params("testpattern"), QueuePolicy::Queue).unwrap();
        assert_eq!(store.take_next().as_deref(), Some(id.as_str()));
        assert!(matches!(store.status_json(&id).unwrap().state.as_str(), "live"));

        let token = store.cancel_token(&id).unwrap();
        assert!(!token.is_cancelled());
        assert_eq!(store.cancel(&id), CancelOutcome::Cancelling);
        assert!(token.is_cancelled());

        // The worker sees AssetAiError::Cancelled and reports it exactly
        // like an ordinary job's cancellation.
        store.cancelled(&id);
        assert_eq!(store.status_json(&id).unwrap().state, "cancelled");
        assert_eq!(store.cancel(&id), CancelOutcome::NotCancellable);
    }

    #[test]
    fn is_live_reports_before_and_after_take_next() {
        let mut store = JobStore::new();
        let live_id = store.submit(live_params("testpattern"), QueuePolicy::Queue).unwrap();
        let gen_id = store.submit(params("testpattern"), QueuePolicy::Queue).unwrap();
        assert!(store.is_live(&live_id));
        assert!(!store.is_live(&gen_id));
        assert!(!store.is_live("job-999"));

        assert_eq!(store.take_next().as_deref(), Some(live_id.as_str()));
        // Still live after take_next moved it to Running/Live state (params
        // still present — take_params has not been called yet).
        assert!(store.is_live(&live_id));
        let taken = store.take_params(&live_id).unwrap();
        assert!(taken.is_live());
        // Once taken, params is None: is_live reports false (nothing left
        // to peek), matching take_params's one-shot contract.
        assert!(!store.is_live(&live_id));
    }
}
