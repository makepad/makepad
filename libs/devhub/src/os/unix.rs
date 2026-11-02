//! Unix (Linux, macOS): a lease run is its own session (setsid), under `systemd-run --user --scope`
//! when there is a user bus, at `nice -n 10`, optionally pinned with taskset; it is ended through
//! its process group.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus};

/// What keeps a run's processes together beyond its pid: nothing on Unix (the process group is
/// named by the leader's pid).
#[derive(Default)]
pub struct Hold;

impl Hold {
    pub fn none() -> Hold {
        Hold
    }
}

/// The argv a lease run executes: the scope, nice and pinning wrappers, then `argv`.
pub fn lease_argv(argv: Vec<String>, unit: Option<&str>, cpus: &str, share: Option<crate::lease::Cost>) -> Vec<String> {
    let mut full: Vec<String> = Vec::new();
    if let Some(u) = unit {
        full.extend(["systemd-run", "--user", "--scope", "--quiet"].map(String::from));
        full.push(format!("--unit={u}"));
        if let Some(cost) = share.filter(|_| cfg!(target_os = "linux")) {
            full.extend(["-p".into(), format!("CPUQuota={:.2}%", cost.cores * 100.0), "-p".into(), format!("MemoryMax={}G", cost.ram_gb * 1.5)]);
        }
        full.push("--".into());
    }
    full.extend(["nice", "-n", "10"].map(String::from));
    if !cpus.is_empty() {
        full.extend(["taskset".to_string(), "-c".to_string(), cpus.to_string()]);
    }
    full.extend(argv);
    full
}

/// Keep the scope alive until its final counters are saved, including runs shorter than a
/// heartbeat. Arguments are positional shell parameters, never interpolated shell source.
pub fn metered_argv(argv: Vec<String>, unit: Option<&str>, usage: &Path) -> Vec<String> {
    if !cfg!(target_os = "linux") || unit.is_none() { return argv; }
    let script = r#"usage=$1
shift
group=
while IFS=: read -r hierarchy controllers path; do
    if [ "$hierarchy" = 0 ]; then group=/sys/fs/cgroup$path; fi
done < /proc/self/cgroup
printf '%s\n' "$group" > "$usage.cgroup"
record() {
    if [ -n "$group" ]; then
        if [ -r "$group/memory.peak" ] && read -r peak < "$group/memory.peak"; then printf 'peak %s\n' "$peak"; fi
        if [ -r "$group/cpu.stat" ]; then
            while read -r key value; do
                if [ "$key" = usage_usec ]; then printf 'cpu %s\n' "$value"; fi
            done < "$group/cpu.stat"
        fi
    fi
}
trap 'record > "$usage"' 0
trap 'exit 143' TERM
"$@" &
child=$!
printf '%s\n' "$child" > "$usage.pid"
wait "$child"
status=$?
exit "$status"
"#;
    let mut full = vec!["sh".into(), "-c".into(), script.into(), "devhub-usage".into(), usage.to_string_lossy().into_owned()];
    full.extend(argv);
    full
}

/// Current scope high-water mark and cumulative CPU. The in-scope wrapper saves these again
/// at exit because systemd removes an empty scope before its parent can read the cgroup.
pub fn scope_usage(pid: u32, start: u64, unit: Option<&str>, usage: &Path) -> (Option<u64>, Option<f64>) {
    if !cfg!(target_os = "linux") { return (None, None); }
    let Some(unit) = unit else { return (None, None) };
    let group = std::fs::read_to_string(usage.with_extension("usage.cgroup")).ok().map(|s| PathBuf::from(s.trim())).or_else(|| {
        if proc_start(pid) != Some(start) { return None; }
        std::fs::read_to_string(format!("/proc/{pid}/cgroup")).ok().and_then(|s| s.lines().find_map(|s| s.strip_prefix("0::").map(|s| Path::new("/sys/fs/cgroup").join(s.trim_start_matches('/')))))
    });
    let Some(group) = group.filter(|g| g.starts_with("/sys/fs/cgroup") && g.file_name().is_some_and(|n| n == format!("{unit}.scope").as_str())) else { return (None, None) };
    let peak = std::fs::read_to_string(group.join("memory.peak")).ok().and_then(|s| s.trim().parse().ok());
    let cpu = std::fs::read_to_string(group.join("cpu.stat")).ok().and_then(|s| s.lines().find_map(|s| s.strip_prefix("usage_usec ").and_then(|s| s.parse::<f64>().ok()))).map(|us| us / 1_000_000.0);
    (peak, cpu)
}

