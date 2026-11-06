// G7 / 3.13 (M1, M8): a real-time thread (stand-in for the audio callback) calls `voice` in a
// tight loop and records its longest single call. v1 edits `voice` to call `heavy`, which no one
// has called yet (a lazy stub), and edits `big` (compiled, so the patch compiles it again while
// holding today's unit lock). The real-time thread must never compile and never wait for the
// compiler: the installer compiles every uncompiled callee reachable from patched code before it
// installs, and the patch compiles off the lock. Expected: no call of `voice` takes 5 ms or more
// of its own in any window (`heavy` is built big enough that compiling it takes far longer).
// A call's own time is what the compiler could take from it: on Linux, its thread's CPU time
// over the call, or all of its time when it blocked (a voluntary switch). Being preempted is
// neither: the stand-in runs at the normal policy, so a busy box preempts it for a scheduler
// slice or more (2-22 ms with 3-8 copies on a loaded node, none with a voluntary switch), where
// a real audio thread runs at a real-time policy.
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

static STARTED: AtomicBool = AtomicBool::new(false);
static ITERS: AtomicU64 = AtomicU64::new(0);
// the longest call's own time (the verdict), and the longest call including preemption
static MAX_NS: AtomicU64 = AtomicU64::new(0);
static MAX_WALL_NS: AtomicU64 = AtomicU64::new(0);
static SINK: AtomicU64 = AtomicU64::new(0);
// voice(7) as last computed on the real-time thread: proves it runs the edited code
static LAST7: AtomicU64 = AtomicU64::new(0);

// x4 nested: copies of a few mixing ops, a body that is slow to compile and fast to run (heavy: 4^6 =
// 4096 copies, ~250 ms to compile on a pool node; big: 4^5)
macro_rules! x4 {
    ($($t:tt)*) => { $($t)* $($t)* $($t)* $($t)* };
}

fn heavy(i: u64) -> u64 {
    let mut h = i;
    x4! { x4! { x4! { x4! { x4! { x4! { h = h.rotate_left(5) ^ (h >> 3).wrapping_mul(0x9E37_79B9_7F4A_7C15); } } } } } }
    h
}

fn big(i: u64) -> u64 {
    let mut h = i ^ 1;
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
    i.wrapping_mul(3) ^ 0x55
}

// The calling thread's CPU time so far (ns) and its voluntary switches; off Linux none (0, 0):
// all of a call's time is its own.
#[cfg(target_os = "linux")]
fn thread_use() -> (u64, i64) {
    #[repr(C)]
    struct Timespec {
        s: i64,
        ns: i64,
    }
    #[repr(C)]
    struct Rusage {
        f: [i64; 18],
    }
    unsafe extern "C" {
        fn clock_gettime(clock: i32, t: *mut Timespec) -> i32;
        fn getrusage(who: i32, u: *mut Rusage) -> i32;
    }
    let mut t = Timespec { s: 0, ns: 0 };
    let mut u = Rusage { f: [0; 18] };
    // CLOCK_THREAD_CPUTIME_ID, RUSAGE_THREAD; ru_nvcsw is the 17th field
    unsafe {
        clock_gettime(3, &mut t);
        getrusage(1, &mut u);
    }
    (t.s as u64 * 1_000_000_000 + t.ns as u64, u.f[16])
}
#[cfg(not(target_os = "linux"))]
fn thread_use() -> (u64, i64) {
    (0, 0)
}

fn rt_loop() {
    let linux = cfg!(target_os = "linux");
    let mut i = 0u64;
    loop {
        // (CPU time does not run while the thread is preempted, wherever that lands)
        let (cpu0, vol0) = thread_use();
        let t = Instant::now();
        let v = voice(i);
        let ns = t.elapsed().as_nanos() as u64;
        let (cpu1, vol1) = thread_use();
        SINK.fetch_xor(v, Ordering::Relaxed);
        let own = if linux && vol1 == vol0 { cpu1.saturating_sub(cpu0).min(ns) } else { ns };
        MAX_NS.fetch_max(own, Ordering::SeqCst);
        MAX_WALL_NS.fetch_max(ns, Ordering::SeqCst);
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
        MAX_WALL_NS.store(0, Ordering::SeqCst);
        return;
    }
    wait_iters(100_000);
    let max = MAX_NS.swap(0, Ordering::SeqCst);
    let wall = MAX_WALL_NS.swap(0, Ordering::SeqCst);
    println!("out voice7 {}", LAST7.load(Ordering::SeqCst));
    if max < 5_000_000 {
        println!("out rt ok");
    } else {
        println!("out rt stall {} us", max / 1000);
    }
    println!("rt longest call {} us, {} us of it its own", wall / 1000, max / 1000);
}
