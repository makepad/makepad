//! The one monotonic clock the audio engine and the Score read (seconds,
//! arbitrary origin), portable where `std::time::Instant` is not: on the
//! web (wasm32) std has no clock, so it asks the page's `js_monotonic_now`,
//! the import Makepad's web runtime gives every thread (the audio
//! worklet's too), as `makepad_platform::monotonic_seconds` does.

#[cfg(not(target_arch = "wasm32"))]
pub fn now() -> f64 {
    use std::sync::OnceLock;
    use std::time::Instant;
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f64()
}

#[cfg(target_arch = "wasm32")]
pub fn now() -> f64 {
    #[link(wasm_import_module = "env")]
    extern "C" {
        fn js_monotonic_now() -> f64;
    }
    // SAFETY: an import every Makepad web thread provides; it takes nothing
    // and returns a number.
    unsafe { js_monotonic_now() }
}
