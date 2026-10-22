//! The flight recorder: always on (MAKEPAD_RUN_FLIGHTREC=0: off), bounded memory, never
//! blocking the threads it records. What a session did in its last minutes stays in memory, so a
//! hitch, a panic or a game-logic bug seen once can be read (and replayed) afterwards.
//!
//! - Marker rings (pstream.rs): every thread's spans, waits, wakes and the owner's service of each
//!   request (class, requester, call site), per thread, lock-free (the timeline's markers).
//! - Fixed-slot rings here (lock-free, seqlock slots, overwritten when full): events (app marks,
//!   log lines, panics and their decisions, script errors, zone swaps, cache writes, hitches,
//!   inspect events), the program's input events (as the remote commands that replay them, with
//!   their frame), JIT installs (slot, tier, origin, bytes; names are read at dump time) and one
//!   summary per UI frame (time, idle, lazy compiles, waits, waits on the owner).
//! - Stall samples: while the UI thread waits on the JIT for more than STALL_NS, or a frame runs
//!   longer than FRAME_STALL_NS, this module's thread samples the UI and owner threads' stacks
//!   (what both sides were doing); raw pcs are named when a recording is written.
//!
//! A recording is a `.mkpf` profile file (makepad-prof-stream; profproto.md, kinds EVENT 16 and
//! INPUT 17): `rec save [last=60s]`, and automatically on a hitch over DUMP_MS
//! (MAKEPAD_RUN_FLIGHTREC_DUMP_MS, default 1000), a panic and a fatal fault, into
//! <makepad run home>/flightrec/. `makepad inspect file=<x.mkpf> pf ...` answers on it.

use makepad_prof_stream as ps;
use relax_session::sync::Mutex;
use std::cell::UnsafeCell;
use std::sync::atomic::{fence, AtomicBool, AtomicU64, Ordering::{Acquire, Relaxed, Release}};

static ON: AtomicBool = AtomicBool::new(false);

/// The recorder records (inspect started it; MAKEPAD_RUN_FLIGHTREC=0 keeps it off).
#[inline]
pub fn on() -> bool {
    ON.load(Relaxed)
}

/// UI waits on the JIT longer than this are sampled (both sides)
const STALL_NS: u64 = 150_000_000;
/// a UI phase (one event's dispatch, a draw) running longer than this is sampled
const FRAME_STALL_NS: u64 = 250_000_000;
/// auto dumps at most this often, and how far back they reach
const DUMP_GAP_NS: u64 = 30_000_000_000;
const DUMP_LAST_NS: u64 = 120_000_000_000;
/// a hitch's recording reaches back this far (it is written right after the hitch)
const HITCH_LAST_NS: u64 = 60_000_000_000;
/// automatic recordings kept in the folder (older `auto-` files are removed)
const AUTO_KEEP: usize = 12;

static DUMP_MS: AtomicU64 = AtomicU64::new(1000);
/// how the run was started (a replay starts the same): (args, cwd, MAKEPAD_* environment)
static RUN: Mutex<Option<(String, String, String)>> = Mutex::new(None);
static THREAD: Mutex<Option<std::thread::Thread>> = Mutex::new(None);

/// Starts the recorder (inspect's start).
pub fn init() {
    if std::env::var("MAKEPAD_RUN_FLIGHTREC").is_ok_and(|v| v == "0") {
        return;
    }
    if let Some(ms) = std::env::var("MAKEPAD_RUN_FLIGHTREC_DUMP_MS").ok().and_then(|v| v.parse::<u64>().ok()) {
        DUMP_MS.store(ms, Relaxed);
    }
    if let Some(us) = std::env::var("MAKEPAD_RUN_FLIGHTREC_ELIDE_US").ok().and_then(|v| v.parse::<u64>().ok()) {
        super::pstream::set_elide_us(us);
    }
    let _k = crate::zone_alloc::kept();
    let args: Vec<String> = std::env::args().collect();
    let cwd = std::env::current_dir().map(|d| d.display().to_string()).unwrap_or_default();
    let env: Vec<String> = std::env::vars().filter(|(k, _)| k.starts_with("MAKEPAD") || k.starts_with("SANDBOX_")).map(|(k, v)| format!("{k}={v}")).collect();
    *RUN.lock().unwrap_or_else(|e| e.into_inner()) = Some((args.join("\u{1f}"), cwd, env.join("\u{1f}")));
    // the rings exist before the first record (no allocation on a recording thread later)
    let _ = (events(), app_events(), inputs(), jits(), frames());
    ON.store(true, Release);
    super::timeline::flag_update();
    let t = std::thread::Builder::new().name("run-flightrec".into()).spawn(watch);
    if let Ok(h) = t {
        *THREAD.lock().unwrap_or_else(|e| e.into_inner()) = Some(h.thread().clone());
    }
}

// ------------------------------------------------------------------ fixed-slot rings

/// A ring of `T` slots any thread writes without a lock: a writer takes a sequence number, marks
/// its slot busy, writes, and stores the number + 1 (a seqlock); a reader keeps a slot only when
/// that number is the same before and after its copy. Overwritten when full.
pub(super) struct Slots<T: Copy> {
    head: AtomicU64,
    seq: Box<[AtomicU64]>,
    data: Box<[UnsafeCell<std::mem::MaybeUninit<T>>]>,
}

