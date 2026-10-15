//! The HAL's one clock: raw monotonic nanoseconds, absolute (not deltas).

#[cfg(target_vendor = "apple")]
pub fn raw_ns() -> u64 {
    extern "C" {
        fn clock_gettime_nsec_np(clock_id: u32) -> u64;
    }
    const CLOCK_UPTIME_RAW: u32 = 8;
    // SAFETY: a libSystem call with no pointer arguments
    unsafe { clock_gettime_nsec_np(CLOCK_UPTIME_RAW) }
}

#[cfg(all(unix, not(target_vendor = "apple")))]
pub fn raw_ns() -> u64 {
    #[repr(C)]
    struct Timespec {
        sec: i64,
        nsec: i64,
    }
    extern "C" {
        fn clock_gettime(clock_id: i32, ts: *mut Timespec) -> i32;
    }
    const CLOCK_MONOTONIC: i32 = 1;
    let mut ts = Timespec { sec: 0, nsec: 0 };
    // SAFETY: `ts` is a valid timespec for the call to fill
    unsafe { clock_gettime(CLOCK_MONOTONIC, &mut ts) };
    ts.sec as u64 * 1_000_000_000 + ts.nsec as u64
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

#[cfg(not(any(unix, windows)))]
pub fn raw_ns() -> u64 {
    use std::sync::OnceLock;
    use std::time::Instant;
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_nanos() as u64 + 1
}
