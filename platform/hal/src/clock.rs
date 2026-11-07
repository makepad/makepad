//! The HAL's one clock: raw monotonic nanoseconds, absolute (not deltas). And CPU time, of the
//! calling thread or of the process: what they ran, which a loaded box does not move (a thread
//! that waits for a core, or for anything else, spends none). And the CPU the process ran
//! serving its remote peers, which observers cost the app.

/// `clock_gettime_nsec_np` of `clock_id`.
#[cfg(target_vendor = "apple")]
fn clock_ns(clock_id: u32) -> u64 {
    extern "C" {
        fn clock_gettime_nsec_np(clock_id: u32) -> u64;
    }
    // SAFETY: a libSystem call with no pointer arguments
    unsafe { clock_gettime_nsec_np(clock_id) }
}

#[cfg(target_vendor = "apple")]
pub fn raw_ns() -> u64 {
    const CLOCK_UPTIME_RAW: u32 = 8;
    clock_ns(CLOCK_UPTIME_RAW)
}

/// CPU time of the calling thread, in nanoseconds.
#[cfg(target_vendor = "apple")]
pub fn thread_cpu_ns() -> u64 {
    const CLOCK_THREAD_CPUTIME_ID: u32 = 16;
    clock_ns(CLOCK_THREAD_CPUTIME_ID)
}

/// CPU time of the process, all its threads, in nanoseconds.
#[cfg(target_vendor = "apple")]
pub fn process_cpu_ns() -> u64 {
    const CLOCK_PROCESS_CPUTIME_ID: u32 = 12;
    clock_ns(CLOCK_PROCESS_CPUTIME_ID)
}

/// `clock_gettime` of `clock_id` in nanoseconds.
#[cfg(all(unix, not(target_vendor = "apple")))]
fn clock_ns(clock_id: i32) -> u64 {
    #[repr(C)]
    struct Timespec {
        sec: std::ffi::c_long,
        nsec: std::ffi::c_long,
    }
    extern "C" {
        fn clock_gettime(clock_id: i32, ts: *mut Timespec) -> i32;
    }
    let mut ts = Timespec { sec: 0, nsec: 0 };
    // SAFETY: `ts` is a valid timespec for the call to fill
    unsafe { clock_gettime(clock_id, &mut ts) };
    ts.sec as u64 * 1_000_000_000 + ts.nsec as u64
}

#[cfg(all(unix, not(target_vendor = "apple")))]
pub fn raw_ns() -> u64 {
    const CLOCK_MONOTONIC: i32 = 1;
    clock_ns(CLOCK_MONOTONIC)
}

/// CPU time of the calling thread, in nanoseconds.
#[cfg(all(unix, not(target_vendor = "apple")))]
pub fn thread_cpu_ns() -> u64 {
    const CLOCK_THREAD_CPUTIME_ID: i32 = 3;
    clock_ns(CLOCK_THREAD_CPUTIME_ID)
}

/// CPU time of the process, all its threads, in nanoseconds.
#[cfg(all(unix, not(target_vendor = "apple")))]
pub fn process_cpu_ns() -> u64 {
    const CLOCK_PROCESS_CPUTIME_ID: i32 = 2;
    clock_ns(CLOCK_PROCESS_CPUTIME_ID)
}

/// Windows: QueryPerformanceCounter as absolute nanoseconds (the epoch DXGI frame statistics
/// and input message times share), with the frequency read once and 128-bit arithmetic.
#[cfg(windows)]
pub fn raw_ns() -> u64 {
    use makepad_windows_sys::Win32::System::Performance::QueryPerformanceCounter;
    let mut t = 0i64;
    // SAFETY: a valid out pointer
    let _ = unsafe { QueryPerformanceCounter(&mut t) };
    qpc_to_ns(t)
}

/// A QueryPerformanceCounter value (such as DXGI's `SyncQPCTime`) on the HAL clock. An op under
/// `makepad run`: the frequency is a static of this crate, read where its one copy lives.
#[cfg(windows)]
#[makepad_relax_api::relax_api]
pub fn qpc_to_ns(qpc: i64) -> u64 {
    use makepad_windows_sys::Win32::System::Performance::QueryPerformanceFrequency;
    use std::sync::OnceLock;
    static FREQ: OnceLock<u64> = OnceLock::new();
    let freq = *FREQ.get_or_init(|| {
        let mut f = 0i64;
        // SAFETY: a valid out pointer; QPF cannot fail on Windows XP and later
        let _ = unsafe { QueryPerformanceFrequency(&mut f) };
        f.max(1) as u64
    });
    ((qpc.max(0) as u128 * 1_000_000_000) / freq as u128) as u64
}

/// A kernel and a user time of `GetThreadTimes` / `GetProcessTimes` together, in nanoseconds.
#[cfg(windows)]
fn filetimes_ns(times: [makepad_windows_sys::Win32::Foundation::FILETIME; 2]) -> u64 {
    times.iter().map(|t| (((t.dwHighDateTime as u64) << 32) | t.dwLowDateTime as u64) * 100).sum()
}

/// CPU time of the calling thread, in nanoseconds (Windows counts it in scheduler ticks).
#[cfg(windows)]
pub fn thread_cpu_ns() -> u64 {
    use makepad_windows_sys::Win32::Foundation::{CloseHandle, FILETIME};
    use makepad_windows_sys::Win32::System::Threading::{GetCurrentThreadId, GetThreadTimes, OpenThread, THREAD_QUERY_LIMITED_INFORMATION};
    // SAFETY: the handle is this thread's own, closed after the one call; valid out pointers
    unsafe {
        let Ok(thread) = OpenThread(THREAD_QUERY_LIMITED_INFORMATION, false, GetCurrentThreadId()) else { return 0 };
        let (mut created, mut exited, mut kernel, mut user) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
        let times = GetThreadTimes(thread, &mut created, &mut exited, &mut kernel, &mut user);
        let _ = CloseHandle(thread);
        times.map_or(0, |()| filetimes_ns([kernel, user]))
    }
}

/// CPU time of the process, all its threads, in nanoseconds (Windows counts it in scheduler
/// ticks).
#[cfg(windows)]
pub fn process_cpu_ns() -> u64 {
    use makepad_windows_sys::Win32::Foundation::FILETIME;
    use makepad_windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
    let (mut created, mut exited, mut kernel, mut user) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
    // SAFETY: the process's pseudo handle; valid out pointers
    let times = unsafe { GetProcessTimes(GetCurrentProcess(), &mut created, &mut exited, &mut kernel, &mut user) };
    times.map_or(0, |()| filetimes_ns([kernel, user]))
}

/// The CPU the process's threads have run serving its remote peers (makepad-remote's count:
/// reading and writing their frames, answering their commands, following their feeds), in
/// nanoseconds: what observing the app costs it, not its own work. Under `makepad run`, the
/// host's: its endpoint serves the program.
#[cfg(any(unix, windows))]
#[makepad_relax_api::relax_api]
pub fn served_cpu_ns() -> u64 {
    makepad_remote::served_cpu_ns()
}

#[cfg(not(any(unix, windows)))]
pub fn raw_ns() -> u64 {
    use std::sync::OnceLock;
    use std::time::Instant;
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_nanos() as u64 + 1
}
