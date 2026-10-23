//! Per-thread heap routing shared by every compiler layer; the allocator lives in relax-base.
use std::cell::Cell;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicU8, Ordering};

const KEPT: u8 = 0;
const COMPILER: u8 = 1;
const OFF: u8 = 0xff;

thread_local! {
    // const, no destructor: readable in the allocator at any point of a thread's life
    static PHASE: Cell<u8> = const { Cell::new(KEPT) };
}

/// Set by relax-base's allocator decision: 0 = not read yet, 1 = on, 2 = off.
/// Until routing is enabled, guards leave the default kept phase alone.
pub static ROUTING: AtomicU8 = AtomicU8::new(0);

/// Whether this thread's allocations belong to the compiler arena.
#[inline]
pub fn in_compiler() -> bool {
    // `try_with` of a const TLS without a destructor never fails and compiles to one
    // thread-pointer-relative load; `with` stays a call (it carries a panic path)
    PHASE.try_with(|p| p.get()).map_or(false, |p| p == COMPILER)
}

/// Puts the calling thread in a phase until dropped. !Send: it restores this thread's TLS.
pub struct PhaseGuard(u8, PhantomData<*mut ()>);

impl Drop for PhaseGuard {
    #[inline]
    fn drop(&mut self) {
        if self.0 != OFF {
            PHASE.with(|p| p.set(self.0));
        }
    }
}

#[inline]
fn enter(phase: u8) -> PhaseGuard {
    if ROUTING.load(Ordering::Relaxed) != 1 {
        return PhaseGuard(OFF, PhantomData);
    }
    let prev = PHASE.with(|p| p.replace(phase));
    PhaseGuard(prev, PhantomData)
}

/// Allocations go to the compiler arena until the guard drops, once routing is enabled.
#[inline]
pub fn compiler() -> PhaseGuard {
    enter(COMPILER)
}

/// Allocations go to the system heap (the program's) until the guard drops.
#[inline]
pub fn kept() -> PhaseGuard {
    enter(KEPT)
}

thread_local! {
    static AHEAD: std::cell::RefCell<Option<Vec<Box<dyn FnOnce()>>>> = const { std::cell::RefCell::new(None) };
}

/// The settle swap restores the collected image ahead of its pause (jit zone_swap.rs), while
/// compiles still run on generation g's statics; the swap takes those statics only once the pause
/// began. Between `ahead_begin` and `ahead_end` the writes this thread's restore makes to such
/// statics are deferred (`defer`) instead; the swap runs them after its take (into the new
/// generation's empty statics), or drops them when it aborts.
pub fn ahead_begin() {
    AHEAD.with(|a| *a.borrow_mut() = Some(Vec::new()));
}

/// Ends the restore ahead of the pause: the writes it deferred, in order.
pub fn ahead_end() -> Vec<Box<dyn FnOnce()>> {
    AHEAD.with(|a| a.borrow_mut().take()).unwrap_or_default()
}

/// Whether this thread restores ahead of a swap's pause (`ahead_begin`).
pub fn ahead() -> bool {
    AHEAD.with(|a| a.borrow().is_some())
}

/// Runs `f` now, or (a restore ahead of the swap's pause) after the swap took the statics.
pub fn defer(f: impl FnOnce() + 'static) {
    let f = AHEAD.with(|a| match a.borrow_mut().as_mut() {
        Some(v) => {
            v.push(Box::new(f));
            None
        }
        None => Some(f),
    });
    if let Some(f) = f {
        f()
    }
}

/// An environment variable read once into `cell`; unset or empty = None. The value lives in the
/// kept heap (a zone swap never resets the static), and the static holds a String, never an
/// `Option`: a `None` written into an inline payload carries a stack temporary's bytes, which
/// MAKEPAD_RUN_ZONE_CHECK=2 reports as stale compiler words.
pub fn env_once(cell: &'static std::sync::OnceLock<String>, name: &str) -> Option<&'static str> {
    let s = cell.get_or_init(|| {
        let _k = kept();
        std::env::var(name).unwrap_or_default()
    });
    (!s.is_empty()).then_some(s.as_str())
}
