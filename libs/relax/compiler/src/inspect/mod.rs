//! makepad inspect: a running program answers questions about itself (local/agent_state/fast-rust/
//! inspect/DESIGN.md). One thread per `makepad run` listens on a Unix socket next to a discovery file
//! in the temp dir; a client (`makepad inspect <pid|app|all> <command>`, or any agent speaking the
//! line protocol) sends one request per line and reads JSON lines back, ending with an `end` line.
//!
//! The basics never need the owner thread: thread lists and stacks come from the OS and the code
//! tables, waits from the lock-free wait registry, events from this module's own queue. Only names
//! and lines of JIT frames ask the owner (Job class: served inside its pumps too), with a short
//! timeout after which the reply says what it could not resolve.

pub mod client;
#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
pub mod cfi;
pub mod eval;
pub mod events;
pub mod gpuprof;
mod hang;
pub mod heap;
pub mod instrument;
pub mod patches;
pub mod probes;
pub mod mem;
mod prof;
pub mod timeline;
pub(crate) mod symbols;
pub mod threads;

#[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
pub mod cfi {
    //! (Linux x86_64 only: other targets walk host frames by their frame pointers)
    pub fn refresh() {}
}

use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use relax_session::sync::Mutex;

/// The kept-state scan (zone_scan.rs, MAKEPAD_RUN_ZONE_CHECK=1) of inspect's records that
/// outlive the compiler arena: instrumented and probed code calls into them after a settle swap.
pub(crate) fn scan_kept(a: &mut crate::jit::zone_scan::Acc) {
    instrument::scan_kept(a);
    patches::scan_kept(a);
    probes::scan_kept(a);
}

/// The inspect thread runs (a socket is listening).
static STARTED: AtomicBool = AtomicBool::new(false);
/// `makepad run --agent`: panics park, events also go to stderr.
pub static AGENT: AtomicBool = AtomicBool::new(false);
/// os id of the inspect threads (not captured: they are the ones asking)
static SERVER_TIDS: Mutex<Vec<u64>> = Mutex::new(Vec::new());
static APP: Mutex<String> = Mutex::new(String::new());

pub use makepad_remote::{json_str, now_ms, Out, Req};

/// The `watch` service's sink: every event also goes to clients following it over makepad-remote.
pub static WATCH: std::sync::OnceLock<makepad_remote::watch::WatchSink> = std::sync::OnceLock::new();

fn server_thread_enter() {
    SERVER_TIDS.lock().unwrap().push(threads::self_os_id());
    heap::exclude_this_thread();
}

fn server_thread_exit() {
    let me = threads::self_os_id();
    SERVER_TIDS.lock().unwrap().retain(|t| *t != me);
}

/// Starts the inspect thread for program `app` (once per process; MAKEPAD_RUN_INSPECT=0: off).
pub fn start(app: &str) {
    if std::env::var("MAKEPAD_RUN_INSPECT").map_or(false, |v| v == "0") || STARTED.swap(true, Relaxed) {
        return;
    }
    let _k = crate::zone_alloc::kept();
    *APP.lock().unwrap() = app.to_string();
    let pid = std::process::id();
    let log = crate::tui::session_log_path().map(|p| p.display().to_string()).unwrap_or_default();
    let extra = format!(",\"session_log\":{},\"run\":{},\"agent\":{}", json_str(&log), json_str(crate::jit::PROVENANCE), AGENT.load(Relaxed));
    let endpoint = makepad_remote::Endpoint::new(app);
    endpoint.set_hooks(makepad_remote::Hooks { enter: server_thread_enter, exit: server_thread_exit });
    endpoint.register_cmd(dispatch);
    // SPEC-V: every answer says whether the code it observed has its borrow-check verdict
    endpoint.set_end_fields(|s| s.push_str(if crate::reload::observations_verified() { ",\"verified\":true" } else { ",\"verified\":false" }));
    let _ = WATCH.set(endpoint.register_watch_with(events::remote_watcher_opened, events::remote_watcher_closed));
    serve_first_services(&endpoint);
    *ENDPOINT.lock().unwrap_or_else(|e| e.into_inner()) = Some(endpoint.clone());
    let sock = match endpoint.listen_unix(&extra) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("makepad-remote: cannot listen on {}: {}", makepad_remote::carrier::unix::paths(pid).0.display(), e);
            return;
        }
    };
    eprintln!("makepad-remote: pid {} app {} socket {} (makepad inspect {} help)", pid, app, sock.display(), pid);
    events::start();
    timeline::hitch_init();
    if AGENT.load(Relaxed) {
        timeline::set_frame_watch(true);
    }
    heap::start_at_load();
}

