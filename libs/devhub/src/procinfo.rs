//! Process and machine numbers for the explorer, read from the OS without asking the process:
//! `/proc` on Linux and Android, `proc_pidinfo` / `sysctl` on macOS. CPU is a percentage over the time since
//! the previous sample of the same pid ([`CpuMeter`]).

use std::collections::HashMap;
use std::time::Instant;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Proc {
    /// user + system CPU time, ns
    pub cpu_ns: u64,
    pub rss_bytes: u64,
    pub threads: u32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Machine {
    pub os: &'static str,
    pub load1: f64,
    pub mem_total_mb: u64,
    pub mem_avail_mb: u64,
    pub uptime_s: u64,
    pub cpus: u32,
    /// battery level 0..=100 and charging (a device: the JIT host's [`set_battery_source`])
    pub battery: Option<(u32, bool)>,
}

static BATTERY: std::sync::OnceLock<fn() -> Option<(u32, bool)>> = std::sync::OnceLock::new();

/// A JIT host on a battery device (Android: BatteryManager through the HAL) names where the
/// battery reading comes from; machines without one report none.
pub fn set_battery_source(f: fn() -> Option<(u32, bool)>) {
    let _ = BATTERY.set(f);
}

/// This machine's numbers, with the battery when a source is set.
pub fn machine() -> Machine {
    let mut m = os::machine();
    m.battery = BATTERY.get().and_then(|f| f());
    m
}

pub fn alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        extern "C" {
            fn kill(pid: i32, sig: i32) -> i32;
        }
        unsafe { kill(pid as i32, 0) == 0 }
    }
    #[cfg(not(unix))]
    {
        crate::os::pid_alive(pid)
    }
}

#[cfg(any(target_os = "linux", target_os = "android"))]
mod os {
    use super::*;

    fn clk_tck() -> u64 {
        extern "C" {
            fn sysconf(name: i32) -> i64;
        }
        // _SC_CLK_TCK = 2 on Linux
        let t = unsafe { sysconf(2) };
        if t > 0 { t as u64 } else { 100 }
    }

    pub fn proc(pid: u32) -> Option<Proc> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        // fields after the parenthesised command name (which may hold spaces)
        let rest = &stat[stat.rfind(')')? + 2..];
        let f: Vec<&str> = rest.split_whitespace().collect();
        // rest[0] is field 3 (state): utime = 14, stime = 15, num_threads = 20, rss pages = 24
        let utime: u64 = f.get(11)?.parse().ok()?;
        let stime: u64 = f.get(12)?.parse().ok()?;
        let threads: u32 = f.get(17)?.parse().ok()?;
        let rss_pages: u64 = f.get(21)?.parse().ok()?;
        let tck = clk_tck();
        Some(Proc { cpu_ns: (utime + stime) * 1_000_000_000 / tck, rss_bytes: rss_pages * 4096, threads })
    }

    pub fn machine() -> Machine {
        let mut m = Machine { os: if cfg!(target_os = "android") { "android" } else { "linux" }, ..Machine::default() };
        if let Ok(l) = std::fs::read_to_string("/proc/loadavg") {
            m.load1 = l.split_whitespace().next().and_then(|v| v.parse().ok()).unwrap_or(0.0);
        }
        if let Ok(t) = std::fs::read_to_string("/proc/meminfo") {
            for line in t.lines() {
                let kb = || line.split_whitespace().nth(1).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0) / 1024;
                if line.starts_with("MemTotal:") {
                    m.mem_total_mb = kb();
                } else if line.starts_with("MemAvailable:") {
                    m.mem_avail_mb = kb();
                }
            }
        }
        if let Ok(u) = std::fs::read_to_string("/proc/uptime") {
            m.uptime_s = u.split_whitespace().next().and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0) as u64;
        } else {
            // an Android app may not read /proc/uptime (nor /proc/loadavg: its load stays 0)
            #[repr(C)]
            struct Timespec {
                sec: i64,
                nsec: i64,
            }
            extern "C" {
                fn clock_gettime(clock: i32, ts: *mut Timespec) -> i32;
            }
            let mut ts = Timespec { sec: 0, nsec: 0 };
            // CLOCK_BOOTTIME
            if unsafe { clock_gettime(7, &mut ts) } == 0 {
                m.uptime_s = ts.sec.max(0) as u64;
            }
        }
        m.cpus = std::thread::available_parallelism().map(|n| n.get() as u32).unwrap_or(1);
        m
    }
}

