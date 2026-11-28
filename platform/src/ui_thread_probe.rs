//! Sample the registered UI thread from the command waiter, without interrupting it.
use super::ThreadExecSample;
#[cfg(any(test, target_os = "linux", target_os = "android", target_vendor = "apple", windows))]
use super::ThreadExecState;

pub use os::{register_ui_thread, UiThreadProbe};

#[cfg(any(test, target_os = "linux", target_os = "android"))]
fn parse_linux_stat(bytes: &[u8], tick_ns: u64) -> Option<ThreadExecSample> {
    // comm can contain spaces, closing parentheses, and non-UTF-8 bytes.
    let end = bytes.iter().rposition(|&byte| byte == b')')?;
    let mut fields = std::str::from_utf8(&bytes[end + 1..]).ok()?.split_ascii_whitespace();
    let state = match fields.next()? {
        "R" => ThreadExecState::RunningOrRunnable,
        "D" => ThreadExecState::UninterruptibleIo,
        "S" | "I" => ThreadExecState::Sleeping,
        "T" | "t" => ThreadExecState::Stopped,
        "Z" | "X" | "x" => ThreadExecState::Exited,
        _ => ThreadExecState::Unknown,
    };
    let user: u64 = fields.nth(10)?.parse().ok()?;
    let system: u64 = fields.next()?.parse().ok()?;
    Some(ThreadExecSample { state, cpu_ns: user.checked_add(system)?.checked_mul(tick_ns), run_delay_ns: None })
}

#[cfg(any(test, target_os = "linux", target_os = "android"))]
fn parse_linux_schedstat(bytes: &[u8]) -> Option<(u64, u64)> {
    let mut fields = std::str::from_utf8(bytes).ok()?.split_ascii_whitespace();
    Some((fields.next()?.parse().ok()?, fields.next()?.parse().ok()?))
}

#[cfg(any(target_os = "linux", target_os = "android"))]
mod os {
    use super::*;
    use std::{ffi::c_long, fs::File, io::{self, Read}, sync::atomic::{AtomicU64, Ordering}};

    static UI_TID: AtomicU64 = AtomicU64::new(0);

    pub fn register_ui_thread() {
        UI_TID.store(makepad_hal::clock::thread_id(), Ordering::Release);
    }

    struct ProcFiles {
        tid: u64,
        stat: String,
        schedstat: String,
        tick_ns: u64,
    }

    impl ProcFiles {
        fn new(tid: u64) -> Self {
            unsafe extern "C" { fn sysconf(name: i32) -> c_long; }
            #[cfg(target_os = "linux")]
            const SC_CLK_TCK: i32 = 2;
            #[cfg(target_os = "android")]
            const SC_CLK_TCK: i32 = 6;
            // SAFETY: sysconf takes a selector, no pointers.
            let tick_ns = 1_000_000_000 / unsafe { sysconf(SC_CLK_TCK) }.max(1) as u64;
            Self {
                tid, tick_ns,
                stat: format!("/proc/self/task/{tid}/stat"),
                schedstat: format!("/proc/self/task/{tid}/schedstat"),
            }
        }
    }

