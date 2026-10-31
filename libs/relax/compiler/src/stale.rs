//! Stale accesses: memory the JIT gave up and never reuses (an unloaded module's window, code
//! made inaccessible) stays reserved and PROT_NONE for the life of the process, so a late access
//! through a stale pointer traps instead of reaching other data. Such a fault is never contained
//! and never unwound (the user's decision, 2026-10-23: a stale pointer is a bug to look at, not a
//! module error to recover from):
//! - the faulting thread is parked where it is, in the fault handler (its stack stays intact, no
//!   frame unwinds, no recovery point is used even inside a guard);
//! - the `run-stale` watcher reports it (stderr, the flight recorder, an inspect event that the Run
//!   panel shows like a parked panic), pauses the program's threads so the state stays inspectable,
//!   and waits for a decision: `restart` (the app domain is relaunched, window kept) or `stop` (the
//!   process ends by the fault's signal). With nobody who could decide (no Run panel, no inspect
//!   watcher or agent; MAKEPAD_RUN_STALE_PARK=1 forces the park) the report is followed by `stop`.
//!
//! Ranges are registered by whoever retires the memory (sandbox::free_arena; code invalidation).
//! The table is append-only (ranges are never reused), so the fault handler's lookup is lock-free.

use std::sync::atomic::{AtomicI32, AtomicPtr, AtomicU32, AtomicU64, AtomicUsize, Ordering};

/// One retired range [lo, hi): what it was, and whose.
pub struct Range {
    pub lo: u64,
    pub hi: u64,
    /// "unloaded module", "invalidated code", ...
    pub kind: &'static str,
    /// the module / fn it held
    pub name: String,
    /// its version when retired (a code version, a module generation; 0: none)
    pub version: u64,
    /// why it was retired
    pub reason: String,
    /// when (wall clock ns): also the time of its `stale.retire` flight-recorder event
    pub retired_ns: u64,
}

const CAP: usize = 1 << 14;
static RANGES: [AtomicPtr<Range>; CAP] = [const { AtomicPtr::new(std::ptr::null_mut()) }; CAP];
static N: AtomicUsize = AtomicUsize::new(0);

/// Registers a retired range (before it turns inaccessible). Entries are never removed. Also
/// a `stale.retire` flight-recorder event (`a` = lo, `b` = hi), found again by `retired_ns`.
pub fn register(lo: u64, hi: u64, kind: &'static str, name: String, version: u64, reason: String) {
    start_watcher();
    let retired_ns = crate::clock::wall_ns();
    crate::inspect::flightrec::event("stale.retire", &format!("{} {} v{}: {}", kind, name, version, reason), lo, hi);
    let k = N.fetch_add(1, Ordering::AcqRel);
    if k >= CAP {
        // (beyond the table a stale access is still a crash: the fault policy's generic path)
        return;
    }
    let r = crate::zone_alloc::kept_with(|| Box::into_raw(Box::new(Range { lo, hi, kind, name, version, reason, retired_ns })));
    RANGES[k].store(r, Ordering::Release);
}

/// The retired range holding `addr` (signal-safe: atomic loads only).
pub fn lookup(addr: u64) -> Option<&'static Range> {
    let n = N.load(Ordering::Acquire).min(CAP);
    for r in &RANGES[..n] {
        let p = r.load(Ordering::Acquire);
        if !p.is_null() {
            // (never freed)
            let r: &'static Range = unsafe { &*p };
            if addr >= r.lo && addr < r.hi {
                return Some(r);
            }
        }
    }
    None
}

/// Whether [lo, hi) touches a retired range (code readers -- disassembly, stub following -- skip
/// such bytes: reading them faults like any stale access).
pub fn overlaps(lo: u64, hi: u64) -> bool {
    let n = N.load(Ordering::Acquire).min(CAP);
    RANGES[..n].iter().any(|r| {
        let p = r.load(Ordering::Acquire);
        !p.is_null() && {
            let r: &Range = unsafe { &*p };
            lo < r.hi && hi > r.lo
        }
    })
}

/// Retired ranges registered.
pub fn count() -> usize {
    N.load(Ordering::Acquire).min(CAP)
}

