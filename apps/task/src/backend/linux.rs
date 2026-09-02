//! Linux backend — everything from `/proc`, no external processes.
//!
//! This module compiles on every target (it only uses `std::fs`) so its
//! parsers stay unit-testable from a mac; [`super::new_backend`] only selects
//! [`LinuxBackend`] under `cfg(target_os = "linux")`.
//!
//! | data | file |
//! |---|---|
//! | per-core cpu ticks | `/proc/stat` |
//! | memory | `/proc/meminfo` |
//! | network bytes | `/proc/net/dev` |
//! | pid/ppid/state/cpu/threads/rss | `/proc/<pid>/stat` |
//! | owning uid | `/proc/<pid>/status` |
//! | command line | `/proc/<pid>/cmdline` |
//! | load / uptime | `/proc/loadavg`, `/proc/uptime` |

// Only the parsers are reachable off Linux (through the tests below); the
// backend itself is constructed by `new_backend` under cfg(target_os).
#![allow(dead_code)]

use super::{cpu_pct_from_ticks, MemInfo, NetInfo, ProcInfo, ProcState, Snapshot, SystemBackend, CPU_STATES};
use std::collections::HashMap;
use std::time::Instant;

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
}

/// USER_HZ. 100 on every mainstream Linux build; the kernel reports
/// `/proc/<pid>/stat` times in these units regardless of CONFIG_HZ.
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
    })
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

#[allow(dead_code)]
pub struct LinuxBackend {
    previous_cores: Vec<[u64; CPU_STATES]>,
    previous_cpu_ticks: HashMap<u32, u64>,
    previous_net: Option<(u64, u64)>,
    last_sample: Option<Instant>,
    user_names: HashMap<u32, String>,
    cmdlines: HashMap<u32, String>,
    page_size: u64,
}

#[allow(dead_code)]
impl LinuxBackend {
    pub fn new() -> Self {
        Self {
            previous_cores: Vec::new(),
            previous_cpu_ticks: HashMap::new(),
            previous_net: None,
            last_sample: None,
            user_names: std::fs::read_to_string("/etc/passwd").map(|t| parse_passwd(&t)).unwrap_or_default(),
            cmdlines: HashMap::new(),
            // 4 KiB everywhere task runs; /proc reports rss in pages.
            page_size: 4096,
        }
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

    fn sample_processes(&mut self, seconds: f64) -> Vec<ProcInfo> {
        let Ok(entries) = std::fs::read_dir("/proc") else { return Vec::new() };
        let mut processes = Vec::new();
        let mut live_ticks = HashMap::new();
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(pid) = name.to_str().and_then(|n| n.parse::<u32>().ok()) else { continue };
            let path = entry.path();
            let Ok(stat) = std::fs::read_to_string(path.join("stat")) else { continue };
            let Some(stat) = parse_pid_stat(&stat) else { continue };
            let uid = std::fs::read_to_string(path.join("status"))
                .ok()
                .and_then(|text| parse_status_uid(&text))
                .unwrap_or(0);
            let cmdline = self.cmdlines.entry(pid).or_insert_with(|| {
                std::fs::read(path.join("cmdline")).map(|bytes| parse_cmdline(&bytes)).unwrap_or_default()
            });
            let cmdline = cmdline.clone();
            // Ticks are USER_HZ; percent-of-one-core = dticks / HZ / seconds.
            let cpu_pct = match self.previous_cpu_ticks.get(&pid) {
                Some(&before) if seconds > 0.0 => {
                    stat.cpu_ticks.saturating_sub(before) as f64 / USER_HZ as f64 / seconds * 100.0
                }
                _ => 0.0,
            };
            live_ticks.insert(pid, stat.cpu_ticks);
            let name = cmdline
                .split_whitespace()
                .next()
                .and_then(|first| first.rsplit('/').next())
                .filter(|name| !name.is_empty())
                .map(str::to_string)
                // Kernel threads have an empty cmdline; comm is all there is.
                .unwrap_or_else(|| stat.comm.clone());
            processes.push(ProcInfo {
                pid,
                ppid: stat.ppid,
                user: self.user_names.get(&uid).cloned().unwrap_or_else(|| uid.to_string()),
                name,
                cmdline,
                cpu_pct: cpu_pct.max(0.0),
                mem_rss: stat.rss_pages.saturating_mul(self.page_size),
                state: stat.state,
                threads: stat.threads,
            });
        }
        self.previous_cpu_ticks = live_ticks;
        if self.cmdlines.len() > processes.len() * 2 + 64 {
            let live: std::collections::HashSet<u32> = processes.iter().map(|p| p.pid).collect();
            self.cmdlines.retain(|pid, _| live.contains(pid));
        }
        processes
    }
}

impl SystemBackend for LinuxBackend {
    fn name(&self) -> &'static str {
        "linux/proc"
    }

    fn sample(&mut self) -> Snapshot {
        let now = Instant::now();
        let seconds = self.last_sample.map(|then| now.duration_since(then).as_secs_f64()).unwrap_or(0.0);
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
            processes: self.sample_processes(seconds),
            load_avg: std::fs::read_to_string("/proc/loadavg").map(|t| parse_loadavg(&t)).unwrap_or_default(),
            uptime_seconds: std::fs::read_to_string("/proc/uptime")
                .ok()
                .and_then(|t| t.split_whitespace().next()?.parse::<f64>().ok())
                .unwrap_or(0.0) as u64,
            backend: "linux/proc",
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
