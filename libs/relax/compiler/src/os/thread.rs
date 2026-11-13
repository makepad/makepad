//! Threads: ids (what the unit lock records as owner), names, stack bounds, the watchdog's
//! interrupt, realtime policy queries, fork hooks.
//!
//! - `current()`: the calling thread's id (nonzero; pthread_t on unix).
//! - `name(t)`: a thread's name for diagnostics.
//! - `kick(t)`: interrupts thread `t` for the hang watchdog / RELAX_COMPILE_WATCH: the fault
//!   layer's handler then runs on that thread (jit::on_watchdog decides what it does).
//! - `realtime()`: the calling thread runs under a real-time policy (an audio callback thread:
//!   SCHED_FIFO/RR on Linux, a time-constraint policy on macOS).
//! - `fork()`: the C library's fork (unix), for the fork the loader hands to programs.
//! - `exit_now(code)`: ends the process at once (no atexit handlers, no unwinding).

pub use imp::*;

#[cfg(unix)]
mod unix {
    extern "C" {
        pub fn pthread_self() -> usize;
        fn pthread_kill(t: usize, sig: i32) -> i32;
        fn pthread_getname_np(t: usize, name: *mut u8, len: usize) -> i32;
        #[link_name = "fork"]
        fn libc_fork() -> i32;
        fn _exit(code: i32) -> !;
    }

    pub fn current() -> u64 {
        unsafe { pthread_self() as u64 }
    }

    fn own_name(b: &mut [u8; 64]) -> usize {
        unsafe { pthread_getname_np(pthread_self(), b.as_mut_ptr(), b.len()) };
        b.iter().position(|&c| c == 0).unwrap_or(0)
    }

