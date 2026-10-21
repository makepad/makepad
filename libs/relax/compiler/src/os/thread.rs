//! Threads: ids (what the unit lock records as owner), names, stack bounds, the watchdog's
//! interrupt, priorities, fork hooks.
//!
//! - `current()`: the calling thread's id (nonzero; pthread_t on unix).
//! - `name(t)`: a thread's name for diagnostics.
//! - `kick(t)`: interrupts thread `t` for the hang watchdog / MAKEPAD_RUN_COMPILE_WATCH: the fault
//!   layer's handler then runs on that thread (jit::on_watchdog decides what it does).
//! - `lower_priority(utility)`: background compile workers below the program's threads.
//! - `background()`: the OS background class (aot-min's compile workers: macOS QOS_CLASS_BACKGROUND,
//!   Linux SCHED_IDLE, Windows EcoQoS).
//! - `interactive()`: the sampler thread asks not to have its timers coalesced.
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

    /// A thread's name (pthread), for diagnostics.
    pub fn name(t: u64) -> String {
        let mut b = [0u8; 64];
        unsafe { pthread_getname_np(t as usize, b.as_mut_ptr(), b.len()) };
        let n = b.iter().position(|&c| c == 0).unwrap_or(0);
        String::from_utf8_lossy(&b[..n]).into_owned()
    }

    pub fn kick(t: u64) {
        unsafe {
            pthread_kill(t as usize, crate::os::fault::watchdog_signal());
        }
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
    pub use super::unix::{current, exit_now, fork, kick, name};

    pub fn lower_priority(_utility: bool) {
        extern "C" {
            fn setpriority(which: i32, who: u32, prio: i32) -> i32;
        }
        // who 0: the calling thread (Linux nices threads one by one)
        unsafe {
            setpriority(0, 0, 5);
        }
    }

    /// The OS background class for this thread: SCHED_IDLE (runs only when a CPU is otherwise idle).
    pub fn background() {
        #[repr(C)]
        struct SchedParam { priority: i32 }
        extern "C" {
            fn sched_setscheduler(pid: i32, policy: i32, param: *const SchedParam) -> i32;
        }
        const SCHED_IDLE: i32 = 5;
        // pid 0: the calling thread
        unsafe {
            sched_setscheduler(0, SCHED_IDLE, &SchedParam { priority: 0 });
        }
    }

    pub fn interactive() {}

    pub fn realtime() -> bool {
        extern "C" {
            fn sched_getscheduler(pid: i32) -> i32;
        }
        // SCHED_FIFO 1, SCHED_RR 2 (pid 0: the calling thread)
        matches!(unsafe { sched_getscheduler(0) }, 1 | 2)
    }
}

#[cfg(target_os = "macos")]
mod imp {
    pub use super::unix::{current, exit_now, fork, kick, name};
    use super::unix::pthread_self;

    extern "C" {
        fn pthread_set_qos_class_self_np(qos_class: u32, relative_priority: i32) -> i32;
    }

    pub fn lower_priority(utility: bool) {
        const QOS_CLASS_DEFAULT: u32 = 0x15;
        const QOS_CLASS_UTILITY: u32 = 0x11;
        unsafe {
            if utility {
                pthread_set_qos_class_self_np(QOS_CLASS_UTILITY, 0);
            } else {
                pthread_set_qos_class_self_np(QOS_CLASS_DEFAULT, -15);
            }
        }
    }

    /// The OS background class for this thread: QOS_CLASS_BACKGROUND (efficiency cores,
    /// throttled, low power).
    pub fn background() {
        const QOS_CLASS_BACKGROUND: u32 = 0x09;
        unsafe {
            pthread_set_qos_class_self_np(QOS_CLASS_BACKGROUND, 0);
        }
    }

    /// macOS coalesces the timers of background-QoS threads (a process started from a daemon or
    /// a script): a 2 ms sleep then lasts tens of ms. Interactive QoS has no coalescing.
    pub fn interactive() {
        unsafe {
            pthread_set_qos_class_self_np(0x21, 0); // QOS_CLASS_USER_INTERACTIVE
        }
    }

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


    /// The watchdog / MAKEPAD_RUN_COMPILE_WATCH interrupt: the thread is suspended and its context read
    /// from this (the watching) thread; the fault layer decides (os::fault::interrupt).
    pub fn kick(t: u64) {
        crate::os::fault::interrupt(t);
    }

    pub fn lower_priority(_utility: bool) {
        const THREAD_PRIORITY_BELOW_NORMAL: i32 = -1;
        unsafe {
            SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
        }
    }

    /// The OS background class for this thread: EcoQoS (execution-speed power throttling) at idle
    /// priority.
    pub fn background() {
        #[repr(C)]
        struct ThreadPowerThrottlingState { version: u32, control_mask: u32, state_mask: u32 }
        const THREAD_POWER_THROTTLING: i32 = 3;
        const THREAD_POWER_THROTTLING_EXECUTION_SPEED: u32 = 1;
        const THREAD_PRIORITY_IDLE: i32 = -15;
        let state = ThreadPowerThrottlingState { version: 1, control_mask: THREAD_POWER_THROTTLING_EXECUTION_SPEED, state_mask: THREAD_POWER_THROTTLING_EXECUTION_SPEED };
        unsafe {
            SetThreadInformation(GetCurrentThread(), THREAD_POWER_THROTTLING, &state as *const _ as *const u8, std::mem::size_of::<ThreadPowerThrottlingState>() as u32);
            SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_IDLE);
        }
    }

    pub fn interactive() {}

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
