//! This machine's capabilities and health: hardware from the OS (Linux `/proc` + `/sys` +
//! `nvidia-smi`; macOS `sysctl` + `statfs`), policy from `~/.makepad/devhub/lease.conf`.
//!
//! `lease.conf` is `key = value` lines: `role = pool|suite|bench|user-wm|device`, `slots.<kind> = n`,
//! `disk_floor_gb`, `disk_warn_gb`, `timed = on|off`, `spec = <text>`, `work = <dir>` (default
//! `~/devhub`), and CPU pinning for leases (`cpus.shared = 8-15,24-31`, `cpus.timed = 0-7,16-23`).

use super::{Caps, Kind, Role};
use std::path::{Path, PathBuf};

/// The node's lease policy (`lease.conf`).
#[derive(Clone, Debug, PartialEq)]
pub struct LeaseConf {
    pub role: Role,
    /// explicit `slots.<kind>` lines; the rest come from [`LeaseConf::slots`]'s defaults
    pub slots: Vec<(Kind, u32)>,
    pub disk_floor_gb: f64,
    pub disk_warn_gb: f64,
    pub timed: bool,
    pub spec: String,
    /// where leases work: `wt/`, `lane/`, `target/`, `tmp/`, `recovered/`
    pub work: PathBuf,
    /// taskset CPU lists for shared kinds and for `timed` (empty: no pinning)
    pub cpus_shared: String,
    pub cpus_timed: String,
}

pub fn home_dir() -> PathBuf {
    crate::os::home_dir()
}

pub fn conf_path() -> PathBuf {
    crate::proto::home().join("lease.conf")
}

impl LeaseConf {
    /// `~/.makepad/devhub/lease.conf`, defaults where it is missing.
    pub fn load() -> LeaseConf {
        LeaseConf::parse(&std::fs::read_to_string(conf_path()).unwrap_or_default(), &home_dir())
    }

    pub fn parse(text: &str, home: &Path) -> LeaseConf {
        let mut c = LeaseConf {
            role: Role::Pool,
            slots: Vec::new(),
            disk_floor_gb: 25.0,
            disk_warn_gb: 50.0,
            timed: true,
            spec: String::new(),
            work: home.join("devhub"),
            cpus_shared: String::new(),
            cpus_timed: String::new(),
        };
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            let Some((k, v)) = line.split_once('=') else { continue };
            let (k, v) = (k.trim(), v.trim());
            match k {
                "role" => c.role = Role::parse(v).unwrap_or(c.role),
                "disk_floor_gb" => c.disk_floor_gb = v.parse().unwrap_or(c.disk_floor_gb),
                "disk_warn_gb" => c.disk_warn_gb = v.parse().unwrap_or(c.disk_warn_gb),
                "timed" => c.timed = v != "off",
                "spec" => c.spec = v.to_string(),
                "work" => c.work = v.strip_prefix("~/").map_or_else(|| PathBuf::from(v), |r| home.join(r)),
                "cpus.shared" if valid_cpus(v) => c.cpus_shared = v.to_string(),
                "cpus.timed" if valid_cpus(v) => c.cpus_timed = v.to_string(),
                _ => {
                    if let (Some(kind), Ok(n)) = (k.strip_prefix("slots.").and_then(Kind::parse), v.parse::<u32>()) {
                        c.slots.retain(|(x, _)| *x != kind);
                        c.slots.push((kind, n));
                    }
                }
            }
        }
        c
    }

    /// Slots per kind: the conf's, else build 2, test 2, screen 3, long 2, gate 1, gpu = #GPUs,
    /// timed 1 (0 with `timed = off`). `inspect` is not counted.
    pub fn slots(&self, gpus: u32) -> Vec<(Kind, u32)> {
        let defaults = [(Kind::Build, 2), (Kind::Test, 2), (Kind::Screen, 3), (Kind::Long, 2), (Kind::Gate, 1), (Kind::Gpu, gpus), (Kind::Timed, self.timed as u32)];
        defaults.iter().map(|(k, d)| (*k, self.slots.iter().find(|(x, _)| x == k).map_or(*d, |(_, n)| *n))).collect()
    }
}