// ------------------------------------------------------------------ the fault side

const FRAMES: usize = 32;

/// One stale fault, written by the handler (signal-safe plain stores), taken by the watcher.
struct Rec {
    /// 0 free, 1 being written, 2 ready, 3 taken
    state: AtomicU32,
    sig: AtomicI32,
    pc: AtomicU64,
    addr: AtomicU64,
    os_id: AtomicU64,
    domain_base: AtomicU64,
    range: AtomicPtr<Range>,
    /// a fetch from retired code (pc in the range): the words at [sp] and [sp - 8] (x86-64: a
    /// call's return address, the address a `ret` just popped) and the link register (arm64)
    sp_top: AtomicU64,
    sp_below: AtomicU64,
    lr: AtomicU64,
    n: AtomicUsize,
    raw: [AtomicU64; FRAMES],
}

const RECS: usize = 16;
static REC: [Rec; RECS] = [const {
    Rec {
        state: AtomicU32::new(0),
        sig: AtomicI32::new(0),
        pc: AtomicU64::new(0),
        addr: AtomicU64::new(0),
        os_id: AtomicU64::new(0),
        domain_base: AtomicU64::new(0),
        range: AtomicPtr::new(std::ptr::null_mut()),
        sp_top: AtomicU64::new(0),
        sp_below: AtomicU64::new(0),
        lr: AtomicU64::new(0),
        n: AtomicUsize::new(0),
        raw: [const { AtomicU64::new(0) }; FRAMES],
    }
}; RECS];

/// The watcher's wake-up pipe (unix: write end, read end); -1 before it runs.
#[cfg(unix)]
static WAKE_W: AtomicI32 = AtomicI32::new(-1);
#[cfg(unix)]
static WAKE_R: AtomicI32 = AtomicI32::new(-1);

#[cfg(unix)]
extern "C" {
    fn pipe(fds: *mut i32) -> i32;
    fn write(fd: i32, buf: *const u8, n: usize) -> isize;
    fn read(fd: i32, buf: *mut u8, n: usize) -> isize;
    fn sleep(s: u32) -> u32;
}

/// In the fault handler: records the fault, wakes the watcher, writes the first report line, then
/// parks this thread for good (it never resumes: nothing it was doing goes on, nothing unwinds).
/// Signal-safe: atomic stores, write(2), sleep(3).
pub unsafe fn on_fault(f: &crate::os::fault::Fault, r: &'static Range, walk: impl FnOnce(&mut [u64]) -> usize) -> ! {
    let os_id = crate::inspect::threads::self_os_id();
    let base = if crate::jit::current_unit().is_null() { 0 } else { crate::domain::current() };
    if let Some(rec) = REC.iter().find(|x| x.state.compare_exchange(0, 1, Ordering::AcqRel, Ordering::Relaxed).is_ok()) {
        rec.sig.store(f.sig, Ordering::Relaxed);
        rec.pc.store(f.pc, Ordering::Relaxed);
        rec.addr.store(f.addr, Ordering::Relaxed);
        rec.os_id.store(os_id, Ordering::Relaxed);
        rec.domain_base.store(base, Ordering::Relaxed);
        rec.range.store(r as *const Range as *mut Range, Ordering::Relaxed);
        // (the faulting thread's own stack: both words were just written or read by the call or
        // `ret` that led here)
        let code = f.pc >= r.lo && f.pc < r.hi && f.sp >= 8;
        rec.sp_top.store(if code { std::ptr::read_volatile(f.sp as *const u64) } else { 0 }, Ordering::Relaxed);
        rec.sp_below.store(if code { std::ptr::read_volatile((f.sp - 8) as *const u64) } else { 0 }, Ordering::Relaxed);
        rec.lr.store(f.lr, Ordering::Relaxed);
        let mut raw = [0u64; FRAMES];
        let n = walk(&mut raw);
        for (k, w) in raw[..n].iter().enumerate() {
            rec.raw[k].store(*w, Ordering::Relaxed);
        }
        rec.n.store(n, Ordering::Relaxed);
        rec.state.store(2, Ordering::Release);
    }
    first_line(f, r);
    #[cfg(unix)]
    {
        let w = WAKE_W.load(Ordering::Acquire);
        if w >= 0 {
            let _ = write(w, b"s".as_ptr(), 1);
        }
        loop {
            sleep(3600);
        }
    }
    #[cfg(not(unix))]
    loop {
        std::thread::sleep(std::time::Duration::from_secs(3600));
    }
}