unsafe impl<T: Copy + Send> Sync for Slots<T> {}

impl<T: Copy> Slots<T> {
    fn new(n: usize) -> Slots<T> {
        assert!(n.is_power_of_two());
        Slots { head: AtomicU64::new(0), seq: (0..n).map(|_| AtomicU64::new(0)).collect(), data: (0..n).map(|_| UnsafeCell::new(std::mem::MaybeUninit::uninit())).collect() }
    }

    fn push(&self, v: T) {
        let n = self.head.fetch_add(1, Relaxed);
        let i = n as usize & (self.seq.len() - 1);
        self.seq[i].store(0, Relaxed);
        fence(Release);
        unsafe { std::ptr::write_volatile(self.data[i].get(), std::mem::MaybeUninit::new(v)) };
        self.seq[i].store(n + 1, Release);
    }

    /// Entries `from..` (at most a ring back): `f(n, entry)`. Returns the cursor to read from next
    /// time and the entries lost (overwritten before or while they were read).
    fn read(&self, from: u64, f: &mut dyn FnMut(u64, T)) -> (u64, u64) {
        let len = self.seq.len() as u64;
        let head = self.head.load(Acquire);
        let mut lost = 0;
        let mut n = from.min(head);
        if head - n > len {
            lost += head - n - len;
            n = head - len;
        }
        while n < head {
            let i = n as usize & (len as usize - 1);
            let s1 = self.seq[i].load(Acquire);
            if s1 != n + 1 {
                if s1 < n + 1 && head - n < 64 {
                    // still being written: read it next time
                    return (n, lost);
                }
                lost += 1;
                n += 1;
                continue;
            }
            let v = unsafe { std::ptr::read_volatile(self.data[i].get()).assume_init() };
            fence(Acquire);
            if self.seq[i].load(Relaxed) != s1 {
                lost += 1;
            } else {
                f(n, v);
            }
            n += 1;
        }
        (head, lost)
    }

    fn bytes(&self) -> usize {
        self.seq.len() * (8 + std::mem::size_of::<T>())
    }

    /// (entries written, ring length)
    fn fill(&self) -> (u64, u64) {
        (self.head.load(Relaxed), self.seq.len() as u64)
    }
}

/// A short text copied into a slot (cut at a char boundary).
fn copy_text<const N: usize>(s: &str) -> ([u8; N], u16) {
    let mut b = [0u8; N];
    let mut n = s.len().min(N);
    while n > 0 && !s.is_char_boundary(n) {
        n -= 1;
    }
    b[..n].copy_from_slice(&s.as_bytes()[..n]);
    (b, n as u16)
}

fn text_of(b: &[u8], n: u16) -> &str {
    std::str::from_utf8(&b[..(n as usize).min(b.len())]).unwrap_or("")
}

#[derive(Clone, Copy)]
struct Ev {
    t: u64,
    os_id: u64,
    a: u64,
    b: u64,
    cat_len: u16,
    len: u16,
    cat: [u8; 28],
    text: [u8; 168],
}

#[derive(Clone, Copy)]
struct In {
    t: u64,
    frame: u64,
    len: u8,
    /// bit0: injected (makepad remote), not the person's
    flags: u8,
    text: [u8; 78],
}

#[derive(Clone, Copy)]
struct Jit {
    t: u64,
    os_id: u64,
    id: u32,
    len: u32,
    tier: u8,
    origin: u8,
}

#[derive(Clone, Copy)]
struct Fr {
    t0: u64,
    t1: u64,
    idle: u64,
    compile_ns: u64,
    wait_ns: u64,
    owner_ns: u64,
    compiles: u32,
    waits: u32,
}

static EVENTS: std::sync::OnceLock<Slots<Ev>> = std::sync::OnceLock::new();
/// the app's own marks (game events): a ring of their own, so a flood of them never pushes out
/// the host's (panics, hitches, logs)
static APP_EVENTS: std::sync::OnceLock<Slots<Ev>> = std::sync::OnceLock::new();
static INPUTS: std::sync::OnceLock<Slots<In>> = std::sync::OnceLock::new();
static JITS: std::sync::OnceLock<Slots<Jit>> = std::sync::OnceLock::new();
static FRAMES: std::sync::OnceLock<Slots<Fr>> = std::sync::OnceLock::new();

fn events() -> &'static Slots<Ev> {
    EVENTS.get_or_init(|| crate::zone_alloc::kept_with(|| Slots::new(1 << 14)))
}
fn app_events() -> &'static Slots<Ev> {
    APP_EVENTS.get_or_init(|| crate::zone_alloc::kept_with(|| Slots::new(1 << 14)))
}

/// The program's categories that are the host's or the platform's (log lines, seeds, the
/// window, replays): they go with the host's events.
fn host_cat(cat: &str) -> bool {
    ["log", "seed.", "window.", "replay.", "script."].iter().any(|p| cat.starts_with(p))
}
fn inputs() -> &'static Slots<In> {
    INPUTS.get_or_init(|| crate::zone_alloc::kept_with(|| Slots::new(1 << 16)))
}
fn jits() -> &'static Slots<Jit> {
    JITS.get_or_init(|| crate::zone_alloc::kept_with(|| Slots::new(1 << 16)))
}
fn frames() -> &'static Slots<Fr> {
    FRAMES.get_or_init(|| crate::zone_alloc::kept_with(|| Slots::new(1 << 16)))
}

