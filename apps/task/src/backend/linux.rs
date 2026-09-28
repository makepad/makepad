//! Linux backend — everything from `/proc`, no external processes.
//!
//! This module compiles on every target (it only uses `std::fs`) so its
//! parsers stay unit-testable from a mac; [`super::new_backend`] only selects
//! [`LinuxBackend`] under `cfg(target_os = "linux")`.
//!
//! | data | file |
//! |---|---|
//! | per-core cpu ticks, boot time | `/proc/stat` |
//! | memory | `/proc/meminfo` |
//! | network bytes | `/proc/net/dev` |
//! | disk bytes | `/proc/diskstats` (whole devices, sectors × 512) |
//! | pid/ppid/state/cpu/threads/rss/start | `/proc/<pid>/stat` |
//! | owning uid | `/proc/<pid>/status` |
//! | peak/anon/file/swap memory, context switches | `/proc/<pid>/status` (detail) |
//! | storage bytes | `/proc/<pid>/io` `read_bytes`/`write_bytes`, every tick where readable (own processes, or root) |
//! | command line | `/proc/<pid>/cmdline` |
//! | threads | `/proc/<pid>/task/<tid>/stat` + `comm` |
//! | open files | `/proc/<pid>/fd/*` (readlink) |
//! | sockets | fd `socket:[inode]` matched in the process' own `/proc/<pid>/net/{tcp,tcp6,udp,udp6}` |
//! | mapped files | `/proc/<pid>/maps` |
//! | load / uptime | `/proc/loadavg`, `/proc/uptime` |
//!
//! Process identity is pid plus the exact start tick (`starttime` in
//! `/proc/<pid>/stat`, clock ticks since boot) combined with the boot time, so
//! a pid handed out again within the same tick is the only ambiguity left and
//! nothing survives a reboot. GPU and power are not measured on Linux.
//!
//! Network traffic per process is not measured: Linux keeps no per-process
//! socket byte counters (`/proc/<pid>/net/dev` is the whole network
//! namespace's), and attributing packets needs eBPF or a capture, so the
//! per-process network fields stay `None`.

// Only the parsers are reachable off Linux (through the tests below); the
// backend itself is constructed by `new_backend` under cfg(target_os).
#![allow(dead_code)]

use super::{
    bounded, cpu_pct_from_ticks, cpu_pct_from_time, not_collected, now_ms, Detail, DiskInfo, FileInfo,
    IdentityDetail, LibraryInfo, MemInfo, MemoryDetail, NetInfo, PortInfo, ProcDetail, ProcExtra,
    ProcInfo, ProcKey, ProcMeta, ProcState, Reading, Snapshot, SystemBackend, ThreadInfo, ThreadState, Want,
    CPU_STATES, MAX_CMDLINE_LEN, MAX_NAME_LEN,
};
use crate::metrics::Measure;
use makepad_widgets::Cx;
use std::collections::HashMap;
use std::sync::Arc;

/// `/proc/<pid>/stat` fields a process manager needs.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PidStat {
    pub pid: u32,
    pub comm: String,
    pub state: ProcState,
    pub ppid: u32,
    /// utime + stime, in USER_HZ clock ticks.
    pub cpu_ticks: u64,
    pub threads: u32,
    /// Resident set, in pages.
    pub rss_pages: u64,
    /// Field 22: clock ticks after boot at which the process started.
    pub start_ticks: u64,
    /// Fields 10 and 12: minor and major faults over the process' life.
    pub minflt: Option<u64>,
    pub majflt: Option<u64>,
    /// Fields 18 and 19: scheduling priority and nice.
    pub priority: Option<i32>,
    pub nice: Option<i32>,
    /// Field 23: virtual memory size in bytes.
    pub vsize: Option<u64>,
}

/// USER_HZ. 100 on every mainstream Linux build; the kernel reports
/// `/proc/<pid>/stat` times in these units regardless of CONFIG_HZ.
/// [`LinuxBackend::new`] asks `sysconf(_SC_CLK_TCK)` and only falls back to
/// this when that fails.
pub const USER_HZ: u64 = 100;

/// Per-core `[user, system, idle, nice]` ticks from `/proc/stat`.
///
/// The aggregate `cpu ` line is skipped — only the numbered cores are kept, so
/// the caller can average them the same way the macOS backend does.
pub fn parse_stat_cpus(text: &str) -> Vec<[u64; CPU_STATES]> {
    text.lines()
        .filter(|line| {
            line.strip_prefix("cpu")
                .and_then(|rest| rest.as_bytes().first())
                .is_some_and(u8::is_ascii_digit)
        })
        .filter_map(parse_cpu_line)
        .collect()
}

fn parse_cpu_line(line: &str) -> Option<[u64; CPU_STATES]> {
    let mut fields = line.split_whitespace().skip(1).map(str::parse::<u64>);
    let user = fields.next()?.ok()?;
    let nice = fields.next()?.ok()?;
    let system = fields.next()?.ok()?;
    let idle = fields.next()?.ok()?;
    let io_wait = fields.next().and_then(Result::ok).unwrap_or(0);
    let irq = fields.next().and_then(Result::ok).unwrap_or(0);
    let soft_irq = fields.next().and_then(Result::ok).unwrap_or(0);
    // Fold irq/softirq into system and iowait into idle, matching the four
    // buckets mach reports so `cpu_pct_from_ticks` is shared.
    Some([user, system + irq + soft_irq, idle + io_wait, nice])
}

