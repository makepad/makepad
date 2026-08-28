//! Hosting the vision-annotation queue: the worker that drains it, and the
//! numbers the app draws.
//!
//! The queue itself is the STORE's (`makepad_asset_store::host::annotate`):
//! the server mints one `annotate.asset` job per newly live mesh/character/
//! prop/… whoever published it, so an import, a generation and a game agent
//! all get their content described without any of them knowing this exists.
//! That is the whole reason it is not an import-queue job — the import
//! queue only knows about imports, and it ends when the window does.
//!
//! This app plays two roles against that queue:
//!
//! * **worker** — [`makepad_asset_annotate::worker`] runs on a thread here,
//!   beside the embedded server, exactly as the generation coordinator does
//!   (`start_job_loop`). One shared loop, two hosts: the standalone
//!   `makepad-asset-annotate --worker` daemon is the same code.
//! * **operator** — the Annotate buttons ask the server to sweep the
//!   backlog into the queue (ONE request; the server picks the assets), and
//!   a poll thread reads `GET /v1/annotate/summary` so the bar says what is
//!   actually true rather than what this process happens to have done.

use makepad_asset_annotate::executor::{self, ExecutorEnv};
use makepad_asset_annotate::pass::SheetPrep;
use makepad_asset_annotate::worker::{self, WorkerConfig};
use makepad_asset_client::{
    AnnotateSummaryDto, ApiEndpoints, AssetClient, ClientConfig,
};
use makepad_widgets::log;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How often the bar asks the server what is true.
///
/// The summary is two correlated-subquery counts over the annotatable set
/// plus one grouped job read, and it runs on the store's exclusive state
/// thread: measured on the operator's own 4531-asset catalog it is ~21 ms +
/// ~21 ms + ~0.2 ms warm. At this cadence that is under 1% of that thread,
/// and the counts only move once per batch (~55 s) anyway.
const POLL_EVERY: Duration = Duration::from_secs(5);
/// Assets one backlog sweep queues (the server's own ceiling).
const SWEEP: u64 = 1000;
/// Top up the queue when it falls below this while a backlog is armed, so
/// a 4023-asset library drains from one button press without a client loop
/// that has to survive for hours.
const TOP_UP_BELOW: u64 = 200;

/// Sheets per executor invocation. Every invocation pays a one-off model
/// load (~15 s on the fleet box), so this is the difference between 4 and
/// 20 seconds per described asset.
const DEFAULT_BATCH: usize = 16;

enum Msg {
    Summary(AnnotateSummaryDto),
    Note(String),
    Error(String),
}

/// What the app knows about the queue.
pub struct AnnotateQueue {
    stop: Arc<AtomicBool>,
    threads: Vec<std::thread::JoinHandle<()>>,
    rx: Option<Receiver<Msg>>,
    tx: Option<Sender<Msg>>,
    /// Set while a backlog sweep should keep topping the queue up.
    armed: Arc<AtomicBool>,
    session: Option<Session>,
    /// "remote qwen38-27b" — or why there is no executor at all.
    pub executor: String,
    pub summary: Option<AnnotateSummaryDto>,
    pub note: String,
    pub error: Option<String>,
    /// Measured from summary deltas: seconds per described asset.
    rate: Option<f64>,
    mark: Option<(Instant, u64)>,
}

#[derive(Clone)]
struct Session {
    endpoints: ApiEndpoints,
    server_id: Option<[u8; 16]>,
    token: String,
    cache_root: PathBuf,
}

impl Default for AnnotateQueue {
    fn default() -> Self {
        Self {
            stop: Arc::new(AtomicBool::new(false)),
            threads: Vec::new(),
            rx: None,
            tx: None,
            armed: Arc::new(AtomicBool::new(false)),
            session: None,
            executor: String::new(),
            summary: None,
            note: String::new(),
            error: None,
            rate: None,
            mark: None,
        }
    }
}

impl Drop for AnnotateQueue {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

impl AnnotateQueue {
    pub fn running(&self) -> bool {
        self.rx.is_some()
    }

