//! Measured first-frame deadlines for runs whose load screen promises a window.
use crate::inspect::snapshot;
use makepad_remote::hot::{Outcome, Record};
use makepad_strict_json::Value;
use std::io::{Read, Seek, Write};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering::{Acquire, Release}};
use std::time::Duration;

pub(crate) static THREAD: OnceLock<std::thread::Thread> = OnceLock::new();

/// A measured first frame: package, state (warm / cold), ms, and the codegen identity (hex) of
/// the JIT build that ran it (`jit_id`)
type Sample = (String, String, u64, String);

fn samples(text: &str) -> Vec<Sample> {
    text.lines().filter_map(|line| {
        let mut words = line.split_whitespace();
        let (package, state, ms, jit) = (words.next()?, words.next()?, words.next()?, words.next()?);
        if !matches!(state, "warm" | "cold") || words.next().is_some() { return None; }
        Some((package.into(), state.into(), ms.parse().ok()?, jit.into()))
    }).collect()
}

/// This JIT build: its codegen identity, what decides whether stored code links (codegen_id.rs).
/// A warm start is one of a build that has code stored: another build's warm runs say nothing of it.
fn jit_id() -> String { format!("{:032x}", crate::codegen_id::id()) }

fn history() -> std::path::PathBuf { crate::gitstore::relax_home().join("first-frame.txt") }

fn measured() -> std::io::Result<Vec<Sample>> {
    let mut file = match std::fs::File::open(history()) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    file.lock_shared()?;
    let mut text = String::new();
    file.read_to_string(&mut text)?;
    Ok(samples(&text))
}

fn remember(package: &str, state: &str, ms: u64) -> std::io::Result<()> {
    std::fs::create_dir_all(crate::gitstore::relax_home())?;
    // Lock the file through the rewrite so concurrent app launches keep each other's samples.
    let mut file = std::fs::OpenOptions::new().create(true).truncate(false).read(true).write(true).open(history())?;
    file.lock()?;
    let mut text = String::new();
    file.read_to_string(&mut text)?;
    let jit = jit_id();
    let mut rows = samples(&text);
    rows.push((package.into(), state.into(), ms, jit.clone()));
    let mut counts = std::collections::HashMap::new();
    let mut keep = Vec::new();
    // the last 9 of each package and state; warm runs of this build only
    for row in rows.into_iter().rev() {
        if row.1 == "warm" && row.3 != jit { continue; }
        let n = counts.entry((row.0.clone(), row.1.clone())).or_insert(0);
        *n += 1;
        if *n <= 9 { keep.push(row); }
    }
    let mut text = String::new();
    for (package, state, ms, jit) in keep.into_iter().rev() { text.push_str(&format!("{package} {state} {ms} {jit}\n")); }
    file.rewind()?;
    file.write_all(text.as_bytes())?;
    // no barrier: one forces a journal commit of the whole filesystem, which on a busy disk held
    // the process's other file operations behind it; a row a crash loses is one measurement
    file.set_len(text.len() as u64)
}

/// The deadline of a start in `state` (warm / cold): 3x the median of its measured first frames,
/// at least the state's floor; and its basis in words.
fn deadline(package: &str, state: &str) -> (u64, String) {
    let rows = measured().unwrap_or_else(|e| {
        eprintln!("relax: cannot read first-frame history {}: {e}", history().display());
        Vec::new()
    });
    deadline_of(&rows, package, state, &jit_id())
}

