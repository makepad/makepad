// Port of box3d/src/timer.c (timing portion; b3Hash lives in core.rs).
// The C version uses platform tick counters; the port uses std::time::Instant
// anchored at first use. Ticks are nanoseconds since the anchor.

/// Get the absolute number of system ticks. The value is platform specific.
pub fn get_ticks() -> u64 {
    (makepad_platform::Cx::monotonic_now() * 1_000_000_000.0) as u64
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
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::yield_now();
    #[cfg(target_arch = "wasm32")]
    std::hint::spin_loop();
}

/// Sleep the current thread for a number of milliseconds.
pub fn sleep(milliseconds: i32) {
    if milliseconds > 0 {
        #[cfg(not(target_arch = "wasm32"))]
        std::thread::sleep(std::time::Duration::from_millis(milliseconds as u64));
        #[cfg(target_arch = "wasm32")]
        let _ = milliseconds;
    }
}
