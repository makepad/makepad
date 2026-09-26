//! Portable clocks. This crate sits below the platform layer, so it cannot
//! ask `Cx`; on wasm it reads the same host clocks the platform does.

/// Seconds on a monotonic clock with an arbitrary origin.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn monotonic_now() -> f64 {
    use std::sync::OnceLock;
    use std::time::Instant;

    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f64()
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn monotonic_now() -> f64 {
    #[link(wasm_import_module = "env")]
    extern "C" {
        fn js_monotonic_now() -> f64;
    }
    unsafe { js_monotonic_now() }
}

/// Nanoseconds since the Unix epoch.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn unix_nanos() -> u128 {
    use std::time::{SystemTime, UNIX_EPOCH};

    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn unix_nanos() -> u128 {
    #[link(wasm_import_module = "env")]
    extern "C" {
        fn js_time_now() -> f64;
    }
    (unsafe { js_time_now() }.max(0.0) * 1_000_000_000.0) as u128
}

/// Elapsed wall time of one measured phase, in the milliseconds the stats
/// report.
#[derive(Clone, Copy)]
pub(crate) struct Stopwatch(f64);

impl Stopwatch {
    pub(crate) fn start() -> Self {
        Self(monotonic_now())
    }

    pub(crate) fn elapsed_ms(&self) -> f64 {
        (monotonic_now() - self.0) * 1000.0
    }
}