/// The commands: (name, arguments, what it answers).
pub const COMMANDS: &[(&str, &str, &str)] = &[
    ("session", "relaunch", "restart the app domain in this process, keeping its host window and GPU device"),
    ("why-hung", "[probe_ms=200]", "is it stuck and why: verdict (deadlock / blocked / busy / idle), the wait chain or cycle, stacks of the threads in it"),
    ("why-slow", "[s=3]", "where the time goes: ranked findings -- the main thread's CPU split (program / compiles / makepad run runtime / system), its hot fns, the owner's service, other busy threads"),
    ("prof", "[s=3] [hz=200] [threads=all|main|owner|program|compiler|<name>] [n=15] [save=1]", "sampling CPU profile of JIT and host frames: per-thread CPU, activity split, top fns by self time with their hottest line, inclusive fns; save=1 writes folded stacks"),
    ("timeline", "start [max_s=30] | stop | frames [n=30] | frame <n|slowest> | save", "dataflow timeline: record UI phases, owner service, waits and wakes; per frame the critical path, work vs critical path vs wall, longest waits and who ended them; save = Chrome trace JSON"),
    ("watch", "[on=panic,hang,...] [since=<seq>] [timeout_s=N]", "block until the next event (panic, hang, ...) and answer it as one JSON line; re-arm with since=<seq>"),
    ("events", "[follow=1] [since=<seq>] [on=...]", "the recent events (follow=1: stream one JSON line per new event)"),
    ("panics", "", "panics parked now, waiting for a decision"),
    ("resume", "<panic id> panic|retry|restart|return <v>[,<v>]", "decide a parked panic: unwind, retry, restart the hosted app domain (otherwise its restartable fn), or return the values"),
    ("memory", "[compiler=1] [n=24]", "where the memory is and why: RSS by mapping kind, malloc in use vs held free, makepad run's own parts, findings, growth since the last `memory`"),
    ("diff", "<snap a> <snap b|now>", "memory growth between two snapshots, largest first"),
    ("mem", "snap <name> | timeline [from_ms= to_ms=]", "keep a named memory snapshot; resident size over time with phase markers and the peak's phase"),
    ("heap", "start [rate=65536] [ttl_s=600] | top [by=live|alloc|freed|peak] | snap <name> | diff <a> <b|now> | path H<id> | stop | status", "program heap by allocation stack (JIT-patched allocator hooks, byte-sampled): live, allocated, freed; growth and leaks between snapshots"),
    ("top", "by=time|alloc|bytes|live [s=3] [n=15]", "the top functions by CPU time (prof) or allocation sites by allocated / live bytes (heap)"),
    ("frame", "<n|slowest>", "one frame of the last timeline: UI phases, critical path, waits and who ended them (alias of timeline frame)"),
    ("path", "<H<n>|R<n>>", "the full stack of a heap site id"),
    ("count", "<fn> [time=1]", "exact call counts (and inclusive time) of every slot of a fn: its code is rebuilt with probes and installed as a probe group G<n>"),
    ("break", "<fn>|<file>:<line>", "breakpoint: a hit parks the thread at the call (stack intact) and raises a `break` event K<n>"),
    ("restartable", "<fn>", "a panic through the fn can be resumed with `restart` (the fn runs again from its entry with its original arguments) or `return <value>` (the call returns the value; integer-class returns)"),
    ("probe", "add <fn|file:line> <probe.rs|code> [mutate=1] | list | read P<n> | rm <P<n>|all>", "injected Rust probes: your `fn probe(<the hooked fn's params>)` runs at the hook, compiled as a child module of the hooked fn's module (its private items, `use`s and `crate::` paths are in scope); std::relax::probe::{emit, count, hist, sample, pause} record for it. A fn is named by its path: `krate::Type::method`, `Type::method`, `crate::Type::method`, `<Type as Trait>::method`"),
    ("eval", "[in=<krate::module>] [panic=P<n>] [mutate=1] <expr>", "evaluates a Rust expression in a module's scope (default: a parked panic's innermost program frame) in the app's domain: its type and {:#?} (bytes when not Debug); reads only unless mutate=1"),
    ("static", "<krate::module::NAME> [debug=1]", "a static's address, type and bytes in the app's domain; a reference or pointer is followed to its target allocation, with whether what it describes fits there"),
    ("locals", "[panic=P<n>] [frame=N]", "a parked panic's frames; frame N's fn, line and named locals with their types, and the by-name probe that reads their values"),
    ("probes", "[since=<seq>]", "installed probes and counts with hits and time, parked breakpoints, pause state; each answer is snapshot <seq>: since=<seq> gives every counter's change, rate and started / stopped since then"),
    ("hitch", "[ms=<n>|off]", "frames over the threshold (default 50 ms, always on) are `hitch` events with phases, waits, lazy compiles, critical path and probe deltas: `events on=hitch`, `watch on=hitch`"),
    ("rm", "<G<n>|all>", "removes probe groups: the code they replaced comes back (no recompile)"),
    ("continue", "[K<n>|all]", "lets parked breakpoint hits go"),
    ("pause", "", "stops the program's threads (makepad run's and inspect's keep running); stacks work while paused"),
    ("resume", "", "after pause: the program runs again (`resume P<n> panic` releases a parked panic)"),
    ("gpu", "[top [by=bytes|owner|site|churn] [n=15]|idle [frames=1800] [n=15]|snap <name>|diff <a> <b|now>|path <session>:<kind>:<index>:<generation>]", "HAL GPU memory: totals per session, kind and owner against the device (native apps: MAKEPAD_INSPECT=1 or MAKEPAD_REMOTE=1)"),
    ("hal", "pump", "HAL pump loop per session: turns and wake causes, totals and per second since the last call (an idle app shows none)"),
    ("vms", "", "every live Splash VM: label, thread, heap objects / strings / arrays and bytes, GC runs and time, errors, evaluations"),
    ("threads", "", "every thread: name, role, state, CPU time, what it waits on"),
    ("stacks", "[thread=<name|tid|main|owner|all>] [depth=12]", "symbolised stacks (identical stacks folded)"),
    ("waits", "", "the wait registry: who waits for what, held by whom"),
    ("owner", "", "the owner thread: what it serves, for whom, queue"),
    ("help", "[<cmd>|schema]", "this list; `help schema`: a JSON tool definition for agents"),
];

fn dispatch(req: &Req, out: &mut Out) {
    let _act = activity::begin(req);
    dispatch_command(req, out);
    // SPEC-V: the client prints the answer's rows, not its end line (which says `verified` too)
    if !crate::reload::observations_verified() && !matches!(req.cmd.as_str(), "help" | "ping" | "events" | "watch") {
        out.always("{\"verified\":false,\"why\":\"a patched fn runs before its borrow-check verdict; a failed verdict reverts it and the patchreverted event lists the observations it invalidates\"}");
    }
}

fn dispatch_command(req: &Req, out: &mut Out) {
    match req.cmd.as_str() {
        "session" => match req.pos.first().map(String::as_str) {
            Some("relaunch") => match crate::run::relaunch() {
                Ok(()) => out.line(&format!("{{\"relaunch\":\"queued\",\"pid\":{},\"window_kept\":true}}", std::process::id())),
                Err(e) => out.err(&e),
            },
            _ => out.err("session relaunch"),
        },
        "help" => help(req, out),
        "ping" => out.line(&format!("{{\"pid\":{},\"app\":{},\"ms\":{}}}", std::process::id(), json_str(&APP.lock().unwrap()), now_ms())),
        "threads" => cmd_threads(out),
        "stacks" => cmd_stacks(req, out),
        "waits" => cmd_waits(out),
        "owner" => cmd_owner(out),
        "why-hung" | "hang" => hang::why_hung(req, out),
        "memory" => mem::cmd_memory(req, out),
        "prof" => prof::cmd_prof(req, out),
        "timeline" => timeline::cmd_timeline(req, out),
        "hitch" => timeline::cmd_hitch(req, out),
        // question-shaped aliases (DESIGN.md §2.1)
        "frame" => timeline::cmd_timeline(&shifted(req, "frame"), out),
        "frames" => timeline::cmd_timeline(&shifted(req, "frames"), out),
        "top" => match req.get("by").unwrap_or("time") {
            "time" | "cpu" => prof::cmd_prof(req, out),
            "alloc" | "bytes" | "live" | "churn" => heap::cmd_heap(&shifted(&with_by(req), "top"), out),
            b => out.err(&format!("top by={}: time | alloc | bytes | live | churn", b)),
        },
        "path" => match req.pos.first().map(|p| p.chars().next().unwrap_or(' ')) {
            Some('H') | Some('R') => heap::cmd_heap(&shifted(req, "path"), out),
            _ => out.err("path <H<n>|R<n>>: heap stacks (frame ids: `frame <n>`)"),
        },
        "why-slow" => prof::cmd_why_slow(req, out),
        "mem" => mem::cmd_mem(req, out),
        "diff" => mem::cmd_diff(req, out),
        "heap" => heap::cmd_heap(req, out),
        "vms" => cmd_vms(out),
        // Host-native providers outlive app domains and never enter program code.
        _ if cmd_host_provided(req, out) => {},
        // Commands the program registered (`relax_register_inspect`), scoped to its domain.
        c if provider(c).is_some() => cmd_provided(req, out),
        "probe" => probes::cmd(req, out),
        "eval" => eval::cmd_eval(req, out),
        "static" => eval::cmd_static(req, out),
        "locals" => eval::cmd_locals(req, out),
        "watch" => events::cmd_watch(req, out),
        "events" => events::cmd_events(req, out),
        "panics" => events::cmd_panics(out),
        "resume" if !req.pos.is_empty() => events::cmd_resume(req, out),
        "count" | "break" | "restartable" | "probes" | "rm" | "continue" | "pause" | "resume" => instrument::cmd(req, out),
        "" => out.err("empty request"),
        c => out.err(&format!("unknown command `{}` (try `help`)", c)),
    }
}