/// Stall samples (raw pcs; named when a recording is written) and memory counters: written by
/// the recorder's thread only, so a lock no recording thread takes.
struct SeqLog<T> {
    base: u64,
    v: std::collections::VecDeque<T>,
    max: usize,
}

impl<T: Clone> SeqLog<T> {
    const fn new(max: usize) -> Self {
        SeqLog { base: 0, v: std::collections::VecDeque::new(), max }
    }
    fn push(&mut self, x: T) {
        if self.v.len() >= self.max {
            self.v.pop_front();
            self.base += 1;
        }
        self.v.push_back(x);
    }
    fn read(&self, cursor: &mut u64) -> Vec<T> {
        let from = (*cursor).max(self.base);
        let out = self.v.iter().skip((from - self.base) as usize).cloned().collect();
        *cursor = self.base + self.v.len() as u64;
        out
    }
}

#[derive(Clone)]
struct RawSample {
    os_id: u64,
    t: u64,
    pcs: Vec<u64>,
    w: u64,
}

static SAMPLES: Mutex<SeqLog<RawSample>> = Mutex::new(SeqLog::new(1 << 15));
static COUNTERS: Mutex<SeqLog<(u64, &'static str, i64)>> = Mutex::new(SeqLog::new(1 << 13));

// ------------------------------------------------------------------ what the program and the JIT report

/// An event: `cat` names its kind (`log`, `panic`, an app's own category like `game.spawn`),
/// `text` says what happened (cut at 168 bytes), `a` / `b` are free numbers (an entity id, a
/// value). Any thread, never blocks.
pub fn event(cat: &str, text: &str, a: u64, b: u64) {
    if !on() {
        return;
    }
    push_ev(events(), cat, text, a, b);
}

fn push_ev(ring: &Slots<Ev>, cat: &str, text: &str, a: u64, b: u64) {
    let (cat_b, cat_len) = copy_text::<28>(cat);
    let (text_b, len) = copy_text::<168>(text);
    ring.push(Ev { t: crate::clock::wall_ns(), os_id: super::threads::self_os_id(), a, b, cat_len, len, cat: cat_b, text: text_b });
}

/// An input event of the program as the remote command that injects it again
/// (`mouse k=down x=.. y=..`, `key k=down c=..`, `text t=..`), at its UI frame.
pub fn input(frame: u64, injected: bool, command: &str) {
    if !on() {
        return;
    }
    if command.len() > 78 {
        INPUT_CUT.fetch_add(1, Relaxed);
    }
    let (text, len) = copy_text::<78>(command);
    inputs().push(In { t: crate::clock::wall_ns(), frame, len: len as u8, flags: injected as u8, text });
}

/// input commands longer than a slot (cut: a replay of them is not exact)
static INPUT_CUT: AtomicU64 = AtomicU64::new(0);

/// Code was installed for slot `id` (jit index_code).
pub fn jit_install(id: u32, len: u32, tier: u8, origin: u8) {
    if !on() {
        return;
    }
    jits().push(Jit { t: crate::clock::wall_ns(), os_id: super::threads::self_os_id(), id, len, tier, origin });
}

/// A UI frame ended (timeline's frame watch): its span, the idle gaps in it, its lazy compiles
/// and its waits (of them on the owner).
#[allow(clippy::too_many_arguments)]
pub fn frame(t0: u64, t1: u64, idle: u64, compile_ns: u64, compiles: u32, wait_ns: u64, owner_ns: u64, waits: u32) {
    if !on() {
        return;
    }
    frames().push(Fr { t0, t1, idle, compile_ns, wait_ns, owner_ns, compiles, waits });
}

/// wall ns the UI thread entered its current root phase (0: between phases: idle in the event
/// loop, or between a frame's phases)
static UI_BUSY_SINCE: AtomicU64 = AtomicU64::new(0);

/// The UI thread entered a root phase (`t`) or left it (0) (timeline's frame watch).
#[inline]
pub fn ui_root(t: u64) {
    if crate::jit::current_thread_id() == crate::jit::MAIN_TID.load(Relaxed) {
        UI_BUSY_SINCE.store(t, Relaxed);
    }
}

/// A frame of `ms` ms ended (a hitch over the timeline's threshold): an event, and a recording
/// when it is over DUMP_MS.
pub fn hitch(ns: u64, typical_ns: u64) {
    if !on() {
        return;
    }
    let typical = if typical_ns > 0 && typical_ns < ns / 2 { format!(" (typical {:.1} ms)", typical_ns as f64 / 1e6) } else { String::new() };
    event("hitch", &format!("frame {:.0} ms{}", ns as f64 / 1e6, typical), ns, typical_ns);
    let th = DUMP_MS.load(Relaxed);
    if th > 0 && ns >= th * 1_000_000 {
        request_dump(Why::Hitch);
    }
}

/// Every inspect event (panic, hang, hitch, script error, ...) also goes to the recorder.
pub fn inspect_event(kind: &str, key: &str) {
    if kind == "hitch" {
        return; // recorded by `hitch` with its numbers
    }
    event(kind, key, 0, 0);
}

/// A panic was raised on this thread (parked or not); later the decision on a parked one.
pub fn panic(message: &str, location: &str) {
    if !on() {
        return;
    }
    let me = crate::os::thread::current();
    let thread = if me == crate::jit::MAIN_TID.load(Relaxed) { "main".to_string() } else { crate::os::thread::name(me) };
    event("panic", &format!("on {}: {} at {}", thread, message, location), 0, 0);
    request_dump(Why::Panic);
}

pub fn panic_decided(id: u64, how: &str) {
    event("panic.decided", &format!("P{} {}", id, how), id, 0);
}

/// A memory counter (resident bytes, ...): the recorder's own thread and the memory detector.
pub fn counter(name: &'static str, value: i64) {
    if !on() {
        return;
    }
    let _k = crate::zone_alloc::kept();
    COUNTERS.lock().unwrap_or_else(|e| e.into_inner()).push((crate::clock::wall_ns(), name, value));
}

// ------------------------------------------------------------------ the program's entries (loader symbols)

/// `relax_flight_mark(cat, cat_len, text, text_len, a, b)`: an app or game event (an entity
/// spawned, an AI state changed, a level loaded) for the recording.
pub extern "C" fn relax_flight_mark(cat: *const u8, cat_len: usize, text: *const u8, text_len: usize, a: u64, b: u64) {
    if !on() {
        return;
    }
    let s = |p: *const u8, n: usize| if p.is_null() || n == 0 { "" } else { unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(p, n)) } };
    let (cat, text) = (s(cat, cat_len), s(text, text_len));
    if host_cat(cat) {
        event(cat, text, a, b);
    } else {
        push_ev(app_events(), cat, text, a, b);
    }
}

