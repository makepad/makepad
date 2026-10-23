//! The owner thread (local/agent_state/fast-rust/JIT_THREADS.md): the one thread that runs every
//! step reading or changing a unit's mutable state -- lazy compiles' fronts and installs, constant
//! and static evaluation (and the JIT code they run), the disk cache, patches, tier installs, the
//! exit summary. There is no unit lock. Other threads hand it closures and wait for them
//! (`call`); it never blocks on another thread: every wait on it is a pump (`pump_wait`) that
//! serves the requests which may be what it waits for.
//!
//! Every request runs in a fresh frame (jit::frame_enter) and in its requester's query context
//! (relax_base::query): queries it runs record their reads where the requester's would, and a
//! query the requester is running is the parent of the ones the request starts, so a real cycle
//! through a request is the query engine's cycle error. A request served nested inside an
//! earlier request's wait that reaches something only that earlier request can finish (a query
//! or a build it has open, a load) is cancelled (`OwnerBusy`) and its requester posts it again.

use std::any::Any;
use std::cell::Cell;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use relax_session::sync::{Condvar, Mutex};

/// Request classes (service order: SERVICE_ORDER).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Class {
    /// asked by a pure job that may hold what the owner waits for (a constant's value for a type
    /// check, a report): served also inside every pump
    Job = 0,
    /// a bounded, guarded step (OWNERARCH.DESIGN.md rule I-PUB, `publish`): a back end's finish,
    /// background bookkeeping, the take of a background build. It runs
    /// no front, no initializer, no JIT code, no wait and no request, so it is never refused and is
    /// served inside every pump. Served after the program's requests: a program thread waits for at
    /// most the one publish running when it asks
    Publish = 1,
    /// the program's threads (its main thread among them, in one FIFO: OWNERARCH.DESIGN.md step 6),
    /// patches, structural steps (loads, adopts), inspect
    Program = 2,
}
pub const CLASSES: usize = 3;
const CLASS_OF: [Class; CLASSES] = [Class::Job, Class::Publish, Class::Program];
/// The order the owner serves its lanes in (OWNERARCH.DESIGN.md §4: Job, Program, Publish)
const SERVICE_ORDER: [usize; CLASSES] = [Class::Job as usize, Class::Program as usize, Class::Publish as usize];
pub const CLASS_NAMES: [&str; CLASSES] = ["job", "publish", "program"];
/// makepad inspect's timeline span names of the owner's service, by class
const SERVE_NAMES: [&str; CLASSES] = ["serve job", "serve publish", "serve program"];

impl Class {
    /// a publish: never refused, served inside pumps
    pub fn is_finish(self) -> bool {
        self == Class::Publish
    }

    /// The requester stays a running compile job while it waits (it holds front guards, queries,
    /// or slots in flight that only its finish releases). An allocator swap or fork waits for
    /// every running job, so these are the classes the owner serves for any thread while the jobs
    /// are paused: one predicate for both sides, so a swap never waits for a job whose request
    /// the pause holds back (an O2/speculation/O3 finish queued as a background finish during a
    /// CEF-load swap hung Stage: "jobs active 1").
    pub fn keeps_job(self) -> bool {
        self == Class::Job || self.is_finish()
    }
}

const QUEUED: u8 = 0;
const RUNNING: u8 = 1;
const DONE: u8 = 2;
/// the owner touches the request no more: the requester may return (and free it)
const RELEASED: u8 = 3;
/// Spin budgets, in microseconds of `spin_loop` iterations. The loops count iterations and read no
/// clock: a clock read per iteration was a tenth of a cold start's cycles. The iteration cost is
/// measured once (`calibrate`, on the owner thread at start), so a budget keeps its length on any
/// core. A sleep/wake pair costs tens of microseconds; warm launches make thousands of short
/// requests in a row. A requester spins only when the owner is free to take its request at once
/// (not asleep, not serving another request, nothing else queued): then the answer of a short
/// request comes within the budget. Behind a long request it sleeps straight away.
///
/// Wakeup protocol (the spin is only an optimisation in front of it, never part of it):
/// - the requester sleeps on `req.done`: it checks `*done` under that mutex before each wait, and
///   the owner sets `*done` and notifies under the same mutex, so a completion between the check
///   and the wait is seen;
/// - the owner sleeps on `WAKE` only with `Q` held and `OWNER_ASLEEP` set, after finding the lanes
///   empty under `Q`; `push` adds under `Q` and then reads `OWNER_ASLEEP` (SeqCst), so a request
///   posted between the owner's check and its wait is either seen by the check or notifies.
const REQUESTER_SPIN_US: u32 = 60;
const OWNER_SPIN_US: u32 = 60;
/// spin iterations per microsecond (0: not measured yet, nobody spins)
static SPIN_PER_US: AtomicU32 = AtomicU32::new(0);

fn calibrate() {
    static PROBE: AtomicU32 = AtomicU32::new(0);
    const N: u32 = 4000;
    let mut best = u128::MAX;
    for _ in 0..3 {
        let t0 = std::time::Instant::now();
        for _ in 0..N {
            if PROBE.load(Ordering::Relaxed) != 0 {
                break;
            }
            std::hint::spin_loop();
        }
        best = best.min(t0.elapsed().as_nanos());
    }
    SPIN_PER_US.store(((N as u128 * 1000 / best.max(1)) as u32).clamp(1, 100_000), Ordering::Relaxed);
}

fn spin_iters(us: u32) -> u32 {
    SPIN_PER_US.load(Ordering::Relaxed).saturating_mul(us)
}

struct Req {
    run: *mut dyn FnMut(),
    class: Class,
    /// where it was asked (postframe's long-request log)
    site: &'static std::panic::Location<'static>,
    /// requester (os thread id)
    from: u64,
    /// asked by background work (a Publish or Job of one of Relax's
    /// own threads): the I-PURE census counts what it runs (`pure_note`)
    bg: bool,
    /// the requester's query context
    ctx: *const (),
    /// the requester's compile record (jit::main_rec)
    rec: *const (),
    /// reads no slot identity (the startup image's identities stay encoded: jit::ids_on_serve)
    quick: bool,
    state: AtomicU8,
    panic: Cell<Option<Box<dyn Any + Send>>>,
    /// false: posted (heap, nobody waits); the owner frees it
    waited: bool,
    /// main-thread request accounting (0: not a main-thread request): wall ns at the push, and at
    /// the owner's completion
    t_push: u64,
    t_serve: AtomicU64,
    t_done: AtomicU64,
    /// the requester sleeps on these until Done (no thread handle: a thread's TLS destructors
    /// compile too, after std::thread::current() is gone)
    done: Mutex<bool>,
    wake: Condvar,
}

struct Queue {
    lanes: [VecDeque<usize>; CLASSES],
}

static Q: Mutex<Queue> = Mutex::new(Queue { lanes: [const { VecDeque::new() }; CLASSES] });

pub(crate) struct ZoneTaken(Queue);

/// None: a request was queued since the quiesce looked (an app thread asks the owner whenever it
/// likes, and the gate does not hold it back): the swap is tried again later.
pub(crate) fn zone_take() -> Option<ZoneTaken> {
    let mut queue = Q.lock().unwrap_or_else(|e| e.into_inner());
    // Quiescence includes draining every request before taking the lanes' buffers.
    if !queue.lanes.iter().all(VecDeque::is_empty) {
        return None;
    }
    let taken = ZoneTaken(std::mem::replace(&mut *queue, Queue { lanes: [const { VecDeque::new() }; CLASSES] }));
    #[cfg(test)]
    assert!(queue.lanes.iter().all(|lane| lane.is_empty() && lane.capacity() == 0));
    Some(taken)
}

pub(crate) fn zone_put_back(taken: ZoneTaken) {
    let mut queue = Q.lock().unwrap_or_else(|e| e.into_inner());
    std::mem::forget(std::mem::replace(&mut *queue, taken.0));
}

static WAKE: Condvar = Condvar::new();
static STARTED: std::sync::Once = std::sync::Once::new();
/// Program requests queued (diagnostics)
static FOREGROUND: AtomicU32 = AtomicU32::new(0);
/// the main thread's requests queued
static MAIN_QUEUED: AtomicU32 = AtomicU32::new(0);
/// requests queued (any class): the idle owner spins on it
static QUEUED_N: AtomicU32 = AtomicU32::new(0);
/// the owner sleeps on WAKE (a push then notifies it; otherwise it is spinning or busy)
static OWNER_ASLEEP: AtomicBool = AtomicBool::new(false);
/// the owner is inside `serve` (a heuristic only: the requesters' spin budget reads it)
static OWNER_SERVING: AtomicBool = AtomicBool::new(false);

