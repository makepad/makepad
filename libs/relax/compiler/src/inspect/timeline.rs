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
//! Analysis per frame (a frame = the UI thread's root spans between two `present`s, or each root
//! span when nothing presents): busy and waiting time per thread, the critical path (from the
//! frame's end back through each wait to the thread that ended it), total work vs critical path
//! vs wall, and the waits on the path ranked. `timeline save` writes Chrome trace-event JSON
//! (Perfetto / chrome://tracing) with flow arrows for the edges.

use super::{json_str, now_ms, short, Out, Req};
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering::{Acquire, Relaxed, Release}};
use relax_session::sync::Mutex;

/// The markers' flag (one relaxed load at each marker site; makepad and Relax std read it
/// through the loader symbols `relax_recording` / `__relax_recording`): on while the timeline
/// records or a watcher wants frame spikes.
#[no_mangle]
pub static MAKEPAD_RUN_RECORDING: AtomicBool = AtomicBool::new(false);
/// the timeline's rings record
static RINGS_ON: AtomicBool = AtomicBool::new(false);
/// frame spikes are watched (`watch`, `events follow=1`, `--agent`)
static FRAME_WATCH: AtomicBool = AtomicBool::new(false);

/// frames slower than this are `hitch` events (ns; 0: off): always on, MAKEPAD_RUN_HITCH_MS
/// (default 50) or `hitch ms=<n>|off`
static HITCH_NS: AtomicU64 = AtomicU64::new(0);

fn flag_update() {
    MAKEPAD_RUN_RECORDING.store(RINGS_ON.load(Relaxed) || FRAME_WATCH.load(Relaxed) || HITCH_NS.load(Relaxed) > 0, Release);
    // the wait hook: the timeline's rings and the UI thread's frame log both want waits
    let f: fn(bool, &'static str, &'static str, u64) = wait_trace;
    crate::waits::TRACE.store(if RINGS_ON.load(Relaxed) || HITCH_NS.load(Relaxed) > 0 { f as usize } else { 0 }, Release);
}

/// Hitch detection on at inspect's start (MAKEPAD_RUN_HITCH_MS, default 50; 0: off).
pub fn hitch_init() {
    let ms = std::env::var("MAKEPAD_RUN_HITCH_MS").ok().and_then(|v| v.parse::<u64>().ok()).unwrap_or(50);
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
}

// a ring is written by its own thread only and read after the capture stopped
unsafe impl Sync for Ring {}

static RINGS: Mutex<Vec<&'static Ring>> = Mutex::new(Vec::new());
static EPOCH: AtomicU64 = AtomicU64::new(0);
static STARTED_NS: AtomicU64 = AtomicU64::new(0);
static STOPPED_NS: AtomicU64 = AtomicU64::new(0);

thread_local! {
    static MY: Cell<*const Ring> = const { Cell::new(std::ptr::null()) };
}

fn ring() -> Option<&'static Ring> {
    let p = MY.try_with(|m| m.get()).ok()?;
    if !p.is_null() {
        return Some(unsafe { &*p });
    }
    // a ring outlives every compiler generation (the owner records on its own thread too)
    let _k = crate::zone_alloc::kept();
    let r: &'static Ring = Box::leak(Box::new(Ring {
        recs: (0..RING).map(|_| std::cell::UnsafeCell::new(Rec { t: 0, kind: 0, label_len: 0, label: 0, a: 0, b: 0 })).collect(),
        next: AtomicUsize::new(0),
        os_id: super::threads::self_os_id(),
        handle: crate::waits::me(),
        epoch: AtomicU64::new(EPOCH.load(Relaxed)),
    }));
    RINGS.lock().unwrap_or_else(|e| e.into_inner()).push(r);
    let _ = MY.try_with(|m| m.set(r));
    Some(r)
}

#[inline]
pub fn recording() -> bool {
    RINGS_ON.load(Relaxed)
}

