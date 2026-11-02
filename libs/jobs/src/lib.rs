//! Jobs — one list of the long operations an app has running, for the
//! status bar's job strip, a jobs popover, or any other view of them.
//!
//! **One list per process.** Long work is started from many places (a
//! model worker, an export thread, an AI hub poll loop, a chat tool call)
//! and shown in one place. A process-wide board means the code that does
//! the work registers it where it is, without a handle threaded through the
//! UI tree to reach the strip, and every view of the work (the status bar,
//! a popover, a headset panel) reads the same snapshot.
//!
//! **The worker owns the words.** A job says what it is doing in plain
//! words: a title ("Building motor mount"), a stage ("fillets") with an
//! optional step count (3 of 7), and a fraction or none (indeterminate).
//! The board does not guess; a job that cannot say how far it is shows a
//! moving bar instead of a made-up percentage.
//!
//! **Cancel is cooperative.** A cancel raises the job's flag (an
//! `AtomicBool` the worker checks at its own sensible points) and calls the
//! job's optional `on_cancel` hook, which may wake a worker or forward the
//! cancel to a remote service. It never blocks: the hook runs on the
//! thread that pressed cancel, the UI thread, and must only signal. The job
//! stays listed as "Cancelling" until its worker confirms with
//! [`JobHandle::cancelled`], which records how long the cancel took.
//!
//! **Updates are cheap and coalesced.** Each update takes the board's lock
//! for a few field writes, bumps a sequence number and raises the UI signal
//! when something visible changed; the UI reads the sequence on
//! `Event::Signal` and redraws only then. Fraction updates that would not
//! move the bar by a visible amount do not wake the UI.
//!
//! **A dropped handle is a finished job.** A worker that unwinds or returns
//! early without saying how it ended drops its handle; the job then ends as
//! failed ("stopped without finishing") or, when a cancel was asked for, as
//! cancelled. Nothing is ever left spinning forever.

use std::cell::RefCell;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, OnceLock,
};

/// How the board wakes the UI and tells time, installed by the UI layer
/// (makepad-widgets installs its UI signal and its portable clock). Before
/// that, nothing is woken and the clock is the process's own.
#[derive(Clone, Copy)]
pub struct Hooks {
    /// Wake the UI thread: something a view shows changed.
    pub wake: fn(),
    /// Seconds on a monotonic clock.
    pub now: fn() -> f64,
    /// A line for the app's log (a cancel's latency).
    pub log: fn(&str),
}

static HOOKS: OnceLock<Hooks> = OnceLock::new();

/// Install the UI's wake and clock. The first call wins.
pub fn install(hooks: Hooks) {
    let _ = HOOKS.set(hooks);
}

fn wake() {
    if let Some(hooks) = HOOKS.get() {
        (hooks.wake)();
    }
}

fn log(line: &str) {
    match HOOKS.get() {
        Some(hooks) => (hooks.log)(line),
        None => eprintln!("{line}"),
    }
}

/// Seconds on the board's monotonic clock (the times in [`JobSnapshot`]).
pub fn now() -> f64 {
    if let Some(hooks) = HOOKS.get() {
        return (hooks.now)();
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        static START: OnceLock<std::time::Instant> = OnceLock::new();
        START.get_or_init(std::time::Instant::now).elapsed().as_secs_f64()
    }
    #[cfg(target_arch = "wasm32")]
    {
        0.0
    }
}

/// A job's id, unique for the life of the process.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default, PartialOrd, Ord)]
pub struct JobId(pub u64);

/// How a job ended, or that it has not.
#[derive(Clone, Debug, PartialEq)]
pub enum JobEnd {
    /// Still running.
    Running,
    /// Finished. `next` is the follow-up the result needs from the user,
    /// in words ("Place it"), when it needs one.
    Done { next: Option<String> },
    /// Failed, with the reason in words ("the node ran out of GPU memory").
    /// `retry` says whether trying again can help.
    Failed { reason: String, retry: bool },
    /// Stopped by a cancel.
    Cancelled,
}

impl JobEnd {
    pub fn is_running(&self) -> bool {
        matches!(self, JobEnd::Running)
    }
}

