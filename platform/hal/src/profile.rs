//! The frame profile of a running app: `makepad-remote app <pid> trace topics=frames` turns it
//! on, `trace off=1` prints it and turns it off; while on, it prints every 5 s to stderr. Each
//! stage of a frame (app CPU, GPU, display hand-off, present) is one metric; every sample is
//! filed under what the user was doing when it was taken (idle, pointer moves, a drag, a scroll),
//! so one recording separates the cases. Off, a sample costs one relaxed load.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use makepad_relax_api::relax_api;

#[relax_api(plain)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Metric {
    /// Interval between the app's frame beats (FrameReady to FrameReady).
    Beat,
    /// App CPU: input/event intake between two beats.
    CpuInput,
    /// App CPU: next-frame callbacks + the draw event (layout and draw are one walk).
    CpuDraw,
    /// App CPU: building the frame packet (passes into commands, shader compiles).
    CpuBuild,
    /// The app's submit of a window frame until the HAL saw its GPU work done.
    SubmitToRendered,
    /// A rendered frame waiting for the display worker to take it.
    DisplayQueue,
    /// Cross-GPU transfer, source side: copy into the shared buffer until it is done.
    XgpuSource,
    /// Cross-GPU transfer, display side: copy out of the shared buffer into the scanout image.
    XgpuDestination,
    /// Display copy + present submit: frame taken until queue_present.
    ToPresent,
    /// queue_present until the present completed (the vblank wait).
    PresentWait,
    /// Interval between completed presents of an output on the rendering GPU.
    PresentInterval,
    /// Interval between completed presents of an output on another GPU (a cross-GPU copy).
    PeerPresentInterval,
    /// Frame credit (FrameReady) until that frame completed its present.
    CreditToGlass,
    /// Interval between new frames of a hosted app, as the host composites them.
    ChildFrame,
}

const METRICS: [Metric; 14] = [
    Metric::Beat, Metric::CpuInput, Metric::CpuDraw, Metric::CpuBuild, Metric::SubmitToRendered,
    Metric::DisplayQueue, Metric::XgpuSource, Metric::XgpuDestination, Metric::ToPresent,
    Metric::PresentWait, Metric::PresentInterval, Metric::PeerPresentInterval, Metric::CreditToGlass, Metric::ChildFrame,
];

fn name(metric: Metric) -> &'static str {
    match metric {
        Metric::Beat => "beat interval",
        Metric::CpuInput => "cpu input",
        Metric::CpuDraw => "cpu draw",
        Metric::CpuBuild => "cpu build",
        Metric::SubmitToRendered => "submit->rendered",
        Metric::DisplayQueue => "display queue",
        Metric::XgpuSource => "xgpu source copy",
        Metric::XgpuDestination => "xgpu dest copy",
        Metric::ToPresent => "taken->present",
        Metric::PresentWait => "present wait",
        Metric::PresentInterval => "present interval",
        Metric::PeerPresentInterval => "xgpu present iv",
        Metric::CreditToGlass => "credit->glass",
        Metric::ChildFrame => "child frame iv",
    }
}

/// What the user was doing when a sample was taken. Fieldless: `input` is an op.
#[relax_api(plain)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Input {
    Move,
    ButtonDown,
    ButtonUp,
    Scroll,
}

const ACTIVITIES: [&str; 4] = ["idle", "cursor", "drag", "scroll"];
/// An input this recent classifies a sample.
const RECENT_NS: u64 = 150_000_000;

static ON: AtomicBool = AtomicBool::new(false);
static LAST_MOVE: AtomicU64 = AtomicU64::new(0);
static LAST_SCROLL: AtomicU64 = AtomicU64::new(0);
static HELD: AtomicBool = AtomicBool::new(false);
static CREDIT: AtomicU64 = AtomicU64::new(0);
static SUBMIT: AtomicU64 = AtomicU64::new(0);

