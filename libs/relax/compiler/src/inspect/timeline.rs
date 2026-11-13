//! The dataflow timeline (DESIGN.md §8): what work ran where, what each piece waited for and who
//! released it, recorded only while a client asks (`timeline start` .. `timeline stop`).
//!
//! Records go to per-thread rings (no lock, no allocation after a thread's first record):
//! - spans: the UI thread's phases (makepad's ui_hang phases: each event's dispatch, draw lists,
//!   widget draw, present, GPU waits, ...), the owner's service of each request;
//! - waits: every wait the registry sees (owner requests, query latches, program mutex / condvar /
//!   park / once / join), with what it waited on;
//! - wakes: the program's futex wakes (mutex unlock with waiters, condvar notify, unpark) and
//!   the owner finishing a request -- the edges that end waits.
//!
//! Analysis per frame (a frame = the UI thread's work that led up to a Draw, `frames_of`; a wake
//! that drew nothing is an event-only beat, `beats_of`): busy and waiting time per thread, the
//! critical path (from the frame's end back through each wait to the thread that ended it), total
//! work vs critical path vs wall, and the waits on the path ranked. `timeline save` writes the capture as a `.mkpf`
//! profile recording (makepad-prof-stream), the format an app and `pf` queries read.
//!
//! The hitch watcher runs a frame from the first phase after the previous frame to the UI
//! thread's next event-loop wait (idle or GPU) after a Draw. Idle waits for input that drew
//! nothing or for the next vsync do not count toward its frame time.

use super::{json_str, now_ms, short, Out, Req};
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering::{Acquire, Relaxed, Release}};
use relax_session::sync::Mutex;

/// The markers' flag (one relaxed load at each marker site; makepad and Relax std read it
/// through the loader symbols `relax_recording` / `__relax_recording`): on while the timeline
/// records or a watcher wants frame spikes.
#[no_mangle]
pub static RELAX_RECORDING: AtomicBool = AtomicBool::new(false);
/// the timeline's rings record
static RINGS_ON: AtomicBool = AtomicBool::new(false);
/// frame spikes are watched (`watch`, `events follow=1`, `--agent`)
static FRAME_WATCH: AtomicBool = AtomicBool::new(false);

/// UI frames finished (the UI thread next waited in the event loop after a Draw) while frame
/// markers are on (hitch detection, on by default): the per-frame denominator of `heap frame`
/// and other rates.
pub static FRAMES: AtomicU64 = AtomicU64::new(0);

/// frames slower than this are `hitch` events (ns; 0: off): always on, RELAX_HITCH_MS
/// (default 50) or `hitch ms=<n>|off`
static HITCH_NS: AtomicU64 = AtomicU64::new(0);

pub(super) fn flag_update() {
    let rec = RINGS_ON.load(Relaxed) || super::pstream::on();
    RELAX_RECORDING.store(rec || FRAME_WATCH.load(Relaxed) || HITCH_NS.load(Relaxed) > 0, Release);
    // the wait hook: the timeline's rings, the prof stream and the UI thread's frame log want waits
    let f: fn(bool, &'static str, &'static str, u64) = wait_trace;
    crate::waits::TRACE.store(if rec || HITCH_NS.load(Relaxed) > 0 { f as usize } else { 0 }, Release);
}

/// Hitch detection on at inspect's start (RELAX_HITCH_MS, default 50; 0: off).
pub fn hitch_init() {
    let ms = std::env::var("RELAX_HITCH_MS").ok().and_then(|v| v.parse::<u64>().ok()).unwrap_or(50);
    HITCH_NS.store(ms * 1_000_000, Relaxed);
    flag_update();
}

/// Frame-time spike detection on (a watcher attached) or off.
pub fn set_frame_watch(on: bool) {
    FRAME_WATCH.store(on, Relaxed);
    flag_update();
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Kind {
    SpanBegin = 1,
    SpanEnd,
    WaitBegin,
    WaitEnd,
    /// a futex wake: `a` = the futex address
    Wake,
    /// the owner finished a request: `a` = the requester's handle
    Served,
    /// the UI thread's event loop waits for events (makepad's `ui_idle`) / is woken: the turns of
    /// the loop, in the rings only (`beats_of`)
    IdleBegin,
    IdleEnd,
}

#[derive(Clone, Copy)]
struct Rec {
    t: u64,
    kind: u8,
    label_len: u16,
    label: u64,
    a: u64,
    b: u64,
}

const RING: usize = 1 << 16;

struct Ring {
    recs: Box<[std::cell::UnsafeCell<Rec>]>,
    next: AtomicUsize,
    os_id: u64,
    handle: u64,
    /// the capture this ring's records belong to (rings are reused across captures)
    epoch: AtomicU64,
    /// its thread has ended: a run's end frees it (`reset_statics`)
    ended: AtomicBool,
}

// a ring is written by its own thread only and read after the capture stopped
unsafe impl Sync for Ring {}

static RINGS: Mutex<Vec<&'static Ring>> = Mutex::new(Vec::new());
static EPOCH: AtomicU64 = AtomicU64::new(0);
static STARTED_NS: AtomicU64 = AtomicU64::new(0);
static STOPPED_NS: AtomicU64 = AtomicU64::new(0);

struct RingEnds(&'static Ring);

impl Drop for RingEnds {
    fn drop(&mut self) {
        self.0.ended.store(true, Relaxed);
    }
}

thread_local! {
    static MY: Cell<*const Ring> = const { Cell::new(std::ptr::null()) };
    static ENDS: Cell<Option<RingEnds>> = const { Cell::new(None) };
}

fn ring() -> Option<&'static Ring> {
    let p = MY.try_with(|m| m.get()).ok()?;
    if !p.is_null() {
        return Some(unsafe { &*p });
    }
    // a ring is process state (the owner records on its own thread too)
    let _k = crate::zone_alloc::kept();
    let r: &'static Ring = Box::leak(Box::new(Ring {
        recs: (0..RING).map(|_| std::cell::UnsafeCell::new(Rec { t: 0, kind: 0, label_len: 0, label: 0, a: 0, b: 0 })).collect(),
        next: AtomicUsize::new(0),
        os_id: super::threads::self_os_id(),
        handle: crate::waits::me(),
        epoch: AtomicU64::new(EPOCH.load(Relaxed)),
        ended: AtomicBool::new(false),
    }));
    RINGS.lock().unwrap_or_else(|e| e.into_inner()).push(r);
    let _ = MY.try_with(|m| m.set(r));
    let _ = ENDS.try_with(|e| e.set(Some(RingEnds(r))));
    Some(r)
}

/// The markers record: a timeline capture runs or a prof viewer is attached.
#[inline]
pub fn recording() -> bool {
    RINGS_ON.load(Relaxed) || super::pstream::on()
}

/// Appends a record to this thread's ring and, while a viewer is attached, its prof stream ring
/// (callers check `recording()` first).
pub fn push(kind: Kind, label: &'static str, a: u64, b: u64) {
    if super::pstream::on() {
        super::pstream::push(kind as u8, label, a, b);
    }
    ring_push(kind, label, a, b);
}

/// Appends a record to this thread's ring only (the idle markers, which the prof stream has no
/// kind for).
fn ring_push(kind: Kind, label: &'static str, a: u64, b: u64) {
    if !RINGS_ON.load(Relaxed) {
        return;
    }
    let Some(r) = ring() else { return };
    let e = EPOCH.load(Relaxed);
    if r.epoch.load(Relaxed) != e {
        r.epoch.store(e, Relaxed);
        r.next.store(0, Relaxed);
    }
    let i = r.next.fetch_add(1, Relaxed);
    if i >= RING {
        return; // full: the capture keeps its first RING records per thread
    }
    unsafe {
        *r.recs[i].get() = Rec { t: crate::clock::wall_ns(), kind: kind as u8, label_len: label.len().min(u16::MAX as usize) as u16, label: label.as_ptr() as u64, a, b };
    }
}

/// A span of this thread's timeline until the guard drops (nothing when nothing records).
pub struct Span(&'static str, bool);

pub fn span(name: &'static str) -> Span {
    let on = recording();
    if on {
        push(Kind::SpanBegin, name, 0, 0);
    }
    Span(name, on)
}

impl Drop for Span {
    fn drop(&mut self) {
        if self.1 {
            push(Kind::SpanEnd, self.0, 0, 0);
        }
    }
}

/// A wait of this thread's timeline on `object` until the guard drops, ended by the latest
/// `relax_wake(object)` (a wait the wait registry does not hold: the owner idle).
pub struct Wait(bool);

pub fn wait(what: &'static str, object: u64) -> Wait {
    let on = recording();
    if on {
        push(Kind::WaitBegin, what, object, 0);
    }
    Wait(on)
}

impl Drop for Wait {
    fn drop(&mut self) {
        if self.0 {
            push(Kind::WaitEnd, "", 0, 0);
        }
    }
}

// ------------------------------------------------------------------ marker entries

/// makepad's UI phases (platform thread/ui_hang.rs, found by dlsym): a phase begins / ends;
/// `name` is a static string of the program.
pub extern "C" fn relax_span_begin(name: *const u8, len: usize) {
    let name = unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(name, len)) };
    if name == "idle" {
        ring_push(Kind::IdleBegin, "idle", 0, 0);
        if FRAME_WATCH.load(Relaxed) || HITCH_NS.load(Relaxed) > 0 {
            frame_idle(true);
        }
        return;
    }
    if recording() {
        push(Kind::SpanBegin, name, 0, 0);
    }
    if FRAME_WATCH.load(Relaxed) || HITCH_NS.load(Relaxed) > 0 {
        frame_watch(true, name);
    }
}

pub extern "C" fn relax_span_end(name: *const u8, len: usize) {
    let name = unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(name, len)) };
    if name == "idle" {
        ring_push(Kind::IdleEnd, "idle", 0, 0);
        if FRAME_WATCH.load(Relaxed) || HITCH_NS.load(Relaxed) > 0 {
            frame_idle(false);
        }
        return;
    }
    if recording() {
        push(Kind::SpanEnd, name, 0, 0);
    }
    if FRAME_WATCH.load(Relaxed) || HITCH_NS.load(Relaxed) > 0 {
        frame_watch(false, name);
    }
}

// ---- frames: spikes and hitches (always on; per UI thread, no lock)
//
// A frame runs from the first phase after the previous frame to the UI thread's next wait in
// the event loop (idle or for the GPU) after a Draw. Its idle waits for input that drew nothing
// or for the next vsync do not count. Each frame's phases, the UI thread's waits and its lazy
// compiles go to a fixed log (cleared per frame): a frame over the hitch threshold is analysed
// from it after it ends and raised as a `hitch` event (at most HITCH_PER_S a second; the rest
// are counted).

const FRAME_RECS: usize = 8192;
const HITCH_PER_S: u64 = 4;

#[derive(Clone, Copy)]
struct FRec {
    t: u64,
    /// 1 phase begin, 2 phase end, 3 wait begin, 4 wait end, 5 compile begin, 6 compile end,
    /// 7 idle begin, 8 idle end.
    kind: u8,
    name: &'static str,
    detail: &'static str,
}

struct FrameLog {
    depth: u32,
    start: u64,
    /// running average of frame time (ns)
    avg: u64,
    idle: u64,
    idle_t0: u64,
    in_idle: bool,
    drew: bool,
    recs: Vec<FRec>,
    dropped: u32,
    compile_depth: u32,
    compile_t0: u64,
    compile_ns: u64,
    compiles: u32,
    /// probe / count values at the frame's start (only while probes are installed)
    probes0: Vec<(String, u64, u64)>,
    /// the frame's waits (outermost): open depth, the open one's start and whether it is on the
    /// owner, their time (of it on the owner) and count (the flight recorder's frame summary)
    wait_depth: u32,
    wait_t0: u64,
    wait_owner: bool,
    wait_ns: u64,
    owner_ns: u64,
    waits: u32,
    /// the open wait began inside a lazy compile; the frame's waits inside lazy compiles (of them
    /// on the owner): the summary's compile and wait times overlap by this much
    wait_in_compile: bool,
    cwait_ns: u64,
    cowner_ns: u64,
    /// the turns of the event loop: what woke this one, and the wakes a Draw may still take in
    turns: Turns,
}

