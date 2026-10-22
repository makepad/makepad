//! Responsiveness after the first present: what the program's main (UI) thread waited for in the
//! WINDOW_NS after it -- its lazy compiles (split by where the wall time went and what the owner
//! thread was serving meanwhile), its other owner requests, real-time threads' lazy compiles -- and
//! the gaps between presented frames. A main-thread compile of STALL_LOG_NS or more and a frame
//! gap of FRAME_GAP_NS or more are logged when they happen; the window's sums once it has passed
//! (and in the run summary). Counters only: nothing here waits or takes a lock.

use crate::jit::{wall_ns, MainCompile, FIRST_PRESENT_NS};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};

pub const WINDOW_NS: u64 = 10_000_000_000;
const STALL_LOG_NS: u64 = 5_000_000;
const FRAME_GAP_NS: u64 = 50_000_000;
/// log lines at most (stalls and gaps; the sums count every one)
const LOG_MAX: u64 = 300;

/// the main thread's compiles that began in the window: [count, wall, typeck ahead, owner queue,
/// front, back, finish, in-flight wait, count of STALL_LOG_NS or more] (ns)
static COMPILES: [AtomicU64; 9] = [const { AtomicU64::new(0) }; 9];
/// the main thread's owner requests in the window, its compiles' included: [count, wait ns]
static OWNER: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];
/// real-time threads' lazy compiles in the window: [count, stall ns]
static RT: [AtomicU64; 2] = [const { AtomicU64::new(0) }; 2];
/// frames presented in the window, and its gaps of FRAME_GAP_NS or more: [frames, gaps, gap ns, longest gap ns]
static GAPS: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];
/// wall ns of the main thread's compiles after the present (a frame gap's compile share)
static MAIN_COMPILE_NS: AtomicU64 = AtomicU64::new(0);
static LAST_FRAME: AtomicU64 = AtomicU64::new(0);
static LOGGED: AtomicU64 = AtomicU64::new(0);
/// the window's main-thread compiles by how they were satisfied (jit::MAIN_HOW_NAMES): count, wall ns
static HOW: [[AtomicU64; 2]; 7] = [const { [const { AtomicU64::new(0) }; 2] }; 7];
static SUMMARY_DONE: AtomicBool = AtomicBool::new(false);
/// the owner's service in the window by class (ns), and of it the requests a program thread's
/// request was queued behind when they ended (ns)
static SERVED: [AtomicU64; crate::owner::CLASSES] = [const { AtomicU64::new(0) }; crate::owner::CLASSES];
static SERVED_BLOCKING: [AtomicU64; crate::owner::CLASSES] = [const { AtomicU64::new(0) }; crate::owner::CLASSES];

// ------------------------------------------------------------ readiness (the warm-up veil)

/// UI-thread JIT waits open now (a lazy compile, an owner request; nested ones count once each)
static UI_WAITING: AtomicU64 = AtomicU64::new(0);
/// wall ns the UI thread's current outermost JIT wait began
static UI_WAIT_START: AtomicU64 = AtomicU64::new(0);
/// wall ns the last UI-thread JIT wait ended
static UI_WAIT_END: AtomicU64 = AtomicU64::new(0);
/// ns the UI thread spent in (outermost) JIT waits so far, and that sum at its last frame
static UI_WAIT_SUM: AtomicU64 = AtomicU64::new(0);
static UI_WAIT_SUM_AT_FRAME: AtomicU64 = AtomicU64::new(0);
/// builds ahead (speculative / prepared) finished after the first present, and running now
pub(crate) static AHEAD_DONE: AtomicU64 = AtomicU64::new(0);
pub(crate) static AHEAD_RUNNING: AtomicU64 = AtomicU64::new(0);

/// The UI (main) thread begins / ends a wait on the JIT (jit.rs lazy compiles, owner.rs requests).
pub(crate) fn ui_wait_begin() {
    let outer = UI_WAITING.load(Relaxed) == 0;
    if outer {
        UI_WAIT_START.store(wall_ns(), Relaxed);
    }
    UI_WAITING.fetch_add(1, Relaxed);
    if outer && present_ns() != 0 {
        let hook = UI_WAIT_HOOK.load(Relaxed);
        if !hook.is_null() {
            // SAFETY: only `on_ui_wait` stores here, always a `fn()`
            unsafe { std::mem::transmute::<*mut (), fn()>(hook)() }
        }
    }
}

