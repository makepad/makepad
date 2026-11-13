//! The live harness rendezvous: tests finish before the watcher applies another edit.

use crate::inspect::json_str;
use std::sync::{Arc, Condvar, Mutex, OnceLock};

#[derive(Clone)]
pub(crate) struct Edit {
    pub paths: Vec<String>,
    pub group: u32,
    pub compile_ms: Option<f64>,
    pub restarted: bool,
    pub changed: Option<Vec<crate::program::DefId>>,
    /// a patch: the items it changed and the fns it installed (the verdict's reader sees both)
    pub patch: Option<(usize, usize)>,
}

impl Edit {
    fn fields(&self) -> String {
        let edit = if self.paths.is_empty() { json_str("initial") }
            else { format!("[{}]", self.paths.iter().map(|p| json_str(p)).collect::<Vec<_>>().join(",")) };
        let patch = self.patch.map_or(String::new(), |(items, fns)| format!(",\"items_changed\":{items},\"fns_installed\":{fns}"));
        format!("\"edit\":{edit},\"group\":{},\"compile_ms\":{},\"restarted\":{}{patch}",
            self.group, self.compile_ms.map_or("null".to_string(), |ms| ms.to_string()), self.restarted)
    }
}

struct State {
    edit: Edit,
    waiting: bool,
    faults: Vec<u32>,
    runs: u64,
    /// the latest completed run's results and its edit group (`test-results`)
    results: Option<(u32, Arc<[TestResult]>)>,
}

static LIVE: OnceLock<(Mutex<State>, Condvar)> = OnceLock::new();

pub(crate) fn enabled() -> bool { LIVE.get().is_some() }

/// Exec carries the triggering edit in argv; no switch or scratch file is inherited by tests.
pub(crate) fn start(restart: Option<&str>) -> Result<(), String> {
    let _k = crate::zone_alloc::kept();
    let edit = match restart {
        None => Edit { paths: Vec::new(), group: 0, compile_ms: None, restarted: false, changed: None, patch: None },
        Some(s) => {
            use crate::json::Json;
            let j = crate::json::parse(s.as_bytes())?;
            let group = match j.get("group") {
                Some(Json::Num(n)) if *n >= 0.0 && *n <= u32::MAX as f64 && n.fract() == 0.0 => *n as u32,
                _ => return Err("invalid harness restart group".into()),
            };
            let paths = match j.get("edit") {
                Some(Json::Arr(v)) if v.iter().all(|p| matches!(p, Json::Str(_))) => v.iter().map(|p| p.str().to_string()).collect(),
                Some(Json::Str(s)) if s == "initial" => Vec::new(),
                _ => return Err("invalid harness restart edit".into()),
            };
            let compile_ms = match j.get("compile_ms") {
                Some(Json::Num(n)) if n.is_finite() && *n >= 0.0 => Some(*n),
                Some(Json::Null) => None,
                _ => return Err("invalid harness restart compile time".into()),
            };
            Edit { paths, group, compile_ms, restarted: true, changed: None, patch: None }
        }
    };
    let _ = LIVE.set((Mutex::new(State { edit, waiting: false, faults: Vec::new(), runs: 0, results: None }), Condvar::new()));
    Ok(())
}

pub(crate) fn group() -> u32 {
    LIVE.get().map_or(0, |(state, _)| state.lock().unwrap().edit.group)
}

pub(super) enum TestStatus { Passed, Failed, Ignored, Filtered }

/// One test's outcome in a completed run.
pub(super) struct TestResult {
    pub name: String,
    pub status: TestStatus,
    /// it executed for this edit; a passed or failed result an earlier run left is kept with false
    pub ran: bool,
}

impl TestResult {
    fn json(&self) -> String {
        let status = match self.status {
            TestStatus::Passed => "passed",
            TestStatus::Failed => "failed",
            TestStatus::Ignored => "ignored",
            TestStatus::Filtered => "filtered",
        };
        format!("{{\"name\":{},\"status\":\"{status}\",\"ran\":{}}}", json_str(&self.name), self.ran)
    }
}

/// `test-results [group=<n>]`: the latest completed run's group and test count, then one line per
/// test with its name, status and whether it ran for that edit. A group other than that run's is
/// refused rather than answered with another edit's results. The rows are formatted after the
/// lock is released (the harness takes it at every verdict; a target's tests can be thousands).
pub(crate) fn cmd_results(req: &crate::inspect::Req, out: &mut crate::inspect::Out) {
    let Some((state, _)) = LIVE.get() else { out.err("not a live test session"); return };
    let asked = match req.get("group").map(str::parse::<u32>).transpose() {
        Ok(group) => group,
        Err(_) => { out.err("group must be an unsigned 32-bit integer"); return; }
    };
    let Some((group, tests)) = state.lock().unwrap().results.clone() else { out.err("no completed test run yet"); return };
    if asked.is_some_and(|asked| asked != group) {
        out.err(&format!("test results are for group {group}; read its testverdict event"));
        return;
    }
    out.line(&format!("{{\"group\":{group},\"test_count\":{}}}", tests.len()));
    for t in tests.iter() { out.line(&t.json()); }
}