/// The UI thread's turns of its event loop (from an idle wait's end to the next) as the frame
/// watcher sees them go by, framed by `frames_of`'s rule: a turn that drew nothing is a wake of
/// its own unless a Draw follows it with no gap over `FRAME_GAP_NS` between the root phases (the
/// frame then starts at it). The timeline's `beats_of` and `why-slow` count the same wakes.
#[derive(Default)]
struct Turns {
    /// this turn's first root phase: what woke the loop ("" before one)
    woke: &'static str,
    drew: bool,
    /// turns since the last frame that drew nothing and that a Draw may still take in
    chain: u64,
    root_end: u64,
}

#[derive(Debug, PartialEq)]
enum TurnEnd {
    /// no phase ran
    Idle,
    /// drew nothing: a wake, unless a Draw takes it in later
    Quiet(&'static str),
    /// drew, and took in this many of the wakes before it
    Drew(u64),
}

impl Turns {
    fn root_begin(&mut self, now: u64, name: &'static str) {
        if now.saturating_sub(self.root_end) > makepad_prof_stream::model::FRAME_GAP_NS {
            // a gap: no Draw takes in the wakes before it
            self.chain = 0;
        }
        if self.woke.is_empty() {
            self.woke = name;
        }
    }

    fn root_end(&mut self, now: u64) {
        self.root_end = now;
    }

    fn draw(&mut self) {
        self.drew = true;
    }

