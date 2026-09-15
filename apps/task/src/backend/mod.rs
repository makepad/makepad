//! The per-OS system backends behind one trait.
//!
//! task is a *true* multi-platform process manager: everything the UI
//! renders arrives as a [`Snapshot`] produced by a [`SystemBackend`], and each
//! OS implements that trait with its own native mechanism — never by shelling
//! out to `ps`/`top` and scraping text.
//!
//! * [`macos`]   — sysctl `KERN_PROC_ALL` (pid/ppid/uid/state/name), libproc
//!                 `proc_pidinfo(PROC_PIDTASKINFO)` (cpu time / rss / threads),
//!                 mach `host_processor_info` (per-core ticks),
//!                 `host_statistics64(HOST_VM_INFO64)` + `hw.memsize` (memory),
//!                 sysctl `NET_RT_IFLIST2` (`if_msghdr2`) for interface bytes.
//! * [`linux`]   — `/proc` (`stat`, `meminfo`, `net/dev`, `loadavg`, `uptime`,
//!                 `<pid>/stat|status|cmdline`, `/etc/passwd` for uid → name).
//! * [`windows`] — Win32: `CreateToolhelp32Snapshot` + `Process32*W` for the
//!                 list and the tree, `GetProcessTimes`/`K32GetProcessMemoryInfo`
//!                 per process, `NtQuerySystemInformation` per core,
//!                 `GlobalMemoryStatusEx` + `K32GetPerformanceInfo` for memory,
//!                 `GetIfTable2Ex` for network, `TerminateProcess` for the kill.
//!
//! The `linux` and `windows` modules compile on *every* target so their pure
//! parsers and helpers stay unit-testable from any machine; only
//! [`new_backend`] picks by `cfg(target_os)`, and only the Windows module's
//! FFI half is behind `cfg(windows)`.
//!
//! Verified live on macOS. Linux and Windows are compile-checked for
//! `x86_64-unknown-linux-gnu` and `x86_64-pc-windows-msvc` with their pure
//! parts covered by tests; neither has run on its own hardware yet.

use std::collections::HashMap;

pub mod linux;
#[cfg(target_os = "macos")]
pub mod macos;
pub mod windows;

/// Scheduler state of a process, normalised across operating systems.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ProcState {
    Running,
    Sleeping,
    /// Uninterruptible / disk wait.
    Waiting,
    Idle,
    Stopped,
    Zombie,
    #[default]
    Unknown,
}

impl ProcState {
    /// The single letter the process table shows (top/btop convention).
    pub fn as_str(self) -> &'static str {
        match self {
            ProcState::Running => "R",
            ProcState::Sleeping => "S",
            ProcState::Waiting => "D",
            ProcState::Idle => "I",
            ProcState::Stopped => "T",
            ProcState::Zombie => "Z",
            ProcState::Unknown => "?",
        }
    }
}

/// One process as every backend reports it.
#[derive(Clone, Debug, Default)]
pub struct ProcInfo {
    pub pid: u32,
    pub ppid: u32,
    pub user: String,
    /// Short program name (basename of the executable, or the kernel's comm).
    pub name: String,
    /// Full command line where the OS lets us read it, else the exe path.
    pub cmdline: String,
    /// Percent of one core, so a busy 8-thread process reads ~800.
    pub cpu_pct: f64,
    pub mem_rss: u64,
    pub state: ProcState,
    pub threads: u32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct MemInfo {
    pub total: u64,
    pub used: u64,
    pub available: u64,
    pub cache: u64,
    pub free: u64,
    pub swap_total: u64,
    pub swap_used: u64,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct NetInfo {
    pub rx_total: u64,
    pub tx_total: u64,
    pub rx_per_second: f64,
    pub tx_per_second: f64,
}

/// Everything one sampler tick collected.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    /// Busy percent averaged over all cores (0..100).
    pub cpu_total: f64,
    /// Busy percent per core (0..100 each).
    pub cpu_cores: Vec<f64>,
    pub mem: MemInfo,
    pub net: NetInfo,
    pub processes: Vec<ProcInfo>,
    pub load_avg: [f64; 3],
    pub uptime_seconds: u64,
    /// Which backend produced this (shown in the window title bar).
    pub backend: &'static str,
}

/// One operating system's view of the machine.
pub trait SystemBackend: Send {
    /// Collect a full snapshot. Called about once a second, off the UI thread.
    fn sample(&mut self) -> Snapshot;
    /// Short backend name for the status line.
    fn name(&self) -> &'static str;
}

/// The backend for the OS we were compiled for.
pub fn new_backend() -> Box<dyn SystemBackend> {
    #[cfg(target_os = "macos")]
    {
        Box::new(macos::MacosBackend::new())
    }
    #[cfg(target_os = "linux")]
    {
        Box::new(linux::LinuxBackend::new())
    }
    #[cfg(target_os = "windows")]
    {
        Box::new(windows::WindowsBackend::new())
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        Box::new(Unsupported)
    }
}

/// Ask a process to exit.
///
/// `force` escalates: SIGTERM → SIGKILL on unix. Windows has no polite
/// equivalent — `TerminateProcess` is always immediate — so `force` changes
/// nothing there and the UI says as much.
pub fn terminate(pid: u32, force: bool) -> Result<(), String> {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        unix_signal::terminate(pid, force)
    }
    #[cfg(target_os = "windows")]
    {
        let _ = force;
        windows::terminate(pid)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = (pid, force);
        Err("no process backend for this platform".to_string())
    }
}