struct Samples {
    started: u64,
    printed: u64,
    /// microseconds, per activity per metric
    data: Vec<Vec<Vec<u32>>>,
    /// a hosted app's last new frame, by the host's key for it
    children: std::collections::HashMap<usize, u64>,
    /// named event counts (e.g. why an app beat without drawing)
    counts: std::collections::BTreeMap<&'static str, u64>,
    /// each window of the app: its last frame, and the intervals between its frames (us) per
    /// activity
    windows: std::collections::BTreeMap<usize, (u64, Vec<Vec<u32>>)>,
}
static SAMPLES: Mutex<Option<Samples>> = Mutex::new(None);

fn lock() -> std::sync::MutexGuard<'static, Option<Samples>> {
    SAMPLES.lock().unwrap_or_else(|e| e.into_inner())
}

// The profile state lives in this (provided) crate: JIT code reaches it only through these ops.
#[relax_api]
pub fn on() -> bool {
    ON.load(Ordering::Relaxed)
}

/// Turn the profile on (a fresh recording) or off (prints the recording).
/// An op like the others: JIT code (the remote's `trace` command) calls the
/// provided crate's own, never a JIT copy with its own statics.
#[relax_api]
pub fn set(enable: bool) {
    if enable == on() {
        return;
    }
    crate::threadcpu::snapshot();
    if enable {
        let now = crate::clock_ns();
        *lock() = Some(Samples { started: now, printed: now, data: vec![vec![Vec::new(); METRICS.len()]; ACTIVITIES.len()], children: Default::default(), counts: Default::default(), windows: Default::default() });
        ON.store(true, Ordering::Release);
    } else {
        ON.store(false, Ordering::Release);
        if let Some(samples) = lock().take() {
            eprintln!("{}", report(&samples));
        }
    }
}

#[relax_api]
pub fn input(input: Input) {
    if !on() {
        return;
    }
    let now = crate::clock_ns();
    match input {
        Input::Move => LAST_MOVE.store(now, Ordering::Relaxed),
        Input::Scroll => LAST_SCROLL.store(now, Ordering::Relaxed),
        Input::ButtonDown => HELD.store(true, Ordering::Relaxed),
        Input::ButtonUp => HELD.store(false, Ordering::Relaxed),
    }
}

fn activity(now: u64) -> usize {
    if now.saturating_sub(LAST_SCROLL.load(Ordering::Relaxed)) < RECENT_NS {
        3
    } else if now.saturating_sub(LAST_MOVE.load(Ordering::Relaxed)) < RECENT_NS {
        if HELD.load(Ordering::Relaxed) { 2 } else { 1 }
    } else {
        0
    }
}

/// One sample of `metric`, `ns` long.
#[relax_api]
pub fn record(metric: Metric, ns: u64) {
    if !on() {
        return;
    }
    let now = crate::clock_ns();
    let mut guard = lock();
    let Some(samples) = guard.as_mut() else { return };
    let i = METRICS.iter().position(|m| *m == metric).unwrap();
    samples.data[activity(now)][i].push((ns / 1000).min(u32::MAX as u64) as u32);
    if now.saturating_sub(samples.printed) > 5_000_000_000 {
        samples.printed = now;
        eprintln!("{}", report(samples));
    }
}

/// `record(metric, now - since)` when `since` is set.
pub fn since(metric: Metric, since: u64) {
    if since != 0 && on() {
        record(metric, crate::clock_ns().saturating_sub(since));
    }
}

/// A hosted app (`key`, the host's) delivered a new frame now.
#[relax_api]
pub fn child_frame(key: usize) {
    if !on() {
        return;
    }
    let now = crate::clock_ns();
    let last = lock().as_mut().and_then(|s| s.children.insert(key, now));
    if let Some(last) = last { record(Metric::ChildFrame, now - last); }
}