/// What a view needs to show one job, copied out of the board.
#[derive(Clone, Debug)]
pub struct JobSnapshot {
    pub id: JobId,
    /// What the job is, in words: "Generating mesh from photo.png".
    pub title: String,
    /// Where it runs: "this Mac", a node's name, "a node · GPU-B".
    pub machine: String,
    /// What it is doing now: "fillets", "denoise", "uploading".
    pub stage: String,
    /// Step `.0` of `.1` within the job, when the job counts steps.
    pub step: Option<(u32, u32)>,
    /// How far, 0..=1, or `None` when the job cannot say.
    pub fraction: Option<f32>,
    /// Seconds on [`now`] when it started.
    pub started: f64,
    /// Seconds on [`now`] when it ended.
    pub finished: Option<f64>,
    pub end: JobEnd,
    /// Whether the job can be cancelled at all.
    pub cancellable: bool,
    /// A cancel was asked for and the worker has not stopped yet.
    pub cancelling: bool,
    /// Seconds from the cancel request to the worker stopping.
    pub cancel_latency: Option<f64>,
    /// The user has seen and dismissed the result.
    pub dismissed: bool,
    /// The app shows this job's progress in a place of its own (a slice in
    /// its print panel): the jobs list holds it, a status line leaves it out.
    pub shown_elsewhere: bool,
}

impl JobSnapshot {
    /// Seconds the job has run, or ran.
    pub fn elapsed(&self, now: f64) -> f64 {
        (self.finished.unwrap_or(now) - self.started).max(0.0)
    }

    /// The one line a status bar shows: "Building motor mount · fillets 3/7".
    pub fn line(&self) -> String {
        let mut line = self.title.clone();
        match &self.end {
            JobEnd::Running if self.cancelling => line.push_str(" · cancelling"),
            JobEnd::Running => {
                if !self.stage.is_empty() || self.step.is_some() {
                    line.push_str(" · ");
                    line.push_str(&self.stage_words());
                }
            }
            JobEnd::Done { next: Some(next) } => {
                line.push_str(" · ready: ");
                line.push_str(next);
            }
            JobEnd::Done { next: None } => line.push_str(" · done"),
            JobEnd::Failed { reason, .. } => {
                line.push_str(" failed: ");
                line.push_str(reason);
            }
            JobEnd::Cancelled => line.push_str(" · cancelled"),
        }
        line
    }

    /// The stage with its step count: "fillets 3/7", "stage 2/4".
    pub fn stage_words(&self) -> String {
        match (self.stage.is_empty(), self.step) {
            (true, Some((i, n))) => format!("step {i}/{n}"),
            (false, Some((i, n))) => format!("{} {i}/{n}", self.stage),
            (_, None) => self.stage.clone(),
        }
    }
}

/// How a job is started: everything the board needs before the first update.
pub struct JobSpec {
    pub title: String,
    pub machine: String,
    pub stage: String,
    /// Called on the UI thread when the user cancels, after the flag is
    /// raised. Must not block: signal a worker, queue a request.
    pub on_cancel: Option<Box<dyn Fn() + Send + Sync>>,
    pub cancellable: bool,
    pub shown_elsewhere: bool,
}

impl JobSpec {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            machine: "this machine".to_string(),
            stage: String::new(),
            on_cancel: None,
            cancellable: true,
            shown_elsewhere: false,
        }
    }

    pub fn machine(mut self, machine: impl Into<String>) -> Self {
        self.machine = machine.into();
        self
    }

    pub fn stage(mut self, stage: impl Into<String>) -> Self {
        self.stage = stage.into();
        self
    }

    pub fn on_cancel(mut self, hook: impl Fn() + Send + Sync + 'static) -> Self {
        self.on_cancel = Some(Box::new(hook));
        self
    }

    /// A job that cannot be stopped once begun shows no cancel.
    pub fn not_cancellable(mut self) -> Self {
        self.cancellable = false;
        self
    }

    /// The app shows this job's progress in a place of its own: the jobs
    /// list holds it with its cancel, a status line does not repeat it.
    pub fn shown_elsewhere(mut self) -> Self {
        self.shown_elsewhere = true;
        self
    }

    /// Put it on the board.
    pub fn start(self) -> JobHandle {
        board().start(self)
    }
}

struct Entry {
    snap: JobSnapshot,
    cancel: Arc<AtomicBool>,
    on_cancel: Option<Arc<dyn Fn() + Send + Sync>>,
    cancel_asked: Option<f64>,
}

#[derive(Default)]
struct BoardState {
    next_id: u64,
    seq: u64,
    entries: Vec<Entry>,
}