/// The `btime` line of `/proc/stat`: boot time, seconds since the epoch.
pub fn parse_stat_btime(text: &str) -> Option<u64> {
    text.lines()
        .find_map(|line| line.strip_prefix("btime "))
        .and_then(|rest| rest.trim().parse().ok())
}

/// `/proc/meminfo` (values are in kB).
pub fn parse_meminfo(text: &str) -> MemInfo {
    let mut values: HashMap<&str, u64> = HashMap::new();
    for line in text.lines() {
        let Some((name, tail)) = line.split_once(':') else { continue };
        let kilobytes = tail.split_whitespace().next().and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
        values.insert(name.trim(), kilobytes.saturating_mul(1024));
    }
    let get = |name: &str| values.get(name).copied().unwrap_or(0);
    let total = get("MemTotal");
    let available = get("MemAvailable");
    let swap_total = get("SwapTotal");
    MemInfo {
        total,
        used: total.saturating_sub(available),
        available,
        cache: get("Cached").saturating_add(get("SReclaimable")),
        free: get("MemFree"),
        swap_total,
        swap_used: swap_total.saturating_sub(get("SwapFree")),
    }
}

/// `/proc/net/dev` → (received, sent) bytes, loopback excluded.
pub fn parse_net_dev(text: &str) -> (u64, u64) {
    text.lines().skip(2).fold((0u64, 0u64), |sum, line| {
        let Some((name, values)) = line.split_once(':') else { return sum };
        if name.trim() == "lo" {
            return sum;
        }
        let fields: Vec<&str> = values.split_whitespace().collect();
        let received = fields.first().and_then(|v| v.parse().ok()).unwrap_or(0u64);
        let sent = fields.get(8).and_then(|v| v.parse().ok()).unwrap_or(0u64);
        (sum.0.saturating_add(received), sum.1.saturating_add(sent))
    })
}

/// `/proc/diskstats` → (bytes read, bytes written) over whole block devices
/// (`sdX`, `nvmeXnY`, `vdX`, `mmcblkX`), partitions excluded so nothing is
/// counted twice. Sectors are 512 bytes in this file whatever the device.
pub fn parse_diskstats(text: &str) -> Option<(u64, u64)> {
    let mut read = 0u64;
    let mut written = 0u64;
    let mut any = false;
    for line in text.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        if fields.len() < 10 {
            continue;
        }
        let name = fields[2];
        if !is_whole_disk(name) {
            continue;
        }
        let sectors_read: u64 = fields[5].parse().unwrap_or(0);
        let sectors_written: u64 = fields[9].parse().unwrap_or(0);
        read = read.saturating_add(sectors_read.saturating_mul(512));
        written = written.saturating_add(sectors_written.saturating_mul(512));
        any = true;
    }
    any.then_some((read, written))
}

fn is_whole_disk(name: &str) -> bool {
    if let Some(rest) = name.strip_prefix("nvme") {
        // nvme0n1 is a disk, nvme0n1p1 a partition.
        return !rest.contains('p');
    }
    if let Some(rest) = name.strip_prefix("mmcblk") {
        return !rest.contains('p');
    }
    if name.starts_with("sd") || name.starts_with("vd") || name.starts_with("hd") || name.starts_with("xvd") {
        return !name.chars().last().is_some_and(|c| c.is_ascii_digit());
    }
    false
}

/// `/proc/<pid>/stat`. The comm field is parenthesised and may itself contain
/// spaces and parens, so the split starts after the *last* `)`.
pub fn parse_pid_stat(text: &str) -> Option<PidStat> {
    let open = text.find('(')?;
    let close = text.rfind(')')?;
    if close < open {
        return None;
    }
    let pid = text[..open].trim().parse().ok()?;
    let comm = text[open + 1..close].to_string();
    // After the comm, field 3 (state) is index 0.
    let fields: Vec<&str> = text[close + 1..].split_whitespace().collect();
    let field = |n: usize| fields.get(n).copied().unwrap_or("0");
    let number = |n: usize| fields.get(n).and_then(|value| value.parse::<u64>().ok());
    let signed = |n: usize| fields.get(n).and_then(|value| value.parse::<i32>().ok());
    let utime: u64 = field(11).parse().unwrap_or(0);
    let stime: u64 = field(12).parse().unwrap_or(0);
    Some(PidStat {
        pid,
        comm,
        state: parse_state(field(0)),
        ppid: field(1).parse().unwrap_or(0),
        cpu_ticks: utime.saturating_add(stime),
        threads: field(17).parse().unwrap_or(0),
        rss_pages: field(21).parse().unwrap_or(0),
        start_ticks: field(19).parse().unwrap_or(0),
        minflt: number(7),
        majflt: number(9),
        priority: signed(15),
        nice: signed(16),
        vsize: number(20),
    })
}

/// A plain count from `/proc/<pid>/status` (`voluntary_ctxt_switches`, …).
pub fn parse_status_count(text: &str, name: &str) -> Option<u64> {
    text.lines()
        .find_map(|line| line.strip_prefix(name).and_then(|rest| rest.strip_prefix(':')))
        .and_then(|tail| tail.split_whitespace().next())
        .and_then(|value| value.parse::<u64>().ok())
}