/// The app built a frame of its window `window` now: per-window frame intervals, so a second
/// window that is paced by the first (or starved by it) shows on its own line.
#[relax_api]
pub fn window_frame(window: usize) {
    if !on() {
        return;
    }
    let now = crate::clock_ns();
    let a = activity(now);
    let mut guard = lock();
    let Some(samples) = guard.as_mut() else { return };
    let entry = samples.windows.entry(window).or_insert_with(|| (0, vec![Vec::new(); ACTIVITIES.len()]));
    let last = std::mem::replace(&mut entry.0, now);
    if last != 0 {
        entry.1[a].push(((now - last) / 1000).min(u32::MAX as u64) as u32);
    }
}

/// Count one `what` (printed with the report).
#[relax_api]
pub fn count(what: &'static str) {
    if !on() {
        return;
    }
    if let Some(samples) = lock().as_mut() { *samples.counts.entry(what).or_default() += 1; }
}

/// The app was given frame credit (FrameReady) / submitted a window frame now.
#[relax_api]
pub fn credit() { if on() { CREDIT.store(crate::clock_ns(), Ordering::Relaxed); } }
pub fn credit_at() -> u64 { CREDIT.load(Ordering::Relaxed) }
#[relax_api]
pub fn submitted() { if on() { SUBMIT.store(crate::clock_ns(), Ordering::Relaxed); } }
pub fn submitted_at() -> u64 { SUBMIT.swap(0, Ordering::Relaxed) }

fn report(samples: &Samples) -> String {
    use std::fmt::Write;
    let secs = (crate::clock_ns().saturating_sub(samples.started)) as f64 / 1e9;
    let mut out = format!("[frame-profile] {secs:.1} s recorded; ms: n p50 p90 p99 max | present/beat interval histogram");
    if !samples.counts.is_empty() {
        let _ = write!(out, "\n[frame-profile] counts:");
        for (what, n) in &samples.counts { let _ = write!(out, " {what}={n}"); }
    }
    for (a, metrics) in samples.data.iter().enumerate() {
        if metrics.iter().all(|m| m.is_empty()) && samples.windows.values().all(|(_, w)| w[a].is_empty()) {
            continue;
        }
        let _ = write!(out, "\n[frame-profile] -- {}", ACTIVITIES[a]);
        for (i, values) in metrics.iter().enumerate() {
            let interval = matches!(METRICS[i], Metric::PresentInterval | Metric::PeerPresentInterval | Metric::Beat | Metric::ChildFrame);
            line(&mut out, name(METRICS[i]), values, interval);
        }
        for (window, (_, per_activity)) in &samples.windows {
            line(&mut out, &format!("window {window} frame iv"), &per_activity[a], true);
        }
    }
    out
}

/// One metric's line: n p50 p90 p99 max, and for an interval its histogram.
fn line(out: &mut String, label: &str, values: &[u32], interval: bool) {
    use std::fmt::Write;
    if values.is_empty() {
        return;
    }
    let mut v = values.to_vec();
    v.sort_unstable();
    let q = |p: f64| v[((v.len() - 1) as f64 * p).round() as usize] as f64 / 1000.0;
    let _ = write!(out, "\n[frame-profile]   {:<18} {:>6} {:>7.2} {:>7.2} {:>7.2} {:>7.2}", label, v.len(), q(0.5), q(0.9), q(0.99), *v.last().unwrap() as f64 / 1000.0);
    if interval {
        const EDGES: [u32; 7] = [5_000, 9_000, 13_000, 17_500, 25_000, 34_000, 50_000];
        let mut counts = [0usize; 8];
        for x in &v {
            counts[EDGES.iter().position(|e| x < e).unwrap_or(7)] += 1;
        }
        let _ = write!(out, " | <5:{} <9:{} <13:{} <17.5:{} <25:{} <34:{} <50:{} >50:{}",
            counts[0], counts[1], counts[2], counts[3], counts[4], counts[5], counts[6], counts[7]);
    }
}