    fn end(&mut self) -> TurnEnd {
        let woke = std::mem::take(&mut self.woke);
        let end = if woke.is_empty() {
            TurnEnd::Idle
        } else if self.drew {
            TurnEnd::Drew(std::mem::take(&mut self.chain))
        } else {
            self.chain += 1;
            TurnEnd::Quiet(woke)
        };
        self.drew = false;
        end
    }
}

thread_local! {
    static FL: std::cell::RefCell<Option<Box<FrameLog>>> = const { std::cell::RefCell::new(None) };
}

static HITCHES: AtomicU64 = AtomicU64::new(0);

/// Turns of the UI thread's event loop that drew nothing (woken by a timer, a task, a signal and
/// no Draw ran) while frame markers are on (`Turns`), of them the ones a Draw took in after
/// (`QUIET_TAKEN`), and what woke the last `QUIET_RING` (each turn's first phase, a static str
/// packed as `detail` reads it; 0 once taken in): `why-slow`'s count of wakes beside the frames
/// that drew (`FRAMES`). A frame may take in wakes from before a reader's mark (`QUIET_CUT`, the
/// latest mark): `QUIET_TAKEN_CUT` counts those, so a window counts its own wakes only.
pub static QUIET: AtomicU64 = AtomicU64::new(0);
pub static QUIET_TAKEN: AtomicU64 = AtomicU64::new(0);
pub static QUIET_CUT: AtomicU64 = AtomicU64::new(0);
pub static QUIET_TAKEN_CUT: AtomicU64 = AtomicU64::new(0);
const QUIET_RING: usize = 64;
static QUIET_WOKE: [AtomicU64; QUIET_RING] = [const { AtomicU64::new(0) }; QUIET_RING];

fn quiet_wake(woke: &'static str) {
    let n = QUIET.load(Relaxed) as usize;
    QUIET_WOKE[n % QUIET_RING].store((woke.as_ptr() as u64 & ((1 << 47) - 1)) | ((woke.len().min(u16::MAX as usize) as u64) << 48), Release);
    QUIET.store(n as u64 + 1, Release);
}

/// A frame took in the last `n` wakes (the UI thread's, the one that counts them).
fn quiet_taken(n: u64) {
    if n == 0 {
        return;
    }
    let q = QUIET.load(Relaxed);
    let from = q.saturating_sub(n);
    for i in q.saturating_sub(n.min(QUIET_RING as u64))..q {
        QUIET_WOKE[i as usize % QUIET_RING].store(0, Release);
    }
    let cut = QUIET_CUT.load(Acquire);
    if from < cut && cut <= q {
        QUIET_TAKEN_CUT.fetch_add(cut - from, Release);
    }
    QUIET_TAKEN.fetch_add(n, Release);
}

/// A mark for `ui_wakes_since`: frames that drew, wakes seen, taken in, taken in across a mark.
pub fn ui_wakes_mark() -> [u64; 4] {
    let seen = QUIET.load(Acquire);
    QUIET_CUT.store(seen, Release);
    [FRAMES.load(Relaxed), seen, QUIET_TAKEN.load(Acquire), QUIET_TAKEN_CUT.load(Acquire)]
}

/// The frames that drew and the wakes that drew nothing since `mark`, with what woke the wakes
/// (of the last `QUIET_RING` seen): None while no frame markers run. The wakes a Draw may still
/// take in count, as the timeline counts them at its stop.
pub fn ui_wakes_since(mark: [u64; 4]) -> Option<(u64, u64, Vec<(&'static str, usize)>)> {
    if !FRAME_WATCH.load(Relaxed) && HITCH_NS.load(Relaxed) == 0 {
        return None;
    }
    let [frames0, seen0, taken0, cut0] = mark;
    let (frames1, seen1) = (FRAMES.load(Relaxed), QUIET.load(Acquire));
    let (taken1, cut1) = (QUIET_TAKEN.load(Acquire), QUIET_TAKEN_CUT.load(Acquire));
    // the wakes seen since the mark that no frame took in
    let taken = taken1.saturating_sub(taken0).saturating_sub(cut1.saturating_sub(cut0));
    let first = seen0.max(seen1.saturating_sub(QUIET_RING as u64));
    let woke = tally((first..seen1).map(|i| QUIET_WOKE[i as usize % QUIET_RING].load(Acquire)).filter(|&w| w != 0).map(detail));
    Some((frames1.saturating_sub(frames0), seen1.saturating_sub(seen0).saturating_sub(taken), woke))
}
static HITCH_SKIPPED: AtomicU64 = AtomicU64::new(0);
static HITCH_LAST_NS: AtomicU64 = AtomicU64::new(0);
static HITCH_WINDOW: AtomicU64 = AtomicU64::new(0);

fn frame_rec(l: &mut FrameLog, kind: u8, name: &'static str, detail: &'static str) {
    if l.recs.len() < FRAME_RECS {
        l.recs.push(FRec { t: crate::clock::wall_ns(), kind, name, detail });
    } else {
        l.dropped += 1;
    }
}

/// A phase begins / ends on this thread (makepad's ui_hang markers).
fn frame_watch(begin: bool, name: &'static str) {
    let _k = crate::zone_alloc::kept();
    let now = crate::clock::wall_ns();
    if begin && name == "gpu-wait" {
        frame_finish(now);
    }
    let _ = FL.try_with(|c| {
        let Ok(mut g) = c.try_borrow_mut() else { return };
        let l = g.get_or_insert_with(|| {
            Box::new(FrameLog { depth: 0, start: 0, avg: 0, idle: 0, idle_t0: 0, in_idle: false, drew: false, recs: Vec::with_capacity(FRAME_RECS), dropped: 0, compile_depth: 0, compile_t0: 0, compile_ns: 0, compiles: 0, probes0: Vec::new(), wait_depth: 0, wait_t0: 0, wait_owner: false, wait_ns: 0, owner_ns: 0, waits: 0, wait_in_compile: false, cwait_ns: 0, cowner_ns: 0, turns: Turns::default() })
        });
        if begin {
            if l.start == 0 {
                l.start = now;
                l.idle = 0;
                l.drew = false;
                l.recs.clear();
                l.dropped = 0;
                l.compile_ns = 0;
                l.compiles = 0;
                l.probes0.clear();
                l.wait_ns = 0;
                l.owner_ns = 0;
                l.waits = 0;
                l.cwait_ns = 0;
                l.cowner_ns = 0;
                if l.wait_depth > 0 {
                    l.wait_t0 = now;
                }
                if HITCH_NS.load(Relaxed) > 0 && super::instrument::any_counters() {
                    l.probes0 = super::instrument::counters();
                }
            }
            if l.depth == 0 {
                super::flightrec::ui_root(now);
                l.turns.root_begin(now, name);
            }
            l.depth += 1;
            frame_rec(l, 1, name, "");
            return;
        }
        l.depth = l.depth.saturating_sub(1);
        if l.start != 0 {
            frame_rec(l, 2, name, "");
        }
        if l.depth == 0 {
            super::flightrec::ui_root(0);
            l.turns.root_end(now);
        }
        if name == "Draw" {
            l.turns.draw();
            if l.start != 0 {
                l.drew = true;
            }
        }
    });
}

/// Idle waits are explicit so short input and vsync waits never inflate frame time.
fn frame_idle(begin: bool) {
    let _k = crate::zone_alloc::kept();
    let now = crate::clock::wall_ns();
    if begin {
        frame_finish(now);
    }
    let _ = FL.try_with(|c| {
        let Ok(mut g) = c.try_borrow_mut() else { return };
        let Some(l) = g.as_mut() else { return };
        if begin {
            if l.depth != 0 {
                return;
            }
            // a turn ends: one that ran and drew nothing is a wake of its own until a Draw takes it in
            match l.turns.end() {
                TurnEnd::Quiet(woke) => quiet_wake(woke),
                TurnEnd::Drew(n) => quiet_taken(n),
                TurnEnd::Idle => {}
            }
            if l.start != 0 {
                frame_rec(l, 7, "idle", "");
            }
            l.in_idle = true;
            l.idle_t0 = now;
        } else if l.in_idle {
            l.in_idle = false;
            if l.start != 0 {
                l.idle += now - l.idle_t0.max(l.start);
                frame_rec(l, 8, "idle", "");
            }
        }
    });
}

/// Finish at a root event-loop wait so the work after Draw stays in the frame.
fn frame_finish(now: u64) {
    let done = FL.try_with(|c| {
        let Ok(mut g) = c.try_borrow_mut() else { return None };
        let Some(l) = g.as_mut() else { return None };
        if l.depth != 0 || l.start == 0 || !l.drew {
            return None;
        }
        FRAMES.fetch_add(1, Relaxed);
        let ms = now.saturating_sub(l.start).saturating_sub(l.idle);
        // a wait still open at the frame's end counts up to it
        let (open_w, open_o) = if l.wait_depth > 0 { let w = now.saturating_sub(l.wait_t0.max(l.start)); (w, if l.wait_owner { w } else { 0 }) } else { (0, 0) };
        let (open_cw, open_co) = if l.wait_in_compile { (open_w, open_o) } else { (0, 0) };
        super::flightrec::frame(super::flightrec::Fr {
            t0: l.start,
            t1: now,
            idle: l.idle,
            compile_ns: l.compile_ns,
            compiles: l.compiles,
            wait_ns: l.wait_ns + open_w,
            owner_ns: l.owner_ns + open_o,
            waits: l.waits,
            cwait_ns: l.cwait_ns + open_cw,
            cowner_ns: l.cowner_ns + open_co,
        });
        let avg = l.avg;
        l.avg = if avg == 0 { ms } else { (avg * 15 + ms) / 16 };
        let start = l.start;
        l.start = 0;
        // a test run's slow frames are its verdict's, not the inbox's
        if !crate::jit::FIRST_PRESENT.load(Relaxed) || super::TEST_RUN.load(Relaxed) {
            return None;
        }
        let spike = FRAME_WATCH.load(Relaxed) && avg > 0 && ms > 100_000_000 && ms > avg * 4;
        let th = HITCH_NS.load(Relaxed);
        let hitch = th > 0 && ms >= th;
        if !spike && !hitch {
            return None;
        }
        let analysis = if hitch { Some(analyse(l, start, now)) } else { None };
        Some((spike, ms, avg, analysis))
    });
    let Ok(Some((spike, ms, avg, analysis))) = done else { return };
    if analysis.is_some() {
        super::flightrec::hitch(ms, avg);
    }
    if spike {
        super::events::emit(
            "spike",
            "frame",
            &format!("\"frame_ms\":{:.1},\"typical_ms\":{:.1},\"more\":[\"why-slow\",\"timeline start\",\"frame slowest\"]", ms as f64 / 1e6, avg as f64 / 1e6),
        );
    }
    if let Some(a) = analysis {
        let n = HITCHES.fetch_add(1, Relaxed) + 1;
        // at most HITCH_PER_S events a second; the others are counted in the next one
        let sec = now / 1_000_000_000;
        let w = HITCH_WINDOW.load(Relaxed);
        let in_window = if w >> 8 == sec { w & 0xff } else { 0 };
        if in_window >= HITCH_PER_S {
            HITCH_SKIPPED.fetch_add(1, Relaxed);
            return;
        }
        HITCH_WINDOW.store((sec << 8) | (in_window + 1), Relaxed);
        let skipped = HITCH_SKIPPED.swap(0, Relaxed);
        let last = HITCH_LAST_NS.swap(now, Relaxed);
        let since_last = if last == 0 { String::new() } else { format!(",\"since_last_hitch_ms\":{:.0}", now.saturating_sub(last) as f64 / 1e6) };
        super::events::emit(
            "hitch",
            &format!("hitch {}", n),
            &format!(
                "\"frame_ms\":{:.1},\"threshold_ms\":{},\"typical_ms\":{:.1},\"hitch\":{},\"not_reported\":{}{},{},\"more\":[\"events on=hitch\",\"why-slow\",\"timeline start\",\"probes\"]",
                ms as f64 / 1e6,
                HITCH_NS.load(Relaxed) / 1_000_000,
                avg as f64 / 1e6,
                n,
                skipped,
                since_last,
                a
            ),
        );
    }
}

/// The JSON members of a hitch: its phases (inclusive / self time by phase path), the UI
/// thread's waits, its lazy compiles, its critical path (where the frame's wall time went, in
/// order) and the probe deltas over the frame.
fn analyse(l: &FrameLog, t0: u64, t1: u64) -> String {
    let ms = |ns: u64| ns as f64 / 1e6;
    // phases by path (outer>inner, depth <= 3)
    let mut stack: Vec<(&'static str, u64, u64)> = Vec::new();
    let mut phases: Vec<(String, u64, u64, u32)> = Vec::new();
    let mut waits: Vec<(String, u64, u64, u32)> = Vec::new();
    // open waits: (label, begin, kind name; the end record carries no kind)
    let mut wait_open: Vec<(String, u64, &'static str)> = Vec::new();
    // the UI thread's waits for the owner thread: (begin, end, label), for what the owner served
    let mut owner_waits: Vec<(u64, u64, String)> = Vec::new();
    // the critical path: (what, ns) pieces in order
    let mut path: Vec<(String, u64)> = Vec::new();
    let mut compile_open = 0u32;
    let mut idle_open = false;
    let mut cur_t = t0;
    let what_now = |stack: &[(&'static str, u64, u64)], wait_open: &[(String, u64, &'static str)], compile_open: u32, idle_open: bool| -> String {
        if let Some(w) = wait_open.last() {
            format!("wait: {}", w.0)
        } else if compile_open > 0 {
            "lazy compile".into()
        } else if stack.is_empty() {
            // Between phases is UI-thread work outside every named phase.
            if idle_open { "idle".into() } else { "(between phases)".into() }
        } else {
            stack.iter().rev().take(3).rev().map(|x| x.0).collect::<Vec<_>>().join(">")
        }
    };
    let add_path = |path: &mut Vec<(String, u64)>, what: String, ns: u64| {
        if ns == 0 || what == "idle" {
            return;
        }
        match path.last_mut() {
            Some(p) if p.0 == what => p.1 += ns,
            _ => path.push((what, ns)),
        }
    };
    for r in &l.recs {
        let t = r.t.max(cur_t);
        add_path(&mut path, what_now(&stack, &wait_open, compile_open, idle_open), t - cur_t);
        cur_t = t;
        match r.kind {
            1 => stack.push((r.name, r.t, 0)),
            2 => {
                while let Some((n, b, child)) = stack.pop() {
                    let dur = r.t.saturating_sub(b);
                    let key = stack.iter().rev().take(2).rev().map(|x| x.0).chain(std::iter::once(n)).collect::<Vec<_>>().join(">");
                    match phases.iter_mut().find(|p| p.0 == key) {
                        Some(p) => {
                            p.1 += dur;
                            p.2 += dur.saturating_sub(child);
                            p.3 += 1;
                        }
                        None => phases.push((key, dur, dur.saturating_sub(child), 1)),
                    }
                    if let Some(parent) = stack.last_mut() {
                        parent.2 += dur;
                    }
                    if n == r.name {
                        break;
                    }
                }
            }
            3 => wait_open.push((if r.detail.is_empty() { r.name.to_string() } else { format!("{} ({})", r.name, r.detail) }, r.t, r.name)),
            4 => {
                if let Some((k, b, kind)) = wait_open.pop() {
                    let dur = r.t.saturating_sub(b);
                    if kind == "owner" {
                        owner_waits.push((b, r.t, k.clone()));
                    }
                    match waits.iter_mut().find(|w| w.0 == k) {
                        Some(w) => {
                            w.1 += dur;
                            w.2 = w.2.max(dur);
                            w.3 += 1;
                        }
                        None => waits.push((k, dur, dur, 1)),
                    }
                }
            }
            5 => compile_open += 1,
            6 => compile_open = compile_open.saturating_sub(1),
            7 => idle_open = true,
            8 => idle_open = false,
            _ => {}
        }
    }
    add_path(&mut path, what_now(&stack, &wait_open, compile_open, idle_open), t1.saturating_sub(cur_t));
    phases.sort_by(|a, b| b.1.cmp(&a.1));
    waits.sort_by(|a, b| b.1.cmp(&a.1));
    let ph: Vec<String> = phases.iter().take(10).map(|p| format!("{{\"phase\":{},\"ms\":{:.2},\"self_ms\":{:.2},\"n\":{}}}", json_str(&short(&p.0, 120)), ms(p.1), ms(p.2), p.3)).collect();
    let wt: Vec<String> = waits.iter().take(6).map(|w| format!("{{\"wait\":{},\"ms\":{:.2},\"max_ms\":{:.2},\"n\":{}}}", json_str(&short(&w.0, 120)), ms(w.1), ms(w.2), w.3)).collect();
    // the path: its 12 longest pieces in time order, and the time per kind of piece
    let total: u64 = path.iter().map(|p| p.1).sum();
    let mut at = 0u64;
    let mut pieces: Vec<(u64, u64, &str)> = Vec::new();
    for (w, ns) in &path {
        pieces.push((at, *ns, w.as_str()));
        at += ns;
    }
    let mut longest: Vec<(u64, u64, &str)> = pieces.clone();
    longest.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    longest.truncate(12);
    longest.sort_by_key(|p| p.0);
    let shown: u64 = longest.iter().map(|p| p.1).sum();
    let cp: Vec<String> = longest.iter().map(|(at, ns, w)| format!("{{\"at_ms\":{:.1},\"ms\":{:.2},\"on\":{}}}", ms(*at), ms(*ns), json_str(&short(w, 120)))).collect();
    let mut by: Vec<(&str, u64, u32)> = Vec::new();
    for (_, ns, w) in &pieces {
        match by.iter_mut().find(|b| b.0 == *w) {
            Some(b) => {
                b.1 += ns;
                b.2 += 1;
            }
            None => by.push((w, *ns, 1)),
        }
    }
    by.sort_by(|a, b| b.1.cmp(&a.1));
    let time_by: Vec<String> = by.iter().take(8).map(|(w, ns, n)| format!("{{\"on\":{},\"ms\":{:.2},\"pieces\":{}}}", json_str(&short(w, 120)), ms(*ns), n)).collect();
    let mut probes = Vec::new();
    if !l.probes0.is_empty() {
        let now = super::instrument::counters();
        let mut d: Vec<(String, u64, u64)> = now
            .iter()
            .map(|(k, h, ns)| {
                let (h0, ns0) = l.probes0.iter().find(|x| &x.0 == k).map_or((0, 0), |x| (x.1, x.2));
                (k.clone(), h.saturating_sub(h0), ns.saturating_sub(ns0))
            })
            .filter(|x| x.1 > 0)
            .collect();
        d.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        probes = d.iter().take(8).map(|(k, h, ns)| format!("{{\"counter\":{},\"delta\":{}{}}}", json_str(&short(k, 120)), h, if *ns > 0 { format!(",\"ms\":{:.2}", ms(*ns)) } else { String::new() })).collect();
    }
    // the 3 longest waits for the owner, each with what the owner served meanwhile
    owner_waits.sort_by(|a, b| (b.1 - b.0).cmp(&(a.1 - a.0)));
    let ow: Vec<String> = owner_waits.iter().filter(|w| w.1 - w.0 >= 1_000_000).take(3).map(|(b, e, k)| {
        format!("{{\"wait\":{},\"at_ms\":{:.1},\"ms\":{:.2},\"owner_served\":{}}}", json_str(&short(k, 120)), ms(b.saturating_sub(t0)), ms(e - b), crate::owner::served_during(*b, *e))
    }).collect();
    format!(
        "\"idle_gap_ms\":{:.1},\"phases\":[{}],\"waits\":[{}],\"owner_waits\":[{}],\"compile\":{{\"ms\":{:.2},\"n\":{}}},\"time_by\":[{}],\"critical_path\":[{}],\"critical_path_other_ms\":{:.2},\"probe_deltas\":[{}],\"records_dropped\":{}",
        ms(l.idle),
        ph.join(","),
        wt.join(","),
        ow.join(","),
        ms(l.compile_ns),
        l.compiles,
        time_by.join(","),
        cp.join(","),
        ms(total.saturating_sub(shown)),
        probes.join(","),
        l.dropped
    )
}

/// A lazy compile on this thread (jit.rs rt_compile): counted in the frame log of a UI thread
/// that is inside a frame (outermost compile only).
pub struct CompileMark(bool);

impl CompileMark {
    #[inline]
    pub fn begin() -> CompileMark {
        if HITCH_NS.load(Relaxed) == 0 {
            return CompileMark(false);
        }
        let on = FL
            .try_with(|c| {
                let Ok(mut g) = c.try_borrow_mut() else { return false };
                let Some(l) = g.as_mut() else { return false };
                if l.start == 0 {
                    return false;
                }
                l.compile_depth += 1;
                if l.compile_depth == 1 {
                    l.compile_t0 = crate::clock::wall_ns();
                    frame_rec(l, 5, "compile", "");
                }
                true
            })
            .unwrap_or(false);
        CompileMark(on)
    }
}

impl Drop for CompileMark {
    fn drop(&mut self) {
        if !self.0 {
            return;
        }
        let _ = FL.try_with(|c| {
            let Ok(mut g) = c.try_borrow_mut() else { return };
            let Some(l) = g.as_mut() else { return };
            l.compile_depth = l.compile_depth.saturating_sub(1);
            if l.compile_depth == 0 {
                l.compile_ns += crate::clock::wall_ns().saturating_sub(l.compile_t0);
                l.compiles += 1;
                frame_rec(l, 6, "compile", "");
            }
        });
    }
}

/// `hitch [ms=<n>|off]`: the hitch threshold (set or shown) and the count so far.
pub fn cmd_hitch(req: &Req, out: &mut Out) {
    match req.get("ms") {
        Some("off") | Some("0") => HITCH_NS.store(0, Relaxed),
        Some(v) => match v.parse::<u64>() {
            Ok(n) => HITCH_NS.store(n * 1_000_000, Relaxed),
            Err(_) => return out.err("hitch ms=<n>|off"),
        },
        None => {}
    }
    flag_update();
    out.line(&format!(
        "{{\"hitch_ms\":{},\"hitches\":{},\"more\":[\"events on=hitch\",\"watch on=hitch\"]}}",
        HITCH_NS.load(Relaxed) / 1_000_000,
        HITCHES.load(Relaxed)
    ));
}

/// The wait registry's hook (relax_base::waits::TRACE, set while recording): a wait begins (its
/// kind, detail label and object) or ends.
fn wait_trace(begin: bool, kind: &'static str, detail: &'static str, object: u64) {
    if HITCH_NS.load(Relaxed) > 0 {
        let _ = FL.try_with(|c| {
            let Ok(mut g) = c.try_borrow_mut() else { return };
            if let Some(l) = g.as_mut() {
                if l.start != 0 {
                    frame_rec(l, if begin { 3 } else { 4 }, kind, detail);
                }
                // the frame summary's waits (outermost only)
                if begin {
                    if l.wait_depth == 0 {
                        l.wait_t0 = crate::clock::wall_ns();
                        l.wait_owner = kind == "owner";
                        l.wait_in_compile = l.compile_depth > 0;
                    }
                    l.wait_depth += 1;
                } else if l.wait_depth > 0 {
                    l.wait_depth -= 1;
                    if l.wait_depth == 0 && l.start != 0 {
                        let w = crate::clock::wall_ns().saturating_sub(l.wait_t0.max(l.start));
                        l.wait_ns += w;
                        if l.wait_owner {
                            l.owner_ns += w;
                        }
                        if l.wait_in_compile {
                            l.cwait_ns += w;
                            if l.wait_owner {
                                l.cowner_ns += w;
                            }
                        }
                        l.waits += 1;
                    }
                }
            }
        });
    }
    if !recording() {
        return;
    }
    if begin {
        // the detail string travels packed in `b`: pointer (47 bits) and length (16 bits)
        push(Kind::WaitBegin, kind, object, (detail.as_ptr() as u64 & ((1 << 47) - 1)) | ((detail.len().min(0xffff) as u64) << 48));
    } else {
        push(Kind::WaitEnd, "", 0, 0);
    }
}

fn detail(b: u64) -> &'static str {
    let (p, n) = (b & ((1 << 47) - 1), (b >> 48) as usize);
    if p == 0 || n == 0 {
        return "";
    }
    unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(p as *const u8, n)) }
}

// ---- the GPU lane: spans the platform reports when its command buffers complete (Metal:
// GPUStartTime / GPUEndTime; Vulkan: timestamp queries calibrated to the CPU clock), in the
// host's raw monotonic clock (Linux CLOCK_MONOTONIC, macOS CLOCK_UPTIME_RAW = mach_absolute_time)

static GPU: Mutex<Vec<(u64, u64, &'static str)>> = Mutex::new(Vec::new());
const GPU_MAX: usize = 1 << 16;

/// The raw monotonic clock in ns (the domain of the platform's GPU span times).
fn raw_now_ns() -> u64 {
    #[cfg(unix)]
    {
        #[repr(C)]
        struct Ts {
            s: i64,
            ns: i64,
        }
        extern "C" {
            fn clock_gettime(id: i32, ts: *mut Ts) -> i32;
        }
        #[cfg(target_vendor = "apple")]
        let id = 8; // CLOCK_UPTIME_RAW
        #[cfg(not(target_vendor = "apple"))]
        let id = 1; // CLOCK_MONOTONIC
        let mut ts = Ts { s: 0, ns: 0 };
        unsafe { clock_gettime(id, &mut ts) };
        ts.s as u64 * 1_000_000_000 + ts.ns as u64
    }
    #[cfg(not(unix))]
    {
        crate::clock::wall_ns()
    }
}

/// makepad's GPU backends: a pass (`name`, static) ran on the GPU from `begin` to `end` (raw
/// monotonic ns). Recorded only while the timeline records.
pub extern "C" fn relax_gpu_span(name: *const u8, len: usize, begin: u64, end: u64) {
    let _kept = crate::zone_alloc::kept();
    if !recording() || end < begin {
        return;
    }
    let name = unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(name, len)) };
    if super::pstream::viewing() {
        super::pstream::gpu_span(name, begin, end);
    }
    if !RINGS_ON.load(Relaxed) {
        return;
    }
    // raw -> the timeline's clock (wall_ns): the offset between the two read now
    let off = raw_now_ns() as i128 - crate::clock::wall_ns() as i128;
    let map = |t: u64| (t as i128 - off).max(0) as u64;
    let mut g = GPU.lock().unwrap_or_else(|e| e.into_inner());
    if g.len() < GPU_MAX {
        g.push((map(begin), map(end), name));
    }
}