    /// Names of the threads that noted themselves (`note_current`), by handle. Another thread's
    /// name is read from here and never through its handle: `pthread_getname_np` of a thread that
    /// has ended reads its freed descriptor (glibc: `pd->tid` in the unmapped stack) -- the x64
    /// UI-thread segfault in `pthread_getname_np` when a main-stall line named the thread whose
    /// owner request the main thread had waited behind, after that thread ended. Fixed slots and
    /// a spin lock: no allocation, any phase. A handle reused by a new thread is noted again.
    mod names {
        use std::sync::atomic::{AtomicBool, Ordering};
        const N: usize = 1024;
        pub const LEN: usize = 32;
        struct Table {
            lock: AtomicBool,
            next: std::cell::UnsafeCell<usize>,
            slots: std::cell::UnsafeCell<[(u64, [u8; LEN]); N]>,
        }
        unsafe impl Sync for Table {}
        static TABLE: Table = Table { lock: AtomicBool::new(false), next: std::cell::UnsafeCell::new(0), slots: std::cell::UnsafeCell::new([(0, [0; LEN]); N]) };
        fn with<R>(f: impl FnOnce(&mut usize, &mut [(u64, [u8; LEN]); N]) -> R) -> R {
            while TABLE.lock.compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
                std::hint::spin_loop();
            }
            let r = f(unsafe { &mut *TABLE.next.get() }, unsafe { &mut *TABLE.slots.get() });
            TABLE.lock.store(false, Ordering::Release);
            r
        }
        pub fn put(t: u64, name: &[u8]) {
            let mut b = [0u8; LEN];
            let k = name.len().min(LEN);
            b[..k].copy_from_slice(&name[..k]);
            with(|next, slots| match slots.iter().position(|s| s.0 == t) {
                Some(i) => slots[i].1 = b,
                None => {
                    // (the oldest entry goes when the table is full: its thread most likely ended)
                    slots[*next % N] = (t, b);
                    *next += 1;
                }
            });
        }
        pub fn get(t: u64) -> Option<[u8; LEN]> {
            with(|_, slots| slots.iter().find(|s| s.0 == t).map(|s| s.1))
        }
    }

    thread_local! {
        static NOTED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }

    /// Records the calling thread's name for `name` (once per thread; one thread-local read after).
    #[inline]
    pub fn note_current() {
        if NOTED.try_with(|n| n.replace(true)).unwrap_or(true) {
            return;
        }
        let mut b = [0u8; 64];
        let n = own_name(&mut b);
        names::put(current(), &b[..n]);
    }

    /// A thread's name, for diagnostics: the calling thread's own, or what a thread noted
    /// (`note_current`) while it ran ("thread <handle>" when it never did).
    pub fn name(t: u64) -> String {
        if t == current() {
            let mut b = [0u8; 64];
            let n = own_name(&mut b);
            return String::from_utf8_lossy(&b[..n]).into_owned();
        }
        match names::get(t) {
            Some(b) => {
                let n = b.iter().position(|&c| c == 0).unwrap_or(names::LEN);
                String::from_utf8_lossy(&b[..n]).into_owned()
            }
            None => format!("thread {t:#x}"),
        }
    }

    pub fn kick(t: u64) {
        unsafe {
            pthread_kill(t as usize, crate::os::fault::watchdog_signal());
        }
    }

    /// A run's end: ends the system call thread `t` is waiting in (EINTR: no handler restarts one).
    /// The thread itself does the rest.
    pub fn end_kick(t: u64, _waiting: usize) {
        kick(t);
    }

    /// (`end_kick`'s second argument: only Windows needs it)
    pub fn waiting_cell() -> usize {
        0
    }

    pub fn fork() -> i32 {
        unsafe { libc_fork() }
    }

    pub fn exit_now(code: i32) -> ! {
        unsafe { _exit(code) }
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
mod imp {
    pub use super::unix::{current, end_kick, waiting_cell, exit_now, fork, kick, name, note_current};

    pub fn realtime() -> bool {
        extern "C" {
            fn sched_getscheduler(pid: i32) -> i32;
        }
        // SCHED_FIFO 1, SCHED_RR 2 (pid 0: the calling thread)
        matches!(unsafe { sched_getscheduler(0) }, 1 | 2)
    }
}

#[cfg(target_vendor = "apple")]
mod imp {
    pub use super::unix::{current, end_kick, waiting_cell, exit_now, fork, kick, name, note_current};
    use super::unix::pthread_self;

    /// A thread with a time-constraint policy (CoreAudio's IO threads, audio workgroups).
    pub fn realtime() -> bool {
        extern "C" {
            fn pthread_mach_thread_np(t: usize) -> u32;
            fn thread_policy_get(thread: u32, flavor: u32, info: *mut u32, count: *mut u32, get_default: *mut u32) -> i32;
        }
        const THREAD_TIME_CONSTRAINT_POLICY: u32 = 2;
        let mut info = [0u32; 4];
        let mut count = 4u32;
        let mut get_default = 0u32;
        let r = unsafe { thread_policy_get(pthread_mach_thread_np(pthread_self()), THREAD_TIME_CONSTRAINT_POLICY, info.as_mut_ptr(), &mut count, &mut get_default) };
        r == 0 && get_default == 0
    }
}

#[cfg(windows)]
mod imp {
    use crate::os::win32::*;

    pub fn current() -> u64 {
        unsafe { GetCurrentThreadId() as u64 }
    }

    /// (thread ids name ended threads safely: nothing to record)
    #[inline]
    pub fn note_current() {}

    /// A thread's description (SetThreadDescription; std names its threads with it).
    pub fn name(t: u64) -> String {
        unsafe {
            let h = OpenThread(THREAD_QUERY_LIMITED_INFORMATION, 0, t as u32);
            if h == 0 {
                return String::new();
            }
            let mut p: *mut u16 = std::ptr::null_mut();
            let mut s = String::new();
            if GetThreadDescription(h, &mut p) >= 0 && !p.is_null() {
                let mut n = 0;
                while *p.add(n) != 0 {
                    n += 1;
                }
                s = String::from_utf16_lossy(std::slice::from_raw_parts(p, n));
                LocalFree(p as *mut u8);
            }
            CloseHandle(h);
            s
        }
    }

    /// The watchdog / RELAX_COMPILE_WATCH interrupt: the thread is suspended and its context read
    /// from this (the watching) thread; the fault layer decides (os::fault::interrupt).
    pub fn kick(t: u64) {
        crate::os::fault::interrupt(t);
    }

    /// A run's end: ends the wait thread `t` is in (os::fault::end_interrupt; `waiting`: its
    /// `waiting_cell`).
    pub fn end_kick(t: u64, waiting: usize) {
        crate::os::fault::end_interrupt(t, waiting)
    }

    /// Where the calling thread says what it waits on, for `end_kick`.
    pub fn waiting_cell() -> usize {
        crate::domain_threads::waits::cell()
    }

    /// A time-critical thread (WASAPI render threads run at THREAD_PRIORITY_TIME_CRITICAL or
    /// in an MMCSS task, which raises the thread into the real-time range).
    pub fn realtime() -> bool {
        const THREAD_PRIORITY_TIME_CRITICAL: i32 = 15;
        unsafe { GetThreadPriority(GetCurrentThread()) >= THREAD_PRIORITY_TIME_CRITICAL }
    }

    pub fn exit_now(code: i32) -> ! {
        unsafe {
            TerminateProcess(GetCurrentProcess(), code as u32);
        }
        std::process::abort()
    }
}
