// 3.1 / 3.3: patching a recursive fn while frames of it are on another thread's stack.
// Each run releases the thread the previous run left parked at the bottom of a 100-deep
// recursion and prints its result, then parks a new one. Frames on the stack across an edit
// finish in the code they started in; fresh calls run the new body.
//   v0: level k adds k, v1: 2k, v2: 3k  (1+..+100 = 5050)
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

static PARKED: AtomicBool = AtomicBool::new(false);
static GO: AtomicBool = AtomicBool::new(false);
static DONE: AtomicBool = AtomicBool::new(false);
static RESULT: AtomicU64 = AtomicU64::new(0);
static HAVE: Mutex<bool> = Mutex::new(false);

fn bottom(hold: bool) {
    if hold {
        PARKED.store(true, Ordering::SeqCst);
        while !GO.load(Ordering::SeqCst) {
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
}

fn rec(n: u64, hold: bool) -> u64 {
    if n == 0 {
        bottom(hold);
        return 0;
    }
    rec(n - 1, hold) + 3 * n
}

fn park_one() {
    PARKED.store(false, Ordering::SeqCst);
    GO.store(false, Ordering::SeqCst);
    DONE.store(false, Ordering::SeqCst);
    std::thread::spawn(|| {
        let r = rec(100, true);
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
    println!("out fresh {}", rec(100, false));
    park_one();
    *have = true;
}