/// GPU spans of the capture overlapping [t0, t1): (begin, end, name).
fn gpu_spans(t0: u64, t1: u64) -> Vec<(u64, u64, &'static str)> {
    GPU.lock().unwrap_or_else(|e| e.into_inner()).iter().filter(|g| g.1 > t0 && g.0 < t1).copied().collect()
}

/// Relax std's futex wakes (sync/raw.rs: a contended unlock, a notify, an unpark).
pub extern "C" fn relax_wake(addr: usize) {
    if recording() {
        push(Kind::Wake, "", addr as u64, 0);
    }
}

// ------------------------------------------------------------------ capture control

/// Records of the last capture: per thread (os id, handle, records in time order).
fn collect() -> Vec<(u64, u64, Vec<Rec>)> {
    let e = EPOCH.load(Acquire);
    let rings = RINGS.lock().unwrap_or_else(|e| e.into_inner());
    rings
        .iter()
        .filter(|r| r.epoch.load(Relaxed) == e)
        .map(|r| {
            let n = r.next.load(Acquire).min(RING);
            let v: Vec<Rec> = (0..n).map(|i| unsafe { *r.recs[i].get() }).filter(|x| x.kind != 0).collect();
            (r.os_id, r.handle, v)
        })
        .filter(|x| !x.2.is_empty())
        .collect()
}

fn label(r: &Rec) -> &'static str {
    if r.label == 0 {
        return "";
    }
    unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(r.label as *const u8, r.label_len as usize)) }
}

/// A piece of a thread's time in the capture.
#[derive(Clone)]
struct Seg {
    t0: u64,
    t1: u64,
    /// span name, or "wait: <what>"
    what: String,
    wait: bool,
    /// a wait: what ended it (thread handle of the waker / server) and when, if known
    woke_by: Option<(u64, u64)>,
    /// a wait: the wake key that ends it (the futex / object; an owner request: u64::MAX - the
    /// requester's handle, the owner's `Served`)
    key: u64,
    depth: u32,
    /// still open when the capture stopped (cut there)
    open: bool,
}

/// Per thread: its spans and waits as segments, the wakes it issued (time, futex address) and its
/// event loop's idle waits (the UI thread's; begin, end).
struct Thread {
    os_id: u64,
    handle: u64,
    segs: Vec<Seg>,
    wakes: Vec<(u64, u64)>,
    idles: Vec<(u64, u64)>,
}

fn build(recs: Vec<(u64, u64, Vec<Rec>)>, stop: u64) -> Vec<Thread> {
    let mut out: Vec<Thread> = Vec::new();
    for (os_id, handle, v) in recs {
        let mut segs = Vec::new();
        let mut stack: Vec<(u64, String, bool, u64, u64)> = Vec::new();
        let mut wakes = Vec::new();
        let mut idles = Vec::new();
        let mut idle_from: Option<u64> = None;
        for r in &v {
            match r.kind {
                1 => stack.push((r.t, label(r).to_string(), false, 0, 0)),
                3 => stack.push((r.t, format!("wait: {} {}", label(r), detail(r.b)).trim_end().to_string(), true, r.a, r.b)),
                2 | 4 => {
                    // the innermost open record of the same kind
                    let want_wait = r.kind == 4;
                    if let Some(k) = stack.iter().rposition(|x| x.2 == want_wait) {
                        let (t0, what, wait, obj, _) = stack.remove(k);
                        let key = wait_key(&what, obj, handle);
                        segs.push(Seg { t0, t1: r.t, what, wait, woke_by: None, key, depth: k as u32, open: false });
                    }
                }
                5 => wakes.push((r.t, r.a)),
                6 => wakes.push((r.t, u64::MAX - r.a)),
                7 => idle_from = Some(r.t),
                // (an idle that began before the capture ends a turn all the same)
                8 => idles.push((idle_from.take().unwrap_or(r.t), r.t)),
                _ => {}
            }
        }
        if let Some(t0) = idle_from {
            idles.push((t0, stop));
        }
        // still open at the stop: cut there
        for (k, (t0, what, wait, obj, _)) in stack.into_iter().enumerate() {
            let key = wait_key(&what, obj, handle);
            segs.push(Seg { t0, t1: stop, what, wait, woke_by: None, key, depth: k as u32, open: true });
        }
        segs.sort_by_key(|s| (s.t0, s.depth));
        out.push(Thread { os_id, handle, segs, wakes, idles });
    }
    // edges: what ended a wait is a wake of its key by another thread (a waiter never wakes
    // itself: its own unlock just after it took the lock is no waker) after the wait began and
    // at most just after its end (a wake stamped just after it: another thread's clock read). A
    // wait with one waiter (the owner idle, a park) is ended by the first such wake: a later one
    // found it waking already. A lock or condvar has many: each wake ends one waiter, the last
    // one before the end ended this wait.
    let all_wakes: Vec<(u64, u64, u64)> = out.iter().flat_map(|th| th.wakes.iter().map(move |w| (w.0, w.1, th.handle))).collect();
    for th in out.iter_mut() {
        let me = th.handle;
        for s in th.segs.iter_mut().filter(|s| s.wait) {
            let cand = all_wakes.iter().filter(|w| w.1 == s.key && w.2 != me && w.0 > s.t0 && w.0 <= s.t1 + 50_000);
            let w = if one_waiter(&s.what) { cand.min_by_key(|w| w.0) } else { cand.max_by_key(|w| w.0) };
            s.woke_by = w.map(|w| (w.2, w.0));
        }
    }
    out
}

/// The wake key that ends wait `what` on `obj` of thread `handle`: an owner request is ended by
/// the owner's `Served` for the thread, anything else by a wake of its object.
fn wait_key(what: &str, obj: u64, handle: u64) -> u64 {
    if what.starts_with("wait: owner") { u64::MAX - handle } else { obj }
}

/// A wait only one thread waits on: the owner idle (any requester's post wakes it), a park.
fn one_waiter(what: &str) -> bool {
    what == "wait: idle" || what == "wait: futex park"
}