static UI_WAIT_HOOK: std::sync::atomic::AtomicPtr<()> = std::sync::atomic::AtomicPtr::new(std::ptr::null_mut());

/// `hook` runs on the UI thread when it begins a JIT wait after the first present (the outermost
/// one): the host's window wakes to show the wait's activity once it lasts. It must not wait.
pub fn on_ui_wait(hook: fn()) {
    UI_WAIT_HOOK.store(hook as *mut (), Relaxed);
}
pub(crate) fn ui_wait_end() {
    let now = wall_ns();
    UI_WAIT_END.store(now, Relaxed);
    if UI_WAITING.fetch_sub(1, Relaxed) == 1 {
        UI_WAIT_SUM.fetch_add(now.saturating_sub(UI_WAIT_START.load(Relaxed)), Relaxed);
    }
}

/// How long the UI thread's open JIT wait has lasted (0: none open, or before the first present):
/// the whole UI stalls on the JIT, no frame. Atomics only.
pub fn ui_stall_ns() -> u64 {
    if present_ns() == 0 || UI_WAITING.load(Relaxed) == 0 {
        return 0;
    }
    wall_ns().saturating_sub(UI_WAIT_START.load(Relaxed))
}

/// How long the UI has been hanging on the JIT: the time since its last frame (or the first present)
/// when the UI thread spent at least half of it in JIT waits (one long lazy compile, or a level
/// load's thousands of short ones back to back: no frame either way); 0 otherwise. Atomics only.
pub fn ui_hang_ns() -> u64 {
    let fp = present_ns();
    if fp == 0 {
        return 0;
    }
    let now = wall_ns();
    let last = LAST_FRAME.load(Relaxed).max(fp);
    let span = now.saturating_sub(last);
    let mut waited = UI_WAIT_SUM.load(Relaxed).saturating_sub(UI_WAIT_SUM_AT_FRAME.load(Relaxed));
    if UI_WAITING.load(Relaxed) > 0 {
        waited += now.saturating_sub(UI_WAIT_START.load(Relaxed).max(last));
    }
    if span > 0 && waited.saturating_mul(2) >= span { span } else { 0 }
}

/// ns since the UI thread last waited on the JIT: 0 while it waits, u64::MAX before the first
/// present. Atomics only.
pub fn ui_idle_ns() -> u64 {
    let fp = present_ns();
    if fp == 0 {
        return u64::MAX;
    }
    if UI_WAITING.load(Relaxed) > 0 {
        return 0;
    }
    wall_ns().saturating_sub(UI_WAIT_END.load(Relaxed).max(fp))
}

/// Functions built ahead since the first present, and those still queued or building (the
/// speculative / prepare queue; a queue being changed right now reads as 1 more).
pub fn ahead_progress() -> (u64, u64) {
    (AHEAD_DONE.load(Relaxed), crate::jit::ahead_queued() + AHEAD_RUNNING.load(Relaxed))
}

/// The JIT's work in flight, for the run's activity badge (a look, not a measurement).
#[derive(Clone, Copy, Debug, Default)]
pub struct Activity {
    /// O3 builds queued or running now
    pub o3: u64,
    /// functions queued or building ahead now
    pub ahead: u64,
    /// ns the UI thread has spent in JIT waits so far, its open one included (monotonic: the
    /// difference between two looks over their interval is its JIT duty cycle)
    pub ui_wait_ns: u64,
}

/// What the JIT is doing now (one lock of the O3 queue; the rest atomics).
pub fn activity() -> Activity {
    let mut ui_wait_ns = UI_WAIT_SUM.load(Relaxed);
    if UI_WAITING.load(Relaxed) > 0 {
        ui_wait_ns += wall_ns().saturating_sub(UI_WAIT_START.load(Relaxed));
    }
    Activity { o3: crate::jit::o3_pending(), ahead: ahead_progress().1, ui_wait_ns }
}

/// The app is ready after its first present (false before it): the UI thread has not waited on the JIT for
/// `quiet_ns` and nothing is left to build ahead.
pub fn ready(quiet_ns: u64) -> bool {
    present_ns() != 0 && ui_idle_ns() >= quiet_ns && ahead_progress().1 == 0
}