/// Whether the master may set `key = value` in a node's lease.conf (`work` stays the node's own:
/// moving the work dir under running leases is not a remote switch).
pub fn settable(key: &str, value: &str) -> Result<(), String> {
    let num = || value.parse::<f64>().map(|_| ()).map_err(|_| format!("{key} takes a number"));
    match key {
        "role" => Role::parse(value).map(|_| ()).ok_or_else(|| "role = pool|suite|bench|user-wm|device".to_string()),
        "disk_floor_gb" | "disk_warn_gb" => num(),
        "timed" => matches!(value, "on" | "off").then_some(()).ok_or_else(|| "timed = on|off".to_string()),
        "spec" => (!value.is_empty() && value.len() <= 64 && value.bytes().all(|b| b.is_ascii_graphic())).then_some(()).ok_or_else(|| "spec: one word".to_string()),
        "cpus.shared" | "cpus.timed" => (value.is_empty() || valid_cpus(value)).then_some(()).ok_or_else(|| format!("{key}: a CPU list like 8-15,24-31")),
        k => match k.strip_prefix("slots.").and_then(Kind::parse) {
            Some(_) => value.parse::<u32>().map(|_| ()).map_err(|_| format!("{key} takes a count")),
            None => Err(format!("`{key}` is not a lease.conf key the master sets (role, slots.<kind>, disk_floor_gb, disk_warn_gb, timed, spec, cpus.shared, cpus.timed)")),
        },
    }
}

/// `text` with each `key = value` of `pairs` set: the key's line replaced in place (comments
/// kept), or appended.
pub fn conf_with(text: &str, pairs: &[(String, String)]) -> String {
    let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
    for (k, v) in pairs {
        let at = lines.iter().position(|l| l.split('#').next().and_then(|b| b.split_once('=')).is_some_and(|(lk, _)| lk.trim() == k));
        let line = format!("{k} = {v}");
        match at {
            Some(i) => {
                let comment = lines[i].find('#').map(|c| format!("  {}", &lines[i][c..])).unwrap_or_default();
                lines[i] = format!("{line}{comment}");
            }
            None => lines.push(line),
        }
    }
    format!("{}\n", lines.join("\n"))
}

fn valid_cpus(v: &str) -> bool {
    !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit() || b == b',' || b == b'-')
}

/// Measures this machine now (`node`: its devhub name; `apply`: DEVHUB2's applier kind).
pub fn measure(node: &str, conf: &LeaseConf, apply: &str) -> Caps {
    let mut c = os::hardware();
    c.node = node.to_string();
    let (free, total) = disk_space(&conf.work).unwrap_or((0, 0));
    c.disk_free_gb = free as f64 / GB;
    c.disk_total_gb = total as f64 / GB;
    c.role = Some(conf.role);
    c.spec = conf.spec.clone();
    c.slots = conf.slots(c.gpus.len() as u32);
    c.disk_floor_gb = conf.disk_floor_gb;
    c.disk_warn_gb = conf.disk_warn_gb;
    c.timed = conf.timed;
    c.apply = apply.to_string();
    c
}

pub const GB: f64 = (1u64 << 30) as f64;

/// `model name` and the number of logical CPUs from `/proc/cpuinfo`.
pub fn parse_cpuinfo(text: &str) -> (String, u32) {
    let mut model = String::new();
    let mut n = 0;
    for line in text.lines() {
        let Some((k, v)) = line.split_once(':') else { continue };
        match k.trim() {
            "processor" => n += 1,
            // x86: `model name`; arm64 kernels may only give `Hardware` / `CPU part`
            "model name" | "Hardware" if model.is_empty() => model = v.trim().to_string(),
            _ => {}
        }
    }
    (model, n)
}

