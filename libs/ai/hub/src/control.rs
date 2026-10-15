//! What the hub control panel reads: a change counter for the long-poll
//! watch, per-job facts the job store does not keep (who asked, from which
//! app, what kind of work, decode rate), and the GPU memory each model was
//! measured to take when it loaded.
//!
//! The snapshot itself is assembled in `server.rs` (it needs the service
//! state); this module holds only the process-wide pieces, so the job store
//! and the backends can report into it without a handle. Contract:
//! local/agent_state/fast-rust/hub-vision/CONTROL_API.md.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// The request header naming the calling app.
pub const HEADER_APP: &str = "X-Hub-App";
/// Watchers served at once.
pub const MAX_WATCHERS: usize = 16;
/// A watcher gets at most one answer per this interval.
pub const WATCH_COALESCE: Duration = Duration::from_millis(250);
pub const WATCH_TIMEOUT_DEFAULT: Duration = Duration::from_secs(25);
pub const WATCH_TIMEOUT_MAX: Duration = Duration::from_secs(55);
/// Finished jobs listed under `recent`.
pub const RECENT: usize = 20;
/// Job facts kept (active + recent, with room to spare).
const META_KEEP: usize = 256;

static SEQ: AtomicU64 = AtomicU64::new(1);
static WATCHERS: AtomicUsize = AtomicUsize::new(0);

fn wake() -> &'static (Mutex<()>, Condvar) {
    static WAKE: OnceLock<(Mutex<()>, Condvar)> = OnceLock::new();
    WAKE.get_or_init(|| (Mutex::new(()), Condvar::new()))
}

/// Something the panel shows changed.
pub fn changed() {
    SEQ.fetch_add(1, Ordering::Relaxed);
    let (lock, cvar) = wake();
    let _guard = lock.lock().unwrap();
    cvar.notify_all();
}

pub fn seq() -> u64 {
    SEQ.load(Ordering::Relaxed)
}

/// Blocks until the counter passes `since` or `timeout` runs out, then
/// waits out the coalescing interval so a burst of changes is one answer.
pub fn wait_changed(since: u64, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    let (lock, cvar) = wake();
    let mut guard = lock.lock().unwrap();
    while seq() <= since {
        let now = Instant::now();
        if now >= deadline {
            return;
        }
        guard = cvar.wait_timeout(guard, deadline - now).unwrap().0;
    }
    drop(guard);
    std::thread::sleep(WATCH_COALESCE);
}

/// A watcher slot; `None` when [`MAX_WATCHERS`] are already waiting.
pub struct WatchSlot(());

impl WatchSlot {
    pub fn take() -> Option<Self> {
        let prev = WATCHERS.fetch_add(1, Ordering::AcqRel);
        if prev >= MAX_WATCHERS {
            WATCHERS.fetch_sub(1, Ordering::AcqRel);
            return None;
        }
        Some(WatchSlot(()))
    }
}

impl Drop for WatchSlot {
    fn drop(&mut self) {
        WATCHERS.fetch_sub(1, Ordering::AcqRel);
    }
}

pub fn app_from_headers(lines: &[String]) -> Option<String> {
    lines.iter().find_map(|line| {
        let (name, value) = line.trim_end().split_once(':')?;
        if !name.trim().eq_ignore_ascii_case(HEADER_APP) {
            return None;
        }
        let value: String = value
            .trim()
            .chars()
            .filter(|c| c.is_ascii_graphic() || *c == ' ')
            .take(64)
            .collect();
        (!value.is_empty()).then_some(value)
    })
}

/// This process's name, for the `X-Hub-App` header the fleet client sends.
pub fn own_app_name() -> &'static str {
    static NAME: OnceLock<String> = OnceLock::new();
    NAME.get_or_init(|| {
        std::env::current_exe()
            .ok()
            .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().to_string()))
            .unwrap_or_else(|| "unknown".to_string())
    })
}

/// What the job store does not know about a job.
#[derive(Clone, Debug, Default)]
pub struct JobMeta {
    pub client: String,
    pub app: Option<String>,
    pub kind: String,
    /// When the first generated token was reported, and how many there were
    /// at the last report: the decode rate excludes prefill and load.
    pub first_token: Option<Instant>,
    pub first_token_count: u64,
    pub last_token: Option<Instant>,
    pub gen_tokens: u64,
}

