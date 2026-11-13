//! The permanent window of `makepad run`. Session 0 presents the boot document on
//! any native HAL backend; compilation and app domains are guests.
//! Only ending the run closes the physical window, or a program that turns out to draw nothing.
use crate::boot_progress::{History, Progress};
use makepad_hal::{domain_stop, open_flags, session::HOST, CmdWriter, Hal, Id, OpenReq};
use makepad_studio_protocol::KeyCode;
use relax_boot::{
    Action, Boot as HostWindow, Document, Environment, Event as BootEvent, Icon, LayerWant, RingAlert, RingWant,
};
use relax_compiler::{
    diag::Diagnostic,
    domain::{AppSpec, DomainState},
    postframe::Activity,
    run_state::{self, Mode, ParkedPanic, Phase, State},
};
use relax_session::sync::{Condvar, Mutex, MutexGuard};
use std::{
    collections::{HashSet, VecDeque},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicI32, Ordering},
    },
    time::{Duration, Instant},
};

struct Run {
    window: Id,
    spec: Option<AppSpec>,
    domain: u32,
    initial_failed: bool,
    retry: bool,
    /// the window opened before the package's folder was known (an app's start: its source came
    /// after): the package's name, icon and document follow when it is
    unnamed: bool,
}

#[derive(Clone)]
enum Update {
    /// the run-actions worker has answered the ring's click: `restart_asked` says from here on
    Relaunched,
    Compiler(run_state::Event),
    Domain(DomainState),
    /// a run ended in place (`next_run`); `start_screen`: the window went to it for the restart
    NextRun { start_screen: bool },
    /// a build that failed before the app started compiles again (retry)
    Begin,
    Failure(String),
    Message(String),
    /// The program turned out to draw nothing: its load screen closes, the run goes on.
    Batch,
    /// What an app's start does before its compile (getting its source).
    Status(String),
    /// The app's name and icon, once its package is known (`Run::unnamed`).
    Identity(String, Option<Icon>),
    SourcesChanged,
    Document(Document),
    Environment(Environment),
}

enum Work {
    Save(PathBuf, String),
    OpenFile {
        file: String,
        line: u32,
        col: u32,
    },
    Resume {
        panic: String,
        choice: String,
        value: String,
    },
    /// The person asked for the restart that waits (the JIT ring's Update ready notice): how,
    /// for the log's line of who asked.
    Relaunch(String),
}

static RUN: Mutex<Option<Run>> = Mutex::new(None);
static RETRY: Condvar = Condvar::new();
static UPDATES: Mutex<VecDeque<Update>> = Mutex::new(VecDeque::new());
static PUMP: Mutex<Option<std::thread::JoinHandle<()>>> = Mutex::new(None);
static QUIT: AtomicBool = AtomicBool::new(false);
static PANICS_CHANGED: AtomicBool = AtomicBool::new(false);
static EXIT: AtomicI32 = AtomicI32::new(0);
static ENVIRONMENT: Mutex<Option<Environment>> = Mutex::new(None);
/// The UI thread began a JIT wait since the window last looked (postframe's hook): the window
/// wakes, and shows the UI block on the JIT ring once the waiting is sustained (BLOCK_AFTER).
static UI_WAIT_ARMED: AtomicBool = AtomicBool::new(false);
/// A true UI block: one episode of UI-thread JIT waits this long (postframe::BLOCK_SHOW_NS: one
/// long lazy compile, or a load's thousands of short ones; never a short hiccup). The JIT ring
/// shows only past one.
const BLOCK_AFTER: Duration = Duration::from_nanos(relax_compiler::postframe::BLOCK_SHOW_NS);
/// The load screen's look comes back over the app only for a sustained wait, beyond the ring's
/// earlier notice (a short wait never flashes it).
const WAIT_AFTER: Duration = Duration::from_millis(500);
/// The UI has not been blocked on the JIT for this long: the warm-up is over.
const READY_QUIET_NS: u64 = 300_000_000;

fn ui_wait_began() {
    if !UI_WAIT_ARMED.swap(true, Ordering::AcqRel) {
        Hal::global().wake_session(HOST);
    }
}

/// The JIT ring shows only past a real wait: a UI block (BLOCK_AFTER), or makepad rebuilding
/// itself; first compiles and background tier-up alone never show it (JITLOOP-B). Once shown it
/// stays while the JIT works, and settles (fades out) once nothing has been built or run for
/// this long (queued builds nobody works on are not work).
const RING_SETTLE_AFTER: Duration = Duration::from_millis(500);
/// the build rate's smoothing (s)
const RATE_TAU: f32 = 0.5;
/// the run looks at the JIT's queues at most this often (the ring eases between looks)
const WATCH_EVERY: Duration = Duration::from_millis(100);
/// how often the run looks at the JIT while the app runs and no ring shows (new work is seen
/// within this; a UI wait wakes it at once)
const RING_LOOK: Duration = Duration::from_millis(500);

/// Whether nobody sees the run window: MAKEPAD_HIDE_WINDOWS (set, any value, as the HAL's
/// `OpenReq::from_env` reads it) keeps it off every screen. Such a run takes no retry from
/// its window: a failure ends it.
fn unseen() -> bool {
    std::env::var_os("MAKEPAD_HIDE_WINDOWS").is_some()
}

/// Whether this run shows the JIT ring: `RELAX_SETTLE_HUD=0` never, `=1` always, else not
/// in hidden or benchmark runs (MAKEPAD_HIDE_WINDOWS, RELAX_O3_PROGRESS,
/// RELAX_BOOT_DUMP).
fn ring_on() -> bool {
    match std::env::var("RELAX_SETTLE_HUD").as_deref() {
        Ok("0") => false,
        Ok("1") => true,
        _ => {
            std::env::var("MAKEPAD_HIDE_WINDOWS").as_deref() != Ok("1")
                && std::env::var_os("RELAX_O3_PROGRESS").is_none()
                && std::env::var_os("RELAX_BOOT_DUMP").is_none()
        }
    }
}

/// What the run sees of the JIT for its ring: builds running now and finished since the last
/// look (the work rate), the queues, the UI thread's JIT duty cycle, a true UI block. Busy means
/// something was built or is building, never only that a queue is non-empty. Atomics and two
/// queue locks a look; the app's threads never wait for it.
struct Watch {
    shown: bool,
    busy_since: Option<Instant>,
    /// the last time anything was running or finished
    worked: Option<Instant>,
    /// the last look at the JIT (at most every WATCH_EVERY), what it showed then
    looked: Instant,
    a: Activity,
    /// the queues' builds finished when this busy spell began
    done_from: Option<u64>,
    /// a program thread's first compiles finished since the last look
    demand: bool,
    /// builds finished a second (smoothed), and of the last look's, warm links only
    rate: f32,
    linking: bool,
    duty: f32,
    /// the UI block shows (for the trace)
    blocked: bool,
    trace: bool,
    traced: Option<Instant>,
}

impl Watch {
    fn new(trace: bool) -> Self {
        Self {
            shown: false,
            busy_since: None,
            worked: None,
            looked: Instant::now(),
            a: relax_compiler::postframe::activity(),
            done_from: None,
            demand: false,
            rate: 0.0,
            linking: false,
            duty: 0.0,
            blocked: false,
            trace,
            traced: None,
        }
    }

    /// The ring's wish now (`None`: the JIT is settled). `clock`: the frame dump's clock (ms),
    /// for the trace.
    fn look(&mut self, clock: Option<u128>, restart_asked: bool) -> Option<RingWant> {
        use relax_compiler::postframe::{activity, ui_blocked_ns};
        let now = Instant::now();
        let dt = now.duration_since(self.looked).as_secs_f32();
        if dt >= WATCH_EVERY.as_secs_f32() {
            let a = activity();
            let waited = a.ui_wait_ns.saturating_sub(self.a.ui_wait_ns) as f32 / 1e9;
            self.duty = (waited / dt).clamp(0.0, 1.0);
            let built = a.finished().saturating_sub(self.a.finished());
            self.linking = built > 0 && a.linked > self.a.linked && a.finished() - a.linked == self.a.finished() - self.a.linked;
            self.demand = a.demand() > self.a.demand();
            let k = 1.0 - (-dt / RATE_TAU).exp();
            self.rate += (built as f32 / dt - self.rate) * k;
            if built > 0 || a.running() || self.duty > 0.02 {
                self.worked = Some(now);
            }
            self.a = a;
            self.looked = now;
        }
        let a = self.a;
        let stalled = Duration::from_nanos(ui_blocked_ns());
        let block = stalled >= BLOCK_AFTER;
        // a program thread compiles lazily: the UI thread's JIT duty is up, or other threads'
        // first compiles finished
        let lazy = self.duty > 0.02 || self.demand;
        use relax_compiler::reload::host::{build_progress, status, Phase};
        let rebuild = match status().0 {
            Phase::Building => 1,
            Phase::Restarting => 2,
            Phase::Idle | Phase::Failed => if restart_asked { 2 } else { 0 },
        };
        if block || rebuild != 0 {
            self.worked = Some(now);
        }
        let busy = self.worked.is_some_and(|t| now.duration_since(t) < RING_SETTLE_AFTER);
        if busy {
            self.done_from.get_or_insert(a.queue_done());
            self.busy_since.get_or_insert(now);
            self.shown |= block || rebuild != 0;
        } else {
            self.shown = false;
            self.busy_since = None;
            self.done_from = None;
            self.rate = 0.0;
        }
        if self.trace && block != self.blocked {
            if block {
                eprintln!("relax: jit ring: UI block (the UI waited {:.0} ms on the JIT)", stalled.as_secs_f64() * 1000.0);
            } else {
                eprintln!("relax: jit ring: UI block over");
            }
        }
        self.blocked = block;
        // the build of the new JIT is what the arc tells while it runs (cargo says how far; it
        // never reads full before the build's end). Cargo waiting, or not yet saying: no arc.
        let cargo = (rebuild == 1).then(build_progress).map(|p| {
            if p.total > 0 && p.waiting.is_empty() { (p.done as u64, (p.total - p.done.min(p.total)).max(1) as u64) } else { (0, 0) }
        });
        let want = RingWant {
            done: cargo.map_or_else(|| a.queue_done().saturating_sub(self.done_from.unwrap_or(a.queue_done())), |c| c.0),
            pending: cargo.map_or_else(|| a.pending(), |c| c.1),
            ahead: a.ahead,
            o2: a.o2,
            o3: a.o3 + a.o3_running,
            lazy,
            block,
            settling: a.settling,
            linking: self.linking,
            rate: self.rate,
            busy: self.busy_since.map_or(Duration::ZERO, |t| now.duration_since(t)),
            rebuild,
            restart_asked,
        };
        // the trace: what the ring was told, at most 4 times a second while it shows
        if self.trace && (self.shown || self.traced.is_some()) && self.traced.is_none_or(|t| now.duration_since(t) >= Duration::from_millis(250)) {
            let at = clock.map_or(String::new(), |ms| format!(" @{ms}"));
            let state = if !self.shown {
                "settled"
            } else if want.ahead > 0 || lazy || block {
                "compiling"
            } else {
                "tier-up"
            };
            eprintln!(
                "relax: jit ring{at}: {state} ({}, ui duty {:.2}){}",
                relax_boot::ring_summary(Some(want), None),
                self.duty,
                if block { " UI BLOCK" } else { "" }
            );
            self.traced = self.shown.then_some(now);
        }
        self.shown.then_some(want)
    }

    /// How soon to look again while the UI thread may be heading into a block (its JIT wait began).
    fn block_wake(&self) -> Option<Duration> {
        use relax_compiler::postframe::{ui_blocked_ns, ui_idle_ns};
        if !UI_WAIT_ARMED.load(Ordering::Acquire) {
            return None;
        }
        // once the UI thread is not waiting the window disarms (the next wait's hook wakes it)
        if ui_idle_ns() >= BLOCK_AFTER.as_nanos() as u64 {
            UI_WAIT_ARMED.store(false, Ordering::Release);
            return None;
        }
        let stalled = Duration::from_nanos(ui_blocked_ns());
        Some(BLOCK_AFTER.saturating_sub(stalled).max(Duration::from_millis(20)))
    }
}