/// `req` with `word` put in front of its positional words (an alias for a subcommand).
fn shifted(req: &Req, word: &str) -> Req {
    let mut pos = vec![word.to_string()];
    pos.extend(req.pos.iter().cloned());
    Req { cmd: req.cmd.clone(), pos, kv: req.kv.clone() }
}

/// `top by=bytes|alloc` speaks heap's words (alloc = allocated bytes, bytes / live = live bytes).
fn with_by(req: &Req) -> Req {
    let mut kv: Vec<(String, String)> = req.kv.iter().filter(|x| x.0 != "by").cloned().collect();
    let by = match req.get("by") {
        Some("alloc") => "alloc",
        Some("churn") => "freed",
        _ => "live",
    };
    kv.push(("by".into(), by.into()));
    Req { cmd: req.cmd.clone(), pos: req.pos.clone(), kv }
}

fn help(req: &Req, out: &mut Out) {
    if req.pos.first().map(String::as_str) == Some("schema") {
        out.line(&schema());
        return;
    }
    out.measured("commands", "static", &[]);
    for (name, args, what) in COMMANDS.iter().chain(makepad_gpu_trace::query::COMMANDS) {
        if let Some(w) = req.pos.first() {
            if w != name {
                continue;
            }
        }
        out.line(&format!("{{\"cmd\":{},\"args\":{},\"answers\":{}}}", json_str(name), json_str(args), json_str(what)));
    }
}

/// A tool definition for agent / MCP-style clients: one tool, `relax_inspect`, whose `command`
/// is one line of this protocol.
pub fn schema() -> String {
    let cmds: Vec<String> = COMMANDS.iter().chain(makepad_gpu_trace::query::COMMANDS).map(|(n, a, w)| format!("{} {} -- {}", n, a, w)).collect();
    format!(
        "{{\"name\":\"relax_inspect\",\"description\":{},\"input_schema\":{{\"type\":\"object\",\"properties\":{{\"target\":{{\"type\":\"string\",\"description\":\"pid, app name, or all\"}},\"command\":{{\"type\":\"string\",\"description\":\"one request line, e.g. `why-hung` or `stacks thread=main`\"}}}},\"required\":[\"target\",\"command\"]}}}}",
        json_str(&format!("Ask a running makepad app about itself. Run as `makepad inspect <target> <command>`. Commands:\n{}", cmds.join("\n")))
    )
}

// ------------------------------------------------------------------ threads and stacks

/// What a thread is to Relax (its role), from its name.
fn role(name: &str, os_id: u64, handle: u64) -> &'static str {
    let main = crate::jit::MAIN_TID.load(Relaxed);
    if handle != 0 && handle == main {
        return "main";
    }
    if handle != 0 && handle == crate::owner::owner_tid() {
        return "owner";
    }
    let _ = os_id;
    match name {
        n if n.starts_with("run-inspect") || n.starts_with("makepad-remote") => "inspect",
        n if n.starts_with("run-spec") || n.starts_with("run-O2") || n.starts_with("run-o3") || n.starts_with("run-ahead") || n == "gandalf" || n.starts_with("run-check") => "compiler",
        n if n.starts_with("run-") => "makepad run",
        _ => "program",
    }
}

/// The threads with their stacks, captured now (the inspect threads themselves left out).
pub struct Captured {
    pub t: threads::Thread,
    pub frames: Vec<u64>,
    pub lr: u64,
    pub sp: u64,
    pub err: Option<&'static str>,
}

/// The threads `want` picks with their stacks, captured now; Err: another capture holds the lock
/// longer than a capture takes ("busy: prof since 12 s by T.."), reported instead of waiting.
pub fn capture_all(want: &dyn Fn(&threads::Thread) -> bool) -> Result<Vec<Captured>, String> {
    cfi::refresh();
    let mut list = threads::list();
    name_handles(&mut list);
    let skip = SERVER_TIDS.lock().unwrap().clone();
    let mut out = Vec::with_capacity(list.len());
    let mut raw = threads::Raw::new();
    let _g = capture::lock("stacks", 2000)?;
    for t in list {
        if is_inspect_thread(&t, &skip) || !want(&t) {
            continue;
        }
        let r = threads::capture(&t, &mut raw);
        let mut t = t;
        if t.handle == 0 && raw.handle != 0 {
            t.handle = raw.handle;
        }
        out.push(Captured { frames: raw.frames().to_vec(), lr: raw.lr, sp: raw.sp, err: r.err(), t });
    }
    Ok(out)
}

/// Inspect's own threads (the server threads in `skip` = SERVER_TIDS, and by name): never
/// suspended or signalled by a capture, a profile or a pause. One of them is the caller; another
/// may be suspending threads itself (two that suspend each other never resume).
pub fn is_inspect_thread(t: &threads::Thread, skip: &[u64]) -> bool {
    skip.contains(&t.os_id) || t.name.starts_with("run-inspect") || t.name.starts_with("makepad-remote") || t.os_id == threads::self_os_id()
}

/// One capture at a time (two capturing threads must not suspend each other's targets in
/// between; Linux pause and capture share the handshake slots). A try-lock: the holder (thread,
/// what, since when) is kept so a waiter that gives up says who holds it, and `threads` / `waits`
/// show it.
pub mod capture {
    use std::sync::atomic::{AtomicU64, Ordering::{AcqRel, Acquire, Relaxed, Release}};
    static HOLDER: AtomicU64 = AtomicU64::new(0);
    static SINCE_NS: AtomicU64 = AtomicU64::new(0);
    static WHAT: AtomicU64 = AtomicU64::new(0);

    pub struct Guard(());
    impl Drop for Guard {
        fn drop(&mut self) {
            WHAT.store(0, Relaxed);
            HOLDER.store(0, Release);
        }
    }

