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
        n: AtomicUsize::new(0),
        raw: [const { AtomicU64::new(0) }; FRAMES],
    }
}; RECS];

/// The watcher's wake-up pipe (unix: write end, read end); -1 before it runs.
static WAKE_W: AtomicI32 = AtomicI32::new(-1);
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

fn handle(rec: &Rec) {
    let r: &Range = unsafe { &*rec.range.load(Ordering::Relaxed) };
    let (pc, addr, sig) = (rec.pc.load(Ordering::Relaxed), rec.addr.load(Ordering::Relaxed), rec.sig.load(Ordering::Relaxed));
    let raw: Vec<u64> = rec.raw[..rec.n.load(Ordering::Relaxed)].iter().map(|w| w.load(Ordering::Relaxed)).collect();
    let frames = crate::jit::stale_frames(pc, &raw);
    let message = format!("stale access into the {} `{}` at {:#x}: its memory is never reused (a stale pointer); the thread is parked, nothing unwound", r.kind, r.name, addr);
    let report = crate::jit::stale_report(&message, &frames, addr);
    eprintln!("makepad: {}\n  report: {}", message, report);
    crate::inspect::flightrec::fault_dump(&format!("stale.access {} {} at {:#x} pc {:#x}", r.kind, r.name, addr, pc));
    let domain = crate::domain::id_of_base(rec.domain_base.load(Ordering::Relaxed));
    let os_id = rec.os_id.load(Ordering::Relaxed);
    let thread = crate::inspect::threads::list().into_iter().find(|t| t.os_id == os_id).map(|t| t.name.clone()).unwrap_or_else(|| os_id.to_string());
    let into = where_in(r, addr);
    let age_ms = crate::clock::wall_ns().saturating_sub(r.retired_ns) / 1_000_000;
    let verdict = format!(
        "stale access: thread `{}` (tid {}) read/wrote {:#x} = {} `{}` v{} + {:#x} ({}), retired {} ms before ({}; flightrec stale.retire at {} ns); the faulting frame is the first of the report's backtrace (fn, tier, code version); its thread is parked, the app paused",
        thread, os_id, addr, r.kind, r.name, r.version, addr - r.lo, into, age_ms, r.reason, r.retired_ns
    );
    crate::zone_alloc::kept_with(|| ANSWERS.lock().unwrap_or_else(|e| e.into_inner()).push((verdict.clone(), report.clone())));
    if !decidable() {
        eprintln!("makepad: nobody can decide on the stale access (no Run panel, inspect watcher or agent; MAKEPAD_RUN_STALE_PARK=1 parks): the process ends");
        crate::os::fault::die_by(sig);
    }
    // (a paused process must answer: `makepad test` and hosted runs may not have started inspect)
    crate::inspect::start("stale-access");
    let paused = crate::inspect::instrument::pause_program();
    eprintln!("makepad: the app is paused ({} threads held) -- decide: makepad inspect {} resume <id> restart | stop (the Run panel offers both)", paused, std::process::id());
    let stop = crate::inspect::events::park_stale(rec.os_id.load(Ordering::Relaxed), domain, &message, &frames);
    if stop {
        eprintln!("makepad: stale access: stopped");
        crate::os::fault::die_by(sig);
    }
    // restarted (the domain relaunched): the program runs again; the faulting thread stays parked
    crate::inspect::instrument::resume_program();
    rec.state.store(0, Ordering::Release);
}

/// What the watcher told, for `makepad inspect <pid> stale`: (verdict line, report JSON).
static ANSWERS: relax_session::sync::Mutex<Vec<(String, String)>> = relax_session::sync::Mutex::new(Vec::new());

/// Where in a retired range `addr` was (what kind of data the stale pointer pointed to).
fn where_in(r: &Range, addr: u64) -> String {
    match r.kind {
        "unloaded module" => crate::sandbox::describe_window_offset(addr - r.lo),
        _ => format!("{:#x} bytes in", addr - r.lo),
    }
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
