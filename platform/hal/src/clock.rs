//! The HAL's one clock: raw monotonic nanoseconds, absolute (not deltas). And CPU time, of the
//! calling thread or of the process: what they ran, which a loaded box does not move (a thread
//! that waits for a core, or for anything else, spends none).

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
#[relax_api::relax_api]
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

/// What the process's threads that run now have spent runnable, waiting for a core, in
/// nanoseconds: CPU they wanted and a loaded box did not give them, which their CPU time does not
/// show (a worker at a fifth of a core can want a whole one). Not the calling thread's, nor those
/// whose names `skip` picks (a name as the OS keeps it: Linux cuts it to 15 bytes). A thread that
/// ended takes its wait out with it. Linux keeps it per thread (/proc/self/task/<tid>/schedstat,
/// its second field, run_delay): two small files a thread, so a caller reads it seldom (a test's
/// wait, once a look). macOS keeps a runnable time only for a whole process (proc_pid_rusage's
/// ri_runnable_time), where the calling thread's and the skipped ones' cannot be told apart, and
/// Windows none outside its event tracing: both answer 0.
#[cfg(any(target_os = "linux", target_os = "android"))]
pub fn threads_waited_ns(skip: impl Fn(&str) -> bool) -> u64 {
    extern "C" {
        fn gettid() -> i32;
    }
    // SAFETY: no arguments; the calling thread's id
    let me = unsafe { gettid() }.to_string();
    let Ok(tasks) = std::fs::read_dir("/proc/self/task") else { return 0 };
    let mut waited = 0;
    for task in tasks.flatten() {
        if task.file_name().to_str() == Some(me.as_str()) {
            continue;
        }
        let path = task.path();
        if skip(std::fs::read_to_string(path.join("comm")).unwrap_or_default().trim_end()) {
            continue;
        }
        // "<run time> <run_delay> <timeslices>", in ns
        let stat = std::fs::read_to_string(path.join("schedstat")).unwrap_or_default();
        waited += stat.split_whitespace().nth(1).and_then(|ns| ns.parse::<u64>().ok()).unwrap_or(0);
    }
    waited
}

/// See the Linux one: this OS keeps no runnable time per thread.
#[cfg(not(any(target_os = "linux", target_os = "android")))]
pub fn threads_waited_ns(skip: impl Fn(&str) -> bool) -> u64 {
    let _ = skip;
    0
}

/// How many of the process's threads wait in the kernel uninterruptibly now: on the disk (a
/// file's sync, a read) or on a page it reads in. Such a thread runs no CPU and is not idle. Not
/// those whose names `skip` picks (a name as the OS keeps it: Linux cuts it to 15 bytes). Linux
/// reads each thread's state from /proc/self/task/<tid>/stat (`D`): a small file a thread, so a
/// caller reads it seldom (a test's wait, once a look).
#[cfg(any(target_os = "linux", target_os = "android"))]
pub fn threads_in_io_wait(skip: impl Fn(&str) -> bool) -> u32 {
    let Ok(tasks) = std::fs::read_dir("/proc/self/task") else { return 0 };
    let mut waiting = 0;
    for task in tasks.flatten() {
        // "tid (comm) state ...": the comm may hold spaces and parentheses
        let stat = std::fs::read_to_string(task.path().join("stat")).unwrap_or_default();
        let Some((head, rest)) = stat.rsplit_once(')') else { continue };
        let name = head.split_once('(').map_or("", |(_, name)| name);
        if rest.split_whitespace().next() == Some("D") && !skip(name) {
            waiting += 1;
        }
    }
    waiting
}