/// `read_bytes` / `write_bytes` from `/proc/<pid>/io`: bytes this process
/// caused to be fetched from and sent to the storage layer.
pub fn parse_io_bytes(text: &str) -> (Option<u64>, Option<u64>) {
    (parse_status_count(text, "read_bytes"), parse_status_count(text, "write_bytes"))
}

fn parse_state(field: &str) -> ProcState {
    match field.chars().next() {
        Some('R') => ProcState::Running,
        Some('S') => ProcState::Sleeping,
        Some('D') => ProcState::Waiting,
        Some('I') => ProcState::Idle,
        Some('T') | Some('t') => ProcState::Stopped,
        Some('Z') | Some('X') => ProcState::Zombie,
        _ => ProcState::Unknown,
    }
}

/// The real uid from a `/proc/<pid>/status` block.
pub fn parse_status_uid(text: &str) -> Option<u32> {
    text.lines()
        .find_map(|line| line.strip_prefix("Uid:"))
        .and_then(|tail| tail.split_whitespace().next())
        .and_then(|value| value.parse().ok())
}

/// A `kB` figure from `/proc/<pid>/status` (`VmRSS`, `VmHWM`, `VmSize`, …).
pub fn parse_status_kb(text: &str, name: &str) -> Option<u64> {
    text.lines()
        .find_map(|line| line.strip_prefix(name).and_then(|rest| rest.strip_prefix(':')))
        .and_then(|tail| tail.split_whitespace().next())
        .and_then(|value| value.parse::<u64>().ok())
        .map(|kb| kb.saturating_mul(1024))
}

/// `/proc/<pid>/cmdline` is NUL separated (and empty for kernel threads).
pub fn parse_cmdline(bytes: &[u8]) -> String {
    let text: Vec<String> = bytes
        .split(|byte| *byte == 0)
        .filter(|part| !part.is_empty())
        .map(|part| String::from_utf8_lossy(part).into_owned())
        .collect();
    text.join(" ")
}

/// `/proc/loadavg` → the three load figures.
pub fn parse_loadavg(text: &str) -> [f64; 3] {
    let mut values = text.split_whitespace().map(|v| v.parse::<f64>().unwrap_or(0.0));
    [
        values.next().unwrap_or(0.0),
        values.next().unwrap_or(0.0),
        values.next().unwrap_or(0.0),
    ]
}

/// Read `/etc/passwd` once into uid → name (no NSS, no libc dependency).
pub fn parse_passwd(text: &str) -> HashMap<u32, String> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split(':');
            let name = fields.next()?;
            let _password = fields.next()?;
            let uid: u32 = fields.next()?.parse().ok()?;
            Some((uid, name.to_string()))
        })
        .collect()
}

/// The precise start stamp: boot time and the start tick folded into
/// microseconds since the epoch, exact for any `clk_tck` dividing 1e6.
pub fn start_identity(btime_secs: u64, start_ticks: u64, clk_tck: u64) -> u64 {
    let tick_micros = 1_000_000u128 * start_ticks as u128 / clk_tck.max(1) as u128;
    (btime_secs as u128 * 1_000_000 + tick_micros).min(u64::MAX as u128) as u64
}

/// One line of `/proc/net/tcp`, `tcp6`, `udp` or `udp6`: (inode, local, remote, state).
pub fn parse_net_socket_line(line: &str, ipv6: bool) -> Option<(u64, String, String, u32)> {
    let fields: Vec<&str> = line.split_whitespace().collect();
    if fields.len() < 10 || !fields[0].ends_with(':') {
        return None;
    }
    let inode: u64 = fields[9].parse().ok()?;
    let state = u32::from_str_radix(fields[3], 16).ok()?;
    Some((inode, hex_socket_address(fields[1], ipv6)?, hex_socket_address(fields[2], ipv6)?, state))
}

/// `0100007F:1F90` → `127.0.0.1:8080`; IPv6 addresses are four little-endian
/// words of hex.
fn hex_socket_address(text: &str, ipv6: bool) -> Option<String> {
    let (address, port) = text.split_once(':')?;
    let port = u16::from_str_radix(port, 16).ok()?;
    if ipv6 {
        if address.len() != 32 {
            return None;
        }
        let mut octets = [0u8; 16];
        for (word, chunk) in address.as_bytes().chunks(8).enumerate() {
            let value = u32::from_str_radix(std::str::from_utf8(chunk).ok()?, 16).ok()?;
            octets[word * 4..word * 4 + 4].copy_from_slice(&value.to_le_bytes());
        }
        Some(format!("[{}]:{port}", std::net::Ipv6Addr::from(octets)))
    } else {
        let value = u32::from_str_radix(address, 16).ok()?;
        Some(format!("{}:{port}", std::net::Ipv4Addr::from(value.to_le_bytes())))
    }
}

pub fn tcp_state_name(state: u32) -> &'static str {
    match state {
        1 => "ESTABLISHED",
        2 => "SYN_SENT",
        3 => "SYN_RECV",
        4 => "FIN_WAIT1",
        5 => "FIN_WAIT2",
        6 => "TIME_WAIT",
        7 => "CLOSE",
        8 => "CLOSE_WAIT",
        9 => "LAST_ACK",
        10 => "LISTEN",
        11 => "CLOSING",
        _ => "?",
    }
}