    /// The lock for `what`, waiting at most `wait_ms` (polling 1 ms: holders keep it for one
    /// capture round, milliseconds).
    pub fn lock(what: &'static str, wait_ms: u64) -> Result<Guard, String> {
        let me = super::threads::self_os_id();
        let t0 = std::time::Instant::now();
        super::activity::phase("waiting for the capture lock", HOLDER.load(Relaxed));
        loop {
            if HOLDER.compare_exchange(0, me, AcqRel, Acquire).is_ok() {
                SINCE_NS.store(crate::clock::wall_ns(), Relaxed);
                WHAT.store(super::activity::pack(what), Relaxed);
                super::activity::phase("running", 0);
                return Ok(Guard(()));
            }
            if t0.elapsed().as_millis() as u64 >= wait_ms {
                super::activity::phase("running", 0);
                return Err(format!("busy: {}", holder_note().unwrap_or_else(|| "another capture".into())));
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    /// "prof since 12.0 s by T123 (makepad-remote-cmd: prof s=30 ...)" while the lock is held.
    pub fn holder_note() -> Option<String> {
        let h = HOLDER.load(Acquire);
        if h == 0 {
            return None;
        }
        let ms = crate::clock::wall_ns().saturating_sub(SINCE_NS.load(Relaxed)) / 1_000_000;
        let doing = super::activity::describe(h).map(|d| format!(": {}", d)).unwrap_or_default();
        Some(format!("capture lock held for {} by T{} for {:.1} s{}", super::activity::unpack(WHAT.load(Relaxed)), h, ms as f64 / 1000.0, doing))
    }
}

/// What each inspect command thread is doing (self-diagnosis: `threads` and `waits` say what a
/// stuck inspect command waits on). Atomics per slot; the command text under a mutex only inspect
/// threads take (they are never suspended).
pub mod activity {
    use std::sync::atomic::{AtomicU64, Ordering::{AcqRel, Acquire, Relaxed, Release}};
    const SLOTS: usize = 32;
    struct Slot {
        tid: AtomicU64,
        since_ns: AtomicU64,
        phase: AtomicU64,
        phase_ns: AtomicU64,
        detail: AtomicU64,
    }
    static ACTS: [Slot; SLOTS] = [const { Slot { tid: AtomicU64::new(0), since_ns: AtomicU64::new(0), phase: AtomicU64::new(0), phase_ns: AtomicU64::new(0), detail: AtomicU64::new(0) } }; SLOTS];
    static CMDS: super::Mutex<Vec<(u64, String)>> = super::Mutex::new(Vec::new());
    thread_local! {
        static MINE: std::cell::Cell<usize> = const { std::cell::Cell::new(usize::MAX) };
    }

    /// A static string packed in a word: pointer (47 bits) and length (16 bits).
    pub fn pack(s: &'static str) -> u64 {
        (s.as_ptr() as u64 & ((1 << 47) - 1)) | ((s.len().min(0xffff) as u64) << 48)
    }
    pub fn unpack(w: u64) -> &'static str {
        let (p, n) = (w & ((1 << 47) - 1), (w >> 48) as usize);
        if p == 0 || n == 0 {
            return "";
        }
        unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(p as *const u8, n)) }
    }

    pub struct Active(usize);
    impl Drop for Active {
        fn drop(&mut self) {
            if self.0 == usize::MAX {
                return;
            }
            let s = &ACTS[self.0];
            let tid = s.tid.load(Relaxed);
            CMDS.lock().unwrap_or_else(|e| e.into_inner()).retain(|c| c.0 != tid);
            let _ = MINE.try_with(|m| m.set(usize::MAX));
            s.tid.store(0, Release);
        }
    }

    /// The calling inspect thread starts answering `req`.
    pub fn begin(req: &super::Req) -> Active {
        let me = super::threads::self_os_id();
        let Some(k) = ACTS.iter().position(|s| s.tid.compare_exchange(0, me, AcqRel, Acquire).is_ok()) else { return Active(usize::MAX) };
        let now = crate::clock::wall_ns();
        let s = &ACTS[k];
        s.since_ns.store(now, Relaxed);
        s.phase.store(pack("running"), Relaxed);
        s.phase_ns.store(now, Relaxed);
        s.detail.store(0, Relaxed);
        let mut line = req.cmd.clone();
        for p in &req.pos {
            line.push(' ');
            line.push_str(p);
        }
        for (k, v) in &req.kv {
            line.push_str(&format!(" {}={}", k, v));
        }
        CMDS.lock().unwrap_or_else(|e| e.into_inner()).push((me, super::short(&line, 120)));
        let _ = MINE.try_with(|m| m.set(k));
        Active(k)
    }

    /// The calling inspect thread's current step (`detail`: the thread it is on, 0: none).
    /// Allocation-free: called around suspends.
    #[inline]
    pub fn phase(p: &'static str, detail: u64) {
        let Ok(k) = MINE.try_with(|m| m.get()) else { return };
        if k == usize::MAX {
            return;
        }
        let s = &ACTS[k];
        s.phase.store(pack(p), Relaxed);
        s.detail.store(detail, Relaxed);
        s.phase_ns.store(crate::clock::wall_ns(), Relaxed);
    }

    /// "prof s=30 threads=main for 12.3 s; now: suspending T45 for 3 ms" for an inspect thread.
    pub fn describe(tid: u64) -> Option<String> {
        let s = ACTS.iter().find(|s| s.tid.load(Acquire) == tid)?;
        let now = crate::clock::wall_ns();
        let cmd = CMDS.try_lock().ok().and_then(|c| c.iter().find(|c| c.0 == tid).map(|c| c.1.clone())).unwrap_or_else(|| "?".into());
        let since = now.saturating_sub(s.since_ns.load(Relaxed)) as f64 / 1e9;
        let ph = unpack(s.phase.load(Relaxed));
        let pms = now.saturating_sub(s.phase_ns.load(Relaxed)) / 1_000_000;
        let d = s.detail.load(Relaxed);
        let on = if d != 0 { format!(" T{}", d) } else { String::new() };
        let mut t = format!("{} for {:.1} s; now: {}{} for {} ms", cmd, since, ph, on, pms);
        if ph == "waiting for the owner" {
            t.push_str(&format!(" (owner serves {})", crate::owner::serving_note()));
        }
        Some(t)
    }

    /// Every busy inspect thread: (os id, description).
    pub fn all() -> Vec<(u64, String)> {
        ACTS.iter().map(|s| s.tid.load(Acquire)).filter(|&t| t != 0).filter_map(|t| describe(t).map(|d| (t, d))).collect()
    }
}

/// Fills in the pthread handles the OS listing lacks (Linux) for the threads Relax names by handle:
/// main, the owner, and every thread in or named by the wait registry.
pub fn name_handles(list: &mut [threads::Thread]) {
    if list.iter().all(|t| t.handle != 0) {
        return;
    }
    let mut hs: Vec<u64> = vec![crate::jit::MAIN_TID.load(Relaxed), crate::owner::owner_tid()];
    for w in crate::waits::snapshot() {
        hs.push(w.thread);
        if w.kind != crate::waits::Kind::Futex {
            hs.push(w.holder);
        }
    }
    hs.extend(crate::waits::threads());
    hs.sort_unstable();
    hs.dedup();
    for h in hs {
        if let Some(tid) = threads::tid_of_handle(h) {
            if let Some(t) = list.iter_mut().find(|t| t.os_id == tid && t.handle == 0) {
                t.handle = h;
            }
        }
    }
}

/// A symbolised frame.
#[derive(Clone)]
pub struct Frame {
    pub pc: u64,
    /// fn name (JIT: the slot's name; host: the demangled symbol)
    pub name: String,
    /// file:line:col for JIT frames
    pub at: String,
    /// "jit" (with its tier) or the host image's file name
    pub module: String,
}

