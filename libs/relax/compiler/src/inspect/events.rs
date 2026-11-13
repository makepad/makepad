//! Events: what an agent must hear about without asking (DESIGN.md §2.1, BRIEF "no human in the
//! loop"): panics (parked at the panic point), hangs, deadlocks, memory growth. Each event is one
//! compact JSON line with a sequence number and drill-down commands. `watch` blocks until the next
//! one and returns it (an agent's background task then exits: its notification), `events
//! follow=1` streams them (one line each), and `makepad run --agent` prints them on stderr too.
//! Repeats of the same event (kind + key) within 30 s are counted, not sent again.
//!
//! Panic parking: a panic in JIT code on a program thread parks with its stack intact for up to
//! 600 s when someone can decide it: an inspect watcher, `--agent`, devhub, the visible Run panel
//! in a live session, or an interactive terminal. Otherwise it parks briefly
//! (RELAX_PANIC_PARK seconds, default 2), once per site. Someone attaching extends a short
//! park to 600 s from its start; a long park never shortens. Resuming or timing out unwinds as
//! usual. `--unattended` never parks. Every park always prints one line, even under --quiet or
//! --silent, naming who can decide, the limit, how to inspect it and how --unattended avoids it;
//! an extension prints one more line naming who attached and the new limit.

use super::{json_str, now_ms, Out, Req};
use std::collections::VecDeque;
use std::io::IsTerminal;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering::{Relaxed, SeqCst}};
use relax_session::sync::{Condvar, Mutex};

pub struct Event {
    pub seq: u64,
    pub kind: &'static str,
    key: String,
    /// the whole event as one JSON object
    pub json: std::sync::Arc<str>,
    at: std::time::Instant,
    /// made while no patch group awaited its borrow-check verdict (SPEC-V)
    verified: bool,
}

const KEEP: usize = 256;
static EVENTS: Mutex<VecDeque<Event>> = Mutex::new(VecDeque::new());
static NEW: Condvar = Condvar::new();
static SEQ: AtomicU64 = AtomicU64::new(0);
/// sessions in `watch` / `events follow=1` now
pub static WATCHERS: AtomicU32 = AtomicU32::new(0);
/// `observe` followers (devhub): can decide a park without arming costly detectors.
static OBSERVERS: AtomicU32 = AtomicU32::new(0);
static RUN_STATE_AVAILABLE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static RUN_STATE_OBSERVER: std::sync::OnceLock<fn()> = std::sync::OnceLock::new();

pub fn set_run_state_observer(observer: fn()) { let _ = RUN_STATE_OBSERVER.set(observer); }
/// The native presenter can reveal a parked panic even while its document is
/// hidden behind the running app. Availability lasts until the host pump exits.
pub fn run_state_available(available: bool) {
    RUN_STATE_AVAILABLE.store(available, SeqCst);
    retry_update();
}
fn parked_changed() { if let Some(observer) = RUN_STATE_OBSERVER.get() { observer(); } }

/// The same records returned by `panics`, retained for the lifetime of the park
/// even if the inspect event history rolls over. No owner-thread/symbol lookup.
pub fn parked_events(domain: u32) -> Option<Vec<std::sync::Arc<str>>> {
    // Panic collection and inspect commands can hold this lock while doing
    // symbol work. A UI retries on its next turn rather than waiting for them.
    let list = match PARKED.try_lock() {
        Ok(list) => list,
        Err(std::sync::TryLockError::Poisoned(error)) => error.into_inner(),
        Err(std::sync::TryLockError::WouldBlock) => return None,
    };
    Some(list.iter()
        .filter(|p| p.domain == Some(domain) && p.decision.load(SeqCst) == 0)
        .map(|p| p.event.clone()).collect())
}

pub const KINDS: &[&str] = &["panic", "hang", "deadlock", "mem", "spike", "hitch", "rtstall", "scripterr", "patchrefused", "patchverified", "patchreverted", "patchfailed", "patchrestarted", "jiterror", "engine_request", "engine_reply", "engine_code_update"];

/// Records an event (`fields`: the kind-specific JSON members, without braces). Returns its seq,
/// or 0 when it repeated one sent in the last 30 s.
pub fn emit(kind: &'static str, key: &str, fields: &str) -> u64 {
    emit_record(kind, key, fields).0
}

fn emit_record(kind: &'static str, key: &str, fields: &str) -> (u64, std::sync::Arc<str>) {
    let _k = crate::zone_alloc::kept();
    if kind != "panic" {
        super::flightrec::inspect_event(kind, key);
    }
    let mut q = EVENTS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(e) = q.iter().rev().find(|e| e.kind == kind && e.key == key) {
        if e.at.elapsed().as_secs() < 30 {
            return (0, e.json.clone());
        }
    }
    let seq = SEQ.fetch_add(1, SeqCst) + 1;
    let verified = crate::reload::observations_verified();
    // a test that crashes or parks this process on purpose says why (RELAX_EXPECTED_CRASH): its
    // panic is the test's, and whoever follows the run (devhub) does not stop for it
    let expected = match std::env::var("RELAX_EXPECTED_CRASH") {
        Ok(why) if kind == "panic" => format!(",\"expected\":{}", json_str(if why.is_empty() { "yes" } else { &why })),
        _ => String::new(),
    };
    let json = format!(
        "{{\"event\":\"{}\",\"seq\":{},\"pid\":{},\"app\":{},\"at_ms\":{},\"verified\":{},{}{}}}",
        kind,
        seq,
        std::process::id(),
        json_str(&super::APP.lock().unwrap_or_else(|e| e.into_inner())),
        now_ms(),
        verified,
        fields,
        expected
    );
    if super::AGENT.load(Relaxed) {
        eprintln!("relax event: {}", json);
    }
    if let Some(w) = super::WATCH.get() {
        crate::zone_alloc::kept_with(|| w.emit(&json));
    }
    let json: std::sync::Arc<str> = json.into();
    q.push_back(Event { seq, kind, key: key.to_string(), json: json.clone(), at: std::time::Instant::now(), verified });
    while q.len() > KEEP {
        q.pop_front();
    }
    NEW.notify_all();
    (seq, json)
}

/// The sequence number of the newest event so far.
pub fn last_seq() -> u64 {
    SEQ.load(SeqCst)
}

/// SPEC-V: the retained events after `seq` that were made while a patch awaited its verdict.
pub fn unverified_seqs_since(seq: u64) -> Vec<u64> {
    EVENTS.lock().unwrap_or_else(|e| e.into_inner()).iter().filter(|e| e.seq > seq && !e.verified).map(|e| e.seq).collect()
}

/// Someone may want events now (a watcher, a stream, `--agent`): detectors with a cost on the
/// program's paths run only then.
#[inline]
pub fn watched() -> bool {
    WATCHERS.load(Relaxed) > 0 || super::AGENT.load(Relaxed)
}