/// The fades' curve.
fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The ring dot's tooltip: what `alert` means, the first thing behind it, and what a click does.
fn ring_tip(alert: Option<RingAlert>, data: &State, diagnostics: &[Diagnostic], update_message: Option<&str>) -> Option<String> {
    let first_error = || diagnostics.iter().find(|d| d.severity == relax_compiler::diag::Severity::Error).map(|d| tip_line(&d.message, 70));
    let text = match alert? {
        RingAlert::UpdateReady => {
            let pending = relax_compiler::reload::restart_pending();
            let what = pending.first().map(|p| tip_line(restart_item(p), 70)).or_else(|| update_message.map(|m| tip_line(m, 70)))?;
            let n = pending.len().max(1);
            let more = if n > 1 { format!(" (+{} more)", n - 1) } else { String::new() };
            let keeps = if relax_compiler::reload::restart_keeps_state() { ", the app keeps its state" } else { "" };
            let click = click_says(relax_compiler::reload::host::route(), &relax_compiler::reload::host::build_progress(), keeps);
            // what a click does comes first: a narrow window cuts the end of this line
            let mut click = click.chars();
            let first = click.next().map(|c| c.to_ascii_uppercase()).unwrap_or(' ');
            format!("{first}{} · {n} change{}: {what}{more}", click.as_str(), if n > 1 { "s" } else { "" })
        }
        RingAlert::PatchFailed => match first_error() {
            Some(error) => format!("Live patch not applied, the previous code runs: {error} · click for details"),
            None => "Live patch not applied, the previous code runs · click for details".into(),
        },
        RingAlert::RebuildFailed => {
            let failure = relax_compiler::reload::host::failure();
            let error = failure.lines().map(str::trim).find(|l| l.starts_with("error")).map(|l| tip_line(l, 70));
            match error {
                Some(error) => format!("Building the new JIT failed, the previous code runs: {error} · click for details"),
                None => format!("Building the new JIT failed, the previous code runs: {} · click for details", tip_line(&failure, 70)),
            }
        }
        RingAlert::Panic => match &data.parked {
            Some(p) => format!("Paused on a panic: {} · click to see it", tip_line(&p.message, 70)),
            None => "Paused on a panic · click to see it".into(),
        },
        RingAlert::Stale => "Paused on code a live edit replaced · click to see it".into(),
    };
    Some(text)
}

/// What a click on the waiting-change dot does, for its tooltip: a click that starts a build of
/// the JIT never reads as a plain restart.
fn click_says(route: relax_compiler::reload::host::Route, progress: &relax_compiler::reload::host::BuildProgress, keeps: &str) -> String {
    use relax_compiler::reload::host::Route;
    match route {
        Route::Plain => format!("click to restart{keeps}"),
        Route::BuildFirst => format!("the JIT changed too: click to build it, then restart{keeps}"),
        Route::Building if progress.total > 0 => format!("the new JIT is building ({}/{}): click to restart when it's done{keeps}", progress.done, progress.total),
        Route::Building => format!("the new JIT is building: click to restart when it's done{keeps}"),
        Route::Built => format!("the new JIT is built: click to restart into it{keeps}"),
        Route::CannotBuild => format!("the JIT changed too and can't rebuild while it runs here: click to restart on this JIT{keeps}; `cargo run` updates it"),
    }
}

/// The ring's tooltip from the click until the restart: what is happening, how far it is, and that
/// nothing is left to click. `keeps`: how the app comes back.
fn restart_says(phase: relax_compiler::reload::host::Phase, secs: u64, progress: &relax_compiler::reload::host::BuildProgress, pinned: bool, keeps: &str) -> String {
    use relax_compiler::reload::host::Phase;
    match phase {
        Phase::Building if !progress.waiting.is_empty() => format!("Waiting for another JIT build · {secs} s · then it builds and restarts{keeps}"),
        Phase::Building if progress.total > 0 => format!("Building the new JIT: {}/{} · {secs} s · then it restarts{keeps}", progress.done, progress.total),
        Phase::Building => format!("Building the new JIT · {secs} s · then it restarts{keeps}"),
        Phase::Restarting if pinned => "New JIT built · the restart waits until the app is unpinned".into(),
        Phase::Restarting => format!("New JIT built · the app restarts{keeps} at its next safe point (unsaved work holds it)"),
        Phase::Idle | Phase::Failed if pinned => "The restart waits until the app is unpinned".into(),
        Phase::Idle | Phase::Failed => format!("The app restarts{keeps} at its next safe point (unsaved work holds it)"),
    }
}

/// A restart-pending entry without the batch's file list in front (`a.rs, b.rs: <what>`).
fn restart_item(entry: &str) -> &str {
    match entry.split_once(": ") {
        Some((files, what)) if files.split(", ").all(|f| !f.is_empty() && !f.contains(' ')) => what,
        _ => entry,
    }
}

/// One short line of a message for the ring's tooltip: its first line, paths cut to their file
/// names, without the advice after ` -- ` or `; `, at most `max` chars.
fn tip_line(text: &str, max: usize) -> String {
    let line = text.lines().next().unwrap_or("");
    let line = line.split(" -- ").next().unwrap_or(line);
    let line = line.split("; ").next().unwrap_or(line);
    let words: Vec<String> = line
        .split(' ')
        .map(|w| match (w.find('/'), w.rfind('/')) {
            (Some(first), Some(last)) => {
                let lead = &w[..first];
                let lead = &lead[..lead.len() - lead.trim_start_matches(|c: char| !c.is_alphanumeric()).len()];
                format!("{lead}{}", &w[last + 1..])
            }
            _ => w.to_string(),
        })
        .collect();
    let line = words.join(" ");
    if line.chars().count() <= max {
        return line;
    }
    let mut cut: String = line.chars().take(max.saturating_sub(1)).collect();
    cut.push('…');
    cut
}

/// A rebuild of makepad failed (its dot shows in the ring until the next rebuild).
fn rebuild_failed() -> bool {
    use relax_compiler::reload::host::{status, Phase};
    status().0 == Phase::Failed
}

/// Where the window layer over the app is in its sequence.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Stage {
    /// the app is running (the JIT ring or a sustained-wait card may show)
    Running,
    /// the boot field, the app's first frames going under it
    Boot,
    /// after the first present, until the app is ready
    WarmUp,
    /// ready: the sharp app comes in, the galaxy goes
    Lift,
}

pub fn set_environment(environment: Environment) {
    *ENVIRONMENT.lock().unwrap_or_else(|e| e.into_inner()) = Some(environment);
    if state().is_some() {
        post(Update::Environment(environment));
    }
}

fn state() -> MutexGuard<'static, Option<Run>> {
    RUN.lock().unwrap_or_else(|e| e.into_inner())
}

impl Update {
    /// A copy built in the kept heap (the caller is in the kept phase). An update crosses from
    /// the compiler's threads to the window pump, which holds its parts for the whole run: none
    /// of it may live in the compiler arena, whose generation the settle swap destroys.
    fn kept(self) -> Update {
        match self {
            Update::Compiler(run_state::Event::Patch { domain, applied, records }) => Update::Compiler(run_state::Event::Patch {
                domain,
                applied,
                records: records.iter().cloned().collect::<Vec<_>>().into(),
            }),
            other => other.clone(),
        }
    }
}

fn post(update: Update) {
    let _kept = relax_compiler::zone_alloc::kept();
    let update = update.kept();
    UPDATES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push_back(update);
    Hal::global().wake_session(HOST);
}

/// The run ends, said once with why: every way it ends names itself here (a quit signal with its
/// sender, the app's exit, a failure no window shows). A run that ended silently left only its
/// app's "the last window closed", read as the person's close (Scope on the Mac, 11-06).
fn end_run(why: &str) {
    if !QUIT.swap(true, Ordering::AcqRel) {
        eprintln!("relax: the run ends: {why}");
    }
}

/// RELAX_QUIT_TRACE=1: each step of the run's end on stderr (thread, ms since the quit
/// began): where a quit that never exits stops.
fn quit_trace(step: &str) {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    static T0: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    if !*ON.get_or_init(|| std::env::var_os("RELAX_QUIT_TRACE").is_some()) {
        return;
    }
    let ms = T0.get_or_init(Instant::now).elapsed().as_secs_f64() * 1e3;
    eprintln!("relax quit: {step} (+{ms:.0} ms, thread `{}`)", std::thread::current().name().unwrap_or("?"));
}

fn panic_changed() {
    PANICS_CHANGED.store(true, Ordering::Release);
    Hal::global().wake_session(HOST);
}

/// What compiles and starts the program: a run's, and the next run's in this process.
static COMPILE: Mutex<Option<Box<dyn FnMut() -> i32 + Send>>> = Mutex::new(None);
static COMPILE_THREAD: Mutex<Option<std::thread::JoinHandle<()>>> = Mutex::new(None);

/// A run's compile thread: it boots and, for an app without the HAL, runs its main.
fn start_compile_thread() {
    // the handle (thread record, result packet) is joined at the end of the run, after the settle swap
    // may have destroyed the compiler generation this frame runs in: it lives in the kept heap
    let thread = relax_compiler::zone_alloc::kept_with(|| std::thread::Builder::new()
        .name("run-compile".into())
        .stack_size(8 << 20)
        .spawn(|| {
            // this thread boots and, for an app without the HAL, runs its main (what the loader's
            // main thread did): a compiler phase, not a swap-quiesced compiler worker
            let _compiler = relax_compiler::zone_alloc::compiler();
            let mut compile = COMPILE.lock().unwrap_or_else(|e| e.into_inner()).take().expect("the run's compile");
            compile_runs(&mut compile);
            *COMPILE.lock().unwrap_or_else(|e| e.into_inner()) = Some(compile);
        })
        .expect("the compiler thread"));
    *COMPILE_THREAD.lock().unwrap_or_else(|e| e.into_inner()) = Some(thread);
}

/// A run's threads have ended (`relax_compiler::run_end`): the ended app's session goes, with what it
/// held of the GPU and its window back to this host. The window stays.
pub fn end_app() {
    let domain = {
        let mut guard = state();
        let Some(run) = guard.as_mut() else { return };
        std::mem::replace(&mut run.domain, 0)
    };
    if domain != 0 {
        Hal::global().report_domain(domain, makepad_hal::domain_state::EXITED, 0, 100.0, "restart");
    }
}

/// A run ended in place (`relax_compiler::run_end`): its compile thread has returned, or waits for
/// the window's retry of a failed build (it returns now: the next run is the retry), and the next
/// run starts in the same window. False: there is no run window to start it in.
pub fn next_run() -> bool {
    end_retry_wait();
    let thread = COMPILE_THREAD.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some(thread) = thread {
        let _ = thread.join();
    }
    NEXT_RUN.store(false, Ordering::Release);
    {
        let mut guard = state();
        let Some(run) = guard.as_mut() else { return false };
        run.spec = None;
        run.initial_failed = false;
        run.retry = false;
    }
    EXIT.store(0, Ordering::Release);
    PANICS_CHANGED.store(false, Ordering::Release);
    let start_screen = LEAVING.swap(false, Ordering::SeqCst);
    QUIT.store(false, Ordering::Release);
    UPDATES.lock().unwrap_or_else(|e| e.into_inner()).clear();
    post(Update::NextRun { start_screen });
    post(Update::Begin);
    start_compile_thread();
    true
}

/// A run ended in place, and its compile thread is to return rather than wait for a retry
/// (`next_run`): set from that end until the thread has gone.
static NEXT_RUN: AtomicBool = AtomicBool::new(false);

/// A run ended in place: a failed build's compile thread that waits for the window's retry
/// returns ([`wait_for_retry`]); the next run is the retry.
fn end_retry_wait() {
    {
        let _run = state();
        NEXT_RUN.store(true, Ordering::Release);
    }
    RETRY.notify_all();
}

fn compile_runs(compile: &mut Box<dyn FnMut() -> i32 + Send>) {
    loop {
        relax_compiler::lifeline::clear_panic_at();
        // a panic in the compiler (exit 101): its message and position go on the load
        // screen, not only into a log nobody on the box sees
        let (code, panicked) = match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| compile())) {
            Ok(code) => (code, None),
            Err(p) => (101, Some(relax_compiler::lifeline::panic_line(&*p))),
        };
        // (a restart ended the run under it: `next_run` starts the next one)
        if relax_compiler::run_end::restarting() {
            return;
        }
        // (a compiler panic's line: the hook keeps a build's panics quiet, they become its compile
        // error, so a run that ends on one says it here or nowhere)
        let cause = panicked.as_ref().map_or(String::new(), |line| format!(" (the compiler panicked: {line})"));
        let mut guard = state();
        let Some(run) = guard.as_mut() else {
            EXIT.store(code, Ordering::Release);
            end_run(&format!("the program returned {code}{cause} with no run window"));
            makepad_hal::stop_main_loop();
            return;
        };
        if code == 0 && run.spec.is_some() {
            return;
        }
        if QUIT.load(Ordering::Acquire) {
            return;
        }
        EXIT.store(code, Ordering::Release);
        // an app's start that could not get its source shows why and waits for the Restart
        let source = relax_compiler::run::source_failure();
        if run.window.is_none() || !(relax_compiler::run::live_session() || source.is_some()) || unseen() {
            // No native window backend: there is no run UI to retry from. A run that is
            // no live session takes no edit that could fix the build: it ends with the
            // code (a test's child under CARGO_BIN_EXE_* is waited for by its parent).
            // A hidden window (MAKEPAD_HIDE_WINDOWS) shows the failure to nobody and nobody
            // retries from it: the run ends with the code instead of waiting for a retry
            // forever (a hidden Sandbox boot whose provided crates differed sat idle 4.6 h).
            end_run(&format!("the program's run ended with {code}{cause} and no window can retry it"));
            Hal::global().wake_session(HOST);
            if PUMP.lock().unwrap_or_else(|e| e.into_inner()).is_none() {
                makepad_hal::stop_main_loop();
            }
            return;
        }
        {
            // With a unit already installed, a retry starts the process again (retry).
            // A typed source mismatch has already posted UpdateReady. It is neither
            // a compiler failure nor a retry of compilation against the stale loader.
            let update_ready = code == 0 && panicked.is_none() && relax_compiler::reload::host::update_message().is_some();
            run.initial_failed = !update_ready && run.spec.is_none();
            if !update_ready {
                post(Update::Failure(match (&panicked, source) {
                    (Some(line), _) => format!("Compilation failed: the compiler panicked: {line}"),
                    (None, Some(source)) => format!("The app's source could not be had: {source}"),
                    (None, None) => format!("Compilation failed (exit {code})"),
                }));
            }
            if run.spec.is_some() {
                return;
            }
            if !wait_for_retry(guard) {
                return;
            }
            post(Update::Begin);
        }
    }
}

