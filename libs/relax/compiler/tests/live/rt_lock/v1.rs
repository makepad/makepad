// G7 / 3.13 (M1, M8): a real-time thread (stand-in for the audio callback) calls `voice` in a
// tight loop and records its longest single call. v1 edits `voice` to call `heavy`, which no one
// has called yet (a lazy stub), and edits `big` (compiled, so the patch compiles it again while
// holding today's unit lock). The real-time thread must never compile and never wait for the
// compiler: the installer compiles every uncompiled callee reachable from patched code before it
// installs, and the patch compiles off the lock. Expected: no call of `voice` takes 5 ms or more
// in any window (`heavy` is built big enough that compiling it takes far longer).
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

static STARTED: AtomicBool = AtomicBool::new(false);
static ITERS: AtomicU64 = AtomicU64::new(0);
static MAX_NS: AtomicU64 = AtomicU64::new(0);
static SINK: AtomicU64 = AtomicU64::new(0);
// voice(7) as last computed on the real-time thread: proves it runs the edited code
static LAST7: AtomicU64 = AtomicU64::new(0);

// x4 nested: copies of a few mixing ops, a body that is slow to compile and fast to run (heavy: 4^6 =
// 4096 copies, ~250 ms to compile on node 165; big: 4^5)
macro_rules! x4 {
    ($($t:tt)*) => { $($t)* $($t)* $($t)* $($t)* };
}

fn heavy(i: u64) -> u64 {
    let mut h = i;
    x4! { x4! { x4! { x4! { x4! { x4! { h = h.rotate_left(5) ^ (h >> 3).wrapping_mul(0x9E37_79B9_7F4A_7C15); } } } } } }
    h
}

fn big(i: u64) -> u64 {
    let mut h = i ^ 2;
    x4! { x4! { x4! { x4! { x4! { h = h.rotate_left(7) ^ (h >> 5).wrapping_add(0x2545_F491_4F6C_DD1D); } } } } }
    h
}

// not inlined into rt_loop (whose frame never returns, so it would keep the old copy): the dead
// locals keep it above O1's source-size inlining gate
fn voice(i: u64) -> u64 {
    let _d0 = 0u64; let _d1 = 1u64; let _d2 = 2u64; let _d3 = 3u64; let _d4 = 4u64;
    let _d5 = 5u64; let _d6 = 6u64; let _d7 = 7u64; let _d8 = 8u64; let _d9 = 9u64;
    let _e0 = 0u64; let _e1 = 1u64; let _e2 = 2u64; let _e3 = 3u64; let _e4 = 4u64;
    let _e5 = 5u64; let _e6 = 6u64; let _e7 = 7u64; let _e8 = 8u64; let _e9 = 9u64;
    heavy(i) ^ 0x55
}

fn rt_loop() {
    let mut i = 0u64;
    loop {
        let t = Instant::now();
        let v = voice(i);
        let ns = t.elapsed().as_nanos() as u64;
        SINK.fetch_xor(v, Ordering::Relaxed);
        MAX_NS.fetch_max(ns, Ordering::SeqCst);
        if ITERS.fetch_add(1, Ordering::SeqCst) % 1024 == 0 {
            LAST7.store(voice(7), Ordering::SeqCst);
        }
        i += 1;
    }
}

fn wait_iters(n: u64) {
    let start = ITERS.load(Ordering::SeqCst);
    while ITERS.load(Ordering::SeqCst) < start + n {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
}

#[test]
fn run() {
    if !STARTED.swap(true, Ordering::SeqCst) {
        std::thread::Builder::new().name("rt-voice".to_string()).spawn(rt_loop).unwrap();
        wait_iters(100_000);
        println!("out big {}", big(1) & 1 < 2);
        MAX_NS.store(0, Ordering::SeqCst);
        return;
    }
    wait_iters(100_000);
    let max = MAX_NS.swap(0, Ordering::SeqCst);
    println!("out voice7 {}", LAST7.load(Ordering::SeqCst));
    if max < 5_000_000 {
        println!("out rt ok");
    } else {
        println!("out rt stall {} us", max / 1000);
    }
}
