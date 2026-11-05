//! One recovery path for live-load failures. The watcher owns reporting and I/O;
//! application callbacks only publish completion, so no UI frame waits for disk.

use makepad_remote::hot::{Outcome, Record, SourceUnit, Verdict};
use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

#[derive(Clone)]
struct Attempt { id: String, units: Arc<Vec<SourceUnit>>, logs: Arc<Vec<String>> }
struct Recovery { attempt: Attempt, failure: String }
#[derive(Default)]
struct State {
    current: Option<Attempt>,
    groups: BTreeMap<u32, Attempt>,
    recovery: Option<Recovery>,
    pending: VecDeque<Record>,
    terminal: Option<String>,
}
static STATE: std::sync::OnceLock<Mutex<State>> = std::sync::OnceLock::new();
static READY: AtomicBool = AtomicBool::new(false);
static ACTIVE: AtomicBool = AtomicBool::new(false);
static REPORTING: AtomicBool = AtomicBool::new(false);
static ERROR: AtomicBool = AtomicBool::new(false);
static RESTART_ERROR: Mutex<Option<String>> = Mutex::new(None);
static WRITER: Mutex<()> = Mutex::new(());
// Only worker callers pump. UI callbacks publish through READY/RESTART_ERROR.
static PUMP: Mutex<()> = Mutex::new(());
static REPLACEMENT: AtomicBool = AtomicBool::new(false);

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.get_or_init(|| Mutex::new(State::default())).lock().unwrap_or_else(|e| e.into_inner())
}
fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |t| t.as_millis() as u64)
}
fn id() -> String {
    static START: std::sync::OnceLock<u128> = std::sync::OnceLock::new();
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let start = START.get_or_init(|| std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |t| t.as_nanos()));
    format!("hot.{start}.{}.{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed))
}

fn record(a: &Attempt, outcome: Outcome, reason: String, failure: String, logs: Vec<String>) -> Record {
    let mut all_logs = if outcome.failure() { (*a.logs).clone() } else { Vec::new() };
    all_logs.extend(logs);
    Record { id: id(), load: a.id.clone(), at_ms: now(), app: std::env::var("MAKEPAD_RUN_PACKAGE").unwrap_or_default(), node: String::new(), pid: std::process::id(),
        outcome, reason, failure, units: if outcome.failure() { (*a.units).clone() } else { Vec::new() }, logs: all_logs }
}

fn current() -> Attempt {
    state().current.clone().unwrap_or_else(empty_attempt)
}

fn empty_attempt() -> Attempt { Attempt { id: id(), units: Arc::new(Vec::new()), logs: Arc::new(Vec::new()) } }

pub(crate) fn classified(lines: &[String]) {
    let _kept = crate::zone_alloc::kept();
    let logs = Arc::new(lines.to_vec());
    let old = state().current.as_mut().map(|a| std::mem::replace(&mut a.logs, logs));
    drop(old);
}

/// Capture the running text before the transaction can replace its file table.
pub(crate) unsafe fn begin(up: *mut crate::jit::Unit, edits: &[crate::reload::Edit]) {
    enable();
    let _kept = crate::zone_alloc::kept();
    let root = crate::run::workspace_dir();
    let before: Vec<_> = crate::owner::call(crate::owner::Class::Program, || {
        let _kept = crate::zone_alloc::kept();
        let p = &(*up).prog;
        edits.iter().map(|edit| p.files.get(edit.file as usize).cloned()).collect()
    });
    // Only Arc snapshots cross the owner. Copying changed text and producing
    // diffs/JSON runs on the watcher, outside the UI's compiler request queue.
    let units = edits.iter().zip(before).map(|(edit, file)| {
        let path = crate::vfs::relative_path(&edit.path, &root).unwrap_or_else(|| std::path::Path::new(&edit.path).file_name().and_then(|p| p.to_str()).unwrap_or("source").to_string());
        let before = file.map(|file| String::from_utf8_lossy(&file.src[..file.src_len as usize]).into_owned()).unwrap_or_default();
        SourceUnit { path, before, after: String::from_utf8_lossy(&edit.src).into_owned() }
    }).collect();
    let old = state().current.replace(Attempt { id: id(), units: Arc::new(units), logs: Arc::new(Vec::new()) });
    drop(old);
}

pub(crate) fn applied(group: u32) {
    let _kept = crate::zone_alloc::kept();
    let a = current();
    let old = state().groups.insert(group, a);
    drop(old);
}

pub(crate) fn verified(group: u32) {
    if !REPORTING.load(Ordering::Acquire) { return; }
    let _kept = crate::zone_alloc::kept();
    let a = { state().groups.remove(&group) };
    let a = a.unwrap_or_else(empty_attempt);
    let r = record(&a, Outcome::Verified, String::new(), String::new(), Vec::new());
    state().pending.push_back(r);
    flush();
}