/// os thread id of the owner thread
static OWNER_TID: AtomicU64 = AtomicU64::new(0);
static OWNER_THREAD: std::sync::OnceLock<std::thread::Thread> = std::sync::OnceLock::new();
/// the threads ending the process (0: none): from then on only their requests (and Job / Finish
/// ones) are served -- other threads' requests wait until the process is gone
static EXIT_TID: AtomicU64 = AtomicU64::new(0);
static EXIT_HELPER: AtomicU64 = AtomicU64::new(0);
/// a forked child: one thread, which runs every request itself
static CHILD: AtomicBool = AtomicBool::new(false);
/// wall ns the owner spent serving, by class (the run summary)
pub static BUSY_NS: [AtomicU64; CLASSES] = [const { AtomicU64::new(0) }; CLASSES];
/// The main thread's requests to the owner (run summary): [count, wait ns, queue delay ns (push ->
/// served), wake delay ns (owner done -> requester running), then count and wait ns of the waits
/// below 20 us, 200 us, 2 ms and above]
pub static MAIN_ACC: [AtomicU64; 12] = [const { AtomicU64::new(0) }; 12];
/// the main thread's longest wait for a request after the first present (ns), and the class and
/// requester the owner was serving when it began (a frame hitch: run summary)
pub static MAIN_WAIT_MAX: AtomicU64 = AtomicU64::new(0);
static MAIN_WAIT_BEHIND: AtomicU64 = AtomicU64::new(u64::MAX);
/// what the owner serves now: class (u8::MAX: idle) and requester
static NOW_CLASS: AtomicU8 = AtomicU8::new(u8::MAX);
static NOW_FROM: AtomicU64 = AtomicU64::new(0);
/// the call site of the request being served (null: none)
static NOW_SITE: std::sync::atomic::AtomicPtr<std::panic::Location<'static>> = std::sync::atomic::AtomicPtr::new(std::ptr::null_mut());

/// (owner) the line of the request being served (0: none)
pub fn serving_line() -> u32 {
    let p = NOW_SITE.load(Ordering::Relaxed);
    if p.is_null() { 0 } else { unsafe { (*p).line() } }
}
/// when the request being served began (wall ns; 0: idle)
static NOW_T0: AtomicU64 = AtomicU64::new(0);

// ------------------------------------------------------------ owner time by request site (diagnostics)

/// Owner time by request site (run summary `owner by site`): what each call site cost the owner,
/// how much of it was O1 optimisation (optimize_rir on the owner), and how long the main thread's
/// requests queued behind it after the first present. One lock-free table: a site is entered by a
/// CAS on an empty row; a full table counts the rest in its last row.
pub struct SiteAcc {
    site: std::sync::atomic::AtomicPtr<std::panic::Location<'static>>,
    class: AtomicU8,
    /// [requests, owner ns, O1 optimisation ns on the owner, main's queue ns behind it, main waits behind it,
    /// services of 1 ms or more, the longest service ns]
    pub acc: [AtomicU64; 7],
}

const SITES_N: usize = 96;
pub static SITES: [SiteAcc; SITES_N] = [const { SiteAcc { site: std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()), class: AtomicU8::new(0), acc: [const { AtomicU64::new(0) }; 7] } }; SITES_N];
pub const SITE_REQS: usize = 0;
pub const SITE_NS: usize = 1;
pub const SITE_OPT_NS: usize = 2;
pub const SITE_BEHIND_NS: usize = 3;
pub const SITE_BEHIND_N: usize = 4;
pub const SITE_LONG_N: usize = 5;
pub const SITE_MAX_NS: usize = 6;
/// main's queue delay (after the first present) by what the owner served when it asked: per class,
/// and the owner idle (last row): [waits, ns]
pub static MAIN_BEHIND: [[AtomicU64; 2]; CLASSES + 1] = [const { [const { AtomicU64::new(0) }; 2] }; CLASSES + 1];

/// The table row of `site` (entered on first use).
pub fn site_acc(site: *const std::panic::Location<'static>, class: Class) -> &'static SiteAcc {
    let h = (site as usize >> 3).wrapping_mul(0x9e37_79b9) % (SITES_N - 1);
    for k in 0..SITES_N - 1 {
        let row = &SITES[(h + k) % (SITES_N - 1)];
        let cur = row.site.load(Ordering::Acquire);
        if cur as *const _ == site {
            return row;
        }
        if cur.is_null() {
            match row.site.compare_exchange(std::ptr::null_mut(), site as *mut _, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => {
                    row.class.store(class as u8, Ordering::Relaxed);
                    return row;
                }
                Err(c) if c as *const _ == site => return row,
                Err(_) => continue,
            }
        }
    }
    &SITES[SITES_N - 1]
}

/// (owner) O1 optimisation time spent inside the request being served: charged to its site.
pub fn note_owner_opt(ns: u64) {
    if !is_owner() {
        return;
    }
    let site = NOW_SITE.load(Ordering::Relaxed);
    if site.is_null() {
        return;
    }
    let class = CLASS_OF[NOW_CLASS.load(Ordering::Relaxed).min(CLASSES as u8 - 1) as usize];
    site_acc(site, class).acc[SITE_OPT_NS].fetch_add(ns, Ordering::Relaxed);
}

/// The run summary's `owner by site` rows: the `n` sites with the most owner time, plus the main
/// thread's queue delay by what it was behind.
pub fn sites_summary(n: usize) -> String {
    let mut rows: Vec<(u64, String)> = Vec::new();
    for row in SITES.iter() {
        let site = row.site.load(Ordering::Acquire);
        let a = |i: usize| row.acc[i].load(Ordering::Relaxed);
        if a(SITE_REQS) == 0 {
            continue;
        }
        // SAFETY: only &'static Locations are stored
        let name = match unsafe { site.as_ref() } {
            Some(l) => format!("{}:{}", l.file().rsplit('/').next().unwrap_or(""), l.line()),
            None => "other".to_string(),
        };
        let c = CLASS_NAMES[row.class.load(Ordering::Relaxed) as usize % CLASSES];
        rows.push((a(SITE_NS).max(a(SITE_BEHIND_NS)), format!("{} {} {}x {:.0} ms (O1 opt {:.0} ms, {} of 1 ms or more, longest {:.1} ms), main behind it {}x {:.0} ms",
            c, name, a(SITE_REQS), a(SITE_NS) as f64 / 1e6, a(SITE_OPT_NS) as f64 / 1e6, a(SITE_LONG_N), a(SITE_MAX_NS) as f64 / 1e6, a(SITE_BEHIND_N), a(SITE_BEHIND_NS) as f64 / 1e6)));
    }
    let mut by_behind: Vec<(u64, String)> = Vec::new();
    for row in SITES.iter() {
        let site = row.site.load(Ordering::Acquire);
        let ns = row.acc[SITE_BEHIND_NS].load(Ordering::Relaxed);
        if ns == 0 {
            continue;
        }
        // SAFETY: only &'static Locations are stored
        let name = match unsafe { site.as_ref() } {
            Some(l) => format!("{}:{}", l.file().rsplit('/').next().unwrap_or(""), l.line()),
            None => "other".to_string(),
        };
        by_behind.push((ns, format!("{} {} {}x {:.0} ms", CLASS_NAMES[row.class.load(Ordering::Relaxed) as usize % CLASSES], name, row.acc[SITE_BEHIND_N].load(Ordering::Relaxed), ns as f64 / 1e6)));
    }
    by_behind.sort_by(|a, b| b.0.cmp(&a.0));
    rows.sort_by(|a, b| b.0.cmp(&a.0));
    let b = |c: usize| format!("{} {}x {:.0} ms", if c == CLASSES { "idle" } else { CLASS_NAMES[c] }, MAIN_BEHIND[c][0].load(Ordering::Relaxed), MAIN_BEHIND[c][1].load(Ordering::Relaxed) as f64 / 1e6);
    format!("main's queue delay after the present by what the owner served: {}; most behind: {}; sites: {}",
        (0..=CLASSES).map(b).collect::<Vec<_>>().join(", "),
        by_behind.into_iter().take(8).map(|r| r.1).collect::<Vec<_>>().join("; "),
        rows.into_iter().take(n).map(|r| r.1).collect::<Vec<_>>().join("; "))
}

// ------------------------------------------------------------ recent requests (diagnostics)

/// The owner's recently served requests, always on (a hitch names what the owner served while the
/// UI thread waited for it: `served_during`). Two rings: every request, and the long ones (>= 1 ms,
/// so a burst of short requests does not push a long one out). One writer (the owner thread);
/// readers on any thread use the per-entry sequence (a seqlock: odd while written, retried).
struct Served {
    seq: AtomicU64,
    class: AtomicU8,
    from: AtomicU64,
    site: std::sync::atomic::AtomicPtr<std::panic::Location<'static>>,
    t0: AtomicU64,
    t1: AtomicU64,
}

const RECENT_N: usize = 64;
const LONG_NS: u64 = 1_000_000;
static RECENT: [Served; RECENT_N] = [const { Served::new() }; RECENT_N];
static RECENT_LONG: [Served; RECENT_N] = [const { Served::new() }; RECENT_N];
static RECENT_AT: AtomicU64 = AtomicU64::new(0);
static RECENT_LONG_AT: AtomicU64 = AtomicU64::new(0);

impl Served {
    const fn new() -> Served {
        Served {
            seq: AtomicU64::new(0),
            class: AtomicU8::new(0),
            from: AtomicU64::new(0),
            site: std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()),
            t0: AtomicU64::new(0),
            t1: AtomicU64::new(0),
        }
    }

    /// (owner only) the seqlock's writer: odd sequence, fields, even sequence (Release)
    fn write(&self, class: u8, from: u64, site: &'static std::panic::Location<'static>, t0: u64, t1: u64) {
        let s = self.seq.load(Ordering::Relaxed);
        self.seq.store(s + 1, Ordering::Relaxed);
        std::sync::atomic::fence(Ordering::Release);
        self.class.store(class, Ordering::Relaxed);
        self.from.store(from, Ordering::Relaxed);
        self.site.store(site as *const _ as *mut _, Ordering::Relaxed);
        self.t0.store(t0, Ordering::Relaxed);
        self.t1.store(t1, Ordering::Relaxed);
        self.seq.store(s + 2, Ordering::Release);
    }

    /// a consistent copy, or None (being written, or never written)
    fn read(&self) -> Option<(u8, u64, *const std::panic::Location<'static>, u64, u64)> {
        let s1 = self.seq.load(Ordering::Acquire);
        if s1 == 0 || s1 & 1 == 1 {
            return None;
        }
        let r = (self.class.load(Ordering::Relaxed), self.from.load(Ordering::Relaxed), self.site.load(Ordering::Relaxed) as *const _, self.t0.load(Ordering::Relaxed), self.t1.load(Ordering::Relaxed));
        std::sync::atomic::fence(Ordering::Acquire);
        (self.seq.load(Ordering::Relaxed) == s1).then_some(r)
    }
}