fn present_ns() -> u64 {
    FIRST_PRESENT_NS.load(Relaxed)
}

fn in_window(t: u64) -> bool {
    let fp = present_ns();
    fp != 0 && t >= fp && t < fp + WINDOW_NS
}

fn may_log() -> bool {
    LOGGED.fetch_add(1, Relaxed) < if owner_long_all() { 20 * LOG_MAX } else { LOG_MAX }
}

/// MAKEPAD_RUN_OWNER_LONG=1: every long owner request of the run is logged, not only the first
/// windows' (diagnostics of what the UI's requests queued behind)
fn owner_long_all() -> bool {
    static ALL: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ALL.get_or_init(|| std::env::var_os("MAKEPAD_RUN_OWNER_LONG").is_some())
}

fn ms(ns: u64) -> f64 {
    ns as f64 / 1e6
}

/// MAKEPAD_RUN_POSTFRAME_LOG=<path>: every main-thread lazy compile after the first present, one
/// line each as it ends (appended): ms after the present, wall ms, how, typeck ahead, owner queue,
/// front, back, finish, in-flight wait (ms), the slot's key (its instance; the fn name when no
/// front ran). A diagnostic: the key is made on the owner thread in the front's request.
pub(crate) fn log_on() -> bool {
    LOG.get_or_init(|| std::env::var_os("MAKEPAD_RUN_POSTFRAME_LOG").and_then(|p| crate::zone_alloc::kept_with(|| std::fs::OpenOptions::new().create(true).append(true).open(p).ok().map(std::sync::Mutex::new)))).is_some()
}
static LOG: std::sync::OnceLock<Option<std::sync::Mutex<std::fs::File>>> = std::sync::OnceLock::new();
/// the key of the main thread's compile the owner ran a front for (log_on only)
pub(crate) static LOG_KEY: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

/// A main-thread lazy compile ended (jit::compile_slot_on_stack_rt): `name` is its fn.
pub(crate) fn main_compile(name: &str, r: &MainCompile) {
    let fp = present_ns();
    let key = if log_on() { std::mem::take(&mut *LOG_KEY.lock().unwrap()) } else { String::new() };
    if fp == 0 || r.start < fp || crate::jit::QUIT_SEEN.load(Relaxed) {
        return;
    }
    let dt = r.end.saturating_sub(r.start);
    if let Some(Some(f)) = LOG.get() {
        use std::io::Write;
        let how = crate::jit::MAIN_HOW_NAMES.get(r.how as usize).copied().unwrap_or("?");
        let line = crate::zone_alloc::kept_with(|| format!("{:.3} {:.3} {} {:.3} {:.3} {:.3} {:.3} {:.3} {:.3} {}\n", ms(r.start - fp), ms(dt), how, ms(r.ns[0]), ms(r.ns[1]), ms(r.ns[2]), ms(r.ns[3]), ms(r.ns[4]), ms(r.ns[5]), if key.is_empty() { name } else { &key }));
        let _ = f.lock().unwrap().write_all(line.as_bytes());
    }
    MAIN_COMPILE_NS.fetch_add(dt, Relaxed);
    if in_window(r.start) {
        COMPILES[0].fetch_add(1, Relaxed);
        COMPILES[1].fetch_add(dt, Relaxed);
        for (a, &ns) in r.ns.iter().enumerate() {
            COMPILES[a + 2].fetch_add(ns, Relaxed);
        }
        if dt >= STALL_LOG_NS {
            COMPILES[8].fetch_add(1, Relaxed);
        }
        if let Some(h) = HOW.get(r.how as usize) {
            h[0].fetch_add(1, Relaxed);
            h[1].fetch_add(dt, Relaxed);
        }
    }
    if dt >= STALL_LOG_NS && may_log() {
        let behind = match r.behind {
            0 => "-".to_string(),
            c => format!("{} for `{}` from {}", crate::owner::CLASS_NAMES[(c - 1) as usize], crate::os::thread::name(r.behind_from), r.behind_site.map_or("?".to_string(), |l| format!("{}:{}", l.file().rsplit('/').next().unwrap_or(""), l.line()))),
        };
        eprintln!(
            "makepad-run main stall: `{}` {:.1} ms at +{:.2} s after the first present (owner queue {:.1} ms, longest behind {} {:.1} ms; typeck ahead {:.1}, front {:.1}, back {:.1}, finish {:.1}, in-flight wait {:.1} ms; at its start {} compile jobs ran, queued: speculative {}, O2 {}, O3 {})",
            name,
            ms(dt),
            (r.start - fp) as f64 / 1e9,
            ms(r.ns[1]),
            behind,
            ms(r.behind_ns),
            ms(r.ns[0]),
            ms(r.ns[2]),
            ms(r.ns[3]),
            ms(r.ns[4]),
            ms(r.ns[5]),
            r.jobs,
            r.queued[0],
            r.queued[1],
            r.queued[2]
        );
    }
    summary_due(r.end);
}