/// Starts a single recovery; a failed recovery is a separate JIT error, never a
/// retry loop or a test failure. The caller requests the existing restart gate.
pub fn failed(outcome: Outcome, lines: &[String], group: Option<u32>) -> bool {
    if !REPORTING.load(Ordering::Acquire) { return false; }
    let _kept = crate::zone_alloc::kept();
    if ACTIVE.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire).is_err() { return false; }
    REPLACEMENT.store(false, Ordering::Release);
    let a = group.and_then(|g| state().groups.remove(&g)).unwrap_or_else(current);
    let r = record(&a, outcome, lines.join("; "), String::new(), lines.to_vec());
    let record_id = r.id.clone();
    let load_id = r.load.clone();
    let mut s = state();
    s.recovery = Some(Recovery { attempt: a, failure: r.id.clone() });
    s.pending.push_back(r);
    drop(s);
    durable();
    crate::inspect::events::emit("patchfailed", &record_id,
        &format!("\"record\":{},\"load\":{},\"outcome\":{}", crate::inspect::json_str(&record_id), crate::inspect::json_str(&load_id), crate::inspect::json_str(outcome.name())));
    true
}

/// devhub's lane verdict channel: the run could not get past a JIT failure (its hot load failed
/// and the warm restart that follows it failed too; `record`: this ledger's id, `reason`: one
/// line). A hot-load failure the warm restart recovers is `patchfailed` then `patchrestarted`,
/// never this. The agent's run reads "JIT error, not your change" from devhub; the text verdict
/// line is a log line only.
fn jit_error(record: &str, reason: &str) {
    let reason = reason.lines().next().unwrap_or("");
    crate::inspect::events::emit("jiterror", record,
        &format!("\"record\":{},\"reason\":{}", crate::inspect::json_str(record), crate::inspect::json_str(reason)));
}

/// Records per run (a checker broken for every body of an app wrote one synced file per body; the
/// run's summary counts them all).
const CHECKER_PANIC_RECORDS: u64 = 16;

/// A panic in a check of `body` (borrowck::lazy's sink, typeck_bg): its own record, since no hot load
/// failed and the app runs on. This ledger is where the next one is seen. Runs on the checking
/// thread: a write that fails stays pending for the next flush, and nothing here waits for it.
pub fn checker_panic(body: &str, message: &str) {
    static RECORDED: AtomicU64 = AtomicU64::new(0);
    if RECORDED.fetch_add(1, Ordering::Relaxed) >= CHECKER_PANIC_RECORDS { return; }
    let _kept = crate::zone_alloc::kept();
    let r = record(&empty_attempt(), Outcome::CheckerPanic, format!("checker panic in `{body}`: {message}"), String::new(), Vec::new());
    state().pending.push_back(r);
    flush();
}

pub fn recovering() -> bool { ACTIVE.load(Ordering::Acquire) }
/// Whether a live edit was attempted in this process.
pub(crate) fn edit_ran() -> bool { state().current.is_some() }
pub(crate) fn enable() { REPORTING.store(true, Ordering::Release); }
pub fn has_error() -> bool { ERROR.load(Ordering::Acquire) }
pub(crate) fn replacement_loaded() {
    if recovering() { REPLACEMENT.store(true, Ordering::Release); }
}
pub(crate) fn replacement_running() -> bool { REPLACEMENT.load(Ordering::Acquire) }

/// Called by the native app at its first present, or by a terminal harness once
/// its replacement has actually run. The watcher does the reporting.
pub fn ready() {
    if recovering() && replacement_running() { READY.store(true, Ordering::Release); crate::reload::watch_kick(); }
}

/// Loader/host completion; the message is copied under a short metadata lock.
pub fn restart_failed(why: &str) {
    if recovering() {
        let _kept = crate::zone_alloc::kept();
        let message = why.to_string();
        let old = RESTART_ERROR.lock().unwrap_or_else(|e| e.into_inner()).replace(message);
        drop(old);
        crate::reload::watch_kick();
    }
}