fn note_served(class: Class, from: u64, site: &'static std::panic::Location<'static>, t0: u64, t1: u64) {
    let i = RECENT_AT.fetch_add(1, Ordering::Relaxed) as usize % RECENT_N;
    RECENT[i].write(class as u8, from, site, t0, t1);
    if t1.saturating_sub(t0) >= LONG_NS {
        let i = RECENT_LONG_AT.fetch_add(1, Ordering::Relaxed) as usize % RECENT_N;
        RECENT_LONG[i].write(class as u8, from, site, t0, t1);
    }
}

/// What the owner served while a thread waited from `t0` to `t1` (wall ns) as a JSON array, longest
/// overlap first (at most 4): class, requester thread, call site, ms into the wait it began
/// (negative: before), its length and its overlap with the wait; a request still being served is
/// `"running":true`. Requests the rings no longer hold are missing (a wait behind more than 64
/// requests of >= 1 ms).
pub fn served_during(t0: u64, t1: u64) -> String {
    let mut rows: Vec<(u64, u8, u64, *const std::panic::Location<'static>, u64, u64, bool)> = Vec::new();
    for e in RECENT.iter().chain(RECENT_LONG.iter()) {
        if let Some((c, from, site, a, b)) = e.read() {
            let ov = b.min(t1).saturating_sub(a.max(t0));
            if ov > 0 && !rows.iter().any(|r| r.4 == a && r.2 == from) {
                rows.push((ov, c, from, site, a, b, false));
            }
        }
    }
    // the request on the owner now (the innermost one when it serves nested inside a wait)
    let (c, from, site, a) = (NOW_CLASS.load(Ordering::Relaxed), NOW_FROM.load(Ordering::Relaxed), NOW_SITE.load(Ordering::Relaxed) as *const _, NOW_T0.load(Ordering::Relaxed));
    if c != u8::MAX && a != 0 && a < t1 {
        let now = crate::jit::wall_ns();
        rows.push((t1.min(now).saturating_sub(a.max(t0)), c, from, site, a, now, true));
    }
    rows.sort_by(|x, y| y.0.cmp(&x.0));
    let ms = |ns: i128| ns as f64 / 1e6;
    let out: Vec<String> = rows.iter().take(4).map(|&(ov, c, from, site, a, b, running)| {
        let site = unsafe { site.as_ref() }.map_or("?".to_string(), |l| format!("{}:{}", l.file().rsplit('/').next().unwrap_or(""), l.line()));
        format!(
            "{{\"class\":\"{}\",\"thread\":{},\"site\":{},\"began_ms\":{:.1},\"ms\":{:.1},\"overlap_ms\":{:.1}{}}}",
            CLASS_NAMES.get(c as usize).copied().unwrap_or("?"),
            crate::inspect::json_str(&crate::os::thread::name(from)),
            crate::inspect::json_str(&site),
            ms(a as i128 - t0 as i128),
            ms(b.saturating_sub(a) as i128),
            ms(ov as i128),
            if running { ",\"running\":true" } else { "" }
        )
    }).collect();
    format!("[{}]", out.join(","))
}

thread_local! {
    /// rule I-CALL (OWNERARCH.DESIGN.md §3.2): this thread runs the path from a JIT call to existing
    /// code (rt_hot), where nothing may wait: a request to the owner there is a bug (debug check)
    static NO_WAIT: Cell<bool> = const { Cell::new(false) };
}

/// Marks the calling thread's I-CALL path (rt_hot) while it lives.
pub struct NoWait(bool);

impl NoWait {
    #[inline]
    pub fn enter() -> NoWait {
        NoWait(NO_WAIT.try_with(|n| n.replace(true)).unwrap_or(false))
    }
}

impl Drop for NoWait {
    #[inline]
    fn drop(&mut self) {
        let _ = NO_WAIT.try_with(|n| n.set(self.0));
    }
}

thread_local! {
    static IS_OWNER: Cell<bool> = const { Cell::new(false) };
    /// requests this thread waits for (0: none): a watchdog kick then is ignored
    static IN_CALL: Cell<u32> = const { Cell::new(0) };
    /// owner: the request being served (class, requester) and how many are open on its stack
    static SERVING: Cell<(Class, u64)> = const { Cell::new((Class::Program, 0)) };
    static DEPTH: Cell<u32> = const { Cell::new(0) };
    /// leaf locks this thread holds (`Leaf`)
    static LEAVES: Cell<u32> = const { Cell::new(0) };
    /// owner: the request being served is background work (Req::bg)
    static SERVING_BG: Cell<bool> = const { Cell::new(false) };
    /// a publish runs on this thread (`publish`: PUB_STRICT, `publish_or_bail`: PUB_BAIL, 0: none)
    static PUBLISHING: Cell<u8> = const { Cell::new(0) };
    /// this thread is one of Relax's own (run-*, gandalf*), not the program's (None: not looked up)
    static RELAX_THREAD: Cell<Option<bool>> = const { Cell::new(None) };
}

// ------------------------------------------------------------ I-PURE census

/// What background work makes the owner do that pure-or-bail (OWNERARCH.DESIGN.md §3.2 rule 3)
/// would refuse, counted before the rule exists so its bail rate is known (§4 risks):
/// - `PURE_BG_FRONT`: a build opened (begin_build) while serving background work (a publish's
///   take of RIR lowered off the owner opens one too);
/// - `PURE_BG_EVAL`: a constant or static initializer evaluated (not memoised) while serving
///   background work: background reading an unevaluated constant, a bail;
/// - `PURE_BG_JIT`: JIT code run while serving background work (includes the code initializers
///   of PURE_BG_EVAL): a bail;
/// - `PURE_AHEAD_BAIL`: a lowering ahead that unwound with NeedsUnit (bails today already);
/// - `PURE_BG_JOB`: Job requests from Relax's own threads (the denominator of their evaluations).
/// Counts are events, totals over the run; `PURE_AT_PRESENT` keeps them at the first present.
pub const PURE_BG_FRONT: usize = 0;
pub const PURE_BG_EVAL: usize = 1;
pub const PURE_BG_JIT: usize = 2;
pub const PURE_AHEAD_BAIL: usize = 3;
pub const PURE_BG_JOB: usize = 4;
pub const PURE_N: usize = 5;
pub const PURE_NAMES: [&str; PURE_N] = ["bg_fronts", "bg_evals", "bg_jit_calls", "ahead_needs_unit", "bg_jobs"];
pub static PURE_BAILS: [AtomicU64; PURE_N] = [const { AtomicU64::new(0) }; PURE_N];
pub static PURE_AT_PRESENT: [AtomicU64; PURE_N] = [const { AtomicU64::new(0) }; PURE_N];

fn relax_thread() -> bool {
    RELAX_THREAD
        .try_with(|r| match r.get() {
            Some(b) => b,
            None => {
                let name = crate::os::thread::name(me());
                let b = name.starts_with("run-") || name.starts_with("gandalf");
                r.set(Some(b));
                b
            }
        })
        .unwrap_or(false)
}

fn is_background_request(class: Class) -> bool {
    match class {
        Class::Publish | Class::Job => relax_thread(),
        _ => false,
    }
}

// ------------------------------------------------------------ publish (rule I-PUB)

const PUB_STRICT: u8 = 1;
const PUB_BAIL: u8 = 2;

pub use crate::preempt::PublishBail;

/// The bail counts of `publish_or_bail`, by cause (run summary): a front, an initializer, JIT code,
/// a wait (request, pump, front write), and builds dropped whole after their rounds
pub static BAILS: [AtomicU64; 5] = [const { AtomicU64::new(0) }; 5];
pub const BAIL_NAMES: [&str; 5] = ["front", "initializer", "jit", "wait", "dropped"];
pub const BAIL_DROPPED: usize = 4;

thread_local! {
    /// the fronts the left-out candidates of the take being served needed (note_needs)
    static TAKE_NEEDS: std::cell::RefCell<Vec<u32>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// (inside a take) a candidate it left out needed fronts for `slots`: its thread makes them for
/// the next round (take_needs)
pub fn note_needs(slots: Vec<u32>) {
    let _ = TAKE_NEEDS.try_with(|n| {
        let mut n = n.borrow_mut();
        for s in slots {
            if !n.contains(&s) {
                n.push(s);
            }
        }
    });
}

/// Drops the fronts a take noted when it ends (they are its own: a later request on the owner never
/// sees them)
struct ClearNeeds;
impl Drop for ClearNeeds {
    fn drop(&mut self) {
        let _ = TAKE_NEEDS.try_with(|n| n.borrow_mut().clear());
    }
}

/// (inside a take) the fronts noted so far
pub fn take_needs() -> Vec<u32> {
    TAKE_NEEDS.try_with(|n| std::mem::take(&mut *n.borrow_mut())).unwrap_or_default()
}

struct Publishing(u8);

impl Publishing {
    fn enter(mode: u8) -> Publishing {
        Publishing(PUBLISHING.with(|p| p.replace(mode)))
    }
}

impl Drop for Publishing {
    fn drop(&mut self) {
        let _ = PUBLISHING.try_with(|p| p.set(self.0));
    }
}

/// Called by the operations a publish may not run (a front's type check and lowering, an
/// initializer, JIT code, a request, a pump, a front write). Inside `publish` it is a bug (panic);
/// inside `publish_or_bail` the publish ends (PublishBail) and its caller fronts `slot` off the
/// owner or drops its build.
#[inline]
#[track_caller]
pub fn forbid_in_publish(what: &'static str, slot: u32) {
    match PUBLISHING.try_with(|p| p.get()).unwrap_or(0) {
        0 => {}
        PUB_BAIL => {
            let cause = match what {
                "front" => 0,
                "initializer" => 1,
                "jit" => 2,
                _ => 3,
            };
            BAILS[cause].fetch_add(1, Ordering::Relaxed);
            // MAKEPAD_RUN_BAIL_TRACE: where a background take bailed (diagnosis of the bail rate)
            if std::env::var_os("MAKEPAD_RUN_BAIL_TRACE").is_some() {
                eprintln!("makepad-run bail: {} (slot {}) at\n{}", what, slot, std::backtrace::Backtrace::force_capture());
            }
            std::panic::resume_unwind(Box::new(PublishBail { what, slots: if slot == u32::MAX { Vec::new() } else { vec![slot] } }))
        }
        _ => panic!("makepad: {} inside a publish to the compiler's owner (rule I-PUB: a publish is bounded and never runs a front, an initializer, JIT code or a wait)", what),
    }
}

/// Inside the take of a background build (`publish_or_bail`): a front bails it.
#[inline]
pub fn bailing() -> bool {
    PUBLISHING.try_with(|p| p.get()).unwrap_or(0) == PUB_BAIL
}

/// Runs `f` on the owner as a publish (rule I-PUB) and returns its result: bounded bookkeeping or
/// an install. `f` may not run what `forbid_in_publish` guards; doing so panics.
#[track_caller]
pub fn publish<R>(f: impl FnOnce() -> R) -> R {
    request(std::panic::Location::caller(), Class::Publish, false, None, move || {
        let _p = Publishing::enter(PUB_STRICT);
        f()
    })
    .unwrap()
}

/// Posts `f` as a publish (it runs later; nobody waits for it).
#[track_caller]
pub fn post_publish(f: impl FnOnce() + Send + 'static) {
    post_queued(Class::Publish, move || {
        let _p = Publishing::enter(PUB_STRICT);
        f()
    })
}

/// `publish` for the take of a background build (rule I-PURE): Err when `f` reached an operation
/// a publish may not run (`PublishBail`: what, and the slot it needed a front for). On the owner
/// itself `f` is the owner's own work (`#[relax(O3, always)]`'s first build, inspect served
/// there): it runs as it is, unguarded.
#[track_caller]
pub fn publish_or_bail<R>(f: impl FnOnce() -> R) -> Result<R, PublishBail> {
    if is_owner() || CHILD.load(Ordering::Relaxed) {
        return Ok(f());
    }
    let site = std::panic::Location::caller();
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        request(site, Class::Publish, false, None, move || {
            let _p = Publishing::enter(PUB_BAIL);
            let _ = take_needs();
            let _clear = ClearNeeds;
            f()
        })
        .unwrap()
    }));
    match r {
        Ok(r) => Ok(r),
        Err(p) => match p.downcast::<PublishBail>() {
            Ok(b) => Err(*b),
            Err(p) => std::panic::resume_unwind(p),
        },
    }
}

