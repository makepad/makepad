// M46, a loop: a thread spins in a loop that has no call left once O3 inlined `step` into it (an
// atomic poll). A live edit of `step` reaches the running loop at its next iteration: the loop's
// exit reads its copy version every iteration (never hoisted out of the loop) and, once the patch
// moved it, leaves for the patched path, whose copy calls the new `step`. The loop runs in bursts
// (`burst`, called again and again: each call enters its newest code, so the loop reaches O3 while
// the spinner runs; a running call never moves to new code). The session settles before its second
// run (RELAX_LIVE_SETTLE), so the edit meets the settled code, and its report names `step`'s
// copies (expect.txt). Each run stops the spinner the previous run started, prints what it computed
// after the run began, then starts a new one.
//   v0: step(i) = i + 10;  v1: step(i) = i + 20
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

static STOP: AtomicBool = AtomicBool::new(false);
static LAST: AtomicU64 = AtomicU64::new(0);
static SPINNER: Mutex<Option<std::thread::JoinHandle<u64>>> = Mutex::new(None);

fn step(i: u64) -> u64 {
    i + 10
}

fn burst() -> u64 {
    let mut v = 0;
    for _ in 0..1000 {
        v = step(1);
        LAST.store(v, Ordering::Relaxed);
    }
    v
}

fn spin(hold: bool) -> u64 {
    loop {
        let v = burst();
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
    STOP.store(false, Ordering::SeqCst);
    *spinner = Some(std::thread::spawn(|| spin(true)));
}
