//! Controlling the vision-annotation queue: what the app asks of it, and
//! the numbers it draws.
//!
//! The queue itself is the STORE's (`makepad_asset_store::host::annotate`):
//! the server mints one `annotate.asset` job per newly live mesh/character/
//! prop/… whoever published it, so an import, a generation and a game agent
//! all get their content described without any of them knowing this exists.
//! That is the whole reason it is not an import-queue job — the import
//! queue only knows about imports, and it ends when the window does.
//!
//! The WORK is not done here. `annotate.asset` is a `vision` job like every
//! other GPU kind, claimed off that queue by the fleet coordinator this app
//! already hosts (`asset_store_state::start_job_loop`) and answered on a box
//! whose `/health` advertises the vision capability. So this module is the
//! OPERATOR:
//!
//! * the Annotate buttons ask the server to sweep the backlog into the
//!   queue (ONE request; the server picks the assets, and keeps it topped
//!   up while a whole-catalog sweep is armed),
//! * Pause cancels what is still queued and stops the top-up — the boxes
//!   that are mid-answer finish, because interrupting a GPU halfway buys
//!   nothing — and Resume sweeps again, which requeues exactly what was
//!   cancelled (the job id is derived from the asset, so a cancelled job is
//!   the same job when it comes back),
//! * a poll thread reads `GET /v1/annotate/summary` and the running jobs, so
//!   the bar says what is actually true — including which box is answering
//!   right now, from the workers' own heartbeats.