/// "makepad-run: stale access ..." on stderr from the handler (a stack buffer, write(2)).
fn first_line(f: &crate::os::fault::Fault, r: &Range) {
    let mut buf = [0u8; 384];
    let mut n = 0;
    let mut put = |b: &[u8]| {
        let k = b.len().min(buf.len() - n);
        buf[n..n + k].copy_from_slice(&b[..k]);
        n += k;
    };
    let hex = |v: u64| {
        let mut h = [0u8; 18];
        h[0] = b'0';
        h[1] = b'x';
        for i in 0..16 {
            let d = ((v >> (60 - 4 * i)) & 15) as u8;
            h[2 + i] = if d < 10 { b'0' + d } else { b'a' + d - 10 };
        }
        h
    };
    put(b"makepad-run: stale access into the ");
    put(r.kind.as_bytes());
    put(b" `");
    put(r.name.as_bytes());
    put(b"` at ");
    put(&hex(f.addr));
    put(b" (pc ");
    put(&hex(f.pc));
    put(b"): its memory is never reused; the thread is parked, the app pauses\n");
    crate::os::fault::raw_stderr(&buf[..n]);
}

// ------------------------------------------------------------------ the watcher

fn start_watcher() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        #[cfg(unix)]
        unsafe {
            let mut fds = [-1i32; 2];
            if pipe(fds.as_mut_ptr()) == 0 {
                WAKE_R.store(fds[0], Ordering::Release);
                WAKE_W.store(fds[1], Ordering::Release);
            }
        }
        let _k = crate::zone_alloc::kept();
        let spawned = std::thread::Builder::new().name("run-stale".into()).spawn(|| {
            crate::inspect::heap::exclude_this_thread();
            loop {
                wait();
                for rec in REC.iter() {
                    if rec.state.compare_exchange(2, 3, Ordering::AcqRel, Ordering::Relaxed).is_ok() {
                        handle(rec);
                    }
                }
                let held = std::mem::take(&mut *HELD.lock().unwrap_or_else(|e| e.into_inner()));
                for h in held {
                    handle_held(h);
                }
            }
        });
        if let Err(e) = spawned {
            eprintln!("makepad: the stale-access watcher did not start ({}): a stale access ends the process without a report", e);
        }
    });
}

fn wait() {
    #[cfg(unix)]
    {
        let r = WAKE_R.load(Ordering::Acquire);
        if r >= 0 {
            let mut b = [0u8; 16];
            unsafe { read(r, b.as_mut_ptr(), b.len()) };
            return;
        }
    }
    std::thread::sleep(std::time::Duration::from_millis(250));
}

/// Somebody can answer a parked stale access: the Run panel, an inspect watcher or agent.
fn decidable() -> bool {
    std::env::var_os("MAKEPAD_RUN_STALE_PARK").is_some_and(|v| v != "0") || crate::inspect::events::can_decide()
}

/// A thread held for good without a fault (`park_held`), waiting for the watcher's report.
struct Held {
    os_id: u64,
    /// its stack: pc, then return addresses (inspect capture order)
    frames: Vec<u64>,
    /// the frame that is in retired code (index into `frames`; usize::MAX: the word at sp)
    at: usize,
    /// "slot #12 `work::step` v1 code [0x.., 0x..)"
    what: String,
    /// why that code never runs again
    reason: String,
    /// false: it could not be held (the process ends after the report)
    held: bool,
}

static HELD: relax_session::sync::Mutex<Vec<Held>> = relax_session::sync::Mutex::new(Vec::new());