#[allow(dead_code)]
/// The honest answer on a platform task has no backend for: an empty
/// snapshot that says so, rather than invented numbers.
pub struct Unsupported;

impl SystemBackend for Unsupported {
    fn name(&self) -> &'static str {
        "unsupported"
    }

    fn sample(&mut self) -> Snapshot {
        Snapshot { backend: "unsupported", ..Snapshot::default() }
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
mod unix_signal {
    extern "C" {
        fn kill(pid: i32, sig: i32) -> i32;
    }

    const SIGTERM: i32 = 15;
    const SIGKILL: i32 = 9;

    pub fn terminate(pid: u32, force: bool) -> Result<(), String> {
        if pid == 0 {
            return Err("refusing to signal pid 0".to_string());
        }
        let signal = if force { SIGKILL } else { SIGTERM };
        // SAFETY: kill() on a plain pid; the kernel validates the pid and
        // reports EPERM/ESRCH through errno like any other syscall.
        let result = unsafe { kill(pid as i32, signal) };
        if result == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error().to_string())
        }
    }
}

// ---- pure helpers shared by every backend (unit-tested below) ----

/// Number of CPU tick buckets we normalise to: user, system, idle, nice.
pub const CPU_STATES: usize = 4;

/// Busy percent between two `[user, system, idle, nice]` tick readings.
///
/// A counter that went backwards means the source rolled over (mach's tick
/// counters are 32-bit) or reset across a suspend. One skipped sample reads
/// better than a garbage spike, so that case reports 0.
pub fn cpu_pct_from_ticks(current: [u64; CPU_STATES], previous: [u64; CPU_STATES]) -> f64 {
    let mut deltas = [0u64; CPU_STATES];
    for state in 0..CPU_STATES {
        match current[state].checked_sub(previous[state]) {
            Some(delta) => deltas[state] = delta,
            None => return 0.0,
        }
    }
    let total: u64 = deltas.iter().sum();
    if total == 0 {
        return 0.0;
    }
    ((total - deltas[2]) as f64 / total as f64 * 100.0).clamp(0.0, 100.0)
}

/// One row of a depth-first process tree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TreeRow {
    /// Index into the `(pid, ppid)` slice handed to [`tree_order`].
    pub index: usize,
    /// 0 for a root, +1 per generation.
    pub depth: usize,
    /// Direct children, so the UI knows whether to draw a fold marker.
    pub children: usize,
}