/// `/proc/<pid>/maps` → distinct mapped files with the bytes mapped.
pub fn parse_maps(text: &str) -> Vec<LibraryInfo> {
    let mut mapped: HashMap<String, u64> = HashMap::new();
    for line in text.lines() {
        let mut fields = line.splitn(6, ' ');
        let range = fields.next().unwrap_or("");
        let path = fields.nth(4).map(str::trim).unwrap_or("");
        if !path.starts_with('/') {
            continue;
        }
        let Some((start, end)) = range.split_once('-') else { continue };
        let (Ok(start), Ok(end)) = (u64::from_str_radix(start, 16), u64::from_str_radix(end, 16)) else { continue };
        *mapped.entry(path.to_string()).or_insert(0) += end.saturating_sub(start);
    }
    let mut libraries: Vec<LibraryInfo> = mapped.into_iter().map(|(path, mapped)| LibraryInfo { path, mapped }).collect();
    libraries.sort_by(|a, b| a.path.cmp(&b.path));
    libraries
}

#[cfg(target_os = "linux")]
mod sys {
    extern "C" {
        fn sysconf(name: i32) -> i64;
        fn getuid() -> u32;
    }
    /// `_SC_CLK_TCK` and `_SC_PAGESIZE` on glibc/musl.
    const SC_CLK_TCK: i32 = 2;
    const SC_PAGESIZE: i32 = 30;

    pub fn page_size() -> u64 {
        // SAFETY: a constant query with no memory effects.
        let value = unsafe { sysconf(SC_PAGESIZE) };
        if value > 0 { value as u64 } else { 4096 }
    }

    pub fn clk_tck() -> u64 {
        // SAFETY: a constant query with no memory effects.
        let value = unsafe { sysconf(SC_CLK_TCK) };
        if value > 0 { value as u64 } else { super::USER_HZ }
    }

    pub fn my_uid() -> u32 {
        // SAFETY: no arguments; always succeeds.
        unsafe { getuid() }
    }
}

#[cfg(not(target_os = "linux"))]
mod sys {
    pub fn clk_tck() -> u64 {
        super::USER_HZ
    }
    pub fn page_size() -> u64 {
        4096
    }
    pub fn my_uid() -> u32 {
        0
    }
}

struct Identity {
    meta: Arc<ProcMeta>,
    /// The kernel comm the metadata was built from: a change means exec.
    comm: String,
    previous_cpu_ns: Option<u64>,
}

/// Threads / descriptors read per detail collection.
const MAX_DETAIL_ITEMS: usize = 4096;
const MAX_PATH_LEN: usize = 1024;

#[allow(dead_code)]
pub struct LinuxBackend {
    previous_cores: Vec<[u64; CPU_STATES]>,
    previous_net: Option<(u64, u64)>,
    previous_disk: Option<(u64, u64)>,
    last_sample: Option<f64>,
    user_names: HashMap<u32, String>,
    /// pid → identity, replaced when the start tick changes (a reused pid).
    identities: HashMap<u32, Identity>,
    page_size: u64,
    clk_tck: u64,
    btime: u64,
    my_uid: u32,
}

#[allow(dead_code)]
impl LinuxBackend {
    pub fn new() -> Self {
        Self {
            previous_cores: Vec::new(),
            previous_net: None,
            previous_disk: None,
            last_sample: None,
            user_names: std::fs::read_to_string("/etc/passwd").map(|t| parse_passwd(&t)).unwrap_or_default(),
            identities: HashMap::new(),
            page_size: sys::page_size(),
            clk_tck: sys::clk_tck(),
            btime: std::fs::read_to_string("/proc/stat").ok().and_then(|t| parse_stat_btime(&t)).unwrap_or(0),
            my_uid: sys::my_uid(),
        }
    }

    fn ticks_to_ns(&self, ticks: u64) -> u64 {
        (ticks as u128 * 1_000_000_000 / self.clk_tck.max(1) as u128).min(u64::MAX as u128) as u64
    }

    fn sample_cores(&mut self) -> Vec<f64> {
        let text = std::fs::read_to_string("/proc/stat").unwrap_or_default();
        let current = parse_stat_cpus(&text);
        let first = self.previous_cores.is_empty();
        let values = current
            .iter()
            .enumerate()
            .map(|(index, ticks)| {
                if first {
                    return 0.0;
                }
                cpu_pct_from_ticks(*ticks, self.previous_cores.get(index).copied().unwrap_or([0; CPU_STATES]))
            })
            .collect();
        self.previous_cores = current;
        values
    }