/// `timeline start [max_s=30]` | `timeline stop` | `timeline frames [n=30]` | `timeline frame
/// <n|slowest>` | `timeline save`
pub fn cmd_timeline(req: &Req, out: &mut Out) {
    match req.pos.first().map(String::as_str).unwrap_or("status") {
        "start" => {
            EPOCH.fetch_add(1, AcqRel);
            STARTED_NS.store(crate::clock::wall_ns(), Relaxed);
            STOPPED_NS.store(0, Relaxed);
            GPU.lock().unwrap_or_else(|e| e.into_inner()).clear();
            RINGS_ON.store(true, Release);
            flag_update();
            let max = req.num("max_s", 30);
            let e = EPOCH.load(Relaxed);
            // a forgotten recording stops itself
            { let _k = crate::zone_alloc::kept();
            let _ = crate::run_threads::spawn("run-inspect-tl", move || {
                super::heap::exclude_this_thread();
                if crate::run_threads::sleep(std::time::Duration::from_secs(max)) && EPOCH.load(Relaxed) == e && RINGS_ON.load(Relaxed) {
                    stop();
                }
            });
            }
            super::mem::mark("timeline start");
            out.line(&format!("{{\"timeline\":\"recording\",\"max_s\":{},\"more\":[\"timeline stop\",\"timeline frames\",\"timeline frame slowest\",\"timeline save\"]}}", max));
        }
        "stop" => {
            stop();
            let recs = collect();
            let n: usize = recs.iter().map(|x| x.2.len()).sum();
            out.line(&format!("{{\"timeline\":\"stopped\",\"threads\":{},\"records\":{},\"ms\":{}}}", recs.len(), n, (STOPPED_NS.load(Relaxed) - STARTED_NS.load(Relaxed)) / 1_000_000));
        }
        "status" => out.line(&format!("{{\"recording\":{}}}", recording())),
        "frames" => frames_cmd(req, out),
        "frame" => frame_cmd(req, out),
        "save" => save_cmd(out),
        "waits" | "contention" => waits_cmd(req, out),
        _ => out.err("usage: timeline start [max_s=30] | stop | frames [n=30] | frame <n|slowest> | waits [s=3] [n=15] | save"),
    }
}
use std::sync::atomic::Ordering::AcqRel;

fn stop() {
    RINGS_ON.store(false, Release);
    flag_update();
    STOPPED_NS.store(crate::clock::wall_ns(), Relaxed);
    super::mem::mark("timeline stop");
}

fn stopped_or_now() -> u64 {
    match STOPPED_NS.load(Relaxed) {
        0 => crate::clock::wall_ns(),
        t => t,
    }
}

/// The UI thread: the one with root spans of makepad's phases (the main thread).
fn ui_thread(ths: &[Thread]) -> Option<usize> {
    let main = crate::jit::MAIN_TID.load(Relaxed);
    ths.iter().position(|t| t.handle == main && main != 0).or_else(|| ths.iter().enumerate().filter(|(_, t)| t.segs.iter().any(|s| !s.wait)).max_by_key(|(_, t)| t.segs.len()).map(|x| x.0))
}

/// Frames: the windows on the UI thread that drew. A frame ends when its `Draw` dispatches end
/// (Draw dispatches less than 1 ms apart, one per window or pass, close one frame) and starts at
/// the first of the UI thread's root spans leading up to it without an idle gap over
/// `FRAME_GAP_NS`, never before the previous frame's end: the rule `pf` and an app frame a saved
/// capture by. A wake that draws nothing (a timer, a task, a signal) is no frame's (`beats_of`).
/// Frames still open at the stop are left out.
fn frames_of(ui: &Thread) -> Vec<(u64, u64)> {
    use makepad_prof_stream::model::FRAME_GAP_NS;
    // depth 0 does not overlap: spans and waits in time order
    let rows: Vec<&Seg> = ui.segs.iter().filter(|s| s.depth == 0).collect();
    let mut v: Vec<(u64, u64)> = Vec::new();
    // Draw dispatches at any depth (a native event phase may wrap them), in time order
    for d in ui.segs.iter().filter(|s| !s.wait && s.what == "Draw" && !s.open) {
        let prev_end = match v.last_mut() {
            Some(last) if d.t0 < last.1 + 1_000_000 => {
                last.1 = last.1.max(d.t1);
                continue;
            }
            Some(last) => last.1,
            None => 0,
        };
        // the root holding the Draw
        let k = rows.partition_point(|r| r.t0 <= d.t0);
        let mut t0 = if k > 0 && rows[k - 1].t1 >= d.t1 { rows[k - 1].t0 } else { d.t0 };
        let mut k = rows.partition_point(|r| r.t0 < t0);
        while k > 0 {
            let r = rows[k - 1];
            if r.t0 < prev_end || t0.saturating_sub(r.t1) > FRAME_GAP_NS {
                break;
            }
            if !r.wait {
                t0 = r.t0;
            }
            k -= 1;
        }
        v.push((t0.max(prev_end), d.t1));
    }
    v
}

/// A wake of the UI thread that drew nothing: the root spans of one turn of its event loop (from
/// an idle wait's end to its next idle wait) when no Draw ran in that turn and no frame holds
/// them. `woke` is the turn's first root span: the event or phase that woke the loop (a timer,
/// a signal, the tasks, ...).
struct Beat {
    t0: u64,
    t1: u64,
    woke: String,
    phases: Vec<String>,
}

fn beats_of(ui: &Thread, frames: &[(u64, u64)]) -> Vec<Beat> {
    // a turn: the idle waits that ended before it
    let turn = |t: u64| ui.idles.partition_point(|i| i.1 <= t);
    let drew: std::collections::HashSet<usize> = ui.segs.iter().filter(|s| !s.wait && s.what == "Draw").map(|s| turn(s.t0)).collect();
    let mut v: Vec<(usize, Beat)> = Vec::new();
    for r in ui.segs.iter().filter(|s| s.depth == 0 && !s.wait && !s.open) {
        let k = turn(r.t0);
        let f = frames.partition_point(|f| f.1 <= r.t0);
        if drew.contains(&k) || frames.get(f).is_some_and(|f| f.0 < r.t1) {
            continue;
        }
        match v.last_mut() {
            Some((kk, b)) if *kk == k => {
                b.t1 = r.t1;
                b.phases.push(r.what.clone());
            }
            _ => v.push((k, Beat { t0: r.t0, t1: r.t1, woke: r.what.clone(), phases: vec![r.what.clone()] })),
        }
    }
    v.into_iter().map(|x| x.1).collect()
}

/// `n` of each name, most first (then by name).
fn tally<'a>(names: impl Iterator<Item = &'a str>) -> Vec<(&'a str, usize)> {
    let mut v: Vec<(&str, usize)> = Vec::new();
    for n in names {
        match v.iter_mut().find(|x| x.0 == n) {
            Some(x) => x.1 += 1,
            None => v.push((n, 1)),
        }
    }
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    v
}

fn tally_json(v: &[(&str, usize)]) -> String {
    v.iter().map(|(n, c)| format!("[{},{}]", json_str(n), c)).collect::<Vec<_>>().join(",")
}

/// The line of a capture's event-only beats: how many, what woke them, the phases they ran and
/// their UI time.
fn beats_line(ui: &Thread, beats: &[Beat], wall_ns: u64) -> String {
    let ranges: Vec<(u64, u64)> = beats.iter().map(|b| (b.t0, b.t1)).collect();
    let mut busy: Vec<u64> = busy_per_range(ui, &ranges).into_iter().map(|x| x.0).collect();
    busy.sort_unstable();
    let at = |p: f64| busy.get(((busy.len() as f64 - 1.0) * p).round() as usize).copied().unwrap_or(0) as f64 / 1e6;
    format!(
        "{{\"event_only_beats\":{{\"n\":{},\"per_s\":{:.1},\"woken_by\":[{}],\"phases\":[{}],\"ui_busy_ms\":{{\"median\":{:.2},\"max\":{:.2}}},\"more\":[\"trace topics=frame_sources\",\"log\"]}}}}",
        beats.len(),
        beats.len() as f64 * 1e9 / wall_ns.max(1) as f64,
        tally_json(&tally(beats.iter().map(|b| b.woke.as_str()))),
        tally_json(&tally(beats.iter().flat_map(|b| b.phases.iter().map(String::as_str)))),
        at(0.5),
        at(1.0)
    )
}

/// Busy and wait time of a thread in each of `ranges` (in time order, not overlapping), one walk of
/// its spans: time inside its non-wait root spans minus the waits inside them (threads without
/// spans of their own count their wait-free time between waits as unknown). The UI thread marks
/// its wait for a frame on the GPU as a phase: that time is a wait too.
fn busy_per_range(th: &Thread, ranges: &[(u64, u64)]) -> Vec<(u64, u64)> {
    let mut spans = vec![0u64; ranges.len()];
    let mut waits = vec![0u64; ranges.len()];
    for s in &th.segs {
        let root = !s.wait && s.depth == 0;
        let wait = s.wait || s.what == "gpu-wait";
        if !root && !wait { continue; }
        let first = ranges.partition_point(|r| r.1 <= s.t0);
        for (i, r) in ranges.iter().enumerate().skip(first).take_while(|(_, r)| r.0 < s.t1) {
            let clipped = s.t1.min(r.1).saturating_sub(s.t0.max(r.0));
            if root { spans[i] += clipped; }
            if wait { waits[i] += clipped; }
        }
    }
    spans.iter().zip(&waits).map(|(s, w)| (s.saturating_sub(*w), *w)).collect()
}

fn busy_in(th: &Thread, t0: u64, t1: u64) -> (u64, u64) {
    busy_per_range(th, &[(t0, t1)])[0]
}

/// The critical path of [t0, t1) ending on thread `ui`: walking back from t1, the UI thread's
/// own busy time counts; a wait ended by another thread's wake moves the walk to that thread at
/// the wake, for the time the wait lasted only: before the wait began the waiting thread ran
/// itself, and the walk returns to it there (the waker's earlier history is not this path's).
fn critical(ths: &[Thread], ui: usize, t0: u64, t1: u64) -> Vec<(usize, u64, u64, String)> {
    let mut path = Vec::new();
    let (mut th, mut t) = (ui, t1);
    // the thread whose wait moved the walk here: the next wait met is explained by its wake if it
    // has one (the owner woke for the UI's own request, not for a post just after it)
    let mut from: Option<usize> = None;
    // the threads the walk left at a wake: (thread, the start of its wait = where it resumes)
    let mut back: Vec<(usize, u64)> = Vec::new();
    // (a cold lazy compile's frame is hundreds of UI -> owner -> UI round trips, three pieces each)
    for _ in 0..16384 {
        let floor = back.last().map_or(t0, |b| b.1);
        if t <= floor {
            match back.pop() {
                Some((b, at)) => {
                    th = b;
                    t = at;
                    from = None;
                    continue;
                }
                None => break,
            }
        }
        // the latest wait on this thread that overlaps [floor, t)
        let w = ths[th].segs.iter().filter(|s| s.wait && s.t0 < t && s.t1 > floor).max_by_key(|s| s.t1.min(t));
        match w {
            Some(s) => {
                let end = s.t1.min(t);
                if end < t {
                    path.push((th, end, t, "run".to_string()));
                }
                let start = s.t0.max(floor);
                // the wake explains the wait only where the walk meets the wait's end: met inside
                // it (this thread's own request woke the owner while it waited), the thread waited
                // since the wait began. A wake stamped just after the end (another thread's clock
                // read) counts as just before it, so the walk only goes back in time.
                let came = from.take().and_then(|f| ths[f].wakes.iter().filter(|w| w.1 == s.key && w.0 > s.t0 && w.0 <= s.t1 + 50_000).min_by_key(|w| w.0).map(|w| (f, w.0)));
                match came.or_else(|| s.woke_by.and_then(|(h, at)| ths.iter().position(|x| x.handle == h).map(|i| (i, at)))) {
                    Some((i, at)) if i != th && at > s.t0 && end == s.t1 => {
                        let at = at.max(start).min(end.saturating_sub(1).max(start));
                        path.push((th, at, end, format!("{} (ended by {})", s.what, super::tid_label(ths[i].os_id))));
                        back.push((th, start));
                        from = Some(th);
                        th = i;
                        t = at;
                    }
                    _ => {
                        path.push((th, start, end, s.what.clone()));
                        t = start;
                    }
                }
            }
            None => {
                path.push((th, floor, t, "run".to_string()));
                t = floor;
            }
        }
    }
    path
}