/// A success quicker than this was never seen running: the history leaves it out.
pub const FLEETING_SECS: f64 = 0.3;

/// How many ended jobs the board remembers for the history.
const HISTORY: usize = 12;

/// The process-wide list. See the module docs.
pub struct JobBoard {
    state: Mutex<BoardState>,
}

/// The process's board.
pub fn board() -> &'static JobBoard {
    static BOARD: OnceLock<JobBoard> = OnceLock::new();
    BOARD.get_or_init(|| JobBoard { state: Mutex::new(BoardState::default()) })
}

/// Shorthand for `board().snapshot()`.
pub fn snapshot() -> Vec<JobSnapshot> {
    board().snapshot()
}

impl JobBoard {
    fn lock(&self) -> std::sync::MutexGuard<'_, BoardState> {
        self.state.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn start(&self, spec: JobSpec) -> JobHandle {
        let cancel = Arc::new(AtomicBool::new(false));
        let id = {
            let mut state = self.lock();
            state.next_id += 1;
            let id = JobId(state.next_id);
            state.entries.push(Entry {
                snap: JobSnapshot {
                    id,
                    title: spec.title,
                    machine: spec.machine,
                    stage: spec.stage,
                    step: None,
                    fraction: None,
                    started: now(),
                    finished: None,
                    end: JobEnd::Running,
                    cancellable: spec.cancellable,
                    cancelling: false,
                    cancel_latency: None,
                    dismissed: false,
                    shown_elsewhere: spec.shown_elsewhere,
                },
                cancel: cancel.clone(),
                on_cancel: spec.on_cancel.map(Arc::from),
                cancel_asked: None,
            });
            state.seq += 1;
            id
        };
        wake();
        JobHandle { reporter: JobReporter { id, cancel }, ended: false }
    }

    /// Changes whenever anything a view shows has changed.
    pub fn seq(&self) -> u64 {
        self.lock().seq
    }

    /// Every job the board holds, oldest first: the running ones and the
    /// recent history.
    pub fn snapshot(&self) -> Vec<JobSnapshot> {
        self.lock().entries.iter().map(|entry| entry.snap.clone()).collect()
    }

    /// One job, when the board still holds it.
    pub fn get(&self, id: JobId) -> Option<JobSnapshot> {
        self.lock().entries.iter().find(|entry| entry.snap.id == id).map(|entry| entry.snap.clone())
    }

    /// The user's cancel: raise the flag, then call the hook outside the lock.
    pub fn cancel(&self, id: JobId) -> bool {
        let hook = {
            let mut state = self.lock();
            let Some(entry) = state.entries.iter_mut().find(|entry| entry.snap.id == id) else {
                return false;
            };
            if !entry.snap.end.is_running() || !entry.snap.cancellable || entry.snap.cancelling {
                return false;
            }
            entry.cancel.store(true, Ordering::Release);
            entry.snap.cancelling = true;
            entry.cancel_asked = Some(now());
            let hook = entry.on_cancel.clone();
            state.seq += 1;
            hook
        };
        if let Some(hook) = hook {
            hook();
        }
        wake();
        true
    }

    /// The user has seen an ended job: it leaves the strip but stays in the
    /// history. A running job cannot be dismissed.
    pub fn dismiss(&self, id: JobId) {
        self.edit(id, |snap| {
            if snap.end.is_running() || snap.dismissed {
                return false;
            }
            snap.dismissed = true;
            true
        });
    }

    /// Apply `f` to a job; `f` says whether anything visible changed.
    fn edit(&self, id: JobId, f: impl FnOnce(&mut JobSnapshot) -> bool) {
        let changed = {
            let mut state = self.lock();
            let Some(entry) = state.entries.iter_mut().find(|entry| entry.snap.id == id) else {
                return;
            };
            let changed = f(&mut entry.snap);
            if changed {
                state.seq += 1;
            }
            changed
        };
        if changed {
            wake();
        }
    }

