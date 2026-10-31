//! `MAKEPAD_HAL_MAIN_LAG=1`: how long the process's main thread (the window system's thread)
//! stays away from its loop. Each shell ticks the meter from its loop at least every 4 ms (macOS:
//! a run-loop timer in the common modes; Linux: a wake of the loop's own fd); the gap between two
//! ticks is how long the window system waited for this process. A gap over 50 ms is printed when
//! it ends, the summary when the main loop does: `makepad-hal: main-lag ticks=.. max_ms=..
//! p99_ms=.. stalls=..`.
#![cfg_attr(not(any(target_os = "macos", target_os = "linux")), allow(dead_code))]
use std::cell::RefCell;
use std::time::{Duration, Instant};

pub const TICK: Duration = Duration::from_millis(4);
const STALL_MS: usize = 50;
/// gaps in whole ms; the last bucket holds every longer one
const BUCKETS: usize = 1001;

struct Meter {
    start: Instant,
    last: Instant,
    ticks: u64,
    max_ms: f64,
    stalls: u64,
    histogram: Vec<u64>,
}

thread_local! {
    static METER: RefCell<Option<Meter>> = const { RefCell::new(None) };
}

pub fn wanted() -> bool {
    std::env::var_os("MAKEPAD_HAL_MAIN_LAG").is_some()
}

/// Starts the meter on the calling (main) thread.
pub fn start() {
    let now = Instant::now();
    METER.with_borrow_mut(|m| *m = Some(Meter { start: now, last: now, ticks: 0, max_ms: 0.0, stalls: 0, histogram: vec![0; BUCKETS] }));
}

/// The main loop came around (no-op without a meter).
pub fn tick() {
    METER.with_borrow_mut(|meter| {
        let Some(m) = meter else { return };
        let now = Instant::now();
        let ms = (now - m.last).as_secs_f64() * 1e3;
        m.last = now;
        m.ticks += 1;
        m.max_ms = m.max_ms.max(ms);
        let bucket = (ms as usize).min(BUCKETS - 1);
        m.histogram[bucket] += 1;
        if bucket >= STALL_MS {
            m.stalls += 1;
            eprintln!("makepad-hal: main thread away {ms:.1} ms (until {:.2} s)", (now - m.start).as_secs_f64());
        }
    });
}

/// Ends the meter and prints its summary (no-op without a meter).
pub fn finish() {
    let Some(m) = METER.with_borrow_mut(Option::take) else { return };
    let rank = m.ticks.saturating_sub(m.ticks / 100).max(1);
    let mut seen = 0;
    let p99 = m.histogram.iter().position(|n| {
        seen += n;
        seen >= rank
    });
    eprintln!(
        "makepad-hal: main-lag ticks={} max_ms={:.1} p99_ms={} stalls={}",
        m.ticks,
        m.max_ms,
        p99.map_or(0, |b| b + 1),
        m.stalls
    );
}