/// `deadline` over measured `rows` for the JIT build `jit`: a warm start counts this build's warm
/// runs; with none (a new JIT build relaunched onto its stored code: what links, and what it
/// compiles lazily, is unmeasured) it is judged as a cold start.
fn deadline_of(rows: &[Sample], package: &str, state: &str, jit: &str) -> (u64, String) {
    let mut runs: Vec<u64> = rows.iter().filter(|s| s.0 == package && s.1 == state && (state == "cold" || s.3 == jit)).map(|s| s.2).collect();
    if state == "warm" && runs.is_empty() {
        let (secs, basis) = deadline_of(rows, package, "cold", jit);
        return (secs, format!("warm, but no measured warm start of this JIT build: {basis}"));
    }
    let floor = if state == "warm" { 30u64 } else { 180 };
    runs.sort_unstable();
    let median = if runs.is_empty() { None } else {
        let (a, b) = (runs[(runs.len() - 1) / 2], runs[runs.len() / 2]);
        Some(a + (b - a) / 2)
    };
    let secs = median.map_or(floor, |ms| ms.saturating_mul(3).saturating_add(999) / 1000).max(floor);
    let basis = match median {
        Some(ms) => format!("{state}: 3x median {:.1} s of {} runs, floor {floor} s", ms as f64 / 1000.0, runs.len()),
        None => format!("{state}: no measured runs, floor {floor} s"),
    };
    (secs, basis)
}

/// Past its deadline a start that is still compiling is not hung: the timeout is for hangs, not
/// slow compiles (a relaunch onto a rebuilt JIT compiled lazily past a warm deadline and was
/// killed, 11-07). Still compiling: a demand compile finished in the last 2 s, or one runs and
/// began (or the last finished) less than a minute ago -- one compile running longer is a hang.
fn still_compiling(running: bool, age_ns: u64) -> bool {
    age_ns < if running { 60_000_000_000 } else { 2_000_000_000 }
}

fn compiling() -> bool {
    let (running, age) = crate::jit::demand_compile_age();
    still_compiling(running, age)
}

/// Past its deadline a start is hung only once it made no progress for this long: its deadline
/// is when the watch starts to look, never a kill by itself.
const STALL_WINDOW_MS: f64 = 10_000.0;
/// The process's CPU over the window that counts as work: half a core. A GPU driver compiling
/// changed shaders ran 0.9 of a core in its shader compiler beside the JIT's tier-up at 0.5,
/// and a start waiting on nothing (its threads parked, its main loop waiting for events) runs
/// a few percent.
const WORKING_CORES: f64 = 0.5;

/// What the process has done, read at one look: its CPU time (all its threads, a GPU driver's
/// shader compiler among them) and the JIT's finished compiles and installed builds. Not the bytes
/// it wrote: the run's own writers (its log, its flight recorder's recordings of the waits) write
/// while a start hangs, and would count as its work.
#[derive(Clone, Copy, Debug)]
struct Work {
    cpu_ns: u64,
    compiles: u64,
}

fn work() -> Work {
    Work { cpu_ns: process_cpu_ns(), compiles: crate::jit::progress_count(crate::jit::PROGRESS_JIT) }
}

/// The process's CPU time, every thread's (ns).
#[cfg(unix)]
fn process_cpu_ns() -> u64 {
    #[repr(C)]
    struct Timespec {
        sec: i64,
        nsec: i64,
    }
    extern "C" {
        fn clock_gettime(clk: i32, ts: *mut Timespec) -> i32;
    }
    #[cfg(any(target_os = "linux", target_os = "android"))]
    const CLOCK_PROCESS_CPUTIME_ID: i32 = 2;
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    const CLOCK_PROCESS_CPUTIME_ID: i32 = 12;
    let mut ts = Timespec { sec: 0, nsec: 0 };
    if unsafe { clock_gettime(CLOCK_PROCESS_CPUTIME_ID, &mut ts) } != 0 { return 0; }
    ts.sec as u64 * 1_000_000_000 + ts.nsec as u64
}

#[cfg(windows)]
fn process_cpu_ns() -> u64 {
    use makepad_windows_sys::Win32::{Foundation::FILETIME, System::Threading::{GetCurrentProcess, GetProcessTimes}};
    let (mut created, mut exited, mut kernel, mut user) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
    if unsafe { GetProcessTimes(GetCurrentProcess(), &mut created, &mut exited, &mut kernel, &mut user) }.is_err() { return 0; }
    let ticks = |t: &FILETIME| (t.dwHighDateTime as u64) << 32 | t.dwLowDateTime as u64;
    (ticks(&kernel) + ticks(&user)).saturating_mul(100)
}

