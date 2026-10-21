//! Relax's shared low-level back end: RIR ([`rir`], including its vector registers and ops), the
//! register allocator ([`regalloc`]), the x86-64 and AArch64 machine-code emitters ([`x64`],
//! [`arm64`]), jump-table lowering ([`switch`]), the C ABI planner ([`cabi`]) and executable
//! memory ([`exec`]), the passes every optimising tier runs ([`passes`]), with the dominator and block-order ([`dom`]), frame-layout ([`frame`]) and bit-op
//! ([`bitops`]) analyses the emitters run. Front ends: Relax's Rust (relax-codegen optimises
//! it first) and Splash kernels. [`probe`] lets a front end observe compile phases.

pub mod arm64;
pub mod asm;
pub mod bitops;
pub mod cabi;
pub mod dom;
pub mod exec;
pub mod frame;
pub mod legal;
pub mod merge;
pub mod pair256;
pub mod passes;
pub mod probe;
pub mod regalloc;
pub mod rir;
pub mod stats;
pub mod switch;
pub mod wasm;
pub mod x64;

/// CPU time of the calling thread in ns (compile-time stats robust to machine load).
#[cfg(unix)]
pub fn cpu_ns() -> u64 {
    #[repr(C)]
    struct Timespec {
        sec: i64,
        nsec: i64,
    }
    extern "C" {
        fn clock_gettime(clk: i32, ts: *mut Timespec) -> i32;
    }
    #[cfg(any(target_os = "linux", target_os = "android"))]
    const CLOCK_THREAD_CPUTIME_ID: i32 = 3;
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    const CLOCK_THREAD_CPUTIME_ID: i32 = 16;
    let mut ts = Timespec { sec: 0, nsec: 0 };
    unsafe {
        clock_gettime(CLOCK_THREAD_CPUTIME_ID, &mut ts);
    }
    ts.sec as u64 * 1_000_000_000 + ts.nsec as u64
}

/// Estimated thread CPU ns on Windows, from scheduled thread cycles. Unlike GetThreadTimes,
/// QueryThreadCycleTime resolves short passes. A single calibration against QPC (Instant on
/// Windows) keeps all threads and all duration differences on the same scale. This is an
/// estimate on hardware whose cycle rate varies with frequency/core; scheduling/deadlines
/// must use wall time, never this clock.
#[cfg(windows)]
pub fn cpu_ns() -> u64 {
    use relax_session::win32::{GetCurrentThread, GetThreadTimes, QueryThreadCycleTime};
    static NS_PER_CYCLE: std::sync::OnceLock<Option<f64>> = std::sync::OnceLock::new();
    let scale = NS_PER_CYCLE.get_or_init(|| unsafe {
        let thread = GetCurrentThread();
        let mut best = f64::INFINITY;
        // Take the least interrupted of three 1 ms busy intervals. A deschedule increases
        // QPC elapsed time without adding thread cycles; using the minimum limits that bias.
        for _ in 0..3 {
            let mut start = 0;
            if QueryThreadCycleTime(thread, &mut start) == 0 {
                return None;
            }
            let t = std::time::Instant::now();
            while t.elapsed().as_nanos() < 1_000_000 {
                std::hint::spin_loop();
            }
            let mut end = 0;
            if QueryThreadCycleTime(thread, &mut end) == 0 || end <= start {
                return None;
            }
            best = best.min(t.elapsed().as_nanos() as f64 / (end - start) as f64);
        }
        Some(best)
    });
    unsafe {
        if let Some(scale) = scale {
            let mut cycles = 0;
            // The pseudo handle always refers to this live thread. Do not switch clock
            // origins mid-build if an OS call fails.
            assert_ne!(QueryThreadCycleTime(GetCurrentThread(), &mut cycles), 0);
            (cycles as f64 * scale) as u64
        } else {
            let (mut c, mut e, mut k, mut u) = (0u64, 0u64, 0u64, 0u64);
            GetThreadTimes(GetCurrentThread(), &mut c, &mut e, &mut k, &mut u);
            (k + u) * 100
        }
    }
}