/// A makepad-remote `watch` channel started following: it counts as a watcher, like a `watch` command.
pub fn remote_watcher_opened() {
    WATCHERS.fetch_add(1, SeqCst);
    super::timeline::set_frame_watch(true);
    retry_update();
}

pub fn remote_watcher_closed() {
    if WATCHERS.fetch_sub(1, SeqCst) == 1 && !super::AGENT.load(Relaxed) {
        super::timeline::set_frame_watch(false);
    }
    retry_update();
}

pub fn observer_opened() {
    OBSERVERS.fetch_add(1, SeqCst);
    retry_update();
}

pub fn observer_closed() {
    OBSERVERS.fetch_sub(1, SeqCst);
    retry_update();
}

/// Retry boundaries are worth setting while someone may resume a panic with `retry`.
fn retry_update() {
    RELAX_RETRY_ON.store(watched() || OBSERVERS.load(Relaxed) > 0 || RUN_STATE_AVAILABLE.load(Relaxed), Relaxed);
}

fn wanted(on: &Option<Vec<String>>, kind: &str) -> bool {
    on.as_ref().map_or(true, |v| v.iter().any(|k| k == kind))
}

fn kinds(req: &Req) -> Option<Vec<String>> {
    req.get("on").map(|s| s.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect())
}

/// `watch [on=..] [since=<seq>] [timeout_s=N]`: the first event after `since` (default: now; a
/// panic parked now always counts), as one line.
pub fn cmd_watch(req: &Req, out: &mut Out) {
    let on = kinds(req);
    // A run that does not watch its sources never emits a patch event: say so now, not after
    // the caller's timeout.
    if on.as_ref().is_some_and(|v| v.iter().any(|k| k.starts_with("patch"))) && !crate::layout::LIVE_SESSION.load(Relaxed) {
        out.line("{\"err\":\"not a live session: this run does not watch its sources, so no edit patches in (a run from a store is live with RELAX_STORE_LIVE=1 and an edits folder)\"}");
        return;
    }
    let parked_since = PARKED.lock().unwrap_or_else(|e| e.into_inner()).iter().map(|p| p.seq).min().map_or(u64::MAX, |s| s - 1);
    let since = req.get("since").and_then(|s| s.parse().ok()).unwrap_or_else(|| SEQ.load(SeqCst).min(parked_since));
    let timeout = req.num("timeout_s", 0);
    let t0 = std::time::Instant::now();
    WATCHERS.fetch_add(1, SeqCst);
    super::timeline::set_frame_watch(true);
    retry_update();
    let mut q = EVENTS.lock().unwrap_or_else(|e| e.into_inner());
    let found = loop {
        if let Some(e) = q.iter().find(|e| e.seq > since && wanted(&on, e.kind)) {
            break Some((e.seq, e.json.clone()));
        }
        if timeout > 0 && t0.elapsed().as_secs() >= timeout {
            break None;
        }
        // the client went away: the watch ends (it counted as a watcher)
        if out.cancelled() {
            break None;
        }
        q = NEW.wait_timeout(q, std::time::Duration::from_millis(500)).unwrap_or_else(|e| e.into_inner()).0;
    };
    drop(q);
    if WATCHERS.fetch_sub(1, SeqCst) == 1 && !super::AGENT.load(Relaxed) {
        super::timeline::set_frame_watch(false);
    }
    retry_update();
    if found.is_none() && out.cancelled() {
        return;
    }
    match found {
        Some((seq, json)) => out.line(&format!("{},\"rearm\":\"watch since={}\"}}", &json[..json.len() - 1], seq)),
        None => out.line(&format!("{{\"event\":\"none\",\"waited_s\":{},\"rearm\":\"watch since={}\"}}", timeout, since)),
    }
}

/// `events [since=<seq>] [follow=1] [on=..]`: the kept events after `since` (default: the last
/// 20), then with follow=1 each new one as it comes, until the client goes away.
pub fn cmd_events(req: &Req, out: &mut Out) {
    let on = kinds(req);
    let follow = req.get("follow") == Some("1");
    let asked = req.get("since").and_then(|s| s.parse().ok());
    let mut since = asked.unwrap_or_else(|| SEQ.load(SeqCst).saturating_sub(20));
    out.measured("events", "event queue", &[]);
    if follow {
        WATCHERS.fetch_add(1, SeqCst);
        super::timeline::set_frame_watch(true);
        retry_update();
    }
    loop {
        let batch: Vec<(u64, std::sync::Arc<str>)> = {
            let q = EVENTS.lock().unwrap_or_else(|e| e.into_inner());
            q.iter().filter(|e| e.seq > since && wanted(&on, e.kind)).map(|e| (e.seq, e.json.clone())).collect()
        };
        for (seq, j) in &batch {
            out.line(j);
            since = *seq;
        }
        if !follow || !out.flush() || out.cancelled() {
            break;
        }
        let q = EVENTS.lock().unwrap_or_else(|e| e.into_inner());
        if q.back().map_or(true, |e| e.seq <= since) {
            let _ = NEW.wait_timeout(q, std::time::Duration::from_millis(500));
        }
        // a heartbeat-free stream: a closed client is noticed at the next write
    }
    if follow && WATCHERS.fetch_sub(1, SeqCst) == 1 && !super::AGENT.load(Relaxed) {
        super::timeline::set_frame_watch(false);
    }
    retry_update();
}

// ------------------------------------------------------------------ parked panics

struct Parked {
    id: u64,
    seq: u64,
    os_id: u64,
    domain: Option<crate::domain::DomainId>,
    /// 0 waiting, 1 resume as a panic, 2 retry the boundary's unit, 3 restart the restartable fn,
    /// 4 make it return `words`, 5 stop the process
    decision: AtomicU32,
    words: Mutex<Vec<u64>>,
    event: std::sync::Arc<str>,
    /// (thread name, message, location): why-hung's verdict names the parked panic
    what: (String, String, String),
    /// its frames, innermost first (lookup pcs): `eval`'s default scope, `locals`
    frames: Vec<u64>,
    /// the panic trampoline's register block at the innermost frame's call (frame pointer,
    /// return address, callee-saved registers; jit.rs REG_WORDS): `locals` reads the frames'
    /// variables from it; empty for a stop without one (a fault)
    regs: Vec<u64>,
    kind: ParkKind,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ParkKind {
    Panic,
    /// a stale access (stale.rs): parked in its fault handler for good; only `restart` / `stop`
    Stale,
    /// a call of code that did not compile (park_compile_error): a live patch that fixes it
    /// resumes the call; only `stop`
    CompileError,
    /// a call whose code a live update could not build (park_update_failed): it waits for the
    /// person; only `restart` / `stop`
    UpdateFailed,
}

/// Parked panic `id`'s (the newest with None) register block at its innermost frame's call
/// (empty: none): (id, block).
pub fn parked_regs(id: Option<u64>) -> Option<(u64, Vec<u64>)> {
    let list = PARKED.lock().unwrap_or_else(|e| e.into_inner());
    let p = match id {
        Some(k) => list.iter().find(|p| p.id == k)?,
        None => list.iter().max_by_key(|p| p.id)?,
    };
    Some((p.id, p.regs.clone()))
}

/// Parked panic `id` (the newest with None): (id, its domain, its frames as lookup pcs, its
/// thread's os id).
pub fn parked_frames(id: Option<u64>) -> Option<(u64, Option<crate::domain::DomainId>, Vec<u64>, u64)> {
    let list = PARKED.lock().unwrap_or_else(|e| e.into_inner());
    let p = match id {
        Some(k) => list.iter().find(|p| p.id == k)?,
        None => list.iter().max_by_key(|p| p.id)?,
    };
    Some((p.id, p.domain, p.frames.clone(), p.os_id))
}

// ---- retry boundaries (DESIGN.md §10b): makepad's event dispatch runs inside one while an agent
// watches; a parked panic resumed with `retry` unwinds to it and the dispatch runs again with the
// code as it is now (after the agent's hot patch).

/// boundaries may be set (makepad checks it through `relax_retry_on`): on while watched
#[no_mangle]
pub static RELAX_RETRY_ON: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

thread_local! {
    /// retry boundaries open on this thread (their unit names, innermost last)
    static BOUNDARIES: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
    /// the panic unwinding now was resumed with `retry`
    static RETRY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// makepad: a retry boundary begins (`name`: what re-runs, e.g. the event's name).
pub extern "C" fn relax_retry_enter(name: *const u8, len: usize) {
    let _kept = crate::zone_alloc::kept();
    let n = unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(name, len)) }.to_string();
    let _ = BOUNDARIES.try_with(|b| b.borrow_mut().push(n));
}