/// Names for raw pcs: JIT frames by the unit (one owner Job request; on timeout `slot#id+off`),
/// host frames by the images' symbol tables. `lookup` pcs are return addresses except the first.
pub fn symbolise(frames: &[Vec<u64>], partial: &mut Vec<String>) -> Vec<Vec<Frame>> {
    symbolise_within(frames, partial, 1000)
}

/// `symbolise`, waiting at most `ms` for the owner (why-hung asks briefly: the owner may be the
/// thing that hangs).
pub fn symbolise_within(frames: &[Vec<u64>], partial: &mut Vec<String>, ms: u64) -> Vec<Vec<Frame>> {
    let mut jit_pcs: Vec<u64> = Vec::new();
    for f in frames {
        for (i, &pc) in f.iter().enumerate() {
            let look = if i == 0 { pc } else { pc.wrapping_sub(1) };
            if crate::jit::in_jit_code(look) {
                jit_pcs.push(look);
            }
        }
    }
    jit_pcs.sort_unstable();
    jit_pcs.dedup();
    let names = jit_names(&jit_pcs, partial, ms);
    frames
        .iter()
        .map(|f| {
            f.iter()
                .enumerate()
                .map(|(i, &pc)| {
                    let look = if i == 0 { pc } else { pc.wrapping_sub(1) };
                    if let Ok(k) = jit_pcs.binary_search(&look) {
                        let (name, at, tier) = names[k].clone();
                        Frame { pc, name, at, module: tier }
                    } else {
                        match symbols::host_symbol(look) {
                            Some((name, module, _)) => Frame { pc, name, at: String::new(), module },
                            None => Frame { pc, name: format!("{:#x}", pc), at: String::new(), module: "?".into() },
                        }
                    }
                })
                .collect()
        })
        .collect()
}

/// (name, file:line:col, tier) of JIT pcs.
fn jit_names(pcs: &[u64], partial: &mut Vec<String>, ms: u64) -> Vec<(String, String, String)> {
    let up = crate::jit::current_unit();
    if pcs.is_empty() || up.is_null() {
        return pcs.iter().map(|pc| (format!("jit {:#x}", pc), String::new(), "jit".into())).collect();
    }
    let pcs_v = pcs.to_vec();
    activity::phase("waiting for the owner", 0);
    let _back = PhaseBack;
    let r = crate::owner::call_within(crate::owner::Class::Job, ms, move || {
        let u = unsafe { &*up };
        pcs_v
            .iter()
            .map(|&pc| match u.pc_to_src(pc) {
                Some((id, at)) => {
                    let f = &u.fns[id as usize];
                    let cur = pc >= f.code && pc < f.code + f.code_len as u64;
                    let tier = if !cur && f.history().retired.iter().any(|r| pc >= r.0 && pc < r.0 + r.1 as u64) {
                        "jit/retired"
                    } else if !cur {
                        "jit/old"
                    } else if f.o3.load(Relaxed) {
                        "jit/O3"
                    } else {
                        "jit"
                    };
                    (display_name(u, id, 0), at, tier.to_string())
                }
                None => (format!("jit {:#x}", pc), String::new(), "jit".into()),
            })
            .collect::<Vec<_>>()
    });
    match r {
        Some(v) => v,
        None => {
            partial.push(format!("jit names: owner busy ({})", crate::owner::serving_note()));
            // the code index is a leaf lock: slot ids without the owner
            let u = unsafe { &*up };
            pcs.iter()
                .map(|&pc| match u.code.try_read().ok().and_then(|t| t.entry_at(pc)) {
                    Some((start, _, id)) => (format!("slot#{}+{:#x}", id, pc - start), String::new(), "jit".into()),
                    None => (format!("jit {:#x}", pc), String::new(), "jit".into()),
                })
                .collect()
        }
    }
}

/// Sets the phase back to running when a step ends (also on unwind).
struct PhaseBack;
impl Drop for PhaseBack {
    fn drop(&mut self) {
        activity::phase("running", 0);
    }
}

/// A slot's name as source spells it: `std::thread::JoinHandle::join` style paths with the generic
/// arguments (types printed without hashes), closures named by the fn they are in (on the owner).
fn display_name(u: &crate::jit::Unit, id: u32, depth: u32) -> String {
    let f = &u.fns[id as usize];
    match &f.key {
        crate::jit::FnKey::Inst(d, args) => {
            let mut s = crate::lower::display_path(&u.prog, *d);
            if !args.is_empty() {
                let a: Vec<String> = args.iter().map(|t| crate::typeck::ty_to_string(false, &u.prog, &u.tcx, u.tcx.tys.erase_regions(*t))).collect();
                s.push('<');
                s.push_str(&short(&a.join(", "), 120));
                s.push('>');
            }
            s
        }
        crate::jit::FnKey::Closure(_, _, p) if depth < 8 && (*p as usize) < u.fns.len() => format!("{{closure}} in {}", display_name(u, *p, depth + 1)),
        _ => f.name.to_string(),
    }
}