/// One look at a start past its deadline ([`Overrun::look`]).
#[derive(Debug, PartialEq)]
enum Look {
    /// look again: the window has not passed, or the start works and said so before
    Wait,
    /// it works: what it did over the window, said once
    Working(String),
    /// no progress over the window: what it did
    Hung(String),
}

/// A start past its first-frame deadline, judged by its progress, never by the clock: each
/// [`STALL_WINDOW_MS`] it is hung only when the window saw no compile finish and under
/// [`WORKING_CORES`] of CPU. Two live sessions' first frames came late while the GPU driver
/// recompiled changed shaders, and the 30 s warm deadline killed both (their reruns passed, 11-18).
struct Overrun {
    since_ms: f64,
    at: Work,
    said: bool,
}

impl Overrun {
    fn new(now_ms: f64, at: Work) -> Overrun {
        Overrun { since_ms: now_ms, at, said: false }
    }

    fn look(&mut self, now_ms: f64, now: Work) -> Look {
        let span = now_ms - self.since_ms;
        if span < STALL_WINDOW_MS { return Look::Wait; }
        let cores = now.cpu_ns.saturating_sub(self.at.cpu_ns) as f64 / 1e6 / span;
        let compiles = now.compiles.saturating_sub(self.at.compiles);
        let what = format!("{:.0}% of a core, {compiles} compile(s) finished", cores * 100.0);
        (self.since_ms, self.at) = (now_ms, now);
        if cores < WORKING_CORES && compiles == 0 {
            return Look::Hung(format!("no progress in its last {:.0} s ({what})", span / 1000.0));
        }
        if self.said { return Look::Wait; }
        self.said = true;
        Look::Working(format!("it still works ({what} in its last {:.0} s): no timeout while it does", span / 1000.0))
    }
}

/// A start counted warm (its cache holds a startup manifest) that compiled most of its code
/// fresh instead of linking it: an edit reached most of the program (a field of a type almost
/// every fn touches), or the entries are gone. It runs as a cold start does and is judged as one.
/// (slots that took cached code: placed by the startup image or linked by a lookup; lookups that
/// missed)
fn links() -> (u64, u64) {
    use crate::cache::warm_count::*;
    (get(IMG_PLACED) + get(HIT), get(NO_ENTRY) + get(INVALIDATED) + get(LINK_FAIL))
}

fn cold_like() -> bool {
    let (linked, miss) = links();
    miss > linked
}

/// When the program's main started (`wall_ns`).
static MAIN_ENTERED_NS: OnceLock<u64> = OnceLock::new();
/// When the program entered `app_main!`'s entry (ms since the process started).
static APP_STARTED_MS: OnceLock<f64> = OnceLock::new();
/// The app domain ended (`app_ended`).
static APP_ENDED: AtomicBool = AtomicBool::new(false);

/// The app domain's main starts now: the quiet of a program with no app counts from here, as the
/// run's boot before it is no part of the program's own time.
pub(crate) fn main_entered() {
    let _ = MAIN_ENTERED_NS.set(crate::jit::wall_ns());
    if let Some(t) = THREAD.get() { t.unpark(); }
}

/// The program entered `app_main!`'s entry: it is an app, and its first frame's deadline runs from
/// here (`watch`).
pub(crate) fn app_started() {
    let _ = APP_STARTED_MS.set(crate::clock::ms_since_process_start());
    if let Some(t) = THREAD.get() { t.unpark(); }
}

/// The program presented its first frame (`relax_first_present`): it is an app.
pub(crate) fn presented() {
    if let Some(t) = THREAD.get() { t.unpark(); }
}

