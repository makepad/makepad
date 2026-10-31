//! The code a callback the app registered runs in. A host that runs several apps' code in one
//! process (relax: each app a domain with its own statics and heap) has a callback run in the
//! app that registered it, whatever thread the HAL calls it on (a realtime audio thread, a
//! driver's worker): the HAL records a [`Scope`] where the app registers the callback (on the
//! app's thread that submitted it) and runs each call through [`Scope::call`]. Without a host
//! scope (a native app) both are free.

use std::sync::OnceLock;

/// The host's hooks (`set_call_scope`).
#[derive(Clone, Copy)]
pub struct CallScope {
    /// On the thread registering a callback: what its calls run in (0: nothing to enter). May lock.
    pub capture: fn() -> u64,
    /// On the thread about to call a callback of `capture`'s value: enters it and returns what
    /// `leave` puts back. Realtime safe: no lock, no allocation on a steady path.
    pub enter: fn(u64) -> [u64; 3],
    pub leave: fn([u64; 3]),
}

static HOOKS: OnceLock<CallScope> = OnceLock::new();

/// Registers the host's hooks, once per process (a later call is ignored).
pub fn set_call_scope(hooks: CallScope) {
    let _ = HOOKS.set(hooks);
}

/// What a registered callback runs in (`Scope::here` where it was registered).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Scope(u64);

impl Scope {
    /// The scope of the calling thread: call where the app registers the callback, never on a
    /// realtime path.
    pub fn here() -> Scope {
        Scope(HOOKS.get().map_or(0, |h| (h.capture)()))
    }

    /// Runs `f` (a call into the callback) in this scope, on the calling thread.
    #[inline]
    pub fn call<R>(self, f: impl FnOnce() -> R) -> R {
        if self.0 == 0 {
            return f();
        }
        let Some(h) = HOOKS.get() else { return f() };
        struct Leave(fn([u64; 3]), [u64; 3]);
        impl Drop for Leave {
            fn drop(&mut self) {
                (self.0)(self.1)
            }
        }
        let _leave = Leave(h.leave, (h.enter)(self.0));
        f()
    }
}
