//! The CPU an endpoint's threads run serving its peers: reading and writing their frames,
//! answering their commands, following their feeds. In an app it is what observing the app costs
//! it, not the app's own work: a test's idle rule leaves it out.

use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

static SERVED_NS: AtomicU64 = AtomicU64::new(0);

thread_local! {
    /// The calling thread's CPU at its last [`tick`] (nothing, at its start).
    static LAST: Cell<u64> = Cell::new(0);
}

/// The CPU the process's serving threads have run to their last ticks, in nanoseconds.
pub fn served_cpu_ns() -> u64 {
    SERVED_NS.load(Relaxed)
}

/// Counts what the calling serving thread has run since its last tick (since it started, at the
/// first): a serving thread ticks after each piece of work, so the count follows its CPU.
pub(crate) fn tick() {
    let now = thread_cpu_ns();
    SERVED_NS.fetch_add(now.saturating_sub(LAST.replace(now)), Relaxed);
}

/// The calling thread's CPU, in nanoseconds.
#[cfg(target_vendor = "apple")]
fn thread_cpu_ns() -> u64 {
    extern "C" {
        fn clock_gettime_nsec_np(clock_id: u32) -> u64;
    }
    const CLOCK_THREAD_CPUTIME_ID: u32 = 16;
    // SAFETY: a libSystem call with no pointer arguments
    unsafe { clock_gettime_nsec_np(CLOCK_THREAD_CPUTIME_ID) }
}

/// The calling thread's CPU, in nanoseconds.
#[cfg(all(unix, not(target_vendor = "apple")))]
fn thread_cpu_ns() -> u64 {
    #[repr(C)]
    struct Timespec {
        sec: std::ffi::c_long,
        nsec: std::ffi::c_long,
    }
    extern "C" {
        fn clock_gettime(clock_id: i32, ts: *mut Timespec) -> i32;
    }
    const CLOCK_THREAD_CPUTIME_ID: i32 = 3;
    let mut ts = Timespec { sec: 0, nsec: 0 };
    // SAFETY: `ts` is a valid timespec for the call to fill
    unsafe { clock_gettime(CLOCK_THREAD_CPUTIME_ID, &mut ts) };
    ts.sec as u64 * 1_000_000_000 + ts.nsec as u64
}

/// The calling thread's CPU, in nanoseconds (Windows counts it in scheduler ticks).
#[cfg(windows)]
fn thread_cpu_ns() -> u64 {
    use windows::Win32::Foundation::{CloseHandle, FILETIME};
    use windows::Win32::System::Threading::{GetCurrentThreadId, GetThreadTimes, OpenThread, THREAD_QUERY_LIMITED_INFORMATION};
    // SAFETY: the handle is this thread's own, closed after the one call; valid out pointers
    unsafe {
        let Ok(thread) = OpenThread(THREAD_QUERY_LIMITED_INFORMATION, false, GetCurrentThreadId()) else { return 0 };
        let (mut created, mut exited, mut kernel, mut user) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
        let times = GetThreadTimes(thread, &mut created, &mut exited, &mut kernel, &mut user);
        let _ = CloseHandle(thread);
        times.map_or(0, |()| [kernel, user].iter().map(|t| (((t.dwHighDateTime as u64) << 32) | t.dwLowDateTime as u64) * 100).sum())
    }
}

/// No thread clock: nothing counts as served.
#[cfg(not(any(unix, windows)))]
fn thread_cpu_ns() -> u64 {
    0
}
