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
