// G8 / 3.11: statics across edits.
//  v1: the initializer is edited: the static KEEPS its value, and the live driver says so (never a
//      silent no-op).
//  v2: the static's type changes (AtomicU64 -> AtomicU64): new code must not read the old-sized
//      storage; the value migrates by name (8 -> 8) or the edit takes the restart rung.
use std::sync::atomic::{AtomicU64, Ordering};

static COUNT: AtomicU64 = AtomicU64::new(100);

fn bump() -> u64 {
    COUNT.fetch_add(1, Ordering::SeqCst) + 1
}

#[test]
fn run() {
    println!("out count {}", bump());
}