    fn read<'a>(path: &str, bytes: &'a mut [u8]) -> io::Result<&'a [u8]> {
        let n = File::open(path)?.read(bytes)?;
        Ok(&bytes[..n])
    }

    fn thread_clock_ns(tid: u64) -> Option<u64> {
        #[repr(C)]
        struct Timespec { sec: c_long, nsec: c_long }
        unsafe extern "C" { fn clock_gettime(id: i32, time: *mut Timespec) -> i32; }
        let id = ((!tid as u32) << 3) as i32 | 6; // MAKE_THREAD_CPUCLOCK(tid, CPUCLOCK_SCHED)
        let mut time = Timespec { sec: 0, nsec: 0 };
        // SAFETY: time is a writable timespec for this call.
        (unsafe { clock_gettime(id, &mut time) } == 0)
            .then(|| time.sec as u64 * 1_000_000_000 + time.nsec as u64)
    }

    pub struct UiThreadProbe {
        local_tid: Option<u64>,
        files: Option<ProcFiles>,
    }

    impl UiThreadProbe {
        pub fn global() -> Self { Self { local_tid: None, files: None } }

        pub fn for_current_thread() -> Self {
            Self { local_tid: Some(makepad_hal::clock::thread_id()), files: None }
        }

        pub fn sample(&mut self) -> ThreadExecSample {
            let tid = self.local_tid.unwrap_or_else(|| UI_TID.load(Ordering::Acquire));
            if tid == 0 { return ThreadExecSample::unknown(); }
            if self.files.as_ref().is_none_or(|files| files.tid != tid) {
                self.files = Some(ProcFiles::new(tid));
            }
            let files = self.files.as_ref().unwrap();
            let mut stat = [0; 512];
            let mut sample = match read(&files.stat, &mut stat) {
                Ok(bytes) => match parse_linux_stat(bytes, files.tick_ns) {
                    Some(sample) => sample,
                    None => return ThreadExecSample::unknown(),
                },
                Err(err) if err.kind() == io::ErrorKind::NotFound => {
                    return ThreadExecSample { state: ThreadExecState::Exited, ..ThreadExecSample::unknown() };
                }
                Err(_) => return ThreadExecSample::unknown(),
            };
            let mut schedstat = [0; 128];
            if let Some((cpu, delay)) = read(&files.schedstat, &mut schedstat).ok().and_then(parse_linux_schedstat) {
                sample.cpu_ns = Some(cpu);
                sample.run_delay_ns = Some(delay);
            } else {
                sample.cpu_ns = thread_clock_ns(tid).or(sample.cpu_ns);
            }
            sample
        }
    }
}

// A probe keeps the original OS handle alive across replacement of the global registration.
#[cfg(any(target_vendor = "apple", windows))]
mod registered {
    use super::os::ThreadHandle;
    use std::sync::{Arc, Mutex};

    static UI_THREAD: Mutex<Option<Arc<ThreadHandle>>> = Mutex::new(None);

    pub fn register_ui_thread() {
        let new = ThreadHandle::current().map(Arc::new);
        let old = std::mem::replace(&mut *UI_THREAD.lock().unwrap_or_else(|p| p.into_inner()), new);
        drop(old);
    }

    pub(super) enum Target {
        Global,
        Captured(Option<Arc<ThreadHandle>>),
    }

    impl Target {
        pub(super) fn current() -> Self { Self::Captured(ThreadHandle::current().map(Arc::new)) }

        pub(super) fn get(&mut self) -> Option<&ThreadHandle> {
            if let Self::Global = self {
                let handle = UI_THREAD.lock().unwrap_or_else(|p| p.into_inner()).clone()?;
                *self = Self::Captured(Some(handle));
            }
            match self {
                Self::Captured(handle) => handle.as_deref(),
                Self::Global => None,
            }
        }
    }
}

#[cfg(target_vendor = "apple")]
mod os {
    use super::{registered::Target, ThreadExecSample, ThreadExecState};
    pub use super::registered::register_ui_thread;

    #[repr(C)]
    struct ThreadExtendedInfo {
        user: u64, system: u64, cpu_usage: i32, policy: i32, run_state: i32, flags: i32,
        sleep_time: i32, curpri: i32, priority: i32, maxpriority: i32, name: [u8; 64],
    }
    const _: () = assert!(std::mem::size_of::<ThreadExtendedInfo>() == 112);

    unsafe extern "C" {
        fn mach_thread_self() -> u32;
        static mach_task_self_: u32;
        fn mach_port_deallocate(task: u32, port: u32) -> i32;
        fn thread_info(thread: u32, flavor: u32, info: *mut i32, count: *mut u32) -> i32;
    }

    pub(super) struct ThreadHandle(u32);
    impl ThreadHandle {
        pub(super) fn current() -> Option<Self> {
            // SAFETY: the returned send right belongs to the caller.
            let port = unsafe { mach_thread_self() };
            (port != 0).then(|| Self(port))
        }
    }
    impl Drop for ThreadHandle {
        fn drop(&mut self) {
            // SAFETY: this object owns one send right in our task.
            unsafe { mach_port_deallocate(mach_task_self_, self.0); }
        }
    }

