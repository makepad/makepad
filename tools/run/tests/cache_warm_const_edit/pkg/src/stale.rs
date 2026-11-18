// The stale direction (run.py's stale_* rows): code that holds a copy of what an edit changes, where
// the edited item's own code is no reader of it. These edits land: the code holding the copy is
// rebuilt. Each row edits one item here.
use std::cell::Cell;
use std::hint::black_box;
use std::sync::atomic::{AtomicPtr, AtomicU64, Ordering::Relaxed};

// stale_inlined_only: a const only an always-inlined fn reads, so that fn has no code of its own;
// its one host loops over the copy, and the host's text never changes
pub const F1_K: u64 = 1;

#[inline(always)]
fn f1_leaf() -> u64 {
    F1_K
}

#[inline(never)]
fn f1_host() -> u64 {
    let mut v = 0;
    for _ in 0..black_box(3) {
        v = black_box(f1_leaf());
    }
    v
}

// stale_unwind: a callee edited from returning to unwinding, under a caller that only calls it: the
// caller's guard runs as the panic passes through it (2: caught, and the guard ran). The caller's
// code is built before the edit; the edited callee panics only once the session reads late (after the
// patch's verdict: no inspect watcher is attached, so the panic does not park)
pub struct Quiet;
static F2_UNWOUND: AtomicU64 = AtomicU64::new(0);

struct F2Guard;

impl Drop for F2Guard {
    fn drop(&mut self) {
        if std::thread::panicking() {
            F2_UNWOUND.store(1, Relaxed);
        }
    }
}

#[inline(never)]
fn f2_callee(x: u64) -> u64 {
    x
}

#[inline(never)]
fn f2_caller(x: u64) -> u64 {
    let _g = F2Guard;
    f2_callee(black_box(x))
}

#[inline(never)]
fn f2_unwind(late: bool) -> u64 {
    F2_UNWOUND.store(0, Relaxed);
    let x = late as u64;
    match std::panic::catch_unwind(|| f2_caller(x)) {
        Ok(_) => 0,
        Err(_) => 1 + F2_UNWOUND.load(Relaxed),
    }
}

// stale_nocapture: a callee edited to keep the pointer it is given, under a caller that only calls
// it: the caller reads its local again after a later call writes through that pointer
static F2_KEPT: AtomicPtr<Cell<u64>> = AtomicPtr::new(std::ptr::null_mut());

#[inline(never)]
fn f2_look(c: &Cell<u64>) -> u64 {
    c.get()
}

#[inline(never)]
fn f2_poke() {
    let p = F2_KEPT.swap(std::ptr::null_mut(), Relaxed);
    if !p.is_null() {
        // (the caller's local is alive: it called f2_look, then this, and reads it after)
        unsafe { (*p).set(2) };
    }
}

#[inline(never)]
fn f2_keeper() -> u64 {
    let c = Cell::new(1);
    black_box(f2_look(&c));
    f2_poke();
    c.get()
}

pub fn row(late: bool) -> Vec<(&'static str, u64)> {
    // stale_unwind's panic is expected: it prints nothing
    static QUIET: std::sync::Once = std::sync::Once::new();
    QUIET.call_once(|| {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if info.payload().downcast_ref::<Quiet>().is_none() {
                prev(info)
            }
        }));
    });
    vec![("stale_inlined_only", f1_host()), ("stale_unwind", f2_unwind(late)), ("stale_nocapture", f2_keeper())]
}