    /// Same incarnation only when the start tick matches exactly. A changed
    /// comm (exec) or parent (reparenting) interns new metadata under the
    /// same key, so older samples keep what was true then.
    fn identity(&mut self, pid: u32, stat: &PidStat, path: &std::path::Path) -> (Arc<ProcMeta>, Option<u64>) {
        let start = start_identity(self.btime, stat.start_ticks, self.clk_tck);
        let mut previous_cpu_ns = None;
        if let Some(identity) = self.identities.get(&pid) {
            if identity.meta.key.start == start {
                if identity.comm == stat.comm && identity.meta.ppid == stat.ppid {
                    return (identity.meta.clone(), identity.previous_cpu_ns);
                }
                previous_cpu_ns = identity.previous_cpu_ns;
            }
        }
        let uid = std::fs::read_to_string(path.join("status"))
            .ok()
            .and_then(|text| parse_status_uid(&text))
            .unwrap_or(0);
        let cmdline = std::fs::read(path.join("cmdline")).map(|bytes| parse_cmdline(&bytes)).unwrap_or_default();
        let name = cmdline
            .split_whitespace()
            .next()
            .and_then(|first| first.rsplit('/').next())
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            // Kernel threads have an empty cmdline; comm is all there is.
            .unwrap_or_else(|| stat.comm.clone());
        let is_app = uid == self.my_uid && !cmdline.is_empty();
        let meta = Arc::new(ProcMeta {
            key: ProcKey { pid, start },
            ppid: stat.ppid,
            user: self.user_names.get(&uid).cloned().unwrap_or_else(|| uid.to_string()),
            name: bounded(name, MAX_NAME_LEN),
            cmdline: bounded(cmdline, MAX_CMDLINE_LEN),
            started_secs: start / 1_000_000,
            is_app,
        });
        self.identities.insert(pid, Identity { meta: meta.clone(), comm: stat.comm.clone(), previous_cpu_ns });
        (meta, previous_cpu_ns)
    }