impl JobMeta {
    /// Decode rate since the first token, when there were at least two
    /// reports to measure between.
    pub fn tok_s(&self) -> Option<f64> {
        let (first, last) = (self.first_token?, self.last_token?);
        let secs = last.duration_since(first).as_secs_f64();
        let tokens = self.gen_tokens.saturating_sub(self.first_token_count);
        (secs > 0.05 && tokens > 0).then(|| tokens as f64 / secs)
    }
}

#[derive(Default)]
struct Meta {
    jobs: HashMap<String, JobMeta>,
    order: VecDeque<String>,
    /// GPU MB a model took when it loaded (free-memory drop across the load).
    model_mb: HashMap<String, u64>,
}

fn meta() -> &'static Mutex<Meta> {
    static META: OnceLock<Mutex<Meta>> = OnceLock::new();
    META.get_or_init(|| Mutex::new(Meta::default()))
}

pub fn note_job(job_id: &str, client: &str, app: Option<&str>, kind: &str) {
    let mut meta = meta().lock().unwrap();
    meta.jobs.insert(
        job_id.to_string(),
        JobMeta {
            client: client.to_string(),
            app: app.map(str::to_string),
            kind: kind.to_string(),
            ..JobMeta::default()
        },
    );
    meta.order.push_back(job_id.to_string());
    while meta.order.len() > META_KEEP {
        if let Some(old) = meta.order.pop_front() {
            meta.jobs.remove(&old);
        }
    }
    drop(meta);
    changed();
}

/// A generated-token count from the serving path.
pub fn note_tokens(job_id: &str, generated: u64) {
    let mut meta = meta().lock().unwrap();
    if let Some(job) = meta.jobs.get_mut(job_id) {
        let now = Instant::now();
        if job.first_token.is_none() && generated > 0 {
            job.first_token = Some(now);
            job.first_token_count = generated;
        }
        job.last_token = Some(now);
        job.gen_tokens = generated;
    }
}

pub fn job_meta(job_id: &str) -> Option<JobMeta> {
    meta().lock().unwrap().jobs.get(job_id).cloned()
}

pub fn set_model_mb(model: &str, mb: Option<u64>) {
    let mut meta = meta().lock().unwrap();
    match mb {
        Some(mb) => meta.model_mb.insert(model.to_string(), mb),
        None => meta.model_mb.remove(model),
    };
}

pub fn model_mb(model: &str) -> Option<u64> {
    meta().lock().unwrap().model_mb.get(model).copied()
}

static VISION_TOWER: AtomicU64 = AtomicU64::new(0); // 0 none, 1 has tower, 2 loaded

/// The chat session's vision tower: whether the model has one, and whether
/// it is resident now.
pub fn set_vision(has_tower: bool, loaded: bool) {
    VISION_TOWER.store(if loaded { 2 } else { has_tower as u64 }, Ordering::Relaxed);
    changed();
}

pub fn vision_loaded() -> bool {
    VISION_TOWER.load(Ordering::Relaxed) == 2
}

/// "lazy" | "start" | "off" | "none" (no tower known).
pub fn vision_mode_name() -> &'static str {
    let mode = std::env::var("MAKEPAD_ASSET_AI_LLM_VISION").map(|v| v.trim().to_ascii_lowercase()).unwrap_or_default();
    if mode == "off" {
        return "off";
    }
    if VISION_TOWER.load(Ordering::Relaxed) == 0 {
        return "none";
    }
    if mode == "start" { "start" } else { "lazy" }
}

/// GPU memory this process holds (nvidia-smi per-process figure), cached
/// for a second; `None` without NVIDIA tools.
pub fn process_gpu_mb() -> Option<u64> {
    static CACHE: OnceLock<Mutex<Option<(Instant, Option<u64>)>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(None));
    let mut guard = cache.lock().unwrap();
    if let Some((when, value)) = *guard {
        if when.elapsed() < Duration::from_secs(1) {
            return value;
        }
    }
    let pid = std::process::id().to_string();
    let value = crate::child_process::output(
        std::process::Command::new("nvidia-smi")
            .args(["--query-compute-apps=pid,used_memory", "--format=csv,noheader,nounits"]),
    )
    .ok()
        .filter(|out| out.status.success())
        .and_then(|out| {
            String::from_utf8_lossy(&out.stdout).lines().find_map(|line| {
                let (p, mb) = line.split_once(',')?;
                (p.trim() == pid).then(|| mb.trim().parse::<u64>().ok()).flatten()
            })
        });
    *guard = Some((Instant::now(), value));
    value
}