/// The app domain ended (domain.rs `end`: its main returned, it exited or it crashed): what it
/// was is settled, and its run ends. The quiet after it is its exit's, never a batch program's:
/// a short program whose exit ran past the quiet was taken for one, and the tier-up, lowering
/// ahead and coherence check that wakes ran in its exit (rwbench: 1.8 s more, and 2075 compiles
/// RELAX_WARM_STRICT counted as warm misses, 11-14).
pub(crate) fn app_ended() {
    APP_ENDED.store(true, Release);
    if let Some(t) = THREAD.get() { t.unpark(); }
}

/// The run ends, or its app did: the watch has nothing left to decide.
fn gone() -> bool {
    crate::run_threads::ending() || APP_ENDED.load(Acquire)
}

/// Until the program shows what it is: an app (`app_started`, the first thing `app_main!`'s main
/// reaches: lazy compiles on the way there keep the quiet from ending; or a frame it presented) or
/// a batch program, whose main goes quiet without either. A batch program draws no frame: no
/// deadline, no load screen. Should it start an app after all, the app has its deadline (and makes
/// its own window). A program that presented is never taken for one: an app whose own main skipped
/// `app_started` lost the window it drew in when its compiles went quiet (an app, 11-06).
fn watch_app() -> f64 {
    let mut batch = false;
    loop {
        if let Some(ms) = APP_STARTED_MS.get() { return *ms; }
        if crate::jit::presented() { return crate::clock::ms_since_process_start(); }
        if gone() { return 0.0; }
        match MAIN_ENTERED_NS.get().filter(|_| !batch).map(|at| crate::jit::quiet_left(*at)) {
            None => std::thread::park(),
            Some(0) => {
                batch = true;
                if crate::run::VERBOSE.load(std::sync::atomic::Ordering::Relaxed) {
                    eprintln!("relax: main went quiet without an app: a batch program, no first frame");
                }
                crate::jit::batch_program();
                crate::run::batch_program();
            }
            Some(left) => std::thread::park_timeout(Duration::from_nanos(left.max(50_000_000))),
        }
    }
}

/// The first frame's watch of a program that links the platform (`watch_app`).
pub(crate) fn arm(package: &str) {
    let _kept = crate::zone_alloc::kept();
    let state = if std::env::var("RELAX_CACHE_STATE").as_deref() == Ok("warm") { "warm" } else { "cold" };
    let package = package.to_string();
    if let Err(e) = crate::run_threads::spawn("first-frame-watch", move || {
        let _kept = crate::zone_alloc::kept();
        let _ = THREAD.set(std::thread::current());
        crate::run_threads::on_end(|| if let Some(t) = THREAD.get() { t.unpark(); });
        let started = watch_app();
        if gone() { return; }
        let (secs, basis) = deadline(&package, state);
        if !crate::run::silent() { eprintln!("relax: first-frame timeout {secs} s ({basis})"); }
        watch(&package, state, started, secs, &basis);
    }) {
        eprintln!("relax: cannot arm first-frame timeout: {e}");
        crate::run::failed_run(&format!("cannot arm the first-frame timeout: {e}"));
    }
}

/// The cache's state once the load classified it (run.rs): 0 not yet (or no cache), 1 cold, 2 warm.
static CACHE_STATE: AtomicU8 = AtomicU8::new(0);
/// The boot window has opened: its first present happened (or the run draws none of its own).
static WINDOW_OPEN: AtomicBool = AtomicBool::new(false);
static WINDOW_THREAD: OnceLock<std::thread::Thread> = OnceLock::new();

/// The load classified the cache (`warm`: it holds this program's startup manifest).
pub(crate) fn classified(warm: bool) {
    CACHE_STATE.store(if warm { 2 } else { 1 }, Release);
    if let Some(t) = WINDOW_THREAD.get() { t.unpark(); }
}