/// (MemTotal, MemAvailable) in GB from `/proc/meminfo`.
pub fn parse_meminfo(text: &str) -> (f64, f64) {
    let kb = |key: &str| text.lines().find(|l| l.starts_with(key)).and_then(|l| l.split_whitespace().nth(1)).and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0) / (1024.0 * 1024.0);
    (kb("MemTotal:"), kb("MemAvailable:"))
}

pub fn parse_loadavg(text: &str) -> f64 {
    text.split_whitespace().next().and_then(|v| v.parse().ok()).unwrap_or(0.0)
}

/// `nvidia-smi --query-gpu=name,memory.total --format=csv,noheader,nounits`: (name, VRAM GB).
pub fn parse_nvidia(text: &str) -> Vec<(String, f64)> {
    text.lines()
        .filter_map(|l| {
            let (name, mib) = l.rsplit_once(',')?;
            Some((name.trim().to_string(), mib.trim().parse::<f64>().ok()? / 1024.0))
        })
        .filter(|(n, _)| !n.is_empty())
        .collect()
}

/// Whether a `/sys/class/power_supply` entry is the machine's battery (`type`, `scope` files).
pub fn system_battery(kind: &str, scope: &str) -> bool {
    kind.trim() == "Battery" && scope.trim() != "Device"
}

/// A battery from `/sys/class/power_supply/<x>/{capacity,status}`: (percent, charging).
pub fn parse_battery(capacity: &str, status: &str) -> Option<(u32, bool)> {
    let pct = capacity.trim().parse().ok()?;
    Some((pct, matches!(status.trim(), "Charging" | "Full")))
}

/// (free, total) bytes of the volume holding `path` (its nearest existing ancestor).
pub fn disk_space(path: &Path) -> Option<(u64, u64)> {
    let mut p = path.to_path_buf();
    while !p.is_dir() {
        p = p.parent()?.to_path_buf();
    }
    os::disk_space(&p)
}

// Android: bionic shares the /proc files, statvfs layout and power-supply nodes
#[cfg(any(target_os = "linux", target_os = "android"))]
mod os {
    use super::*;

    pub fn hardware() -> Caps {
        let read = |p: &str| std::fs::read_to_string(p).unwrap_or_default();
        let (cpu, cores) = parse_cpuinfo(&read("/proc/cpuinfo"));
        let (ram, avail) = parse_meminfo(&read("/proc/meminfo"));
        let gpu = crate::top::gpu_use();
        // CPU use since the previous measure (the master asks every 10 s)
        static LAST: std::sync::Mutex<Option<(u64, u64)>> = std::sync::Mutex::new(None);
        let now = crate::top::parse_proc_stat(&read("/proc/stat"));
        let cpu_pct = match (LAST.lock().unwrap().replace(now.unwrap_or((0, 0))), now) {
            (Some(a), Some(b)) if b.1 > a.1 => Some(crate::top::cpu_pct(a, b)),
            _ => None,
        };
        Caps {
            cpu_pct,
            gpu_busy: gpu.iter().map(|g| (g.util_pct, g.mem_used_gb)).collect(),
            arch: std::env::consts::ARCH.to_string(),
            os: std::env::consts::OS.to_string(),
            cpu,
            cores: cores.max(1),
            load1: parse_loadavg(&read("/proc/loadavg")),
            ram_gb: ram,
            ram_avail_gb: avail,
            gpus: gpu.iter().map(|g| (g.name.clone(), g.mem_total_gb)).collect(),
            display: display(),
            battery: battery(),
            ..Default::default()
        }
    }