pub extern "C" fn relax_retry_leave() {
    let _ = BOUNDARIES.try_with(|b| b.borrow_mut().pop());
}

/// makepad, after catching a panic at its boundary: run the unit again? (clears the request)
pub extern "C" fn relax_retry_take() -> bool {
    RETRY.try_with(|r| r.replace(false)).unwrap_or(false)
}

fn innermost_boundary() -> Option<String> {
    BOUNDARIES.try_with(|b| b.borrow().last().cloned()).ok().flatten()
}

static PARKED: Mutex<Vec<std::sync::Arc<Parked>>> = Mutex::new(Vec::new());
static RESUME: Condvar = Condvar::new();
static PANIC_IDS: AtomicU64 = AtomicU64::new(0);
/// sites that parked already with nobody watching (the next panic there does not wait again)
static SITES_SEEN: Mutex<Vec<(u64, String)>> = Mutex::new(Vec::new());

/// The run is one no one attends (`makepad run --unattended`): no panic parks, not even briefly.
static UNATTENDED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Marks this run as one no one attends ([`UNATTENDED`]).
pub fn unattended() {
    UNATTENDED.store(true, Relaxed);
}

fn park_secs_unwatched() -> u64 {
    std::env::var("RELAX_PANIC_PARK").ok().and_then(|v| v.parse().ok()).unwrap_or(2)
}

/// Who can decide a panic now, including a person reading an interactive terminal.
fn decider(domain: Option<crate::domain::DomainId>) -> Option<&'static str> {
    if WATCHERS.load(SeqCst) > 0 {
        Some("an inspect watcher")
    } else if super::AGENT.load(Relaxed) {
        Some("an agent")
    } else if OBSERVERS.load(SeqCst) > 0 {
        Some("devhub")
    } else if RUN_STATE_AVAILABLE.load(SeqCst) && domain.is_some()
        && domain == crate::run::app_domain() && crate::run::live_session()
    {
        Some("the Run panel")
    } else if std::io::stdin().is_terminal() && crate::tui::stderr_is_terminal() {
        Some("this terminal")
    } else {
        None
    }
}

/// A panic is about to unwind on this (program) thread: park it if someone may want to look
/// (jit.rs raise_with; not on the owner, not without a recovery point).
/// The program frames of thread `os_id` when it is parked at a stale access (pc first, then
/// return addresses, as a capture gives them).
pub fn stale_frames_of(os_id: u64) -> Option<Vec<u64>> {
    let list = PARKED.lock().unwrap_or_else(|e| e.into_inner());
    let p = list.iter().find(|p| p.kind == ParkKind::Stale && p.os_id == os_id)?;
    Some(p.frames.iter().enumerate().map(|(i, &f)| if i == 0 { f } else { f + 1 }).collect())
}

/// True when `os_id` is a thread parked at a panic.
pub fn panic_parked(os_id: u64) -> bool {
    PARKED.lock().unwrap_or_else(|e| e.into_inner()).iter().any(|p| p.os_id == os_id)
}

/// A quit was asked for (a signal, the app's own quit): nothing parked waits for a decision any
/// more, whatever its domain -- each ends as its domain's stop ends it (`release_domain`), a stale
/// access's report resumes the program it paused -- and a program paused for an inspection goes
/// on: the quit waits for the app's main to return, and a paused main never did (a quit waited
/// forever behind a stale report's pause that named no domain, 11-08).
pub fn quit_releases() {
    for p in PARKED.lock().unwrap_or_else(|e| e.into_inner()).iter() {
        let _ = p.decision.compare_exchange(0, 1, SeqCst, SeqCst);
    }
    RESUME.notify_all();
    super::instrument::quit_resumes();
}

/// A domain is stopping: let its parked panics unwind so its ordinary cleanup can run.
pub fn release_domain(domain: crate::domain::DomainId) {
    for p in PARKED.lock().unwrap_or_else(|e| e.into_inner()).iter().filter(|p| p.domain == Some(domain)) {
        p.decision.store(1, SeqCst);
    }
    RESUME.notify_all();
}