/// A program thread with a frame in code that never runs again, but that this code is not
/// protected against (a slot's first code shares its pages with other code: invalid.rs): a
/// layout-changing patch found it in the stacks it captured while the program was paused, and
/// holds it for good (inspect::instrument::hold_for_good) instead of letting it return there.
/// Reported as a stale access is -- stderr, the flight recorder, `stale`, `panics`, the Run panel
/// with `restart` | `stop` -- but there was no fault: the report is the captured stack. Returns at
/// once (the watcher thread reports and waits for the decision). `held` false: the thread could
/// not be held; the process ends after the report (it must not run that code).
pub fn park_held(os_id: u64, frames: Vec<u64>, at: usize, what: String, reason: String, held: bool) {
    start_watcher();
    crate::inspect::flightrec::event("stale.held", &format!("tid {}: frame in {}: {}", os_id, what, reason), os_id, frames.get(at).copied().unwrap_or(0));
    crate::zone_alloc::kept_with(|| HELD.lock().unwrap_or_else(|e| e.into_inner()).push(Held { os_id, frames, at, what, reason, held }));
    #[cfg(unix)]
    {
        let w = WAKE_W.load(Ordering::Acquire);
        if w >= 0 {
            let _ = unsafe { write(w, b"h".as_ptr(), 1) };
        }
    }
}

fn handle_held(h: Held) {
    let pc = h.frames.first().copied().unwrap_or(0);
    let raw: Vec<u64> = h.frames.iter().skip(1).copied().collect();
    let frames = crate::jit::stale_frames(pc, &raw);
    let thread = crate::inspect::threads::list().into_iter().find(|t| t.os_id == h.os_id).map(|t| t.name.clone()).unwrap_or_else(|| h.os_id.to_string());
    let frame = if h.at == usize::MAX { "the return address at its sp".to_string() } else { format!("frame #{}", h.at) };
    let message = format!(
        "stale access on a stack (no fault): thread `{}` (tid {}) has {} in {}: code that never runs again ({}); the thread is held where it was, nothing unwound",
        thread, h.os_id, frame, h.what, h.reason
    );
    let report = crate::jit::stale_report(&message, &frames, frames.get(h.at.min(frames.len().saturating_sub(1))).copied().unwrap_or(0));
    eprintln!("makepad: {}\n  report: {}", message, report);
    crate::inspect::flightrec::fault_dump(&format!("stale.held tid {} {}", h.os_id, h.what));
    let verdict = format!(
        "stale access on a stack: thread `{}` (tid {}) {} at a layout-changing patch: its {} is in {} ({}). HOW: frame in first-version code at a layout patch (this code shares pages with other code, so it is not made inaccessible: the pause found the frame in the captured stacks and holds the thread before it returns there). {}",
        thread,
        h.os_id,
        if h.held { "is held" } else { "could NOT be held" },
        frame,
        h.what,
        h.reason,
        if h.held { "The app paused" } else { "The process ends" }
    );
    eprintln!("makepad: {}", verdict);
    crate::zone_alloc::kept_with(|| ANSWERS.lock().unwrap_or_else(|e| e.into_inner()).push((verdict.clone(), report.clone())));
    if !h.held {
        eprintln!("makepad: the thread could not be held: the process ends (it must not run that code)");
        crate::os::fault::die_by(11);
    }
    if !decidable() {
        eprintln!("makepad: nobody can decide on the stale access (no Run panel, inspect watcher or agent; MAKEPAD_RUN_STALE_PARK=1 parks): the process ends");
        crate::os::fault::die_by(11);
    }
    crate::inspect::start("stale-access");
    let paused = crate::inspect::instrument::pause_program();
    eprintln!("makepad: the app is paused ({} threads held) -- decide: makepad inspect {} resume <id> restart | stop (the Run panel offers both)", paused, std::process::id());
    // (no app domain: the held thread's domain is not known, so `restart` restarts the process)
    let decision = crate::inspect::events::park_stale(h.os_id, None, &message, &frames);
    if decision == crate::inspect::events::StaleDecision::Stop {
        eprintln!("makepad: stale access on a stack: stopped");
        crate::os::fault::die_by(11);
    }
    restart_process();
}

