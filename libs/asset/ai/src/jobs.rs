//! Async job model: generations take seconds to minutes, one GPU runs one
//! job at a time. `JobStore` is the pure state machine (unit-testable without
//! threads); `SharedJobs` wraps it in Mutex+Condvar for the single worker
//! thread the server spawns.

use crate::backend::{CancelToken, GenerateParams};
use crate::error::AssetAiError;
use crate::protocol::{
    ArtifactRefJson, JobStatusJson, JOB_STATE_CANCELLED, JOB_STATE_DONE, JOB_STATE_ERROR,
    JOB_STATE_QUEUED, JOB_STATE_RUNNING,
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
    Done { artifacts: Vec<ArtifactRefJson> },
    Error { message: String },
    /// Cancelled — either dropped from the queue, or the running worker
    /// noticed the raised cancel flag and unwound.
    Cancelled,
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
    pub params: Option<GenerateParams>,
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

pub struct JobStore {
    next_id: u64,
    jobs: HashMap<String, JobRecord>,
    queue: VecDeque<String>,
    running: Option<String>,
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
            running: None,
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

    pub fn is_busy(&self) -> bool {
        self.running.is_some() || !self.queue.is_empty()
    }

    /// Jobs queued plus the running one — the `/health` `jobs_pending` value
    /// fleet schedulers use as an affinity tiebreak.
    pub fn pending_count(&self) -> u64 {
        self.queue.len() as u64 + self.running.is_some() as u64
    }

    pub fn submit(
        &mut self,
        params: GenerateParams,
        policy: QueuePolicy,
    ) -> Result<String, AssetAiError> {
        if policy == QueuePolicy::Reject && self.is_busy() {
            return Err(AssetAiError::Busy);
        }
        if self.queue.len() >= self.queue_limit {
            return Err(AssetAiError::QueueFull(self.queue_limit));
        }
        self.next_id += 1;
        let id = format!("job-{}", self.next_id);
        let model = params.model.clone();
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
                log: Vec::new(),
                log_phase: String::new(),
                log_at_ms: 0,
            },
        );
        self.queue.push_back(id.clone());
        Ok(id)
    }

    /// Pops the next queued job and marks it running. Returns None while a
    /// job is already running (one GPU = one job) or the queue is empty.
    pub fn take_next(&mut self) -> Option<String> {
        if self.running.is_some() {
            return None;
        }
        let id = self.queue.pop_front()?;
        if let Some(job) = self.jobs.get_mut(&id) {
            job.state = JobState::Running {
                stage: "starting".to_string(),
                progress: 0.0,
            };
            job.started_ms = Some(now_ms());
        }
        self.running = Some(id.clone());
        Some(id)
    }

    /// Moves the potentially large/sensitive request into the worker. A job
    /// has one execution attempt; retaining or cloning these bytes after it
    /// starts only wastes memory and keeps expired transfer tickets alive.
    pub fn take_params(&mut self, id: &str) -> Option<GenerateParams> {
        self.jobs.get_mut(id)?.params.take()
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
                JobState::Running { .. } => {
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
        if self.running.as_deref() == Some(id) {
            self.running = None;
        }
    }

    pub fn set_partial_text(&mut self, id: &str, text: String) {
        if let Some(job) = self.jobs.get_mut(id) {
            job.partial_text = Some(text);
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

    pub fn finish(&mut self, id: &str, artifacts: Vec<ArtifactRefJson>) {
        if let Some(job) = self.jobs.get_mut(id) {
            job.state = JobState::Done { artifacts };
            job.finished_ms = Some(now_ms());
        }
        self.finished.push_back(id.to_string());
        if self.running.as_deref() == Some(id) {
            self.running = None;
        }
    }

    pub fn fail(&mut self, id: &str, message: String) {
        if let Some(job) = self.jobs.get_mut(id) {
            job.state = JobState::Error { message };
            job.finished_ms = Some(now_ms());
        }
        self.finished.push_back(id.to_string());
        if self.running.as_deref() == Some(id) {
            self.running = None;
        }
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
    fn queued_detail(&self, id: &str) -> String {
        let ahead = self.queue.iter().take_while(|q| *q != id).count();
        let busy = self.running.as_ref().and_then(|r| {
            self.jobs.get(r).and_then(|job| match &job.state {
                JobState::Running { stage, progress } => {
                    Some(format!("{stage} {:.0}%", progress * 100.0))
                }
                _ => None,
            })
        });
        match busy {
            Some(busy) => format!("{ahead} ahead; box: {busy}"),
            None => format!("{ahead} ahead"),
        }
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
        params: GenerateParams,
        policy: QueuePolicy,
    ) -> Result<String, AssetAiError> {
        self.with(|store| store.submit(params, policy))
    }

    /// Blocks (with a timeout so the worker stays responsive to process
    /// shutdown) until a job can start.
    pub fn wait_take_next(&self, timeout: Duration) -> Option<String> {
        let mut store = self.inner.0.lock().unwrap();
        if let Some(id) = store.take_next() {
            return Some(id);
        }
        let (mut store, _timed_out) = self.inner.1.wait_timeout(store, timeout).unwrap();
        store.take_next()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(model: &str) -> GenerateParams {
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
            pull_only: false,
            input_bytes: Vec::new(),
            input_content_type: String::new(),
            frames: None,
            codec: String::new(),
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
            peer_sources: Vec::new(),
            peer_tickets: Vec::new(),
        }
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
}