pub fn on_panic(info: &crate::jit::PanicInfo, regs: *const u64) {
    let unwinds = !regs.is_null();
    let _k = crate::zone_alloc::kept();
    if !super::STARTED.load(Relaxed) || super::TEST_RUN.load(Relaxed) || UNATTENDED.load(Relaxed) || crate::owner::is_owner() || super::probes::in_probe() || super::eval::in_eval() {
        return;
    }
    let domain = crate::domain::current_app_domain();
    if crate::hot_failure::replacement_running() && domain == crate::run::app_domain()
        && crate::hot_failure::restart_failed(&format!("{}: {} at {}", info.kind, info.message, info.location))
    {
        // Recovery is an infrastructure attempt. Publish its failure to the
        // watcher and unwind; parking it would ask the agent to repair the JIT.
        return;
    }
    // a call of a stub whose build hit an internal compiler error, built off this thread or
    // panicking in its compile: a failed hot load like a call's own (jit.rs compile_ice), no panic
    // of the program to park. It parks only when no fresh process took over. In a restarted run
    // every such call is its JIT error's, the ones after its report included (another thread's
    // call of the stub met the error after the watcher reported it)
    if crate::jit::is_ice_stub_message(&info.message)
        && (!crate::hot_failure::has_error() || crate::hot_failure::replacement())
        && crate::jit::ice_at_call(&info.message)
    {
        // reported as the JIT's failure (a restarted run's): the call unwinds, no program panic parks
        return;
    }
    let deciding = decider(domain);
    let mut watched = deciding.is_some();
    let key = format!("{}|{}|{}", info.site, info.location, info.message);
    super::flightrec::panic(&super::short(&info.message, 120), if info.location.is_empty() { "?" } else { &info.location });
    if !watched {
        let mut seen = SITES_SEEN.lock().unwrap_or_else(|e| e.into_inner());
        if park_secs_unwatched() == 0 || seen.iter().any(|s| s.1 == key) {
            return;
        }
        seen.push((info.site, key.clone()));
    }
    let id = PANIC_IDS.fetch_add(1, SeqCst) + 1;
    let mut partial = Vec::new();
    let top: Vec<u64> = info.frames.iter().copied().take(12).collect();
    // panic frames are lookup pcs already (collect_frames subtracts one); symbolise wants the
    // first as is and the rest as return addresses
    let as_raw: Vec<u64> = top.iter().enumerate().map(|(i, &p)| if i == 0 { p } else { p + 1 }).collect();
    let frames = super::symbolise(&[as_raw], &mut partial).pop().unwrap_or_default();
    let location = if info.location.is_empty() { panic_site(info.site) } else { info.location.clone() };
    let fr: Vec<String> = frames.iter().map(super::frame_json).collect();
    let me = crate::os::thread::current();
    let thread = if me == crate::jit::MAIN_TID.load(Relaxed) { "main".to_string() } else { crate::os::thread::name(me) };
    let mut wait_s = if watched { 600 } else { park_secs_unwatched() };
    let primary = domain.is_some() && domain == crate::run::app_domain();
    let boundary = innermost_boundary();
    // a restartable frame (`restartable <fn>`) on the stack: restart / return are offered
    let restartable = if unwinds && info.kind == "panic" { super::instrument::restartable_in(&info.frames) } else { None };
    let fields = format!(
        "\"id\":\"P{}\",\"kind\":{},\"message\":{},\"location\":{},\"thread\":{},\"frames\":[{}]{},\"parked_s\":{},\"retry\":{}{},\"more\":[\"stacks\",\"why-hung\",{}{}\"resume P{} panic\"]",
        id,
        json_str(&info.kind),
        json_str(&super::short(&info.message, 400)),
        json_str(&location),
        json_str(&thread),
        fr.join(","),
        super::script_stack_field("script_stack", me),
        wait_s,
        match &boundary {
            Some(b) => format!("{{\"reruns\":{},\"note\":\"side effects since it began may repeat\"}}", json_str(b)),
            None => "null".into(),
        },
        if primary { format!(",\"restart\":{{\"domain\":{},\"window_kept\":{}}}", domain.unwrap(), crate::run::hosted_run()) } else { match &restartable {
            Some((_, name, rets)) => format!(
                ",\"restart\":{{\"fn\":{},\"return_words\":{}}}",
                json_str(&super::short(name, 160)),
                rets.map_or("null".to_string(), |n| n.to_string())
            ),
            None => String::new(),
        } },
        if boundary.is_some() { format!("\"resume P{} retry\",", id) } else { String::new() },
        match &restartable {
            Some((_, _, Some(_))) => format!("\"resume P{} restart\",\"resume P{} return <value>\",", id, id),
            Some(_) => format!("\"resume P{} restart\",", id),
            None if primary => format!("\"resume P{} restart\",", id),
            None => String::new(),
        },
        id
    );
    let mut list = PARKED.lock().unwrap_or_else(|e| e.into_inner());
    // Stop may have released the old parked list while this panic collected its stack.
    // Do not park a new cleanup panic after that release.
    if domain.is_some_and(crate::domain::cancelled) { return; }
    let (seq, event) = emit_record("panic", &format!("P{}", id), &fields);
    let p = std::sync::Arc::new(Parked { id, seq, os_id: super::threads::self_os_id(), domain, decision: AtomicU32::new(0), words: Mutex::new(Vec::new()), event, what: (thread.clone(), info.message.clone(), location.clone()), frames: info.frames.iter().copied().take(64).collect(), regs: if unwinds { unsafe { std::slice::from_raw_parts(regs, crate::jit::REG_WORDS) }.to_vec() } else { Vec::new() }, kind: ParkKind::Panic });
    list.push(p.clone());
    parked_changed();
    // Unconditional: --quiet/--silent do not filter stderr, and the session-log tee forwards
    // every complete line to its saved real stderr even when the progress bar is disabled.
    let tool = crate::tool_name();
    let pid = std::process::id();
    let thread_line = thread.replace(['\n', '\r'], " ");
    let message = super::short(&info.message, 120).replace(['\n', '\r'], " ");
    if let Some(who) = deciding {
        eprintln!("relax: panic parked on thread {thread_line} ({message}) for {wait_s} s ({who} can decide): {tool} inspect {pid} panics, then resume P{id} panic; rerun with --unattended to never park");
    } else {
        eprintln!("relax: panic parked on thread {thread_line} ({message}) for {wait_s} s (nothing attached): {tool} inspect {pid} panics within that time, or rerun with --unattended");
    }
    let t0 = std::time::Instant::now();
    loop {
        // Check before expiry so an attachment during the last wait can still decide. Once
        // someone could decide, detaching never shortens the park or clears the watched state.
        if !watched {
            if let Some(who) = decider(domain) {
                watched = true;
                wait_s = 600;
                eprintln!("relax: panic P{id} on thread {thread_line}: {who} attached; park extended to {wait_s} s from its start");
            }
        }
        if p.decision.load(SeqCst) != 0 || t0.elapsed().as_secs() >= wait_s || crate::run_threads::run_ending() {
            break;
        }
        list = RESUME.wait_timeout(list, std::time::Duration::from_millis(200)).unwrap_or_else(|e| e.into_inner()).0;
    }
    list.retain(|x| x.id != id);
    drop(list);
    parked_changed();
    let decision = p.decision.load(SeqCst);
    super::flightrec::panic_decided(id, ["timed out: unwinds", "panic", "retry", "restart", "return"].get(decision as usize).copied().unwrap_or("?"));
    if decision == 2 && boundary.is_some() {
        let _ = RETRY.try_with(|r| r.set(true));
    }
    if restartable.is_none() && unwinds && watched && info.kind == "panic" {
        super::instrument::auto_restartable(&info.frames);
    }
    if let (3 | 4, Some((slot, _, rets))) = (decision, &restartable) {
        let words = std::mem::take(&mut *p.words.lock().unwrap_or_else(|e| e.into_inner()));
        if decision == 3 || rets.is_some() {
            super::instrument::set_pending(super::instrument::Pending { slot: *slot, mode: if decision == 3 { 1 } else { 2 }, words });
        }
    }
}