/// Counts census event `what` if the owner serves background work now (or, for `PURE_AHEAD_BAIL`,
/// always). One thread-local read on the paths that call it (builds, evaluations, JIT calls).
#[inline]
pub fn pure_note(what: usize) {
    if what == PURE_AHEAD_BAIL || SERVING_BG.try_with(|b| b.get()).unwrap_or(false) {
        PURE_BAILS[what].fetch_add(1, Ordering::Relaxed);
    }
}

/// The request being served is background work: a publish or Job of one of Relax's own threads.
pub fn serving_background() -> bool {
    SERVING_BG.try_with(|b| b.get()).unwrap_or(false)
}

/// The census at the first present (jit's present hook).
pub fn pure_mark_present() {
    for i in 0..PURE_N {
        PURE_AT_PRESENT[i].store(PURE_BAILS[i].load(Ordering::Relaxed), Ordering::Relaxed);
    }
}

/// The census as a JSON object (makepad inspect owner): totals and the part before the present.
pub fn pure_note_json() -> String {
    let mut out = String::from("{");
    for i in 0..PURE_N {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&format!("\"{}\":[{},{}]", PURE_NAMES[i], PURE_BAILS[i].load(Ordering::Relaxed), PURE_AT_PRESENT[i].load(Ordering::Relaxed)));
    }
    out.push('}');
    out
}

/// Held with a leaf lock (a unit's `fn_create`): while it lives this thread may not wait for the
/// owner, pump, or serve requests -- a request served there may need the same lock (the owner
/// creating a slot pumped inside it and served a build that created a slot: self-deadlock). The
/// check is a thread-local read on the waiting paths only.
pub(crate) struct Leaf(());

impl Leaf {
    pub(crate) fn enter() -> Leaf {
        let _ = LEAVES.try_with(|l| l.set(l.get() + 1));
        Leaf(())
    }
}

impl Drop for Leaf {
    fn drop(&mut self) {
        let _ = LEAVES.try_with(|l| l.set(l.get() - 1));
    }
}

#[track_caller]
fn assert_no_leaf(what: &str) {
    if LEAVES.try_with(|l| l.get()).unwrap_or(0) > 0 {
        panic!("makepad: {} while holding a leaf lock (fn_create): lock order violated, this would deadlock", what);
    }
}

fn me() -> u64 {
    // a request's thread may be named later, after it ended (os::thread::name)
    crate::os::thread::note_current();
    crate::os::thread::current()
}

/// Is this the owner thread (or the only thread of a forked child)?
#[inline]
pub fn is_owner() -> bool {
    IS_OWNER.try_with(|o| o.get()).unwrap_or(false)
}

/// Is this thread waiting for a request?
pub fn in_call() -> bool {
    IN_CALL.try_with(|c| c.get() > 0).unwrap_or(false)
}

fn in_call_add(d: i32) {
    let _ = IN_CALL.try_with(|c| c.set((c.get() as i32 + d) as u32));
}

/// The request being served on the owner: (class, requester).
pub fn serving() -> (Class, u64) {
    SERVING.with(|s| s.get())
}

/// A request served inside an earlier request's wait (owner only).
pub fn nested() -> bool {
    DEPTH.with(|d| d.get()) > 1
}

/// Is the main thread the one this work is for (it runs on main, or the owner serves main)?
pub fn for_main() -> bool {
    let main = crate::jit::MAIN_TID.load(Ordering::Relaxed);
    main != 0 && if is_owner() { serving().1 == main } else { me() == main }
}