    fn end(&self, id: JobId, end: JobEnd) {
        {
            let mut state = self.lock();
            let Some(entry) = state.entries.iter_mut().find(|entry| entry.snap.id == id) else {
                return;
            };
            if !entry.snap.end.is_running() {
                return;
            }
            let now = now();
            // A job asked to stop that then finishes anyway (its last step
            // was already past the check) still reports how it ended.
            if let Some(asked) = entry.cancel_asked {
                entry.snap.cancel_latency = Some(now - asked);
            }
            entry.snap.end = end;
            entry.snap.cancelling = false;
            entry.snap.finished = Some(now);
            entry.on_cancel = None;
            // A plain success needs nothing from the user: it goes straight
            // to the history, or, when it was over before anyone could see
            // it run, nowhere (a read, an instant rebuild).
            if matches!(entry.snap.end, JobEnd::Done { next: None } | JobEnd::Cancelled) {
                entry.snap.dismissed = true;
            }
            let fleeting = matches!(entry.snap.end, JobEnd::Done { next: None }) && now - entry.snap.started < FLEETING_SECS;
            if let Some(latency) = entry.snap.cancel_latency {
                log(&format!("[jobs] {} stopped {:.0} ms after cancel", entry.snap.title, latency * 1000.0));
            }
            if fleeting {
                state.entries.retain(|e| e.snap.id != id);
            }
            // The same failure again replaces the earlier one on the board:
            // repeated presses show one notice, not a pile.
            let repeat = state.entries.iter().find(|e| e.snap.id == id).and_then(|e| match &e.snap.end {
                JobEnd::Failed { reason, .. } => Some((e.snap.title.clone(), reason.clone())),
                _ => None,
            });
            if let Some((title, reason)) = repeat {
                state.entries.retain(|e| {
                    e.snap.id == id
                        || e.snap.title != title
                        || !matches!(&e.snap.end, JobEnd::Failed { reason: r, .. } if *r == reason)
                });
            }
            state.seq += 1;
            // Keep the running jobs and the newest ended ones.
            let ended = state.entries.iter().filter(|entry| !entry.snap.end.is_running()).count();
            if ended > HISTORY {
                let mut drop = ended - HISTORY;
                state.entries.retain(|entry| {
                    if drop > 0 && !entry.snap.end.is_running() {
                        drop -= 1;
                        false
                    } else {
                        true
                    }
                });
            }
        }
        wake();
    }
}

/// Reports on one job from anywhere: cheap to clone and to send to another
/// thread. It cannot end the job; only its [`JobHandle`] does.
#[derive(Clone)]
pub struct JobReporter {
    id: JobId,
    cancel: Arc<AtomicBool>,
}

impl JobReporter {
    pub fn id(&self) -> JobId {
        self.id
    }

    /// Whether the user asked this job to stop.
    pub fn is_cancelled(&self) -> bool {
        self.cancel.load(Ordering::Acquire)
    }

    /// The flag itself, for code that checks it without the reporter.
    pub fn cancel_flag(&self) -> Arc<AtomicBool> {
        self.cancel.clone()
    }

    /// Where it runs, once known (a hub job learns its node after routing).
    pub fn set_machine(&self, machine: impl Into<String>) {
        let machine = machine.into();
        board().edit(self.id, |snap| {
            if snap.machine == machine {
                return false;
            }
            snap.machine = machine;
            true
        });
    }

    pub fn set_title(&self, title: impl Into<String>) {
        let title = title.into();
        board().edit(self.id, |snap| {
            if snap.title == title {
                return false;
            }
            snap.title = title;
            true
        });
    }

    /// A new stage. `step` is this stage's place in its count (fillet 3 of
    /// 7, stage 2 of 4), when there is one. `fraction` is how far the whole
    /// job is, when known.
    pub fn stage(&self, stage: impl Into<String>, step: Option<(u32, u32)>, fraction: Option<f32>) {
        let stage = stage.into();
        let fraction = fraction.map(|f| f.clamp(0.0, 1.0));
        board().edit(self.id, |snap| {
            if snap.stage == stage && snap.step == step && snap.fraction == fraction {
                return false;
            }
            snap.stage = stage;
            snap.step = step;
            snap.fraction = fraction;
            true
        });
    }

    /// Step `i` of `n` within the current stage; also sets the fraction.
    pub fn step(&self, i: u32, n: u32) {
        let fraction = if n > 0 { Some((i as f32 / n as f32).min(1.0)) } else { None };
        board().edit(self.id, |snap| {
            if snap.step == Some((i, n)) {
                return false;
            }
            snap.step = Some((i, n));
            snap.fraction = fraction;
            true
        });
    }