/// Somebody could answer a parked stop now: the Run panel, an inspect watcher, an agent, devhub.
/// A terminal alone cannot justify a stale access's park, which has no timeout. A test run's stop
/// is the test's, as its panics are (`makepad test`: a fixture that asserts its child dies parked
/// under devhub's observer and paused the whole job, J13263, 11-13), and an unattended run has
/// nobody by definition: both end with the report.
pub fn can_decide() -> bool {
    super::STARTED.load(Relaxed)
        && !super::TEST_RUN.load(Relaxed)
        && !UNATTENDED.load(Relaxed)
        && (WATCHERS.load(SeqCst) > 0 || super::AGENT.load(Relaxed) || OBSERVERS.load(SeqCst) > 0 || RUN_STATE_AVAILABLE.load(SeqCst))
}

/// What the user decided for a parked stale access.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StaleDecision {
    /// the process ends by the fault's signal
    Stop,
    /// the run starts again with its command line
    Restart,
    /// the run ends meanwhile (a restart or stop taken elsewhere: the ring, the WM's toast): no
    /// second restart
    Ending,
}

/// A stale access (stale.rs), parked on behalf of thread `os_id` (it waits in its fault handler
/// for good): listed and announced as a parked panic -- `panics`, `watch`, the Run panel -- with the
/// choices `restart` (the process starts again) and `stop`. Waits for the decision without a
/// timeout.
pub fn park_stale(os_id: u64, domain: Option<crate::domain::DomainId>, message: &str, frames: &[u64]) -> StaleDecision {
    let (id, decision) = park_for_restart(ParkKind::Stale, "stale access", os_id, domain, message, frames,
        |id| format!("relax: stale access parked as P{} -- {tool} inspect {} panics; resume P{} restart | stop", id, std::process::id(), id, tool = crate::tool_name()));
    let d = match decision { 3 => StaleDecision::Restart, 5 => StaleDecision::Stop, _ => StaleDecision::Ending };
    super::flightrec::panic_decided(id, match d { StaleDecision::Stop => "stale: stop", StaleDecision::Restart => "stale: restart", StaleDecision::Ending => "stale: the run ended" });
    d
}

/// How a call parked at a failed live update ended (park_update_failed).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UpdateParked {
    /// the person chose to restart: the next run starts with the update
    Restart,
    /// the person stopped the app
    Stop,
    /// the run ends meanwhile (a restart taken elsewhere: the ring, the WM's toast)
    Ending,
}

/// A program thread's call whose code the live update could not build (an internal compiler error
/// at the call after live edits, jit.rs ice_at_call): the thread waits at the call -- the window
/// and the app's state stay -- listed and announced as a parked panic (`panics`, `watch`, the
/// paused view) with `message` saying so, and the choices `restart` and `stop`. No timeout: the app
/// is never restarted by itself.
pub fn park_update_failed(message: &str, frames: &[u64]) -> UpdateParked {
    let (id, decision) = park_for_restart(ParkKind::UpdateFailed, "update failed", super::threads::self_os_id(), crate::domain::current_app_domain(), message, frames,
        |id| format!("relax: {} -- parked as P{} (the app holds still, its state kept): {tool} inspect {} panics; resume P{} restart | stop", super::short(message, 200), id, std::process::id(), id, tool = crate::tool_name()));
    let d = match decision { 3 => UpdateParked::Restart, 5 => UpdateParked::Stop, _ => UpdateParked::Ending };
    super::flightrec::panic_decided(id, match d { UpdateParked::Restart => "update failed: restart", UpdateParked::Stop => "update failed: stop", UpdateParked::Ending => "update failed: the run ended" });
    d
}

/// A thread that cannot go on, parked on behalf of thread `os_id` until the person decides (no
/// timeout): `restart` (3) or `stop` (5); 0 when the run ends first. `kind` names it in the event.
fn park_for_restart(kind: ParkKind, label: &str, os_id: u64, domain: Option<crate::domain::DomainId>, message: &str, frames: &[u64], announce: impl FnOnce(u64) -> String) -> (u64, u32) {
    let _k = crate::zone_alloc::kept();
    let id = PANIC_IDS.fetch_add(1, SeqCst) + 1;
    let top: Vec<u64> = frames.iter().copied().take(12).collect();
    let as_raw: Vec<u64> = top.iter().enumerate().map(|(i, &p)| if i == 0 { p } else { p + 1 }).collect();
    let mut partial = Vec::new();
    let sym = super::symbolise_within(&[as_raw], &mut partial, 500).pop().unwrap_or_default();
    let location = sym.first().map(|f| f.at.clone()).unwrap_or_default();
    let fr: Vec<String> = sym.iter().map(super::frame_json).collect();
    // (the UI thread is "main", as a parked panic names it: the paused view leads with its choices)
    let main = os_id == super::threads::self_os_id() && crate::os::thread::current() == crate::jit::MAIN_TID.load(Relaxed);
    let thread = if main { "main".to_string() } else { super::threads::list().into_iter().find(|t| t.os_id == os_id).map(|t| t.name.clone()).unwrap_or_else(|| os_id.to_string()) };
    let fields = format!(
        "\"id\":\"P{}\",\"kind\":{},\"message\":{},\"location\":{},\"thread\":{},\"frames\":[{}],\"parked_s\":null{},\"more\":[\"stacks\",\"why-hung\",{}\"resume P{} stop\"]",
        id,
        json_str(label),
        json_str(&super::short(message, 400)),
        json_str(&location),
        json_str(&thread),
        fr.join(","),
        format!(",\"restart\":{{\"window_kept\":{},\"state_kept\":false}}", crate::run::hosted_run()),
        format!("\"resume P{} restart\",", id),
        id
    );
    let mut list = PARKED.lock().unwrap_or_else(|e| e.into_inner());
    let (seq, event) = emit_record("panic", &format!("P{}", id), &fields);
    let p = std::sync::Arc::new(Parked { id, seq, os_id, domain, decision: AtomicU32::new(0), words: Mutex::new(Vec::new()), event, what: (thread, message.to_string(), location), frames: frames.iter().copied().take(64).collect(), regs: Vec::new(), kind });
    list.push(p.clone());
    parked_changed();
    eprintln!("{}", announce(id));
    while p.decision.load(SeqCst) == 0 && !crate::run_threads::run_ending() {
        list = RESUME.wait_timeout(list, std::time::Duration::from_millis(500)).unwrap_or_else(|e| e.into_inner()).0;
    }
    list.retain(|x| x.id != id);
    drop(list);
    parked_changed();
    (id, p.decision.load(SeqCst))
}

/// How a call parked at a compile error ended.
pub enum CompileParked {
    /// a live patch fixed the callee: its code
    Fixed(u64),
    /// the user stopped the run
    Stop,
    /// its domain is stopping: the call fails as the error's panic, so the domain's cleanup runs
    Unwind,
}