    /// A live makepad WM socket (`$XDG_RUNTIME_DIR/makepad-wm-<n>` that accepts), else a Wayland one.
    fn display() -> Option<String> {
        let rt = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(format!("/run/user/{}", uid())));
        let mut wayland = None;
        for e in std::fs::read_dir(&rt).ok()?.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with("makepad-wm-") && !name.ends_with(".lock") && std::os::unix::net::UnixStream::connect(e.path()).is_ok() {
                return Some(format!("wm:{name}"));
            }
            if name.starts_with("wayland-") && !name.ends_with(".lock") {
                wayland = Some(format!("wayland:{name}"));
            }
        }
        wayland.or_else(|| std::env::var("WAYLAND_DISPLAY").ok().filter(|w| !w.is_empty()).map(|w| format!("wayland:{w}")))
    }

    fn uid() -> u32 {
        extern "C" {
            fn getuid() -> u32;
        }
        unsafe { getuid() }
    }

    /// The machine's own battery: a power supply of type Battery whose scope is not `Device`
    /// (wireless mice and keyboards report theirs as `scope = Device`).
    fn battery() -> Option<(u32, bool)> {
        for e in std::fs::read_dir("/sys/class/power_supply").ok()?.flatten() {
            let p = e.path();
            let read = |f: &str| std::fs::read_to_string(p.join(f)).unwrap_or_default();
            if system_battery(&read("type"), &read("scope")) {
                if let Some(b) = parse_battery(&std::fs::read_to_string(p.join("capacity")).unwrap_or_default(), &std::fs::read_to_string(p.join("status")).unwrap_or_default()) {
                    return Some(b);
                }
            }
        }
        None
    }

    pub fn disk_space(path: &Path) -> Option<(u64, u64)> {
        use std::os::unix::ffi::OsStrExt;
        // 64-bit glibc and bionic share this layout
        #[repr(C)]
        struct StatVfs {
            bsize: u64,
            frsize: u64,
            blocks: u64,
            bfree: u64,
            bavail: u64,
            files: u64,
            ffree: u64,
            favail: u64,
            fsid: u64,
            flag: u64,
            namemax: u64,
            spare: [i32; 6],
        }
        extern "C" {
            fn statvfs(path: *const std::ffi::c_char, out: *mut StatVfs) -> i32;
        }
        let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
        let mut s: StatVfs = unsafe { std::mem::zeroed() };
        if unsafe { statvfs(c.as_ptr(), &mut s) } != 0 {
            return None;
        }
        let unit = if s.frsize > 0 { s.frsize } else { s.bsize };
        Some((s.bavail.saturating_mul(unit), s.blocks.saturating_mul(unit)))
    }
}

#[cfg(target_os = "macos")]
mod os {
    use super::*;

    extern "C" {
        fn sysctlbyname(name: *const u8, oldp: *mut core::ffi::c_void, oldlenp: *mut usize, newp: *mut core::ffi::c_void, newlen: usize) -> i32;
    }

    fn sysctl_u64(name: &str) -> u64 {
        let n = format!("{name}\0");
        let mut v = 0u64;
        let mut len = std::mem::size_of::<u64>();
        if unsafe { sysctlbyname(n.as_ptr(), &mut v as *mut u64 as *mut _, &mut len, std::ptr::null_mut(), 0) } != 0 {
            return 0;
        }
        // 4-byte answers (hw.logicalcpu) land in the low half
        if len == 4 { v & 0xffff_ffff } else { v }
    }

    fn sysctl_str(name: &str) -> String {
        let n = format!("{name}\0");
        let mut buf = [0u8; 256];
        let mut len = buf.len();
        if unsafe { sysctlbyname(n.as_ptr(), buf.as_mut_ptr() as *mut _, &mut len, std::ptr::null_mut(), 0) } != 0 {
            return String::new();
        }
        String::from_utf8_lossy(&buf[..len]).trim_end_matches('\0').to_string()
    }

    pub fn hardware() -> Caps {
        let m = crate::procinfo::machine();
        Caps {
            arch: std::env::consts::ARCH.to_string(),
            os: "macos".into(),
            cpu: sysctl_str("machdep.cpu.brand_string"),
            cores: (sysctl_u64("hw.logicalcpu") as u32).max(1),
            load1: m.load1,
            ram_gb: sysctl_u64("hw.memsize") as f64 / GB,
            ram_avail_gb: m.mem_avail_mb as f64 / 1024.0,
            // a Mac shows windows on its own session; the GPU is the SoC's (not a lease resource)
            display: Some("macos".into()),
            ..Default::default()
        }
    }