/// The panel's kind for a job: registry domain, refined for the LLM by what
/// the request is (a chat turn with pictures is `vision`).
pub fn job_kind(domain: &str, backend: &str, target_domain: &str, has_images: bool) -> String {
    if backend == "llm" {
        return match (target_domain, has_images) {
            ("chat", true) => "vision",
            ("chat", false) => "chat",
            _ => "expand",
        }
        .to_string();
    }
    model_kind(domain).to_string()
}

/// The panel's kind for a model, from its registry domain.
pub fn model_kind(domain: &str) -> &'static str {
    match domain {
        "text" => "chat",
        "vision" | "ocr" => "vision",
        "stt" => "stt",
        "speech" => "tts",
        "music" | "audio" | "beats" | "stems" | "notes" | "sections" => "music",
        "image" | "edit" | "inpaint" | "control" | "upscale" | "matte" | "depth" | "segment"
        | "enhance" | "paint" | "garment" => "image",
        "video" => "video",
        "mesh" | "splat" | "rig" | "motion" | "world" | "body" => "mesh",
        _ => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_header_is_read_and_sanitised() {
        let lines = vec!["Host: x".to_string(), "x-hub-app: makepad-app-code\r".to_string()];
        assert_eq!(app_from_headers(&lines).as_deref(), Some("makepad-app-code"));
        assert_eq!(app_from_headers(&["X-Hub-App: \u{1}".to_string()]), None);
    }

    #[test]
    fn decode_rate_starts_at_the_first_token() {
        let mut job = JobMeta::default();
        assert_eq!(job.tok_s(), None);
        let t0 = Instant::now();
        job.first_token = Some(t0);
        job.first_token_count = 3;
        job.last_token = Some(t0 + Duration::from_secs(2));
        job.gen_tokens = 103;
        assert!((job.tok_s().unwrap() - 50.0).abs() < 1e-9);
    }

    #[test]
    fn a_change_wakes_a_watcher() {
        let since = seq();
        let waiter = std::thread::spawn(move || {
            let started = Instant::now();
            wait_changed(since, Duration::from_secs(10));
            started.elapsed()
        });
        std::thread::sleep(Duration::from_millis(50));
        changed();
        assert!(waiter.join().unwrap() < Duration::from_secs(5));
    }

    #[test]
    fn watcher_slots_are_bounded() {
        let slots: Vec<_> = (0..MAX_WATCHERS).filter_map(|_| WatchSlot::take()).collect();
        assert_eq!(slots.len(), MAX_WATCHERS);
        assert!(WatchSlot::take().is_none());
        drop(slots);
        assert!(WatchSlot::take().is_some());
    }
}

/// Whether a running job's `stage` is still getting its model ready (a download, a
/// weight load, making room for it) rather than generating with it.
pub fn is_load_stage(stage: &str) -> bool {
    let stage = stage.trim_start();
    stage.is_empty()
        || stage.starts_with("load")
        || stage.starts_with("download")
        || stage.starts_with("evict")
        || stage.starts_with("using ")
        || stage.starts_with("queued-for-memory")
        || stage.contains("switching models")
}

#[cfg(test)]
mod load_stage_tests {
    #[test]
    fn load_stages_are_told_from_generation() {
        for stage in ["", "load t5 0.5/4.6GB", "download", "using flux1-schnell-q4: …", "queued-for-memory: need 1 MB", "switching models: evicting x"] {
            assert!(super::is_load_stage(stage), "{stage}");
        }
        for stage in ["denoise 3/4", "text-encode t5 block 4/24", "decode", "generate 120/400"] {
            assert!(!super::is_load_stage(stage), "{stage}");
        }
    }
}