/// `relax_flight_input(frame, injected, command, len)`: an input event the program dispatched,
/// as the remote command that injects it.
pub extern "C" fn relax_flight_input(frame: u64, injected: u64, cmd: *const u8, len: usize) {
    if !on() || cmd.is_null() {
        return;
    }
    input(frame, injected != 0, unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(cmd, len)) });
}

/// `relax_flight_on()`: the recorder records (programs skip formatting otherwise).
pub extern "C" fn relax_flight_on() -> bool {
    on()
}

// ------------------------------------------------------------------ the recorder's thread

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Why {
    None = 0,
    Hitch,
    Panic,
    Fault,
    Asked,
}

impl Why {
    fn name(self) -> &'static str {
        match self {
            Why::None => "",
            Why::Hitch => "hitch",
            Why::Panic => "panic",
            Why::Fault => "fault",
            Why::Asked => "asked",
        }
    }
}

static PENDING: AtomicU64 = AtomicU64::new(0);
static LAST_AUTO: AtomicU64 = AtomicU64::new(0);
static DUMPS: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn request_dump(why: Why) {
    PENDING.store(why as u64, Relaxed);
    if let Some(t) = THREAD.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        t.unpark();
    }
}

fn is_ui_or_owner(t: &super::threads::Thread) -> bool {
    t.handle != 0 && (t.handle == crate::jit::MAIN_TID.load(Relaxed) || t.handle == crate::owner::owner_tid())
}

fn watch() {
    // everything this thread keeps outlives compiler generations (a zone swap frees their heaps)
    let _k = crate::zone_alloc::kept();
    super::heap::exclude_this_thread();
    let mut last_heap = std::time::Instant::now();
    loop {
        std::thread::park_timeout(std::time::Duration::from_millis(50));
        if !on() {
            continue;
        }
        let why = PENDING.swap(0, Relaxed);
        if why != 0 {
            let why = match why {
                1 => Why::Hitch,
                2 => Why::Panic,
                3 => Why::Fault,
                _ => Why::Asked,
            };
            let now = crate::clock::wall_ns();
            let last = LAST_AUTO.load(Relaxed);
            let gap = if why == Why::Panic { DUMP_GAP_NS / 3 } else { DUMP_GAP_NS };
            if last == 0 || now.saturating_sub(last) >= gap {
                LAST_AUTO.store(now, Relaxed);
                match save(if why == Why::Hitch { HITCH_LAST_NS } else { DUMP_LAST_NS }, why, None, true) {
                    Ok((path, n)) => eprintln!("makepad-run flightrec: {} recorded: {} ({} KB; makepad inspect file={} pf brief)", why.name(), path.display(), n / 1024, path.display()),
                    Err(e) => eprintln!("makepad-run flightrec: cannot write the {} recording: {}", why.name(), e),
                }
            }
        }
        // the app's heap once a second (live and committed bytes)
        if last_heap.elapsed().as_secs() >= 1 {
            last_heap = std::time::Instant::now();
            if let Some((live, committed)) = crate::domain::app_heap_stats() {
                counter("app_heap", live as i64);
                counter("app_heap_committed", committed as i64);
            }
        }
        // a UI wait on the JIT or a frame that runs long: sample both sides until it ends
        let stalled = || {
            let f = UI_BUSY_SINCE.load(Relaxed);
            crate::postframe::ui_stall_ns() >= STALL_NS || (f != 0 && crate::clock::wall_ns().saturating_sub(f) >= FRAME_STALL_NS)
        };
        if stalled() {
            let t0 = std::time::Instant::now();
            while stalled() && t0.elapsed().as_secs() < 30 {
                let run = super::prof::collect(0.1, 200, &is_ui_or_owner, &|| !stalled());
                let mut log = SAMPLES.lock().unwrap_or_else(|e| e.into_inner());
                for s in run.samples {
                    log.push(RawSample { os_id: s.os_id, t: s.t, pcs: s.frames, w: s.weight });
                }
            }
        }
    }
}

