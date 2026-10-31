// cold has a slot in v0, but no call until v2. Keep the branch dynamic, and call cold through an
// opaque fn pointer: O1 inlining would otherwise lower it with its caller in v0.
use std::sync::atomic::{AtomicBool, Ordering};

static TAKE_COLD: AtomicBool = AtomicBool::new(false);

#[inline(never)]
fn cold() -> u32 { 40 }

#[inline(never)]
fn a() -> u32 {
    (if TAKE_COLD.load(Ordering::SeqCst) { std::hint::black_box(cold as fn() -> u32)() } else { 0 }) + 2
}

#[test]
fn run() {
    TAKE_COLD.store(false, Ordering::SeqCst);
    println!("out {}", a());
}
