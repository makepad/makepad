//! Relax's middle and back end. The front end lowers into RIR ([`rir`]).
//! [`opt`] is the O1 optimiser, [`regalloc`] the shared register allocator, [`x64`] and
//! [`arm64`] the machine-code emitters, [`cabi`] the C ABI planner and [`exec`] executable
//! memory for standalone code (kernels; Relax's live image manages its own).

pub mod arm64;
pub mod cabi;
pub mod exec;
pub mod fncost;
pub mod interp;
pub mod exec_census;
pub mod warm;
pub mod loops;
pub mod opt;
pub mod ranges;
pub mod regalloc;
pub mod rir;
pub mod x64;

/// This crate has no movable compiler statics; opt::T remains a process-wide OnceLock.
pub struct ZoneTaken;

/// Takes this crate's compiler statics while every compiler thread is quiesced.
pub fn zone_take() -> ZoneTaken {
    // opt::T is a OnceLock<Option<String>>: shared statics cannot reset OnceLock safely.
    // MAKEPAD_RUN_TRACE_OPT's cached string therefore still needs kept allocation before swaps
    // are enabled with tracing; this reset deliberately does not initialise that lazy.
    #[cfg(test)]
    assert!(opt::T.get().is_none(), "trace_opt's OnceLock cannot be reset once initialised");
    ZoneTaken
}

pub fn zone_put_back(_: ZoneTaken) {}

#[test]
fn zone_reset_fresh_process() {
    const CHILD: &str = "MAKEPAD_RUN_ZONE_RESET_TEST_CHILD";
    if std::env::var_os(CHILD).is_some() {
        std::mem::forget(zone_take());
        std::mem::forget(zone_take());
        return;
    }
    // Other tests use these statics; the reset and its empty-state assertions need isolation.
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "zone_reset_fresh_process"])
        .env(CHILD, "1")
        .status().unwrap();
    assert!(status.success());
}

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