/// A call of code that did not compile (`makepad run`, jit.rs park_compile_error): the calling
/// thread waits at the call -- nothing unwound, its arguments kept -- listed and announced like a
/// parked panic (`panics`, `watch`, the Run panel) with the one choice `stop` (the user may end
/// the run). After every live patch (the patch epoch moved) `rebuild` compiles the callee again:
/// once it builds, the call goes on with that code. No timeout: a compile error never ends the
/// program by itself (the user's rule, 2026-11-02).
pub fn park_compile_error(message: &str, location: &str, frames: &[u64], mut rebuild: impl FnMut() -> Option<u64>) -> CompileParked {
    let _k = crate::zone_alloc::kept();
    let id = PANIC_IDS.fetch_add(1, SeqCst) + 1;
    let domain = crate::domain::current_app_domain();
    let as_raw: Vec<u64> = frames.iter().copied().take(12).enumerate().map(|(i, p)| if i == 0 { p } else { p + 1 }).collect();
    let mut partial = Vec::new();
    let sym = super::symbolise_within(&[as_raw], &mut partial, 500).pop().unwrap_or_default();
    let fr: Vec<String> = sym.iter().map(super::frame_json).collect();
    let me = crate::os::thread::current();
    let thread = if me == crate::jit::MAIN_TID.load(Relaxed) { "main".to_string() } else { crate::os::thread::name(me) };
    let fields = format!(
        "\"id\":\"P{}\",\"kind\":\"compile error\",\"message\":{},\"location\":{},\"thread\":{},\"frames\":[{}],\"parked_s\":null,\"resumes\":\"a live edit that makes it compile\",\"more\":[\"stacks\",\"resume P{} stop\"]",
        id,
        json_str(&super::short(message, 400)),
        json_str(location),
        json_str(&thread),
        fr.join(","),
        id
    );
    let mut list = PARKED.lock().unwrap_or_else(|e| e.into_inner());
    let (seq, event) = emit_record("panic", &format!("P{}", id), &fields);
    let p = std::sync::Arc::new(Parked { id, seq, os_id: super::threads::self_os_id(), domain, decision: AtomicU32::new(0), words: Mutex::new(Vec::new()), event, what: (thread.clone(), message.to_string(), location.to_string()), frames: frames.iter().copied().take(64).collect(), regs: Vec::new(), kind: ParkKind::CompileError });
    list.push(p.clone());
    parked_changed();
    super::flightrec::panic(&super::short(message, 120), if location.is_empty() { "?" } else { location });
    // Unconditional, like a panic's park line: a run nobody attends (a script, CI) learns here why it
    // waits and that --unattended ends it at the error instead.
    let tool = crate::tool_name();
    eprintln!("relax: compile error parked on thread {} at its call of code that did not compile: edit the code (a live edit that makes it compile resumes the call), or stop: {tool} inspect {} panics (resume P{} stop); rerun with --unattended to end the run at a compile error instead", thread, std::process::id(), id);
    let mut epoch = crate::jit::patch_epoch();
    let end = loop {
        match p.decision.load(SeqCst) {
            0 if crate::run_threads::run_ending() => break CompileParked::Unwind,
            0 => {}
            5 => break CompileParked::Stop,
            _ => break CompileParked::Unwind,
        }
        list = RESUME.wait_timeout(list, std::time::Duration::from_millis(200)).unwrap_or_else(|e| e.into_inner()).0;
        let now = crate::jit::patch_epoch();
        if now != epoch && p.decision.load(SeqCst) == 0 {
            epoch = now;
            drop(list);
            let fixed = rebuild();
            list = PARKED.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(a) = fixed { break CompileParked::Fixed(a); }
        }
    };
    list.retain(|x| x.id != id);
    drop(list);
    parked_changed();
    super::flightrec::panic_decided(id, match end { CompileParked::Fixed(_) => "compile error: fixed by a live edit", CompileParked::Stop => "compile error: stop", CompileParked::Unwind => "compile error: domain stopping" });
    end
}

/// "file:line:col" of a compiler-known panic site.
fn panic_site(site: u64) -> String {
    let up = crate::jit::current_unit();
    if up.is_null() || site == u64::MAX {
        return String::new();
    }
    crate::owner::call_within(crate::owner::Class::Job, 300, move || {
        let u = unsafe { &*up };
        let s = u.sites.lock().unwrap_or_else(|e| e.into_inner()).get(site as usize).map(|st| (st.file, st.pos));
        s.map(|(f, p)| u.pos_str(f, p)).unwrap_or_default()
    })
    .unwrap_or_default()
}

/// The panics parked now: (id, thread os id, thread name, message, location), oldest first.
pub fn parked_now() -> Vec<(u64, u64, String, String, String)> {
    PARKED.lock().unwrap_or_else(|e| e.into_inner()).iter().map(|p| (p.id, p.os_id, p.what.0.clone(), p.what.1.clone(), p.what.2.clone())).collect()
}

pub fn cmd_panics(out: &mut Out) {
    let list = PARKED.lock().unwrap_or_else(|e| e.into_inner());
    out.measured("parked panics", "event queue", &[]);
    for p in list.iter() {
        out.line(&p.event);
    }
}