/// A main-thread owner request ended after the present (owner::request): waited `w` ns from `t0`.
pub(crate) fn main_owner_wait(t0: u64, w: u64) {
    if in_window(t0) {
        OWNER[0].fetch_add(1, Relaxed);
        OWNER[1].fetch_add(w, Relaxed);
    }
}

/// The owner served a request of class `class` asked at `site` by thread `from` from `t0` to `t1`;
/// `foreground` program-thread requests were queued at its end. Nested requests count in their
/// outer one too.
pub(crate) fn owner_served(class: usize, site: &std::panic::Location<'_>, from: u64, t0: u64, t1: u64, foreground: u32) {
    let fp = present_ns();
    if fp == 0 || t0 < fp || crate::jit::QUIT_SEEN.load(Relaxed) {
        return;
    }
    let dt = t1.saturating_sub(t0);
    if in_window(t0) {
        SERVED[class].fetch_add(dt, Relaxed);
        if foreground > 0 {
            SERVED_BLOCKING[class].fetch_add(dt, Relaxed);
        }
    }
    // MAKEPAD_RUN_OWNER_LONG=1: every long request of the run, not only the first windows'
    let all = owner_long_all();
    if dt >= STALL_LOG_NS && (all || t0 < fp + 3 * WINDOW_NS) && may_log() {
        eprintln!(
            "makepad-run owner long: {} request of `{}` from {}:{} served {:.1} ms at +{:.2} s ({} program-thread requests queued at its end)",
            crate::owner::CLASS_NAMES[class],
            crate::os::thread::name(from),
            site.file().rsplit('/').next().unwrap_or(""),
            site.line(),
            ms(dt),
            (t0 - fp) as f64 / 1e9,
            foreground
        );
    }
}

/// A real-time thread's lazy compile began at `t0` and stalled it `dt` ns.
pub(crate) fn rt_compile(t0: u64, dt: u64) {
    if in_window(t0) {
        RT[0].fetch_add(1, Relaxed);
        RT[1].fetch_add(dt, Relaxed);
    }
}

/// The program presented a frame (makepad-platform calls the loader symbol `relax_frame_done`
/// after each frame the HAL finished; on the thread that handles the HAL's events).
pub(crate) extern "C" fn relax_frame_done() {
    let now = wall_ns();
    let fp = present_ns();
    let last = LAST_FRAME.swap(now, Relaxed);
    UI_WAIT_SUM_AT_FRAME.store(UI_WAIT_SUM.load(Relaxed), Relaxed);
    let compiled = MAIN_COMPILE_NS.load(Relaxed);
    static COMPILED_AT_LAST: AtomicU64 = AtomicU64::new(0);
    let compiled_before = COMPILED_AT_LAST.swap(compiled, Relaxed);
    if fp == 0 || last < fp || crate::jit::QUIT_SEEN.load(Relaxed) {
        return;
    }
    let gap = now - last;
    if in_window(now) {
        GAPS[0].fetch_add(1, Relaxed);
        if gap >= FRAME_GAP_NS {
            GAPS[1].fetch_add(1, Relaxed);
            GAPS[2].fetch_add(gap, Relaxed);
            GAPS[3].fetch_max(gap, Relaxed);
        }
    }
    if gap >= FRAME_GAP_NS && now < fp + 3 * WINDOW_NS && may_log() {
        eprintln!(
            "makepad-run frame gap: {:.0} ms ending at +{:.2} s after the first present ({:.0} ms of it main-thread lazy compiles)",
            ms(gap),
            (now - fp) as f64 / 1e9,
            ms(compiled.saturating_sub(compiled_before))
        );
    }
    summary_due(now);
}

