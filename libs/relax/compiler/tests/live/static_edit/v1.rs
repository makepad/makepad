// G8 / 3.11: statics across edits.
//  v1: the initializer is edited: the static KEEPS its value, and the live driver says so (never a
//      silent no-op).
use std::sync::atomic::{AtomicU32, Ordering};

static COUNT: AtomicU32 = AtomicU32::new(100);

fn bump() -> u64 {
    COUNT.fetch_add(1, Ordering::SeqCst) as u64 + 1
}

#[test]
fn run() {
    println!("out count {}", bump());
}
