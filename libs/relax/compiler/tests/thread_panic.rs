// A panicking thread ends only itself (rustc: std thread start runs main under catch_unwind and stores
// Err in the packet -- library/std/src/thread/lifecycle.rs; catch_unwind returns 1 on unwind --
// core/src/intrinsics/mod.rs catch_unwind).
// catch_unwind returns Err; nested catch_unwind inside a thread. Output must equal rustc's
// (stdout only; the panic messages go to stderr). Needs std-next.
use std::panic;
use std::thread;

#[inline(never)]
fn boom(x: u32) -> u32 {
    if x > 2 {
        panic!("boom {}", x);
    }
    x * 10
}

#[test]
fn threads() {
    let ok = thread::spawn(|| boom(2)).join();
    let ok = ok.is_ok();
    println!("ok thread: {}", ok);
    let bad = thread::spawn(|| boom(5)).join();
    let bad = bad.is_err();
    println!("panicking thread: is_err {}", bad);
    let mut hs = Vec::new();
    for i in 0..6u32 {
        hs.push(thread::spawn(move || boom(i)));
    }
    let mut n_ok = 0;
    let mut n_err = 0;
    for h in hs {
        let r = h.join();
        if r.is_ok() {
            n_ok += 1;
        } else {
            n_err += 1;
        }
    }
    println!("six threads: ok {} err {}", n_ok, n_err);
    println!("still alive");
}

#[test]
fn catch_unwind() {
    let r = panic::catch_unwind(|| boom(7));
    println!("catch_unwind: is_err {}", r.is_err());
    let r = panic::catch_unwind(|| boom(1));
    println!("catch_unwind ok: {:?}", r.ok());
    let h = thread::spawn(|| {
        let inner = panic::catch_unwind(|| boom(9));
        (inner.is_err(), boom(1))
    });
    println!("nested: {:?}", h.join().ok());
}