    pub fn disk_space(path: &Path) -> Option<(u64, u64)> {
        use std::os::unix::ffi::OsStrExt;
        #[repr(C)]
        struct StatFs {
            bsize: u32,
            iosize: i32,
            blocks: u64,
            bfree: u64,
            bavail: u64,
            files: u64,
            ffree: u64,
            fsid: [i32; 2],
            owner: u32,
            fstype: u32,
            flags: u32,
            fssubtype: u32,
            fstypename: [u8; 16],
            mntonname: [u8; 1024],
            mntfromname: [u8; 1024],
            flags_ext: u32,
            reserved: [u32; 7],
        }
        extern "C" {
            #[cfg_attr(target_arch = "x86_64", link_name = "statfs$INODE64")]
            fn statfs(path: *const std::ffi::c_char, out: *mut StatFs) -> i32;
        }
        let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
        let mut s: StatFs = unsafe { std::mem::zeroed() };
        if unsafe { statfs(c.as_ptr(), &mut s) } != 0 {
            return None;
        }
        Some((s.bavail.saturating_mul(s.bsize as u64), s.blocks.saturating_mul(s.bsize as u64)))
    }
}

#[cfg(windows)]
mod os {
    use super::*;

    pub fn hardware() -> Caps {
        let (total, avail) = crate::os::memory();
        // CPU use since the previous measure (the master asks every 10 s)
        static LAST: std::sync::Mutex<Option<(u64, u64)>> = std::sync::Mutex::new(None);
        let now = crate::os::cpu_times();
        let cpu_pct = match (LAST.lock().unwrap().replace(now.unwrap_or((0, 0))), now) {
            (Some(a), Some(b)) if b.1 > a.1 => Some(crate::top::cpu_pct(a, b)),
            _ => None,
        };
        let gpu = crate::top::gpu_use();
        Caps {
            cpu_pct,
            gpu_busy: gpu.iter().map(|g| (g.util_pct, g.mem_used_gb)).collect(),
            arch: std::env::consts::ARCH.to_string(),
            os: "windows".into(),
            cpu: crate::os::cpu_name(),
            cores: std::thread::available_parallelism().map(|n| n.get() as u32).unwrap_or(1),
            // no load average on Windows: the busy share of the cores stands in
            load1: cpu_pct.map_or(0.0, |p| p / 100.0 * std::thread::available_parallelism().map(|n| n.get() as f64).unwrap_or(1.0)),
            ram_gb: total as f64 / GB,
            ram_avail_gb: avail as f64 / GB,
            gpus: gpu.iter().map(|g| (g.name.clone(), g.mem_total_gb)).collect(),
            // the user's desktop session (a lease's windows go there only with display=1)
            display: Some("windows".into()),
            battery: crate::os::battery(),
            ..Default::default()
        }
    }

    pub fn disk_space(path: &Path) -> Option<(u64, u64)> {
        crate::os::disk_space(path)
    }
}

#[cfg(not(any(target_os = "linux", target_os = "android", target_os = "macos", windows)))]
mod os {
    use super::*;

    pub fn hardware() -> Caps {
        Caps { arch: std::env::consts::ARCH.to_string(), os: std::env::consts::OS.to_string(), cores: 1, ..Default::default() }
    }