/// The boot window's first present happened (its open returned).
pub(crate) fn window_opened() {
    WINDOW_OPEN.store(true, Release);
    if let Some(t) = WINDOW_THREAD.get() { t.unpark(); }
}

/// From before the boot window opens: the run's start waits for the window's first present
/// (finish_window), so a window that never presents holds the app forever. Its first present has
/// the first frame's deadline (`deadline`: cold until the load classifies its cache); past it,
/// the run is captured, reported and failed as a first-frame timeout.
pub(crate) fn arm_window(package: &str) {
    let _kept = crate::zone_alloc::kept();
    let package = package.to_string();
    if let Err(e) = crate::run_threads::spawn("boot-window-watch", move || {
        let _kept = crate::zone_alloc::kept();
        let _ = WINDOW_THREAD.set(std::thread::current());
        watch_window(&package);
    }) {
        eprintln!("relax: cannot arm the boot window's first-frame timeout: {e}");
        crate::run::failed_run(&format!("cannot arm the boot window's first-frame timeout: {e}"));
    }
}

fn watch_window(package: &str) {
    let mut said = false;
    let mut overrun: Option<Overrun> = None;
    let (secs, basis, hung) = loop {
        if WINDOW_OPEN.load(Acquire) || crate::run_threads::ending() { return; }
        // a warm start that compiles most of its code fresh is judged as a cold one (`watch`)
        let state = if CACHE_STATE.load(Acquire) == 2 && !cold_like() { "warm" } else { "cold" };
        let (secs, basis) = deadline(package, state);
        let left = secs as f64 * 1000.0 - crate::clock::ms_since_process_start();
        if left <= 0.0 {
            if compiling() {
                overrun = None;
                if !said {
                    said = true;
                    if !crate::run::silent() { eprintln!("relax: first-frame: {secs} s passed ({basis}) and the JIT is still compiling: no timeout while it compiles"); }
                }
            } else {
                let now = crate::clock::ms_since_process_start();
                match overrun.get_or_insert_with(|| Overrun::new(now, work())).look(now, work()) {
                    Look::Hung(why) => break (secs, basis, why),
                    Look::Working(what) => if !crate::run::silent() { eprintln!("relax: first-frame: {secs} s passed ({basis}) and {what}"); },
                    Look::Wait => {}
                }
            }
            std::thread::park_timeout(Duration::from_millis(500));
            continue;
        }
        // woken by the window's open and the cache's classification; a warm start's
        // compile-or-link verdict is looked at again each second
        std::thread::park_timeout(Duration::from_secs_f64((left / 1000.0).min(1.0)));
    };
    if WINDOW_OPEN.load(Acquire) { return; }
    let s = snapshot::capture(&format!("first-frame timeout after {secs} s ({basis}), then {hung}: the boot window has not presented"));
    eprintln!("relax: first-frame timeout: {package}'s boot window presented no frame in {secs} s ({basis}), then {hung}");
    eprintln!("relax: likely cause: {}", s.cause);
    eprintln!("relax: report: {}", s.path.display());
    let reason = format!("{} [report {}]", s.cause, s.path.display());
    persist("ff", Outcome::FirstFrameTimeout, package, &reason);
    crate::run::failed_run(&format!("first-frame timeout: {}", s.cause));
}