/// The path inspect gives code it compiles for an agent (probes, eval): a file in this process's
/// staging folder in the temp dir (its diagnostics name it).
pub(crate) fn staged_path(name: &str) -> String {
    let dir = std::env::temp_dir().join(format!("relax-probes-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&dir);
    dir.join(name).to_string_lossy().into_owned()
}

/// One frame as a short JSON object.
pub fn frame_json(f: &Frame) -> String {
    if f.at.is_empty() {
        format!("{{\"fn\":{},\"in\":{}}}", json_str(&short(&f.name, 140)), json_str(&f.module))
    } else {
        format!("{{\"fn\":{},\"at\":{},\"in\":{}}}", json_str(&short(&f.name, 140)), json_str(&f.at), json_str(&f.module))
    }
}

pub fn short(s: &str, n: usize) -> String {
    if s.len() <= n {
        return s.to_string();
    }
    let mut k = n;
    while !s.is_char_boundary(k) {
        k -= 1;
    }
    format!("{}..", &s[..k])
}

/// The thread a registry handle names, as a short label.
pub fn thread_label(handle: u64, threads: &[threads::Thread]) -> String {
    if handle == 0 {
        return "?".into();
    }
    match threads.iter().find(|t| t.handle == handle) {
        Some(t) => format!("{} (T{})", if t.name.is_empty() { "-" } else { &t.name }, t.os_id),
        None => format!("thread {:#x}", handle),
    }
}

/// The thread whose stack holds the page a mutex holder tag names (Relax std's RawMutex tag:
/// `((sp >> 12) << 1 | 1) & 0x7fff_ffff`).
pub fn holder_of_tag(tag: u64, caps: &[Captured]) -> Option<usize> {
    if tag == 0 {
        return None;
    }
    let page = (tag >> 1) & 0x3fff_ffff;
    caps.iter().position(|c| {
        let (lo, hi) = if c.t.stack.1 != 0 { c.t.stack } else { (c.sp, c.sp + (8 << 20)) };
        lo != 0 && (lo >> 12..=hi >> 12).any(|p| p & 0x3fff_ffff == page)
    })
}

fn cmd_threads(out: &mut Out) {
    let mut list = threads::list();
    name_handles(&mut list);
    let waits = crate::waits::snapshot();
    out.measured("threads", "os+registry", &[]);
    for t in &list {
        let w = waits.iter().find(|w| w.thread == t.handle && t.handle != 0);
        let mut wait = match w {
            Some(w) => format!(",\"waits\":{}", json_str(&wait_text(w, &list))),
            None => String::new(),
        };
        if let Some(d) = activity::describe(t.os_id) {
            wait.push_str(&format!(",\"inspect\":{}", json_str(&d)));
        }
        out.line(&format!(
            "{{\"id\":\"T{}\",\"name\":{},\"role\":\"{}\",\"state\":\"{}\",\"cpu_ms\":{}{}}}",
            t.os_id,
            json_str(&t.name),
            role(&t.name, t.os_id, t.handle),
            t.state,
            t.cpu_ns / 1_000_000,
            wait
        ));
    }
}

pub fn wait_text(w: &crate::waits::Wait, list: &[threads::Thread]) -> String {
    let ms = crate::clock::wall_ns().saturating_sub(w.since_ns) / 1_000_000;
    let kind = crate::waits::KIND_NAMES[w.kind as usize];
    match w.kind {
        crate::waits::Kind::Futex => format!("{} {:#x} for {} ms", if w.label.is_empty() { "futex" } else { w.label }, w.object, ms),
        crate::waits::Kind::Owner => format!("owner ({} request) for {} ms; owner serves {}", w.label, ms, crate::owner::serving_note()),
        _ => format!("{} held by {} for {} ms", kind, thread_label(w.holder, list), ms),
    }
}

fn pick_threads(sel: &str) -> Box<dyn Fn(&threads::Thread) -> bool> {
    let sel = sel.to_string();
    Box::new(move |t: &threads::Thread| match sel.as_str() {
        "all" | "" => true,
        "main" => t.handle != 0 && t.handle == crate::jit::MAIN_TID.load(Relaxed) || t.os_id == std::process::id() as u64,
        "owner" => t.handle != 0 && t.handle == crate::owner::owner_tid(),
        s => s.trim_start_matches('T').parse::<u64>().map_or(false, |id| id == t.os_id) || t.name.starts_with(s),
    })
}

fn cmd_stacks(req: &Req, out: &mut Out) {
    let sel = req.get("thread").or(req.pos.first().map(String::as_str)).unwrap_or("all");
    let depth = req.num("depth", 12) as usize;
    let caps = match capture_all(&*pick_threads(sel)) {
        Ok(c) => c,
        Err(e) => return out.err(&e),
    };
    let mut partial = Vec::new();
    let frames: Vec<Vec<u64>> = caps.iter().map(|c| c.frames.iter().copied().take(depth.max(1) + 1).collect()).collect();
    let syms = symbolise(&frames, &mut partial);
    out.measured(&format!("stacks of {} thread(s), captured now", caps.len()), "os capture (no owner)", &partial);
    // identical stacks folded
    let mut groups: Vec<(Vec<String>, Vec<usize>)> = Vec::new();
    for (i, s) in syms.iter().enumerate() {
        let key: Vec<String> = s.iter().take(depth).map(frame_json).collect();
        match groups.iter_mut().find(|g| g.0 == key) {
            Some(g) => g.1.push(i),
            None => groups.push((key, vec![i])),
        }
    }
    for (key, members) in &groups {
        let names: Vec<String> = members.iter().map(|&i| json_str(&format!("{} (T{})", caps[i].t.name, caps[i].t.os_id))).collect();
        let c = &caps[members[0]];
        let mut err = c.err.map(|e| format!(",\"err\":{}", json_str(e))).unwrap_or_default();
        if c.t.handle != 0 && instrument::stopped(&c.t) {
            err.push_str(&script_stack_field("script", c.t.handle));
        }
        out.line(&format!("{{\"threads\":[{}],\"state\":\"{}\",\"frames\":[{}]{}}}", names.join(","), c.t.state, key.join(","), err));
    }
}

fn cmd_waits(out: &mut Out) {
    let mut list = threads::list();
    name_handles(&mut list);
    let waits = crate::waits::snapshot();
    out.measured("wait registry", "registry (no owner)", &[]);
    // inspect's own command threads (a stuck command says on what) and the capture lock
    for (tid, d) in activity::all() {
        let name = list.iter().find(|t| t.os_id == tid).map(|t| t.name.as_str()).unwrap_or("makepad-remote-cmd");
        out.line(&format!("{{\"thread\":{},\"inspect\":{}}}", json_str(&format!("{} (T{})", name, tid)), json_str(&d)));
    }
    if let Some(h) = capture::holder_note() {
        out.line(&format!("{{\"capture_lock\":{}}}", json_str(&h)));
    }
    for w in &waits {
        if w.kind == crate::waits::Kind::None {
            continue;
        }
        out.line(&format!("{{\"thread\":{},\"waits\":{}}}", json_str(&thread_label(w.thread, &list)), json_str(&wait_text(w, &list))));
    }
}

fn cmd_owner(out: &mut Out) {
    out.measured("owner thread", "owner counters (no owner)", &[]);
    out.line(&format!("{{\"serving\":{},\"queued\":{}}}", json_str(&crate::owner::serving_note()), crate::owner::queued_note()));
}

// ------------------------------------------------------------------ Splash VMs

/// The Splash VM registry's summaries (makepad-script, compiled into the makepad binary: its
/// main.rs sets the provider; relax-compiler cannot depend on the script crate).
static VM_PROVIDER: relax_session::sync::Mutex<Option<fn() -> Vec<String>>> = relax_session::sync::Mutex::new(None);

pub fn set_vm_provider(f: fn() -> Vec<String>) {
    *VM_PROVIDER.lock().unwrap_or_else(|e| e.into_inner()) = Some(f);
}

/// One JSON object per live Splash VM (empty when the script crate is not in this binary).
pub fn vm_summaries() -> Vec<String> {
    let f = *VM_PROVIDER.lock().unwrap_or_else(|e| e.into_inner());
    f.map(|f| f()).unwrap_or_default()
}

/// The Splash script stack of a thread (makepad-script, set by main.rs): only valid for a thread
/// that is stopped (parked at a panic or breakpoint, or paused).
static SCRIPT_STACK_PROVIDER: relax_session::sync::Mutex<Option<fn(u64, usize) -> Option<Vec<String>>>> = relax_session::sync::Mutex::new(None);

pub fn set_script_stack_provider(f: fn(u64, usize) -> Option<Vec<String>>) {
    *SCRIPT_STACK_PROVIDER.lock().unwrap_or_else(|e| e.into_inner()) = Some(f);
}

/// The script frames (innermost first, at most 16) of the VM that the thread with pthread handle
/// `thread` is evaluating; None when it is in no VM. The caller guarantees the thread is stopped.
pub fn script_stack(thread: u64) -> Option<Vec<String>> {
    let f = (*SCRIPT_STACK_PROVIDER.lock().unwrap_or_else(|e| e.into_inner()))?;
    f(thread, 16)
}

/// `,"<key>":[...]` for a stopped thread inside a VM, else an empty string.
pub fn script_stack_field(key: &str, thread: u64) -> String {
    match script_stack(thread) {
        Some(v) if !v.is_empty() => format!(",\"{}\":[{}]", key, v.iter().map(|s| json_str(s)).collect::<Vec<_>>().join(",")),
        _ => String::new(),
    }
}

/// A Splash script error (makepad-script's error hook): an agent's `scripterr` event.
pub fn script_error_event(line: &str) {
    events::emit("scripterr", line, &format!("\"error\":{},\"more\":[\"vms\"]", json_str(&short(line, 400))));
}

/// A command answered directly by code linked into the host, without entering an app domain.
pub type HostQuery = fn(&Req, &mut Out) -> bool;

static HOST_PROVIDERS: Mutex<Vec<(&'static str, HostQuery)>> = Mutex::new(Vec::new());

/// Registers a process-lifetime provider (for example the HAL's GPU ledger). The loader calls
/// this before starting inspect or any app; domain teardown only removes program providers.
pub fn register_host_inspect(name: &'static str, query: HostQuery) {
    let mut providers = HOST_PROVIDERS.lock().unwrap_or_else(|e| e.into_inner());
    providers.retain(|(n, _)| *n != name);
    providers.push((name, query));
}

fn cmd_host_provided(req: &Req, out: &mut Out) -> bool {
    let query = HOST_PROVIDERS.lock().unwrap_or_else(|e| e.into_inner()).iter().find(|(n, _)| *n == req.cmd).map(|(_, query)| *query);
    query.is_some_and(|query| query(req, out))
}

/// A command the running program answers itself: `query(line, sink, ctx)` gets the request as the
/// host received it (one flat JSON object), applies the request's own size rule (`full=1`) and
/// writes the reply's JSON lines through `sink` as it produces them (chunks, not lines; called
/// from the inspect thread, the program's code does its own thread hand-over). Returns whether
/// it cut the reply at the limit.
pub type ProgramQuery = extern "C" fn(line: *const u8, len: usize, sink: extern "C" fn(*const u8, usize, *mut std::ffi::c_void), ctx: *mut std::ffi::c_void) -> bool;

static PROVIDERS: relax_session::sync::Mutex<Vec<(String, ProgramQuery, crate::domain::Callback)>> = relax_session::sync::Mutex::new(Vec::new());

/// The program registers the command `name` (makepad-platform's remote commands); a later
/// registration from the same domain replaces its earlier one. Every domain of the process
/// registers the same commands: the process answers with its first domain's (the root: the window
/// manager hosting app domains, or the one app), never with whichever domain started last.
pub extern "C" fn relax_register_inspect(name: *const u8, len: usize, query: ProgramQuery) {
    let Some(scope) = crate::domain::callback() else { return };
    let name = unsafe { String::from_utf8_lossy(std::slice::from_raw_parts(name, len)).into_owned() };
    let mut p = PROVIDERS.lock().unwrap_or_else(|e| e.into_inner());
    p.retain(|x| !(x.0 == name && x.2.domain() == scope.domain()));
    p.push((name, query, scope));
}

fn provider(cmd: &str) -> Option<(ProgramQuery, crate::domain::Callback)> {
    PROVIDERS.lock().unwrap_or_else(|e| e.into_inner()).iter().filter(|x| x.0 == cmd).min_by_key(|x| x.2.domain()).map(|x| (x.1, x.2))
}

/// What the sink of [`cmd_provided`] writes into: the reply, cut into lines.
struct ProvidedReply<'a, 'b> {
    out: &'a mut Out<'b>,
    pending: Vec<u8>,
}

fn cmd_provided(req: &Req, out: &mut Out) {
    let Some((query, scope)) = provider(&req.cmd) else { return };
    let mut words = vec![req.cmd.clone()];
    words.extend(req.pos.iter().cloned());
    let line = makepad_remote::app::request_line(&words, &req.kv);
    extern "C" fn sink(p: *const u8, len: usize, ctx: *mut std::ffi::c_void) {
        let reply = unsafe { &mut *(ctx as *mut ProvidedReply) };
        reply.pending.extend_from_slice(unsafe { std::slice::from_raw_parts(p, len) });
        while let Some(nl) = reply.pending.iter().position(|b| *b == b'\n') {
            let l: Vec<u8> = reply.pending.drain(..=nl).collect();
            // the program applied the size rule itself: not metered again here
            reply.out.always(String::from_utf8_lossy(&l[..nl]).as_ref());
        }
    }
    let mut reply = ProvidedReply { out, pending: Vec::new() };
    let Some(truncated) = scope.call(|| query(line.as_ptr(), line.len(), sink, &mut reply as *mut ProvidedReply as *mut std::ffi::c_void)) else {
        reply.out.err("the app domain is not running");
        return;
    };
    if !reply.pending.is_empty() {
        let rest = String::from_utf8_lossy(&reply.pending).into_owned();
        reply.out.always(&rest);
    }
    if truncated {
        reply.out.truncated = true;
    }
}

// ------------------------------------------------------------------ program services

/// `sink(ctx, kind, ptr, len)` of a service channel: kind 0 sends `ptr..len` as one message (false:
/// the client is gone), 1 ends the stream, 2 ends it with `ptr..len` as the reason. The end call
/// is the last the program makes for the channel.
pub type ServiceSink = extern "C" fn(ctx: *mut std::ffi::c_void, kind: u32, ptr: *const u8, len: usize) -> bool;
/// Starts a producer for one channel (`args` = the OPEN arguments); returns its handle, 0 when
/// refused (after an end call with the reason).
pub type ServiceOpen = extern "C" fn(args: *const u8, len: usize, sink: ServiceSink, ctx: *mut std::ffi::c_void) -> usize;
/// The client closed the channel: the producer for `handle` stops.
pub type ServiceClose = extern "C" fn(handle: usize);

struct ServiceChannel {
    tx: makepad_remote::ChannelTx,
    reason: Mutex<Option<String>>,
}

extern "C" fn service_sink(ctx: *mut std::ffi::c_void, kind: u32, ptr: *const u8, len: usize) -> bool {
    let channel = unsafe { &*(ctx as *const ServiceChannel) };
    let bytes = if len == 0 { &[][..] } else { unsafe { std::slice::from_raw_parts(ptr, len) } };
    if kind == 0 {
        return channel.tx.send(bytes.to_vec()).is_ok();
    }
    // the end: close the channel and give back the program's reference
    let why = String::from_utf8_lossy(bytes).into_owned();
    if kind == 2 {
        *channel.reason.lock().unwrap() = Some(why.clone());
    }
    channel.tx.close(if kind == 2 { makepad_remote::conn::CLOSE_REFUSED } else { makepad_remote::conn::CLOSE_NORMAL }, &why);
    drop(unsafe { std::sync::Arc::from_raw(ctx as *const ServiceChannel) });
    true
}

static ENDPOINT: Mutex<Option<makepad_remote::Endpoint>> = Mutex::new(None);
static SERVICES: Mutex<Vec<(String, u32, ServiceOpen, ServiceClose, crate::domain::Callback)>> = Mutex::new(Vec::new());
static ACTIVE_SERVICES: Mutex<Vec<std::sync::Arc<ActiveService>>> = Mutex::new(Vec::new());

struct ActiveService {
    scope: crate::domain::Callback,
    close: ServiceClose,
    handle: usize,
    channel: std::sync::Arc<ServiceChannel>,
    closed: AtomicBool,
}

impl ActiveService {
    fn close(&self) {
        if !self.closed.swap(true, std::sync::atomic::Ordering::AcqRel) {
            self.channel.tx.close(makepad_remote::conn::CLOSE_NORMAL, "app service closed");
            let _ = self.scope.cleanup(|| (self.close)(self.handle));
        }
    }
}

/// The inspect endpoint is host-owned. Retire app handlers and cancel their producers before
/// joining a domain; old endpoint factories reject calls through their incarnation's scope.
pub fn stop_domain(id: u32) {
    PROVIDERS.lock().unwrap_or_else(|e| e.into_inner()).retain(|x| x.2.domain() != id);
    SERVICES.lock().unwrap_or_else(|e| e.into_inner()).retain(|x| x.4.domain() != id);
    // a stopped first domain hands its services to the next one
    if let Some(endpoint) = ENDPOINT.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        serve_first_services(endpoint);
    }
    let channels: Vec<_> = {
        let mut active = ACTIVE_SERVICES.lock().unwrap_or_else(|e| e.into_inner());
        let channels = active.iter().filter(|c| c.scope.domain() == id).cloned().collect();
        active.retain(|c| c.scope.domain() != id);
        channels
    };
    for channel in channels { channel.close(); }
}

fn serve_program_service(endpoint: &makepad_remote::Endpoint, name: &str, version: u32, open: ServiceOpen, close: ServiceClose, scope: crate::domain::Callback) {
    let _k = crate::zone_alloc::kept();
    endpoint.register(
        name,
        version,
        std::sync::Arc::new(move |_conn, opened| {
            let channel = std::sync::Arc::new(ServiceChannel { tx: opened.channel.tx(), reason: Mutex::new(None) });
            // one reference for the program (freed by its end call), one kept here
            let for_program = std::sync::Arc::into_raw(channel.clone()) as *mut std::ffi::c_void;
            let active = scope.call(|| {
                let handle = open(opened.args.as_ptr(), opened.args.len(), service_sink, for_program);
                if handle == 0 { return None; }
                let active = std::sync::Arc::new(ActiveService { scope, close, handle, channel: channel.clone(), closed: AtomicBool::new(false) });
                ACTIVE_SERVICES.lock().unwrap_or_else(|e| e.into_inner()).push(active.clone());
                // Stop can race an open already in flight: it either drains this entry or this
                // check closes it. The callback's registration keeps the domain alive throughout.
                if !scope.running() { active.close(); }
                Some(active)
            });
            if active.is_none() {
                drop(unsafe { std::sync::Arc::from_raw(for_program as *const ServiceChannel) });
            }
            let Some(active) = active.flatten() else {
                let why = channel.reason.lock().unwrap().take().unwrap_or_else(|| "the program refused".into());
                return Err((makepad_remote::conn::CLOSE_REFUSED, why));
            };
            let ch = opened.channel;
            let service = active.clone();
            { let _k = crate::zone_alloc::kept();
            if let Err(e) = std::thread::Builder::new().name("makepad-remote-service".into()).spawn(move || {
                while let Some(ev) = ch.recv() {
                    if matches!(ev, makepad_remote::Event::Closed(..)) {
                        break;
                    }
                }
                service.close();
                ACTIVE_SERVICES.lock().unwrap_or_else(|e| e.into_inner()).retain(|c| !std::sync::Arc::ptr_eq(c, &service));
            }) {
                active.close();
                ACTIVE_SERVICES.lock().unwrap_or_else(|e| e.into_inner()).retain(|c| !std::sync::Arc::ptr_eq(c, &active));
                return Err((makepad_remote::conn::CLOSE_REFUSED, e.to_string()));
            }
            }
            Ok(())
        }),
    );
}

/// The program registers the service `name` (makepad-platform: `frames`): the host serves it on
/// its endpoint, each channel a producer the program runs. The same mechanism as
/// [`relax_register_inspect`], for streams.
pub extern "C" fn relax_register_remote_service(name: *const u8, len: usize, version: u32, open: ServiceOpen, close: ServiceClose) {
    let Some(scope) = crate::domain::callback() else { return };
    let name = unsafe { String::from_utf8_lossy(std::slice::from_raw_parts(name, len)).into_owned() };
    let mut services = SERVICES.lock().unwrap_or_else(|e| e.into_inner());
    services.retain(|x| !(x.0 == name && x.4.domain() == scope.domain()));
    services.push((name.clone(), version, open, close, scope));
    // like the commands: the first domain's service is the process's; a later domain's waits
    let first = services.iter().filter(|x| x.0 == name).min_by_key(|x| x.4.domain()).is_some_and(|x| x.4 == scope);
    drop(services);
    // the endpoint may be up already; otherwise `start` serves what is registered by then
    if first {
        if let Some(endpoint) = ENDPOINT.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            serve_program_service(endpoint, &name, version, open, close, scope);
        }
    }
}

/// Each service name served by its first remaining domain (after `start`, or a domain stopped).
fn serve_first_services(endpoint: &makepad_remote::Endpoint) {
    let services = SERVICES.lock().unwrap_or_else(|e| e.into_inner());
    for (i, (name, version, open, close, scope)) in services.iter().enumerate() {
        let first = services.iter().enumerate().filter(|(_, x)| x.0 == *name).min_by_key(|(_, x)| x.4.domain()).map(|(j, _)| j);
        if first == Some(i) {
            serve_program_service(endpoint, name, *version, *open, *close, *scope);
        }
    }
}

fn cmd_vms(out: &mut Out) {
    let v = vm_summaries();
    out.measured(&format!("{} live Splash VM(s): heap counts and GC as of their last evaluation / collection", v.len()), "script VM registry (atomics)", &[]);
    for l in v {
        out.line(&l);
    }
}

// ------------------------------------------------------------------ program wait entries

/// Relax std's sync/raw.rs and sys/posix.rs: a program thread blocks on a futex (kind: 1 mutex, 2
/// condvar, 3 read lock, 4 write lock, 5 park, 6 once; holder: a mutex holder's tag) or joins a
/// thread (kind 7; holder: its pthread handle).
pub extern "C" fn relax_wait_enter(kind: u32, addr: usize, holder: usize) {
    if kind == 7 {
        // a join: the holder is the joined thread's pthread handle
        crate::waits::enter(crate::waits::Kind::Join, 0, holder as u64, "join");
        return;
    }
    const LABELS: [&str; 7] = ["futex", "mutex", "condvar", "rwlock read", "rwlock write", "park", "once"];
    crate::waits::enter(crate::waits::Kind::Futex, addr as u64, holder as u64, LABELS.get(kind as usize).copied().unwrap_or("futex"));
}

pub extern "C" fn relax_wait_leave() {
    crate::waits::leave();
}