/// The main thread waits for a request now.
pub fn main_waiting() -> bool {
    MAIN_QUEUED.load(Ordering::Relaxed) > 0
}

/// What the owner serves now (diagnostics): "idle", or the class and its requester's name.
pub fn serving_note() -> String {
    match NOW_CLASS.load(Ordering::Relaxed) {
        u8::MAX => "idle".to_string(),
        c => format!("{} for thread `{}`", CLASS_NAMES[c as usize], crate::os::thread::name(NOW_FROM.load(Ordering::Relaxed))),
    }
}

/// The request queue as a JSON object (makepad inspect; atomics only, never the queue's lock):
/// queued requests, of which foreground (main + app) and main.
pub fn queued_note() -> String {
    format!(
        "{{\"total\":{},\"foreground\":{},\"main\":{}}}",
        QUEUED_N.load(Ordering::Relaxed),
        FOREGROUND.load(Ordering::Relaxed),
        MAIN_QUEUED.load(Ordering::Relaxed)
    )
}

/// The queued requests as `class:thread` (diagnostics: an allocator swap's long wait names them).
pub fn queued_detail() -> String {
    let q = Q.lock().unwrap_or_else(|e| e.into_inner());
    let mut out = Vec::new();
    for (c, lane) in q.lanes.iter().enumerate() {
        for r in lane {
            let from = unsafe { (*(*r as *const Req)).from };
            out.push(format!("{}:{}", CLASS_NAMES[c], crate::os::thread::name(from)));
        }
    }
    out.join(" ")
}

/// What the main thread's longest post-present wait was behind (diagnostics).
pub fn main_wait_behind() -> String {
    match MAIN_WAIT_BEHIND.load(Ordering::Relaxed) {
        u64::MAX => "-".to_string(),
        c => CLASS_NAMES[c as usize & 7].to_string(),
    }
}

/// The owner thread's id (0: not started).
pub fn owner_tid() -> u64 {
    OWNER_TID.load(Ordering::Relaxed)
}

/// Starts the owner thread (once; every unit shares it).
pub fn start() {
    STARTED.call_once(|| {
        // The blocking context retains the owner's thread handle, including its name.
        let _k = crate::zone_alloc::kept();
        crate::query::set_wait_hook(crate::query::WaitHook { on_owner: is_owner, pump: pump_latch, wake, me });
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("run-owner".into())
            .stack_size(256 << 20)
            .spawn(move || {
                {
                    let _k = crate::zone_alloc::kept();
                    // std's blocking context survives every compiler generation on this thread.
                    let (_tx, rx) = std::sync::mpsc::sync_channel::<()>(0);
                    let _ = rx.recv_timeout(std::time::Duration::ZERO);
                }
                let _z = crate::zone_alloc::compiler_thread();
                IS_OWNER.with(|o| o.set(true));
                OWNER_TID.store(me(), Ordering::SeqCst);
                crate::waits::register();
                let _ = OWNER_THREAD.set(std::thread::current());
                crate::jit::ensure_altstack();
                let _ = crate::zone_alloc::kept_with(|| tx.send(()));
                owner_loop();
            })
            .expect("makepad: cannot start the compiler's owner thread");
        let _ = rx.recv();
    });
}

/// Runs `f` on the owner thread and returns its result (its panic resumes here). On the owner it
/// runs at once (nested: constant evaluation's JIT code compiling what it calls). The caller
/// waits, so `f` may borrow its locals (as `std::thread::scope`'s threads do).
#[track_caller]
pub fn call<R>(class: Class, f: impl FnOnce() -> R) -> R {
    request(std::panic::Location::caller(), class, false, None, f).unwrap()
}

/// `call` for work that reads no slot identity (the startup identities stay encoded).
#[track_caller]
pub fn call_quick<R>(class: Class, f: impl FnOnce() -> R) -> R {
    request(std::panic::Location::caller(), class, true, None, f).unwrap()
}

/// `call`, unless the owner has not started it within `ms`: then it is withdrawn (None).
#[track_caller]
pub fn call_within<R>(class: Class, ms: u64, f: impl FnOnce() -> R) -> Option<R> {
    // (at least 2 ms: an idle owner picks a request up in microseconds; 0 means "only if the
    // owner is free now")
    request(std::panic::Location::caller(), class, false, Some(std::time::Instant::now() + std::time::Duration::from_millis(ms.max(2))), f)
}

