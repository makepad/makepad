//! A cross-thread stop for a running script. Another thread sets it, and the interpreter bails
//! within 1024 instructions with "script interrupted": the same uncatchable Bail as an exhausted
//! instruction limit. It is sampled where that limit decrements, so only runs under an
//! instruction limit (`with_instruction_limit`) see it, and runs without one pay nothing.
//!
//! A host that ends a run for its own reasons (the work was cancelled) stops it instead
//! ([`ScriptInterrupt::stop`]): the same uncatchable bail, with no error raised or reported.
//! Nothing went wrong in the script, and an error would be logged as one.
use relax_api::relax_api;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

/// The interrupt a VM samples ([`crate::vm::ScriptVmBase::interrupt_handle`]). Clones share one
/// flag, which stays set until [`ScriptInterrupt::clear`].
#[relax_api(handle, clone, run)]
#[derive(Clone, Default)]
pub struct ScriptInterrupt(Arc<AtomicU8>);

const INTERRUPTED: u8 = 1;
const STOPPED: u8 = 2;

#[relax_api(run)]
impl ScriptInterrupt {
    /// Stops the VM's current run (and every later one under a limit) until cleared.
    #[relax_api(run)]
    pub fn interrupt(&self) {
        self.0.store(INTERRUPTED, Ordering::Relaxed);
    }

    /// [`Self::interrupt`] with no error: the run bails with nil and nothing is reported.
    #[relax_api(run)]
    pub fn stop(&self) {
        self.0.store(STOPPED, Ordering::Relaxed);
    }

    #[relax_api(run)]
    pub fn clear(&self) {
        self.0.store(0, Ordering::Relaxed);
    }

    #[relax_api(run)]
    pub fn is_interrupted(&self) -> bool {
        self.0.load(Ordering::Relaxed) != 0
    }
}

impl ScriptInterrupt {
    /// The interpreter's sample: a plain load, no op.
    #[inline(always)]
    pub(crate) fn is_set(&self) -> bool {
        self.0.load(Ordering::Relaxed) != 0
    }

    pub(crate) fn is_stop(&self) -> bool {
        self.0.load(Ordering::Relaxed) == STOPPED
    }
}