#[cfg(target_os = "macos")]
mod os {
    use super::*;

    #[repr(C)]
    #[derive(Default)]
    struct ProcTaskInfo {
        virtual_size: u64,
        resident_size: u64,
        total_user: u64,
        total_system: u64,
        threads_user: u64,
        threads_system: u64,
        policy: i32,
        faults: i32,
        pageins: i32,
        cow_faults: i32,
        messages_sent: i32,
        messages_received: i32,
        syscalls_mach: i32,
        syscalls_unix: i32,
        csw: i32,
        threadnum: i32,
        numrunning: i32,
        priority: i32,
    }

    #[repr(C)]
    struct Timebase {
        numer: u32,
        denom: u32,
    }

    extern "C" {
        fn proc_pidinfo(pid: i32, flavor: i32, arg: u64, buffer: *mut core::ffi::c_void, size: i32) -> i32;
        fn mach_timebase_info(info: *mut Timebase) -> i32;
        fn sysctlbyname(name: *const u8, oldp: *mut core::ffi::c_void, oldlenp: *mut usize, newp: *mut core::ffi::c_void, newlen: usize) -> i32;
        fn getloadavg(loads: *mut f64, n: i32) -> i32;
        fn mach_host_self() -> u32;
        fn host_statistics64(host: u32, flavor: i32, info: *mut i32, count: *mut u32) -> i32;
    }

    /// `vm_statistics64` (mach/vm_statistics.h), the fields the measure reads; 38 `integer_t`s.
    #[repr(C, align(8))]
    #[derive(Default)]
    #[allow(dead_code)] // the C layout: every field is there so the kernel's writes land in place
    pub(super) struct VmStats64 {
        free_count: u32,
        active_count: u32,
        inactive_count: u32,
        wire_count: u32,
        counters: [u64; 9],
        purgeable_count: u32,
        speculative_count: u32,
        counters2: [u64; 4],
        compressor_page_count: u32,
        throttled_count: u32,
        external_page_count: u32,
        internal_page_count: u32,
        total_uncompressed_pages_in_compressor: u64,
    }

    const HOST_VM_INFO64: i32 = 4;

    /// Available pages: free + inactive (clean file cache the kernel reclaims first) + speculative
    /// + purgeable. Counting only free (+ speculative, purgeable) read a Mac whose RAM is file cache
    /// and compressed memory as full ("ram 15.7/16 GB used", every mini lease "node busy" 10-30).
    fn available_pages() -> Option<u64> {
        let mut v = VmStats64::default();
        let mut count = (std::mem::size_of::<VmStats64>() / 4) as u32;
        if unsafe { host_statistics64(mach_host_self(), HOST_VM_INFO64, &mut v as *mut _ as *mut i32, &mut count) } != 0 {
            return None;
        }
        Some(v.free_count as u64 + v.inactive_count as u64 + v.speculative_count as u64 + v.purgeable_count as u64)
    }

    const PROC_PIDTASKINFO: i32 = 4;

    pub fn proc(pid: u32) -> Option<Proc> {
        let mut info = ProcTaskInfo::default();
        let size = std::mem::size_of::<ProcTaskInfo>() as i32;
        if unsafe { proc_pidinfo(pid as i32, PROC_PIDTASKINFO, 0, &mut info as *mut _ as *mut _, size) } != size {
            return None;
        }
        // task times are mach absolute-time units on Apple silicon
        let mut tb = Timebase { numer: 1, denom: 1 };
        unsafe { mach_timebase_info(&mut tb) };
        let ticks = info.total_user + info.total_system;
        let cpu_ns = (ticks as u128 * tb.numer as u128 / tb.denom.max(1) as u128) as u64;
        Some(Proc { cpu_ns, rss_bytes: info.resident_size, threads: info.threadnum.max(0) as u32 })
    }