    pub struct UiThreadProbe(Target);
    impl UiThreadProbe {
        pub fn global() -> Self { Self(Target::Global) }

        pub fn for_current_thread() -> Self { Self(Target::current()) }

        pub fn sample(&mut self) -> ThreadExecSample {
            let Some(port) = self.0.get() else { return ThreadExecSample::unknown() };
            let mut count = 28;
            // SAFETY: this integer-only record is valid zeroed and holds 28 Mach words.
            let mut info: ThreadExtendedInfo = unsafe { std::mem::zeroed() };
            let rc = unsafe { thread_info(port.0, 5, (&mut info as *mut ThreadExtendedInfo).cast(), &mut count) };
            match rc {
                0 if count == 28 => ThreadExecSample {
                    state: match info.run_state {
                        1 => ThreadExecState::RunningOrRunnable,
                        2 => ThreadExecState::Stopped,
                        3 => ThreadExecState::Sleeping,
                        4 => ThreadExecState::UninterruptibleIo,
                        5 => ThreadExecState::Exited,
                        _ => ThreadExecState::Unknown,
                    },
                    cpu_ns: Some(info.user.saturating_add(info.system)),
                    run_delay_ns: None,
                },
                4 | 37 | 0x1000_0003 => ThreadExecSample { state: ThreadExecState::Exited, ..ThreadExecSample::unknown() },
                _ => ThreadExecSample::unknown(),
            }
        }
    }
}

#[cfg(any(test, all(windows, target_pointer_width = "64")))]
mod windows_info {
    use super::ThreadExecState;
    use std::mem::{offset_of, size_of};

    // The fixed-width prefixes and strides of the 64-bit SystemProcessInformation ABI.
    #[repr(C)]
    struct SystemProcessInformation {
        next: u32, count: u32, before_pid: [u8; 0x48], pid: u64, rest: [u8; 0xa8],
    }
    #[repr(C)]
    struct SystemThreadInformation {
        before_tid: [u8; 0x30], tid: u64, priorities_and_switches: [u8; 12],
        state: u32, wait_reason: u32, padding: u32,
    }
    const _: () = assert!(size_of::<SystemProcessInformation>() == 0x100);
    const _: () = assert!(size_of::<SystemThreadInformation>() == 0x50);

    // Parse bytes rather than casting the OS buffer to potentially unaligned references.
    fn word(bytes: &[u8], offset: usize) -> u32 {
        u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
    }
    fn id(bytes: &[u8], offset: usize) -> u64 {
        u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
    }

    pub(super) fn parse_windows_process_threads(mut buf: &[u8], pid: u64, tid: u64) -> ThreadExecState {
        use ThreadExecState::*;
        let header = size_of::<SystemProcessInformation>();
        let stride = size_of::<SystemThreadInformation>();
        while buf.len() >= header {
            let next = word(buf, 0) as usize;
            let end = if next == 0 { buf.len() } else { next };
            let Some(record) = buf.get(..end) else { return Unknown };
            let Some(threads) = record.get(header..) else { return Unknown };
            let count = word(record, 4) as usize;
            if count > threads.len() / stride { return Unknown; }
            if id(record, offset_of!(SystemProcessInformation, pid)) == pid {
                for thread in threads[..count * stride].chunks_exact(stride) {
                    if id(thread, offset_of!(SystemThreadInformation, tid)) != tid { continue; }
                    return match word(thread, offset_of!(SystemThreadInformation, state)) {
                        1 | 2 | 3 | 7 => RunningOrRunnable,
                        4 => Exited,
                        5 if word(thread, offset_of!(SystemThreadInformation, wait_reason)) == 5 => Stopped,
                        5 => Sleeping,
                        6 => UninterruptibleIo,
                        _ => Unknown,
                    };
                }
                return Unknown;
            }
            if next == 0 { break; }
            buf = &buf[next..];
        }
        Unknown
    }
}

#[cfg(windows)]
mod os {
    use super::{registered::Target, ThreadExecSample, ThreadExecState};
    pub use super::registered::register_ui_thread;
    use std::{sync::OnceLock, time::{Duration, Instant}};