fn watch(package: &str, state: &str, started: f64, secs: u64, basis: &str) {
    let (mut state, mut secs, mut basis) = (state, secs, basis.to_string());
    let mut deadline_ms = started + secs as f64 * 1000.0;
    let mut wm = false;
    let mut said = false;
    let mut overrun: Option<Overrun> = None;
    let hung = loop {
        if crate::run_threads::ending() { return; }
        if let Some(ms) = snapshot::first_present_ms().filter(|ms| *ms <= deadline_ms) {
            let shell = snapshot::hal_shell();
            wm |= shell.as_ref().is_some_and(|s| s.get("backend").and_then(Value::as_str) == Some("wm"));
            let shown = shell.as_ref().is_some_and(|s| s.get("app_frame_shown").and_then(Value::as_bool) == Some(true));
            if !wm || shown {
                // a warm start that compiled its code fresh is a cold start's sample
                let state = if state == "warm" && cold_like() { "cold" } else { state };
                if let Err(e) = remember(package, state, ms.round() as u64) {
                    eprintln!("relax: cannot write first-frame history {}: {e}", history().display());
                }
                return;
            }
        }
        let mut left = deadline_ms - crate::clock::ms_since_process_start();
        if left <= 0.0 && state == "warm" && cold_like() {
            // judged by what the start does: it compiles what a warm one links
            let (linked, miss) = links();
            (state, (secs, basis)) = ("cold", deadline(package, "cold"));
            deadline_ms = started + secs as f64 * 1000.0;
            left = deadline_ms - crate::clock::ms_since_process_start();
            if !crate::run::silent() {
                eprintln!("relax: first-frame: the warm start took cached code for {linked} slots and missed {miss} lookups (an edit reached most of the program, or its entries are gone): it starts cold, timeout {secs} s ({basis})");
            }
        }
        if left <= 0.0 {
            let now = crate::clock::ms_since_process_start();
            if compiling() {
                overrun = None;
                if !said {
                    said = true;
                    if !crate::run::silent() { eprintln!("relax: first-frame: {secs} s passed ({basis}) and the JIT is still compiling: no timeout while it compiles"); }
                }
            } else {
                match overrun.get_or_insert_with(|| Overrun::new(now, work())).look(now, work()) {
                    Look::Hung(why) => break why,
                    Look::Working(what) => if !crate::run::silent() { eprintln!("relax: first-frame: {secs} s passed ({basis}) and {what}"); },
                    Look::Wait => {}
                }
            }
            // (a first present from now on still counts: the deadline moves with its progress)
            deadline_ms = now + 500.0;
            continue;
        }
        if crate::jit::first_present_seen() {
            if !crate::run_threads::sleep(Duration::from_secs_f64((left / 1000.0).min(0.1))) { return; }
        } else {
            std::thread::park_timeout(Duration::from_secs_f64(left / 1000.0));
        }
    };
    if snapshot::hal_shell().as_ref().is_some_and(snapshot::all_hidden) {
        eprintln!("relax: {package} hid its window before its first frame; no first-frame timeout");
        return;
    }
    let presented = crate::jit::first_present_seen();
    let s = snapshot::capture(&format!("first-frame timeout after {secs} s ({basis}), then {hung}"));
    if presented {
        eprintln!("relax: first-frame timeout: {package} drew, but no frame of it reached the WM in {secs} s ({basis}), then {hung}");
    } else {
        eprintln!("relax: first-frame timeout: {package} drew no frame in {secs} s ({basis}), then {hung}");
    }
    eprintln!("relax: likely cause: {}", s.cause);
    eprintln!("relax: report: {}", s.path.display());
    let reason = format!("{} [report {}]", s.cause, s.path.display());
    persist("ff", Outcome::FirstFrameTimeout, package, &reason);
    // An app that drew keeps running: the WM link is the suspect and ending the app shows nothing.
    // A live session parked on a compile error or a panic waits for the edit or the decision.
    if presented || crate::layout::LIVE_SESSION.load(Acquire) && snapshot::awaiting_decision() { return; }
    crate::run::failed_run(&format!("first-frame timeout: {}", s.cause));
}

fn persist(prefix: &str, outcome: Outcome, package: &str, reason: &str) {
    let id = format!("{prefix}.{}.{pid}", (crate::clock::process_start_epoch() * 1e9) as u64, pid = std::process::id());
    let record = Record {
        load: id.clone(), id, at_ms: (crate::clock::process_start_epoch() * 1000.0 + crate::clock::ms_since_process_start()) as u64,
        app: package.into(), node: String::new(), pid: std::process::id(), outcome, reason: reason.into(),
        failure: String::new(), units: Vec::new(), logs: Vec::new(),
    };
    if let Err(e) = makepad_remote::hot::persist(&record) { eprintln!("relax: cannot persist {}: {e}", outcome.name()); }
}