/// RSS in bytes across recorded process groups (ps reports KiB), in one process-table
/// snapshot per lease. A departed leader can still have children; a reused pid is excluded.
pub fn groups_rss(groups: &[(u32, u64)]) -> Option<u64> {
    let groups: Vec<_> = groups.iter().filter(|(pid, start)| *pid != 0 && proc_start(*pid).is_none_or(|s| s == *start)).map(|(pid, _)| pid.to_string()).collect();
    if groups.is_empty() { return None; }
    let out = Command::new("ps").args(["-o", "rss=", "-g", &groups.join(",")]).output().ok()?;
    if !out.status.success() { return None; }
    let rss: Vec<u64> = String::from_utf8_lossy(&out.stdout).split_whitespace().map(str::parse).collect::<Result<_, _>>().ok()?;
    (!rss.is_empty()).then(|| rss.iter().sum::<u64>() * 1024)
}

/// Reap just this child, retaining user + system CPU including its waited-for descendants.
/// `Child::wait` discards rusage; a process-wide RUSAGE_CHILDREN delta would mix leases.
pub fn wait_usage(child: &mut Child) -> std::io::Result<(ExitStatus, Option<f64>)> {
    use std::os::raw::c_long;
    use std::os::unix::process::ExitStatusExt;
    #[repr(C)]
    #[derive(Default)]
    struct Timeval {
        sec: c_long,
        #[cfg(target_os = "macos")]
        usec: i32,
        #[cfg(not(target_os = "macos"))]
        usec: c_long,
    }
    #[repr(C)]
    #[derive(Default)]
    struct RUsage { user: Timeval, system: Timeval, rest: [c_long; 14] }
    extern "C" { fn wait4(pid: i32, status: *mut i32, options: i32, usage: *mut RUsage) -> i32; }
    let mut status = 0;
    let mut usage = RUsage::default();
    loop {
        if unsafe { wait4(child.id() as i32, &mut status, 0, &mut usage) } >= 0 {
            let cpu = (usage.user.sec + usage.system.sec) as f64 + (usage.user.usec + usage.system.usec) as f64 / 1_000_000.0;
            return Ok((ExitStatus::from_raw(status), Some(cpu)));
        }
        let e = std::io::Error::last_os_error();
        if e.kind() != std::io::ErrorKind::Interrupted { return Err(e); }
    }
}

/// Starts a lease run in its own session and process group (the group is what end signals).
pub fn lease_spawn(cmd: &mut Command) -> std::io::Result<(Child, Hold)> {
    use std::os::unix::process::CommandExt;
    extern "C" {
        fn setsid() -> i32;
    }
    unsafe {
        cmd.pre_exec(|| {
            setsid();
            Ok(())
        });
    }
    Ok((cmd.spawn()?, Hold))
}

/// The exit code, 128 + the signal for a run a signal ended.
pub fn exit_code(s: ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    s.code().unwrap_or_else(|| 128 + s.signal().unwrap_or(0))
}

/// Whether `pid` lives (signal 0; EPERM still means it lives).
pub fn pid_alive(pid: u32) -> bool {
    extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    unsafe { kill(pid as i32, 0) == 0 || std::io::Error::last_os_error().raw_os_error() == Some(1) }
}

pub fn home_dir() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

/// `program` at nice 15 (background builds).
pub fn niced(program: &std::ffi::OsStr) -> Command {
    let mut cmd = Command::new("nice");
    cmd.arg("-n").arg("15").arg(program);
    cmd
}

/// The login name.
pub fn login() -> Option<String> {
    std::env::var("USER").ok()
}

/// (allocated bytes, mtime in s) of one directory entry.
pub fn on_disk(m: &std::fs::Metadata) -> (u64, u64) {
    use std::os::unix::fs::MetadataExt;
    (m.blocks() * 512, m.mtime().max(0) as u64)
}