fn request<R>(site: &'static std::panic::Location<'static>, class: Class, quick: bool, deadline: Option<std::time::Instant>, f: impl FnOnce() -> R) -> Option<R> {
    forbid_in_publish("a request", u32::MAX);
    debug_assert!(!NO_WAIT.try_with(|n| n.get()).unwrap_or(false), "makepad: a request to the compiler's owner on a JIT call's path (rule I-CALL: rt_hot waits for nothing)");
    if is_owner() || CHILD.load(Ordering::Relaxed) {
        return Some(f());
    }
    assert_no_leaf("a request to the compiler's owner");
    // a front reader that must not wait (lowering ahead) gives the work back first; a back
    // end's finish is never refused (its slots stay in flight until it is served)
    if !class.is_finish() {
        crate::jit::lower_ahead_needs_unit_at(site);
        crate::preempt::point();
    }
    start();
    let mut out: Option<R> = None;
    let outp: *mut Option<R> = &mut out;
    let mut f = Some(f);
    let mut run = move || unsafe { *outp = Some((f.take().unwrap())()) };
    let run: &mut dyn FnMut() = &mut run;
    // SAFETY: the request is done (or withdrawn) before this frame returns; the owner runs the
    // closure only between this push and the Done store
    let run: *mut (dyn FnMut() + 'static) = unsafe { std::mem::transmute(run) };
    let on_main = crate::jit::MAIN_TID.load(Ordering::Relaxed) == me();
    let main_wait = on_main && class == Class::Program && crate::jit::FIRST_PRESENT.load(Ordering::Acquire) && !crate::jit::QUIT_SEEN.load(Ordering::Relaxed);
    // a thread waiting for its work on the owner is not a compile job meanwhile (an allocator
    // swap or fork waits for the jobs running; its work runs on the owner, which serves the
    // pauser): pure jobs (Job class) are, they hold front guards and queries
    let _idle = (!class.keeps_job()).then(crate::jit::JobIdle::enter);
    let req = Req {
        run,
        class,
        site,
        from: me(),
        bg: is_background_request(class),
        ctx: crate::query::context_ptr(),
        rec: crate::jit::main_rec_target(),
        quick,
        state: AtomicU8::new(QUEUED),
        panic: Cell::new(None),
        waited: true,
        t_push: if class == Class::Program && on_main { crate::jit::wall_ns().max(1) } else { 0 },
        t_serve: AtomicU64::new(0),
        t_done: AtomicU64::new(0),
        done: Mutex::new(false),
        wake: Condvar::new(),
    };
    let behind = NOW_CLASS.load(Ordering::Relaxed);
    let behind_from = NOW_FROM.load(Ordering::Relaxed);
    // SAFETY: only &'static Locations are stored
    let behind_site = unsafe { NOW_SITE.load(Ordering::Relaxed).as_ref() };
    let t_req = std::time::Instant::now();
    let t0 = if main_wait { crate::jit::wall_ns() } else { 0 };
    in_call_add(1);
    if req.t_push != 0 {
        crate::postframe::ui_wait_begin();
    }
    push(&req as *const Req as usize, class);
    let withdrawn = {
        let _w = crate::waits::begin(crate::waits::Kind::Owner, class as u64, OWNER_TID.load(Ordering::Relaxed), CLASS_NAMES[class as usize]);
        wait(&req, deadline)
    };
    in_call_add(-1);
    if req.t_push != 0 {
        crate::postframe::ui_wait_end();
        let now = crate::jit::wall_ns();
        let w = now.saturating_sub(req.t_push);
        let done = req.t_done.load(Ordering::Relaxed);
        MAIN_ACC[0].fetch_add(1, Ordering::Relaxed);
        MAIN_ACC[1].fetch_add(w, Ordering::Relaxed);
        if done != 0 {
            MAIN_ACC[3].fetch_add(now.saturating_sub(done), Ordering::Relaxed);
        }
        let b = if w < 20_000 { 4 } else if w < 200_000 { 6 } else if w < 2_000_000 { 8 } else { 10 };
        MAIN_ACC[b].fetch_add(1, Ordering::Relaxed);
        MAIN_ACC[b + 1].fetch_add(w, Ordering::Relaxed);
        let served = req.t_serve.load(Ordering::Relaxed);
        if served != 0 {
            let q = served.saturating_sub(req.t_push);
            crate::jit::main_rec_behind(q, behind, behind_from, behind_site);
            if main_wait {
                let c = if behind == u8::MAX { CLASSES } else { (behind as usize).min(CLASSES - 1) };
                MAIN_BEHIND[c][0].fetch_add(1, Ordering::Relaxed);
                MAIN_BEHIND[c][1].fetch_add(q, Ordering::Relaxed);
                if let Some(site) = behind_site {
                    let row = site_acc(site, CLASS_OF[c.min(CLASSES - 1)]);
                    row.acc[SITE_BEHIND_N].fetch_add(1, Ordering::Relaxed);
                    row.acc[SITE_BEHIND_NS].fetch_add(q, Ordering::Relaxed);
                }
            }
        }
        crate::postframe::main_owner_wait(req.t_push, w);
    }
    // makepad inspect: a real-time thread (an audio callback) that waited for the compiler
    if crate::inspect::events::watched() {
        let waited = std::time::Instant::now().duration_since(t_req);
        if waited.as_millis() >= 2 && crate::os::thread::realtime() {
            crate::inspect::events::emit(
                "rtstall",
                &format!("{}", me()),
                &format!(
                    "\"thread\":{},\"waited_ms\":{:.1},\"class\":\"{}\",\"owner_serves\":{},\"more\":[\"why-slow\",\"timeline start\"]",
                    crate::inspect::json_str(&crate::os::thread::name(me())),
                    waited.as_secs_f64() * 1e3,
                    CLASS_NAMES[class as usize],
                    crate::inspect::json_str(&serving_note())
                ),
            );
        }
    }
    if main_wait {
        let w = crate::jit::wall_ns() - t0;
        if MAIN_WAIT_MAX.fetch_max(w, Ordering::Relaxed) < w {
            MAIN_WAIT_BEHIND.store(behind as u64, Ordering::Relaxed);
        }
    }
    if withdrawn {
        return None;
    }
    match req.panic.take() {
        // served nested and blocked by an earlier request on the owner: the requester's
        // work is cancelled as a preemption is (its queries too) and redone later. Only
        // Job requests are served nested, and those that can be blocked come from type
        // checks (a constant's value), which retry a preempted body.
        Some(p) if p.is::<crate::preempt::OwnerBusy>() => {
            std::panic::resume_unwind(Box::new(crate::preempt::Preempted));
        }
        Some(p) => std::panic::resume_unwind(p),
        None => Some(out.unwrap()),
    }
}

/// Posts `f` to the owner and returns at once (it runs later, in class order).
#[track_caller]
pub fn post(class: Class, f: impl FnOnce() + Send + 'static) {
    if is_owner() {
        f();
        return;
    }
    post_queued(class, f);
}

/// `post`, but queued also when called on the owner (`post` runs `f` at once there): work the
/// owner hands itself for later, after the requests waiting now.
#[track_caller]
pub fn post_queued(class: Class, f: impl FnOnce() + Send + 'static) {
    if CHILD.load(Ordering::Relaxed) {
        f();
        return;
    }
    start();
    let mut f = Some(f);
    let run: Box<dyn FnMut()> = Box::new(move || (f.take().unwrap())());
    let req = Box::new(Req {
        run: Box::into_raw(run),
        class,
        site: std::panic::Location::caller(),
        from: me(),
        bg: is_background_request(class),
        ctx: std::ptr::null(),
        rec: std::ptr::null(),
        quick: false,
        state: AtomicU8::new(QUEUED),
        panic: Cell::new(None),
        waited: false,
        t_push: 0,
        t_serve: AtomicU64::new(0),
        t_done: AtomicU64::new(0),
        done: Mutex::new(false),
        wake: Condvar::new(),
    });
    push(Box::into_raw(req) as usize, class);
}

fn push(r: usize, class: Class) {
    let mut q = Q.lock().unwrap_or_else(|e| e.into_inner());
    q.lanes[class as usize].push_back(r);
    if class == Class::Program {
        FOREGROUND.fetch_add(1, Ordering::Relaxed);
        if from_main(r) {
            MAIN_QUEUED.fetch_add(1, Ordering::Relaxed);
        }
    }
    QUEUED_N.fetch_add(1, Ordering::SeqCst);
    if OWNER_ASLEEP.load(Ordering::SeqCst) {
        WAKE.notify_all();
    }
}

fn dequeued(class: usize, r: usize) {
    QUEUED_N.fetch_sub(1, Ordering::SeqCst);
    if class == Class::Program as usize {
        FOREGROUND.fetch_sub(1, Ordering::Relaxed);
        if from_main(r) {
            MAIN_QUEUED.fetch_sub(1, Ordering::Relaxed);
        }
    }
}

/// A queued request asked by the program's main thread.
fn from_main(r: usize) -> bool {
    // SAFETY: a queued request stays alive until the owner releases it
    let from = unsafe { (*(r as *const Req)).from };
    from != 0 && from == crate::jit::MAIN_TID.load(Ordering::Relaxed)
}

/// Waits for `req` to be done; true: withdrawn at the deadline before it started.
fn wait(req: &Req, deadline: Option<std::time::Instant>) -> bool {
    let behind_other = OWNER_SERVING.load(Ordering::Relaxed) && req.state.load(Ordering::Relaxed) == QUEUED;
    if !behind_other && !OWNER_ASLEEP.load(Ordering::Relaxed) && QUEUED_N.load(Ordering::Relaxed) <= 1 {
        for _ in 0..spin_iters(REQUESTER_SPIN_US) {
            if req.state.load(Ordering::Acquire) == RELEASED {
                return false;
            }
            std::hint::spin_loop();
        }
    }
    let mut done = req.done.lock().unwrap_or_else(|e| e.into_inner());
    while !*done {
        match deadline {
            Some(d) if req.state.load(Ordering::Acquire) == QUEUED => {
                let now = std::time::Instant::now();
                if now >= d {
                    drop(done);
                    let mut q = Q.lock().unwrap_or_else(|e| e.into_inner());
                    // the owner takes a request out of its lane under this mutex: still in the
                    // lane = not started
                    let lane = &mut q.lanes[req.class as usize];
                    if let Some(i) = lane.iter().position(|x| *x == req as *const Req as usize) {
                        lane.remove(i);
                        dequeued(req.class as usize, req as *const Req as usize);
                        return true;
                    }
                    drop(q);
                    done = req.done.lock().unwrap_or_else(|e| e.into_inner());
                    continue;
                }
                done = req.wake.wait_timeout(done, d - now).unwrap_or_else(|e| e.into_inner()).0;
            }
            _ => done = req.wake.wait(done).unwrap_or_else(|e| e.into_inner()),
        }
    }
    drop(done);
    // The owner's last touch follows its unlock. The unlock's wake can put this thread on the
    // owner's CPU and preempt it right there, before that store: spinning then holds the CPU the
    // owner needs to make it (Linux, cold Stage: 8 s of the app thread's CPU in this loop, most of
    // it after the wait). A few spins, then yield.
    let mut spins = 0u32;
    while req.state.load(Ordering::Acquire) != RELEASED {
        if spins < 64 {
            spins += 1;
            std::hint::spin_loop();
        } else {
            std::thread::yield_now();
        }
    }
    false
}

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    /// the owner's idle loop: everything, in class order
    Top,
    /// a wait inside a request: Job and Publish only (Class::keeps_job: what the awaited thread
    /// may be waiting for; a program thread's request waits for the owner's current one to end)
    Pump,
}

fn pick(q: &mut Queue, mode: Mode) -> Option<usize> {
    // the process ends (only the exit set's requests), or the program swaps its allocator (only
    // the pauser's): other threads' foreground and background requests wait
    let (only, pause) = match EXIT_TID.load(Ordering::Acquire) {
        0 => (crate::jit::paused_for(), true),
        e => (e, false),
    };
    let helper = EXIT_HELPER.load(Ordering::Acquire);
    for c in SERVICE_ORDER {
        // a pump serves only what the awaited thread may be waiting for: a job's request, a
        // publish. Never a program thread's request: it opens builds of its own on top of the
        // request that pumps -- which holds its own state open meanwhile (Sandbox's race game hung
        // on a speculative build served inside a main-thread request's pump, 2026-10-21; builds
        // are no publishes since rule I-PUB)
        if mode == Mode::Pump && !CLASS_OF[c].keeps_job() {
            continue;
        }
        let lane = &mut q.lanes[c];
        // a pause serves every request that keeps its job running (Class::keeps_job)
        let held_back = c >= Class::Program as usize && !(pause && CLASS_OF[c].keeps_job());
        let at = if only != 0 && held_back {
            lane.iter().position(|r| {
                let from = unsafe { (*(*r as *const Req)).from };
                from == only || from == helper && helper != 0
            })
        } else if lane.is_empty() {
            None
        } else {
            Some(0)
        };
        if let Some(i) = at {
            let r = lane.remove(i).unwrap();
            dequeued(c, r);
            return Some(r);
        }
    }
    None
}

