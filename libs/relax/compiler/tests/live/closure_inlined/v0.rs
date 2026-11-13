// M46: closure_on_stack with A built at O3 (RELAX_O3_CALLS=1 makes it hot at once; each run
// sleeps so its O3 code installs before the thread parks in it): A's code inlined the closure, so
// the parked frame's next call of it is the old copy inside old A.
//   A = keep(0) + keep(1);  v0: keep(i) = i + base;  v1: keep(i) = i + base * 2  (base = 10)
//   stacked across v0 -> v1: keep(0) old (10) + keep(1) new (21): 31
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

fn a(hold: bool) -> u64 {
    let base = 10u64;
    let keep = |i: u64| -> u64 { i + base };
    let mut acc = 0;
    for i in 0..2 {
        if i == 1 {
            wait_go(hold);
        }
        acc += keep(i);
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
