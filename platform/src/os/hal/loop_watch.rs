//! Records the loop's waits so an inspector need not wake the UI thread to explain them.

use crate::thread::SignalToUI;
use makepad_remote::json_str;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering::Relaxed};
use std::sync::OnceLock;
use std::time::Instant;

static BASE: OnceLock<Instant> = OnceLock::new();
static IN_PUMP_SINCE: AtomicU64 = AtomicU64::new(0);
static OWED_SINCE: AtomicU64 = AtomicU64::new(0);
static ASKS: AtomicU64 = AtomicU64::new(0);
static BEAT_PENDING: AtomicBool = AtomicBool::new(false);
static LAST_READY: AtomicU64 = AtomicU64::new(0);
static IN_FLIGHT: AtomicUsize = AtomicUsize::new(0);
static MAX: AtomicUsize = AtomicUsize::new(0);
static OLDEST: AtomicU64 = AtomicU64::new(0);

fn stamp(at: Instant) -> u64 {
    // Zero means absent, including at the clock's first instant.
    at.saturating_duration_since(*BASE.get_or_init(Instant::now)).as_nanos() as u64 + 1
}

pub(super) fn in_pump(waiting: bool) {
    IN_PUMP_SINCE.store(if waiting { stamp(Instant::now()) } else { 0 }, Relaxed);
}

pub(super) fn requested(at: Instant) {
    if OWED_SINCE.load(Relaxed) == 0 {
        OWED_SINCE.store(stamp(at), Relaxed);
    }
    ASKS.fetch_add(1, Relaxed);
}

fn clear_owed() {
    OWED_SINCE.store(0, Relaxed);
    ASKS.store(0, Relaxed);
}

pub(super) fn ready() {
    clear_owed();
    LAST_READY.store(stamp(Instant::now()), Relaxed);
}

pub(super) fn publish(has_work: bool, beat_pending: bool, in_flight: usize, oldest: Option<Instant>, max: usize) {
    BASE.get_or_init(Instant::now);
    if !has_work { clear_owed(); }
    BEAT_PENDING.store(beat_pending, Relaxed);
    IN_FLIGHT.store(in_flight, Relaxed);
    OLDEST.store(oldest.map_or(0, stamp), Relaxed);
    MAX.store(max, Relaxed);
}

pub fn report() -> String {
    let now = stamp(Instant::now());
    let age = |at: &AtomicU64| {
        let at = at.load(Relaxed);
        (at != 0).then(|| now.saturating_sub(at) / 1_000_000)
    };
    let pump = age(&IN_PUMP_SINCE);
    let owed = age(&OWED_SINCE);
    let asks = ASKS.load(Relaxed);
    let beat = BEAT_PENDING.load(Relaxed);
    let last_ready = age(&LAST_READY);
    let in_flight = IN_FLIGHT.load(Relaxed);
    let max = MAX.load(Relaxed);
    let oldest = age(&OLDEST);
    let signals = SignalToUI::pending_signal_ages_ms();
    let pending = signals.iter().position(|age| age.is_some_and(|ms| ms >= 1000));
    let (verdict, kind, why) = if let Some(i) = pending.filter(|_| pump.is_some_and(|ms| ms >= 1000)) {
        ("STUCK", "undelivered signal", format!(
            "UI idle with a finished job not delivered: the {} signal was raised {} ms ago and the event loop has slept through it for {} ms (its wake did not reach the pump)",
            ["ui", "internal", "action"][i], signals[i].unwrap(), pump.unwrap()))
    } else if let Some(ms) = owed.filter(|ms| *ms >= 1000 && !beat) {
        ("STUCK", "frame owed", format!(
            "UI idle with a frame owed: it asked the window system for a frame {ms} ms ago ({asks} asks) and no FrameReady came"))
    } else if let Some(ms) = oldest.filter(|ms| *ms >= 1000 && beat && in_flight >= max) {
        ("WAITING", "gpu", format!(
            "a frame is ready but the GPU has not finished the last {in_flight} frames (oldest {ms} ms)"))
    } else {
        ("OK", "", String::new())
    };
    let number = |ms: Option<u64>| ms.map(|n| n.to_string()).unwrap_or_else(|| "null".into());
    format!(
        "{{\"verdict\":{},\"kind\":{},\"why\":{},\"main\":{{\"in_pump_ms\":{}}},\"frame\":{{\"owed_ms\":{},\"asks\":{},\"beat_pending\":{},\"last_ready_ms_ago\":{}}},\"gpu\":{{\"in_flight\":{},\"max\":{},\"oldest_ms\":{}}},\"signals\":{{\"ui_ms\":{},\"internal_ms\":{},\"action_ms\":{}}}}}",
        json_str(verdict), json_str(kind), json_str(&why), number(pump), number(owed), asks, beat,
        number(last_ready), in_flight, max, number(oldest), number(signals[0]), number(signals[1]), number(signals[2]))
}
