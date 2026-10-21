//! Events: what an agent must hear about without asking (DESIGN.md §2.1, BRIEF "no human in the
//! loop"): panics (parked at the panic point), hangs, deadlocks, memory growth. Each event is one
//! compact JSON line with a sequence number and drill-down commands. `watch` blocks until the next
//! one and returns it (an agent's background task then exits: its notification), `events
//! follow=1` streams them (one line each), and `makepad run --agent` prints them on stderr too.
//! Repeats of the same event (kind + key) within 30 s are counted, not sent again.
//!
//! Panic parking: a panic in JIT code on a program thread waits at the panic point (its stack
//! intact for `stacks`) while a watcher is attached or under `--agent`, until a client resumes it
//! or the wait times out; then it unwinds as usual. With nobody watching, the first panic at a site
//! parks briefly (MAKEPAD_RUN_PANIC_PARK seconds, default 2) and says how to attach.

use super::{json_str, now_ms, Out, Req};
use std::collections::VecDeque;
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

pub const KINDS: &[&str] = &["panic", "hang", "deadlock", "mem", "spike", "hitch", "rtstall", "scripterr", "patchrefused", "patchverified", "patchreverted"];

/// Records an event (`fields`: the kind-specific JSON members, without braces). Returns its seq,
/// or 0 when it repeated one sent in the last 30 s.
pub fn emit(kind: &'static str, key: &str, fields: &str) -> u64 {
    emit_record(kind, key, fields).0
}