fn frames_cmd(req: &Req, out: &mut Out) {
    let stop = stopped_or_now();
    let ths = build(collect(), stop);
    let Some(ui) = ui_thread(&ths) else {
        out.err("no records: `timeline start`, use the app, `timeline stop` (makepad's UI phases need the platform's Relax hook)");
        return;
    };
    let fr = frames_of(&ths[ui]);
    let beats = beats_of(&ths[ui], &fr);
    out.measured(&format!("{} frames drew on the UI thread, {} wakes drew nothing, {} threads recorded", fr.len(), beats.len(), ths.len()), "timeline rings", &[]);
    if !beats.is_empty() {
        out.line(&beats_line(&ths[ui], &beats, stop.saturating_sub(STARTED_NS.load(Relaxed))));
    }
    let durs: Vec<f64> = fr.iter().map(|f| (f.1 - f.0) as f64 / 1e6).collect();
    let mut sorted = durs.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let pct = |p: f64| sorted.get(((sorted.len() as f64 - 1.0) * p).round() as usize).copied().unwrap_or(0.0);
    // every frame's duration in order (a sparkline's data), 0.1 ms resolution
    let series: Vec<String> = durs.iter().map(|d| format!("{:.1}", d)).collect();
    out.line(&format!("{{\"frame_ms\":{{\"n\":{},\"median\":{:.2},\"p95\":{:.2},\"max\":{:.2}}},\"series_ms\":[{}]}}", durs.len(), pct(0.5), pct(0.95), pct(1.0), series.join(",")));
    // over every frame: the frames listed below are the slowest, not a sample of them
    let of_all = busy_per_range(&ths[ui], &fr);
    let ms_at = |mut v: Vec<u64>, p: f64| { v.sort_unstable(); v.get(((v.len() as f64 - 1.0) * p).round() as usize).copied().unwrap_or(0) as f64 / 1e6 };
    let (busy_all, wait_all): (Vec<u64>, Vec<u64>) = of_all.into_iter().unzip();
    out.line(&format!(
        "{{\"ui_busy_ms\":{{\"median\":{:.2},\"p95\":{:.2}}},\"ui_wait_ms\":{{\"median\":{:.2},\"p95\":{:.2}}},\"frames_listed\":\"slowest first\"}}",
        ms_at(busy_all.clone(), 0.5), ms_at(busy_all, 0.95), ms_at(wait_all.clone(), 0.5), ms_at(wait_all, 0.95)
    ));
    let n = req.num("n", 30) as usize;
    let mut idx: Vec<usize> = (0..fr.len()).collect();
    idx.sort_by(|a, b| durs[*b].partial_cmp(&durs[*a]).unwrap().then(a.cmp(b)));
    for &i in idx.iter().take(n) {
        let (t0, t1) = fr[i];
        let (busy, wait) = busy_in(&ths[ui], t0, t1);
        let work: u64 = ths.iter().map(|t| busy_in(t, t0, t1).0).sum();
        let cp: u64 = critical(&ths, ui, t0, t1).iter().filter(|p| p.3 == "run").map(|p| p.2 - p.1).sum();
        out.line(&format!(
            "{{\"frame\":\"F{}\",\"ms\":{:.2},\"ui_busy_ms\":{:.2},\"ui_wait_ms\":{:.2},\"work_ms\":{:.2},\"critical_path_ms\":{:.2},\"achieved_parallelism\":{:.2},\"gpu_ms\":{:.2},\"more\":\"timeline frame {}\"}}",
            i,
            durs[i],
            busy as f64 / 1e6,
            wait as f64 / 1e6,
            work as f64 / 1e6,
            cp as f64 / 1e6,
            work as f64 / (t1 - t0).max(1) as f64,
            gpu_spans(t0, t1).iter().map(|g| g.1.min(t1) - g.0.max(t0)).sum::<u64>() as f64 / 1e6,
            i
        ));
    }
}

