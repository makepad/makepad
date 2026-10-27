//! The calling thread's retired instructions and cycles in user mode, exact (counted, not
//! sampled) and unprivileged, the same way in a rustc build and under `makepad run`:
//! - macOS: `thread_selfcounts` (THSC_CPI, the fixed counters), no setup;
//! - Linux: a perf_event group (instructions:u leader, cycles:u member) on the calling thread,
//!   opened at its first read and kept for the thread's life (perf_event_paranoid <= 2);
//! - elsewhere: None.
//! Readers take differences of two reads on the same thread (`uicounts`, Sandbox's DET windows).

/// (instructions, cycles) retired by the calling thread so far; None where the OS gives no
/// counter (or perf_event_open refused).
#[cfg(target_os = "macos")]
pub fn read() -> Option<(u64, u64)> {
    extern "C" {
        fn thread_selfcounts(kind: i32, buf: *mut u64, nbytes: usize) -> i32;
    }
    let mut b = [0u64; 2];
    if unsafe { thread_selfcounts(1, b.as_mut_ptr(), 16) } != 0 {
        return None;
    }
    Some((b[0], b[1]))
}

/// The calling thread's (instructions, cycles) per CPU kind, `[performance, efficiency]`
/// (macOS thread_selfcounts THSC_CPI_PER_PERF_LEVEL; a single-kind machine has zeros in the
/// second): how much of a thread's run was on E cores, where the same code takes more cycles.
/// None where the OS does not split them.
#[cfg(target_os = "macos")]
pub fn read_levels() -> Option<[(u64, u64); 2]> {
    extern "C" {
        fn thread_selfcounts(kind: i32, buf: *mut u64, nbytes: usize) -> i32;
        fn sysctlbyname(
            name: *const std::ffi::c_char,
            old: *mut std::ffi::c_void,
            size: *mut usize,
            new: *mut std::ffi::c_void,
            new_size: usize,
        ) -> i32;
    }
    static LEVELS: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    let n = *LEVELS.get_or_init(|| {
        let (mut v, mut l) = (0u32, 4usize);
        if unsafe { sysctlbyname(b"hw.nperflevels\0".as_ptr().cast(), (&mut v as *mut u32).cast(), &mut l, std::ptr::null_mut(), 0) } == 0 { (v as usize).clamp(1, 2) } else { 1 }
    });
    let mut b = [0u64; 4];
    if unsafe { thread_selfcounts(2, b.as_mut_ptr(), 16 * n) } != 0 {
        return None;
    }
    Some([(b[0], b[1]), (b[2], b[3])])
}

#[cfg(not(target_os = "macos"))]
pub fn read_levels() -> Option<[(u64, u64); 2]> {
    None
}

/// The whole process's (instructions, cycles) so far, every thread (macOS proc_pid_rusage
/// RUSAGE_INFO_V4 ri_instructions / ri_cycles): with `read`, what the other threads (a JIT's
/// compile and owner threads, workers) ran beside the calling one. None elsewhere.
#[cfg(target_os = "macos")]
pub fn read_process() -> Option<(u64, u64)> {
    extern "C" {
        fn proc_pid_rusage(pid: i32, flavor: i32, buf: *mut u64) -> i32;
        fn getpid() -> i32;
    }
    // rusage_info_v4: 16-byte uuid, then u64 fields; ri_instructions / ri_cycles are fields 29 / 30
    let mut b = [0u64; 2 + 36];
    if unsafe { proc_pid_rusage(getpid(), 4, b.as_mut_ptr()) } != 0 {
        return None;
    }
    Some((b[2 + 29], b[2 + 30]))
}

#[cfg(not(target_os = "macos"))]
pub fn read_process() -> Option<(u64, u64)> {
    None
}

#[cfg(target_os = "linux")]
pub fn read() -> Option<(u64, u64)> {
    linux::read()
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn read() -> Option<(u64, u64)> {
    None
}

#[cfg(target_os = "linux")]
mod linux {
    use std::cell::Cell;

    extern "C" {
        fn syscall(n: i64, ...) -> i64;
        #[link_name = "read"]
        fn sys_read(fd: i32, buf: *mut u8, n: usize) -> isize;
    }
    #[cfg(target_arch = "x86_64")]
    const SYS_PERF_EVENT_OPEN: i64 = 298;
    #[cfg(target_arch = "aarch64")]
    const SYS_PERF_EVENT_OPEN: i64 = 241;
    /// PERF_COUNT_HW_INSTRUCTIONS / PERF_COUNT_HW_CPU_CYCLES
    const INSTRUCTIONS: u64 = 1;
    const CYCLES: u64 = 0;
    /// read_format PERF_FORMAT_GROUP: one read gives every member's value
    const FORMAT_GROUP: u64 = 8;

    thread_local! {
        /// the group leader's fd: -2 not opened yet, -1 refused
        static LEADER: Cell<i32> = const { Cell::new(-2) };
    }

    /// A counting event of the calling thread (pid 0, any CPU), user mode only.
    fn open(config: u64, group: i32) -> i32 {
        let mut attr = [0u8; 128];
        let put = |a: &mut [u8; 128], off: usize, v: &[u8]| a[off..off + v.len()].copy_from_slice(v);
        put(&mut attr, 0, &0u32.to_ne_bytes()); // PERF_TYPE_HARDWARE
        put(&mut attr, 4, &128u32.to_ne_bytes());
        put(&mut attr, 8, &config.to_ne_bytes());
        put(&mut attr, 32, &FORMAT_GROUP.to_ne_bytes());
        put(&mut attr, 40, &((1u64 << 5) | (1u64 << 6)).to_ne_bytes()); // exclude_kernel | exclude_hv
        unsafe { syscall(SYS_PERF_EVENT_OPEN, attr.as_ptr(), 0i32, -1i32, group, 0u64) as i32 }
    }

    pub fn read() -> Option<(u64, u64)> {
        let fd = LEADER.with(|l| {
            if l.get() == -2 {
                let lead = open(INSTRUCTIONS, -1);
                // the member's fd stays open for the thread's life (the group reads through the leader)
                let ok = lead >= 0 && open(CYCLES, lead) >= 0;
                l.set(if ok { lead } else { -1 });
            }
            l.get()
        });
        if fd < 0 {
            return None;
        }
        // { nr, instructions, cycles }
        let mut b = [0u64; 3];
        let n = unsafe { sys_read(fd, b.as_mut_ptr() as *mut u8, 24) };
        (n == 24 && b[0] == 2).then_some((b[1], b[2]))
    }
}