/// A process's start time (Linux: clock ticks since boot; macOS: µs since the epoch).
pub fn proc_start(pid: u32) -> Option<u64> {
    proc_stat(pid).map(|s| s.start)
}

struct Stat {
    start: u64,
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pgrp: u32,
    zombie: bool,
}

#[cfg(target_os = "linux")]
fn proc_stat(pid: u32) -> Option<Stat> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let f: Vec<&str> = stat[stat.rfind(')')? + 2..].split_whitespace().collect();
    // f[0] = field 3 (state), f[2] = 5 (pgrp), f[19] = 22 (starttime)
    Some(Stat { start: f.get(19)?.parse().ok()?, pgrp: f.get(2)?.parse().ok()?, zombie: f.first() == Some(&"Z") })
}

#[cfg(target_os = "macos")]
fn proc_stat(pid: u32) -> Option<Stat> {
    #[repr(C)]
    struct BsdInfo {
        flags: u32,
        status: u32,
        xstatus: u32,
        pid: u32,
        ppid: u32,
        uid: u32,
        gid: u32,
        ruid: u32,
        rgid: u32,
        svuid: u32,
        svgid: u32,
        rfu: u32,
        comm: [u8; 16],
        name: [u8; 32],
        nfiles: u32,
        pgid: u32,
        pjobc: u32,
        tdev: u32,
        tpgid: u32,
        nice: i32,
        start_sec: u64,
        start_usec: u64,
    }
    extern "C" {
        fn proc_pidinfo(pid: i32, flavor: i32, arg: u64, buffer: *mut core::ffi::c_void, size: i32) -> i32;
    }
    let mut b: BsdInfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<BsdInfo>() as i32;
    // PROC_PIDTBSDINFO = 3
    if unsafe { proc_pidinfo(pid as i32, 3, 0, &mut b as *mut BsdInfo as *mut _, size) } != size {
        return None;
    }
    // SZOMB = 5
    Some(Stat { start: b.start_sec * 1_000_000 + b.start_usec, pgrp: b.pgid, zombie: b.status == 5 })
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn proc_stat(_pid: u32) -> Option<Stat> {
    None
}

/// Whether `pid` is in the process group led by `leader` (a lease run is its own session).
pub fn in_group(pid: u32, leader: u32, _hold: &Hold) -> bool {
    proc_stat(pid).is_some_and(|s| s.pgrp == leader && !s.zombie)
}

/// The recorded leader still is the process we started (not gone, not a reused pid).
pub fn same(pid: u32, start: u64) -> bool {
    proc_stat(pid).is_some_and(|s| !s.zombie && s.start == start)
}

/// Whether the run's process group still has a live member. Its pgid is the leader's pid,
/// which the OS does not hand out again while the group exists; a reused leader pid (a
/// different start time) means the group is gone.
pub fn group_alive(pid: u32, start: u64, _hold: &Hold) -> bool {
    if let Some(s) = proc_stat(pid) {
        if s.start != start {
            return false;
        }
        if !s.zombie {
            return true;
        }
    }
    #[cfg(target_os = "linux")]
    {
        for e in std::fs::read_dir("/proc").into_iter().flatten().flatten() {
            if let Ok(p) = e.file_name().to_string_lossy().parse::<u32>() {
                if p != pid && proc_stat(p).is_some_and(|s| s.pgrp == pid && !s.zombie) {
                    return true;
                }
            }
        }
    }
    false
}

/// TERM (`hard`: KILL) one process.
pub fn signal_one(pid: u32, hard: bool) {
    extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }
    unsafe {
        kill(pid as i32, if hard { 9 } else { 15 });
    }
}

/// The live processes of this user whose environment has `marker` (`NAME=value`), whatever session
/// or process group they moved to.
#[cfg(target_os = "linux")]
pub fn with_env(marker: &str) -> Vec<u32> {
    env_values(marker).into_iter().filter(|(_, v)| v.is_empty()).map(|(p, _)| p).collect()
}

