//! How far a timing taken now can be trusted: machine load, CPU frequency against its maximum,
//! thermal throttling, power state. Every timing answer of inspect ends with this line, so an
//! agent never compares numbers from a busy or throttled machine without knowing it. Read from
//! what the OS exposes to an unprivileged process (no prompts, no root).

#[cfg(unix)]
extern "C" {
    fn getloadavg(loads: *mut f64, n: i32) -> i32;
}

fn cpus() -> usize {
    std::thread::available_parallelism().map_or(1, |n| n.get())
}

#[cfg(unix)]
fn load1() -> Option<f64> {
    let mut l = [0f64; 3];
    (unsafe { getloadavg(l.as_mut_ptr(), 3) } >= 1).then_some(l[0])
}

#[cfg(not(unix))]
fn load1() -> Option<f64> {
    None
}

/// One line: `machine: load 9.1 on 32 CPUs (ok); CPU at 98% of max; no throttling` and a verdict
/// word (`ok`, `BUSY`, `THROTTLED`) the reader can key on. `timing`: the answer holds timings
/// (else only what the state is).
pub fn line(timing: bool) -> String {
    let n = cpus();
    let mut parts = Vec::new();
    let mut verdict = "ok";
    if let Some(l) = load1() {
        let busy = l > n as f64 * 0.7;
        if busy {
            verdict = "BUSY";
        }
        parts.push(format!("load {:.1} on {} CPUs", l, n));
    }
    for (s, bad) in imp::state() {
        if bad {
            verdict = "THROTTLED";
        }
        parts.push(s);
    }
    format!("machine {}: {}{}", verdict, parts.join("; "), if timing && verdict != "ok" { " -- timings here are unreliable" } else { "" })
}

#[cfg(target_os = "linux")]
mod imp {
    fn read_u64(p: &str) -> Option<u64> {
        std::fs::read_to_string(p).ok()?.trim().parse().ok()
    }

    /// CPU frequency (mean of the CPUs that report one) against their maximum, and the kernel's
    /// thermal-throttle event counts.
    pub fn state() -> Vec<(String, bool)> {
        let mut v = Vec::new();
        let (mut cur, mut max, mut k) = (0u64, 0u64, 0u64);
        let mut throttle = 0u64;
        for c in 0..1024 {
            let base = format!("/sys/devices/system/cpu/cpu{}", c);
            if !std::path::Path::new(&base).exists() {
                break;
            }
            if let (Some(a), Some(b)) = (read_u64(&format!("{}/cpufreq/scaling_cur_freq", base)), read_u64(&format!("{}/cpufreq/cpuinfo_max_freq", base))) {
                cur += a;
                max += b;
                k += 1;
            }
            throttle += read_u64(&format!("{}/thermal_throttle/core_throttle_count", base)).unwrap_or(0);
        }
        if k > 0 && max > 0 {
            v.push((format!("CPU clock {:.0}% of max ({:.2} GHz mean)", cur as f64 * 100.0 / max as f64, cur as f64 / k as f64 / 1e6), false));
        }
        if let Some(gov) = std::fs::read_to_string("/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor").ok() {
            v.push((format!("governor {}", gov.trim()), false));
        }
        if throttle > 0 {
            v.push((format!("{} thermal throttle events since boot", throttle), false));
        }
        v
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use std::ffi::c_void;
    extern "C" {
        fn objc_getClass(name: *const u8) -> *mut c_void;
        fn sel_registerName(name: *const u8) -> *mut c_void;
        fn objc_msgSend();
    }

    /// The process info's thermal state and low-power mode (Foundation; no permission needed).
    pub fn state() -> Vec<(String, bool)> {
        let mut v = Vec::new();
        unsafe {
            let cls = objc_getClass(b"NSProcessInfo\0".as_ptr());
            if cls.is_null() {
                return v;
            }
            let send_obj: extern "C" fn(*mut c_void, *mut c_void) -> *mut c_void = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
            let send_int: extern "C" fn(*mut c_void, *mut c_void) -> isize = std::mem::transmute(objc_msgSend as unsafe extern "C" fn());
            let info = send_obj(cls, sel_registerName(b"processInfo\0".as_ptr()));
            if info.is_null() {
                return v;
            }
            let t = send_int(info, sel_registerName(b"thermalState\0".as_ptr()));
            let name = ["nominal", "fair", "serious", "critical"].get(t as usize).copied().unwrap_or("unknown");
            v.push((format!("thermal {}", name), t >= 2));
            let low = send_int(info, sel_registerName(b"isLowPowerModeEnabled\0".as_ptr())) & 0xff;
            if low != 0 {
                v.push(("low power mode on".to_string(), true));
            }
        }
        v
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod imp {
    pub fn state() -> Vec<(String, bool)> {
        Vec::new()
    }
}
