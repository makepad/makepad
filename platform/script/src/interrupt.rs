//! A cross-thread stop for a running script. Another thread sets it, and the interpreter bails
//! within 1024 instructions with "script interrupted": the same uncatchable Bail as an exhausted
//! instruction limit. It is sampled where that limit decrements, so only runs under an
//! instruction limit (`with_instruction_limit`) see it, and runs without one pay nothing.
use makepad_relax_api::relax_api;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// The interrupt a VM samples ([`crate::vm::ScriptVmBase::interrupt_handle`]). Clones share one
/// flag, which stays set until [`ScriptInterrupt::clear`].
#[relax_api(handle, clone)]
#[derive(Clone, Default)]
pub struct ScriptInterrupt(Arc<AtomicBool>);

#[relax_api]
impl ScriptInterrupt {
    /// Stops the VM's current run (and every later one under a limit) until cleared.
    #[relax_api]
    pub fn interrupt(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    #[relax_api]
    pub fn clear(&self) {
        self.0.store(false, Ordering::Relaxed);
    }

    #[relax_api]
    pub fn is_interrupted(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

impl ScriptInterrupt {
    /// The interpreter's sample: a plain load, no op.
    #[inline(always)]
    pub(crate) fn is_set(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}