/// A build that failed before the app started waits for the window's retry. True: it compiles
/// again. False: the run goes instead (the window closed, or a restart ended the run and its
/// next run is the retry: `next_run`).
fn wait_for_retry(mut guard: MutexGuard<'static, Option<Run>>) -> bool {
    while !QUIT.load(Ordering::Acquire) && !NEXT_RUN.load(Ordering::Acquire) && !guard.as_ref().is_some_and(|r| r.retry) {
        guard = RETRY.wait(guard).unwrap_or_else(|e| e.into_inner());
    }
    if QUIT.load(Ordering::Acquire) || NEXT_RUN.load(Ordering::Acquire) {
        return false;
    }
    if let Some(run) = guard.as_mut() {
        run.retry = false;
        run.initial_failed = false;
    }
    true
}

/// `quit` before the app answers it. The endpoint (and the startup line naming it) is the host's
/// from the run's start, and the app registers its own commands only once its main runs: a quit
/// sent right after that line, a cold start still compiling, was answered "unknown command" and
/// the app went on (a harness's hub ran on to its kill). Until the app has its `quit`, the host
/// ends the run as a SIGTERM does (quit_signal.rs): the quit write, then the app domain stops
/// with QUIT and the window closes. False once the app registered one: the app's own answers.
fn host_quit(_req: &makepad_remote::Req, out: &mut makepad_remote::Out) -> bool {
    if relax_compiler::inspect::program_provides("quit") {
        return false;
    }
    relax_compiler::run::quit_requested();
    end_run("a remote quit before the app took commands");
    RETRY.notify_all();
    Hal::global().wake_session(HOST);
    out.line("{\"ok\":1,\"by\":\"host\"}");
    true
}

pub fn run(mut compile: Box<dyn FnMut() -> i32 + Send>) -> i32 {
    // Java owns Android's UI loop; this boot thread compiles and runs domain 0.
    if cfg!(target_os = "android") {
        // the APK's boot app is the window manager when the host was built `--privileged`
        let privileged = relax_runner::privileged();
        return makepad_hal::with_caps(makepad_hal::Caps { privileged }, || compile());
    }
    makepad_hal::expect_main_loop();
    *COMPILE.lock().unwrap_or_else(|e| e.into_inner()) = Some(compile);
    relax_compiler::inspect::register_host_inspect("quit", host_quit);
    start_compile_thread();
    // the app is a domain of this host: its platform installs no termination handler, so the
    // host answers SIGINT/SIGTERM/SIGHUP (quit_signal.rs): the cache's quit write, then the run
    // ends as closing its window ends it. Its worker thread (handle, name, result packet, closure)
    // waits for the whole run: it lives in the kept heap, not in the compiler generation the
    // settle swap destroys (it then faulted on its own packet as the signal woke it)
    relax_compiler::zone_alloc::kept_with(|| crate::quit_signal::install(|signal| {
        let _at_the_run = relax_compiler::run_threads::process_work();
        relax_compiler::run::quit_requested();
        end_run(&signal);
        RETRY.notify_all();
        Hal::global().wake_session(HOST);
    }));
    {
        let _kept = relax_compiler::zone_alloc::kept();
        makepad_hal::run_main_loop();
    }
    quit_trace("the main loop returned");
    end_run("the window system's main loop returned");
    RETRY.notify_all();
    Hal::global().wake_session(HOST);
    let mut code = None;
    let thread = COMPILE_THREAD.lock().unwrap_or_else(|e| e.into_inner()).take();
    if thread.is_some_and(|thread| thread.join().is_err()) {
        quit_trace("the compile thread panicked");
        code = Some(101);
    }
    let pump = PUMP.lock().unwrap_or_else(|e| e.into_inner()).take();
    if let Some(pump) = pump {
        quit_trace("the compile thread joined");
        if pump.join().is_err() {
            code = code.or(Some(101));
        }
    }
    // Every way the run ends comes here (window close, a signal's quit, a failed boot, no window
    // backend) and the process exits right after: the HAL's driver threads stop and are joined
    // first. exit() runs the driver's exit handlers (NVIDIA's EGL teardown), and a Vulkan worker
    // still in vkQueueSubmit faulted inside it (a SIGSEGV at a SIGTERM mid-frame).
    quit_trace("before_exit");
    Hal::global().before_exit();
    quit_trace("the run ends");
    code.unwrap_or_else(|| EXIT.load(Ordering::Acquire))
}