/// `timeline waits [s=3] [n=15]` (alias `contention`): lock / condvar / park / owner waits of a
/// capture grouped by what was waited on: count, total and longest wait, per frame, which threads
/// waited and who ended the waits most. `s=`: record that long first (else the last capture).
fn waits_cmd(req: &Req, out: &mut Out) {
    if let Some(sv) = req.get("s").and_then(|v| v.parse::<f64>().ok()) {
        if recording() {
            return out.err("a timeline capture is running: `timeline stop` first (or ask without s=)");
        }
        EPOCH.fetch_add(1, AcqRel);
        STARTED_NS.store(crate::clock::wall_ns(), Relaxed);
        STOPPED_NS.store(0, Relaxed);
        RINGS_ON.store(true, Release);
        flag_update();
        let gone = out.gone();
        let t0 = std::time::Instant::now();
        while t0.elapsed().as_secs_f64() < sv.clamp(0.2, 30.0) && !gone() {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        stop();
    }
    let stop_t = stopped_or_now();
    let ths = build(collect(), stop_t);
    if ths.is_empty() {
        return out.err("no capture: `timeline waits s=3` records 3 s first");
    }
    let wall = stop_t.saturating_sub(STARTED_NS.load(Relaxed)).max(1);
    let frames = ui_thread(&ths).map_or(0, |ui| frames_of(&ths[ui]).len());
    let tname = |h: u64| -> String {
        let all = super::threads::list();
        match ths.iter().find(|t| t.handle == h) {
            Some(t) => all.iter().find(|x| x.os_id == t.os_id).map(|x| if x.name.is_empty() { format!("T{}", x.os_id) } else { x.name.clone() }).unwrap_or_else(|| format!("T{}", t.os_id)),
            None => "?".into(),
        }
    };
    // (what, total ns, max ns, count, waiting threads, wakers)
    let mut groups: Vec<(String, u64, u64, u64, Vec<u64>, Vec<(u64, u64)>)> = Vec::new();
    for th in &ths {
        for sg in th.segs.iter().filter(|s| s.wait) {
            let d = sg.t1.saturating_sub(sg.t0);
            let what = sg.what.trim_start_matches("wait: ").to_string();
            let k = match groups.iter().position(|g| g.0 == what) {
                Some(k) => k,
                None => {
                    groups.push((what, 0, 0, 0, Vec::new(), Vec::new()));
                    groups.len() - 1
                }
            };
            let g = &mut groups[k];
            g.1 += d;
            g.2 = g.2.max(d);
            g.3 += 1;
            if !g.4.contains(&th.handle) {
                g.4.push(th.handle);
            }
            if let Some((w, _)) = sg.woke_by.filter(|w| w.0 != 0) {
                match g.5.iter_mut().find(|x| x.0 == w) {
                    Some(x) => x.1 += 1,
                    None => g.5.push((w, 1)),
                }
            }
        }
    }
    groups.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let total: u64 = groups.iter().map(|g| g.1).sum();
    let Some(top) = groups.first() else {
        out.line(&format!("no waits in {:.1} s ({} threads recorded): nothing blocked", wall as f64 / 1e9, ths.len()));
        return;
    };
    out.line(&format!(
        "waits took {:.1} ms of thread time in {:.1} s{}; most: `{}` {:.1} ms in {} waits (longest {:.2} ms)",
        total as f64 / 1e6,
        wall as f64 / 1e9,
        if frames > 0 { format!(" ({} frames)", frames) } else { String::new() },
        short(&top.0, 80),
        top.1 as f64 / 1e6,
        top.3,
        top.2 as f64 / 1e6
    ));
    out.line("");
    out.line("| waited on | total ms | waits | longest ms | per frame ms | waiting threads | ended by |");
    out.line("|---|---:|---:|---:|---:|---|---|");
    let n = req.num("n", 15) as usize;
    for g in groups.iter().take(n) {
        let mut wk = g.5.clone();
        wk.sort_by(|a, b| b.1.cmp(&a.1));
        out.line(&format!(
            "| {} | {:.2} | {} | {:.2} | {} | {} | {} |",
            short(&g.0.replace('|', "/"), 70),
            g.1 as f64 / 1e6,
            g.3,
            g.2 as f64 / 1e6,
            if frames > 0 { format!("{:.3}", g.1 as f64 / 1e6 / frames as f64) } else { "-".into() },
            g.4.iter().take(3).map(|h| tname(*h)).collect::<Vec<_>>().join(", "),
            wk.iter().take(2).map(|(h, c)| format!("{} ({})", tname(*h), c)).collect::<Vec<_>>().join(", ")
        ));
    }
    if groups.len() > n {
        let rest: u64 = groups[n..].iter().map(|g| g.1).sum();
        out.line(&format!("+{} more waited-on objects, {:.2} ms", groups.len() - n, rest as f64 / 1e6));
    }
    out.line("note: waits the registry sees (program Mutex / Condvar / park / once / join, owner requests, query latches); a wait's waker is the futex wake or owner reply that ended it");
    out.line(&format!("note: {}", super::reliability::line(true)));
    out.line("more: timeline frames  timeline frame slowest  why-hung  waits");
}

fn frame_cmd(req: &Req, out: &mut Out) {
    let stop = stopped_or_now();
    let ths = build(collect(), stop);
    let Some(ui) = ui_thread(&ths) else {
        out.err("no records");
        return;
    };
    let fr = frames_of(&ths[ui]);
    let which = req.pos.get(1).map(String::as_str).unwrap_or("slowest");
    let i = match which {
        "slowest" => (0..fr.len()).max_by_key(|&i| fr[i].1 - fr[i].0),
        w => w.trim_start_matches('F').parse::<usize>().ok().filter(|&i| i < fr.len()),
    };
    let Some(i) = i else {
        let beats = beats_of(&ths[ui], &fr).len();
        out.err(&format!("no frame `{}` ({} frames drew, {} wakes drew nothing: `timeline frames`)", which, fr.len(), beats));
        return;
    };
    let (t0, t1) = fr[i];
    out.measured(&format!("frame F{}: {:.2} ms", i, (t1 - t0) as f64 / 1e6), "timeline rings", &[]);
    // the UI thread's phases in the frame: exclusive time (nested phases subtracted), longest first
    let ui_spans: Vec<&Seg> = ths[ui].segs.iter().filter(|s| !s.wait && s.t1 > t0 && s.t0 < t1).collect();
    let mut phases: Vec<(String, u64)> = Vec::new();
    for s in &ui_spans {
        let own = s.t1.min(t1) - s.t0.max(t0);
        let inner: u64 = ui_spans.iter().filter(|c| c.depth == s.depth + 1 && c.t0 >= s.t0 && c.t1 <= s.t1).map(|c| c.t1.min(t1) - c.t0.max(t0)).sum();
        let d = own.saturating_sub(inner);
        match phases.iter_mut().find(|p| p.0 == s.what) {
            Some(p) => p.1 += d,
            None => phases.push((s.what.clone(), d)),
        }
    }
    phases.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let ph: Vec<String> = phases.iter().take(12).map(|(k, d)| format!("[{},{:.2}]", json_str(k), *d as f64 / 1e6)).collect();
    out.line(&format!("{{\"ui_phases_exclusive_ms\":[{}]}}", ph.join(",")));
    // the critical path, oldest first: each piece labelled by what its thread was doing (its
    // innermost span), consecutive pieces of the same thread and label merged
    let mut cp = critical(&ths, ui, t0, t1);
    cp.reverse();
    let total: u64 = cp.iter().filter(|p| p.3 == "run").map(|p| p.2 - p.1).sum();
    let mut steps: Vec<(usize, u64, String)> = Vec::new();
    for (th, a, b, w) in cp.iter().filter(|p| p.2 > p.1) {
        let what = if w == "run" {
            let mid = a + (b - a) / 2;
            ths[*th].segs.iter().filter(|s| !s.wait && s.t0 <= mid && s.t1 >= mid).max_by_key(|s| s.depth).map(|s| s.what.clone()).unwrap_or_else(|| "run (unmarked)".into())
        } else {
            w.clone()
        };
        match steps.last_mut() {
            Some(l) if l.0 == *th && (l.2 == what || (b - a) < 200_000) => l.1 += b - a,
            _ => steps.push((*th, b - a, what)),
        }
    }
    let st: Vec<String> = steps.iter().take(16).map(|(th, d, w)| format!("{{\"thread\":\"{}\",\"ms\":{:.2},\"what\":{}}}", super::tid_label(ths[*th].os_id), *d as f64 / 1e6, json_str(&short(w, 120)))).collect();
    out.line(&format!("{{\"critical_path_busy_ms\":{:.2},\"steps\":[{}]}}", total as f64 / 1e6, st.join(",")));
    // the path's time by thread and label, most first (a path of many short round trips)
    let mut by: Vec<(usize, String, u64, usize)> = Vec::new();
    for (th, d, w) in &steps {
        let w = w.split(" (ended by ").next().unwrap_or(w);
        match by.iter_mut().find(|x| x.0 == *th && x.1 == w) {
            Some(x) => { x.2 += d; x.3 += 1; }
            None => by.push((*th, w.to_string(), *d, 1)),
        }
    }
    by.sort_by(|a, b| b.2.cmp(&a.2));
    let by: Vec<String> = by.iter().take(10).map(|(th, w, d, n)| format!("{{\"thread\":\"{}\",\"what\":{},\"ms\":{:.2},\"pieces\":{}}}", super::tid_label(ths[*th].os_id), json_str(&short(w, 120)), *d as f64 / 1e6, n)).collect();
    out.line(&format!("{{\"critical_path_by\":[{}]}}", by.join(",")));
    // the UI thread's waits in the frame, longest first, with who ended them
    let mut waits: Vec<(u64, String, Option<u64>)> = Vec::new();
    for s in ths[ui].segs.iter().filter(|s| s.wait && s.t1 > t0 && s.t0 < t1) {
        let by = s.woke_by.and_then(|(h, _)| ths.iter().find(|x| x.handle == h).map(|x| x.os_id));
        waits.push((s.t1.min(t1) - s.t0.max(t0), s.what.clone(), by));
    }
    waits.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let w: Vec<String> = waits
        .iter()
        .take(10)
        .map(|(d, what, by)| format!("{{\"ms\":{:.2},\"what\":{}{}}}", *d as f64 / 1e6, json_str(&short(what, 100)), by.map(|b| format!(",\"ended_by\":\"{}\"", super::tid_label(b))).unwrap_or_default()))
        .collect();
    out.line(&format!("{{\"ui_waits\":[{}]}}", w.join(",")));
    // the GPU lane: passes overlapping the frame
    let g = gpu_spans(t0, t1);
    if !g.is_empty() {
        let busy: u64 = g.iter().map(|x| x.1.min(t1) - x.0.max(t0)).sum();
        let mut per: Vec<(&str, u64)> = Vec::new();
        for x in &g {
            let d = x.1.min(t1) - x.0.max(t0);
            match per.iter_mut().find(|p| p.0 == x.2) {
                Some(p) => p.1 += d,
                None => per.push((x.2, d)),
            }
        }
        per.sort_by(|a, b| b.1.cmp(&a.1));
        let ps: Vec<String> = per.iter().take(8).map(|(n, d)| format!("[{},{:.2}]", json_str(n), *d as f64 / 1e6)).collect();
        out.line(&format!("{{\"gpu\":{{\"busy_ms\":{:.2},\"passes_ms\":[{}]}}}}", busy as f64 / 1e6, ps.join(",")));
    }
    // per thread busy / wait in the frame, with its lane: one column per 1/60 of the frame,
    // `█` busy most of it, `▄` busy some, `░` waiting, ` ` not recorded (idle or unmarked)
    let cols = 60u64;
    let os_threads = super::threads::list();
    let tname = |id: u64| os_threads.iter().find(|t| t.os_id == id).map(|t| t.name.clone()).unwrap_or_default();
    let mut per: Vec<String> = Vec::new();
    for th in &ths {
        let (b, wt) = busy_in(th, t0, t1);
        if b > 0 {
            let step = ((t1 - t0) / cols).max(1);
            let lane: String = (0..cols)
                .map(|c| {
                    let (a, z) = (t0 + c * step, (t0 + (c + 1) * step).min(t1));
                    let (cb, cw) = busy_in(th, a, z);
                    if cb * 2 >= z - a { '█' } else if cb > 0 { '▄' } else if cw * 2 >= z - a { '░' } else { ' ' }
                })
                .collect();
            per.push(format!("{{\"thread\":\"{}\",\"name\":{},\"busy_ms\":{:.2},\"wait_ms\":{:.2},\"lane\":{}}}", super::tid_label(th.os_id), json_str(&tname(th.os_id)), b as f64 / 1e6, wt as f64 / 1e6, json_str(&lane)));
        }
    }
    out.line(&format!("{{\"threads\":[{}],\"frame_ms\":{:.2}}}", per.join(","), (t1 - t0) as f64 / 1e6));
}

/// `timeline save`: the capture as a `.mkpf` profile recording (makepad-prof-stream; the format
/// a live `prof` viewer receives): an app opens it, `makepad inspect file=<x.mkpf> pf <query>`
/// answers on it.
fn save_cmd(out: &mut Out) {
    use makepad_prof_stream as ps;
    let mut names = super::threads::list();
    super::name_handles(&mut names);
    let off = ps::mono_ns() as i128 - crate::clock::wall_ns() as i128;
    let map = |t: u64| (t as i128 + off).max(0) as u64;
    let mut strings = ps::Interner::default();
    let _ = strings.id("");
    let mut body = Vec::new();
    let mut n_recs = 0usize;
    let mut lanes = Vec::new();
    for (k, (os_id, handle, v)) in collect().into_iter().enumerate() {
        let tid = k as u32 + 1;
        let nm = names.iter().find(|t| t.os_id == os_id).map(|t| t.name.clone()).unwrap_or_default();
        let sid = strings.id(&nm);
        let mut recs = Vec::with_capacity(v.len());
        // the idle markers are the rings' own (the recording's span kinds stop at Served)
        for r in v.iter().filter(|r| r.kind <= Kind::Served as u8) {
            let name = if r.kind == Kind::WaitBegin as u8 {
                let d = detail(r.b);
                strings.id(&if d.is_empty() { label(r).to_string() } else { format!("{} {}", label(r), d) })
            } else {
                strings.id(label(r))
            };
            recs.push(ps::SpanRec { kind: r.kind, t: map(r.t), name, a: r.a });
        }
        n_recs += recs.len();
        lanes.push((ps::thread_payload(tid, os_id, handle, super::pstream::role_of(handle, &nm), sid), ps::spans_payload(tid, &recs)));
    }
    let gpu = gpu_spans(0, u64::MAX);
    let mut gpu_recs = Vec::new();
    for (b, e, n) in &gpu {
        let name = strings.id(n);
        gpu_recs.push(ps::SpanRec { kind: ps::span::BEGIN, t: map(*b), name, a: 0 });
        gpu_recs.push(ps::SpanRec { kind: ps::span::END, t: map(*e), name, a: 0 });
    }
    let gpu_sid = strings.id("GPU");
    let mut file = ps::prelude().to_vec();
    let app = super::APP.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let hello = ps::Hello { app, host: String::new(), pid: std::process::id() as u64, build: crate::jit::PROVENANCE.to_string(), device: String::new(), refresh_mhz: 0, flags: 1, start_mono: (ps::mono_ns() as i128 - crate::clock::wall_ns() as i128).max(0) as u64 };
    ps::record(&mut file, ps::kind::HELLO, &hello.payload());
    let unix_now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
    ps::record(&mut file, ps::kind::CLOCK, &ps::clock_payload(ps::mono_ns(), unix_now));
    if let Some(s) = strings.take_new() {
        ps::record(&mut file, ps::kind::STRINGS, &s);
    }
    for (t, sp) in &lanes {
        ps::record(&mut file, ps::kind::THREAD, t);
        ps::record(&mut body, ps::kind::SPANS, sp);
    }
    if !gpu_recs.is_empty() {
        ps::record(&mut file, ps::kind::THREAD, &ps::thread_payload(0xFFFF, 0, 0, ps::role::GPU, gpu_sid));
        ps::record(&mut body, ps::kind::SPANS, &ps::spans_payload(0xFFFF, &gpu_recs));
    }
    file.extend_from_slice(&body);
    let path = std::env::temp_dir().join(format!("relax-inspect-{}-timeline-{}.mkpf", std::process::id(), now_ms()));
    match std::fs::write(&path, &file) {
        Ok(()) => out.line(&format!("{{\"saved\":{},\"records\":{},\"lanes\":{},\"bytes\":{},\"more\":[\"{tool} inspect file={} pf summary\"]}}", json_str(&path.display().to_string()), n_recs, lanes.len(), file.len(), path.display(), tool = crate::tool_name())),
        Err(e) => out.err(&format!("cannot write {}: {}", path.display(), e)),
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_frames_gpu_wait_is_not_ui_busy_and_a_span_counts_in_each_frame_it_crosses() {
        use super::{busy_in, busy_per_range, Seg, Thread};
        let seg = |t0, t1, what: &str| Seg { t0, t1, what: what.to_string(), wait: false, woke_by: None, key: 0, depth: 0, open: false };
        let th = Thread { os_id: 0, handle: 0, segs: vec![seg(0, 10, "beat"), seg(10, 14, "gpu-wait"), seg(14, 20, "submit")], wakes: Vec::new(), idles: Vec::new() };
        assert_eq!(busy_per_range(&th, &[(0, 16), (16, 30)]), vec![(12, 4), (4, 0)]);
        assert_eq!(busy_in(&th, 0, 30), (16, 4));
    }

    /// A hitch's owner_waits come from the UI thread's waits of kind owner (the end record carries
    /// no kind: the begin's names it).
    #[test]
    fn hitch_names_its_owner_waits() {
        let rec = |t: u64, kind: u8, name: &'static str, detail: &'static str| super::FRec { t, kind, name, detail };
        let l = super::FrameLog {
            depth: 0,
            start: 1,
            avg: 0,
            idle: 0,
            idle_t0: 0,
            in_idle: false,
            drew: false,
            recs: vec![rec(1_000, 3, "owner", "main"), rec(3_001_000, 4, "none", ""), rec(3_002_000, 3, "futex", ""), rec(9_002_000, 4, "none", "")],
            dropped: 0,
            compile_depth: 0,
            compile_t0: 0,
            compile_ns: 0,
            compiles: 0,
            probes0: Vec::new(),
            wait_depth: 0,
            wait_t0: 0,
            wait_owner: false,
            wait_ns: 0,
            owner_ns: 0,
            waits: 0,
            wait_in_compile: false,
            cwait_ns: 0,
            cowner_ns: 0,
            turns: super::Turns::default(),
        };
        let s = super::analyse(&l, 0, 10_000_000);
        assert!(s.contains("\"owner_waits\":[{\"wait\":\"owner (main)\",\"at_ms\":0.0,\"ms\":3.00,"), "{s}");
    }

    fn rec(t: u64, kind: super::Kind, label: &'static str, a: u64, detail: &'static str) -> super::Rec {
        let b = if detail.is_empty() { 0 } else { (detail.as_ptr() as u64 & ((1 << 47) - 1)) | ((detail.len() as u64) << 48) };
        super::Rec { t, kind: kind as u8, label_len: label.len() as u16, label: label.as_ptr() as u64, a, b }
    }

    /// A lock wait is ended by the holder's unlock, not by the waiter's own unlock just after it
    /// took the lock; the critical path follows the holder.
    #[test]
    fn a_lock_wait_is_ended_by_the_holder_not_the_waiter() {
        use super::Kind::*;
        const M: u64 = 0x1000;
        let ui = vec![rec(0, SpanBegin, "Actions", 0, ""), rec(10_000_000, WaitBegin, "futex", M, "mutex"), rec(110_000_000, WaitEnd, "", 0, ""), rec(110_020_000, Wake, "", M, ""), rec(120_000_000, SpanEnd, "Actions", 0, "")];
        let holder = vec![rec(0, SpanBegin, "work", 0, ""), rec(109_990_000, Wake, "", M, ""), rec(115_000_000, SpanEnd, "work", 0, "")];
        let ths = super::build(vec![(1, 11, ui), (2, 22, holder)], 120_000_000);
        let w = ths[0].segs.iter().find(|s| s.wait).unwrap();
        assert_eq!(w.woke_by, Some((22, 109_990_000)));
        let cp = super::critical(&ths, 0, 0, 120_000_000);
        assert!(cp.iter().any(|p| p.0 == 1 && p.3 == "run" && p.2 - p.1 > 50_000_000), "{:?}", cp.iter().map(|p| (p.0, p.1, p.2, p.3.clone())).collect::<Vec<_>>());
    }

    /// A waker's history before the wait it ended began is not on the path: the UI posts while
    /// the owner serves another thread's request (woken by that thread's post), and the walk
    /// returns to the UI at its wait's start instead of following the other thread back.
    #[test]
    fn the_walk_returns_to_the_waiter_at_its_wait_start() {
        use super::Kind::*;
        const W: u64 = 0x3000;
        let owner = vec![rec(0, WaitBegin, "idle", W, ""), rec(12_000, WaitEnd, "", 0, ""), rec(12_000, SpanBegin, "serve publish", 0, ""), rec(15_000, Served, "", 22, ""), rec(15_000, SpanEnd, "serve publish", 0, ""),
            rec(15_000, SpanBegin, "serve program", 0, ""), rec(19_000, Served, "", 11, ""), rec(19_000, SpanEnd, "serve program", 0, ""), rec(19_100, WaitBegin, "idle", W, "")];
        let ui = vec![rec(0, SpanBegin, "Actions", 0, ""), rec(13_000, Wake, "", W, ""), rec(13_100, WaitBegin, "owner", 0, "program"), rec(19_500, WaitEnd, "", 0, ""), rec(20_000, SpanEnd, "Actions", 0, "")];
        let other = vec![rec(0, SpanBegin, "work", 0, ""), rec(11_900, Wake, "", W, ""), rec(11_950, SpanEnd, "work", 0, ""), rec(11_950, WaitBegin, "owner", 0, "publish"), rec(15_200, WaitEnd, "", 0, "")];
        let ths = super::build(vec![(3, 33, owner), (1, 11, ui), (2, 22, other)], 20_000);
        let cp = super::critical(&ths, 1, 0, 20_000);
        let steps: Vec<_> = cp.iter().map(|p| (p.0, p.1, p.2, p.3.clone())).collect();
        assert!(!cp.iter().any(|p| p.0 == 2), "{steps:?}");
        // the owner's service from the UI's request back to its wait start, then the UI's own time
        assert!(cp.iter().any(|p| p.0 == 0 && p.1 == 13_100), "{steps:?}");
        assert!(cp.iter().any(|p| p.0 == 1 && p.1 == 0 && p.2 == 13_100), "{steps:?}");
    }

    /// The owner idle is ended by the request of the thread the walk came from (or the first post
    /// after it began), never by a post that came while it was waking: a run monitor's post
    /// right after the UI's is not on the UI's critical path.
    #[test]
    fn the_owner_idle_is_ended_by_the_walks_own_request() {
        use super::Kind::*;
        const W: u64 = 0x2000;
        for monitor_post in [45_000, 38_000] {
            let owner = vec![rec(0, WaitBegin, "idle", W, ""), rec(50_000, WaitEnd, "", 0, ""), rec(60_000, Served, "", 11, ""), rec(68_000, Served, "", 44, ""), rec(68_500, WaitBegin, "idle", W, "")];
            let ui = vec![rec(0, SpanBegin, "Actions", 0, ""), rec(40_000, Wake, "", W, ""), rec(40_500, WaitBegin, "owner", 0, "program"), rec(60_500, WaitEnd, "", 0, ""), rec(70_000, SpanEnd, "Actions", 0, "")];
            let monitor = vec![rec(monitor_post, Wake, "", W, ""), rec(monitor_post + 500, WaitBegin, "owner", 0, "job"), rec(68_500, WaitEnd, "", 0, "")];
            let ths = super::build(vec![(3, 33, owner), (1, 11, ui), (4, 44, monitor)], 70_000);
            let cp = super::critical(&ths, 1, 0, 70_000);
            let steps: Vec<_> = cp.iter().map(|p| (p.0, p.1, p.2, p.3.clone())).collect();
            assert!(!cp.iter().any(|p| p.0 == 2), "monitor post at {monitor_post}: {steps:?}");
            assert!(cp.iter().any(|p| p.0 == 0 && p.3 == "run"), "{steps:?}");
        }
    }

    /// The UI thread of an idle app with a timer, as makepad's event loop records it: each wake
    /// of the timer is a turn (idle wait, the Timer event, platform ops twice, the tasks, the
    /// submit, idle again) and draws nothing. Such turns are no frames: the capture of an idle
    /// app page counted each of their root spans as one (80 "frames" in 8 s and 0 draws).
    fn timer_turn(v: &mut Vec<super::Rec>, t: u64) {
        use super::Kind::*;
        v.push(rec(t, IdleEnd, "idle", 0, ""));
        for (i, name) in ["Timer", "platform-ops", "platform-ops", "tasks", "submit"].into_iter().enumerate() {
            let at = t + 10_000 + i as u64 * 100_000;
            v.push(rec(at, SpanBegin, name, 0, ""));
            v.push(rec(at + 50_000, SpanEnd, name, 0, ""));
        }
        v.push(rec(t + 600_000, IdleBegin, "idle", 0, ""));
    }

    #[test]
    fn an_idle_apps_timer_wakes_are_beats_not_frames() {
        let mut ui = vec![rec(0, super::Kind::IdleBegin, "idle", 0, "")];
        for k in 0..16u64 {
            timer_turn(&mut ui, 500_000_000 * (k + 1));
        }
        let ths = super::build(vec![(1, 11, ui)], 8_600_000_000);
        let fr = super::frames_of(&ths[0]);
        assert!(fr.is_empty(), "{fr:?}");
        let beats = super::beats_of(&ths[0], &fr);
        assert_eq!(beats.len(), 16);
        assert!(beats.iter().all(|b| b.woke == "Timer" && b.phases == ["Timer", "platform-ops", "platform-ops", "tasks", "submit"]));
        let line = super::beats_line(&ths[0], &beats, 8_000_000_000);
        assert!(line.starts_with("{\"event_only_beats\":{\"n\":16,\"per_s\":2.0,\"woken_by\":[[\"Timer\",16]],\"phases\":[[\"platform-ops\",32],[\"Timer\",16],[\"submit\",16],[\"tasks\",16]],\"ui_busy_ms\":{\"median\":0.25,"), "{line}");
    }

    /// A turn that draws is a frame from its first root span to its Draw; its work after the Draw
    /// (the submit) is in no beat; the timer's turns before it stay beats.
    #[test]
    fn a_drawing_turn_is_a_frame_and_the_timer_turns_around_it_beats() {
        use super::Kind::*;
        let mut ui = vec![rec(0, IdleBegin, "idle", 0, "")];
        timer_turn(&mut ui, 500_000_000);
        let t = 1_000_000_000;
        ui.extend([
            rec(t, IdleEnd, "idle", 0, ""),
            rec(t + 10_000, SpanBegin, "Timer", 0, ""),
            rec(t + 60_000, SpanEnd, "Timer", 0, ""),
            rec(t + 100_000, SpanBegin, "beat", 0, ""),
            rec(t + 110_000, SpanBegin, "Draw", 0, ""),
            rec(t + 4_000_000, SpanEnd, "Draw", 0, ""),
            rec(t + 4_100_000, SpanEnd, "beat", 0, ""),
            rec(t + 4_200_000, SpanBegin, "submit", 0, ""),
            rec(t + 4_300_000, SpanEnd, "submit", 0, ""),
            rec(t + 4_400_000, IdleBegin, "idle", 0, ""),
        ]);
        timer_turn(&mut ui, 1_500_000_000);
        let ths = super::build(vec![(1, 11, ui)], 2_000_000_000);
        let fr = super::frames_of(&ths[0]);
        assert_eq!(fr, vec![(t + 10_000, t + 4_000_000)]);
        let beats = super::beats_of(&ths[0], &fr);
        assert_eq!(beats.iter().map(|b| (b.t0, b.woke.as_str())).collect::<Vec<_>>(), vec![(500_010_000, "Timer"), (1_500_010_000, "Timer")]);
    }

    /// The always-on frame watcher counts the same: a turn that ran and drew nothing is a quiet
    /// wake, named by its first phase, unless a frame takes it in; a turn that drew is a frame
    /// (`why-slow`'s UI line).
    #[test]
    fn the_frame_watcher_counts_quiet_wakes_by_what_woke_them() {
        super::set_frame_watch(true);
        let wakes = std::thread::spawn(|| {
            let turn = |drew: bool| {
                super::frame_idle(false);
                super::frame_watch(true, "Timer");
                super::frame_watch(false, "Timer");
                if drew {
                    super::frame_watch(true, "Draw");
                    super::frame_watch(false, "Draw");
                }
                super::frame_watch(true, "tasks");
                super::frame_watch(false, "tasks");
                super::frame_idle(true);
            };
            // two wakes before the mark, one after it: the Draw right after takes in all three,
            // the window's own one off its count; the wake after the Draw stands
            turn(false);
            turn(false);
            let mark = super::ui_wakes_mark();
            for drew in [false, true, false] {
                turn(drew);
            }
            super::ui_wakes_since(mark)
        })
        .join()
        .unwrap();
        super::set_frame_watch(false);
        assert_eq!(wakes, Some((1, 1, vec![("Timer", 1)])));
    }

    #[test]
    fn a_draw_takes_in_the_wakes_up_to_it_not_those_before_a_gap() {
        use super::TurnEnd::*;
        let mut t = super::Turns::default();
        let mut turn = |t0: u64, names: &[&'static str]| {
            for (i, n) in names.iter().enumerate() {
                let at = t0 + i as u64 * 100_000;
                t.root_begin(at, n);
                if *n == "Draw" {
                    t.draw();
                }
                t.root_end(at + 50_000);
            }
            t.end()
        };
        let ms = 1_000_000;
        // an idle app's timer: 500 ms apart, each wake stands
        assert_eq!(turn(0, &["Timer", "tasks"]), Quiet("Timer"));
        assert_eq!(turn(500 * ms, &["Timer", "tasks"]), Quiet("Timer"));
        // 60 Hz: the wakes 5 ms before the Draw are its frame's (the first stood: 500 ms gap)
        assert_eq!(turn(1000 * ms, &["platform-ops"]), Quiet("platform-ops"));
        assert_eq!(turn(1005 * ms, &["platform-ops"]), Quiet("platform-ops"));
        assert_eq!(turn(1010 * ms, &["platform-ops", "Draw", "submit"]), Drew(2));
        // a wake 40 ms before a Draw stands (the gap is over FRAME_GAP_NS)
        assert_eq!(turn(1020 * ms, &["Timer"]), Quiet("Timer"));
        assert_eq!(turn(1060 * ms, &["Draw"]), Drew(0));
        // a turn with no phase is no wake
        assert_eq!(t.end(), Idle);
    }

}

/// What this module holds of a run, as a fresh process has it (`run_end`). Nothing of the run is left that could see it.
pub(crate) unsafe fn reset_statics() {
    use relax_session::reset_static as reset;
    // the rings of the threads that stay go on with them; no one reads these rings outside a capture
    RINGS.lock().unwrap_or_else(|e| e.into_inner()).retain(|r| {
        let ended = r.ended.load(Relaxed);
        if ended {
            drop(Box::from_raw(*r as *const Ring as *mut Ring));
        }
        !ended
    });
    reset(&GPU, Mutex::new(Vec::new()));
}