// ------------------------------------------------------------------ recordings

/// Per-reader positions in the rings (a live viewer keeps one; a recording starts from 0).
pub(super) struct Cursors {
    ev: u64,
    app_ev: u64,
    input: u64,
    jit: u64,
    frames: u64,
    samples: u64,
    counters: u64,
    /// the last FRAME written: (frame number, its t0 on the stream clock)
    prev_frame: Option<(u64, u64)>,
    /// the run's start (args, cwd, environment) was sent
    run_sent: bool,
}

impl Cursors {
    /// From the rings' start (`backfill`), else from now on.
    pub fn new(backfill: bool) -> Cursors {
        if backfill {
            return Cursors { ev: 0, app_ev: 0, input: 0, jit: 0, frames: 0, samples: 0, counters: 0, prev_frame: None, run_sent: false };
        }
        Cursors {
            ev: events().fill().0,
            app_ev: app_events().fill().0,
            input: inputs().fill().0,
            jit: jits().fill().0,
            frames: frames().fill().0,
            samples: u64::MAX,
            counters: u64::MAX,
            prev_frame: None,
            run_sent: false,
        }
    }
}

/// The recorder's records from `cur` on (and at or after `min_t`, the rings' clock): FRAME,
/// JIT, EVENT, INPUT, SAMPLES, COUNTERS and DROP records appended to `body`.
pub(super) fn encode(cur: &mut Cursors, enc: &mut super::pstream::StreamEnc, min_t: u64, body: &mut Vec<u8>) {
    if !on() {
        return;
    }
    let mut out = Vec::new();
    let mut lost_total = 0;
    // frames
    let mut fr: Vec<(u64, Fr)> = Vec::new();
    let (c, lost) = frames().read(cur.frames, &mut |n, f| {
        if f.t0 >= min_t {
            fr.push((n, f));
        }
    });
    cur.frames = c;
    lost_total += lost;
    for (n, f) in fr {
        let t0 = enc.map(f.t0);
        let mut e = ps::Enc::default();
        let (pn, pt) = cur.prev_frame.unwrap_or((0, 0));
        // frame numbers count from the run's first frame (frame n is F<n+1>)
        e.u(n + 1 - pn.min(n + 1)).d(t0, pt).u(f.t1.saturating_sub(f.t0)).u(0).u(0).u(0);
        e.u(f.idle).u(f.compile_ns).u(f.compiles as u64).u(f.wait_ns).u(f.owner_ns).u(f.waits as u64);
        cur.prev_frame = Some((n + 1, t0));
        ps::record(&mut out, ps::kind::FRAME, &e.0);
    }
    // JIT installs (names from the code tables, read now)
    let mut jit = Vec::new();
    let (c, lost) = jits().read(cur.jit, &mut |_, j| {
        if j.t >= min_t {
            jit.push(j);
        }
    });
    cur.jit = c;
    lost_total += lost;
    let mut jrecs = Vec::with_capacity(jit.len());
    for j in jit {
        let tid = enc.known_tid(j.os_id).unwrap_or(0);
        let name = crate::jit::slot_name(j.id).unwrap_or_else(|| format!("slot {}", j.id));
        jrecs.push(ps::JitRec { t: enc.map(j.t), tid, ev: ps::jit::INSTALL, name: enc.strings.id(&name), tier: j.tier, origin: j.origin, a: j.len as u64, b: j.id as u64, src: 0 });
    }
    // events
    let mut evs: Vec<Ev> = Vec::new();
    let (c, lost) = events().read(cur.ev, &mut |_, e| {
        if e.t >= min_t {
            evs.push(e);
        }
    });
    cur.ev = c;
    lost_total += lost;
    let (c, lost) = app_events().read(cur.app_ev, &mut |_, e| {
        if e.t >= min_t {
            evs.push(e);
        }
    });
    cur.app_ev = c;
    lost_total += lost;
    // how the run started (whole strings, separated by U+001F): a replay starts it the same way
    let mut run_evs: Vec<(&'static str, String)> = Vec::new();
    if !cur.run_sent {
        cur.run_sent = true;
        if let Some((args, cwd, env)) = RUN.lock().unwrap_or_else(|e| e.into_inner()).clone() {
            run_evs = vec![("run.args", args), ("run.cwd", cwd), ("run.env", env), ("run.home", crate::gitstore::relax_home().display().to_string())];
        }
    }
    let mut ev_payload = None;
    if !evs.is_empty() || !run_evs.is_empty() {
        evs.sort_by_key(|e| e.t);
        let mut e = ps::Enc::default();
        let base = enc.map(evs.first().map_or(crate::clock::wall_ns(), |x| x.t));
        e.u((evs.len() + run_evs.len()) as u64).u(base);
        let mut prev = base;
        for (cat, text) in &run_evs {
            let cat = enc.strings.id(cat);
            e.d(base, prev).u(0).u(cat as u64).u(0).u(0).s(text);
            prev = base;
        }
        for x in &evs {
            let t = enc.map(x.t);
            let tid = enc.tid(x.os_id, 0);
            let cat = enc.strings.id(text_of(&x.cat, x.cat_len));
            e.d(t, prev).u(tid as u64).u(cat as u64).u(x.a).u(x.b).s(text_of(&x.text, x.len));
            prev = t;
        }
        ev_payload = Some(e.0);
    }
    // input
    let mut ins: Vec<In> = Vec::new();
    let (c, lost) = inputs().read(cur.input, &mut |_, i| {
        if i.t >= min_t {
            ins.push(i);
        }
    });
    cur.input = c;
    lost_total += lost;
    let mut in_payload = None;
    if !ins.is_empty() {
        let mut e = ps::Enc::default();
        let base = enc.map(ins[0].t);
        e.u(ins.len() as u64).u(base);
        let mut prev = base;
        for x in &ins {
            let t = enc.map(x.t);
            e.d(t, prev).u(x.frame).u(x.flags as u64).s(text_of(&x.text, x.len as u16));
            prev = t;
        }
        in_payload = Some(e.0);
    }
    // stall samples (a live viewer's start: none of the earlier ones)
    let raw: Vec<RawSample> = {
        let log = SAMPLES.lock().unwrap_or_else(|e| e.into_inner());
        if cur.samples == u64::MAX {
            cur.samples = log.base + log.v.len() as u64;
        }
        log.read(&mut cur.samples).into_iter().filter(|s| s.t >= min_t).collect()
    };
    let mut samples = Vec::new();
    if !raw.is_empty() {
        let run = super::prof::Run {
            samples: raw.into_iter().map(|s| super::prof::Sample { os_id: s.os_id, frames: s.pcs, weight: s.w, t: s.t }).collect(),
            threads: Vec::new(),
            secs: 0.0,
            hz: 200,
            lost: 0,
            notes: Vec::new(),
            ev: super::prof::Ev::Cpu,
        };
        let mut partial = Vec::new();
        let mut per: Vec<(u32, Vec<ps::SampleRec>)> = Vec::new();
        for (os_id, t, fr, w) in super::calltree::expanded_raw(&run, &mut partial) {
            let tid = enc.tid(os_id, 0);
            let rec = ps::SampleRec { t: enc.map(t), weight: w * 5_000_000, frames: fr.iter().map(|f| (enc.strings.id(&f.name), super::pstream::tier_code(&f.tier))).collect() };
            match per.iter_mut().find(|x| x.0 == tid) {
                Some(x) => x.1.push(rec),
                None => per.push((tid, vec![rec])),
            }
        }
        for (tid, mut v) in per {
            v.sort_by_key(|r| r.t);
            samples.push(ps::samples_payload(tid, 0, &v));
        }
    }
    // memory counters
    let cs: Vec<(u64, &'static str, i64)> = {
        let log = COUNTERS.lock().unwrap_or_else(|e| e.into_inner());
        if cur.counters == u64::MAX {
            cur.counters = log.base + log.v.len() as u64;
        }
        let mut c = cur.counters;
        let v = log.read(&mut c).into_iter().filter(|x| x.0 >= min_t).collect();
        cur.counters = c;
        v
    };
    let crecs: Vec<ps::CounterRec> = cs.iter().map(|(t, n, v)| ps::CounterRec { t: enc.map(*t), tid: 0, counter: enc.strings.id(n), value: *v }).collect();
    // tables first, then the records that name them
    enc.flush_tables(body);
    body.extend_from_slice(&out);
    if !jrecs.is_empty() {
        ps::record(body, ps::kind::JIT, &ps::jit_payload(&jrecs));
    }
    if let Some(p) = ev_payload {
        ps::record(body, ps::kind::EVENT, &p);
    }
    if let Some(p) = in_payload {
        ps::record(body, ps::kind::INPUT, &p);
    }
    for p in &samples {
        ps::record(body, ps::kind::SAMPLES, p);
    }
    if !crecs.is_empty() {
        ps::record(body, ps::kind::COUNTERS, &ps::counters_payload(&crecs));
    }
    if lost_total > 0 && min_t == 0 && cur.prev_frame.is_some() {
        ps::record(body, ps::kind::DROP, &ps::drop_payload(0, lost_total));
    }
}

/// The recording of the last `last_ns` (0: everything the rings hold) as `.mkpf` bytes.
/// `name_samples`: name the stall samples' JIT frames (asks the owner; not in a fault).
pub fn snapshot(last_ns: u64, name_samples: bool) -> Vec<u8> {
    let _k = crate::zone_alloc::kept();
    let mut body = ps::prelude().to_vec();
    super::pstream::hello(&mut body);
    let now = crate::clock::wall_ns();
    let min_t = if last_ns == 0 { 0 } else { now.saturating_sub(last_ns) };
    let mut enc = super::pstream::StreamEnc::new();
    let mut cursors: Vec<_> = super::pstream::rings().into_iter().map(|r| (r, 0u64)).collect();
    enc.rings(&mut cursors, min_t.max(1), &mut body);
    let mut cur = Cursors::new(true);
    if !name_samples {
        cur.samples = u64::MAX - 1; // past every sample
    }
    encode(&mut cur, &mut enc, min_t.max(1), &mut body);
    body
}

/// The folder recordings go to: <makepad run home>/flightrec.
pub fn folder() -> std::path::PathBuf {
    crate::gitstore::relax_home().join("flightrec")
}

/// Writes a recording of the last `last_ns` to `path` (default: the folder, named by app, pid,
/// time and why). Returns the path and its size.
pub fn save(last_ns: u64, why: Why, path: Option<&str>, name_samples: bool) -> Result<(std::path::PathBuf, usize), String> {
    let bytes = snapshot(last_ns, name_samples);
    let auto = why != Why::Asked;
    let path = match path {
        Some(p) => std::path::PathBuf::from(p),
        None => {
            let dir = folder();
            std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {}", dir.display(), e))?;
            let app = super::APP.lock().unwrap_or_else(|e| e.into_inner()).clone();
            let app: String = app.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).collect();
            let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
            let name = format!("{}{}-{}-{}-{}.mkpf", if auto { "auto-" } else { "" }, app, std::process::id(), stamp(secs), if auto { why.name() } else { "saved" });
            if auto {
                prune(&dir);
            }
            dir.join(name)
        }
    };
    std::fs::write(&path, &bytes).map_err(|e| format!("cannot write {}: {}", path.display(), e))?;
    let _k = crate::zone_alloc::kept();
    DUMPS.lock().unwrap_or_else(|e| e.into_inner()).push(path.display().to_string());
    Ok((path, bytes.len()))
}