/// See the Linux one: macOS keeps each thread's run state (THREAD_EXTENDED_INFO,
/// TH_STATE_UNINTERRUPTIBLE).
#[cfg(target_os = "macos")]
pub fn threads_in_io_wait(skip: impl Fn(&str) -> bool) -> u32 {
    #[repr(C)]
    struct Ext { user: u64, system: u64, cpu_usage: i32, policy: i32, run_state: i32, flags: i32, sleep_time: i32, curpri: i32, pri: i32, maxpri: i32, name: [std::ffi::c_char; 64] }
    extern "C" {
        static mach_task_self_: u32;
        fn task_threads(task: u32, list: *mut *mut u32, count: *mut u32) -> i32;
        fn thread_info(thread: u32, flavor: i32, info: *mut i32, count: *mut u32) -> i32;
        fn mach_port_deallocate(task: u32, name: u32) -> i32;
        fn mach_vm_deallocate(task: u32, address: u64, size: u64) -> i32;
    }
    const THREAD_EXTENDED_INFO: i32 = 5;
    const TH_STATE_UNINTERRUPTIBLE: i32 = 4;
    let mut waiting = 0;
    // SAFETY: the task's own thread list, each port released and the list deallocated after use;
    // `ext` is a zeroed THREAD_EXTENDED_INFO of the count passed
    unsafe {
        let task = mach_task_self_;
        let (mut list, mut count): (*mut u32, u32) = (std::ptr::null_mut(), 0);
        if task_threads(task, &mut list, &mut count) != 0 {
            return 0;
        }
        for &thread in std::slice::from_raw_parts(list, count as usize) {
            let mut ext: Ext = std::mem::zeroed();
            let mut n = (std::mem::size_of::<Ext>() / 4) as u32;
            if thread_info(thread, THREAD_EXTENDED_INFO, &mut ext as *mut Ext as *mut i32, &mut n) == 0
                && ext.run_state == TH_STATE_UNINTERRUPTIBLE
                && !skip(&std::ffi::CStr::from_ptr(ext.name.as_ptr()).to_string_lossy())
            {
                waiting += 1;
            }
            mach_port_deallocate(task, thread);
        }
        mach_vm_deallocate(task, list as u64, count as u64 * 4);
    }
    waiting
}

/// See the Linux one: Windows keeps no such state outside its event tracing, and answers 0.
#[cfg(not(any(target_os = "linux", target_os = "android", target_os = "macos")))]
pub fn threads_in_io_wait(skip: impl Fn(&str) -> bool) -> u32 {
    let _ = skip;
    0
}

/// The processes `pid` started that run now, and the ones they started in turn (not `pid`), each
/// as (its pid, its CPU so far in nanoseconds), into `out`: what a program runs in the programs it
/// launched (a tool it runs for a request, a build), which its own CPU time does not show. A
/// handful of small files a process (each thread's children, the child's stat), so a caller asks
/// seldom (a test's wait, once a look). Linux reads each thread's `children` and, on a kernel that
/// keeps none, every process's parent from its stat; its CPU counts in clock ticks.
#[cfg(any(target_os = "linux", target_os = "android"))]
pub fn started_cpu_ns(pid: u32, out: &mut Vec<(u32, u64)>) {
    extern "C" {
        fn sysconf(name: i32) -> std::ffi::c_long;
    }
    const SC_CLK_TCK: i32 = 2;
    // SAFETY: no pointer arguments
    let tick_ns = 1_000_000_000 / (unsafe { sysconf(SC_CLK_TCK) }.max(1) as u64);
    // "pid (comm) state ppid ... utime stime ...": the comm may hold spaces and parentheses
    let stat = |pid: u32| -> Option<(u32, u64)> {
        let text = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        let fields: Vec<&str> = text.rsplit_once(')')?.1.split_whitespace().collect();
        let field = |n: usize| fields.get(n - 3)?.parse::<u64>().ok();
        Some((field(4)? as u32, (field(14)? + field(15)?) * tick_ns))
    };
    let kept = std::path::Path::new(&format!("/proc/{pid}/task/{pid}/children")).exists();
    let parents: Vec<(u32, u32, u64)> = if kept {
        Vec::new()
    } else {
        let Ok(all) = std::fs::read_dir("/proc") else { return };
        all.flatten().filter_map(|entry| entry.file_name().to_str()?.parse::<u32>().ok())
            .filter_map(|child| stat(child).map(|(parent, cpu)| (child, parent, cpu))).collect()
    };
    let mut parents_left = vec![pid];
    while let Some(parent) = parents_left.pop() {
        if kept {
            let Ok(tasks) = std::fs::read_dir(format!("/proc/{parent}/task")) else { continue };
            for task in tasks.flatten() {
                let children = std::fs::read_to_string(task.path().join("children")).unwrap_or_default();
                for child in children.split_whitespace().filter_map(|c| c.parse::<u32>().ok()) {
                    if let Some((_, cpu)) = stat(child) {
                        out.push((child, cpu));
                        parents_left.push(child);
                    }
                }
            }
        } else {
            for &(child, _, cpu) in parents.iter().filter(|(_, p, _)| *p == parent) {
                out.push((child, cpu));
                parents_left.push(child);
            }
        }
    }
}

