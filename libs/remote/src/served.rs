//! The CPU an endpoint's threads run serving its peers: reading and writing their frames,
//! answering their commands (an app's UI thread too, for the commands it answers:
//! [`charge_served`]), following their feeds. In an app it is what observing the app costs
//! it, not the app's own work: a test's idle rule leaves it out. And the CPU a client's
//! connection threads run carrying its own asks of a peer ([`asked_cpu_ns`]): in a test, what
//! observing the app costs the test process, not the work of a service it runs. And the CPU the
//! runtime that runs the program has run ([`runtime_cpu_ns`]), none of it the program's work.
//!
//! Under `makepad run` the served count and the runtime's CPU are the loader's, which it gives the
//! program it runs as symbols found at their first use (`relax_served_ns`, `relax_runtime_cpu_ns`,
//! `relax_runtime_waited_ns`): its endpoint serves the program, and every copy of this crate the
//! program runs counts with it, whichever provided crates bind. A rustc build has no loader: its count is this crate's own, and
//! no runtime runs beside the program.

use std::cell::Cell;
use std::ffi::CStr;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::OnceLock;

static SERVED_NS: AtomicU64 = AtomicU64::new(0);
static ASKED_NS: AtomicU64 = AtomicU64::new(0);

/// The served count: the loader's (`relax_served_ns`, the loader's own copy of this count), or in a
/// process with no loader this copy's.
fn served() -> &'static AtomicU64 {
    static AT: OnceLock<usize> = OnceLock::new();
    let at = *AT.get_or_init(|| match loader_symbol(c"relax_served_ns") {
        0 => &SERVED_NS as *const AtomicU64 as usize,
        at => at,
    });
    // SAFETY: the address of this count, or of the loader's copy of it, a static of the process
    unsafe { &*(at as *const AtomicU64) }
}

/// The served count a loader gives the programs it runs (`relax_served_ns`): its own.
pub fn served_count() -> &'static AtomicU64 {
    served()
}

/// The CPU the threads of the runtime that runs this program have run, in nanoseconds: under
/// `makepad run`, the JIT's own (its compiles, checks and monitor), which the loader reads
/// (`relax_runtime_cpu_ns`). None of it is the program's work: a test's idle rule leaves it out. With
/// no loader, 0: every thread is the program's.
pub fn runtime_cpu_ns() -> u64 {
    static READ: OnceLock<usize> = OnceLock::new();
    match *READ.get_or_init(|| loader_symbol(c"relax_runtime_cpu_ns")) {
        0 => 0,
        // SAFETY: the loader's `extern "C" fn() -> u64` of that name
        read => unsafe { std::mem::transmute::<usize, extern "C" fn() -> u64>(read)() },
    }
}

/// What the runtime's threads that run now have spent runnable, waiting for a core, in nanoseconds:
/// under `makepad run`, the JIT's own (`relax_runtime_waited_ns`), CPU none of the program wanted,
/// which a test's idle rule takes out of what the app's threads wanted. With no loader, 0.
pub fn runtime_waited_ns() -> u64 {
    static READ: OnceLock<usize> = OnceLock::new();
    match *READ.get_or_init(|| loader_symbol(c"relax_runtime_waited_ns")) {
        0 => 0,
        // SAFETY: the loader's `extern "C" fn() -> u64` of that name
        read => unsafe { std::mem::transmute::<usize, extern "C" fn() -> u64>(read)() },
    }
}

/// Whether a thread of this name (as the OS keeps it: Linux cuts it to 15 bytes) is one of this
/// crate's: its serving and connection threads, all named `makepad-remote-*`. What they run and
/// wait for is observing a process, never its own work.
pub fn own_thread(name: &str) -> bool {
    name.starts_with("makepad-remote")
}

/// A symbol the loader provides to the programs it runs, or 0 in a process with no loader:
/// dlsym's default scope on unix; on Windows GetProcAddress on the process's own module, which
/// makepad run answers from its loader symbols (a PE exe exports none).
fn loader_symbol(name: &CStr) -> usize {
    #[cfg(unix)]
    {
        extern "C" {
            fn dlsym(handle: *mut std::ffi::c_void, name: *const std::ffi::c_char) -> *mut std::ffi::c_void;
        }
        #[cfg(target_vendor = "apple")]
        let default = -2isize as *mut std::ffi::c_void;
        #[cfg(not(target_vendor = "apple"))]
        let default = std::ptr::null_mut();
        // SAFETY: a C string and the default handle
        unsafe { dlsym(default, name.as_ptr()) as usize }
    }
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        extern "system" {
            fn GetModuleHandleW(name: *const u16) -> *mut std::ffi::c_void;
            fn GetProcAddress(module: *mut std::ffi::c_void, name: *const std::ffi::c_char) -> *mut std::ffi::c_void;
        }
        // SAFETY: the process's own module and a C string
        unsafe { GetProcAddress(GetModuleHandleW(std::ptr::null()), name.as_ptr()) as usize }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = name;
        0
    }
}

thread_local! {
    /// The calling thread's CPU at its last [`tick`] (nothing, at its start).
    static LAST: Cell<u64> = Cell::new(0);
}

/// The CPU the process's serving threads have run to their last ticks, in nanoseconds.
pub fn served_cpu_ns() -> u64 {
    served().load(Relaxed)
}

/// Counts `ns` as served: what a thread that is not a serving one ran answering a peer's command
/// (an app's UI thread answering an ask), measured by that thread around its answer.
pub fn charge_served(ns: u64) {
    served().fetch_add(ns, Relaxed);
}

/// The CPU the process's client connection threads (a connection that serves nothing: it
/// carries this process's asks of its peer and their answers) have run to their last ticks, in
/// nanoseconds.
pub fn asked_cpu_ns() -> u64 {
    ASKED_NS.load(Relaxed)
}

/// Counts what the calling connection thread has run since its last tick (since it started, at
/// the first), as served (`serves`) or asked: a connection thread ticks after each piece of
/// work, so the count follows its CPU.
pub(crate) fn tick(serves: bool) {
    let now = thread_cpu_ns();
    let ran = now.saturating_sub(LAST.replace(now));
    let count = if serves { served() } else { &ASKED_NS };
    count.fetch_add(ran, Relaxed);
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