/// `resume <Pn> panic`: the parked panic unwinds as usual.
pub fn cmd_resume(req: &Req, out: &mut Out) {
    let Some(id) = req.pos.first().and_then(|s| s.trim_start_matches('P').parse::<u64>().ok()) else {
        out.err("usage: resume <panic id> panic");
        return;
    };
    let how = req.pos.get(1).map(String::as_str).unwrap_or("panic");
    let decision = match how {
        "panic" => 1,
        "retry" => 2,
        "restart" => 3,
        "return" => 4,
        "stop" => 5,
        _ => {
            out.err("resume <P<n>> panic | retry | restart | return <value>[,<value>..] | stop (a stale access)");
            return;
        }
    };
    // return values: integers (decimal, 0x hex, negative), true / false, separated by commas
    let mut words = Vec::new();
    if decision == 4 {
        let text: String = req.pos.iter().skip(2).cloned().collect::<Vec<_>>().join(",");
        for w in text.split(',').map(str::trim).filter(|w| !w.is_empty()) {
            let v = match w {
                "true" => Some(1u64),
                "false" => Some(0),
                _ if w.starts_with("0x") => u64::from_str_radix(&w[2..], 16).ok(),
                _ => w.parse::<i64>().ok().map(|x| x as u64).or_else(|| w.parse::<u64>().ok()),
            };
            match v {
                Some(v) => words.push(v),
                None => {
                    out.err(&format!("cannot read `{}` as a return value (integers, 0x.., true, false)", w));
                    return;
                }
            }
        }
    }
    let list = PARKED.lock().unwrap_or_else(|e| e.into_inner());
    match list.iter().find(|p| p.id == id) {
        Some(p) if p.kind == ParkKind::Stale && decision != 3 && decision != 5 => out.err(&format!("P{} is a stale access: its thread cannot go on (resume P{} restart | stop)", id, id)),
        Some(p) if p.kind == ParkKind::UpdateFailed && decision != 3 && decision != 5 => out.err(&format!("P{} is a call the live update could not build: its thread cannot go on (resume P{} restart | stop)", id, id)),
        Some(p) if p.kind == ParkKind::CompileError && decision != 5 => out.err(&format!("P{} is a call of code that did not compile: a live edit that fixes it resumes it (resume P{} stop ends the run)", id, id)),
        Some(p) if p.kind == ParkKind::Panic && decision == 5 => out.err(&format!("P{} is a panic: stop is for stale accesses and compile errors", id)),
        Some(p) if matches!(p.kind, ParkKind::Stale | ParkKind::UpdateFailed) && decision == 3 => {
            // the thread it parked for starts the next run (stale.rs restart_run, jit.rs ice_at_call)
            p.decision.store(3, SeqCst);
            RESUME.notify_all();
            out.line(&format!("{{\"resumed\":\"P{}\",\"as\":\"restart\",\"window_kept\":{},\"state_kept\":false}}", id, crate::run::hosted_run()));
        }
        Some(p) => {
            if decision == 3 && p.domain.is_some() && p.domain == crate::run::app_domain() {
                // the app's own panic: the next run starts (after this reply went out)
                let why = format!("resume P{id} restart");
                let spawned = crate::run_threads::spawn("resume-restart", move || {
                    std::thread::sleep(std::time::Duration::from_millis(200));
                    if let Err(e) = crate::reload::restart_run(&why) {
                        crate::hot_failure::restart_failed(&e);
                        eprintln!("relax: {why}: {e}");
                    }
                });
                match spawned {
                    Ok(_) => out.line(&format!("{{\"resumed\":\"P{}\",\"as\":\"restart\",\"window_kept\":{},\"state_kept\":false}}", id, crate::run::hosted_run())),
                    Err(e) => out.err(&format!("cannot start the restart: {e}")),
                }
                return;
            }
            *p.words.lock().unwrap_or_else(|e| e.into_inner()) = words;
            p.decision.store(decision, SeqCst);
            RESUME.notify_all();
            out.line(&format!("{{\"resumed\":\"P{}\",\"as\":\"{}\"}}", id, how));
        }
        None => out.err(&format!("no parked panic P{}", id)),
    }
}

/// A run's end began: what is parked stops waiting for a decision.
pub(crate) fn run_ends() {
    RESUME.notify_all();
    super::instrument::run_ends();
    super::pstream::run_ends();
}

// ------------------------------------------------------------------ detectors

