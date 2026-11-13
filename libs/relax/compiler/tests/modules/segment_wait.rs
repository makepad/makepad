// More threads than a module has segments (64): a thread that finds every segment in use waits
// for one, and every call returns. The first 64 calls stay inside until the 64th arrives, so
// the rest must wait. Run with RELAX_SANDBOX_NOCHECK=1 (the module's atomics are std::sync)
// and RELAX_SANDBOX_FUEL=0 (the calls spin).
use std::relax::Module;

const M: &str = r#"
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

static INSIDE: AtomicU64 = AtomicU64::new(0);
static MOST: AtomicU64 = AtomicU64::new(0);
static OPEN: AtomicBool = AtomicBool::new(false);

pub fn hold(full: u64) -> u64 {
    let n = INSIDE.fetch_add(1, Ordering::SeqCst) + 1;
    MOST.fetch_max(n, Ordering::SeqCst);
    if n >= full {
        OPEN.store(true, Ordering::SeqCst);
    }
    while !OPEN.load(Ordering::SeqCst) {
        std::hint::spin_loop();
    }
    INSIDE.fetch_sub(1, Ordering::SeqCst);
    1
}

pub fn most() -> u64 {
    MOST.load(Ordering::SeqCst)
}
"#;

#[test]
fn threads_wait_for_a_segment() {
    let m = Module::load_source("segment_wait", "segment_wait.rs", M, &[]).unwrap();
    let hold: fn(u64) -> u64 = m.get("hold").unwrap();
    let most: fn() -> u64 = m.get("most").unwrap();
    let threads: Vec<_> = (0..100).map(|_| std::thread::spawn(move || hold(64))).collect();
    let returned: u64 = threads.into_iter().map(|t| t.join().unwrap()).sum();
    println!("out calls returned: {}", returned);
    println!("out most inside at once: {}", most());
    m.unload();
}
