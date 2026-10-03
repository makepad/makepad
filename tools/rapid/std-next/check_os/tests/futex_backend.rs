//! The macOS futex backend choice: os_sync when the OS has it, ulock with RAPID_FORCE_ULOCK=1.
//! Run twice (os_diff.sh does): plain, and with RAPID_FORCE_ULOCK=1 FUTEX_EXPECT=ulock.
#![cfg(target_os = "macos")]
extern crate rapid_std_os_check as hrs;

use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

#[test]
fn backend_matches_env_and_works() {
    let expect = std::env::var("FUTEX_EXPECT").unwrap_or_else(|_| String::from("os_sync"));
    assert_eq!(hrs::sys::os::futex_backend(), expect);
    // cached: same answer again
    assert_eq!(hrs::sys::os::futex_backend(), expect);
    let f = AtomicU32::new(7);
    // value differs: returns at once
    assert!(hrs::sys::os::futex_wait(&f, 8, Some(Duration::from_millis(500))));
    // value matches: times out, and not early
    let t = Instant::now();
    assert!(!hrs::sys::os::futex_wait(&f, 7, Some(Duration::from_millis(20))));
    assert!(t.elapsed() >= Duration::from_millis(20));
    // sub-microsecond timeout still times out (ulock rounds up)
    assert!(!hrs::sys::os::futex_wait(&f, 7, Some(Duration::from_nanos(10))));
    assert!(!hrs::sys::os::futex_wake(&f));
    // a waiter is woken by another thread
    let f2: &'static AtomicU32 = Box::leak(Box::new(AtomicU32::new(0)));
    let h = std::thread::spawn(move || {
        while f2.load(Ordering::SeqCst) == 0 {
            hrs::sys::os::futex_wait(f2, 0, None);
        }
        f2.load(Ordering::SeqCst)
    });
    std::thread::sleep(Duration::from_millis(10));
    f2.store(5, Ordering::SeqCst);
    hrs::sys::os::futex_wake_all(f2);
    assert_eq!(h.join().unwrap(), 5);
}
