// Threads the OS refuses, and scopes that panic. A spawn the OS refuses (EAGAIN: a thread limit
// the program sets, RLIMIT_NPROC, counts threads on Linux; elsewhere the spawns here succeed)
// returns Err from the Builder and panics from Scope::spawn, and its scope still ends once its
// other threads are done. A scope whose closure panics waits for the threads it started before
// the panic goes on. Code compiled for the first time while no thread may start runs too.
// Relax issue 231: the JIT's pool panicked when the OS refused it a thread; a refused scoped spawn
// counted its thread out twice, so the scope waited forever; a panicking scope did not wait.
// Differential file: `| ` lines must match under rustc --test and makepad test.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::Duration;

#[cfg(target_os = "linux")]
fn without_new_threads<R>(f: impl FnOnce() -> R) -> R {
    extern "C" {
        fn setrlimit(resource: i32, rlim: *const [u64; 2]) -> i32;
        fn getrlimit(resource: i32, rlim: *mut [u64; 2]) -> i32;
    }
    const RLIMIT_NPROC: i32 = 6;
    let mut old = [0u64; 2];
    unsafe { getrlimit(RLIMIT_NPROC, &mut old) };
    unsafe { setrlimit(RLIMIT_NPROC, &[0, old[1]]) };
    let r = f();
    unsafe { setrlimit(RLIMIT_NPROC, &old) };
    r
}
#[cfg(not(target_os = "linux"))]
fn without_new_threads<R>(f: impl FnOnce() -> R) -> R {
    f()
}

#[inline(never)]
fn fresh<const K: usize>(n: u64) -> u64 {
    let v: Vec<String> = (0..n).map(|i| format!("{i}:{K}")).collect();
    let m: std::collections::BTreeMap<&String, usize> = v.iter().map(|s| (s, s.len())).collect();
    m.values().map(|&l| l as u64).sum::<u64>() + K as u64
}

#[test]
fn a01_a_refused_spawn_in_a_scope() {
    let data = vec![1u32, 2, 3];
    let refused_on_linux = std::thread::scope(|s| {
        let h = s.spawn(|| {
            std::thread::sleep(Duration::from_millis(20));
            data.iter().sum::<u32>()
        });
        let (builder, panicked, fresh_sum) = without_new_threads(|| {
            let builder = std::thread::Builder::new().spawn_scoped(s, || 0u32).is_err();
            let panicked = catch_unwind(AssertUnwindSafe(|| {
                s.spawn(|| 5u32);
            }))
            .is_err();
            (builder, panicked, fresh::<1>(30) + fresh::<2>(30) + fresh::<3>(30))
        });
        println!("| first thread: {}", h.join().unwrap());
        println!("| fresh code: {fresh_sum}");
        builder == panicked && (builder == cfg!(target_os = "linux") || !builder)
    });
    println!("| the scope ended: {refused_on_linux}");
}

#[test]
fn a02_a_panicking_scope_waits_for_its_threads() {
    let done = AtomicBool::new(false);
    let seen = AtomicU32::new(0);
    let r = catch_unwind(AssertUnwindSafe(|| {
        std::thread::scope(|s| {
            s.spawn(|| {
                std::thread::sleep(Duration::from_millis(30));
                seen.fetch_add(1, Ordering::SeqCst);
                done.store(true, Ordering::SeqCst);
            });
            panic!("the scope's closure panics");
        })
    }));
    println!("| panicked: {}, its thread was done first: {}, seen {}", r.is_err(), done.load(Ordering::SeqCst), seen.load(Ordering::SeqCst));
    let msg = r.err().and_then(|e| e.downcast_ref::<&str>().map(|s| s.to_string()));
    println!("| payload: {msg:?}");
}