pub(crate) fn no_window(package: &str, reason: &str) {
    let _kept = crate::zone_alloc::kept();
    persist("nw", Outcome::NoWindow, package, reason);
}

/// What this module holds of a run, as a fresh process has it (`run_end`). Nothing of the run is left that could see it.
pub(crate) unsafe fn reset_statics() {
    use relax_session::reset_static as reset;
    reset(&THREAD, OnceLock::new());
    reset(&MAIN_ENTERED_NS, OnceLock::new());
    reset(&APP_STARTED_MS, OnceLock::new());
    reset(&APP_ENDED, AtomicBool::new(false));
    reset(&CACHE_STATE, AtomicU8::new(0));
}

#[cfg(test)]
mod tests {
    /// A warm start's deadline counts this JIT build's warm runs only: a relaunch onto a rebuilt
    /// JIT (whose warm runs are another build's) is judged as a cold start, as one with no warm
    /// runs at all is; cold runs count whichever build measured them. A history row names its build.
    #[test]
    fn a_warm_start_of_a_new_jit_build_has_the_cold_deadline() {
        let rows = super::samples("app warm 4000 aaaa\napp cold 70000 aaaa\napp warm 5000\nother warm 9000 bbbb\n");
        assert_eq!(rows.len(), 3, "a row without its build is no sample");
        // a new build: no warm run of its own -- the cold deadline (3x the cold median, above the floor)
        let (secs, basis) = super::deadline_of(&rows, "app", "warm", "bbbb");
        assert_eq!(secs, 210, "{basis}");
        assert!(basis.starts_with("warm, but no measured warm start of this JIT build"), "{basis}");
        // the build that measured it: the warm floor (3x 4 s is below it)
        assert_eq!(super::deadline_of(&rows, "app", "warm", "aaaa").0, 30);
        // cold runs count whichever build measured them
        assert_eq!(super::deadline_of(&rows, "app", "cold", "bbbb").0, 210);
        assert_eq!(super::deadline_of(&rows, "new", "warm", "aaaa").0, 180);
    }

    /// The timeout is for hangs: past its deadline a start whose demand compiles still make
    /// progress (one finished in the last 2 s, or one runs that began under a minute ago) waits.
    #[test]
    fn a_start_still_compiling_is_not_hung() {
        let s = 1_000_000_000u64;
        assert!(super::still_compiling(false, s), "a compile finished a second ago");
        assert!(!super::still_compiling(false, 3 * s), "quiet for 3 s: hung");
        assert!(super::still_compiling(true, 30 * s), "a compile runs");
        assert!(!super::still_compiling(true, 61 * s), "one compile running past a minute is a hang");
    }