use makepad_asset_client::{
    AnnotateSummaryDto, ApiEndpoints, AssetClient, ClientConfig, JobRowDto, JobStateDto,
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
/// and a described asset lands every few seconds at best anyway.
const POLL_EVERY: Duration = Duration::from_secs(5);
/// Assets one backlog sweep queues (the server's own ceiling).
const SWEEP: u64 = 1000;
/// Top up the queue when it falls below this while a backlog is armed, so
/// a 4023-asset library drains from one button press without a client loop
/// that has to survive for hours.
const TOP_UP_BELOW: u64 = 200;

/// The job kind the queue holds, spelled where the kind table spells it.
const JOB_KIND: &str = "annotate.asset";
/// Running jobs one poll looks at. More boxes than this on one annotation
/// backlog would be a fleet nobody has.
const RUNNING_PAGE: u64 = 64;
/// Queued jobs one Pause cancels per page, and how many pages it will walk
/// before leaving the rest to the next press (a whole-catalog backlog is
/// thousands of jobs and this runs on one thread).
const CANCEL_PAGE: u64 = 500;
const CANCEL_PAGES: usize = 20;

enum Msg {
    Summary(AnnotateSummaryDto),
    /// One line per box answering right now, from the running jobs.
    Boxes(Vec<String>),
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
    /// True while the operator has paused the backlog.
    pub paused: bool,
    /// One line per box answering right now.
    pub boxes: Vec<String>,
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
            paused: false,
            boxes: Vec::new(),
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

    /// Start the summary poll against the server this process is talking
    /// to. Idempotent: a second call is ignored.
    ///
    /// Nothing is spawned to DO the work: `annotate.asset` is claimed by the
    /// fleet coordinator this app already runs, on whichever box advertises
    /// the `vision` capability. If no box does, the bar still says how much
    /// the catalog owes and the queue simply waits — which is the honest
    /// picture, and exactly what happens when a GPU is switched off.
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
            token,
            cache_root: home.join("annotate-client"),
        };
        let (tx, rx) = mpsc::channel();
        self.rx = Some(rx);
        self.tx = Some(tx.clone());
        self.stop = Arc::new(AtomicBool::new(false));
        self.session = Some(session.clone());

        let stop = self.stop.clone();
        let armed = self.armed.clone();
        if let Ok(t) = std::thread::Builder::new()
            .name("annotate-poll".into())
            .spawn(move || poll_loop(session, tx, stop, armed))
        {
            self.threads.push(t);
        }
    }

    /// Stop topping the backlog up and cancel what is still queued.
    ///
    /// Jobs a box is already answering are LEFT: a vision answer is seconds
    /// of GPU time that is already spent, and killing it mid-flight would
    /// only mean paying for it again. Cancelled jobs are terminal, and the
    /// store's derived job id turns Resume into a requeue of exactly these
    /// assets rather than a new pile of work.
    pub fn pause(&mut self) {
        self.armed.store(false, Ordering::Release);
        self.paused = true;
        let Some(session) = self.session.clone() else {
            return;
        };
        let Some(tx) = self.tx.clone() else { return };
        self.note = "pausing · cancelling what is still queued…".to_string();
        let _ = std::thread::Builder::new()
            .name("annotate-pause".into())
            .spawn(move || {
                let client = match connect(&session, "pause") {
                    Ok(c) => c,
                    Err(e) => {
                        let _ = tx.send(Msg::Error(e));
                        return;
                    }
                };
                let mut cancelled = 0u64;
                for _ in 0..CANCEL_PAGES {
                    let page = match client.list_jobs(
                        None,
                        Some(JOB_KIND),
                        Some(JobStateDto::Pending),
                        CANCEL_PAGE,
                    ) {
                        Ok(page) => page,
                        Err(e) => {
                            let _ = tx.send(Msg::Error(format!("pause: {e}")));
                            return;
                        }
                    };
                    if page.is_empty() {
                        break;
                    }
                    for row in &page {
                        if client.cancel_job(&row.job).is_ok() {
                            cancelled += 1;
                        }
                    }
                }
                let _ = tx.send(Msg::Note(format!(
                    "paused · {cancelled} cancelled · running jobs finish"
                )));
            });
    }

    /// Sweep again. Every asset Pause cancelled comes back as the same job
    /// (the id is derived from the asset and the annotator version), so this
    /// resumes the backlog instead of starting a second one.
    pub fn resume(&mut self) {
        self.paused = false;
        self.sweep(None);
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
        self.paused = false;
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
                Some(Ok(Msg::Boxes(boxes))) => {
                    if self.boxes != boxes {
                        self.boxes = boxes;
                        changed = true;
                    }
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
        if self.paused {
            line.push_str(" · paused");
        }
        // WHICH box is answering, from the running jobs' own heartbeats —
        // the only honest source, because the work is not happening in this
        // process at all.
        for text in self.boxes.iter().take(2) {
            line.push_str(" · ");
            line.push_str(text);
        }
        if self.boxes.len() > 2 {
            line.push_str(&format!(" · +{} more", self.boxes.len() - 2));
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
                let running = summary.running;
                let _ = tx.send(Msg::Summary(summary));
                // Which box is answering, and how fast. Only worth a request
                // when the server says something IS running.
                if running > 0 {
                    match client.list_jobs(
                        None,
                        Some(JOB_KIND),
                        Some(JobStateDto::Running),
                        RUNNING_PAGE,
                    ) {
                        Ok(rows) => {
                            let _ = tx.send(Msg::Boxes(box_lines(&rows)));
                        }
                        Err(e) => {
                            let _ = tx.send(Msg::Error(format!("running jobs: {e}")));
                        }
                    }
                } else {
                    let _ = tx.send(Msg::Boxes(Vec::new()));
                }
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

/// What one running job's heartbeat says about where it is running.
///
/// The note is the worker's (`vision · <model> @ <host> · <n> s`); this
/// reads it back rather than inventing a second format, so the line the app
/// draws and the line the worker wrote cannot drift.
fn parse_vision_note(note: &str) -> Option<(String, String, f64)> {
    let rest = note.strip_prefix("vision · ")?;
    let (model, rest) = rest.split_once(" @ ")?;
    let (host, seconds) = rest.split_once(" · ")?;
    let seconds = seconds.trim_end_matches(" s").trim().parse::<f64>().ok()?;
    Some((model.trim().to_string(), host.trim().to_string(), seconds))
}

/// One line per box that is answering right now: how many jobs it holds and
/// how long the oldest of them has been thinking.
///
/// A job whose worker has not reported a vision note yet (it is still
/// waiting for fleet admission) is counted as waiting rather than dropped —
/// a queue with nothing visible on any box is exactly the state an operator
/// needs to see.
pub fn box_lines(rows: &[JobRowDto]) -> Vec<String> {
    let mut boxes: Vec<(String, String, usize, f64)> = Vec::new();
    let mut waiting = 0usize;
    for row in rows {
        let parsed = row
            .progress
            .as_ref()
            .and_then(|p| parse_vision_note(&p.note));
        let Some((model, host, seconds)) = parsed else {
            waiting += 1;
            continue;
        };
        match boxes.iter_mut().find(|(h, m, _, _)| *h == host && *m == model) {
            Some((_, _, count, longest)) => {
                *count += 1;
                *longest = longest.max(seconds);
            }
            None => boxes.push((host, model, 1, seconds)),
        }
    }
    boxes.sort_by(|a, b| a.0.cmp(&b.0));
    let mut out: Vec<String> = boxes
        .into_iter()
        .map(|(host, model, count, seconds)| {
            let each = if count > 1 { format!(" ×{count}") } else { String::new() };
            format!("{host} {model}{each} · {seconds:.1} s")
        })
        .collect();
    if waiting > 0 {
        out.push(format!("{waiting} waiting for a vision box"));
    }
    out
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
        // And on the job kind, which is the wire between the store that
        // mints the job and the kind table the worker claims by.
        assert_eq!(
            makepad_asset_store::host::annotate::JOB_KIND,
            makepad_asset_importer::gen_kinds::kind_of("annotate.asset")
                .expect("the annotation kind is wired")
                .kind,
        );
        assert_eq!(makepad_asset_store::host::annotate::JOB_KIND, super::JOB_KIND);
    }

    #[test]
    fn the_line_says_how_much_of_the_catalog_is_searchable() {
        let mut q = AnnotateQueue::default();
        assert_eq!(q.status_line(), "", "nothing known yet says nothing");
        q.summary = Some(summary(312, 3711, 700, 16));
        q.boxes = vec!["10.0.0.203 qwen3.8-27b-vision ×2 · 3.4 s".to_string()];
        let line = q.status_line();
        assert!(line.starts_with("Annotation · 312/4023 · 716 queued"), "{line}");
        assert!(line.ends_with("10.0.0.203 qwen3.8-27b-vision ×2 · 3.4 s"), "{line}");
        assert!((q.progress_fraction() - 312.0 / 4023.0).abs() < 1e-5);
        assert!(q.has_work());
    }

    fn running_row(note: &str) -> JobRowDto {
        JobRowDto {
            job: makepad_asset_client::JobId::parse(&format!(
                "job_{:032x}",
                note.len() as u128 + 1
            ))
            .expect("job id"),
            namespace: "kenney".into(),
            kind: JOB_KIND.into(),
            state: JobStateDto::Running,
            enqueued_by: None,
            created_ms: 0,
            prompt: None,
            progress: (!note.is_empty()).then(|| makepad_asset_client::JobProgressDto {
                permille: 0,
                note: note.to_string(),
                updated_ms: None,
            }),
        }
    }

    /// The per-box lines come from the WORKERS' heartbeats, so the app can
    /// say which GPU is answering without knowing anything about the fleet.
    #[test]
    fn the_boxes_answering_come_from_the_running_jobs() {
        let rows = vec![
            running_row("vision · qwen3.8-27b-vision @ 10.0.0.203 · 3.4 s"),
            running_row("vision · qwen3.8-27b-vision @ 10.0.0.203 · 8.1 s"),
            running_row("vision · qwen3.8-27b-vision @ 10.0.0.217 · 2.0 s"),
            // Claimed, but not yet on a box.
            running_row("waiting-for-fleet-admission"),
            running_row(""),
        ];
        let lines = box_lines(&rows);
        assert_eq!(
            lines,
            vec![
                // Two on one box collapse into one line carrying the
                // LONGEST-running of them.
                "10.0.0.203 qwen3.8-27b-vision ×2 · 8.1 s".to_string(),
                "10.0.0.217 qwen3.8-27b-vision · 2.0 s".to_string(),
                "2 waiting for a vision box".to_string(),
            ]
        );
        assert!(box_lines(&[]).is_empty());
    }

    /// Pause is a state the line says out loud: an operator who paused a
    /// backlog must never wonder whether it is simply slow.
    #[test]
    fn a_paused_backlog_says_so() {
        let mut q = AnnotateQueue::default();
        q.summary = Some(summary(10, 90, 40, 0));
        assert!(!q.status_line().contains("paused"));
        q.paused = true;
        assert!(q.status_line().contains("· paused"), "{}", q.status_line());
        assert!(q.has_work(), "a paused backlog is still owed");
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