fn handle(rec: &Rec) {
    let r: &Range = unsafe { &*rec.range.load(Ordering::Relaxed) };
    let (pc, addr, sig) = (rec.pc.load(Ordering::Relaxed), rec.addr.load(Ordering::Relaxed), rec.sig.load(Ordering::Relaxed));
    let mut raw: Vec<u64> = rec.raw[..rec.n.load(Ordering::Relaxed)].iter().map(|w| w.load(Ordering::Relaxed)).collect();
    // a fetch from retired code: how the thread got there and who stored the address (jit.rs)
    let code = pc >= r.lo && pc < r.hi;
    let how = if code {
        let (how, caller) = crate::jit::stale_code_how(pc, rec.sp_top.load(Ordering::Relaxed), rec.sp_below.load(Ordering::Relaxed), rec.lr.load(Ordering::Relaxed));
        // entering the code: the caller's frame is not on the frame-pointer chain yet
        if let Some(c) = caller.filter(|c| raw.first() != Some(c)) {
            raw.insert(0, c);
        }
        Some(how)
    } else {
        None
    };
    let frames = crate::jit::stale_frames(pc, &raw);
    let message = match &how {
        Some(_) => format!("stale access into the {} `{}` at {:#x}: code that never runs again (it was invalidated: {}); the thread is parked, nothing unwound", r.kind, r.name, addr, r.reason),
        None => format!("stale access into the {} `{}` at {:#x}: its memory is never reused (a stale pointer); the thread is parked, nothing unwound", r.kind, r.name, addr),
    };
    let report = crate::jit::stale_report(&message, &frames, addr);
    eprintln!("makepad: {}\n  report: {}", message, report);
    crate::inspect::flightrec::fault_dump(&format!("stale.access {} {} at {:#x} pc {:#x}", r.kind, r.name, addr, pc));
    let domain = crate::domain::id_of_base(rec.domain_base.load(Ordering::Relaxed));
    let os_id = rec.os_id.load(Ordering::Relaxed);
    let thread = crate::inspect::threads::list().into_iter().find(|t| t.os_id == os_id).map(|t| t.name.clone()).unwrap_or_else(|| os_id.to_string());
    let into = where_in(r, addr);
    let age_ms = crate::clock::wall_ns().saturating_sub(r.retired_ns) / 1_000_000;
    let verdict = match &how {
        Some(how) => format!(
            "stale access: thread `{}` (tid {}) ran code at {:#x} = {} `{}` v{} + {:#x} ({}), invalidated {} ms before ({}; flightrec stale.retire at {} ns). HOW: {}. Its thread is parked, the app paused",
            thread, os_id, pc, r.kind, r.name, r.version, pc - r.lo, into, age_ms, r.reason, r.retired_ns, how
        ),
        None => format!(
            "stale access: thread `{}` (tid {}) read/wrote {:#x} = {} `{}` v{} + {:#x} ({}), retired {} ms before ({}; flightrec stale.retire at {} ns); the faulting frame is the first of the report's backtrace (fn, tier, code version); its thread is parked, the app paused",
            thread, os_id, addr, r.kind, r.name, r.version, addr - r.lo, into, age_ms, r.reason, r.retired_ns
        ),
    };
    if let Some(h) = &how {
        eprintln!("makepad: stale access: {}", h);
    }
    crate::zone_alloc::kept_with(|| ANSWERS.lock().unwrap_or_else(|e| e.into_inner()).push((verdict.clone(), report.clone())));
    if !decidable() {
        eprintln!("makepad: nobody can decide on the stale access (no Run panel, inspect watcher or agent; MAKEPAD_RUN_STALE_PARK=1 parks): the process ends");
        crate::os::fault::die_by(sig);
    }
    // (a paused process must answer: `makepad test` and hosted runs may not have started inspect)
    crate::inspect::start("stale-access");
    let paused = crate::inspect::instrument::pause_program();
    eprintln!("makepad: the app is paused ({} threads held) -- decide: makepad inspect {} resume <id> restart | stop (the Run panel offers both)", paused, std::process::id());
    let decision = crate::inspect::events::park_stale(rec.os_id.load(Ordering::Relaxed), domain, &message, &frames);
    if decision == crate::inspect::events::StaleDecision::Stop {
        eprintln!("makepad: stale access: stopped");
        crate::os::fault::die_by(sig);
    }
    if decision == crate::inspect::events::StaleDecision::RestartProcess {
        restart_process();
    }
    // restarted (the domain relaunched): the program runs again; the faulting thread stays parked
    crate::inspect::instrument::resume_program();
    rec.state.store(0, Ordering::Release);
}