/// Appends a record to this thread's ring (callers check `recording()` first).
pub fn push(kind: Kind, label: &'static str, a: u64, b: u64) {
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

// ------------------------------------------------------------------ marker entries

/// makepad's UI phases (platform thread/ui_hang.rs, found by dlsym): a phase begins / ends;
/// `name` is a static string of the program.
pub extern "C" fn relax_span_begin(name: *const u8, len: usize) {
    let name = unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(name, len)) };
    if recording() {
        push(Kind::SpanBegin, name, 0, 0);
    }
    if FRAME_WATCH.load(Relaxed) || HITCH_NS.load(Relaxed) > 0 {
        frame_watch(true, name);
    }
}

pub extern "C" fn relax_span_end(name: *const u8, len: usize) {
    let name = unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(name, len)) };
    if recording() {
        push(Kind::SpanEnd, name, 0, 0);
    }
    if FRAME_WATCH.load(Relaxed) || HITCH_NS.load(Relaxed) > 0 {
        frame_watch(false, name);
    }
}

// ---- frames: spikes and hitches (always on; per UI thread, no lock)
//
// A frame runs from the first phase that begins after the previous frame's Draw ended to the end
// of this frame's Draw; gaps over GAP_NS between its root phases (the UI thread idle in the
// event loop) do not count. Each frame's phases, the UI thread's waits and its lazy compiles go
// to a fixed log (cleared per frame): a frame over the hitch threshold is analysed from it after
// it ends and raised as a `hitch` event (at most HITCH_PER_S a second; the rest are counted).

const GAP_NS: u64 = 16_000_000;
const FRAME_RECS: usize = 8192;
const HITCH_PER_S: u64 = 4;

#[derive(Clone, Copy)]
struct FRec {
    t: u64,
    /// 1 phase begin, 2 phase end, 3 wait begin, 4 wait end, 5 compile begin, 6 compile end
    kind: u8,
    name: &'static str,
    detail: &'static str,
}

struct FrameLog {
    depth: u32,
    start: u64,
    /// running average of frame time (ns)
    avg: u64,
    last_root_end: u64,
    idle: u64,
    recs: Vec<FRec>,
    dropped: u32,
    compile_depth: u32,
    compile_t0: u64,
    compile_ns: u64,
    compiles: u32,
    /// probe / count values at the frame's start (only while probes are installed)
    probes0: Vec<(String, u64, u64)>,
}

thread_local! {
    static FL: std::cell::RefCell<Option<Box<FrameLog>>> = const { std::cell::RefCell::new(None) };
}

