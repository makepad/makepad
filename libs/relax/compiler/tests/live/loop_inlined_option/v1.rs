// M46, a loop over an Option result: the spinner's inner loop takes values from `next` until it
// returns None. O3 inlines `next` into it behind its patch flag; the Option is returned in memory,
// so the copy's slow call writes the result buffer and the loop reads it back from there, while
// the copy's own path keeps it in registers. A live edit of `next` reaches the running loop: its
// slow call's result is what the loop adds up. The loop runs in bursts (`burst`, called again and
// again: each call enters its newest code, so it reaches O3 while the spinner runs); the session
// settles before its second run (MAKEPAD_RUN_LIVE_SETTLE) and the edit's report names `next`'s
// copies (expect.txt). Each run stops the spinner the previous run started, prints what it
// computed after the run began, then starts a new one.
//   v0: next(i) = Some(i + 10) for i < N;  v1: Some(i + 20)
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

static STOP: AtomicBool = AtomicBool::new(false);
static LAST: AtomicU64 = AtomicU64::new(0);
static N: AtomicU64 = AtomicU64::new(4);
static SPINNER: Mutex<Option<std::thread::JoinHandle<u64>>> = Mutex::new(None);

fn next(i: u64, n: u64) -> Option<u64> {
    if i < n {
        Some(i + 20)
    } else {
        None
    }
}

fn burst() -> u64 {
    let mut last = 0;
    for _ in 0..1000 {
        let n = N.load(Ordering::Relaxed);
        let (mut i, mut v) = (0, 0);
        while let Some(x) = next(i, n) {
            v += x;
            i += 1;
        }
        LAST.store(v, Ordering::Relaxed);
        last = v;
    }
    last
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
