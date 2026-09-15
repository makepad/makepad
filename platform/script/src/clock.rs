#[cfg(not(target_arch = "wasm32"))]
// This native shim implements the script VM's portable monotonic clock.
#[allow(clippy::disallowed_types, clippy::disallowed_methods)]
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