fn serve(r: usize) {
    let req = unsafe { &*(r as *const Req) };
    req.state.store(RUNNING, Ordering::Release);
    let was_serving = OWNER_SERVING.swap(true, Ordering::Relaxed);
    let t0 = crate::jit::wall_ns();
    if req.t_push != 0 {
        MAIN_ACC[2].fetch_add(t0.saturating_sub(req.t_push), Ordering::Relaxed);
        req.t_serve.store(t0.max(1), Ordering::Relaxed);
    }
    let prev_ctx = unsafe { crate::query::swap_context(req.ctx) };
    let prev = SERVING.with(|s| s.replace((req.class, req.from)));
    let prev_bg = SERVING_BG.with(|b| b.replace(req.bg));
    if req.class == Class::Job && req.bg {
        PURE_BAILS[PURE_BG_JOB].fetch_add(1, Ordering::Relaxed);
    }
    let prev_now = (NOW_CLASS.swap(req.class as u8, Ordering::Relaxed), NOW_FROM.swap(req.from, Ordering::Relaxed));
    let prev_site = NOW_SITE.swap(req.site as *const std::panic::Location<'static> as *mut _, Ordering::Relaxed);
    let prev_t0 = NOW_T0.swap(t0.max(1), Ordering::Relaxed);
    DEPTH.with(|d| d.set(d.get() + 1));
    let frame = crate::jit::frame_enter(req.rec);
    // makepad inspect's timeline: the owner's service of this request is a span
    let traced = crate::inspect::timeline::recording();
    if traced {
        crate::inspect::timeline::push(crate::inspect::timeline::Kind::SpanBegin, SERVE_NAMES[req.class as usize], req.from, crate::inspect::pstream::SITE | (req.site as *const std::panic::Location<'static> as u64));
    }
    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if !req.quick {
            crate::jit::ids_on_serve();
        }
        unsafe { (*req.run)() }
    }));
    let busy = crate::jit::frame_leave(frame);
    if traced {
        crate::inspect::timeline::push(crate::inspect::timeline::Kind::SpanEnd, SERVE_NAMES[req.class as usize], req.from, 0);
    }
    // a compile the request's code reached was cancelled and nothing cancelled the request yet:
    // its result is not used
    let res = match res {
        Ok(()) if busy => Err(Box::new(crate::preempt::OwnerBusy) as Box<dyn Any + Send>),
        r => r,
    };
    // MAKEPAD_RUN_OWNER_TRACE: a request's panic, with where it happened on the owner (the compiler's
    // panic hook is silent there; the requester resumes it)
    if let Err(p) = &res {
        if std::env::var_os("MAKEPAD_RUN_OWNER_TRACE").is_some() {
            let msg = p.downcast_ref::<String>().cloned().or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_else(|| "(cancellation)".to_string());
            eprintln!("makepad-run owner: a {:?} request of thread `{}` panicked on the owner: {}{}", req.class, crate::os::thread::name(req.from), msg, crate::jit::take_ice_location());
        }
    }
    DEPTH.with(|d| d.set(d.get() - 1));
    NOW_CLASS.store(prev_now.0, Ordering::Relaxed);
    NOW_FROM.store(prev_now.1, Ordering::Relaxed);
    NOW_SITE.store(prev_site, Ordering::Relaxed);
    NOW_T0.store(prev_t0, Ordering::Relaxed);
    SERVING.with(|s| s.set(prev));
    SERVING_BG.with(|b| b.set(prev_bg));
    unsafe { crate::query::swap_context(prev_ctx) };
    OWNER_SERVING.store(was_serving, Ordering::Relaxed);
    let t1 = crate::jit::wall_ns();
    BUSY_NS[req.class as usize].fetch_add(t1.saturating_sub(t0), Ordering::Relaxed);
    let row = site_acc(req.site, req.class);
    row.acc[SITE_REQS].fetch_add(1, Ordering::Relaxed);
    row.acc[SITE_NS].fetch_add(t1.saturating_sub(t0), Ordering::Relaxed);
    if t1.saturating_sub(t0) >= 1_000_000 {
        row.acc[SITE_LONG_N].fetch_add(1, Ordering::Relaxed);
    }
    row.acc[SITE_MAX_NS].fetch_max(t1.saturating_sub(t0), Ordering::Relaxed);
    note_served(req.class, req.from, req.site, t0, t1);
    crate::postframe::owner_served(req.class as usize, req.site, req.from, t0, t1, FOREGROUND.load(Ordering::Relaxed));
    // the process is ending on another thread: this request is never answered (its requester
    // waits until the process is gone)
    if res.as_ref().is_err_and(|p| p.is::<ExitParked>()) {
        return;
    }
    match req.waited {
        true => {
            if let Err(p) = res {
                req.panic.set(Some(p));
            }
            if traced {
                // the edge that ends the requester's wait
                crate::inspect::timeline::push(crate::inspect::timeline::Kind::Served, "", req.from, 0);
            }
            if req.t_push != 0 {
                req.t_done.store(crate::jit::wall_ns(), Ordering::Relaxed);
            }
            req.state.store(DONE, Ordering::Release);
            {
                let mut done = req.done.lock().unwrap_or_else(|e| e.into_inner());
                *done = true;
                req.wake.notify_one();
            }
            // the last touch: after this store the requester may return and `req` is gone
            req.state.store(RELEASED, Ordering::Release);
        }
        false => unsafe {
            drop(Box::from_raw(req.run));
            drop(Box::from_raw(r as *mut Req));
            if let Err(p) = res {
                if !crate::preempt::is_cancelled(&*p) {
                    let msg = p.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| p.downcast_ref::<String>().cloned()).unwrap_or_default();
                    eprintln!("makepad: a posted compiler task panicked: {}", msg);
                }
            }
        },
    }
}

fn owner_loop() -> ! {
    calibrate();
    loop {
        // idle: a short spin catches the next request of a burst without a sleep and wake
        for _ in 0..spin_iters(OWNER_SPIN_US) {
            if QUEUED_N.load(Ordering::Relaxed) != 0 || FORK.load(Ordering::Relaxed) == 1 {
                break;
            }
            std::hint::spin_loop();
        }
        let r = {
            let mut q = Q.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                // a fork: parked here, between two requests (no request's state half done)
                if FORK.load(Ordering::SeqCst) == 1 {
                    drop(q);
                    FORK.store(2, Ordering::SeqCst);
                    while FORK.load(Ordering::SeqCst) == 2 {
                        std::thread::park();
                    }
                    q = Q.lock().unwrap_or_else(|e| e.into_inner());
                    continue;
                }
                if let Some(r) = pick(&mut q, Mode::Top) {
                    break r;
                }
                OWNER_ASLEEP.store(true, Ordering::SeqCst);
                q = WAKE.wait(q).unwrap_or_else(|e| e.into_inner());
                OWNER_ASLEEP.store(false, Ordering::SeqCst);
            }
        };
        serve(r);
        // between top-level requests: the vtables made for real-time trait-object types
        let frame = crate::jit::frame_enter(std::ptr::null());
        let _ = std::panic::catch_unwind(crate::jit::realtime_drain);
        crate::jit::frame_leave(frame);
    }
}

/// The owner waits for `done`, serving meanwhile what may be holding it up: Job and Finish
/// requests (a type check's constant, a back end's finish). On another thread it waits
/// plainly. `done` only reads state or takes leaf locks.
pub fn pump_wait(done: &dyn Fn() -> bool) {
    pump_wait_for(0, done)
}

/// `pump_wait` for a wait on thread `runner` (0: unknown), as the wait registry records it.
fn pump_wait_for(runner: u64, done: &dyn Fn() -> bool) {
    if done() {
        return;
    }
    assert_no_leaf("a wait that serves the owner's requests");
    forbid_in_publish("a wait", u32::MAX);
    let _w = crate::waits::begin(crate::waits::Kind::Pump, 0, runner, "pump");
    if !is_owner() || CHILD.load(Ordering::Relaxed) {
        // a job waiting for another thread is not running meanwhile (jit::job_sleep's reason)
        let _idle = (!CHILD.load(Ordering::Relaxed)).then(crate::jit::JobIdle::enter);
        let t0 = std::time::Instant::now();
        while !done() {
            if CHILD.load(Ordering::Relaxed) && t0.elapsed().as_secs() >= 10 {
                eprintln!("makepad: forked child waited 10 s for work of a thread of the parent -- exiting");
                crate::os::thread::exit_now(134);
            }
            std::thread::sleep(std::time::Duration::from_micros(200));
        }
        return;
    }
    loop {
        if done() {
            return;
        }
        let r = {
            let mut q = Q.lock().unwrap_or_else(|e| e.into_inner());
            match pick(&mut q, Mode::Pump) {
                Some(r) => Some(r),
                None => {
                    // a latch completing or a request arriving wakes it; the timeout bounds a
                    // condition nothing announces (an in-flight flag cleared by a Finish)
                    OWNER_ASLEEP.store(true, Ordering::SeqCst);
                    let _ = WAKE.wait_timeout(q, std::time::Duration::from_millis(1));
                    OWNER_ASLEEP.store(false, Ordering::SeqCst);
                    None
                }
            }
        };
        if let Some(r) = r {
            serve(r);
        }
    }
}