    pub fn disk_space(_path: &Path) -> Option<(u64, u64)> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpuinfo_meminfo_loadavg_nvidia_battery() {
        let cpu = "processor\t: 0\nvendor_id\t: AuthenticAMD\nmodel name\t: AMD Ryzen 7 9800X3D 8-Core Processor\n\nprocessor\t: 1\nmodel name\t: AMD Ryzen 7 9800X3D 8-Core Processor\n";
        assert_eq!(parse_cpuinfo(cpu), ("AMD Ryzen 7 9800X3D 8-Core Processor".to_string(), 2));
        let (t, a) = parse_meminfo("MemTotal:       65536000 kB\nMemFree:  100 kB\nMemAvailable:   32768000 kB\n");
        assert!((t - 62.5).abs() < 0.01 && (a - 31.25).abs() < 0.01);
        assert_eq!(parse_loadavg("3.52 2.10 1.00 2/900 1234\n"), 3.52);
        let g = parse_nvidia("NVIDIA GeForce RTX 5090, 32607\nNVIDIA GeForce RTX 4090, 24564\n");
        assert_eq!(g.len(), 2);
        assert_eq!(g[0].0, "NVIDIA GeForce RTX 5090");
        assert!((g[1].1 - 23.99).abs() < 0.01);
        assert!(parse_nvidia("").is_empty());
        assert_eq!(parse_battery("87\n", "Discharging\n"), Some((87, false)));
        assert_eq!(parse_battery("100", "Full"), Some((100, true)));
        assert_eq!(parse_battery("", "Charging"), None);
        assert!(system_battery("Battery\n", ""));
        assert!(system_battery("Battery\n", "System\n"));
        assert!(!system_battery("Battery\n", "Device\n"));
        assert!(!system_battery("Mains\n", ""));
    }

    #[test]
    fn lease_conf() {
        let home = Path::new("/home/arch");
        let d = LeaseConf::parse("", home);
        assert_eq!((d.role, d.disk_floor_gb, d.disk_warn_gb, d.timed), (Role::Pool, 25.0, 50.0, true));
        assert_eq!(d.work, PathBuf::from("/home/arch/devhub"));
        assert_eq!(d.slots(1), vec![(Kind::Build, 2), (Kind::Test, 2), (Kind::Screen, 3), (Kind::Long, 2), (Kind::Gate, 1), (Kind::Gpu, 1), (Kind::Timed, 1)]);
        let c = LeaseConf::parse("# the 166 conf\nrole = user-wm\nslots.build = 1\nslots.nope = 4\nslots.test=6\ndisk_floor_gb = 40\ntimed = off\nspec = 7800X3D+4090  # comment\nwork = ~/scratch/dh\ncpus.shared = 8-15,24-31\ncpus.timed = rm -rf\n", home);
        assert_eq!(c.role, Role::UserWm);
        assert_eq!(c.disk_floor_gb, 40.0);
        assert!(!c.timed);
        assert_eq!(c.spec, "7800X3D+4090");
        assert_eq!(c.work, PathBuf::from("/home/arch/scratch/dh"));
        assert_eq!(c.cpus_shared, "8-15,24-31");
        assert_eq!(c.cpus_timed, "");
        let s = c.slots(0);
        assert!(s.contains(&(Kind::Build, 1)) && s.contains(&(Kind::Test, 6)) && s.contains(&(Kind::Timed, 0)) && s.contains(&(Kind::Gpu, 0)));
    }

    #[test]
    fn master_sets_role_and_budgets() {
        assert!(settable("role", "user-wm").is_ok() && settable("role", "boss").is_err());
        assert!(settable("slots.build", "3").is_ok() && settable("slots.build", "x").is_err() && settable("slots.nope", "1").is_err());
        assert!(settable("work", "/tmp").is_err() && settable("cpus.shared", "rm -rf").is_err());
        let t = conf_with("# 165\nrole = pool  # was\nslots.build = 2\n", &[("role".into(), "user-wm".into()), ("cpus.shared".into(), "8-15,24-31".into())]);
        assert_eq!(t, "# 165\nrole = user-wm  # was\nslots.build = 2\ncpus.shared = 8-15,24-31\n");
        let c = LeaseConf::parse(&t, Path::new("/h"));
        assert_eq!((c.role, c.cpus_shared.as_str()), (Role::UserWm, "8-15,24-31"));
    }

    #[test]
    fn measures_this_machine() {
        let c = measure("t", &LeaseConf::parse("", &std::env::temp_dir()), "checkout");
        assert!(c.cores >= 1 && c.ram_gb > 0.0 && c.disk_total_gb > 0.0, "{c:?}");
        assert_eq!(Caps::parse(&c.to_json()).map(|p| p.cores), Some(c.cores));
    }
}
