// 3.11: a static's type changes (AtomicU32 -> AtomicU64) while it holds a value: new code must
// never read the old-sized storage. Target: the value migrates by name (7 -> 8 as u64).
use std::sync::atomic::{AtomicU32, Ordering};

static COUNT: AtomicU32 = AtomicU32::new(5);

fn bump() -> u64 {
    COUNT.fetch_add(1, Ordering::SeqCst) as u64 + 1
}

#[test]
fn run() {
    println!("out count {}", bump());
}