    // The handle type matches this crate's other kernel32 declarations (clashing_extern_declarations).
    type Handle = *mut std::ffi::c_void;

    #[link(name = "kernel32", kind = "raw-dylib")]
    unsafe extern "system" {
        fn GetCurrentThreadId() -> u32;
        fn GetCurrentThread() -> Handle;
        fn OpenThread(access: u32, inherit: i32, tid: u32) -> Handle;
        fn CloseHandle(handle: Handle) -> i32;
        fn GetExitCodeThread(handle: Handle, status: *mut u32) -> i32;
        fn QueryThreadCycleTime(handle: Handle, cycles: *mut u64) -> i32;
        fn GetThreadTimes(handle: Handle, created: *mut [u32; 2], ended: *mut [u32; 2],
            kernel: *mut [u32; 2], user: *mut [u32; 2]) -> i32;
    }

    pub(super) struct ThreadHandle {
        handle: usize,
        #[cfg(target_pointer_width = "64")]
        tid: u32,
    }

    impl ThreadHandle {
        pub(super) fn current() -> Option<Self> {
            // SAFETY: open a query-only handle to the calling thread, owned until Drop.
            let tid = unsafe { GetCurrentThreadId() };
            let handle = unsafe { OpenThread(0x0800, 0, tid) } as usize;
            if handle == 0 { return None; }
            Some(Self { handle, #[cfg(target_pointer_width = "64")] tid })
        }
    }
    impl Drop for ThreadHandle {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.handle as Handle); }
        }
    }

    fn cycles(handle: usize) -> Option<u64> {
        let mut n = 0;
        (unsafe { QueryThreadCycleTime(handle as Handle, &mut n) } != 0).then_some(n)
    }

    fn cpu_ns(handle: usize) -> Option<u64> {
        // Calibrate once on the sampling thread. The shortest interval per cycle is least
        // affected by the scheduler preempting that thread during calibration.
        static NS_PER_CYCLE: OnceLock<Option<f64>> = OnceLock::new();
        let scale = NS_PER_CYCLE.get_or_init(|| {
            let current = unsafe { GetCurrentThread() } as usize;
            let mut best = f64::INFINITY;
            for _ in 0..3 {
                let before = cycles(current)?;
                let start = Instant::now();
                while start.elapsed() < Duration::from_millis(1) { std::hint::spin_loop(); }
                let elapsed = start.elapsed().as_nanos() as f64;
                let delta = cycles(current)?.checked_sub(before)?;
                if delta != 0 { best = best.min(elapsed / delta as f64); }
            }
            best.is_finite().then_some(best)
        });
        if let Some(scale) = scale {
            if let Some(n) = cycles(handle) { return Some((n as f64 * scale) as u64); }
        }
        let (mut created, mut ended, mut kernel, mut user) = ([0; 2], [0; 2], [0; 2], [0; 2]);
        // SAFETY: the live handle permits querying and the four FILETIME buffers are writable.
        let ok = unsafe { GetThreadTimes(handle as Handle, &mut created, &mut ended, &mut kernel, &mut user) };
        let ticks = |ft: [u32; 2]| (u64::from(ft[1]) << 32) | u64::from(ft[0]);
        (ok != 0).then(|| (ticks(kernel) + ticks(user)) * 100)
    }

    #[cfg(target_pointer_width = "64")]
    fn run_state(buf: &mut Vec<u8>, tid: u32) -> ThreadExecState {
        #[link(name = "ntdll", kind = "raw-dylib")]
        unsafe extern "system" {
            fn NtQuerySystemInformation(class: u32, buf: *mut u8, size: u32, returned: *mut u32) -> i32;
        }
        const MAX: usize = 16 << 20;
        if buf.is_empty() { buf.resize(256 << 10, 0); }
        for attempt in 0..=4 {
            let mut returned = 0;
            // SAFETY: the query may fill at most buf.len() bytes. Only its reported data is parsed.
            let status = unsafe { NtQuerySystemInformation(5, buf.as_mut_ptr(), buf.len() as u32, &mut returned) };
            if status == 0 {
                let Some(data) = buf.get(..returned as usize) else { break };
                return super::windows_info::parse_windows_process_threads(data, std::process::id().into(), tid.into());
            }
            if status as u32 != 0xc0000004 || attempt == 4 || buf.len() == MAX || returned as usize > MAX { break; }
            buf.resize((buf.len() * 2).max(returned as usize).min(MAX), 0);
        }
        ThreadExecState::Unknown
    }

    pub struct UiThreadProbe {
        target: Target,
        #[cfg(target_pointer_width = "64")]
        buf: Vec<u8>,
    }
    impl UiThreadProbe {
        fn new(target: Target) -> Self {
            Self { target, #[cfg(target_pointer_width = "64")] buf: Vec::new() }
        }
        pub fn global() -> Self { Self::new(Target::Global) }
        pub fn for_current_thread() -> Self { Self::new(Target::current()) }

        pub fn sample(&mut self) -> ThreadExecSample {
            let Some(thread) = self.target.get() else { return ThreadExecSample::unknown() };
            let mut status = 0;
            if unsafe { GetExitCodeThread(thread.handle as Handle, &mut status) } != 0 && status != 259 {
                return ThreadExecSample { state: ThreadExecState::Exited, ..ThreadExecSample::unknown() };
            }
            #[cfg(target_pointer_width = "64")]
            let state = run_state(&mut self.buf, thread.tid);
            #[cfg(not(target_pointer_width = "64"))]
            let state = ThreadExecState::Unknown;
            ThreadExecSample { state, cpu_ns: cpu_ns(thread.handle), run_delay_ns: None }
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple", windows)))]
mod os {
    use super::ThreadExecSample;
    pub fn register_ui_thread() {}
    pub struct UiThreadProbe;
    impl UiThreadProbe {
        pub fn global() -> Self { Self }
        pub fn for_current_thread() -> Self { Self }
        pub fn sample(&mut self) -> ThreadExecSample { ThreadExecSample::unknown() }
    }
}

#[cfg(test)]
mod ui_thread_probe_tests {
    use super::*;

    #[test]
    fn linux_proc_parsers_classify_all_states_and_starvation_run_delay() {
        use ThreadExecState::*;
        for (code, state) in [
            ("R", RunningOrRunnable), ("D", UninterruptibleIo), ("S", Sleeping), ("I", Sleeping),
            ("T", Stopped), ("t", Stopped), ("Z", Exited), ("X", Exited), ("x", Exited), ("?", Unknown),
        ] {
            let line = format!("17 (name with ) in it) {code} 0 0 0 0 0 0 0 0 0 0 7 4 0 0");
            let sample = parse_linux_stat(line.as_bytes(), 10).unwrap();
            assert_eq!(sample, ThreadExecSample { state, cpu_ns: Some(110), run_delay_ns: None });
        }
        let name = b"17 (\xff) R 0 0 0 0 0 0 0 0 0 0 7 4";
        assert_eq!(parse_linux_stat(name, 10).unwrap().state, RunningOrRunnable);
        assert_eq!(parse_linux_stat(b"17 (short) R", 10), None);
        assert_eq!(parse_linux_schedstat(b"1000 3000 7\n"), Some((1000, 3000)));
        assert_eq!(parse_linux_schedstat(b"1000 4300 8\n"), Some((1000, 4300)));
        assert_eq!(parse_linux_schedstat(b"1000"), None);
    }

    #[test]
    fn windows_process_info_parser_classifies_all_thread_states() {
        use windows_info::parse_windows_process_threads as parse;
        use ThreadExecState::*;
        let mut buf = vec![0u8; 0x100 + 0x100 + 2 * 0x50];
        buf[..4].copy_from_slice(&0x100u32.to_le_bytes());
        buf[0x104..0x108].copy_from_slice(&2u32.to_le_bytes());
        buf[0x150..0x158].copy_from_slice(&17u64.to_le_bytes());
        buf[0x280..0x288].copy_from_slice(&23u64.to_le_bytes());
        for (state, reason, expected) in [
            (1u32, 0u32, RunningOrRunnable), (2, 0, RunningOrRunnable), (3, 0, RunningOrRunnable),
            (7, 0, RunningOrRunnable), (5, 5, Stopped), (5, 6, Sleeping), (4, 0, Exited),
            (6, 0, UninterruptibleIo), (0, 0, Unknown), (99, 0, Unknown),
        ] {
            buf[0x294..0x298].copy_from_slice(&state.to_le_bytes());
            buf[0x298..0x29c].copy_from_slice(&reason.to_le_bytes());
            assert_eq!(parse(&buf, 17, 23), expected, "state={state} reason={reason}");
        }
        buf[0x294..0x298].copy_from_slice(&1u32.to_le_bytes());
        assert_eq!(parse(&buf, 18, 23), Unknown);
        assert_eq!(parse(&buf, 17, 24), Unknown);
        assert_eq!(parse(&buf[..buf.len() - 1], 17, 23), Unknown);
        buf[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(parse(&buf, 17, 23), Unknown, "a thread array cannot cross into the next process");
        buf[4..8].copy_from_slice(&0u32.to_le_bytes());
        for next in [8u32, u32::MAX] {
            buf[..4].copy_from_slice(&next.to_le_bytes());
            assert_eq!(parse(&buf, 17, 23), Unknown);
        }
    }

    #[test]
    fn os_thread_probe_distinguishes_runnable_and_parked_threads() {
        #[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple", all(windows, target_pointer_width = "64")))]
        {
            use std::sync::{atomic::{AtomicBool, Ordering}, mpsc, Arc, Condvar, Mutex};
            use std::thread;
            let (tx, rx) = mpsc::channel();
            let (parking, parked) = mpsc::channel();
            let spin = Arc::new(AtomicBool::new(true));
            let gate = Arc::new((Mutex::new(false), Condvar::new()));
            let worker = {
                let (spin, gate) = (spin.clone(), gate.clone());
                thread::spawn(move || {
                    tx.send(UiThreadProbe::for_current_thread()).unwrap();
                    while spin.load(Ordering::Acquire) { std::hint::spin_loop(); }
                    let mut done = gate.0.lock().unwrap();
                    parking.send(()).unwrap();
                    while !*done { done = gate.1.wait(done).unwrap(); }
                })
            };
            fn observe(
                probe: &mut UiThreadProbe,
                wanted: fn(ThreadExecSample, ThreadExecSample) -> bool,
            ) -> Result<(), (ThreadExecSample, ThreadExecSample)> {
                let mut last = probe.sample();
                let mut previous = last;
                for _ in 0..100_000 {
                    thread::yield_now();
                    previous = last;
                    last = probe.sample();
                    if wanted(previous, last) { return Ok(()); }
                }
                Err((previous, last))
            }
            let mut probe = rx.recv().unwrap();
            let running = observe(&mut probe, |prev, now| {
                prev.state == ThreadExecState::RunningOrRunnable && now.state == ThreadExecState::RunningOrRunnable
                    && matches!((prev.cpu_ns, now.cpu_ns), (Some(a), Some(b)) if b > a)
            });
            spin.store(false, Ordering::Release);
            parked.recv().unwrap();
            let sleeping = observe(&mut probe, |prev, now| {
                prev.state == ThreadExecState::Sleeping && now.state == ThreadExecState::Sleeping
                    && now.cpu_ns.is_some() && prev.cpu_ns == now.cpu_ns
            });
            *gate.0.lock().unwrap() = true;
            gate.1.notify_one();
            worker.join().unwrap();
            let exited = observe(&mut probe, |_, now| now.state == ThreadExecState::Exited);
            assert!(running.is_ok(), "runnable with advancing CPU: {running:?}");
            assert!(sleeping.is_ok(), "sleeping with flat CPU: {sleeping:?}");
            assert!(exited.is_ok(), "exited after join: {exited:?}");
        }
        #[cfg(all(windows, not(target_pointer_width = "64")))]
        assert_eq!(UiThreadProbe::for_current_thread().sample().state, ThreadExecState::Unknown);
        #[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple", windows)))]
        assert_eq!(UiThreadProbe::for_current_thread().sample(), ThreadExecSample::unknown());
    }
}
