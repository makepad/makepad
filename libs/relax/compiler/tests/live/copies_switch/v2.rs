// Copy versions (opt::inline_site, jit::note_guard_reads): a patch switches every inlined copy, of
// the callee itself and of a body the callee inlined unguarded. Two threads spin in loops (`spin_a`,
// `spin_b`) whose O3 code holds a copy of `mid`; `mid`'s body inlined `leaf` before any call or
// loop (an unguarded copy). Each guard reads its loop's copy version first, and the flags of `mid`
// and `leaf` only once it moved. Each run stops the spinners the previous run started, prints what
// they computed after the run began, then starts new ones (each run sleeps so its O3 code installs
// before the spinners enter it).
//   v0: leaf = 1, mid(i) = leaf * 10 + i;  v1: leaf = 2;  v2: mid(i) = leaf * 100 + i
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

static STOP: AtomicBool = AtomicBool::new(false);
static LAST_A: AtomicU64 = AtomicU64::new(0);
static LAST_B: AtomicU64 = AtomicU64::new(0);
static SPINNERS: Mutex<Vec<std::thread::JoinHandle<u64>>> = Mutex::new(Vec::new());

fn leaf() -> u64 {
    2
}

fn mid(i: u64) -> u64 {
    leaf() * 100 + i
}

fn spin_a(hold: bool) -> u64 {
    loop {
        let v = mid(1);
        LAST_A.store(v, Ordering::Relaxed);
        if !hold || STOP.load(Ordering::Relaxed) {
            return v;
        }
    }
}

fn spin_b(hold: bool) -> u64 {
    loop {
        let v = mid(2);
        LAST_B.store(v, Ordering::Relaxed);
        if !hold || STOP.load(Ordering::Relaxed) {
            return v;
        }
    }
}

#[test]
fn run() {
    let mut spinners = SPINNERS.lock().unwrap();
    if !spinners.is_empty() {
        // twice: an iteration in flight at the patch stores its value at most once
        for last in [&LAST_A, &LAST_B] {
            for _ in 0..2 {
                last.store(0, Ordering::SeqCst);
                while last.load(Ordering::SeqCst) == 0 {
                    std::thread::yield_now();
                }
            }
        }
        println!("out spinning {} {}", LAST_A.load(Ordering::SeqCst), LAST_B.load(Ordering::SeqCst));
        STOP.store(true, Ordering::SeqCst);
        for h in spinners.drain(..) {
            h.join().unwrap();
        }
    }
    println!("out fresh {} {}", spin_a(false), spin_b(false));
    std::thread::sleep(std::time::Duration::from_millis(300));
    STOP.store(false, Ordering::SeqCst);
    spinners.push(std::thread::spawn(|| spin_a(true)));
    spinners.push(std::thread::spawn(|| spin_b(true)));
}
