// 3.11: a static's type changes (AtomicU32 -> AtomicU64) while it holds a value: new code must
// never read the old-sized storage. The edit takes the restart rung, its reason naming the static
// and both types (a value migration is not built: a static's type change is rare, and a restart
// is the safe answer); the running code keeps counting (7, then 8).
use std::sync::atomic::{AtomicU64, Ordering};

static COUNT: AtomicU64 = AtomicU64::new(100);

fn bump() -> u64 {
    COUNT.fetch_add(1, Ordering::SeqCst) + 1
}

#[test]
fn run() {
    println!("out count {}", bump());
}