/// Whole-target results, with counts for this run's selection.
pub(super) struct Verdict {
    pub ran: usize,
    pub passed: usize,
    pub ignored: usize,
    pub failures: Vec<String>,
    pub stale: Vec<String>,
    pub why: Option<String>,
    pub skipped: usize,
    /// every test's outcome, in the table's order
    pub tests: Vec<TestResult>,
}

/// Park outside the compiler owner until the next verified edit.
pub(super) fn complete(v: Verdict, ms: f64) -> Edit {
    crate::hot_failure::pump();
    if crate::hot_failure::has_error() { std::process::exit(crate::hot_failure::verdict(101).code()); }
    let (state, ready) = LIVE.get().unwrap();
    let mut s = state.lock().unwrap();
    s.runs += 1;
    let Verdict { ran, passed, ignored, failures, stale, why, skipped, tests } = v;
    let failed = failures.len();
    let selected = if why.is_none() { "affected" } else { "all" };
    let why_field = why.as_ref().map_or(String::new(), |s| format!(",\"why\":{}", json_str(s)));
    // the verdict stays small at any test count: each test's outcome is the `results` query's
    let fields = format!("{},\"ran\":{ran},\"passed\":{passed},\"failed\":{failed},\"ignored\":{ignored},\"failures\":[{}],\"ms\":{ms},\"selected\":\"{selected}\",\"skipped\":{skipped},\"stale\":[{}]{why_field},\"results\":\"test-results group={}\"",
        s.edit.fields(), failures.iter().map(|n| json_str(n)).collect::<Vec<_>>().join(","),
        stale.iter().map(|n| json_str(n)).collect::<Vec<_>>().join(","), s.edit.group);
    s.results = Some((s.edit.group, tests.into()));
    crate::inspect::events::emit("testverdict", &s.runs.to_string(), &fields);
    println!("testverdict group={} ran={ran} passed={passed} failed={failed} ms={ms:.1} selected={selected} skipped={skipped}{}", s.edit.group, why.as_ref().map_or(String::new(), |s| format!(" why={s:?}")));
    use std::io::Write;
    let _ = std::io::stdout().flush();
    s.waiting = true;
    ready.notify_all();
    while s.waiting { s = ready.wait(s).unwrap(); }
    s.edit.clone()
}

/// Watcher startup waits for the initial verdict, including the test threads' cleanup.
pub(crate) fn wait_initial() {
    let Some((state, ready)) = LIVE.get() else { return };
    let mut s = state.lock().unwrap();
    while !s.waiting { s = ready.wait(s).unwrap(); }
    s.faults.clear();
}

/// Wake the program after verification and wait for its verdict before another patch can land.
pub(crate) fn rerun(edit: &Edit) -> Vec<u32> {
    let _k = crate::zone_alloc::kept();
    let (state, ready) = LIVE.get().unwrap();
    let mut s = state.lock().unwrap();
    s.edit = edit.clone();
    s.waiting = false;
    ready.notify_all();
    while !s.waiting { s = ready.wait(s).unwrap(); }
    std::mem::take(&mut s.faults)
}

thread_local! {
    static FAULT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Contained hardware faults fail even `should_panic` tests; normal panics remain libtest's.
pub(crate) fn fault(up: *mut crate::jit::Unit, p: &crate::jit::PanicInfo) {
    let Some((state, _)) = LIVE.get() else { return };
    if up != crate::run::running_unit() { return; }
    FAULT.with(|f| f.set(true));
    super::reach::fault();
    let group = crate::owner::call(crate::owner::Class::Program, || unsafe {
        let u = &*up;
        p.frames.iter().filter_map(|pc| u.find_fn(*pc)).map(|id| u.fns[id as usize].patch_gen).max().unwrap_or(0)
    });
    if group > 0 {
        let _k = crate::zone_alloc::kept();
        let mut s = state.lock().unwrap();
        if !s.faults.contains(&group) { s.faults.push(group); }
    }
}

pub(super) extern "C" fn take_fault() -> u32 { FAULT.with(|f| f.replace(false)) as u32 }

pub(crate) fn restart(mut edit: Edit, why: &str) {
    edit.restarted = true;
    LIVE.get().unwrap().0.lock().unwrap().edit = edit;
    if let Err(e) = crate::reload::restart_run(why) {
        eprintln!("relax test: {e}");
        crate::hot_failure::restart_failed(&e);
        crate::hot_failure::pump();
        if crate::hot_failure::has_error() { std::process::exit(crate::hot_failure::verdict(101).code()); }
    }
}

/// Replace the previous exec marker before the package, preserving the libtest argv verbatim.
#[cfg(any(unix, windows))]
pub(crate) fn restart_args(args: Vec<std::ffi::OsString>) -> Vec<std::ffi::OsString> {
    let Some((state, _)) = LIVE.get() else { return args };
    let mut out = Vec::new();
    let mut words = args.into_iter();
    if let Some(command) = words.next() { out.push(command); }
    out.push("--harness-restart".into());
    out.push(format!("{{{}}}", state.lock().unwrap().edit.fields()).into());
    while let Some(w) = words.next() {
        if w == "--" { out.push(w); out.extend(words); break; }
        if w == "--harness-restart" { words.next(); } else { out.push(w); }
    }
    out
}

/// What this module holds of a run, as a fresh process has it (`run_end`). Nothing of the run is left that could see it.
pub(crate) unsafe fn reset_statics() {
    use relax_session::reset_static as reset;
    reset(&LIVE, OnceLock::new());
}
