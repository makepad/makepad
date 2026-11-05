//! Per-thread heap routing (makepad-alloc-phase) and what reads settings in the kept phase.
pub use makepad_alloc_phase::*;

/// An environment variable read once into `cell`; unset or empty = None. The value lives in the
/// kept heap: it is process state, whichever thread and phase read it first. Inlined: the runtime
/// asks on every JIT entry (jit.rs `profile_path`), and the call across crates cost more than
/// the initialized cell's load.
#[inline]
pub fn env_once(cell: &'static std::sync::OnceLock<String>, name: &str) -> Option<&'static str> {
    let s = cell.get_or_init(|| {
        let _k = kept();
        std::env::var(name).unwrap_or_default()
    });
    (!s.is_empty()).then_some(s.as_str())
}