pub fn pump() {
    let _pump = PUMP.lock().unwrap_or_else(|e| e.into_inner());
    let _kept = crate::zone_alloc::kept();
    let error = RESTART_ERROR.lock().unwrap_or_else(|e| e.into_inner()).take();
    let ready = READY.swap(false, Ordering::AcqRel);
    if error.is_some() || ready {
        let recovery = state().recovery.take();
        if let Some(recovery) = recovery {
            let outcome = if error.is_some() { Outcome::RestartFailed } else { Outcome::Restarted };
            let reason = error.unwrap_or_else(|| "warm restart completed".into());
            let r = record(&recovery.attempt, outcome, reason.clone(), recovery.failure, vec![reason.clone()]);
            let record_id = r.id.clone();
            let load_id = r.load.clone();
            state().pending.push_back(r);
            durable();
            if outcome == Outcome::RestartFailed {
                let verdict = Verdict::JitError { record: record_id.clone() };
                eprintln!("{}", verdict.line());
                state().terminal = Some(record_id.clone());
                ERROR.store(true, Ordering::Release);
            }
            if outcome == Outcome::RestartFailed {
                jit_error(&record_id, &reason);
            } else {
                crate::inspect::events::emit("patchrestarted", &record_id,
                    &format!("\"record\":{},\"load\":{}", crate::inspect::json_str(&record_id), crate::inspect::json_str(&load_id)));
            }
        }
        ACTIVE.store(false, Ordering::Release);
        REPLACEMENT.store(false, Ordering::Release);
    }
    flush();
}

/// Only the reporting worker calls this. Failed writes keep their exact records
/// and retry on the watcher's next round; no record is dropped for a size budget.
fn flush() -> bool {
    let _writer = WRITER.lock().unwrap_or_else(|e| e.into_inner());
    let mut records = std::mem::take(&mut state().pending);
    while let Some(r) = records.front() {
        if let Err(e) = makepad_remote::hot::persist(r) {
            eprintln!("makepad hot: record {} pending: {e}", r.id);
            let mut s = state();
            records.append(&mut s.pending);
            s.pending = records;
            return false;
        }
        records.pop_front();
    }
    true
}

fn durable() {
    while !flush() { std::thread::sleep(std::time::Duration::from_secs(1)); }
}

pub fn verdict(code: i32) -> Verdict {
    match state().terminal.clone() {
        Some(record) => Verdict::JitError { record },
        None => Verdict::Tests { code },
    }
}

/// An existing process restart carries the recovery identity and changed units
/// alongside the app's snapshot. It does not manufacture another hot load.
#[cfg(any(unix, windows))]
pub(crate) fn prepare_exec(cmd: &mut std::process::Command) -> Result<(), String> {
    let _kept = crate::zone_alloc::kept();
    let recovery = state().recovery.as_ref().map(|r| (r.attempt.clone(), r.failure.clone()));
    let Some((attempt, failure)) = recovery else { return Ok(()) };
    let mut data = record(&attempt, Outcome::Refused, String::new(), failure, Vec::new());
    data.units = (*attempt.units).clone();
    let path = makepad_remote::hot::home().join("hot-restart").join(format!("{}.json", data.id));
    std::fs::create_dir_all(path.parent().unwrap()).and_then(|_| std::fs::write(&path, data.to_json())).map_err(|e| e.to_string())?;
    cmd.env("MAKEPAD_JIT_RECOVERY", path);
    Ok(())
}

/// Windows: the next run never started, and the run that asked for it has ended, so its parent
/// (restart_parent.rs) closes the recovery that run's file `path` describes.
#[cfg(windows)]
pub(crate) fn next_run_failed(path: &std::path::Path, why: &str) {
    let Some(r) = std::fs::read_to_string(path).ok().and_then(|s| Record::parse(&s)) else {
        eprintln!("makepad hot: cannot read the recovery record {}", path.display());
        return;
    };
    let _ = std::fs::remove_file(path);
    let attempt = Attempt { id: r.load, units: Arc::new(r.units), logs: Arc::new(r.logs) };
    let mut done = record(&attempt, Outcome::RestartFailed, why.to_string(), r.failure, vec![why.to_string()]);
    done.app = r.app;
    eprintln!("{}", Verdict::JitError { record: done.id.clone() }.line());
    if let Err(e) = makepad_remote::hot::persist(&done) {
        eprintln!("makepad hot: record {} not written: {e}", done.id);
    }
}

pub(crate) fn restore() {
    let _kept = crate::zone_alloc::kept();
    let Some(path) = std::env::var_os("MAKEPAD_JIT_RECOVERY") else { return };
    std::env::remove_var("MAKEPAD_JIT_RECOVERY");
    match std::fs::read_to_string(&path).ok().and_then(|s| Record::parse(&s)) {
        Some(r) => {
            state().recovery = Some(Recovery { attempt: Attempt { id: r.load, units: Arc::new(r.units), logs: Arc::new(r.logs) }, failure: r.failure });
            ACTIVE.store(true, Ordering::Release);
            REPLACEMENT.store(true, Ordering::Release);
            let _ = std::fs::remove_file(path);
        }
        None => eprintln!("makepad hot: cannot restore the pending recovery record"),
    }
}