    /// How far, 0..=1, or `None` for a moving bar. Changes too small to see
    /// (under half a percent) are stored without waking the UI.
    pub fn fraction(&self, fraction: Option<f32>) {
        let fraction = fraction.map(|f| f.clamp(0.0, 1.0));
        board().edit(self.id, |snap| {
            let visible = match (snap.fraction, fraction) {
                (Some(a), Some(b)) => (a - b).abs() >= 0.005 || (b >= 1.0 && a < 1.0),
                (None, None) => false,
                _ => true,
            };
            snap.fraction = fraction;
            visible
        });
    }
}

thread_local! {
    static CURRENT: RefCell<Option<JobReporter>> = const { RefCell::new(None) };
}

/// The job this thread is working for, when its caller said so with
/// [`with_current`]. Code deep inside a computation (a kernel's step loop)
/// reports through it without a reporter threaded through every signature;
/// a worker thread that serves a caller enters the caller's job for the
/// duration of the request.
pub fn current() -> Option<JobReporter> {
    CURRENT.with(|current| current.borrow().clone())
}

/// Run `f` with `job` as this thread's [`current`] job, the previous one
/// restored afterwards (also when `f` unwinds).
pub fn with_current<R>(job: Option<JobReporter>, f: impl FnOnce() -> R) -> R {
    let _scope = enter(job);
    f()
}

/// Make `job` this thread's [`current`] job until the returned scope drops,
/// for a worker loop whose body has many exits.
pub fn enter(job: Option<JobReporter>) -> CurrentScope {
    let previous = CURRENT.with(|current| std::mem::replace(&mut *current.borrow_mut(), job));
    CurrentScope { previous }
}

/// See [`enter`]: dropping it puts the previous current job back.
pub struct CurrentScope {
    previous: Option<JobReporter>,
}

impl Drop for CurrentScope {
    fn drop(&mut self) {
        let previous = self.previous.take();
        CURRENT.with(|current| *current.borrow_mut() = previous);
    }
}

/// The worker's side of a job: it reports (it derefs to a [`JobReporter`])
/// and it ends the job. Dropping it without saying how ends the job too.
pub struct JobHandle {
    reporter: JobReporter,
    ended: bool,
}

impl std::ops::Deref for JobHandle {
    type Target = JobReporter;
    fn deref(&self) -> &JobReporter {
        &self.reporter
    }
}

impl JobHandle {
    /// A reporter for this job, to hand to other code or threads.
    pub fn reporter(&self) -> JobReporter {
        self.reporter.clone()
    }

    /// Run `f` with this job as the thread's [`current`] one.
    pub fn enter<R>(&self, f: impl FnOnce() -> R) -> R {
        with_current(Some(self.reporter.clone()), f)
    }

    /// Finished; nothing more needed from the user.
    pub fn done(mut self) {
        self.ended = true;
        board().end(self.reporter.id, JobEnd::Done { next: None });
    }

    /// Finished, and the result waits for the user: `next` names the step
    /// ("Place it"). The strip offers it until the user acts or dismisses.
    pub fn done_with_next(mut self, next: impl Into<String>) {
        self.ended = true;
        board().end(self.reporter.id, JobEnd::Done { next: Some(next.into()) });
    }

    /// Failed, with the reason in words. `retry`: trying again can help.
    pub fn fail(mut self, reason: impl Into<String>, retry: bool) {
        self.ended = true;
        board().end(self.reporter.id, JobEnd::Failed { reason: reason.into(), retry });
    }

    /// Stopped because the user cancelled.
    pub fn cancelled(mut self) {
        self.ended = true;
        board().end(self.reporter.id, JobEnd::Cancelled);
    }

    /// End the job the way `result` says: `Ok` is done, an `Err` is a
    /// cancel when one was asked for, else a failure with that reason.
    pub fn finish<T, E: std::fmt::Display>(self, result: &Result<T, E>, retry: bool) {
        match result {
            Ok(_) => self.done(),
            Err(_) if self.is_cancelled() => self.cancelled(),
            Err(e) => self.fail(e.to_string(), retry),
        }
    }
}

impl Drop for JobHandle {
    fn drop(&mut self) {
        if self.ended {
            return;
        }
        let end = if std::thread::panicking() {
            JobEnd::Failed { reason: "it crashed".to_string(), retry: true }
        } else if self.is_cancelled() {
            JobEnd::Cancelled
        } else {
            JobEnd::Failed { reason: "stopped without finishing".to_string(), retry: true }
        };
        board().end(self.reporter.id, end);
    }
}