    /// Start the worker and the summary poll against the server this
    /// process is talking to. Idempotent: a second call is ignored.
    ///
    /// A missing executor does NOT stop the poll: the bar still says how
    /// much the catalog owes, and the reason nothing is draining it is on
    /// screen instead of nowhere.
    pub fn start(
        &mut self,
        endpoints: ApiEndpoints,
        server_id: Option<[u8; 16]>,
        token: String,
        home: PathBuf,
    ) {
        if self.running() {
            return;
        }
        let session = Session {
            endpoints,
            server_id,
            token: token.clone(),
            cache_root: home.join("annotate-worker"),
        };
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        self.tx = Some(tx.clone());
        self.stop = Arc::new(AtomicBool::new(false));
        self.session = Some(session.clone());

        // ---- the worker
        let repo = crate::asset_store_state::checkout_root();
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(std::path::Path::to_path_buf))
            .unwrap_or_else(|| repo.join("target/release"));
        match executor::choose_real(&ExecutorEnv::from_env(repo, exe_dir)) {
            Ok(choice) => {
                self.executor = choice.label();
                log!(
                    "annotate: executor {} · argv {:?}",
                    choice.label(),
                    choice.argv
                );
                let batch = std::env::var("MAKEPAD_VLM_BATCH")
                    .ok()
                    .and_then(|v| v.trim().parse::<usize>().ok())
                    .filter(|n| *n > 0)
                    .unwrap_or(DEFAULT_BATCH);
                let cfg = WorkerConfig {
                    endpoints,
                    server_id,
                    token: token.clone(),
                    cache_root: session.cache_root.clone(),
                    work: home.join("annotate-work"),
                    data: endpoints.data,
                    executor: choice,
                    batch,
                    suffix: "annotate".to_string(),
                    version: makepad_asset_annotate::ANNOTATOR_VERSION,
                    prep: SheetPrep::default(),
                    log: true,
                };
                let stop = self.stop.clone();
                if let Ok(t) = std::thread::Builder::new()
                    .name("annotate-worker".into())
                    .spawn(move || worker::run(&cfg, &stop))
                {
                    self.threads.push(t);
                }
            }
            Err(e) => {
                self.executor = "no executor".to_string();
                self.error = Some(e.clone());
                log!("annotate: {e}");
            }
        }

        // ---- the summary poll, which also tops up an armed backlog
        let stop = self.stop.clone();
        let armed = self.armed.clone();
        if let Ok(t) = std::thread::Builder::new()
            .name("annotate-poll".into())
            .spawn(move || poll_loop(session, tx, stop, armed))
        {
            self.threads.push(t);
        }
    }

    /// Ask the server to sweep un-annotated assets into the queue.
    ///
    /// `category` narrows it to one kit; `None` is the whole catalog. One
    /// request goes out now, and the poll thread keeps the queue topped up
    /// until nothing is owed — a library's backlog is hours of GPU work and
    /// must not depend on a loop inside a button handler.
    pub fn sweep(&mut self, category: Option<String>) {
        let Some(session) = self.session.clone() else {
            self.error = Some("no server session yet".to_string());
            return;
        };
        let Some(tx) = self.tx.clone() else {
            return;
        };
        if category.is_none() {
            self.armed.store(true, Ordering::Release);
        }
        let label = category.clone().unwrap_or_else(|| "every kit".to_string());
        self.note = format!("queueing {label}…");
        let _ = std::thread::Builder::new()
            .name("annotate-sweep".into())
            .spawn(move || {
                let client = match connect(&session, "sweep") {
                    Ok(c) => c,
                    Err(e) => {
                        let _ = tx.send(Msg::Error(e));
                        return;
                    }
                };
                match client.annotate_backlog(SWEEP, category.as_deref(), 0) {
                    Ok(r) => {
                        let _ = tx.send(Msg::Note(format!(
                            "queued {} of {label} · {} already done · {} still owed",
                            r.enqueued, r.annotated, r.remaining
                        )));
                    }
                    Err(e) => {
                        let _ = tx.send(Msg::Error(format!("backlog sweep refused: {e}")));
                    }
                }
            });
    }

