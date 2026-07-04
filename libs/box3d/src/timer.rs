// Port of box3d/src/timer.c (timing portion; b3Hash lives in core.rs).
// The C version uses platform tick counters; the port uses std::time::Instant
// anchored at first use. Ticks are nanoseconds since the anchor.

use std::sync::OnceLock;
use std::time::Instant;

fn anchor() -> &'static Instant {
    static ANCHOR: OnceLock<Instant> = OnceLock::new();
    ANCHOR.get_or_init(Instant::now)
}

/// Get the absolute number of system ticks. The value is platform specific.
pub fn get_ticks() -> u64 {
    anchor().elapsed().as_nanos() as u64
}

/// Get the milliseconds passed from an initial tick value.
pub fn get_milliseconds(ticks: u64) -> f32 {
    let now = get_ticks();
    (now - ticks) as f32 / 1.0e6
}

/// Get the milliseconds passed from an initial tick value. Resets the tick value.
pub fn get_milliseconds_and_reset(ticks: &mut u64) -> f32 {
    let now = get_ticks();
    let ms = (now - *ticks) as f32 / 1.0e6;
    *ticks = now;
    ms
}

/// Yield to be used in a busy loop.
pub fn yield_thread() {
    std::thread::yield_now();
}

/// Sleep the current thread for a number of milliseconds.
pub fn sleep(milliseconds: i32) {
    if milliseconds > 0 {
        std::thread::sleep(std::time::Duration::from_millis(milliseconds as u64));
    }
}