/// The detector thread: hangs (the main thread blocked in a registry wait past its threshold),
/// deadlocks (a wait cycle seen twice in a row), memory growth (RSS). Four checks a second, each
/// a registry scan and one resident-size read: nothing the program pays for.
pub fn start() {
    retry_update();
    { let _k = crate::zone_alloc::kept();
    let _ = crate::run_threads::spawn("run-inspect-watch", || {
        super::heap::exclude_this_thread();
        let hang_ms: u64 = std::env::var("RELAX_INSPECT_HANG_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(3000);
        let mut last_cycle: Vec<u64> = Vec::new();
        let mut last_rows: Vec<(u64, u64, u64)> = Vec::new();
        let mut base_rss = 0u64;
        let mut tick = 0u64;
        let (mut marked_main, mut marked_present) = (false, false);
        // the main thread outside any registry wait, no wait cycle, the resident size unchanged:
        // one look a second (the tick counts quarter seconds: it moves to the next whole second)
        let (mut idle, mut last_rss) = (false, 0u64);
        while crate::run_threads::sleep(std::time::Duration::from_millis(if idle { 1000 } else { 250 })) {
            tick = if idle { (tick / 4 + 1) * 4 } else { tick + 1 };
            let waits = crate::waits::snapshot();
            let main = crate::jit::MAIN_TID.load(Relaxed);
            let now = crate::clock::wall_ns();
            // the main thread waiting, in a program whose main runs an event loop (it presented a
            // frame): on the owner (a compile stall), on a lock / condvar / join. A test or bench
            // run's main waits on its work by design: its hangs are its verdict's, not the inbox's
            let judged = crate::jit::FIRST_PRESENT.load(Relaxed) && !super::TEST_RUN.load(Relaxed);
            if let Some(w) = waits.iter().find(|w| judged && w.thread == main && main != 0) {
                let ms = now.saturating_sub(w.since_ns) / 1_000_000;
                let relevant = match w.kind {
                    crate::waits::Kind::Owner => true,
                    crate::waits::Kind::Futex => w.label != "park" && w.label != "condvar" || ms > hang_ms * 4,
                    crate::waits::Kind::None => false,
                    _ => true,
                };
                if relevant && ms >= hang_ms {
                    let mut list = super::threads::list();
                    super::name_handles(&mut list);
                    let what = super::wait_text(w, &list);
                    emit("hang", &format!("main|{:?}|{:#x}", w.kind, w.object), &format!("\"thread\":\"main\",\"blocked_ms\":{},\"waits\":{},\"more\":[\"why-hung\",\"stacks thread=main\"]", ms, json_str(&what)));
                }
            }
            // a cycle among registry waits (thread -> holder handle)
            let cycle = super::hang::registry_cycle(&waits);
            // each cycle thread's row: a deadlock is the same waits in both looks, not a new short
            // wait each time that happens to close a cycle again
            let rows: Vec<(u64, u64, u64)> = cycle.iter().filter_map(|h| waits.iter().find(|w| w.thread == *h).map(|w| (w.thread, w.object, w.since_ns))).collect();
            if !cycle.is_empty() && cycle == last_cycle && rows == last_rows {
                let mut list = super::threads::list();
                super::name_handles(&mut list);
                let names: Vec<String> = cycle.iter().map(|h| json_str(&super::thread_label(*h, &list))).collect();
                let what: Vec<String> = cycle.iter().filter_map(|h| waits.iter().find(|w| w.thread == *h)).map(|w| json_str(&format!("{} waits: {}", super::thread_label(w.thread, &list), super::wait_text(w, &list)))).collect();
                let mut key = cycle.clone();
                key.sort_unstable();
                emit("deadlock", &format!("{:?}", key), &format!("\"cycle\":[{}],\"waits\":[{}],\"more\":[\"why-hung\"]", names.join(","), what.join(",")));
            }
            last_cycle = cycle;
            last_rows = rows;
            super::heap::expire();
            // the memory timeline and its phase markers
            let rss = crate::memstat::resident();
            idle = last_cycle.is_empty() && rss == last_rss && crate::jit::FIRST_PRESENT.load(Relaxed) && !waits.iter().any(|w| w.thread == main);
            last_rss = rss;
            super::mem::tick(tick, rss);
            if !marked_main && main != 0 {
                marked_main = true;
                super::mem::mark("main");
            }
            if !marked_present && crate::jit::FIRST_PRESENT.load(Relaxed) {
                marked_present = true;
                super::mem::mark("first present");
            }
            // memory: resident size against the level 5 s after the first present
            if tick % 8 == 0 && crate::jit::FIRST_PRESENT.load(Relaxed) {
                if base_rss == 0 {
                    base_rss = rss;
                } else if rss > base_rss + (base_rss / 2).max(512 << 20) {
                    emit("mem", "rss", &format!("\"rss_mb\":{},\"from_mb\":{},\"more\":[\"memory\"]", rss >> 20, base_rss >> 20));
                    base_rss = rss;
                }
            }
        }
    });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_recent_events_end_at_the_newest_and_arrive_whole() {
        // twelve 3 KB events: more than the 16 KB a reply was once cut at (it kept the oldest,
        // and devhub took a 30 s old verdict for the edit it had just shipped, 11-04)
        let pad = "x".repeat(3 << 10);
        let seqs: Vec<u64> = (0..12).map(|n| emit("cuttest", &format!("cut{n}"), &format!("\"pad\":\"{pad}\""))).collect();
        let shown = |line: &str| -> Vec<u64> {
            let mut buf = Vec::new();
            cmd_events(&Req::parse(line), &mut Out::new(&mut buf));
            let text = String::from_utf8(buf).unwrap();
            text.lines().filter_map(|l| l.split("\"seq\":").nth(1)?.split(',').next()?.parse().ok()).collect()
        };
        let recent = shown("events on=cuttest");
        assert_eq!(recent.last(), seqs.last(), "the recent events end at the newest");
        assert!(recent.windows(2).all(|w| w[0] < w[1]), "{recent:?}");
        assert_eq!(shown(&format!("events on=cuttest since={}", seqs[0] - 1)), seqs, "every event arrives");
    }

    /// A call the live update could not build (an internal compiler error at a call after a live
    /// edit) in an app the person runs: recorded and offered, no restart started, and the call
    /// parks for the person -- listed with its message and the choices restart and stop, with no
    /// timeout -- until they choose. (A landing restarted the person's running app in place.)
    #[test]
    fn a_call_the_update_could_not_build_parks_for_the_person() {
        isolated_test!(a_call_the_update_could_not_build_parks_for_the_person);
        let home = std::env::temp_dir().join(format!("relax-update-failed-{}", std::process::id()));
        std::fs::create_dir_all(&home).unwrap();
        std::env::set_var("MAKEPAD_HOME", &home);
        std::env::set_var("RELAX_LIVE_LOG", home.join("live.jsonl"));
        crate::hot_failure::enable();
        crate::hot_failure::edit_ran_for_test();
        let ice = "internal compiler error compiling `f` (slot 7) on its first call: test".to_string();
        assert!(crate::reload::update_failed_at_call(&ice), "after a live edit the update failed in the call");
        assert!(crate::reload::restart_pending().contains(&ice), "the restart is offered: {:?}", crate::reload::restart_pending());
        assert!(!crate::reload::RESTART_CONFIRMED.load(SeqCst), "nobody chose a restart: none is under way");
        assert!(!crate::run_threads::ending(), "the run goes on");
        let why = "The live update failed in this call: test".to_string();
        let caller = std::thread::spawn(move || park_update_failed(&why, &[]));
        let t0 = std::time::Instant::now();
        while parked_now().is_empty() && t0.elapsed().as_secs() < 10 { std::thread::sleep(std::time::Duration::from_millis(10)); }
        let (id, event) = { let list = PARKED.lock().unwrap(); let p = list.first().expect("the call parked"); (p.id, p.event.clone()) };
        for part in ["\"kind\":\"update failed\"", "\"message\":\"The live update failed in this call: test\"", "\"parked_s\":null", &format!("\"resume P{id} restart\""), &format!("\"resume P{id} stop\"")] {
            assert!(event.contains(part), "{part} in {event}");
        }
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(!caller.is_finished(), "it waits for the person's choice");
        let resume = |words: &str| { let mut buf = Vec::new(); cmd_resume(&Req::parse(words), &mut Out::new(&mut buf)); String::from_utf8(buf).unwrap() };
        assert!(resume(&format!("resume P{id} panic")).contains("could not build"), "its thread cannot go on: no unwind into the app");
        assert!(resume(&format!("resume P{id} restart")).contains("\"as\":\"restart\""));
        assert_eq!(caller.join().unwrap(), UpdateParked::Restart, "the restart is its thread's to start");
        let _ = std::fs::remove_dir_all(&home);
    }

    /// A stale access parked while its run ends (a restart or stop taken elsewhere, its domain
    /// stopping) starts no restart of its own: the run's end is its outcome. (The ending run got a
    /// second restart from the stale watcher.)
    #[test]
    fn a_stale_access_parked_as_its_run_ends_starts_no_second_restart() {
        isolated_test!(a_stale_access_parked_as_its_run_ends_starts_no_second_restart);
        let home = std::env::temp_dir().join(format!("relax-stale-ending-{}", std::process::id()));
        std::fs::create_dir_all(&home).unwrap();
        std::env::set_var("MAKEPAD_HOME", &home);
        let parked = || { let t0 = std::time::Instant::now(); while parked_now().is_empty() && t0.elapsed().as_secs() < 10 { std::thread::sleep(std::time::Duration::from_millis(10)); } !parked_now().is_empty() };
        let caller = std::thread::spawn(|| park_stale(1, None, "stale access: test", &[]));
        assert!(parked(), "the stale access parked");
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(!caller.is_finished(), "it waits for the person's choice");
        crate::run_threads::run_ends();
        RESUME.notify_all();
        assert_eq!(caller.join().unwrap(), StaleDecision::Ending, "the run ends: no restart");
        assert!(parked_now().is_empty(), "the ended park leaves the list");
        crate::run_threads::begin_run();
        let domain: crate::domain::DomainId = 9;
        let caller = std::thread::spawn(move || park_stale(1, Some(domain), "stale access: test", &[]));
        assert!(parked(), "the stale access parked");
        release_domain(domain);
        assert_eq!(caller.join().unwrap(), StaleDecision::Ending, "its domain stops: no restart");
        // a quit ends one of no domain too, and a program paused for its report goes on: the quit
        // waits for the app's main (a park that named no domain outlived its domain's stop, and
        // the quit waited for the paused main forever, 11-08)
        let caller = std::thread::spawn(|| park_stale(1, None, "stale access: test", &[]));
        assert!(parked(), "the stale access parked");
        crate::inspect::instrument::pause_program();
        assert!(crate::inspect::instrument::paused(), "paused for the report");
        quit_releases();
        assert_eq!(caller.join().unwrap(), StaleDecision::Ending, "a quit: no restart");
        assert!(!crate::inspect::instrument::paused(), "a quit resumes the paused program");
        assert!(parked_now().is_empty(), "nothing stays parked at a quit");
        let _ = std::fs::remove_dir_all(&home);
    }
}

/// What this module holds of a run, as a fresh process has it (`run_end`). Nothing of the run is left that could see it.
pub(crate) unsafe fn reset_statics() {
    use relax_session::reset_static as reset;
    reset(&PARKED, Mutex::new(Vec::new()));
    reset(&SITES_SEEN, Mutex::new(Vec::new()));
}