/// The query engine's wait for a query another thread runs (or, nested, a lower frame of this
/// one): on the owner a pump; a query running lower on the owner's own stack cannot finish
/// before this request does, so a nested request is cancelled instead.
fn pump_latch(runner: u64, done: &dyn Fn() -> bool) {
    if runner != 0 && runner == OWNER_TID.load(Ordering::Relaxed) && !done() {
        if nested() {
            std::panic::resume_unwind(Box::new(crate::preempt::OwnerBusy));
        }
        panic!("makepad: a query waits for itself on the compiler's owner thread (a query cycle the engine did not see)");
    }
    pump_wait_for(runner, done);
}

/// A request served nested cannot do what only the owner's earlier request may finish (a load,
/// a front write, waiting for a back end): it is cancelled and posted again later.
pub fn busy_if_nested() {
    if is_owner() && nested() {
        std::panic::resume_unwind(Box::new(crate::preempt::OwnerBusy));
    }
}

/// Wakes a pumping owner (a query latch it may wait for completed).
pub fn wake() {
    if CHILD.load(Ordering::Relaxed) || !STARTED.is_completed() {
        return;
    }
    let _q = Q.lock().unwrap_or_else(|e| e.into_inner());
    WAKE.notify_all();
}

/// The process is ending on this thread: from now on the owner serves only its requests and its
/// exit helper's (and Job / Finish ones); other threads' requests stay queued until the process
/// is gone.
pub fn begin_exit() {
    let _ = EXIT_TID.compare_exchange(0, me(), Ordering::AcqRel, Ordering::Acquire);
    wake();
}

/// This thread works for the exiting one (run.rs's exit worker): its requests are served too.
pub fn exit_helper() {
    EXIT_HELPER.store(me(), Ordering::Release);
}

/// The thread ending the process (0: none).
pub fn exit_tid() -> u64 {
    EXIT_TID.load(Ordering::Acquire)
}

/// The request being served is the exit's (the exiting thread's or its helper's).
pub fn serving_exit() -> bool {
    let from = serving().1;
    from == EXIT_TID.load(Ordering::Acquire) || from != 0 && from == EXIT_HELPER.load(Ordering::Acquire)
}

/// The unwind payload of a request left unanswered because the process is ending.
pub struct ExitParked;

/// Ends the request being served without an answer: the process is ending on another thread.
pub fn park_for_exit() -> ! {
    std::panic::resume_unwind(Box::new(ExitParked))
}

/// The owner serves nothing and nothing is queued (the settle swap's drain).
pub fn idle() -> bool {
    NOW_CLASS.load(Ordering::Acquire) == u8::MAX && QUEUED_N.load(Ordering::SeqCst) == 0
}

/// The owner is serving a request of another thread than `tid` now.
pub fn busy_for_other(tid: u64) -> bool {
    NOW_CLASS.load(Ordering::Acquire) != u8::MAX && NOW_FROM.load(Ordering::Acquire) != tid
}

/// The allocator swap ended: the owner serves everyone again.
pub fn resume() {
    wake();
}

// ------------------------------------------------------------ fork

/// fork() in a program run by Relax: the owner is parked between two requests across the fork
/// (holding no lock), so the child -- which has no owner thread -- finds the units consistent
/// and runs every request on its one thread. 0: no fork, 1: asked, 2: owner parked, 3: released.
static FORK: AtomicU8 = AtomicU8::new(0);

/// One fork at a time (two threads forking at once would release each other's parked owner).
static FORKING: AtomicBool = AtomicBool::new(false);

pub fn fork_prepare() {
    if is_owner() || CHILD.load(Ordering::Relaxed) || !STARTED.is_completed() {
        return;
    }
    while FORKING.swap(true, Ordering::SeqCst) {
        std::thread::yield_now();
    }
    FORK.store(1, Ordering::SeqCst);
    wake();
    while FORK.load(Ordering::SeqCst) != 2 {
        std::thread::yield_now();
    }
}

pub fn fork_parent() {
    if FORK.load(Ordering::SeqCst) != 2 || CHILD.load(Ordering::Relaxed) {
        return;
    }
    FORK.store(3, Ordering::SeqCst);
    if let Some(t) = OWNER_THREAD.get() {
        t.unpark();
    }
    FORKING.store(false, Ordering::SeqCst);
}

pub fn fork_child() {
    CHILD.store(true, Ordering::SeqCst);
    FORKING.store(false, Ordering::SeqCst);
    IS_OWNER.with(|o| o.set(true));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Sandbox race-game hang (2026-10-21): the owner, serving a main-thread request, pumped
    /// inside it (a query latch) and took a queued background request -- a speculative build --
    /// which reached the slot lock the outer request held. A pump serves Job and Publish requests
    /// only; a program thread's request waits for the top of the owner loop.
    #[test]
    fn a_pump_serves_jobs_but_no_program_request() {
        isolated_test!(a_pump_serves_jobs_but_no_program_request);
        static APP_RAN: AtomicBool = AtomicBool::new(false);
        static JOB_RAN: AtomicBool = AtomicBool::new(false);
        call(Class::Program, || {
            assert!(is_owner());
            post_queued(Class::Program, || APP_RAN.store(true, Ordering::SeqCst));
            // the job comes only once the pump has looked at the queue and gone to sleep
            let job = std::thread::spawn(|| {
                while !OWNER_ASLEEP.load(Ordering::SeqCst) {
                    std::thread::yield_now();
                }
                call(Class::Job, || JOB_RAN.store(true, Ordering::SeqCst));
            });
            pump_wait(&|| JOB_RAN.load(Ordering::SeqCst));
            assert!(!APP_RAN.load(Ordering::SeqCst), "a program request was served inside a pump");
            job.join().unwrap();
        });
        // the owner's top loop serves it once the request ended
        call(Class::Program, || {});
        assert!(APP_RAN.load(Ordering::SeqCst));
    }

    /// Rule I-PUB: a publish that asks the owner (or pumps) is a bug and panics; under
    /// publish_or_bail it is a bail naming the slot, and the owner serves on.
    #[test]
    fn a_publish_never_waits() {
        isolated_test!(a_publish_never_waits);
        let r = std::thread::spawn(|| publish(|| call(Class::Program, || {}))).join();
        let p = r.expect_err("a request inside a publish panics");
        let msg = p.downcast_ref::<String>().cloned().unwrap_or_default();
        assert!(msg.contains("inside a publish"), "{msg}");
        let b = std::thread::spawn(|| publish_or_bail(|| forbid_in_publish("front", 7))).join().unwrap();
        assert_eq!(b.err().map(|b| b.slots), Some(vec![7]));
        let b = std::thread::spawn(|| publish_or_bail(|| pump_wait(&|| false))).join().unwrap();
        assert_eq!(b.err().map(|b| (b.slots, b.what)), Some((Vec::new(), "a wait")));
        // on the owner it is the owner's own work: no guard
        assert_eq!(call(Class::Program, || publish_or_bail(|| 4).ok()), Some(4));
        assert_eq!(publish(|| 5), 5);
        assert_eq!(call(Class::Program, || 6), 6);
    }

    /// The served-request rings name what the owner served during a wait (hitch events).
    #[test]
    fn served_during_names_the_owners_requests() {
        isolated_test!(served_during_names_the_owners_requests);
        let t0 = crate::jit::wall_ns();
        call(Class::Program, || std::thread::sleep(std::time::Duration::from_millis(3)));
        let t1 = crate::jit::wall_ns();
        let s = served_during(t0, t1);
        assert!(s.contains("\"class\":\"program\"") && s.contains("owner.rs:"), "{s}");
        assert_eq!(served_during(t1 + 1_000_000_000, t1 + 2_000_000_000), "[]");
    }

    /// A thread holding a leaf lock (fn_create) that asks the owner, or pumps, is a lock-order
    /// violation: it panics instead of deadlocking.
    #[test]
    fn a_wait_under_a_leaf_lock_panics() {
        isolated_test!(a_wait_under_a_leaf_lock_panics);
        let r = std::thread::spawn(|| {
            let _leaf = Leaf::enter();
            call(Class::Program, || {});
        })
        .join();
        let p = r.expect_err("a request under a leaf lock panics");
        let msg = p.downcast_ref::<String>().cloned().unwrap_or_default();
        assert!(msg.contains("leaf lock"), "{msg}");
        // the owner itself, pumping under one
        let r = std::panic::catch_unwind(|| call(Class::Program, || {
            let _leaf = Leaf::enter();
            pump_wait(&|| false);
        }));
        assert!(r.is_err(), "a pump under a leaf lock panics");
        // and the guard is released by the unwind: requests work again
        assert_eq!(call(Class::Program, || 5), 5);
    }
}
