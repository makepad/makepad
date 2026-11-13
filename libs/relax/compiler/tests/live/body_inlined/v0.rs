// M46: an ordinary fn B that A's code inlined is edited while a frame of A is on another thread's
// stack (parked inside A after A's first B call). A is built at O3 (RELAX_O3_CALLS=1 makes
// it hot at once; each run sleeps so its O3 code installs before the thread parks in it), which
// inlines B: the parked frame's next B is the old copy inside old A. Each run releases the thread
// the previous run parked inside A, prints what that A call returned, then parks a new one.
//   A = B(0) + B(1);  v0: B(i) = i + 10;  v1: B(i) = i + 20
//   stacked across v0 -> v1: B(0) old (10) + B(1) new (21): 31
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

fn b(i: u64) -> u64 {
    i + 10
}

fn a(hold: bool) -> u64 {
    let mut acc = 0;
    for i in 0..2 {
        if i == 1 {
            wait_go(hold);
        }
        acc += b(i);
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
    std::thread::sleep(std::time::Duration::from_millis(300));
    park_one();
    *have = true;
}