/// What the watcher told, for `makepad inspect <pid> stale`: (verdict line, report JSON).
static ANSWERS: relax_session::sync::Mutex<Vec<(String, String)>> = relax_session::sync::Mutex::new(Vec::new());

/// Where in a retired range `addr` was (what kind of data the stale pointer pointed to; for
/// code, which fn's code version and what invalidated it: invalid.rs).
fn where_in(r: &Range, addr: u64) -> String {
    if let Some(s) = crate::invalid::why_at(addr, |w| {
        let i = w.installs.iter().rev().find(|i| addr >= i.at && addr < i.at + i.len as u64).copied();
        let what = match i {
            Some(i) => format!("code of slot #{} `{}` v{} +{:#x}", i.slot, crate::jit::slot_name(i.slot).unwrap_or_default(), i.version, addr - i.at),
            None => format!("{:?} chunk +{:#x} (between code bodies)", w.group, addr - w.lo),
        };
        let made = match w.group {
            crate::invalid::Group::Patch { gen, .. } | crate::invalid::Group::Module { gen, .. } if gen > 0 => {
                crate::invalid::patch_event_of(gen).map(|(t, e)| format!("; made by flightrec reload.patch at {} ns: {}", t, e)).unwrap_or_default()
            }
            _ => String::new(),
        };
        let by = if w.by_gen > 0 { crate::invalid::patch_event_of(w.by_gen).map(|(t, e)| format!("; invalidated by flightrec reload.patch at {} ns: {}", t, e)).unwrap_or_default() } else { String::new() };
        format!("{} [{:?}]{}{}", what, w.group, made, by)
    }) {
        return s;
    }
    match r.kind {
        "unloaded module" => crate::sandbox::describe_window_offset(addr - r.lo),
        _ => format!("{:#x} bytes in", addr - r.lo),
    }
}

/// `resume <id> restart` of a program that runs in no app domain: the process starts again with
/// its own command line (the current sources; its state is not carried over). Never returns.
fn restart_process() {
    eprintln!("makepad: stale access: restarting the process (its sources as they are now)");
    // (the inspect reply goes out first)
    std::thread::sleep(std::time::Duration::from_millis(200));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // fd 2 back to where it went before the session log took it (its forwarder ends with exec)
        crate::tui::finish(false);
        let args: Vec<std::ffi::OsString> = std::env::args_os().collect();
        if let Ok(exe) = std::env::current_exe() {
            let e = std::process::Command::new(exe).args(&args[1..]).exec();
            eprintln!("makepad: stale access: restart failed: {}", e);
        }
    }
    crate::os::fault::die_by(11);
}

/// `stale`: answer first -- the parked stale accesses (verdict, then the report), then the retired
/// ranges (newest last, at most `n`, default 8).
pub fn cmd(req: &crate::inspect::Req, out: &mut crate::inspect::Out) {
    let answers = ANSWERS.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if answers.is_empty() {
        out.line("no stale access so far");
    }
    for (v, rep) in &answers {
        out.line(v);
        out.line(&format!("  report: {}", rep));
    }
    // a parked access into code: every thread's stack with the tier and version per frame (an
    // INVALIDATED frame is code that parks at its next instruction)
    if answers.iter().any(|(v, _)| v.contains(" HOW: ")) {
        out.line("every thread's stack (frames: fn, tier, code version; a parked thread shows its recorded frames):");
        crate::inspect::cmd_stacks(req, out);
    }
    let n = req.num("n", 8) as usize;
    let total = count();
    out.line(&format!("retired ranges: {} (never reused){}", total, if total > n { format!(", the last {}", n) } else { String::new() }));
    for slot in &RANGES[total.saturating_sub(n)..total] {
        let p = slot.load(Ordering::Acquire);
        if p.is_null() {
            continue;
        }
        let r = unsafe { &*p };
        out.line(&format!("  [{:#x}, {:#x}) {} `{}` v{}: {} (at {} ns)", r.lo, r.hi, r.kind, r.name, r.version, r.reason, r.retired_ns));
    }
}
