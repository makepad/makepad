// M46, a loop: a thread spins in a loop that has no call left once O3 inlined `step` into it (an
// atomic poll). A live edit of `step` reaches the running loop at its next iteration: the inlined
// copy's guard reads `step`'s patch flag every iteration (never hoisted out of the loop). Each run
// stops the spinner the previous run started, prints what it computed after the run began, then
// starts a new one (each run sleeps so its O3 code installs before the spinner enters it).
//   v0: step(i) = i + 10;  v1: step(i) = i + 20
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

static STOP: AtomicBool = AtomicBool::new(false);
static LAST: AtomicU64 = AtomicU64::new(0);
static SPINNER: Mutex<Option<std::thread::JoinHandle<u64>>> = Mutex::new(None);

fn step(i: u64) -> u64 {
    i + 10
}

fn spin(hold: bool) -> u64 {
    loop {
        let v = step(1);
        LAST.store(v, Ordering::Relaxed);
        if !hold || STOP.load(Ordering::Relaxed) {
            return v;
        }
    }
}

#[test]
fn run() {
    let mut spinner = SPINNER.lock().unwrap();
    if let Some(h) = spinner.take() {
        // twice: an iteration in flight at the patch stores its value at most once
        for _ in 0..2 {
            LAST.store(0, Ordering::SeqCst);
            while LAST.load(Ordering::SeqCst) == 0 {
                std::thread::yield_now();
            }
        }
        println!("out spinning {}", LAST.load(Ordering::SeqCst));
        STOP.store(true, Ordering::SeqCst);
        h.join().unwrap();
    }
    println!("out fresh {}", spin(false));
    std::thread::sleep(std::time::Duration::from_millis(300));
    STOP.store(false, Ordering::SeqCst);
    *spinner = Some(std::thread::spawn(|| spin(true)));
}
