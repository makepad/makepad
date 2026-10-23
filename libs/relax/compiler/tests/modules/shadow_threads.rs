// Per-thread module state (MEMRECLAIM.REVIEW F4): shadow stacks, wrapper result slots and fuel
// belong to one thread's entry, never shared.
// 1. Re-entry on one thread, A -> host -> B -> host -> A: the inner A call continues below the
//    outer A's live shadow frames (it used to restart at the top of A's shadow stack, over them).
// 2. Two threads in one module at once: each keeps its own shadow frames and result slots.
// 3. Fuel is per thread and per call: another thread entering the module does not refill it.
// Run with MAKEPAD_RUN_MODULE_FUEL=2000000.
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

pub static TO_B: AtomicUsize = AtomicUsize::new(0);
pub static TO_A: AtomicUsize = AtomicUsize::new(0);
pub static ENTRIES: AtomicU64 = AtomicU64::new(0);
pub static WANT: AtomicBool = AtomicBool::new(false);

/// Host API: A calls into B.
pub fn host_to_b(x: u64) -> u64 {
    let f: fn(u64) -> u64 = unsafe { std::mem::transmute(TO_B.load(Ordering::SeqCst)) };
    f(x)
}
/// Host API: B calls back into A.
pub fn host_to_a(x: u64) -> u64 {
    let f: fn(u64) -> u64 = unsafe { std::mem::transmute(TO_A.load(Ordering::SeqCst)) };
    f(x)
}
/// Host API: lets the other thread run (both threads' frames are live meanwhile).
pub fn host_yield(_x: u64) -> u64 {
    for _ in 0..20 { std::thread::yield_now(); }
    0
}
/// Host API: waits until another thread has entered the module since this call.
pub fn host_tick(_x: u64) -> u64 {
    let n = ENTRIES.load(Ordering::SeqCst);
    WANT.store(true, Ordering::SeqCst);
    let t = std::time::Instant::now();
    while ENTRIES.load(Ordering::SeqCst) == n && t.elapsed() < std::time::Duration::from_secs(2) {
        std::hint::spin_loop();
    }
    0
}

const A: &str = r#"
pub fn outer(x: u64) -> u64 {
    // live shadow frame across the host call
    let mut keep = [0u64; 256];
    for i in 0..256 { keep[i] = x.wrapping_mul(i as u64 + 1); }
    let r = test_crate::host_to_b(x);
    let mut ok = true;
    for i in 0..256 { ok &= keep[i] == x.wrapping_mul(i as u64 + 1); }
    if ok { r } else { u64::MAX }
}
pub fn inner(x: u64) -> u64 {
    // a big frame: overwrites whatever is at the top of A's shadow stack
    let mut scratch = [0u64; 1024];
    for i in 0..1024 { scratch[i] = !(x + i as u64); }
    let mut s = 0u64;
    for i in 0..1024 { s = s.wrapping_add(scratch[i]); }
    s
}
"#;
const B: &str = "pub fn mid(x: u64) -> u64 { let mut v = [x; 64]; v[3] += 1; test_crate::host_to_a(v[3]) + v[0] }\n";
const T: &str = r#"
pub fn work(seed: u64) -> u64 {
    let mut a = [0u64; 512];
    for i in 0..512 { a[i] = seed.wrapping_mul(2654435761).wrapping_add(i as u64); }
    test_crate::host_yield(seed);
    let mut bad = 0u64;
    for i in 0..512 { if a[i] != seed.wrapping_mul(2654435761).wrapping_add(i as u64) { bad += 1; } }
    bad
}
pub fn pair(seed: u64) -> (u64, [u64; 6]) {
    let v = [seed, seed + 1, seed + 2, seed + 3, seed + 4, seed + 5];
    test_crate::host_yield(seed);
    (seed, v)
}
pub fn quick(x: u64) -> u64 { x + 1 }
pub fn spin(n: u64) -> u64 {
    let mut x = 1u64;
    let mut i = 0;
    while i < n {
        x = x.wrapping_mul(6364136223846793005).wrapping_add(i);
        if i % 20000 == 0 { test_crate::host_tick(i); }
        i += 1;
    }
    x
}
"#;

fn inner_want(x: u64) -> u64 {
    let mut s = 0u64;
    for i in 0..1024u64 { s = s.wrapping_add(!(x + i)); }
    s
}

#[test]
fn shadow_threads() {
    use std::relax::Module;
    // 1. A -> host -> B -> host -> A
    let a = Module::load_source("sh_a", "sh_a.rs", A, &["test_crate"]).unwrap();
    let b = Module::load_source("sh_b", "sh_b.rs", B, &["test_crate"]).unwrap();
    let outer: fn(u64) -> u64 = a.get("outer").unwrap();
    let inner: fn(u64) -> u64 = a.get("inner").unwrap();
    let mid: fn(u64) -> u64 = b.get("mid").unwrap();
    TO_B.store(mid as usize, Ordering::SeqCst);
    TO_A.store(inner as usize, Ordering::SeqCst);
    let x = 7;
    let r = outer(x);
    println!("out re-entry keeps the outer frames: {}", r == inner_want(x + 1) + x);

    // 2. two threads in one module
    let t = Module::load_source("sh_t", "sh_t.rs", T, &["test_crate"]).unwrap();
    let work: fn(u64) -> u64 = t.get("work").unwrap();
    let pair: fn(u64) -> (u64, [u64; 6]) = t.get("pair").unwrap();
    let bad = Arc::new(AtomicU64::new(0));
    let mut hs = Vec::new();
    for k in 0..4u64 {
        let bad = bad.clone();
        hs.push(std::thread::spawn(move || {
            for j in 0..200u64 {
                let seed = k * 1_000_000 + j;
                bad.fetch_add(work(seed), Ordering::Relaxed);
                let (s, v) = pair(seed);
                if s != seed || v != [seed, seed + 1, seed + 2, seed + 3, seed + 4, seed + 5] {
                    bad.fetch_add(1, Ordering::Relaxed);
                }
            }
        }));
    }
    for h in hs { h.join().unwrap(); }
    eprintln!("shadow_threads: {} corrupted words/results across threads", bad.load(Ordering::SeqCst));
    println!("out threads keep their own frames: {}", bad.load(Ordering::SeqCst) == 0);

    // 3. fuel: a long call is stopped although another thread enters the module meanwhile
    let quick: fn(u64) -> u64 = t.get("quick").unwrap();
    let spin: fn(u64) -> u64 = t.get("spin").unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let s2 = stop.clone();
    let hammer = std::thread::spawn(move || {
        while !s2.load(Ordering::SeqCst) {
            if WANT.swap(false, Ordering::SeqCst) {
                quick(1);
                ENTRIES.fetch_add(1, Ordering::SeqCst);
            }
            std::hint::spin_loop();
        }
    });
    let long = std::relax::guard(0, || spin(10_000_000));
    stop.store(true, Ordering::SeqCst);
    hammer.join().unwrap();
    eprintln!("shadow_threads: entries by the other thread during the long call: {}", ENTRIES.load(Ordering::SeqCst));
    println!("out fuel is per thread: {}", long.is_err() && ENTRIES.load(Ordering::SeqCst) > 0);
    let short = std::relax::guard(0, || spin(10_000));
    println!("out short call still runs: {}", short.is_ok());
}