static HITCHES: AtomicU64 = AtomicU64::new(0);
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
    let done = FL.try_with(|c| {
        let Ok(mut g) = c.try_borrow_mut() else { return None };
        let l = g.get_or_insert_with(|| {
            Box::new(FrameLog { depth: 0, start: 0, avg: 0, last_root_end: 0, idle: 0, recs: Vec::with_capacity(FRAME_RECS), dropped: 0, compile_depth: 0, compile_t0: 0, compile_ns: 0, compiles: 0, probes0: Vec::new() })
        });
        if begin {
            if l.start == 0 {
                l.start = now;
                l.idle = 0;
                l.recs.clear();
                l.dropped = 0;
                l.compile_ns = 0;
                l.compiles = 0;
                l.probes0.clear();
                if HITCH_NS.load(Relaxed) > 0 && super::instrument::any_counters() {
                    l.probes0 = super::instrument::counters();
                }
            } else if l.depth == 0 && l.last_root_end > l.start && now.saturating_sub(l.last_root_end) > GAP_NS {
                l.idle += now - l.last_root_end;
            }
            l.depth += 1;
            frame_rec(l, 1, name, "");
            return None;
        }
        l.depth = l.depth.saturating_sub(1);
        if l.start != 0 {
            frame_rec(l, 2, name, "");
        }
        if l.depth == 0 {
            l.last_root_end = now;
        }
        if name != "Draw" || l.start == 0 {
            return None;
        }
        let ms = now.saturating_sub(l.start).saturating_sub(l.idle);
        let avg = l.avg;
        l.avg = if avg == 0 { ms } else { (avg * 15 + ms) / 16 };
        let start = l.start;
        l.start = 0;
        if !crate::jit::FIRST_PRESENT.load(Relaxed) {
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
    let mut wait_open: Vec<(String, u64)> = Vec::new();
    // the critical path: (what, ns) pieces in order
    let mut path: Vec<(String, u64)> = Vec::new();
    let mut compile_open = 0u32;
    let mut cur_t = t0;
    let what_now = |stack: &[(&'static str, u64, u64)], wait_open: &[(String, u64)], compile_open: u32| -> String {
        if let Some(w) = wait_open.last() {
            format!("wait: {}", w.0)
        } else if compile_open > 0 {
            "lazy compile".into()
        } else if stack.is_empty() {
            "(between phases)".into()
        } else {
            stack.iter().rev().take(3).rev().map(|x| x.0).collect::<Vec<_>>().join(">")
        }
    };
    let add_path = |path: &mut Vec<(String, u64)>, what: String, ns: u64| {
        if ns == 0 {
            return;
        }
        match path.last_mut() {
            Some(p) if p.0 == what => p.1 += ns,
            _ => path.push((what, ns)),
        }
    };
    for r in &l.recs {
        let t = r.t.max(cur_t);
        add_path(&mut path, what_now(&stack, &wait_open, compile_open), t - cur_t);
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
            3 => wait_open.push((if r.detail.is_empty() { r.name.to_string() } else { format!("{} ({})", r.name, r.detail) }, r.t)),
            4 => {
                if let Some((k, b)) = wait_open.pop() {
                    let dur = r.t.saturating_sub(b);
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
            _ => {}
        }
    }
    add_path(&mut path, what_now(&stack, &wait_open, compile_open), t1.saturating_sub(cur_t));
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
    format!(
        "\"idle_gap_ms\":{:.1},\"phases\":[{}],\"waits\":[{}],\"compile\":{{\"ms\":{:.2},\"n\":{}}},\"time_by\":[{}],\"critical_path\":[{}],\"critical_path_other_ms\":{:.2},\"probe_deltas\":[{}],\"records_dropped\":{}",
        ms(l.idle),
        ph.join(","),
        wt.join(","),
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
        #[cfg(target_os = "macos")]
        let id = 8; // CLOCK_UPTIME_RAW
        #[cfg(not(target_os = "macos"))]
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
    if !recording() || end < begin {
        return;
    }
    let name = unsafe { std::str::from_utf8_unchecked(std::slice::from_raw_parts(name, len)) };
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
    depth: u32,
    /// still open when the capture stopped (cut there)
    open: bool,
}

/// Per thread: its spans and waits as segments, and the wakes it issued (time, futex address).
struct Thread {
    os_id: u64,
    handle: u64,
    segs: Vec<Seg>,
    wakes: Vec<(u64, u64)>,
}

fn build(recs: Vec<(u64, u64, Vec<Rec>)>, stop: u64) -> Vec<Thread> {
    let mut out: Vec<Thread> = Vec::new();
    for (os_id, handle, v) in recs {
        let mut segs = Vec::new();
        let mut stack: Vec<(u64, String, bool, u64, u64)> = Vec::new();
        let mut wakes = Vec::new();
        for r in &v {
            match r.kind {
                1 => stack.push((r.t, label(r).to_string(), false, 0, 0)),
                3 => stack.push((r.t, format!("wait: {} {}", label(r), detail(r.b)).trim_end().to_string(), true, r.a, r.b)),
                2 | 4 => {
                    // the innermost open record of the same kind
                    let want_wait = r.kind == 4;
                    if let Some(k) = stack.iter().rposition(|x| x.2 == want_wait) {
                        let (t0, what, wait, obj, holder) = stack.remove(k);
                        let _ = holder;
                        segs.push(Seg { t0, t1: r.t, what, wait, woke_by: if wait { Some((0, obj)) } else { None }, depth: k as u32, open: false });
                    }
                }
                5 => wakes.push((r.t, r.a)),
                6 => wakes.push((r.t, u64::MAX - r.a)),
                _ => {}
            }
        }
        // still open at the stop: cut there
        for (k, (t0, what, wait, obj, _)) in stack.into_iter().enumerate() {
            segs.push(Seg { t0, t1: stop, what, wait, woke_by: if wait { Some((0, obj)) } else { None }, depth: k as u32, open: true });
        }
        segs.sort_by_key(|s| (s.t0, s.depth));
        out.push(Thread { os_id, handle, segs, wakes });
    }
    // edges: a wait on futex A ending at t is ended by the latest wake of A at or before t; a wait
    // for the owner is ended by the owner's `Served` for this thread
    let all_wakes: Vec<(u64, u64, u64)> = out.iter().flat_map(|th| th.wakes.iter().map(move |w| (w.0, w.1, th.handle))).collect();
    for th in out.iter_mut() {
        let me = th.handle;
        for s in th.segs.iter_mut().filter(|s| s.wait) {
            let obj = s.woke_by.map_or(0, |w| w.1);
            let key = if s.what.starts_with("wait: owner") { u64::MAX - me } else { obj };
            let w = all_wakes.iter().filter(|w| w.1 == key && w.0 <= s.t1 + 50_000 && w.0 + 50_000_000 >= s.t1).max_by_key(|w| w.0);
            s.woke_by = w.map(|w| (w.2, w.0));
        }
    }
    out
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
            let _ = std::thread::Builder::new().name("run-inspect-tl".into()).spawn(move || {
                super::heap::exclude_this_thread();
                std::thread::sleep(std::time::Duration::from_secs(max));
                if EPOCH.load(Relaxed) == e && RINGS_ON.load(Relaxed) {
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
        _ => out.err("usage: timeline start [max_s=30] | stop | frames [n=30] | frame <n|slowest> | save"),
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

/// Frames: windows on the UI thread. A makepad frame ends when its `Draw` dispatches end (the
/// frame runs from the previous one's end: NextFrame, input, draw; Draw dispatches less than 1 ms
/// apart belong to one frame); with no Draw dispatches, each root span is one. Frames still open
/// at the stop are left out.
fn frames_of(ui: &Thread) -> Vec<(u64, u64)> {
    let roots: Vec<&Seg> = ui.segs.iter().filter(|s| !s.wait && s.depth == 0).collect();
    // Draw dispatches at any depth (a native event phase may wrap them)
    let draws: Vec<&Seg> = ui.segs.iter().filter(|s| !s.wait && s.what == "Draw" && !s.open).collect();
    if draws.len() >= 2 {
        // back-to-back Draw dispatches (one per window or pass) close one frame
        let mut ends: Vec<u64> = Vec::new();
        for d in &draws {
            match ends.last_mut() {
                Some(e) if d.t0 < *e + 1_000_000 => *e = d.t1,
                _ => ends.push(d.t1),
            }
        }
        let mut v = Vec::new();
        let first = roots.first().map_or(draws[0].t0, |r| r.t0.min(draws[0].t0));
        v.push((first, ends[0]));
        for w in ends.windows(2) {
            v.push((w[0], w[1]));
        }
        v
    } else {
        roots.iter().filter(|s| !s.open).map(|s| (s.t0, s.t1)).collect()
    }
}

/// Busy time of a thread in [t0, t1): time inside its non-wait root spans minus the waits inside
/// them (threads without spans of their own count their wait-free time between waits as unknown).
fn busy_in(th: &Thread, t0: u64, t1: u64) -> (u64, u64) {
    let clip = |s: &Seg| s.t1.min(t1).saturating_sub(s.t0.max(t0));
    let spans: u64 = th.segs.iter().filter(|s| !s.wait && s.depth == 0).map(clip).sum();
    let waits: u64 = th.segs.iter().filter(|s| s.wait).map(clip).sum();
    (spans.saturating_sub(waits), waits)
}

/// The critical path of [t0, t1) ending on thread `ui`: walking back from t1, the UI thread's
/// own busy time counts; a wait ended by another thread's wake moves the walk to that thread at
/// the wake. Returns the path as (thread index, from, to, what) pieces, newest first.
fn critical(ths: &[Thread], ui: usize, t0: u64, t1: u64) -> Vec<(usize, u64, u64, String)> {
    let mut path = Vec::new();
    let (mut th, mut t) = (ui, t1);
    for _ in 0..256 {
        if t <= t0 {
            break;
        }
        // the latest wait on this thread that overlaps (.., t)
        let w = ths[th].segs.iter().filter(|s| s.wait && s.t0 < t && s.t1 > t0).filter(|s| s.t0 < t).max_by_key(|s| s.t1.min(t));
        match w {
            Some(s) if s.t1 >= t.min(s.t1) => {
                let end = s.t1.min(t);
                if end < t {
                    path.push((th, end, t, "run".to_string()));
                }
                match s.woke_by.and_then(|(h, at)| ths.iter().position(|x| x.handle == h).map(|i| (i, at))) {
                    Some((i, at)) if i != th && at > s.t0 => {
                        path.push((th, at.max(s.t0), end, format!("{} (ended by T{})", s.what, ths[i].os_id)));
                        th = i;
                        t = at.max(t0);
                    }
                    _ => {
                        path.push((th, s.t0.max(t0), end, s.what.clone()));
                        t = s.t0;
                    }
                }
            }
            _ => {
                path.push((th, t0, t, "run".to_string()));
                break;
            }
        }
    }
    path
}

fn frames_cmd(req: &Req, out: &mut Out) {
    let stop = stopped_or_now();
    let ths = build(collect(), stop);
    let Some(ui) = ui_thread(&ths) else {
        out.err("no records: `timeline start`, use the app, `timeline stop` (makepad's UI phases need the platform's makepad run hook)");
        return;
    };
    let fr = frames_of(&ths[ui]);
    out.measured(&format!("{} frames on the UI thread, {} threads recorded", fr.len(), ths.len()), "timeline rings", &[]);
    let durs: Vec<f64> = fr.iter().map(|f| (f.1 - f.0) as f64 / 1e6).collect();
    let mut sorted = durs.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let pct = |p: f64| sorted.get(((sorted.len() as f64 - 1.0) * p).round() as usize).copied().unwrap_or(0.0);
    out.line(&format!("{{\"frame_ms\":{{\"n\":{},\"median\":{:.2},\"p95\":{:.2},\"max\":{:.2}}}}}", durs.len(), pct(0.5), pct(0.95), pct(1.0)));
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
        out.err(&format!("no frame `{}` ({} frames)", which, fr.len()));
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
    let st: Vec<String> = steps.iter().take(16).map(|(th, d, w)| format!("{{\"thread\":\"T{}\",\"ms\":{:.2},\"what\":{}}}", ths[*th].os_id, *d as f64 / 1e6, json_str(&short(w, 120)))).collect();
    out.line(&format!("{{\"critical_path_busy_ms\":{:.2},\"steps\":[{}]}}", total as f64 / 1e6, st.join(",")));
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
        .map(|(d, what, by)| format!("{{\"ms\":{:.2},\"what\":{}{}}}", *d as f64 / 1e6, json_str(&short(what, 100)), by.map(|b| format!(",\"ended_by\":\"T{}\"", b)).unwrap_or_default()))
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
    // per thread busy / wait in the frame
    let mut per: Vec<String> = Vec::new();
    for th in &ths {
        let (b, wt) = busy_in(th, t0, t1);
        if b > 0 {
            per.push(format!("{{\"thread\":\"T{}\",\"busy_ms\":{:.2},\"wait_ms\":{:.2}}}", th.os_id, b as f64 / 1e6, wt as f64 / 1e6));
        }
    }
    out.line(&format!("{{\"threads\":[{}]}}", per.join(",")));
}

/// Chrome trace-event JSON (Perfetto, chrome://tracing, speedscope): complete events per segment,
/// flow events for the edges that ended waits.
fn save_cmd(out: &mut Out) {
    let stop = stopped_or_now();
    let t0 = STARTED_NS.load(Relaxed);
    let ths = build(collect(), stop);
    let mut ev: Vec<String> = Vec::new();
    let pid = std::process::id();
    let mut names = super::threads::list();
    super::name_handles(&mut names);
    let mut flow = 0u64;
    for th in &ths {
        let nm = names.iter().find(|t| t.os_id == th.os_id).map(|t| t.name.clone()).unwrap_or_default();
        ev.push(format!("{{\"ph\":\"M\",\"pid\":{},\"tid\":{},\"name\":\"thread_name\",\"args\":{{\"name\":{}}}}}", pid, th.os_id, json_str(&format!("{} T{}", nm, th.os_id))));
        for s in &th.segs {
            let ts = s.t0.saturating_sub(t0) as f64 / 1000.0;
            let dur = (s.t1 - s.t0) as f64 / 1000.0;
            ev.push(format!("{{\"ph\":\"X\",\"pid\":{},\"tid\":{},\"ts\":{:.3},\"dur\":{:.3},\"name\":{},\"cat\":\"{}\"}}", pid, th.os_id, ts, dur, json_str(&s.what), if s.wait { "wait" } else { "work" }));
            if let Some((h, at)) = s.woke_by {
                if let Some(src) = ths.iter().find(|x| x.handle == h) {
                    flow += 1;
                    ev.push(format!("{{\"ph\":\"s\",\"pid\":{},\"tid\":{},\"ts\":{:.3},\"id\":{},\"name\":\"wake\",\"cat\":\"edge\"}}", pid, src.os_id, at.saturating_sub(t0) as f64 / 1000.0, flow));
                    ev.push(format!("{{\"ph\":\"f\",\"bp\":\"e\",\"pid\":{},\"tid\":{},\"ts\":{:.3},\"id\":{},\"name\":\"wake\",\"cat\":\"edge\"}}", pid, th.os_id, s.t1.saturating_sub(t0) as f64 / 1000.0, flow));
                }
            }
        }
    }
    // the GPU lane as its own track
    let gpu = gpu_spans(0, u64::MAX);
    if !gpu.is_empty() {
        ev.push(format!("{{\"ph\":\"M\",\"pid\":{},\"tid\":1,\"name\":\"thread_name\",\"args\":{{\"name\":\"GPU\"}}}}", pid));
        for (b, e, n) in gpu {
            ev.push(format!("{{\"ph\":\"X\",\"pid\":{},\"tid\":1,\"ts\":{:.3},\"dur\":{:.3},\"name\":{},\"cat\":\"gpu\"}}", pid, b.saturating_sub(t0) as f64 / 1000.0, (e - b) as f64 / 1000.0, json_str(n)));
        }
    }
    let path = std::env::temp_dir().join(format!("makepad-run-inspect-{}-timeline-{}.json", pid, now_ms()));
    match std::fs::write(&path, format!("{{\"traceEvents\":[{}]}}", ev.join(",\n"))) {
        Ok(()) => out.line(&format!("{{\"saved\":{},\"events\":{},\"format\":\"Chrome trace events (open in ui.perfetto.dev or chrome://tracing)\"}}", json_str(&path.display().to_string()), ev.len())),
        Err(e) => out.err(&format!("cannot write {}: {}", path.display(), e)),
    }
}