/// The live processes of this user with an environment entry starting `prefix` (`NAME=`), and
/// what follows it.
#[cfg(target_os = "linux")]
pub fn env_values(prefix: &str) -> Vec<(u32, String)> {
    let mut out = Vec::new();
    for e in std::fs::read_dir("/proc").into_iter().flatten().flatten() {
        let Ok(pid) = e.file_name().to_string_lossy().parse::<u32>() else { continue };
        if let Ok(env) = std::fs::read(e.path().join("environ")) {
            if let Some(v) = env.split(|b| *b == 0).find_map(|v| v.strip_prefix(prefix.as_bytes())) {
                out.push((pid, String::from_utf8_lossy(v).into_owned()));
            }
        }
    }
    out
}

/// As on Linux: the environment comes with a process's arguments (KERN_PROCARGS2).
#[cfg(target_os = "macos")]
pub fn with_env(marker: &str) -> Vec<u32> {
    env_values(marker).into_iter().filter(|(_, v)| v.is_empty()).map(|(p, _)| p).collect()
}

#[cfg(target_os = "macos")]
pub fn env_values(prefix: &str) -> Vec<(u32, String)> {
    extern "C" {
        fn proc_listallpids(buffer: *mut core::ffi::c_void, size: i32) -> i32;
        fn sysctl(name: *const i32, namelen: u32, oldp: *mut u8, oldlenp: *mut usize, newp: *const u8, newlen: usize) -> i32;
    }
    const CTL_KERN: i32 = 1;
    const KERN_PROCARGS2: i32 = 49;
    let mut pids = vec![0i32; 8192];
    let n = unsafe { proc_listallpids(pids.as_mut_ptr() as *mut _, (pids.len() * 4) as i32) };
    let mut buf = vec![0u8; 1 << 20];
    let mut out = Vec::new();
    for &pid in pids.iter().take(n.max(0) as usize).filter(|p| **p > 0) {
        let mib = [CTL_KERN, KERN_PROCARGS2, pid];
        let mut len = buf.len();
        if unsafe { sysctl(mib.as_ptr(), 3, buf.as_mut_ptr(), &mut len, std::ptr::null(), 0) } == 0 {
            if let Some(v) = buf[..len].split(|b| *b == 0).find_map(|v| v.strip_prefix(prefix.as_bytes())) {
                out.push((pid as u32, String::from_utf8_lossy(v).into_owned()));
            }
        }
    }
    out
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn with_env(_marker: &str) -> Vec<u32> {
    Vec::new()
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn env_values(_prefix: &str) -> Vec<(u32, String)> {
    Vec::new()
}

/// TERM (`hard`: KILL) the run's process group.
pub fn signal_group(pid: u32, hard: bool, _hold: &Hold) {
    makepad_remote::group::signal(pid, hard);
}


/// The measuring walk runs at nice 10 (Linux: per thread).
pub fn nice_this_thread() {
    #[cfg(target_os = "linux")]
    {
        extern "C" {
            fn setpriority(which: i32, who: u32, prio: i32) -> i32;
            fn gettid() -> i32;
        }
        unsafe {
            setpriority(0, gettid() as u32, 10);
        }
    }
}


/// Raises the soft open-files limit to the hard one (macOS caps it at 10240), as hygiene: a hub
/// holds a descriptor or two per link and per lease, and a 256 soft limit is a small ceiling.
/// Returns the soft limit now.
pub fn raise_fd_limit() -> u64 {
    #[repr(C)]
    struct RLimit {
        cur: u64,
        max: u64,
    }
    extern "C" {
        fn getrlimit(resource: i32, rlim: *mut RLimit) -> i32;
        fn setrlimit(resource: i32, rlim: *const RLimit) -> i32;
    }
    const RLIMIT_NOFILE: i32 = 8;
    let mut r = RLimit { cur: 0, max: 0 };
    if unsafe { getrlimit(RLIMIT_NOFILE, &mut r) } != 0 {
        return 0;
    }
    #[cfg(target_os = "macos")]
    let want = r.max.min(10240);
    #[cfg(not(target_os = "macos"))]
    let want = r.max;
    if want > r.cur {
        let n = RLimit { cur: want, max: r.max };
        if unsafe { setrlimit(RLIMIT_NOFILE, &n) } == 0 {
            return want;
        }
    }
    r.cur
}