/// Keeps the newest AUTO_KEEP - 1 automatic recordings of the folder (before a new one).
fn prune(dir: &std::path::Path) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut autos: Vec<(std::time::SystemTime, std::path::PathBuf)> = rd
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("auto-") && e.file_name().to_string_lossy().ends_with(".mkpf"))
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    if autos.len() < AUTO_KEEP {
        return;
    }
    autos.sort();
    let extra = autos.len() + 1 - AUTO_KEEP;
    for (_, p) in autos.into_iter().take(extra) {
        let _ = std::fs::remove_file(p);
    }
}

/// `YYYYMMDD-HHMMSS` (UTC) of unix seconds.
fn stamp(secs: u64) -> String {
    let days = (secs / 86400) as i64;
    let rem = secs % 86400;
    // civil from days (proleptic Gregorian)
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    format!("{:04}{:02}{:02}-{:02}{:02}{:02}", y, m, d, rem / 3600, rem % 3600 / 60, rem % 60)
}

/// A fatal fault is ending the process: the recording is written now, on the faulting thread
/// (no stall-sample names: they would ask the owner).
pub fn fault_dump(what: &str) {
    static ONCE: AtomicBool = AtomicBool::new(false);
    if !on() || ONCE.swap(true, Relaxed) {
        return;
    }
    event("fault", what, 0, 0);
    match save(DUMP_LAST_NS, Why::Fault, None, false) {
        Ok((path, n)) => eprintln!("makepad-run flightrec: fault recorded: {} ({} KB; makepad inspect file={} pf brief)", path.display(), n / 1024, path.display()),
        Err(e) => eprintln!("makepad-run flightrec: cannot write the fault recording: {}", e),
    }
}