    fn sysctl_u64(name: &str) -> u64 {
        let name = format!("{name}\0");
        let mut v = 0u64;
        let mut len = std::mem::size_of::<u64>();
        if unsafe { sysctlbyname(name.as_ptr(), &mut v as *mut _ as *mut _, &mut len, std::ptr::null_mut(), 0) } != 0 {
            return 0;
        }
        v
    }

    pub fn machine() -> Machine {
        let mut load = [0f64; 3];
        unsafe { getloadavg(load.as_mut_ptr(), 3) };
        let total = sysctl_u64("hw.memsize") >> 20;
        // memory the kernel hands out without paging anything in use ([`available_pages`])
        let page = sysctl_u64("hw.pagesize").max(4096);
        let avail = (available_pages().unwrap_or(0) * page >> 20).min(total);
        Machine { os: "macos", load1: load[0], mem_total_mb: total, mem_avail_mb: avail, uptime_s: 0, cpus: std::thread::available_parallelism().map(|n| n.get() as u32).unwrap_or(1), battery: None }
    }
}

#[cfg(windows)]
mod os {
    use super::*;
    pub fn proc(pid: u32) -> Option<Proc> {
        let (cpu_ns, rss_bytes) = crate::os::proc_numbers(pid)?;
        let threads = crate::os::processes().into_iter().find(|p| p.0 == pid).map_or(1, |p| p.2);
        Some(Proc { cpu_ns, rss_bytes, threads })
    }
    pub fn machine() -> Machine {
        let (total, avail) = crate::os::memory();
        Machine { os: "windows", load1: 0.0, mem_total_mb: total >> 20, mem_avail_mb: avail >> 20, uptime_s: crate::os::uptime_s(), cpus: std::thread::available_parallelism().map(|n| n.get() as u32).unwrap_or(1), battery: None }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "android", target_os = "macos", windows)))]
mod os {
    use super::*;
    pub fn proc(_pid: u32) -> Option<Proc> {
        None
    }
    pub fn machine() -> Machine {
        Machine { os: std::env::consts::OS, ..Machine::default() }
    }
}

pub use os::proc;

/// CPU % per pid between samples (the hub samples every couple of seconds; readers take the
/// last rate).
#[derive(Default)]
pub struct CpuMeter {
    last: HashMap<u32, (Instant, u64)>,
    rate: HashMap<u32, f64>,
}

impl CpuMeter {
    /// The pid's numbers and its CPU % since the previous call for it (`None` the first time).
    pub fn sample(&mut self, pid: u32) -> Option<(Proc, Option<f64>)> {
        let p = proc(pid)?;
        let now = Instant::now();
        let pct = self.last.get(&pid).and_then(|(t, ns)| {
            let wall = now.duration_since(*t).as_nanos() as f64;
            (wall > 1e7).then(|| p.cpu_ns.saturating_sub(*ns) as f64 * 100.0 / wall)
        });
        self.last.insert(pid, (now, p.cpu_ns));
        if let Some(r) = pct {
            self.rate.insert(pid, r);
        }
        Some((p, pct))
    }

    /// The rate the last two samples measured.
    pub fn rate(&self, pid: u32) -> Option<f64> {
        self.rate.get(&pid).copied()
    }

    pub fn forget(&mut self, keep: &dyn Fn(u32) -> bool) {
        self.last.retain(|pid, _| keep(*pid));
        self.rate.retain(|pid, _| keep(*pid));
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn this_process_is_measured() {
        let p = super::proc(std::process::id());
        if cfg!(any(target_os = "linux", target_os = "macos")) {
            let p = p.unwrap();
            assert!(p.rss_bytes > 0 && p.threads >= 1);
        }
        assert!(super::alive(std::process::id()));
    }
}

#[cfg(all(test, target_os = "macos"))]
mod macos_tests {
    /// `vm_statistics64` is 38 `integer_t`s (HOST_VM_INFO64_COUNT), and the measure reads a sane
    /// share of the RAM: some memory is always free or cache, never more than the machine has.
    #[test]
    fn available_memory_is_free_plus_cache() {
        assert_eq!(std::mem::size_of::<super::os::VmStats64>(), 38 * 4);
        let m = super::machine();
        assert!(m.mem_avail_mb > 0 && m.mem_avail_mb <= m.mem_total_mb, "{} of {} MB", m.mem_avail_mb, m.mem_total_mb);
    }
}