    /// Drain worker/poll messages. True when something changed.
    pub fn poll(&mut self) -> bool {
        if self.rx.is_none() {
            return false;
        }
        let mut changed = false;
        loop {
            let msg = self.rx.as_ref().map(|rx| rx.try_recv());
            match msg {
                Some(Ok(Msg::Summary(s))) => {
                    self.observe_rate(&s);
                    if s.owed == 0 {
                        self.armed.store(false, Ordering::Release);
                    }
                    self.summary = Some(s);
                    changed = true;
                }
                Some(Ok(Msg::Note(n))) => {
                    log!("annotate: {n}");
                    self.note = n;
                    changed = true;
                }
                Some(Ok(Msg::Error(e))) => {
                    log!("annotate: {e}");
                    self.error = Some(e);
                    changed = true;
                }
                Some(Err(TryRecvError::Empty)) | None => break,
                Some(Err(TryRecvError::Disconnected)) => {
                    self.rx = None;
                    break;
                }
            }
        }
        changed
    }

    /// Seconds per described asset, measured from what the server reports
    /// rather than from anything this process timed.
    fn observe_rate(&mut self, next: &AnnotateSummaryDto) {
        match self.mark {
            Some((at, annotated)) if next.annotated > annotated => {
                let gained = (next.annotated - annotated) as f64;
                let per = at.elapsed().as_secs_f64() / gained;
                // Smoothed, so one slow batch does not make the estimate
                // jump around while somebody is reading it.
                self.rate = Some(match self.rate {
                    Some(prev) => prev * 0.7 + per * 0.3,
                    None => per,
                });
                self.mark = Some((Instant::now(), next.annotated));
            }
            None => self.mark = Some((Instant::now(), next.annotated)),
            _ => {}
        }
    }

    /// The one line the app draws. Empty when there is nothing to say.
    pub fn status_line(&self) -> String {
        let Some(s) = &self.summary else {
            return String::new();
        };
        let total = s.annotated + s.owed;
        if total == 0 {
            return String::new();
        }
        let mut line = format!("Annotation · {}/{}", s.annotated, total);
        if s.queued() > 0 {
            line.push_str(&format!(" · {} queued", s.queued()));
        }
        if s.failed > 0 {
            line.push_str(&format!(" · {} failed", s.failed));
        }
        if let Some(rate) = self.rate.filter(|_| s.queued() > 0) {
            line.push_str(&format!(" · {rate:.1} s/asset"));
            let left = (s.owed as f64 * rate / 60.0).round() as u64;
            if left > 0 {
                line.push_str(&if left >= 120 {
                    format!(" · ~{} h left", left / 60)
                } else {
                    format!(" · ~{left} min left")
                });
            }
        }
        if !self.executor.is_empty() {
            line.push_str(&format!(" · {}", self.executor));
        }
        if let Some(e) = &self.error {
            line.push_str(" · ");
            line.push_str(e);
        }
        line
    }

    /// The header chip: short enough to sit beside the connection state.
    pub fn chip(&self) -> String {
        let Some(s) = &self.summary else {
            return String::new();
        };
        let total = s.annotated + s.owed;
        if total == 0 {
            return String::new();
        }
        if s.queued() == 0 && s.owed == 0 {
            return format!("SEARCHABLE · {total}");
        }
        format!("SEARCHABLE · {}/{}", s.annotated, total)
    }

    /// 0..1 over the annotatable catalog.
    pub fn progress_fraction(&self) -> f32 {
        let Some(s) = &self.summary else {
            return 0.0;
        };
        let total = s.annotated + s.owed;
        if total == 0 {
            return 0.0;
        }
        (s.annotated as f32 / total as f32).clamp(0.0, 1.0)
    }

    /// Whether the bar is worth a row: something is queued, owed or broken.
    pub fn has_work(&self) -> bool {
        self.summary
            .as_ref()
            .map(|s| s.owed > 0 || s.queued() > 0 || s.failed > 0)
            .unwrap_or(false)
    }
}

fn connect(session: &Session, tag: &str) -> Result<AssetClient, String> {
    // Every client gets its OWN cache dir: an AssetClient cache is
    // single-owner and a shared one refuses every fetch.
    let mut cfg = ClientConfig::new(session.cache_root.join(tag));
    cfg.token = Some(session.token.clone());
    AssetClient::connect(cfg, session.endpoints, session.server_id)
        .map_err(|e| format!("connect: {e}"))
}