// ------------------------------------------------------------------ `rec` (inspect command)

/// `last=60s|5m|500ms|all` -> ns (0: all).
fn parse_last(v: &str) -> Result<u64, String> {
    if v == "all" {
        return Ok(0);
    }
    let (num, mul) = if let Some(n) = v.strip_suffix("ms") {
        (n, 1e6)
    } else if let Some(n) = v.strip_suffix('s') {
        (n, 1e9)
    } else if let Some(n) = v.strip_suffix('m') {
        (n, 60e9)
    } else {
        (v, 1e9)
    };
    num.parse::<f64>().map(|x| (x * mul) as u64).map_err(|_| format!("last={v}: a time like 60s, 5m, 500ms or all"))
}

fn mb(b: usize) -> String {
    format!("{:.1}", b as f64 / (1 << 20) as f64)
}

/// `rec [status] | save [last=60s] [path=<file>] | mark <text> | on | off`.
pub fn cmd(req: &super::Req, out: &mut super::Out) {
    match req.pos.first().map(String::as_str).unwrap_or("status") {
        "status" => status(out),
        "save" => {
            if !on() {
                return out.err("the flight recorder is off (MAKEPAD_RUN_FLIGHTREC=0, or `rec on`)");
            }
            let last = match req.get("last").map(parse_last).unwrap_or(Ok(DUMP_LAST_NS)) {
                Ok(v) => v,
                Err(e) => return out.err(&e),
            };
            match save(last, Why::Asked, req.get("path"), true) {
                Ok((path, n)) => {
                    let p = path.display().to_string();
                    let span = if last == 0 { "everything the recorder holds".to_string() } else { format!("the last {:.0} s", last as f64 / 1e9) };
                    out.line(&format!("recorded {}: {} ({} MB)", span, p, mb(n)));
                    out.line(&format!("more: makepad inspect file={p} pf brief  makepad inspect file={p} pf hitches  makepad inspect file={p} pf events  makepad inspect file={p} rec replay"));
                }
                Err(e) => out.err(&e),
            }
        }
        "mark" => {
            let text = req.pos[1..].join(" ");
            event("mark", &text, 0, 0);
            out.line(&format!("marked: {text}"));
        }
        "on" => {
            if !on() {
                ON.store(true, Release);
                if THREAD.lock().unwrap_or_else(|e| e.into_inner()).is_none() {
                    init();
                }
                super::timeline::flag_update();
            }
            status(out);
        }
        "off" => {
            ON.store(false, Release);
            super::timeline::flag_update();
            out.line("the flight recorder is off (its rings keep what they hold; `rec on` resumes)");
        }
        "replay" => out.err("rec replay is run by the client: makepad inspect file=<x.mkpf> rec replay [until=<s>]"),
        w => out.err(&format!("rec {w}? rec [status] | save [last=60s|5m|all] [path=<file>] | mark <text> | on | off")),
    }
}