fn document(package_dir: &str) -> Result<Document, String> {
    let manifest = format!("{package_dir}/Cargo.toml");
    let text =
        relax_compiler::vfs::read_to_string(&manifest).map_err(|e| format!("{manifest}: {e}"))?;
    let manifest = relax_compiler::toml::parse_toml(&text).map_err(|e| e.to_string())?;
    let Some(value) = manifest.get_path(&["package", "metadata", "makepad", "bootloader"]) else {
        // the binary's own document: embedded, never the source tree's copy
        return Ok(Document::bundled());
    };
    let relative = value
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or("package.metadata.makepad.bootloader must name a Splash document")?;
    let path = PathBuf::from(package_dir).join(relative);
    let source = relax_compiler::vfs::read_to_string(&path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(Document {
        path,
        source: source.into(),
        live: true,
    })
}

fn identity(package: &str, package_dir: &str) -> (String, Option<Icon>) {
    let manifest = relax_compiler::vfs::read_to_string(format!("{package_dir}/Cargo.toml"))
        .ok()
        .and_then(|text| relax_compiler::toml::parse_toml(&text).ok());
    let metadata = |path: &[&str]| {
        manifest
            .as_ref()?
            .get_path(path)?
            .as_str()
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    let name = metadata(&["package", "metadata", "makepad", "android", "label"])
        .or_else(|| metadata(&["package", "metadata", "makepad", "name"]))
        .or_else(|| metadata(&["package", "metadata", "makepad", "desktop", "name"]))
        .unwrap_or_else(|| package.to_owned());
    if std::env::var_os("MAKEPAD_NO_ICON").is_some() {
        return (name, None);
    }
    let root = PathBuf::from(package_dir);
    let mut paths = Vec::new();
    for path in [
        &["package", "metadata", "makepad", "icon"][..],
        &["package", "metadata", "makepad", "desktop", "icon"][..],
    ] {
        if let Some(path) = metadata(path) {
            paths.push(root.join(path));
        }
    }
    // The Android packager's launcher resources, then its existing desktop PNG
    // overrides/resources. A missing/unsupported icon never blocks boot.
    for density in ["xxxhdpi", "xxhdpi", "xhdpi", "hdpi", "mdpi"] {
        paths.push(root.join(format!(
            "resources/android/res/mipmap-{density}/ic_launcher.png"
        )));
    }
    for size in [256, 512, 128, 64, 1024, 32] {
        if let Some(path) = std::env::var_os(format!("MAKEPAD_APP_ICON_{size}")) {
            paths.push(PathBuf::from(path));
        }
        paths.push(root.join(format!("resources/icon_{size}.png")));
    }
    let icon = paths.into_iter().find_map(|path| {
        let bytes = relax_compiler::vfs::read(&path).ok()?;
        Icon::decode(&bytes)
    });
    (name, icon)
}

/// Called after graph resolution, before build scripts or application sources; for an app's
/// start (`relax_compiler::run::cmd_run_app`) before its source too, with no `package_dir`: the
/// window then has the launch's title, and the package's own name, icon and document come with
/// the call after its graph.
pub fn open(package: &str, package_dir: &str) -> Result<(), String> {
    // JNI captures the Activity before compilation; the app opens the device when ready.
    if cfg!(target_os = "android") {
        return Ok(());
    }
    if let Some(run) = state().as_mut() {
        if run.unnamed && !package_dir.is_empty() {
            run.unnamed = false;
            let _kept = relax_compiler::zone_alloc::kept();
            let (app, icon) = identity(package, package_dir);
            post(Update::Identity(app, icon));
            match document(package_dir) {
                Ok(document) if document.live => post(Update::Document(document)),
                Ok(_) => {}
                Err(e) => post(Update::Message(e)),
            }
        }
        return Ok(());
    }
    let _kept = relax_compiler::zone_alloc::kept();
    let hal = Hal::global();
    let remote = relax_compiler::os::proc::args_bytes()
        .iter()
        .any(|a| a == b"--remote")
        || std::env::var_os("MAKEPAD_REMOTE").is_some();
    let flags = open_flags::RETAIN_WINDOWS
        | open_flags::HIDDEN_UNTIL_PRESENT
        | if remote { open_flags::NO_FOCUS } else { 0 };
    // Inside a makepad window manager the run is one of its windows: it is named by the app.
    #[cfg(target_os = "linux")]
    makepad_hal::wm_socket::set_client_name(package);
    // RELAX_DPI: the scale of a hidden window (tests of how the boot window looks on a HiDPI screen)
    let dpi_hint = std::env::var("RELAX_DPI").ok().and_then(|v| v.parse().ok()).unwrap_or(0.0);
    let info = hal.open_host(&OpenReq::from_env(0, flags, dpi_hint));
    if info.backend == makepad_hal::backend_id::NONE {
        return Err("could not open the host HAL session".into());
    }
    let key = package
        .as_bytes()
        .iter()
        .fold(0xcbf29ce484222325u64, |h, b| {
            (h ^ *b as u64).wrapping_mul(0x100000001b3)
        });
    let state_path = relax_compiler::dirs::relax_dir()
        .join("windows")
        .join(format!("{key:016x}.geom"));
    let remembered = std::fs::read_to_string(&state_path)
        .ok()
        .and_then(|s| {
            let values: Vec<f32> = s
                .split_whitespace()
                .map(str::parse)
                .collect::<Result<_, _>>()
                .ok()?;
            let g: [f32; 4] = values.try_into().ok()?;
            (g.iter().all(|v| v.is_finite()) && g[2] > 0.0 && g[3] > 0.0).then_some(g)
        });
    let geometry = remembered.unwrap_or([f32::NAN, f32::NAN, 1280.0, 800.0]);
    // `MAKEPAD_WINDOW=WxH@scale` sizes the app's first window, which adopts this one: it is made
    // at that size from the start, exactly, and the size is not kept for the next run
    let launch = launch_window(info.dpi);
    let geometry = launch.unwrap_or(geometry);
    // the window is remembered for the user's next run: not a launch size, nor a hidden run's
    // window, which nobody sized (a command-line app's hidden 320x200 opened the user's next run
    // at that size)
    let remember = launch.is_none() && !unseen();
    let history = History::load(
        relax_compiler::dirs::cache_dir()
            .join("boot")
            .join(format!("{key:016x}.timings")),
    );
    let unnamed = package_dir.is_empty();
    let document = if unnamed {
        Document::bundled()
    } else {
        document(package_dir).unwrap_or_else(|e| {
            post(Update::Message(e));
            Document::bundled()
        })
    };
    *state() = Some(Run {
        window: Id::NONE,
        spec: None,
        domain: 0,
        initial_failed: false,
        retry: false,
        unnamed,
    });
    run_state::set_observer(|event| post(Update::Compiler(event)));
    relax_compiler::tui::track_for_host();
    relax_compiler::inspect::events::set_run_state_observer(panic_changed);
    relax_compiler::inspect::events::run_state_available(
        info.dialect != makepad_hal::dialect::NONE && !unseen(),
    );
    let (app, icon) = if unnamed {
        (std::env::var("MAKEPAD_APP_TITLE").ok().filter(|t| !t.is_empty()).unwrap_or_else(|| package.to_owned()), None)
    } else {
        identity(package, package_dir)
    };
    let initial = State {
        app,
        domain: "Compiling".into(),
        ..State::default()
    };
    let package_dir = PathBuf::from(package_dir);
    let watched_document = document.clone();
    let (ready, first) = std::sync::mpsc::sync_channel(1);
    *PUMP.lock().unwrap_or_else(|e| e.into_inner()) = Some(
        std::thread::Builder::new()
            .name("run-window".into())
            .stack_size(8 << 20)
            .spawn(move || {
                struct StopLoop;
                impl Drop for StopLoop {
                    fn drop(&mut self) {
                        quit_trace("the window pump ends: stop_main_loop");
                        end_run("the run window's thread ended");
                        makepad_hal::stop_main_loop();
                    }
                }
                let _stop = StopLoop;
                let _kept = relax_compiler::zone_alloc::kept();
                let mut host = match HostWindow::new(info, document, initial.clone(), geometry, launch.is_some(), remembered.is_some()) {
                    Ok(host) => host,
                    Err(error) => {
                        let _ = ready.try_send(Err(error));
                        return;
                    }
                };
                let environment = ENVIRONMENT.lock().unwrap_or_else(|e| e.into_inner()).unwrap_or_default();
                host.environment(environment);
                if let Some(icon) = icon { host.icon(icon); }
                if remember {
                    let _ = GEOMETRY.set(state_path.clone());
                }
                pump(
                    host,
                    initial,
                    history,
                    remember.then_some(state_path),
                    package_dir,
                    watched_document,
                    ready,
                );
            })
            .map_err(|e| e.to_string())?,
    );
    first.recv().map_err(|e| e.to_string())?
}

/// The boot window's geometry for `MAKEPAD_WINDOW=WxH@scale`: the native size that gives the app's
/// window a `W`x`H` layout at `scale` on a display running at `dpi` (the app's `--window` does the
/// same for a native launch: `makepad_platform::remote::app_clock::LaunchWindow::native_size`).
fn launch_window(dpi: f32) -> Option<[f32; 4]> {
    let value = std::env::var("MAKEPAD_WINDOW").ok()?;
    let (size, scale) = value.split_once('@')?;
    let (w, h) = size.split_once('x')?;
    let [w, h, scale] = [w, h, scale].map(|v| v.trim().parse::<f32>().ok().filter(|v| v.is_finite() && *v > 0.0));
    let dpi = if dpi.is_finite() && dpi > 0.0 { dpi } else { 1.0 };
    let scale = scale?;
    Some([f32::NAN, f32::NAN, w? * scale / dpi, h? * scale / dpi])
}

pub fn start(spec: AppSpec) -> Result<(), String> {
    let _kept = relax_compiler::zone_alloc::kept();
    // the spec is built by the boot in the compiler phase; the run state keeps it for the whole run
    let spec = spec.clone();
    if QUIT.load(Ordering::Acquire) {
        return Err("run closed during compilation".into());
    }
    let launch =
        Hal::global().with_world(|w| w.start_domain(HOST, 0, spec.package.clone(), String::new()));
    {
        let mut state = state();
        let r = state.as_mut().ok_or("no host window")?;
        r.domain = launch.domain;
        r.spec = Some(spec);
    }
    relax_compiler::run::set_app_domain(launch.domain);
    crate::hal_host::start(launch)
}

/// The program went quiet without starting an app (`MainHost::batch`).
pub fn batch() {
    post(Update::Batch);
}

/// What an app's start does before its compile (`MainHost::status`).
pub fn status(text: &str) {
    post(Update::Status(text.to_string()));
}

pub fn spec(id: u32) -> Option<AppSpec> {
    state()
        .as_ref()
        .filter(|r| r.domain == id)
        .and_then(|r| r.spec.clone())
}

/// Where the run window's geometry is remembered (the app's next run, and a restart, opens there);
/// unset for a launch size (`MAKEPAD_WINDOW`) and a hidden run.
static GEOMETRY: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// The restart itself has begun: the window goes to the start screen.
static LEAVING: AtomicBool = AtomicBool::new(false);
/// The window shows the start screen (or cannot): the restart goes on.
static LEFT: (std::sync::Mutex<bool>, std::sync::Condvar) = (std::sync::Mutex::new(false), std::sync::Condvar::new());
/// The app's content fades to the start screen in this time, and in from it.
const VEIL_IN: Duration = Duration::from_millis(250);
const VEIL_OUT: Duration = Duration::from_millis(500);
/// A restarted run whose window went to the start screen before the restart: "1".
const RESTART_SCREEN: &str = "RELAX_RESTART_SCREEN";

/// The restart itself begins (on the thread that restarts, the run's end still to do): the app's
/// content fades to the start screen in the same window; this returns when it covers the app. A
/// window that cannot show it (none yet, a panel over it, its loop gone) never holds up the restart.
pub fn restart_begins() {
    // (an edit's restart keeps the app's last frame on the window until the new one replaces it)
    if !relax_compiler::reload::relaunch_asked() || state().is_none() {
        return;
    }
    *LEFT.0.lock().unwrap_or_else(|e| e.into_inner()) = false;
    LEAVING.store(true, Ordering::SeqCst);
    Hal::global().wake_session(HOST);
    let left = LEFT.0.lock().unwrap_or_else(|e| e.into_inner());
    let _ = LEFT.1.wait_timeout_while(left, VEIL_IN * 3, |left| !*left);
}

/// Right before the exec restart (relax_compiler::reload::set_before_exec): the restarted run
/// opens its window exactly where this one is -- the geometry is written now, not once it held
/// still -- and keeps the window manager's window (Linux) or its place in the window stacking
/// and the focus (macOS, makepad_hal::restack_handover). Returns the environment for the new
/// image. (Windows: the restarted run is a new process, which only opens its window there.)
#[cfg(any(unix, windows))]
pub fn prepare_exec() -> Vec<(String, String)> {
    let window = state().as_ref().map(|r| r.window).filter(|w| !w.is_none());
    if let (Some(window), Some(path)) = (window, GEOMETRY.get()) {
        let geom = Hal::global().with_world(|w| w.session(HOST).and_then(|s| s.windows.get(HOST, window).ok()).map(|w| w.val.geom));
        if let Some(g) = geom.filter(|g| g.iter().all(|v| v.is_finite()) && g[2] > 0.0 && g[3] > 0.0) {
            if let Err(e) = crate::boot_progress::save(path, &format!("{} {} {} {}\n", g[0], g[1], g[2], g[3])) {
                eprintln!("makepad: cannot remember {}: {e}", path.display());
            }
        }
    }
    #[allow(unused_mut)]
    let mut env = Vec::new();
    #[cfg(target_os = "linux")]
    env.extend(makepad_hal::wm_prepare_exec());
    #[cfg(target_os = "macos")]
    env.extend(makepad_hal::restack_handover());
    // the window went to the start screen: the new run goes on from it
    env.push((RESTART_SCREEN.to_string(), if LEAVING.load(Ordering::SeqCst) { "1" } else { "0" }.to_string()));
    env
}

/// The exec after [`prepare_exec`] failed: the window manager's connection is this process's again.
#[cfg(any(unix, windows))]
pub fn undo_exec() {
    #[cfg(target_os = "linux")]
    makepad_hal::wm_undo_exec();
    // the app stays: the window goes back to it
    LEAVING.store(false, Ordering::SeqCst);
    Hal::global().wake_session(HOST);
}

pub fn observe(id: u32, next: &DomainState) {
    if state().as_ref().is_some_and(|r| r.domain == id) {
        post(Update::Domain(next.clone()));
    }
}

/// A click on the ring's dot, for the log's line of who asked for the restart: how long the dot
/// had shown and that the press began on it (the only press the dot takes), and whether the remote
/// endpoint fed it rather than a person (`input::MOD_INJECTED`).
fn dot_click(injected: bool, shown: Duration) -> String {
    let by = if injected { "a click the remote endpoint injected on the ring's dot (Update ready), no person's" } else { "a click on the ring's dot (Update ready)" };
    format!("{by}, pressed on the dot itself {:.1} s after it showed", shown.as_secs_f64())
}

/// The run window's Restart: a build that failed before the app started compiles again; a crashed
/// app's next run starts; a running app restarts as `session relaunch` does (with its state when
/// it restarts itself).
fn retry(crashed: bool, keyed: bool) -> Result<(), String> {
    if retry_failed_build()? {
        return Ok(());
    }
    if crashed {
        return relax_compiler::reload::restart_run("restart after the app crashed");
    }
    relax_compiler::reload::relaunch_pending(if keyed { "the R key in the run window" } else { "a click on the run window's Restart" }).map(|_| ())
}

/// A build that failed before the app started compiles again (true: no app runs, nothing of it to
/// keep). False: the app started. Err: it has not started yet.
fn retry_failed_build() -> Result<bool, String> {
    let mut guard = state();
    let r = guard.as_mut().ok_or("no run window")?;
    if r.initial_failed {
        // (its boot is due from here: a restart asked meanwhile waits for it)
        relax_compiler::run::boot_begins();
        r.retry = true;
        RETRY.notify_one();
        return Ok(true);
    }
    if r.domain == 0 {
        return Err("the app has not started yet".into());
    }
    Ok(false)
}

/// What a change of the sources did while the window shows a failure.
#[derive(Debug, PartialEq, Eq)]
enum AfterFailure {
    /// the build that failed before the app started compiles again
    Rebuild,
    /// the app crashed: its next run is offered (Restart), not started
    Offered,
}

/// The sources changed while the window shows a failure. A build that failed before the app
/// started compiles again. A crashed app waits for the person's Restart (the window's, the WM's
/// toast, `session relaunch`): offered, never started on its own -- a landing changes the sources
/// as a save does.
fn sources_changed_after_failure() -> Result<AfterFailure, String> {
    if retry_failed_build()? {
        return Ok(AfterFailure::Rebuild);
    }
    relax_compiler::reload::offer_restart_after_crash();
    Ok(AfterFailure::Offered)
}

fn work(rx: std::sync::mpsc::Receiver<Work>, package_dir: PathBuf, document: Document) {
    let _kept = relax_compiler::zone_alloc::kept();
    // only a package's own document is watched (the bundled one is the binary's)
    let document_path = if document.live { document.path.clone() } else { PathBuf::new() };
    let mut roots = Vec::new();
    if package_dir.is_dir() {
        roots.push(makepad_filesystem_watcher::WatchRoot {
            mount: "run".into(),
            path: package_dir.clone(),
        });
    }
    if let Some(parent) = document_path
        .parent()
        .filter(|p| p.is_dir() && !document_path.starts_with(&package_dir))
    {
        roots.push(makepad_filesystem_watcher::WatchRoot {
            mount: "boot".into(),
            path: parent.into(),
        });
    }
    let contents = std::sync::Arc::new(Mutex::new(document.source));
    let _watcher = if roots.is_empty() {
        None
    } else {
        use makepad_filesystem_watcher::FileSystemWatcher;
        FileSystemWatcher::start(roots, move |event| {
            // Watch roots catch atomic saves and backend rescan notifications too.
            if !document_path.as_os_str().is_empty() && (event.path == document_path || event.path.is_dir()) {
                match relax_compiler::vfs::read_to_string(&document_path) {
                    Ok(source) => {
                        let source: std::sync::Arc<str> = source.into();
                        let mut old = contents.lock().unwrap_or_else(|e| e.into_inner());
                        if *old != source {
                            *old = source.clone();
                            drop(old);
                            post(Update::Document(Document {
                                path: document_path.clone(),
                                source,
                                live: true,
                            }));
                        }
                    }
                    Err(error) => post(Update::Message(format!(
                        "{}: {error}",
                        document_path.display()
                    ))),
                }
            }
            if event.path.is_dir()
                || matches!(
                    event.path.extension().and_then(|e| e.to_str()),
                    Some("rs" | "toml")
                )
            {
                post(Update::SourcesChanged);
            }
        })
        .map_err(|e| eprintln!("makepad: run source watcher: {e}"))
        .ok()
    };
    let mut editor = crate::boot_editor::Editor::new();
    loop {
        for error in editor.poll() {
            post(Update::Message(error));
        }
        // the Studio connection is polled every 100 ms while there is one; without it the
        // worker sleeps until there is work
        let work = if editor.has_network() {
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(work) => work,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
        } else {
            match rx.recv() {
                Ok(work) => work,
                Err(_) => break,
            }
        };
        match work {
            Work::Save(path, text) => {
                if let Err(e) = crate::boot_progress::save(&path, &text) {
                    eprintln!("makepad: cannot remember {}: {e}", path.display());
                }
            }
            // the waiting edits stay noted until a run loads them (`domain_loaded`, or the new
            // process): the app may postpone the restart (unsaved work), a rebuild may come first
            Work::Relaunch(who) => {
                if let Err(e) = relax_compiler::reload::relaunch_pending(&who) {
                    post(Update::Message(e));
                }
                post(Update::Relaunched);
            }
            Work::OpenFile { file, line, col } => {
                if let Err(error) = editor.open_file(file, line, col) {
                    post(Update::Message(error));
                }
            }
            Work::Resume {
                panic,
                choice,
                value,
            } if choice == "restart-state" => {
                let _ = value;
                // the parked thread stays parked; the app restarts with its state at its safe
                // point (its UI thread is free) and the old run's parked threads go with it
                if let Err(e) = relax_compiler::reload::restart_with_state(format!("a restart after the panic {panic}")) {
                    post(Update::Message(e));
                }
                panic_changed();
            }
            Work::Resume {
                panic,
                choice,
                value,
            } => {
                // Reuse inspect's validation and its domain-restart / restartable-fn logic.
                let reply = makepad_remote::cmd::answer(
                    relax_compiler::inspect::events::cmd_resume,
                    &format!("resume {panic} {choice} {value}"),
                );
                for line in reply.split(|b| *b == b'\n') {
                    if let Ok(json) = relax_compiler::json::parse(line) {
                        if let Some(error) = json.get("err") {
                            post(Update::Message(error.str().into()));
                        }
                    }
                }
                panic_changed();
            }
        }
    }
}

fn parked(domain: u32) -> Option<Option<ParkedPanic>> {
    for event in relax_compiler::inspect::events::parked_events(domain)? {
        let Ok(json) = relax_compiler::json::parse(event.as_bytes()) else {
            continue;
        };
        let Some(id) = json.get("id") else { continue };
        let id = id.str().to_string();
        let offered: Vec<String> = json
            .get("more")
            .map(|j| j.arr())
            .unwrap_or(&[])
            .iter()
            .filter_map(|j| {
                let mut words = j.str().split_whitespace();
                (words.next() == Some("resume") && words.next() == Some(id.as_str()))
                    .then(|| words.next().map(str::to_string))
                    .flatten()
            })
            .collect();
        let thread = json.get("thread").map_or("", |j| j.str()).to_string();
        return Some(Some(ParkedPanic {
            id,
            message: json.get("message").map_or("", |j| j.str()).into(),
            location: json.get("location").map_or("", |j| j.str()).into(),
            choices: view_choices(&offered, thread == "main"),
            keeps_state: thread != "main" && relax_compiler::reload::restart_keeps_state(),
            thread,
            stale: json.get("kind").is_some_and(|k| k.str() == "stale access"),
        }));
    }
    Some(None)
}

/// The choices the paused view offers, first the one it leads with. A retry reruns the frame
/// that panicked; on another thread than the UI's the work it belonged to (a job, a pool task)
/// is not waiting for it, so a worker's panic leads with a restart and the unwind (the thread's
/// own catch boundary runs the app's failure path) and offers no retry.
fn view_choices(offered: &[String], main: bool) -> Vec<String> {
    let order: &[&str] = if main { &["retry", "restart", "panic", "return", "stop"] } else { &["restart", "panic", "return", "stop"] };
    order.iter().filter(|c| offered.iter().any(|o| o == *c)).map(|c| c.to_string()).collect()
}

impl Work {
    /// What the work is, for the log.
    fn kind(&self) -> String {
        match self {
            Work::Save(path, _) => format!("a save of {}", path.display()),
            Work::OpenFile { .. } => "an open-file request".into(),
            Work::Resume { .. } => "a resume".into(),
            Work::Relaunch(_) => "a restart".into(),
        }
    }
}

fn enqueue(queue: &mut VecDeque<Work>, work: Work) {
    // Geometry/timings are snapshots; a pending write of the same path can take
    // the newer value. User actions retain their order and are never dropped.
    match &work {
        Work::Save(path, text) => {
            for pending in queue.iter_mut().rev() {
                match pending {
                    Work::Save(old_path, old_text) if old_path == path => {
                        old_text.clone_from(text);
                        return;
                    }
                    _ => {}
                }
            }
        }
        Work::Resume { .. } | Work::OpenFile { .. } | Work::Relaunch(_) => {}
    }
    queue.push_back(work);
}

/// What became of the run's window when the run let it go ([`close_run_window`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum WindowGone {
    Closed,
    /// a WM's tile, which cannot close without ending the process
    Tile,
    /// lent to the app, which draws in it: the app's window now
    Apps,
}

/// The run's window goes and the run stays (`Boot::close_window`), for `why`, said in the log:
/// from here on the run is one without a window: nobody answers a parked panic from it, and a
/// failure ends the run. A WM's tile stays, and so does a window lent to the app: only the app
/// or the person closes the window the app draws in. (A program the run took for a batch program
/// lost the window it drew in, and with its last window it ended: Scope, 28-109 s after each
/// start, 11-06.)
fn close_run_window(host: &mut HostWindow, why: &str) -> WindowGone {
    let lent = host.window().is_some_and(|window| {
        Hal::global().with_world(|w| w.session(HOST).and_then(|s| s.windows.get(HOST, window).ok()).is_some_and(|e| e.lent_to != 0))
    });
    if lent {
        eprintln!("relax: the run's window stays ({why}): the app draws in it");
        return WindowGone::Apps;
    }
    if !host.close_window() {
        return WindowGone::Tile;
    }
    eprintln!("relax: the run's window closes: {why}");
    relax_compiler::inspect::events::run_state_available(false);
    if let Some(r) = state().as_mut() {
        r.window = Id::NONE;
    }
    WindowGone::Closed
}

fn pump(
    mut host: HostWindow,
    mut data: State,
    history: History,
    geometry_path: Option<PathBuf>,
    package_dir: PathBuf,
    document: Document,
    ready: std::sync::mpsc::SyncSender<Result<(), String>>,
) {
    let hal = Hal::global();
    let (send, recv) = std::sync::mpsc::sync_channel(1);
    let worker = std::thread::Builder::new()
        .name("run-run-actions".into())
        .spawn(move || work(recv, package_dir, document))
        .expect("run action worker");
    let mut work_pending = VecDeque::new();
    let mut work_full = false;
    let mut has_window = host.window().is_some();
    let unseen = unseen();
    let mut probe = crate::boot_probe::Probe::from_env();
    if probe.is_some() {
        host.measure_input();
    }
    // A run after a restart (a restart in this process keeps the window: `Update::NextRun`; a process
    // restarted into a rebuilt makepad, reload_host) draws nothing of its own while it compiles: the
    // app's last frame stays until the new one replaces it, with no flash of another picture. A
    // failure, a parked thread or an offered update still shows. So its compile does not wait for
    // the window's first present (there is none before the app's): the window is open now. A restart
    // the person asked for went to the start screen first (`restart_begins`): that run shows it
    // from its first frame, as a first start does.
    let restart_t0 = relax_compiler::run_end::run_input("RELAX_RESTART_T0").is_some();
    let mut restarted = restart_t0 && std::env::var_os(RESTART_SCREEN).is_none_or(|v| v != "1");
    if !restarted && restart_t0 {
        host.continues_start_screen();
    }
    let mut ready = if has_window && !restarted {
        Some(ready)
    } else {
        let _ = ready.try_send(Ok(()));
        None
    };
    let mut selected = 0usize;
    let mut input = crate::boot_input::Input::default();
    let mut return_entry: Option<(String, String)> = None;
    let mut progress = Progress::new(history);
    let mut failed = false;
    let mut patch_notice: Option<String> = None;
    let mut action_failed = false;
    let mut update_ready = false;
    let mut overlay_dismissed = false;
    let mut active = true;
    let mut diagnostics = Vec::new();
    let mut diagnostic_keys = HashSet::new();
    let mut diagnostics_dirty = true;
    let mut document_errors = Vec::new();
    let mut saved_geometry = None;
    // the geometry last seen and since when: saved once it has held still (not every frame of
    // a drag, a resize or a window animation, which kept the action worker full)
    let mut settling_geometry: Option<([f32; 4], Instant)> = None;
    let mut next_frame = Instant::now();
    let mut next_status = Instant::now();
    let mut dirty = true;
    let mut stage = Stage::Running;
    // the ring's dot was clicked: it answers at once, before the worker has taken the request
    let mut ring_clicked = false;
    // when the app began to fade to the start screen, and in from it
    let mut leave_at: Option<Instant> = None;
    let mut arrive_at: Option<Instant> = None;
    let mut dot_noted = None;
    let ring = ring_on();
    // A dump-only preview: after the running app's first half second, show a UI wait's look
    // for three seconds and fade it out. No synthetic JIT wait or input change. Works hidden
    // even with the ring disabled: RELAX_WAIT_PREVIEW=1.
    let wait_preview = host.dumping() && std::env::var_os("RELAX_WAIT_PREVIEW").is_some();
    let mut wait_preview_at: Option<Instant> = None;
    // the run's inspect view (the halted view over the app without a stop): opened by
    // `makepad inspect <app> session view open` (the JIT ring is display only)
    let mut inspect_open = false;
    // what the view over the app showed last (its `session view` answer and the log line)
    let mut view_shown: Option<String> = None;
    // a WM's tile hidden for a tray app (`Boot::hide_window`): back once it has something to show
    let mut window_hidden = false;
    relax_compiler::inspect::run_view::set_wake(|| Hal::global().wake_session(HOST));
    let mut warm_up_at: Option<Instant> = None;
    // a sustained UI wait's look over the app came on at (the log says how long it stayed)
    let mut wait_at: Option<Instant> = None;
    // RELAX_POSTFRAME_LOG (the responsiveness trace): the JIT ring's UI blocks too
    let mut watch = Watch::new(std::env::var_os("RELAX_POSTFRAME_LOG").is_some());
    relax_compiler::postframe::on_ui_wait(ui_wait_began);
    // (it is at the run's state except while it waits for the window)
    let mut gate = Some(relax_compiler::run_threads::process_work());
    loop {
        let (domain, window, initial_failed) = {
            let guard = state();
            let r = guard.as_ref().expect("host run");
            (r.domain, r.window, r.initial_failed)
        };
        if (unseen || !has_window) && relax_compiler::hot_failure::has_error() {
            EXIT.store(makepad_remote::hot::JIT_ERROR_EXIT, Ordering::Release);
            end_run("a JIT error that no window shows");
        }
        if let Some(probe) = &mut probe {
            probe.turn(active, &data);
        }
        if QUIT.load(Ordering::Acquire) {
            break;
        }
        let updates = std::mem::take(&mut *UPDATES.lock().unwrap_or_else(|e| e.into_inner()));
        for update in updates {
            dirty = true;
            match update {
                Update::NextRun { start_screen } => {
                    restarted = !start_screen;
                    if start_screen {
                        host.continues_start_screen();
                    }
                    ring_clicked = false;
                    leave_at = None;
                    arrive_at = None;
                    inspect_open = false;
                    view_shown = None;
                }
                Update::Begin => {
                    stage = Stage::Running;
                    progress.reset();
                    data.phase = Phase::Resolve;
                    data.done = None;
                    data.total = None;
                    data.current.clear();
                    data.warm = false;
                    data.elapsed = 0.0;
                    data.completed_weight = 0.0;
                    data.phase_elapsed = 0.0;
                    data.parked = None;
                    data.domain = "Compiling".into();
                    diagnostics.clear();
                    diagnostic_keys.clear();
                    diagnostics_dirty = true;
                    failed = false;
                    patch_notice = None;
                    action_failed = false;
                    update_ready = false;
                    overlay_dismissed = false;
                    active = true;
                }
                Update::Failure(message) if !has_window || unseen => {
                    if relax_compiler::hot_failure::recovering() {
                        relax_compiler::hot_failure::restart_failed(&message);
                        continue;
                    }
                    // No window shows this failure and nobody can retry from it (a backend without
                    // a shader dialect: headless, a plain program; or a hidden window, which nobody
                    // sees (MAKEPAD_HIDE_WINDOWS)): the run ends with it. A program that panicked
                    // in main used to leave the process running until it was killed.
                    eprintln!("makepad: {message}");
                    let code = relax_compiler::hot_failure::verdict(101).code();
                    let _ = EXIT.compare_exchange(0, code, Ordering::AcqRel, Ordering::Acquire);
                    end_run("its failure (above) has no window to show it");
                }
                Update::Failure(message) => {
                    diagnostics.push(Diagnostic::new("run", "", 0, 0, message));
                    diagnostics_dirty = true;
                    failed = true;
                    active = false;
                    data.domain = "Boot failed".into();
                }
                Update::Relaunched => ring_clicked = false,
                Update::Message(message) => {
                    diagnostics.push(Diagnostic::new("run", "", 0, 0, message));
                    diagnostics_dirty = true;
                    overlay_dismissed = false;
                    action_failed = true;
                }
                Update::Batch => {
                    if close_run_window(&mut host, "the program went quiet without starting an app: a batch program draws no frame") == WindowGone::Closed {
                        has_window = false;
                    }
                }
                Update::Document(document) => host.reload(document),
                Update::Status(text) => {
                    data.domain = text;
                    data.current.clear();
                }
                Update::Identity(app, icon) => {
                    data.app = app;
                    if let Some(icon) = icon {
                        host.icon(icon);
                    }
                }
                Update::Environment(environment) => host.environment(environment),
                Update::SourcesChanged if failed => match sources_changed_after_failure() {
                    Ok(AfterFailure::Rebuild) => failed = false,
                    Ok(AfterFailure::Offered) => data.domain = "Boot failed · sources changed: Restart".into(),
                    Err(e) => post(Update::Message(e)),
                },
                Update::SourcesChanged => {}
                Update::Domain(next) => match next {
                    DomainState::Compiling(name) => {
                        let phase = match name.as_str() {
                            "loading crates" | "load" => Phase::Parse,
                            "collect" => Phase::Typecheck,
                            "link" => Phase::Link,
                            "main" => Phase::Codegen,
                            _ => Phase::Resolve,
                        };
                        progress.phase(phase);
                        data.phase = phase;
                        data.done = None;
                        data.total = None;
                        data.current = name;
                        data.domain = "Compiling".into();
                    }
                    DomainState::Running => {
                        data.domain = "Starting".into();
                        progress.phase(Phase::FirstFrame);
                        data.phase = Phase::FirstFrame;
                        data.done = None;
                        data.total = None;
                    }
                    // main returned or the program called exit: the process ends with its code,
                    // as a rustc build's does (a program that runs another as a child waits for
                    // that code); a crash is what the run window stays up for
                    DomainState::Exited(code) => {
                        EXIT.store(code, Ordering::Release);
                        end_run(&format!("the app exited ({code})"));
                    }
                    // a run that is no live session takes no fix: it ends as a rustc build's
                    // panic does (101), with the message on stderr
                    DomainState::Crashed(message) if !relax_compiler::run::live_session() => {
                        eprintln!("makepad: {message}");
                        EXIT.store(relax_compiler::hot_failure::verdict(101).code(), Ordering::Release);
                        end_run("the app crashed (above), and no live session restarts it");
                    }
                    DomainState::Crashed(message) => post(Update::Failure(message)),
                    DomainState::Stopped => {}
                },
                Update::Compiler(event) => match event {
                    run_state::Event::UpdateReady { domain: id, message }
                        if id.is_none() || id == Some(domain) =>
                    {
                        active = false;
                        update_ready = true;
                        overlay_dismissed = false;
                        data.domain = message;
                        data.current.clear();
                        if let Some(r) = state().as_mut() {
                            r.initial_failed = false;
                        }
                    }
                    run_state::Event::Progress {
                        domain: id,
                        phase,
                        done,
                        total,
                        item,
                    } if id.is_none() || id == Some(domain) => {
                        progress.phase(phase);
                        data.phase = phase;
                        data.done = done;
                        data.total = total;
                        data.current = item;
                    }
                    run_state::Event::Warm { domain: None, warm } =>
                    {
                        data.warm = warm
                    }
                    run_state::Event::Diagnostic { domain: id, record }
                        if id.is_none() || id == Some(domain) =>
                    {
                        if diagnostic_keys.insert(record.to_json()) {
                            if !active && record.severity == relax_compiler::diag::Severity::Error {
                                patch_notice = Some(format!("{}: {}", relax_boot::ring_summary(None, Some(RingAlert::PatchFailed)), record.message));
                                if !update_ready { data.domain = "Patch not applied · previous code is running".into(); }
                            }
                            diagnostics.push(record);
                            diagnostics_dirty = true;
                        }
                    }
                    run_state::Event::Patch {
                        domain: id,
                        applied,
                        records,
                    } if id.is_none() || id == Some(domain) => {
                        patch_notice = (!applied).then(|| format!("{}: {}", relax_boot::ring_summary(None, Some(RingAlert::PatchFailed)), records.iter().map(|r| r.message.as_str()).collect::<Vec<_>>().join("; ")));
                        action_failed = false;
                        if !active && !update_ready {
                            data.domain = if applied {
                                "Running"
                            } else {
                                "Patch not applied · previous code is running"
                            }
                            .into();
                        }
                        diagnostic_keys = records.iter().map(Diagnostic::to_json).collect();
                        diagnostics = records.to_vec();
                        diagnostics_dirty = true;
                    }
                    _ => {}
                },
            }
        }
        if QUIT.load(Ordering::Acquire) {
            break;
        }
        if PANICS_CHANGED.swap(false, Ordering::AcqRel) {
            if let Some(parked) = parked(domain) {
                if data.parked.as_ref().map(|p| &p.id) != parked.as_ref().map(|p| &p.id) {
                    if parked.is_some() {
                        overlay_dismissed = false;
                    }
                }
                data.parked = parked;
                dirty = true;
            } else {
                PANICS_CHANGED.store(true, Ordering::Release);
            }
        }
        if return_entry
            .as_ref()
            .is_some_and(|(id, _)| data.parked.as_ref().map(|p| &p.id) != Some(id))
        {
            return_entry = None;
            host.input(None);
        }
        let physical = hal.with_world(|w| {
            w.session(HOST)
                .and_then(|s| s.windows.get(HOST, window).ok())
                .map(|w| {
                    (
                        w.val.geom,
                        w.val.borrower_frame,
                        w.val.presentations,
                        w.val.borrower_presented,
                        w.lent_to,
                        // a layer the HAL composes that has not taken the app's newest frame
                        w.val.overlay.as_ref().is_some_and(|o| o.composed && o.fresh && !o.layer.is_none()),
                    )
                })
        });
        let mut may_present = true;
        let mut app_frame = false;
        let mut app_frames = 0;
        let mut layer_behind = false;
        let mut lent_to_app = false;
        let first_present = active;
        if let Some((geom, borrower_frame, presentations, borrower_presented, lent, behind)) = physical {
            app_frames = presentations;
            layer_behind = behind;
            lent_to_app = lent == domain && domain != 0;
            app_frame = borrower_frame && lent == domain && domain != 0;
            may_present = !borrower_frame || lent == 0;
            if settling_geometry.is_none_or(|(g, _)| g != geom) {
                settling_geometry = Some((geom, Instant::now()));
            }
            let settled = settling_geometry.is_some_and(|(_, since)| since.elapsed() >= Duration::from_millis(500));
            if let Some(path) = geometry_path.as_ref().filter(|_| {
                settled
                    && saved_geometry != Some(geom)
                    && geom.iter().all(|v| v.is_finite())
                    && geom[2] > 0.0
                    && geom[3] > 0.0
            }) {
                saved_geometry = Some(geom);
                enqueue(
                    &mut work_pending,
                    Work::Save(
                        path.clone(),
                        format!("{} {} {} {}\n", geom[0], geom[1], geom[2], geom[3]),
                    ),
                );
            }
            if presentations > 0 {
                if let Some(ready) = ready.take() {
                    relax_compiler::run::boot_window_waits(None);
                    let _ = ready.try_send(Ok(()));
                }
            }
            if active
                && borrower_frame
                && lent == domain
                && domain != 0
                && borrower_presented
                && presentations > 0
            {
                active = false;
                failed = false;
                data.domain = "Running".into();
                EXIT.store(0, Ordering::Release);
                let (path, contents) = progress.finish(data.warm);
                enqueue(&mut work_pending, Work::Save(path, contents));
                dirty = true;
            }
        }
        // The app showed itself without the run's window (a tray app: its first present was its
        // tray, or a window of its own such as a dropdown): nothing will take the load screen
        // over, so it goes now, as a batch program's does. A window the app opens later is one of
        // its own. A WM's tile cannot close without ending the process: it is hidden (the WM
        // shows a minimised tile, as for a tray app's closed window) and comes back below.
        if active && !lent_to_app && domain != 0 {
            if let Some(ago) = relax_compiler::domain::presented_ago(domain) {
                active = false;
                failed = false;
                stage = Stage::Running;
                data.domain = "Running".into();
                EXIT.store(0, Ordering::Release);
                let (path, contents) = progress.finish(data.warm);
                enqueue(&mut work_pending, Work::Save(path, contents));
                dirty = true;
                let gone = if !has_window {
                    "had no window"
                } else {
                    match close_run_window(&mut host, "the app showed itself without it (a tray app)") {
                        WindowGone::Closed => {
                            has_window = false;
                            "closes"
                        }
                        WindowGone::Tile => {
                            host.hide_window(true);
                            window_hidden = true;
                            "hides (a WM's tile)"
                        }
                        WindowGone::Apps => "stays: the app draws in it",
                    }
                };
                eprintln!(
                    "relax: the app showed itself without the run's window (a tray app), {:.0} ms ago: the load screen {gone}",
                    ago.as_secs_f64() * 1000.0
                );
            }
        }
        // the hidden tile comes back with something to show: the app's window (the app took it),
        // a failure, or a new start
        if window_hidden && (lent_to_app || failed || active) {
            host.hide_window(false);
            window_hidden = false;
        }
        // the view over an app whose UI thread is parked stays: nothing else answers its window
        let ui_parked = data.parked.as_ref().is_some_and(|p| p.thread == "main");
        match relax_compiler::inspect::run_view::take() {
            Some(true) => {
                inspect_open = true;
                overlay_dismissed = false;
                dirty = true;
            }
            Some(false) if ui_parked => {}
            Some(false) => {
                overlay_dismissed = true;
                if std::mem::take(&mut inspect_open) && data.domain.starts_with("JIT · ") {
                    data.domain = "Running".into();
                }
                dirty = true;
            }
            None => {}
        }
        // A refused patch is a ring notice: it never covers the running app.
        let overlay = has_window
            && app_frame
            && !overlay_dismissed
            && (action_failed
                || failed
                || update_ready
                || inspect_open
                || !document_errors.is_empty()
                || data.parked.is_some());
        // what the view over the app shows and why: `session view`, and a line when it changes
        let why = overlay.then(|| {
            if data.parked.as_ref().is_some_and(|p| p.stale) {
                "paused on invalidated code"
            } else if data.parked.is_some() {
                "paused on a panic"
            } else if failed {
                "the app stopped"
            } else if update_ready {
                "the JIT itself changed"
            } else if action_failed {
                "a run action failed"
            } else if !document_errors.is_empty() {
                "the boot document has errors"
            } else {
                "the inspect view (session view open)"
            }
        });
        // what the paused view offers (the choices in its order) and how it is drawn: over the
        // app's dimmed last frame in its own window (a window layer) or as the opaque panel
        let paused = data.parked.as_ref().map_or(String::new(), |p| {
            let choices: Vec<String> = p.choices.iter().map(|c| format!("\"{c}\"")).collect();
            format!(
                ",\"thread\":{},\"choices\":[{}],\"keeps_state\":{}",
                makepad_remote::json_str(&p.thread),
                choices.join(","),
                p.keeps_state
            )
        });
        let drawn = if !overlay { "null" } else if host.layers() { "\"layer\"" } else { "\"panel\"" };
        // the run's window: on the screen, hidden (a WM's tile for a tray app) or closed
        let window_is = if !has_window { "closed" } else if window_hidden { "hidden" } else { "open" };
        let shown = format!(
            "\"over_app\":{},\"why\":{},\"dismissed\":{},\"drawn\":{}{},\"pid\":{},\"patch_failed\":{},\"patch_notice\":{},\"window\":\"{window_is}\"",
            overlay,
            why.map_or("null".to_string(), |w| format!("\"{w}\"")),
            overlay_dismissed,
            drawn,
            paused,
            std::process::id(),
            patch_notice.is_some(),
            patch_notice.as_deref().map_or_else(|| "null".into(), makepad_remote::json_str)
        );
        if view_shown.as_deref() != Some(shown.as_str()) {
            let was_over = view_shown.as_deref().is_some_and(|v| v.starts_with("\"over_app\":true"));
            if why.is_some() && !was_over {
                // in front of the app's other windows (a window left over from a restart in place
                // stood over it on macOS: the view was seen only once that window closed)
                host.raise();
            }
            match why {
                Some(why) if ui_parked => eprintln!("relax: the run's view is over the app: {why}; its UI thread is parked, so the view stays until a choice is made"),
                Some(why) => eprintln!(
                    "relax: the run's view is over the app: {why}; Esc or `makepad inspect {} session view dismiss` dismisses it",
                    std::process::id()
                ),
                None if was_over => eprintln!("relax: the run's view is off the app"),
                None => {}
            }
            relax_compiler::inspect::run_view::publish(shown.clone());
            view_shown = Some(shown);
        }
        let mode = if update_ready { Mode::Update } else if overlay { Mode::Overlay } else { Mode::Boot };
        if data.mode != mode {
            data.mode = mode;
            dirty = true;
        }
        let visible = overlay
            || (has_window
                && may_present
                && (failed
                    || update_ready
                    || data.parked.is_some()
                    || (active && !restarted)));
        // the window layer: the boot field the app's first frames go under, the warm-up
        // after its first present (until the JIT's readiness signal). An opaque overlay, a failure
        // or a stop takes it away. A later sustained UI wait brings the warm-up look back, below.
        // the boot stage begins before the app has the window, once the field has presented
        // (its first frame then never reaches the window before the layer)
        let layers = host.layers() && !overlay && !failed && !update_ready && data.parked.is_none();
        if !layers || (stage != Stage::Boot && !lent_to_app) {
            stage = Stage::Running;
        }
        stage = match stage {
            Stage::Running if layers && active && visible && host.field_mode() && app_frames > 0 => {
                Stage::Boot
            }
            // compile errors in the boot window show its halted view, not the field
            Stage::Boot if active && !host.field_mode() => Stage::Running,
            Stage::Boot if !active && first_present => {
                warm_up_at = Some(Instant::now());
                Stage::WarmUp
            }
            Stage::WarmUp if relax_compiler::postframe::ready(READY_QUIET_NS) => {
                if let Some(at) = warm_up_at.take() {
                    eprintln!(
                        "relax: warm-up {:.0} ms after the first present ({})",
                        at.elapsed().as_secs_f64() * 1000.0,
                        if data.warm { "warm" } else { "cold" }
                    );
                }
                Stage::Lift
            }
            Stage::Lift if host.layer_done() => Stage::Running,
            stage => stage,
        };
        // the restart itself: the window goes to the start screen, in its badge layer
        let leaving = LEAVING.load(Ordering::SeqCst);
        if leaving {
            stage = Stage::Running;
            if leave_at.is_none() {
                host.start_screen_anew();
            }
        } else {
            leave_at = None;
        }
        // a backend without window layers: the start screen in the badge layer from the moment
        // the app has the window, so its first frames land under it; the first present fades in
        // (a window layer does this itself: `Stage`)
        let veil_holds = !host.layers() && host.badges() && lent_to_app && active && !restarted && !overlay && !failed && host.field_mode();
        if first_present && !active && !restarted && !host.layers() && host.badges() && lent_to_app {
            arrive_at = Some(Instant::now());
        }
        let veil = if leaving {
            let t = leave_at.get_or_insert_with(Instant::now).elapsed().as_secs_f32() / VEIL_IN.as_secs_f32();
            (lent_to_app && !overlay).then(|| smooth(t))
        } else if veil_holds {
            Some(1.0)
        } else if let Some(t) = arrive_at.map(|at| at.elapsed().as_secs_f32() / VEIL_OUT.as_secs_f32()) {
            if t >= 1.0 || overlay || failed { arrive_at = None; None } else { Some(1.0 - smooth(t)) }
        } else {
            None
        };
        let mut wake_in = None;
        let want = match stage {
            Stage::Boot => Some(LayerWant::Boot),
            Stage::WarmUp => {
                let (done, queued) = relax_compiler::postframe::ahead_progress();
                Some(LayerWant::WarmUp { done, queued })
            }
            Stage::Lift => Some(LayerWant::Ready),
            Stage::Running => None,
        };
        // the view over a paused app: in the app's own window, over its dimmed last frame, where
        // the backend composes window layers (else the opaque panel `overlay` asks for)
        let mut want = if overlay && host.layers() { Some(LayerWant::Paused) } else { want };
        // the JIT ring while the app runs: the JIT's work in flight, a UI block, what the program
        // is stopped on (while it is)
        let running = stage == Stage::Running && lent_to_app && !active && !failed && veil.is_none() && !leaving;
        let update_message = relax_compiler::reload::host::update_message();
        // the person asked for the restart: the ring says how far it is, no dot offers it again
        let restart_asked = ring_clicked || relax_compiler::reload::host::restart_asked();
        let alert = data
            .parked
            .as_ref()
            .map(|p| if p.stale { RingAlert::Stale } else { RingAlert::Panic })
            .or_else(|| rebuild_failed().then_some(RingAlert::RebuildFailed))
            .or_else(|| (!restart_asked && (update_message.is_some() || !relax_compiler::reload::restart_pending().is_empty())).then_some(RingAlert::UpdateReady))
            .or_else(|| (!restart_asked && patch_notice.is_some()).then_some(RingAlert::PatchFailed));
        let ring_detail = match alert {
            Some(RingAlert::UpdateReady) => update_message.as_deref(),
            Some(RingAlert::PatchFailed) => patch_notice.as_deref(),
            _ => None,
        };
        // the dot's tooltip: what it means and what a click on it does (only while it shows)
        let ring_tip = (running && ring).then(|| match alert {
            Some(_) => ring_tip(alert, &data, &diagnostics, update_message.as_deref()),
            None => restart_asked.then(|| {
                use relax_compiler::reload::host;
                let (phase, since) = host::status();
                // (clicked, the worker yet to start the build)
                let phase = if ring_clicked && host::route() == host::Route::BuildFirst { host::Phase::Building } else { phase };
                let keeps = if relax_compiler::reload::restart_keeps_state() { " with its state" } else { "" };
                restart_says(phase, since.as_secs(), &host::build_progress(), host::restart_pinned(), keeps)
            }),
        }).flatten();
        // The existing wait hook is the only idle check. No wait work (layout or drawing) until
        // it is armed. An app present (or an episode gap without a wait) ends the episode, and
        // with it the wait's look.
        let can_wait = running && layers && !update_ready && patch_notice.is_none() && !action_failed
            && document_errors.is_empty() && !matches!(alert, Some(RingAlert::RebuildFailed | RingAlert::UpdateReady));
        // ui_blocked_ns is the episode's: a load's thousands of short waits count as one wait
        // until the UI thread has been free for an episode gap (it drew a frame), so the look
        // holds through the gaps between them instead of flickering
        let waiting = can_wait && UI_WAIT_ARMED.load(Ordering::Acquire)
            && relax_compiler::postframe::ui_blocked_ns() > WAIT_AFTER.as_nanos() as u64;
        if waiting {
            want = Some(LayerWant::Waiting);
        }
        if waiting != wait_at.is_some() {
            match wait_at.take() {
                Some(at) => {
                    // (a view that takes the window, an alert, a failure: the look gives way to it)
                    let why = if can_wait { "the UI is free again: the load screen fades out" } else { "the load screen gives way to the run's view" };
                    eprintln!("relax: after {:.0} ms {why}", at.elapsed().as_secs_f64() * 1000.0);
                }
                None => {
                    wait_at = Some(Instant::now());
                    eprintln!("relax: the UI waits on the JIT ({:.0} ms): the load screen fades in", relax_compiler::postframe::ui_blocked_ns() as f64 / 1e6);
                }
            }
        }
        if can_wait {
            if wait_preview {
                let elapsed = wait_preview_at.get_or_insert_with(Instant::now).elapsed();
                if elapsed >= WAIT_AFTER && elapsed < Duration::from_millis(3500) {
                    want = Some(LayerWant::Waiting);
                } else if elapsed < WAIT_AFTER {
                    wake_in = Some(WAIT_AFTER - elapsed);
                }
            }
            if let Some(wait) = watch.block_wake() {
                wake_in = Some(wake_in.map_or(wait, |w: Duration| w.min(wait)));
            }
        }
        host.layer(want, app_frames, layer_behind);
        let veiled = host.veil(veil);
        if let Some(wait) = veiled {
            wake_in = Some(wake_in.map_or(wait, |w: Duration| w.min(wait)));
        }
        if leaving && (veiled.is_none() || veil.is_some_and(|opacity| opacity >= 1.0)) {
            *LEFT.0.lock().unwrap_or_else(|e| e.into_inner()) = true;
            LEFT.1.notify_all();
        }
        if running && ring && host.badges() {
            let want = watch.look(host.dump_ms(), restart_asked).filter(|_| data.parked.is_none());
            if inspect_open {
                let summary = ring_detail.map(str::to_owned)
                    .unwrap_or_else(|| format!("JIT · {}", relax_boot::ring_summary(want, alert)));
                if data.domain != summary && data.parked.is_none() && data.diagnostics.is_empty() {
                    data.domain = summary;
                    dirty = true;
                }
            }
            let shown = host.ring(want, alert, ring_tip.as_deref());
            for wait in [shown, watch.block_wake(), Some(RING_LOOK)].into_iter().flatten() {
                wake_in = Some(wake_in.map_or(wait, |w: Duration| w.min(wait)));
            }
        } else if let Some(wait) = host.ring(None, None, None) {
            // a ring fading out
            wake_in = Some(wake_in.map_or(wait, |w: Duration| w.min(wait)));
        }
        if running && host.dumping() {
            host.dump_app();
            wake_in = Some(wake_in.map_or(RING_LOOK, |w: Duration| w.min(RING_LOOK)));
        }
        let dot = host.ring_place();
        if dot != dot_noted {
            dot_noted = dot;
            relax_compiler::reload::host::note_dot(dot);
        }
        let layered = host.layer_on();
        if active && (dirty || Instant::now() >= next_status) {
            progress.tick(&mut data);
            if matches!(data.phase, Phase::Codegen | Phase::FirstFrame) {
                if let Some(item) = relax_compiler::tui::current_item() {
                    data.current = item;
                }
            }
            next_status = Instant::now() + Duration::from_millis(100);
            dirty = true;
        }
        if diagnostics_dirty {
            let records = diagnostics.iter().cloned();
            let records = records.chain(document_errors.iter().cloned());
            // root causes first, a folded group's members together (the boot window's list order;
            // the selection, Copy and Open index the same list)
            let mut records = records.collect::<Vec<_>>();
            relax_boot::order_diagnostics(&mut records);
            data.diagnostics = records.into();
            diagnostics_dirty = false;
            dirty = true;
        }
        let count = data.diagnostics.len() + usize::from(data.parked.is_some());
        let next = selected.min(count.saturating_sub(1));
        if next != selected {
            selected = next;
            host.select(selected);
        }
        if dirty {
            data.can_retry = !update_ready && (initial_failed || (failed && domain != 0));
            data.can_restart = update_ready || domain != 0;
            host.update(data.clone());
            dirty = false;
        }
        let hold = !visible && !layered && host.hold();
        host.offscreen(hold);
        // paced by the display (the window is lent): the frames draw on its beats, which the
        // boot asks for and which wake this loop; else on the clock below
        let paced = host.paced();
        let frames = visible || hold || layered;
        let draw = frames && (paced || Instant::now() >= next_frame);
        if let Some(wait) = host.frame_wait().filter(|_| frames) {
            wake_in = Some(wake_in.map_or(wait, |w: Duration| w.min(wait)));
        }
        let timeout = if frames && !paced {
            next_frame
                .saturating_duration_since(Instant::now())
                .as_nanos() as u64
        } else if let Some(wake_in) = wake_in {
            wake_in.as_nanos() as u64
        } else if (has_window && active)
            || !work_pending.is_empty()
            || PANICS_CHANGED.load(Ordering::Acquire)
        {
            16_666_667
        } else {
            u64::MAX
        };
        drop(gate.take());
        let alive = host.turn(timeout, draw);
        gate = Some(relax_compiler::run_threads::process_work());
        if window.is_none() {
            if let Some(window) = host.window() {
                if let Some(r) = state().as_mut() {
                    r.window = window;
                }
            }
        }
        if let Some(probe) = &mut probe {
            if let Some(window) = host.window() {
                probe.window(window);
            }
            probe.parts(host.turn_parts());
            if host.turn_drew() {
                probe.frame(&data);
            }
            probe.latencies(host.take_latencies(), &data);
        }
        if !alive {
            end_run("the run window ended (above)");
            break;
        }
        // until this window's first present: what it waits for (a frame drawn and not presented
        // yet: the window system's); the run says it while its start waits for the window
        if ready.is_some() {
            relax_compiler::run::boot_window_waits(Some(
                host.first_frame_waits().unwrap_or("its frame is drawn; the window system has presented none"),
            ));
        }
        if draw && !paced {
            // frames on a fixed 60 Hz schedule: the time a turn takes is not added to every
            // interval; a turn that fell behind starts the schedule again from now
            let period = Duration::from_nanos(host.frame_period_ns());
            let now = Instant::now();
            next_frame += period;
            if next_frame <= now {
                next_frame = now + period;
            }
        }
        if !visible {
            input.cancel(&mut host);
        }
        let mut close = false;
        for event in host.events() {
            // (the log names a restart's key apart from a click)
            let keyed = matches!(event, BootEvent::KeyDown(_));
            let action = match event {
                // a click on the ring's dot acts on what it shows
                BootEvent::RingPressed { injected, shown } => {
                    match alert {
                        // the restart a waiting change asks for (the paused view's Restart):
                        // with the app's state where it restarts with it, never without the click
                        Some(RingAlert::UpdateReady) => {
                            enqueue(&mut work_pending, Work::Relaunch(dot_click(injected, shown)));
                            // the dot answers now: the circle in the next frame, which is the
                            // next pass of this loop
                            ring_clicked = true;
                        }
                        // the inspect view, its first error selected
                        Some(RingAlert::RebuildFailed | RingAlert::PatchFailed) => {
                            inspect_open = true;
                            overlay_dismissed = false;
                            let first = data.diagnostics.iter().position(|d| d.severity == relax_compiler::diag::Severity::Error);
                            if let Some(index) = first {
                                selected = index;
                                host.select(selected);
                            }
                            dirty = true;
                        }
                        // the paused view
                        Some(RingAlert::Stale | RingAlert::Panic) => {
                            overlay_dismissed = false;
                            dirty = true;
                        }
                        None => {}
                    }
                    None
                }
                BootEvent::DocumentErrors(errors) => {
                    if !errors.is_empty() {
                        overlay_dismissed = false;
                    }
                    document_errors = errors;
                    diagnostics_dirty = true;
                    None
                }
                BootEvent::Text(text) if visible && return_entry.is_some() => {
                    if let Some((_, value)) = &mut return_entry {
                        value.push_str(&text.replace(['\r', '\n'], " "));
                        host.input(Some(value.clone()));
                    }
                    None
                }
                BootEvent::Input(raw, payload) if visible => raw
                    .decode(&payload)
                    .and_then(|event| input.event(&mut host, event)),
                BootEvent::KeyDown(raw) if visible => {
                    let Some(makepad_hal::HalEvent::KeyDown {
                        key, repeat, mods, ..
                    }) = raw.decode(&[])
                    else {
                        continue;
                    };
                    let key = KeyCode::from_index(key);
                    if return_entry.is_some() {
                        match key {
                            KeyCode::Escape => Some(Action::Cancel),
                            KeyCode::Backspace => {
                                if let Some((_, value)) = &mut return_entry {
                                    value.pop();
                                    host.input(Some(value.clone()));
                                }
                                None
                            }
                            KeyCode::ReturnKey | KeyCode::NumpadEnter if !repeat => {
                                Some(Action::Submit)
                            }
                            _ => None,
                        }
                    } else if mods
                        & (makepad_hal::input::MOD_CONTROL
                            | makepad_hal::input::MOD_ALT
                            | makepad_hal::input::MOD_LOGO)
                        != 0
                    {
                        None
                    } else {
                        match key {
                            KeyCode::Escape if !repeat => Some(Action::Dismiss),
                            KeyCode::KeyR if !repeat => Some(Action::Restart),
                            KeyCode::KeyC if !repeat => Some(Action::Copy),
                            KeyCode::KeyE if !repeat => Some(Action::Open),
                            KeyCode::ArrowUp => {
                                selected = selected.saturating_sub(1);
                                host.select(selected);
                                None
                            }
                            KeyCode::ArrowDown => {
                                let count =
                                    data.diagnostics.len() + usize::from(data.parked.is_some());
                                selected = (selected + 1).min(count.saturating_sub(1));
                                host.select(selected);
                                None
                            }
                            KeyCode::PageUp => {
                                host.scroll(-1.0);
                                None
                            }
                            KeyCode::PageDown => {
                                host.scroll(1.0);
                                None
                            }
                            key if !repeat => {
                                let index = match key {
                                    KeyCode::Key1 => Some(0),
                                    KeyCode::Key2 => Some(1),
                                    KeyCode::Key3 => Some(2),
                                    KeyCode::Key4 => Some(3),
                                    KeyCode::Key5 => Some(4),
                                    KeyCode::Key6 => Some(5),
                                    KeyCode::Key7 => Some(6),
                                    KeyCode::Key8 => Some(7),
                                    KeyCode::Key9 => Some(8),
                                    _ => None,
                                };
                                index.map(Action::Resume)
                            }
                            _ => None,
                        }
                    }
                }
                _ => None,
            };
            // Both input paths use these checks and the existing action worker.
            match action {
                Some(Action::Restart) if update_ready => {
                    enqueue(&mut work_pending, Work::Relaunch(if keyed { "the R key in the update view" } else { "a click on the update view's Restart" }.into()));
                }
                Some(Action::Restart) if data.can_retry || data.can_restart => match retry(failed, keyed) {
                    Ok(()) => failed = false,
                    Err(error) => post(Update::Message(error)),
                },
                Some(Action::Copy) => {
                    let text = data
                        .diagnostics
                        .get(selected)
                        .map(relax_boot::diagnostic_text)
                        .or_else(|| {
                            data.parked
                                .as_ref()
                                .map(|p| format!("{} {}\n{}", p.id, p.location, p.message))
                        });
                    if let Some(text) = text {
                        let mut cmd = CmdWriter::new(HOST);
                        cmd.clip_set(&makepad_hal::clipboard::ClipboardData { text, ..Default::default() }.pack());
                        hal.submit(&cmd.buf());
                    }
                }
                Some(Action::Open) => {
                    if let Some(d) = data
                        .diagnostics
                        .get(selected)
                        .filter(|d| !d.file.is_empty())
                    {
                        enqueue(
                            &mut work_pending,
                            Work::OpenFile {
                                file: d.file.clone(),
                                line: d.line,
                                col: d.col,
                            },
                        );
                    }
                }
                Some(Action::Dismiss) if overlay && !data.parked.as_ref().is_some_and(|p| p.thread == "main") => {
                    overlay_dismissed = true;
                    if std::mem::take(&mut inspect_open) && data.domain.starts_with("JIT · ") {
                        data.domain = "Running".into();
                    }
                    dirty = true;
                }
                Some(Action::Select(index))
                    if index < data.diagnostics.len() + usize::from(data.parked.is_some()) =>
                {
                    selected = index;
                    host.choose(selected);
                }
                Some(Action::Close) => close = true,
                Some(action @ (Action::Minimize | Action::Maximize)) => host.window_control(action),
                Some(Action::Cancel) if return_entry.is_some() => {
                    return_entry = None;
                    host.input(None);
                }
                Some(Action::Submit) if return_entry.is_some() => {
                    let (panic, value) = return_entry.take().unwrap();
                    enqueue(
                        &mut work_pending,
                        Work::Resume {
                            panic,
                            choice: "return".into(),
                            value,
                        },
                    );
                    host.input(None);
                }
                Some(Action::Resume(index)) if return_entry.is_none() => {
                    if let Some(panic) = &data.parked {
                        if let Some(choice) = panic.choices.get(index) {
                            if choice == "return" {
                                return_entry = Some((panic.id.clone(), String::new()));
                                host.input(Some(String::new()));
                            } else {
                                let choice = if choice == "restart" && panic.keeps_state { "restart-state".to_string() } else { choice.clone() };
                                enqueue(
                                    &mut work_pending,
                                    Work::Resume {
                                        panic: panic.id.clone(),
                                        choice,
                                        value: String::new(),
                                    },
                                );
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        if close {
            end_run("the boot window's close button");
            break;
        }
        while let Some(work) = work_pending.pop_front() {
            match send.try_send(work) {
                Ok(()) => {
                    work_full = false;
                }
                Err(std::sync::mpsc::TrySendError::Full(work)) => {
                    if !work_full {
                        eprintln!(
                            "makepad: run action worker busy; retaining {} for a later frame",
                            work.kind()
                        );
                        work_full = true;
                    }
                    work_pending.push_front(work);
                    break;
                }
                Err(std::sync::mpsc::TrySendError::Disconnected(work)) => {
                    work_pending.push_front(work);
                    EXIT.store(101, Ordering::Release);
                    end_run("the run's action worker stopped");
                    break;
                }
            }
        }
    }
    if let Some(probe) = &mut probe {
        probe.finish();
    }
    quit_trace("the window pump quits");
    relax_compiler::run::quitting();
    relax_compiler::inspect::events::run_state_available(false);
    RETRY.notify_all();
    if let Some(ready) = ready {
        let _ = ready.try_send(Err("run closed before its first frame".into()));
    }
    let (domain, window) = state()
        .as_ref()
        .map_or((0, Id::NONE), |r| (r.domain, r.window));
    let mut cmd = CmdWriter::new(HOST);
    if domain != 0 {
        cmd.domain_stop(domain, domain_stop::QUIT);
    }
    if !window.is_none() {
        cmd.window_close(window);
    }
    hal.submit(&cmd.buf());
    // The HAL runs the app's stop on a thread of its own (Hal::run_host_calls) and nobody waited
    // for it: the process exited while the app's threads still ran JIT code and drew, its heap
    // was being torn down and its HAL session freed (crashes at every quit). The run ends after
    // the stop: the app quit, its registered threads and TLS ended, its heap gone.
    // Waited for here, while the main loop still serves the window system.
    if domain != 0 {
        quit_trace("waiting for the app's stop");
        if let Err(e) = relax_compiler::domain::stop(domain, relax_compiler::domain::Stop::Quit) {
            eprintln!("makepad: stopping the app at quit: {e}");
        }
    }
    // The UI has stopped: finish pending persistence/actions before ending this run.
    for work in work_pending {
        let _ = send.send(work);
    }
    drop(send);
    quit_trace("the app stopped; joining the action worker");
    for compile in makepad_script_compute::boundary::kernels_compiling() {
        quit_trace(&format!("still compiling at the app's stop: {compile}"));
    }
    if worker.join().is_err() {
        eprintln!("makepad: run action worker panicked");
    }
}

#[cfg(test)]
mod ring_tip_tests {
    use super::{click_says, dot_click, restart_says};
    use relax_compiler::reload::host::{BuildProgress, Phase, Route};

    /// A restart from the ring's dot says how long the dot had shown, that the press began on
    /// it, and whether a person or the remote endpoint pressed it.
    #[test]
    fn a_click_on_the_dot_says_how_long_it_showed_and_whose_it_was() {
        let shown = std::time::Duration::from_millis(2400);
        assert_eq!(dot_click(false, shown), "a click on the ring's dot (Update ready), pressed on the dot itself 2.4 s after it showed");
        assert_eq!(dot_click(true, shown), "a click the remote endpoint injected on the ring's dot (Update ready), no person's, pressed on the dot itself 2.4 s after it showed");
    }

    #[test]
    fn a_click_that_builds_the_jit_never_reads_as_a_plain_restart() {
        let none = BuildProgress::default();
        assert_eq!(click_says(Route::Plain, &none, ", the app keeps its state"), "click to restart, the app keeps its state");
        for route in [Route::BuildFirst, Route::Building, Route::Built, Route::CannotBuild] {
            assert!(click_says(route, &none, "").contains("JIT"), "{route:?}");
        }
        assert!(click_says(Route::CannotBuild, &none, "").contains("this JIT"));
    }

    #[test]
    fn from_the_click_the_tooltip_says_what_cargo_says() {
        let at = |done, total, building: &str, waiting: &str| BuildProgress { done, total, building: building.into(), waiting: waiting.into() };
        let says = |p: &BuildProgress| restart_says(Phase::Building, 12, p, false, " with its state");
        assert_eq!(says(&BuildProgress::default()), "Building the new JIT · 12 s · then it restarts with its state");
        assert_eq!(says(&at(301, 790, "relax-compiler", "")), "Building the new JIT: 301/790 · 12 s · then it restarts with its state");
        assert!(says(&at(0, 0, "", "build directory")).starts_with("Waiting for another JIT build"));
        assert!(restart_says(Phase::Restarting, 0, &BuildProgress::default(), true, "").contains("unpinned"));
    }
}

#[cfg(test)]
mod crash_offer_tests {
    use super::{end_retry_wait, sources_changed_after_failure, state, wait_for_retry, AfterFailure, Run, NEXT_RUN};
    use makepad_hal::Id;
    use std::sync::atomic::Ordering;

    /// The run state is the process's: one of these tests at a time sets it.
    static ONE: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// A crashed app's sources change (the person's save, or a landing in the checkout): its next
    /// run is offered and waits for the person's Restart, it is not started (a landing restarted a
    /// crashed app on its own). A build that failed before the app started compiles again.
    #[test]
    fn a_crashed_apps_changed_sources_wait_for_its_restart() {
        let _one = ONE.lock().unwrap_or_else(|e| e.into_inner());
        let home = std::env::temp_dir().join(format!("relax-crash-offer-{}", std::process::id()));
        std::fs::create_dir_all(&home).unwrap();
        let log = home.join("live.jsonl");
        std::env::set_var("RELAX_LIVE_LOG", &log);
        let run = |domain, initial_failed| Some(Run { window: Id::NONE, spec: None, domain, initial_failed, retry: false, unnamed: false });
        *state() = run(7, false);
        for _ in 0..2 {
            assert_eq!(sources_changed_after_failure(), Ok(AfterFailure::Offered));
        }
        let why = relax_compiler::reload::CRASHED_SOURCES_CHANGED;
        assert!(relax_compiler::reload::restart_pending().iter().any(|p| p == why), "the restart is offered: {:?}", relax_compiler::reload::restart_pending());
        assert!(!state().as_ref().unwrap().retry, "nothing starts");
        let offers = std::fs::read_to_string(&log).unwrap_or_default().matches("\"event\":\"restart_offered\"").count();
        assert_eq!(offers, 1, "offered once (the WM's toast): every save after it says it again");
        *state() = run(0, true);
        assert_eq!(sources_changed_after_failure(), Ok(AfterFailure::Rebuild));
        assert!(state().as_ref().unwrap().retry, "the failed build compiles again");
        *state() = run(0, false);
        assert!(sources_changed_after_failure().is_err(), "no app has started");
        *state() = None;
        let _ = std::fs::remove_dir_all(&home);
    }

    /// A build that failed before the app started waits for the window's retry, and a restart that
    /// ends the run meanwhile (a `session relaunch`) ends that wait: its compile thread returns, so
    /// the restart's next run starts instead of waiting for that thread forever.
    #[test]
    fn a_restart_ends_a_failed_builds_wait_for_its_retry() {
        let _one = ONE.lock().unwrap_or_else(|e| e.into_inner());
        let run = |retry| Some(Run { window: Id::NONE, spec: None, domain: 0, initial_failed: true, retry, unnamed: false });
        *state() = run(true);
        assert!(wait_for_retry(state()), "the window's retry compiles again");
        assert!(state().as_ref().is_some_and(|r| !r.retry && !r.initial_failed), "the retry is taken");
        *state() = run(false);
        let waiting = std::thread::spawn(|| wait_for_retry(state()));
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(!waiting.is_finished(), "it waits for the retry");
        end_retry_wait();
        assert!(!waiting.join().unwrap(), "the restart's next run is the retry: the build does not compile again in this thread");
        NEXT_RUN.store(false, Ordering::Release);
        *state() = None;
    }
}