fn poll_loop(session: Session, tx: Sender<Msg>, stop: Arc<AtomicBool>, armed: Arc<AtomicBool>) {
    let client = match connect(&session, "poll") {
        Ok(c) => c,
        Err(e) => {
            let _ = tx.send(Msg::Error(e));
            return;
        }
    };
    while !stop.load(Ordering::Acquire) {
        match client.annotate_summary(None) {
            Ok(summary) => {
                // Keep an armed backlog fed. The server caps one sweep, so
                // this is what turns one button press into a whole library
                // without a loop anybody has to keep alive by hand.
                if armed.load(Ordering::Acquire)
                    && summary.owed > 0
                    && summary.queued() < TOP_UP_BELOW
                {
                    match client.annotate_backlog(SWEEP, None, 0) {
                        Ok(r) if r.enqueued > 0 => {
                            let _ = tx.send(Msg::Note(format!(
                                "topped the queue up with {} · {} still owed",
                                r.enqueued, r.remaining
                            )));
                        }
                        Ok(_) => armed.store(false, Ordering::Release),
                        Err(e) => {
                            let _ = tx.send(Msg::Error(format!("top-up refused: {e}")));
                            armed.store(false, Ordering::Release);
                        }
                    }
                }
                let _ = tx.send(Msg::Summary(summary));
            }
            Err(e) => {
                let _ = tx.send(Msg::Error(format!("summary: {e}")));
                // A server that is not up yet is not an error worth
                // hammering; back off to the same cadence either way.
            }
        }
        let deadline = Instant::now() + POLL_EVERY;
        while Instant::now() < deadline {
            if stop.load(Ordering::Acquire) {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(annotated: u64, owed: u64, pending: u64, running: u64) -> AnnotateSummaryDto {
        AnnotateSummaryDto {
            version_tag: "vlm-v7".into(),
            owed,
            annotated,
            pending,
            running,
            succeeded: annotated,
            failed: 0,
            cancelled: 0,
        }
    }

    #[test]
    fn the_store_and_the_pass_agree_on_the_annotator_version() {
        // The store cannot depend on the annotate crate, so it mirrors the
        // version — and this app links both. A drift here means the server
        // queues work the pass thinks is done, or never queues what it owes.
        assert_eq!(
            makepad_asset_store::host::annotate::ANNOTATOR_VERSION,
            makepad_asset_annotate::ANNOTATOR_VERSION,
        );
        assert_eq!(
            makepad_asset_store::host::annotate::version_tag(),
            format!("vlm-v{}", makepad_asset_annotate::ANNOTATOR_VERSION),
        );
        // And on the job kind, which is the wire between them.
        assert_eq!(
            makepad_asset_store::host::annotate::JOB_KIND,
            makepad_asset_annotate::worker::JOB_KIND,
        );
    }

    #[test]
    fn the_line_says_how_much_of_the_catalog_is_searchable() {
        let mut q = AnnotateQueue::default();
        q.executor = "remote qwen38-27b".into();
        assert_eq!(q.status_line(), "", "nothing known yet says nothing");
        q.summary = Some(summary(312, 3711, 700, 16));
        let line = q.status_line();
        assert!(line.starts_with("Annotation · 312/4023 · 716 queued"), "{line}");
        assert!(line.ends_with("remote qwen38-27b"), "{line}");
        assert!((q.progress_fraction() - 312.0 / 4023.0).abs() < 1e-5);
        assert!(q.has_work());
    }

    #[test]
    fn a_finished_catalog_is_a_chip_not_a_bar() {
        let mut q = AnnotateQueue::default();
        q.summary = Some(summary(4023, 0, 0, 0));
        assert_eq!(q.chip(), "SEARCHABLE · 4023");
        assert!(!q.has_work());
    }

    #[test]
    fn the_rate_and_the_estimate_come_from_the_servers_own_counts() {
        let mut q = AnnotateQueue::default();
        q.mark = Some((Instant::now() - Duration::from_secs(40), 100));
        q.observe_rate(&summary(110, 100, 50, 1));
        let rate = q.rate.expect("a rate after the count moved");
        assert!((rate - 4.0).abs() < 0.5, "{rate}");
        q.summary = Some(summary(110, 100, 50, 1));
        let line = q.status_line();
        assert!(line.contains("s/asset"), "{line}");
        assert!(line.contains("min left"), "{line}");
    }

    #[test]
    fn a_failure_is_visible_in_the_line() {
        let mut q = AnnotateQueue::default();
        let mut s = summary(10, 5, 0, 0);
        s.failed = 3;
        q.summary = Some(s);
        assert!(q.status_line().contains("3 failed"), "{}", q.status_line());
        assert!(q.has_work());
    }
}