fn status(out: &mut super::Out) {
    let (ring_bytes, rings) = super::pstream::ring_stats();
    let fixed = events().bytes() + app_events().bytes() + inputs().bytes() + jits().bytes() + frames().bytes();
    let now = crate::clock::wall_ns();
    // how far back the UI thread's and the owner's lanes reach
    let mut reach: Vec<(String, f64, u64)> = Vec::new();
    for r in super::pstream::rings() {
        let mut oldest = u64::MAX;
        let mut n = 0u64;
        let mut who = (0u64, 0u64);
        super::pstream::read_ring(r, 0, &mut |os, h, x| {
            oldest = oldest.min(x.t());
            n += 1;
            who = (os, h);
        });
        if n == 0 {
            continue;
        }
        let name = if who.1 != 0 && who.1 == crate::jit::MAIN_TID.load(Relaxed) {
            "main".to_string()
        } else if who.1 != 0 && who.1 == crate::owner::owner_tid() {
            "owner".to_string()
        } else {
            continue;
        };
        reach.push((name, now.saturating_sub(oldest) as f64 / 1e9, n));
    }
    let fill = |f: (u64, u64)| format!("{}/{}", f.0.min(f.1), f.1);
    out.line(&format!(
        "flight recorder {}: {} MB in {} thread rings + {} MB fixed; {} automatic recording(s) this run",
        if on() { "on" } else { "off" },
        mb(ring_bytes),
        rings,
        mb(fixed),
        DUMPS.lock().unwrap_or_else(|e| e.into_inner()).len()
    ));
    out.line("| ring | reaches back s | records |");
    out.line("|---|---|---|");
    for (n, s, c) in &reach {
        out.line(&format!("| {} | {:.0} | {} |", n, s, c));
    }
    // how far back a fixed ring reaches: its oldest entry kept
    fn oldest<T: Copy>(s: &Slots<T>, t: impl Fn(&T) -> u64) -> u64 {
        let mut o = u64::MAX;
        let (head, len) = s.fill();
        let _ = s.read(head.saturating_sub(len), &mut |_, x| o = o.min(t(&x)));
        o
    }
    let back = |o: u64| if o == u64::MAX { String::new() } else { format!("{:.0}", now.saturating_sub(o) as f64 / 1e9) };
    out.line(&format!("| events | {} | {} |", back(oldest(events(), |e| e.t)), fill(events().fill())));
    out.line(&format!("| app marks | {} | {} |", back(oldest(app_events(), |e| e.t)), fill(app_events().fill())));
    out.line(&format!("| input | {} | {} |", back(oldest(inputs(), |e| e.t)), fill(inputs().fill())));
    out.line(&format!("| JIT installs | {} | {} |", back(oldest(jits(), |e| e.t)), fill(jits().fill())));
    out.line(&format!("| frames | {} | {} |", back(oldest(frames(), |e| e.t0)), fill(frames().fill())));
    let elided: u64 = super::pstream::rings().iter().map(|r| r.elided.load(Relaxed)).sum();
    if elided > 0 {
        out.line(&format!("note: {} spans under {} µs with nothing inside were not kept (MAKEPAD_RUN_FLIGHTREC_ELIDE_US)", elided, super::pstream::elide_us()));
    }
    let no_ring = super::pstream::NO_RING.load(Relaxed);
    if no_ring > 0 {
        out.line(&format!("note: {} threads found no ring (the {} MB cap)", no_ring, super::pstream::RINGS_MAX_BYTES >> 20));
    }
    out.line(&format!("note: automatic recordings on a hitch over {} ms, a panic and a fatal fault go to {}", DUMP_MS.load(Relaxed), folder().display()));
    for d in DUMPS.lock().unwrap_or_else(|e| e.into_inner()).iter().rev().take(5) {
        out.line(&format!("recorded: {d}"));
    }
    out.line("more: rec save last=60s  rec save last=all");
}
