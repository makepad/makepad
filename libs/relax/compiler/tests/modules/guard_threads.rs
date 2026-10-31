// Run with MAKEPAD_RUN_SANDBOX_FUEL=0: these must be watchdog errors, not fuel exhaustion.
use std::relax::{guard, Module};
use std::sync::{Arc, Barrier};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

fn module(name: &str) -> Module {
    Module::load_source(name, "guard_threads_module.rs", r#"
        pub fn quick(x: u64) -> u64 { x.wrapping_add(1) }
        pub fn slow() { loop { std::hint::spin_loop(); } }
    "#, &[]).unwrap()
}

#[test]
fn concurrent_and_nested() {
    // Separate arenas: simultaneous calls must not share the module's mutable state.
    let slow_module = module("deadline_slow");
    let fast_module = module("deadline_fast");
    let slow: fn() = slow_module.get("slow").unwrap();
    let quick: fn(u64) -> u64 = fast_module.get("quick").unwrap();
    assert_eq!(guard(1000, || quick(1)).unwrap(), 2);
    let start = Arc::new(Barrier::new(2));
    let done = Arc::new(AtomicBool::new(false));
    let start_slow = start.clone();
    let done_slow = done.clone();
    let slow_thread = std::thread::spawn(move || {
        start_slow.wait();
        let now = Instant::now();
        assert!(guard(150, || slow()).is_err());
        assert!(now.elapsed() < Duration::from_secs(3));
        done_slow.store(true, Ordering::SeqCst);
    });
    let fast_thread = std::thread::spawn(move || {
        start.wait();
        let now = Instant::now();
        let mut calls = 0;
        while !done.load(Ordering::SeqCst) || calls < 100 {
            assert_eq!(guard(1000, || quick(calls)).unwrap(), calls + 1);
            calls += 1;
            // Repeated arm/disarm overlaps the slow deadline, with time for signal delivery.
            std::thread::sleep(Duration::from_millis(1));
            assert!(now.elapsed() < Duration::from_secs(15));
        }
    });
    slow_thread.join().unwrap();
    fast_thread.join().unwrap();
    println!("out concurrent guards: true");

    // Inner completion must leave the outer deadline armed.
    assert!(guard(100, || {
        assert_eq!(guard(1000, || quick(4)).unwrap(), 5);
        slow();
    }).is_err());
    // An unbounded inner guard cannot extend the outer deadline. The outer guard
    // owns this timeout; control must never return to its closure after the inner call.
    let returned_inner = Arc::new(AtomicBool::new(false));
    let flag = returned_inner.clone();
    assert!(guard(50, || {
        let _ = guard(0, || slow());
        flag.store(true, Ordering::SeqCst);
    }).is_err());
    assert!(!returned_inner.load(Ordering::SeqCst));
    // Inner timeout must not cancel or mark the outer call as timed out.
    assert_eq!(guard(2000, || {
        assert!(guard(50, || slow()).is_err());
        guard(0, || quick(6)).unwrap()
    }).unwrap(), 7);
    // A completed short timer cannot interrupt a subsequent no-deadline call.
    for _ in 0..100 { assert_eq!(guard(20, || quick(0)).unwrap(), 1); }
    assert_eq!(guard(0, || {
        std::thread::sleep(Duration::from_millis(40));
        quick(8)
    }).unwrap(), 9);
    println!("out nested guards: true");
    slow_module.unload();
    fast_module.unload();
}
