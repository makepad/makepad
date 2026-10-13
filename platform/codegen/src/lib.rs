//! Makepad's one middle and back end (local/plans/loader-core.md, step 6). Front ends lower
//! into RIR ([`rir`]): Rapid's Rust dialect and Splash kernels (makepad-script-compute).
//! [`opt`] is the tier-0 optimiser, [`regalloc`] the shared register allocator, [`x64`] and
//! [`arm64`] the machine-code emitters, [`cabi`] the C ABI planner and [`exec`] executable
//! memory for standalone code (kernels; Rapid's live image manages its own).

pub mod arm64;
pub mod cabi;
pub mod exec;
pub mod opt;
pub mod regalloc;
pub mod rir;
pub mod x64;

#[cfg(unix)]
#[repr(C)]
struct Timespec {
    sec: i64,
    nsec: i64,
}

#[cfg(unix)]
extern "C" {
    fn clock_gettime(clk: i32, ts: *mut Timespec) -> i32;
}

#[cfg(target_os = "linux")]
const CLOCK_THREAD_CPUTIME_ID: i32 = 3;
#[cfg(all(unix, not(target_os = "linux")))]
const CLOCK_THREAD_CPUTIME_ID: i32 = 16;

/// CPU time of the calling thread in ns (compile-time stats robust to machine load).
#[cfg(unix)]
pub fn cpu_ns() -> u64 {
    let mut ts = Timespec { sec: 0, nsec: 0 };
    unsafe {
        clock_gettime(CLOCK_THREAD_CPUTIME_ID, &mut ts);
    }
    ts.sec as u64 * 1_000_000_000 + ts.nsec as u64
}

/// CPU time of the calling thread in ns (GetThreadTimes: kernel + user, 100 ns units).
#[cfg(windows)]
pub fn cpu_ns() -> u64 {
    #[link(name = "kernel32")]
    extern "system" {
        fn GetCurrentThread() -> isize;
        fn GetThreadTimes(thread: isize, creation: *mut u64, exit: *mut u64, kernel: *mut u64, user: *mut u64) -> i32;
    }
    let (mut c, mut e, mut k, mut u) = (0u64, 0u64, 0u64, 0u64);
    unsafe {
        GetThreadTimes(GetCurrentThread(), &mut c, &mut e, &mut k, &mut u);
    }
    (k + u) * 100
}