    /// Past its deadline a start is judged by its progress, never by the clock: a start whose
    /// GPU driver recompiles changed shaders (steady CPU, no compile of the JIT's) presents its
    /// first frame at 45 s, past a 30 s deadline, and is not killed; one whose JIT compiles works
    /// too; one idle for the window is hung at its end, and says what it did. Two live sessions were killed at the 30 s warm deadline while
    /// the driver compiled (11-18).
    #[test]
    fn a_start_past_its_deadline_is_hung_only_without_progress() {
        use super::{Look, Overrun, Work};
        // looks every 500 ms from the deadline at 30 s until its frame; the ms of its hang
        let run = |cores: f64, compile_every_ms: Option<f64>, frame_ms: f64| -> (Option<f64>, Vec<String>) {
            let at = |ms: f64| Work { cpu_ns: (ms * cores * 1e6) as u64, compiles: compile_every_ms.map_or(0, |e| (ms / e) as u64) };
            let mut said = Vec::new();
            let mut o = Overrun::new(30_000.0, at(30_000.0));
            let mut ms = 30_000.0;
            while ms < frame_ms {
                match o.look(ms, at(ms)) {
                    Look::Hung(why) => { said.push(why); return (Some(ms), said); }
                    Look::Working(what) => said.push(what),
                    Look::Wait => {}
                }
                ms += 500.0;
            }
            (None, said)
        };
        let (hung, said) = run(0.9, None, 45_000.0);
        assert_eq!(hung, None, "a driver compiling shaders at 0.9 of a core: its frame at 45 s counts");
        assert_eq!(said, vec!["it still works (90% of a core, 0 compile(s) finished in its last 10 s): no timeout while it does".to_string()], "said once");
        assert_eq!(run(0.05, Some(4_000.0), 60_000.0).0, None, "the JIT's compiles finish: it works");
        let (hung, said) = run(0.05, None, 60_000.0);
        assert_eq!(hung, Some(40_000.0), "idle for the window: hung at its end, not at the deadline");
        assert_eq!(said, vec!["no progress in its last 10 s (5% of a core, 0 compile(s) finished)".to_string()]);
        // it works, then stops: hung one window after its last work
        let mut o = Overrun::new(30_000.0, Work { cpu_ns: 0, compiles: 0 });
        assert_eq!(o.look(39_500.0, Work { cpu_ns: 9_000_000_000, compiles: 0 }), Look::Wait);
        assert!(matches!(o.look(40_000.0, Work { cpu_ns: 9_000_000_000, compiles: 0 }), Look::Working(_)));
        assert_eq!(o.look(45_000.0, Work { cpu_ns: 9_100_000_000, compiles: 0 }), Look::Wait);
        assert!(matches!(o.look(50_000.0, Work { cpu_ns: 9_200_000_000, compiles: 0 }), Look::Hung(_)));
    }

    /// A batch program is one whose main goes quiet while it runs. Not one whose app ended: a
    /// short program whose exit ran past the quiet was taken for one, and its tier-up, lowering
    /// ahead and coherence check ran in the exit (rwbench, 1.8 s and 2075 compiles counted as warm
    /// misses, 11-14). Not one that presented: an app whose own entry never says it is one (no
    /// `app_main!`, so no `app_started`) is an app when its main goes quiet; an app wrote its entry
    /// out by hand, and the run closed the window it drew in once its compiles went quiet (28-109 s
    /// after each start, 11-06).
    #[test]
    fn only_a_running_program_that_never_presented_is_a_batch_program() {
        isolated_test!(only_a_running_program_that_never_presented_is_a_batch_program);
        let wait = |watch: &std::thread::JoinHandle<f64>| {
            // (past the 2 s quiet: a watch that takes it for a batch program says so first)
            let t0 = std::time::Instant::now();
            while !watch.is_finished() && !crate::jit::past_startup_headless() && t0.elapsed() < std::time::Duration::from_secs(5) {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        };
        super::main_entered();
        assert!(super::APP_STARTED_MS.get().is_none(), "the entry never said it is an app");
        // its main returned: the exit's quiet is no batch program's
        super::app_ended();
        let watch = std::thread::spawn(super::watch_app);
        wait(&watch);
        assert!(!crate::jit::past_startup_headless(), "the exit of a program whose app ended was taken for a batch program");
        assert!(watch.is_finished(), "the app ended: the watch has nothing left to decide");
        assert_eq!(watch.join().unwrap(), 0.0, "no start to time");
        // it presented: the watch knows an app at once
        crate::jit::FIRST_PRESENT.store(true, std::sync::atomic::Ordering::Release);
        let watch = std::thread::spawn(super::watch_app);
        wait(&watch);
        assert!(watch.is_finished(), "it presented: the watch knows an app at once");
        assert!(watch.join().unwrap() > 0.0, "an app's start");
        assert!(!crate::jit::past_startup_headless(), "a program that presented was taken for a batch program");
    }
}