/// Depth-first tree order for `(pid, ppid)` pairs.
///
/// Sibling order and root order follow the *input* order, so the caller's
/// chosen sort (cpu desc, name, …) still decides what comes first inside each
/// generation. Entries whose parent is not in the slice become roots; entries
/// caught in a parent cycle are emitted as roots after the reachable ones, so
/// the output always contains every input exactly once.
pub fn tree_order(items: &[(u32, u32)]) -> Vec<TreeRow> {
    let mut index_of_pid: HashMap<u32, usize> = HashMap::with_capacity(items.len());
    for (index, (pid, _)) in items.iter().enumerate() {
        index_of_pid.entry(*pid).or_insert(index);
    }
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); items.len()];
    let mut roots: Vec<usize> = Vec::new();
    for (index, (pid, ppid)) in items.iter().enumerate() {
        match index_of_pid.get(ppid) {
            Some(&parent) if parent != index && ppid != pid => children[parent].push(index),
            _ => roots.push(index),
        }
    }

    let mut out = Vec::with_capacity(items.len());
    let mut seen = vec![false; items.len()];
    let mut stack: Vec<(usize, usize)> = Vec::new();
    let mut start_from = 0usize;
    loop {
        stack.extend(roots.drain(..).rev().map(|index| (index, 0usize)));
        while let Some((index, depth)) = stack.pop() {
            if seen[index] {
                continue;
            }
            seen[index] = true;
            out.push(TreeRow { index, depth, children: children[index].len() });
            for &child in children[index].iter().rev() {
                stack.push((child, depth + 1));
            }
        }
        // Anything left over sat in a parent cycle: restart from it as a root.
        match (start_from..items.len()).find(|&index| !seen[index]) {
            Some(index) => {
                start_from = index + 1;
                roots.push(index);
            }
            None => break,
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_delta_excludes_idle_ticks() {
        // 20 user + 20 system busy, 40 idle -> 50%.
        let percent = cpu_pct_from_ticks([120, 40, 140, 0], [100, 20, 100, 0]);
        assert!((percent - 50.0).abs() < 1e-9, "{percent}");
    }

    #[test]
    fn cpu_delta_is_zero_without_movement() {
        assert_eq!(cpu_pct_from_ticks([5, 5, 5, 5], [5, 5, 5, 5]), 0.0);
    }

    #[test]
    fn cpu_delta_skips_the_sample_when_a_counter_goes_backwards() {
        // The 32-bit mach counters wrap; report 0 rather than a fake 100%.
        let previous = [u32::MAX as u64 - 10, 0, 0, 0];
        let current = [10u64, 0, 0, 100];
        assert_eq!(cpu_pct_from_ticks(current, previous), 0.0);
    }

    #[test]
    fn cpu_delta_is_all_busy_when_idle_never_moves() {
        assert_eq!(cpu_pct_from_ticks([50, 50, 0, 0], [0, 0, 0, 0]), 100.0);
    }

    #[test]
    fn tree_nests_children_under_parents() {
        // launchd(1) -> {loginwindow(100) -> Finder(200), sshd(300)}
        let items = [(1, 0), (100, 1), (200, 100), (300, 1)];
        let rows = tree_order(&items);
        let shape: Vec<(u32, usize, usize)> = rows
            .iter()
            .map(|row| (items[row.index].0, row.depth, row.children))
            .collect();
        assert_eq!(shape, vec![(1, 0, 2), (100, 1, 1), (200, 2, 0), (300, 1, 0)]);
    }

    #[test]
    fn tree_keeps_sibling_input_order() {
        // Siblings must come out in the order the caller sorted them.
        let items = [(1, 0), (30, 1), (10, 1), (20, 1)];
        let pids: Vec<u32> = tree_order(&items).iter().map(|r| items[r.index].0).collect();
        assert_eq!(pids, vec![1, 30, 10, 20]);
    }

    #[test]
    fn tree_treats_orphans_and_self_parents_as_roots() {
        // 900's parent is gone; 5 claims itself as its own parent.
        let items = [(900, 404), (5, 5)];
        let rows = tree_order(&items);
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|row| row.depth == 0));
    }

    #[test]
    fn tree_emits_every_process_even_in_a_cycle() {
        // A cycle has no root; every entry must still appear exactly once.
        let items = [(1, 0), (7, 8), (8, 7), (9, 7)];
        let rows = tree_order(&items);
        assert_eq!(rows.len(), items.len());
        let mut indices: Vec<usize> = rows.iter().map(|row| row.index).collect();
        indices.sort_unstable();
        assert_eq!(indices, vec![0, 1, 2, 3]);
    }

    #[test]
    fn tree_of_nothing_is_nothing() {
        assert!(tree_order(&[]).is_empty());
    }
}
