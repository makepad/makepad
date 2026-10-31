// 3.1: A loops calling B; both are edited while a frame of A is on another thread's stack.
// A's running call finishes in A's old code, but its next call of B runs B's new code (B is called
// through its slot). Each run releases the thread the previous run parked inside A (after A's
// first B call), prints what that A call returned, then parks a new one.
//   A = sum over i in 0..2 of B(i) * WA;  v0: B(i) = i + 10, WA = 1;  v1: B(i) = i + 20, WA = 100
//   stacked across v0 -> v1: old A (WA 1) with B(0) old (10) and B(1) new (21): 31
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

static PARKED: AtomicBool = AtomicBool::new(false);
static GO: AtomicBool = AtomicBool::new(false);
static DONE: AtomicBool = AtomicBool::new(false);
static RESULT: AtomicU64 = AtomicU64::new(0);
static HAVE: Mutex<bool> = Mutex::new(false);

fn wait_go(hold: bool) {
    if hold {
        PARKED.store(true, Ordering::SeqCst);
        while !GO.load(Ordering::SeqCst) {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
}

// not inlined into a (else B(1) would run the old copy inside old A): the dead locals keep it above
// O1's source-size inlining gate (#[inline(never)] is read as an inline hint today)
fn b(i: u64) -> u64 {
    let _d0 = 0u64; let _d1 = 1u64; let _d2 = 2u64; let _d3 = 3u64; let _d4 = 4u64;
    let _d5 = 5u64; let _d6 = 6u64; let _d7 = 7u64; let _d8 = 8u64; let _d9 = 9u64;
    let _e0 = 0u64; let _e1 = 1u64; let _e2 = 2u64; let _e3 = 3u64; let _e4 = 4u64;
    let _e5 = 5u64; let _e6 = 6u64; let _e7 = 7u64; let _e8 = 8u64; let _e9 = 9u64;
    i + 10
}

fn a(hold: bool) -> u64 {
    let mut acc = 0;
    for i in 0..2 {
        if i == 1 {
            wait_go(hold);
        }
        acc += b(i) * 1;
    }
    acc
}

fn park_one() {
    PARKED.store(false, Ordering::SeqCst);
    GO.store(false, Ordering::SeqCst);
    DONE.store(false, Ordering::SeqCst);
    std::thread::spawn(|| {
        let r = a(true);
        RESULT.store(r, Ordering::SeqCst);
        DONE.store(true, Ordering::SeqCst);
    });
    while !PARKED.load(Ordering::SeqCst) {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

#[test]
fn run() {
    let mut have = HAVE.lock().unwrap();
    if *have {
        GO.store(true, Ordering::SeqCst);
        while !DONE.load(Ordering::SeqCst) {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        println!("out stacked {}", RESULT.load(Ordering::SeqCst));
    }
    println!("out fresh {}", a(false));
    park_one();
    *have = true;
}
