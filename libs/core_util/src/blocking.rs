//! A blocking wait that holds no CPU.
//!
//! A host whose threads run under a CPU budget (a pool that admits jobs by permit) installs one
//! function here; a std-only crate's wait for another thread's work goes through
//! [`wait_off_cpu`], so the host can release the waiter's CPU while it waits and take it back
//! after. Without a host (tools, tests) the wait runs as it is.

use std::sync::OnceLock;

/// The host's wait: it runs `wait` (which blocks until another thread's work is done) and may
/// release the caller's CPU around it. `what` names what is waited for (a type name), for the
/// host's diagnostics.
pub type Hook = fn(wait: &mut dyn FnMut(), what: &'static str);

static HOOK: OnceLock<Hook> = OnceLock::new();

/// Installs the host's wait. Returns true for the first installation in the process; later ones
/// are ignored.
pub fn install(hook: Hook) -> bool {
    HOOK.set(hook).is_ok()
}

/// Runs `wait`, a blocking wait for another thread's work, through the host's hook when one is
/// installed. Hold no lock across it that the awaited work needs. One atomic load besides the
/// wait.
pub fn wait_off_cpu(what: &'static str, wait: impl FnOnce()) {
    let mut wait = Some(wait);
    let mut run = || {
        if let Some(wait) = wait.take() {
            wait();
        }
    };
    match HOOK.get() {
        Some(hook) => hook(&mut run, what),
        None => run(),
    }
}
