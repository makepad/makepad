// The deadline worker must retain outer and concurrent guards and cancel returned
// calls. Fuel is disabled for this test so only the wall-clock limit can stop spin.
use std::relax::{guard, Module};
use std::time::{Duration, Instant};

#[test]
fn deadlines() {
    let m = Module::load_source("deadline_spin", "deadline_spin.rs", "pub fn spin(n: u64) -> u64 { let mut x = 1u64; for i in 0..n { x = x.wrapping_mul(6364136223846793005).wrapping_add(i); } x }", &[]).unwrap();
    let spin: fn(u64) -> u64 = m.get("spin").unwrap();
    // Warm the body so the deadline never measures its first lazy compile.
    for _ in 0..10000 { assert_ne!(spin(10), 0); }
    std::thread::sleep(Duration::from_millis(800));
    let start = Instant::now();
    let outer = guard(30, || guard(1000, || spin(u64::MAX)));
    assert!(outer.is_err() || outer.unwrap().is_err());
    assert!(start.elapsed() < Duration::from_millis(800));
    // Returning an inner guard must restore the still-active outer deadline.
    let start = Instant::now();
    let outer = guard(30, || { guard(1000, || spin(10)).unwrap(); spin(u64::MAX) });
    assert!(outer.is_err());
    assert!(start.elapsed() < Duration::from_millis(800));
    // A newer guard on another thread must not cancel this thread's deadline.
    let ready = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = ready.clone();
    let other = std::thread::spawn(move || {
        while !flag.load(std::sync::atomic::Ordering::SeqCst) { std::thread::yield_now(); }
        guard(1000, || std::thread::sleep(Duration::from_millis(150))).unwrap();
    });
    let start = Instant::now();
    let r = guard(30, || { ready.store(true, std::sync::atomic::Ordering::SeqCst); spin(u64::MAX) });
    assert!(r.is_err());
    assert!(start.elapsed() < Duration::from_millis(800));
    other.join().unwrap();
    // Canceled deadlines must not interrupt a later call.
    for _ in 0..100 { guard(1, || spin(10)).unwrap(); }
    std::thread::sleep(Duration::from_millis(20));
    assert!(guard(1000, || spin(10000)).is_ok());
    println!("out deadlines: true");
}