    fn sample_processes(&mut self, seconds: f64) -> Vec<ProcInfo> {
        let Ok(entries) = std::fs::read_dir("/proc") else { return Vec::new() };
        let mut processes = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let elapsed_ns = (seconds * 1e9) as u64;
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(pid) = name.to_str().and_then(|n| n.parse::<u32>().ok()) else { continue };
            let path = entry.path();
            let Ok(stat) = std::fs::read_to_string(path.join("stat")) else { continue };
            let Some(stat) = parse_pid_stat(&stat) else { continue };
            seen.insert(pid);
            let (meta, previous_cpu_ns) = self.identity(pid, &stat, &path);
            let cpu_ns = self.ticks_to_ns(stat.cpu_ticks);
            let cpu_pct = match previous_cpu_ns {
                Some(before) => cpu_pct_from_time(cpu_ns, before, elapsed_ns),
                None => 0.0,
            };
            if let Some(identity) = self.identities.get_mut(&pid) {
                identity.previous_cpu_ns = Some(cpu_ns);
            }
            // Another user's io file refuses the read: no figure, not zero.
            let (disk_read, disk_written) = std::fs::read_to_string(path.join("io")).map(|t| parse_io_bytes(&t)).unwrap_or((None, None));
            processes.push(ProcInfo {
                meta,
                cpu_pct: cpu_pct.max(0.0),
                mem_rss: stat.rss_pages.saturating_mul(self.page_size),
                cpu_time_ns: Some(cpu_ns),
                state: stat.state,
                threads: stat.threads,
                extra: ProcExtra {
                    virtual_bytes: stat.vsize,
                    faults: stat.minflt.zip(stat.majflt).map(|(minor, major)| minor.saturating_add(major)),
                    pageins: stat.majflt,
                    priority: stat.priority,
                    nice: stat.nice,
                    disk_read,
                    disk_written,
                    ..ProcExtra::default()
                },
            });
        }
        self.identities.retain(|pid, _| seen.contains(pid));
        processes
    }

    fn sample_disk(&mut self, seconds: f64) -> Reading<DiskInfo> {
        let Some((read_total, write_total)) = std::fs::read_to_string("/proc/diskstats").ok().and_then(|t| parse_diskstats(&t)) else {
            self.previous_disk = None;
            return Reading::Unavailable("no whole-disk rows in /proc/diskstats");
        };
        let (read_per_second, write_per_second) = match self.previous_disk {
            Some((read, write)) if seconds > 0.0 => (
                read_total.saturating_sub(read) as f64 / seconds,
                write_total.saturating_sub(write) as f64 / seconds,
            ),
            _ => (0.0, 0.0),
        };
        self.previous_disk = Some((read_total, write_total));
        Reading::Value(DiskInfo { read_total, write_total, read_per_second, write_per_second })
    }

    fn read_stat(&self, pid: u32) -> Option<PidStat> {
        std::fs::read_to_string(format!("/proc/{pid}/stat")).ok().and_then(|t| parse_pid_stat(&t))
    }

    /// Why a `/proc/<pid>/...` read failed, in the inspector's words.
    fn refusal<T>(error: std::io::Error) -> Detail<T> {
        match error.kind() {
            std::io::ErrorKind::PermissionDenied => Detail::denied(),
            std::io::ErrorKind::NotFound => Detail::Unavailable("the process is gone".to_string()),
            _ => Detail::Unavailable(format!("the OS refused: {error}")),
        }
    }

    /// The thread list, and whether it is whole: not cut at the cap, and
    /// every listed thread's stat read.
    fn detail_threads(&self, pid: u32) -> (Detail<Vec<ThreadInfo>>, bool) {
        let entries = match std::fs::read_dir(format!("/proc/{pid}/task")) {
            Ok(entries) => entries,
            Err(error) => return (Self::refusal(error), false),
        };
        let mut threads = Vec::new();
        let mut complete = true;
        for entry in entries.flatten() {
            if threads.len() >= MAX_DETAIL_ITEMS {
                complete = false;
                break;
            }
            let Some(tid) = entry.file_name().to_str().and_then(|n| n.parse::<u64>().ok()) else { continue };
            let Some(stat) = std::fs::read_to_string(entry.path().join("stat")).ok().and_then(|t| parse_pid_stat(&t)) else {
                complete = false;
                continue;
            };
            let name = std::fs::read_to_string(entry.path().join("comm")).map(|t| t.trim().to_string()).unwrap_or(stat.comm);
            let state = match stat.state {
                ProcState::Running => ThreadState::Running,
                ProcState::Sleeping | ProcState::Idle => ThreadState::Waiting,
                ProcState::Waiting => ThreadState::Uninterruptible,
                ProcState::Stopped => ThreadState::Stopped,
                ProcState::Zombie => ThreadState::Halted,
                ProcState::Unknown => ThreadState::Unknown,
            };
            threads.push(ThreadInfo {
                id: tid,
                name,
                state,
                // /proc keeps no per-thread recent-usage estimate; only the
                // cumulative time is real here, so the percentage is unknown.
                cpu_pct: None,
                cpu_time_ns: Some(self.ticks_to_ns(stat.cpu_ticks)),
            });
        }
        (Detail::Ready(threads), complete)
    }

    /// Descriptors and sockets, and whether the descriptor list is whole.
    fn detail_files(&self, pid: u32) -> (Detail<Vec<FileInfo>>, Detail<Vec<PortInfo>>, bool) {
        let entries = match std::fs::read_dir(format!("/proc/{pid}/fd")) {
            Ok(entries) => entries,
            Err(error) => {
                let files: Detail<Vec<FileInfo>> = Self::refusal(std::io::Error::new(error.kind(), error.to_string()));
                return (files, Self::refusal(error), false);
            }
        };
        let mut files = Vec::new();
        let mut socket_inodes: Vec<(i32, u64)> = Vec::new();
        let mut complete = true;
        for entry in entries.flatten() {
            if files.len() >= MAX_DETAIL_ITEMS {
                complete = false;
                break;
            }
            let Some(fd) = entry.file_name().to_str().and_then(|n| n.parse::<i32>().ok()) else { continue };
            // An fd closed between the listing and the readlink reads as an
            // empty target: its path is unknown, not "".
            let target = bounded(std::fs::read_link(entry.path()).map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(), MAX_PATH_LEN);
            if target.is_empty() {
                files.push(FileInfo { fd, kind: "file", path: "(path not readable)".to_string() });
                continue;
            }
            if let Some(inode) = target.strip_prefix("socket:[").and_then(|rest| rest.strip_suffix(']')).and_then(|n| n.parse().ok()) {
                socket_inodes.push((fd, inode));
                files.push(FileInfo { fd, kind: "socket", path: target });
            } else if target.starts_with("pipe:") {
                files.push(FileInfo { fd, kind: "pipe", path: target });
            } else if target.starts_with("anon_inode:") {
                files.push(FileInfo { fd, kind: "anon", path: target });
            } else {
                files.push(FileInfo { fd, kind: "file", path: target });
            }
        }
        let mut ports = Vec::new();
        // The process' own network namespace, not the host's: a container's
        // sockets are only listed under /proc/<pid>/net.
        let mut readable = false;
        for (file, protocol, ipv6) in [("tcp", "tcp", false), ("tcp6", "tcp", true), ("udp", "udp", false), ("udp6", "udp", true)] {
            let Ok(text) = std::fs::read_to_string(format!("/proc/{pid}/net/{file}")) else { continue };
            readable = true;
            for line in text.lines().skip(1) {
                let Some((inode, local, remote, state)) = parse_net_socket_line(line, ipv6) else { continue };
                if socket_inodes.iter().any(|(_, wanted)| *wanted == inode) {
                    ports.push(PortInfo {
                        protocol,
                        local,
                        remote,
                        state: if protocol == "tcp" { tcp_state_name(state).to_string() } else { String::new() },
                    });
                }
            }
        }
        let ports = if readable || socket_inodes.is_empty() {
            Detail::Ready(ports)
        } else {
            Detail::Unavailable(format!("/proc/{pid}/net is not readable; {} sockets not resolved", socket_inodes.len()))
        };
        (Detail::Ready(files), ports, complete)
    }
}