fn summary_due(now: u64) {
    let fp = present_ns();
    if fp != 0 && now >= fp + WINDOW_NS && !SUMMARY_DONE.swap(true, Relaxed) {
        eprintln!("{}", summary());
    }
}

fn served(a: &[AtomicU64; crate::owner::CLASSES]) -> String {
    (0..crate::owner::CLASSES).map(|c| format!("{} {:.0}", crate::owner::CLASS_NAMES[c], ms(a[c].load(Relaxed)))).collect::<Vec<_>>().join(", ")
}

/// A burst of the ahead helpers ended (jit::ahead_helper): their work since the present, when it
/// changed since the last report.
pub(crate) fn ahead_over() {
    static LAST: AtomicU64 = AtomicU64::new(u64::MAX);
    static LAST_AT: AtomicU64 = AtomicU64::new(0);
    let fp = present_ns();
    let n = crate::jit::AHEAD_STATS[0].load(Relaxed);
    let now = wall_ns();
    // (at most one line per 5 s)
    if fp == 0 || crate::jit::QUIT_SEEN.load(Relaxed) || LAST.load(Relaxed) == n || now < LAST_AT.load(Relaxed) + 5_000_000_000 {
        return;
    }
    LAST.store(n, Relaxed);
    LAST_AT.store(now, Relaxed);
    eprintln!("{} (a burst ended at +{:.1} s)", ahead_line(), (wall_ns() - fp) as f64 / 1e9);
}

fn ahead_line() -> String {
    let ah = |i: usize| crate::jit::AHEAD_STATS[i].load(Relaxed).saturating_sub(crate::jit::AHEAD_AT_PRESENT[i].load(Relaxed));
    format!("makepad-run post-present ahead since the present: {} bodies checked ({} taken by builds, {} dropped untaken at burst ends, {} late: their build had begun), cpu off the UI thread {:.0} ms, on it {:.0} ms; {} lowered ({} taken), cpu {:.0} ms; {}",
        ah(0), ah(1), crate::jit::AHEAD_DROPPED.load(Relaxed), crate::jit::AHEAD_LATE.load(Relaxed), ms(ah(4)), ms(ah(3)), ah(5), ah(6), ms(ah(7)), crate::jit::bmemo::line())
}

/// The window's sums (one line).
pub fn summary() -> String {
    let c = |i: usize| COMPILES[i].load(Relaxed);
    let g = |i: usize| GAPS[i].load(Relaxed);
    format!(
        "{}\nmakepad-run post-present {:.0} s: main thread stalled {:.1} ms in {} lazy compiles ({} of {:.0} ms or more; owner queue {:.1}, typeck ahead {:.1}, front {:.1}, back {:.1}, finish {:.1}, in-flight wait {:.1} ms; by how: {}), {} owner requests waited {:.1} ms in all; real-time threads stalled {:.1} ms in {} lazy compiles; {} frames, {} gaps of {:.0} ms or more ({:.0} ms, longest {:.0} ms); owner served ms {} (with program-thread requests queued at the end: {})",
        ahead_line(),
        WINDOW_NS as f64 / 1e9,
        ms(c(1)),
        c(0),
        c(8),
        ms(STALL_LOG_NS),
        ms(c(3)),
        ms(c(2)),
        ms(c(4)),
        ms(c(5)),
        ms(c(6)),
        ms(c(7)),
        (0..7).map(|k| format!("{} {} ({:.0} ms)", crate::jit::MAIN_HOW_NAMES[k], HOW[k][0].load(Relaxed), ms(HOW[k][1].load(Relaxed)))).collect::<Vec<_>>().join(", "),
        OWNER[0].load(Relaxed),
        ms(OWNER[1].load(Relaxed)),
        ms(RT[1].load(Relaxed)),
        RT[0].load(Relaxed),
        g(0),
        g(1),
        ms(FRAME_GAP_NS),
        ms(g(2)),
        ms(g(3)),
        served(&SERVED),
        served(&SERVED_BLOCKING)
    )
}