fn emit_record(kind: &'static str, key: &str, fields: &str) -> (u64, std::sync::Arc<str>) {
    let _k = crate::zone_alloc::kept();
    let mut q = EVENTS.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(e) = q.iter().rev().find(|e| e.kind == kind && e.key == key) {
        if e.at.elapsed().as_secs() < 30 {
            return (0, e.json.clone());
        }
    }
    let seq = SEQ.fetch_add(1, SeqCst) + 1;
    let verified = crate::reload::observations_verified();
    let json = format!(
        "{{\"event\":\"{}\",\"seq\":{},\"pid\":{},\"app\":{},\"at_ms\":{},\"verified\":{},{}}}",
        kind,
        seq,
        std::process::id(),
        json_str(&super::APP.lock().unwrap_or_else(|e| e.into_inner())),
        now_ms(),
        verified,
        fields
    );
    if super::AGENT.load(Relaxed) {
        eprintln!("makepad-run event: {}", json);
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

/// Retry boundaries are worth setting while someone may resume a panic with `retry`.
fn retry_update() {
    MAKEPAD_RUN_RETRY_ON.store(watched() || RUN_STATE_AVAILABLE.load(Relaxed), Relaxed);
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
    let mut since = req.get("since").and_then(|s| s.parse().ok()).unwrap_or_else(|| SEQ.load(SeqCst).saturating_sub(20));
    out.measured("events", "event queue", &[]);
    if follow {
        // a stream is unbounded by design
        out.limit = usize::MAX;
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
    /// 4 make it return `words`
    decision: AtomicU32,
    words: Mutex<Vec<u64>>,
    event: std::sync::Arc<str>,
    /// (thread name, message, location): why-hung's verdict names the parked panic
    what: (String, String, String),
    /// its frames, innermost first (lookup pcs): `eval`'s default scope, `locals`
    frames: Vec<u64>,
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
pub static MAKEPAD_RUN_RETRY_ON: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

thread_local! {
    /// retry boundaries open on this thread (their unit names, innermost last)
    static BOUNDARIES: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
    /// the panic unwinding now was resumed with `retry`
    static RETRY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// makepad: a retry boundary begins (`name`: what re-runs, e.g. the event's name).
pub extern "C" fn relax_retry_enter(name: *const u8, len: usize) {
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

fn park_secs_unwatched() -> u64 {
    std::env::var("MAKEPAD_RUN_PANIC_PARK").ok().and_then(|v| v.parse().ok()).unwrap_or(2)
}

/// A panic is about to unwind on this (program) thread: park it if someone may want to look
/// (jit.rs raise_with; not on the owner, not without a recovery point).
/// True when `os_id` is a thread parked at a panic.
pub fn panic_parked(os_id: u64) -> bool {
    PARKED.lock().unwrap_or_else(|e| e.into_inner()).iter().any(|p| p.os_id == os_id)
}

/// A domain is stopping: let its parked panics unwind so its ordinary cleanup can run.
pub fn release_domain(domain: crate::domain::DomainId) {
    for p in PARKED.lock().unwrap_or_else(|e| e.into_inner()).iter().filter(|p| p.domain == Some(domain)) {
        p.decision.store(1, SeqCst);
    }
    RESUME.notify_all();
}

pub fn on_panic(info: &crate::jit::PanicInfo, unwinds: bool) {
    let _k = crate::zone_alloc::kept();
    if !super::STARTED.load(Relaxed) || crate::owner::is_owner() || super::probes::in_probe() || super::eval::in_eval() {
        return;
    }
    let domain = crate::domain::current_app_domain();
    let watched = WATCHERS.load(SeqCst) > 0 || super::AGENT.load(Relaxed)
        || (RUN_STATE_AVAILABLE.load(SeqCst) && domain.is_some() && domain == crate::run::app_domain());
    let key = format!("{}|{}|{}", info.site, info.location, info.message);
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
    let wait_s = if watched { 600 } else { park_secs_unwatched() };
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
        if primary { format!(",\"restart\":{{\"domain\":{},\"window_kept\":true}}", domain.unwrap()) } else { match &restartable {
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
    let p = std::sync::Arc::new(Parked { id, seq, os_id: super::threads::self_os_id(), domain, decision: AtomicU32::new(0), words: Mutex::new(Vec::new()), event, what: (thread.clone(), info.message.clone(), location.clone()), frames: info.frames.iter().copied().take(64).collect() });
    list.push(p.clone());
    parked_changed();
    if !watched {
        eprintln!(
            "makepad: panic parked on thread {} ({}) for {} s -- attach: makepad inspect {} panics (resume P{} panic)",
            thread,
            super::short(&info.message, 120),
            wait_s,
            std::process::id(),
            id
        );
    }
    let t0 = std::time::Instant::now();
    while p.decision.load(SeqCst) == 0 && t0.elapsed().as_secs() < wait_s {
        // an unwatched park ends early once nobody could still attach in time
        list = RESUME.wait_timeout(list, std::time::Duration::from_millis(200)).unwrap_or_else(|e| e.into_inner()).0;
    }
    list.retain(|x| x.id != id);
    drop(list);
    parked_changed();
    let decision = p.decision.load(SeqCst);
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
        _ => {
            out.err("resume <P<n>> panic | retry | restart | return <value>[,<value>..]");
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
        Some(p) => {
            if decision == 3 && p.domain.is_some() && p.domain == crate::run::app_domain() {
                match crate::run::relaunch() {
                    Ok(()) => {
                        p.decision.store(1, SeqCst);
                        RESUME.notify_all();
                        out.line(&format!("{{\"resumed\":\"P{}\",\"as\":\"domain relaunch\",\"window_kept\":true}}", id));
                    }
                    Err(e) => out.err(&e),
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

// ------------------------------------------------------------------ detectors

/// The detector thread: hangs (the main thread blocked in a registry wait past its threshold),
/// deadlocks (a wait cycle seen twice in a row), memory growth (RSS). Four checks a second, each
/// a registry scan and one resident-size read: nothing the program pays for.
pub fn start() {
    retry_update();
    { let _k = crate::zone_alloc::kept();
    let _ = std::thread::Builder::new().name("run-inspect-watch".into()).spawn(|| {
        super::heap::exclude_this_thread();
        let hang_ms: u64 = std::env::var("MAKEPAD_RUN_INSPECT_HANG_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(3000);
        let mut last_cycle: Vec<u64> = Vec::new();
        let mut base_rss = 0u64;
        let mut tick = 0u64;
        let (mut marked_main, mut marked_present) = (false, false);
        loop {
            std::thread::sleep(std::time::Duration::from_millis(250));
            tick += 1;
            let waits = crate::waits::snapshot();
            let main = crate::jit::MAIN_TID.load(Relaxed);
            let now = crate::clock::wall_ns();
            // the main thread waiting: on the owner after the first present (a compile stall),
            // on a lock / condvar / join any time
            if let Some(w) = waits.iter().find(|w| w.thread == main && main != 0) {
                let ms = now.saturating_sub(w.since_ns) / 1_000_000;
                let present = crate::jit::FIRST_PRESENT.load(Relaxed);
                let relevant = match w.kind {
                    crate::waits::Kind::Owner => present,
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
            if !cycle.is_empty() && cycle == last_cycle {
                let mut list = super::threads::list();
                super::name_handles(&mut list);
                let names: Vec<String> = cycle.iter().map(|h| json_str(&super::thread_label(*h, &list))).collect();
                let mut key = cycle.clone();
                key.sort_unstable();
                emit("deadlock", &format!("{:?}", key), &format!("\"cycle\":[{}],\"more\":[\"why-hung\"]", names.join(",")));
            }
            last_cycle = cycle;
            super::heap::expire();
            // the memory timeline and its phase markers
            let rss = crate::memstat::resident();
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