impl SystemBackend for LinuxBackend {
    fn name(&self) -> &'static str {
        "linux/proc"
    }

    fn sample(&mut self) -> Snapshot {
        let now = Cx::monotonic_now();
        let seconds = self.last_sample.map(|then| (now - then).max(0.0)).unwrap_or(0.0);
        self.last_sample = Some(now);

        let cpu_cores = self.sample_cores();
        let cpu_total = if cpu_cores.is_empty() {
            0.0
        } else {
            cpu_cores.iter().sum::<f64>() / cpu_cores.len() as f64
        };

        let (rx_total, tx_total) =
            std::fs::read_to_string("/proc/net/dev").map(|t| parse_net_dev(&t)).unwrap_or((0, 0));
        let net = match self.previous_net {
            Some((rx, tx)) if seconds > 0.0 => NetInfo {
                rx_total,
                tx_total,
                rx_per_second: rx_total.saturating_sub(rx) as f64 / seconds,
                tx_per_second: tx_total.saturating_sub(tx) as f64 / seconds,
            },
            _ => NetInfo { rx_total, tx_total, ..NetInfo::default() },
        };
        self.previous_net = Some((rx_total, tx_total));

        Snapshot {
            cpu_total,
            cpu_cores,
            mem: std::fs::read_to_string("/proc/meminfo").map(|t| parse_meminfo(&t)).unwrap_or_default(),
            net,
            disk: self.sample_disk(seconds),
            gpu_pct: Reading::Unavailable("not measured on linux"),
            power_watts: Reading::Unavailable("not measured on linux"),
            processes: self.sample_processes(seconds),
            load_avg: std::fs::read_to_string("/proc/loadavg").map(|t| parse_loadavg(&t)).unwrap_or_default(),
            uptime_seconds: std::fs::read_to_string("/proc/uptime")
                .ok()
                .and_then(|t| t.split_whitespace().next()?.parse::<f64>().ok())
                .unwrap_or(0.0) as u64,
            backend: "linux/proc",
        }
    }

    fn detail(&mut self, key: ProcKey, want: Want) -> ProcDetail {
        let pid = key.pid;
        let time_ms = now_ms();
        let Some(stat) = self.read_stat(pid) else { return ProcDetail::gone(key, time_ms) };
        if start_identity(self.btime, stat.start_ticks, self.clk_tck) != key.start {
            return ProcDetail::gone(key, time_ms);
        }
        let status_text = std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap_or_default();
        let (threads, threads_complete) = if want.threads { self.detail_threads(pid) } else { (not_collected(), false) };
        let running_threads = match &threads {
            Detail::Ready(list) => list.iter().filter(|t| t.state == ThreadState::Running).count() as u32,
            Detail::Unavailable(_) => 0,
        };
        let voluntary = parse_status_count(&status_text, "voluntary_ctxt_switches");
        let involuntary = parse_status_count(&status_text, "nonvoluntary_ctxt_switches");
        let context_switches = voluntary.zip(involuntary).map(|(a, b)| a.saturating_add(b));
        // Owner-only (ptrace read access): refused for other users' processes.
        let (disk_read, disk_written) = std::fs::read_to_string(format!("/proc/{pid}/io")).map(|t| parse_io_bytes(&t)).unwrap_or((None, None));
        let identity = IdentityDetail {
            path: std::fs::read_link(format!("/proc/{pid}/exe")).map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(),
            status: stat.state.describe().to_string(),
            cpu_time_ns: Some(self.ticks_to_ns(stat.cpu_ticks)),
            threads: stat.threads,
            running_threads,
        };
        let libraries = want.libraries.then(|| match std::fs::read_to_string(format!("/proc/{pid}/maps")) {
            Ok(text) => Detail::Ready(parse_maps(&text)),
            Err(error) => Self::refusal(error),
        });
        let mut measures = Vec::new();
        for (field, measure) in [("VmHWM", Measure::PeakResident), ("RssAnon", Measure::AnonResident), ("RssFile", Measure::FileResident), ("VmSwap", Measure::Swapped)] {
            if let Some(bytes) = parse_status_kb(&status_text, field) {
                measures.push((measure, bytes as i64));
            }
        }
        if let Some(switches) = context_switches {
            measures.push((Measure::ContextSwitches, switches as i64));
        }
        if let Some(read) = disk_read {
            measures.push((Measure::DiskRead, read as i64));
        }
        if let Some(written) = disk_written {
            measures.push((Measure::DiskWritten, written as i64));
        }
        let memory = Detail::Ready(MemoryDetail { regions: None });
        let (files, ports, files_complete) = if want.files { self.detail_files(pid) } else { (not_collected(), not_collected(), false) };
        if let (Detail::Ready(list), true) = (&files, files_complete) {
            measures.push((Measure::OpenFds, list.len() as i64));
        }
        ProcDetail { key, time_ms, identity: Detail::Ready(identity), memory, threads, files, ports, libraries, measures, threads_complete, files_complete }
    }

    fn is_alive(&mut self, key: ProcKey) -> Option<bool> {
        match std::fs::read_to_string(format!("/proc/{}/stat", key.pid)) {
            Ok(text) => parse_pid_stat(&text).map(|stat| start_identity(self.btime, stat.start_ticks, self.clk_tck) == key.start),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Some(false),
            Err(_) => None,
        }
    }

    fn signal_verified(&mut self, key: ProcKey, force: bool) -> Result<(), String> {
        let Some(stat) = self.read_stat(key.pid) else {
            return Err(format!("PID {} is gone; not signalled", key.pid));
        };
        if start_identity(self.btime, stat.start_ticks, self.clk_tck) != key.start {
            return Err(format!("PID {} now belongs to another process; not signalled", key.pid));
        }
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        {
            super::unix_signal::terminate(key.pid, force)
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        {
            let _ = force;
            Err("signals are not available on this platform".to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stat_cpu_lines_skip_the_aggregate_row() {
        let text = "cpu  100 0 100 800 0 0 0 0 0 0\n\
                    cpu0 10 1 10 80 1 1 1 0 0 0\n\
                    cpu1 20 2 20 60 0 0 0 0 0 0\n\
                    intr 12345\n";
        let cores = parse_stat_cpus(text);
        assert_eq!(cores.len(), 2);
        // cpu0: user 10, system 10+irq1+soft1 = 12, idle 80+iowait1 = 81, nice 1
        assert_eq!(cores[0], [10, 12, 81, 1]);
        assert_eq!(cores[1], [20, 20, 60, 2]);
    }

    #[test]
    fn stat_cpu_percent_comes_from_the_delta() {
        let before = parse_stat_cpus("cpu0 100 0 100 800 0 0 0\n");
        let after = parse_stat_cpus("cpu0 150 0 150 900 0 0 0\n");
        let percent = cpu_pct_from_ticks(after[0], before[0]);
        assert!((percent - 50.0).abs() < 1e-9, "{percent}");
    }

    #[test]
    fn meminfo_used_is_total_minus_available() {
        let memory = parse_meminfo(
            "MemTotal:  1000 kB\nMemFree: 100 kB\nMemAvailable: 400 kB\n\
             Cached: 200 kB\nSReclaimable: 50 kB\nSwapTotal: 500 kB\nSwapFree: 200 kB\n",
        );
        assert_eq!(memory.total, 1000 * 1024);
        assert_eq!(memory.used, 600 * 1024);
        assert_eq!(memory.cache, 250 * 1024);
        assert_eq!(memory.free, 100 * 1024);
        assert_eq!(memory.swap_used, 300 * 1024);
    }

    #[test]
    fn net_dev_sums_every_interface_but_loopback() {
        let text = "Inter-|   Receive                    |  Transmit\n\
                    face |bytes packets errs drop fifo frame compressed multicast|bytes packets\n\
                    \x20   lo:  111 1 0 0 0 0 0 0  222 2 0 0 0 0 0 0\n\
                    \x20 eth0:  1000 1 0 0 0 0 0 0  2000 2 0 0 0 0 0 0\n\
                    \x20 wlan0: 3000 1 0 0 0 0 0 0  4000 2 0 0 0 0 0 0\n";
        assert_eq!(parse_net_dev(text), (4000, 6000));
    }

    #[test]
    fn pid_stat_survives_a_comm_with_spaces_and_parens() {
        // 52 fields; comm is "(weird name)" which contains both hazards.
        let mut text = String::from("4242 ((weird name)) S 1 ");
        // fields 5..13 (pgrp..cmajflt)
        text.push_str("0 0 0 0 0 0 0 0 0 ");
        // 14 utime, 15 stime
        text.push_str("700 300 ");
        // 16..19
        text.push_str("0 0 0 0 ");
        // 20 num_threads
        text.push_str("12 ");
        // 21..23
        text.push_str("0 0 0 ");
        // 24 rss (pages)
        text.push_str("4096 ");
        let stat = parse_pid_stat(&text).expect("parses");
        assert_eq!(stat.pid, 4242);
        assert_eq!(stat.comm, "(weird name)");
        assert_eq!(stat.state, ProcState::Sleeping);
        assert_eq!(stat.ppid, 1);
        assert_eq!(stat.cpu_ticks, 1000);
        assert_eq!(stat.threads, 12);
        assert_eq!(stat.rss_pages, 4096);
    }

    #[test]
    fn pid_stat_maps_every_state_letter() {
        let make = |state: &str| format!("1 (x) {state} 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0 0");
        assert_eq!(parse_pid_stat(&make("R")).unwrap().state, ProcState::Running);
        assert_eq!(parse_pid_stat(&make("D")).unwrap().state, ProcState::Waiting);
        assert_eq!(parse_pid_stat(&make("Z")).unwrap().state, ProcState::Zombie);
        assert_eq!(parse_pid_stat(&make("t")).unwrap().state, ProcState::Stopped);
        assert_eq!(parse_pid_stat(&make("I")).unwrap().state, ProcState::Idle);
    }

    #[test]
    fn status_uid_takes_the_real_uid() {
        let text = "Name:\tbash\nState:\tS (sleeping)\nTgid:\t900\nUid:\t1000\t1000\t1000\t1000\n";
        assert_eq!(parse_status_uid(text), Some(1000));
        assert_eq!(parse_status_uid("Name:\tbash\n"), None);
    }

    #[test]
    fn cmdline_joins_nul_separated_argv() {
        assert_eq!(parse_cmdline(b"/bin/sh\0-c\0echo hi\0"), "/bin/sh -c echo hi");
        // Kernel threads report an empty cmdline.
        assert_eq!(parse_cmdline(b""), "");
    }

    #[test]
    fn loadavg_reads_the_first_three_numbers() {
        assert_eq!(parse_loadavg("1.50 0.75 0.25 2/512 12345\n"), [1.5, 0.75, 0.25]);
    }

    #[test]
    fn passwd_maps_uid_to_name() {
        let map = parse_passwd("root:x:0:0:root:/root:/bin/bash\nrik:x:1000:1000::/home/rik:/bin/zsh\n#bad\n");
        assert_eq!(map.get(&0).map(String::as_str), Some("root"));
        assert_eq!(map.get(&1000).map(String::as_str), Some("rik"));
    }
}