/// See the Linux one. macOS asks libproc: each process's children, and its CPU in Mach time units.
#[cfg(target_vendor = "apple")]
pub fn started_cpu_ns(pid: u32, out: &mut Vec<(u32, u64)>) {
    extern "C" {
        fn proc_listchildpids(ppid: i32, buffer: *mut std::ffi::c_void, size: i32) -> i32;
        fn proc_pid_rusage(pid: i32, flavor: i32, buffer: *mut std::ffi::c_void) -> i32;
        fn mach_timebase_info(info: *mut [u32; 2]) -> i32;
    }
    const RUSAGE_INFO_V0: i32 = 0;
    let mut base = [1u32, 1];
    // SAFETY: a valid out pointer to {numer, denom}
    unsafe { mach_timebase_info(&mut base) };
    let mut children = [0i32; 512];
    let mut parents_left = vec![pid as i32];
    while let Some(parent) = parents_left.pop() {
        // SAFETY: the buffer and its size in bytes; the answer is how many pids it filled
        let n = unsafe { proc_listchildpids(parent, children.as_mut_ptr().cast(), std::mem::size_of_val(&children) as i32) };
        for &child in &children[..n.clamp(0, children.len() as i32) as usize] {
            // rusage_info_v0: a 16-byte uuid, then the user and the system time
            let mut info = [0u64; 12];
            // SAFETY: the buffer outlasts the call and holds a whole rusage_info_v0 (96 bytes)
            if child <= 0 || unsafe { proc_pid_rusage(child, RUSAGE_INFO_V0, info.as_mut_ptr().cast()) } != 0 {
                continue;
            }
            let ticks = (info[2] + info[3]) as u128;
            out.push((child as u32, (ticks * base[0] as u128 / base[1].max(1) as u128) as u64));
            parents_left.push(child);
        }
    }
}

/// See the Linux one: this OS answers none.
#[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
pub fn started_cpu_ns(pid: u32, out: &mut Vec<(u32, u64)>) {
    let _ = (pid, out);
}

#[cfg(not(any(unix, windows)))]
pub fn raw_ns() -> u64 {
    use std::sync::OnceLock;
    use std::time::Instant;
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_nanos() as u64 + 1
}

/// A capture result's sensor timestamp (the start of its exposure) on the HAL clock, read as the
/// result arrives. `says_boot`: the camera says it counts in the boot clock, `boot_offset` ahead
/// of the HAL's. A camera that does not say counts in one clock or the other as far as it can be
/// told: the one where this exposure has only just begun. None: in neither.
pub fn sensor_start_on_hal_clock(stamp: i64, says_boot: bool, hal_now: i64, boot_offset: i64) -> Option<u64> {
    const RESULT_AGE_MAX_NS: i64 = 1_000_000_000;
    let just_begun = |age: i64| (0..RESULT_AGE_MAX_NS).contains(&age);
    if stamp <= 0 {
        None
    } else if says_boot {
        Some((stamp - boot_offset).max(0) as u64)
    } else if just_begun(hal_now - stamp) {
        Some(stamp as u64)
    } else if just_begun(hal_now + boot_offset - stamp) {
        Some((stamp - boot_offset).max(0) as u64)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::sensor_start_on_hal_clock;

    #[test]
    fn a_sensor_stamp_reaches_the_hal_clock_by_its_arrival() {
        const MS: i64 = 1_000_000;
        let (now, offset) = (500_000 * MS, 90_000 * MS);
        // says the boot clock: taken at its word, whatever the age
        assert_eq!(sensor_start_on_hal_clock(now + offset - 40 * MS, true, now, offset), Some((now - 40 * MS) as u64));
        assert_eq!(sensor_start_on_hal_clock(now + offset - 5000 * MS, true, now, offset), Some((now - 5000 * MS) as u64));
        // says nothing: the clock where it has just begun
        assert_eq!(sensor_start_on_hal_clock(now - 40 * MS, false, now, offset), Some((now - 40 * MS) as u64));
        assert_eq!(sensor_start_on_hal_clock(now + offset - 40 * MS, false, now, offset), Some((now - 40 * MS) as u64));
        // no suspend since boot: the clocks agree
        assert_eq!(sensor_start_on_hal_clock(now - 40 * MS, false, now, 0), Some((now - 40 * MS) as u64));
        // on neither clock, or none at all
        assert_eq!(sensor_start_on_hal_clock(1000 * MS, false, now, offset), None);
        assert_eq!(sensor_start_on_hal_clock(now + 5 * MS, false, now, 0), None);
        assert_eq!(sensor_start_on_hal_clock(0, true, now, offset), None);
    }
}
