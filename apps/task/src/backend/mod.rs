//! The per-OS system backends behind one trait.
//!
//! task is a *true* multi-platform process manager: everything the UI
//! renders arrives as a [`Snapshot`] produced by a [`SystemBackend`], and each
//! OS implements that trait with its own native mechanism — never by shelling
//! out to `ps`/`top` and scraping text.
//!
//! * [`macos`]   — sysctl `KERN_PROC_ALL` (pid/ppid/uid/state/name/start),
//!                 libproc `proc_pidinfo` (task/thread/fd/region info),
//!                 `proc_pid_rusage` (footprint, disk bytes),
//!                 mach `host_processor_info` (per-core ticks),
//!                 `host_statistics64(HOST_VM_INFO64)` + `hw.memsize` (memory),
//!                 sysctl `NET_RT_IFLIST2` (`if_msghdr2`) for interface bytes,
//!                 IOKit `IOBlockStorageDriver` statistics (disk bytes) and
//!                 `IOAccelerator` performance statistics (GPU utilisation).
//! * [`linux`]   — `/proc` (`stat`, `meminfo`, `net/dev`, `diskstats`,
//!                 `loadavg`, `uptime`, `<pid>/stat|status|cmdline|task|fd|maps`,
//!                 `net/tcp|udp`, `/etc/passwd` for uid → name).
//! * [`windows`] — Win32: `CreateToolhelp32Snapshot` for processes, threads
//!                 and modules, `GetProcessTimes`/`K32GetProcessMemoryInfo`
//!                 per process, `NtQuerySystemInformation` per core,
//!                 `GlobalMemoryStatusEx` + `K32GetPerformanceInfo` for memory,
//!                 `GetIfTable2Ex` for network, `TerminateProcess` for the kill.
//!
//! The `linux` and `windows` modules compile on *every* target so their pure
//! parsers and helpers stay unit-testable from any machine; only
//! [`new_backend`] picks by `cfg(target_os)`, and only the Windows module's
//! FFI half is behind `cfg(windows)`.
//!
//! A metric an OS cannot measure is reported as [`Reading::Unavailable`] with
//! the reason, never as a number made up from something else.

use std::collections::HashMap;
use std::sync::Arc;

pub mod linux;
#[cfg(target_os = "macos")]
pub mod macos;
#[cfg(target_os = "macos")]
pub mod macos_ntstat;
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

    pub fn describe(self) -> &'static str {
        match self {
            ProcState::Running => "running",
            ProcState::Sleeping => "sleeping",
            ProcState::Waiting => "uninterruptible wait",
            ProcState::Idle => "idle",
            ProcState::Stopped => "stopped",
            ProcState::Zombie => "zombie",
            ProcState::Unknown => "not inspectable",
        }
    }

    pub fn to_u8(self) -> u8 {
        match self {
            ProcState::Running => 1,
            ProcState::Sleeping => 2,
            ProcState::Waiting => 3,
            ProcState::Idle => 4,
            ProcState::Stopped => 5,
            ProcState::Zombie => 6,
            ProcState::Unknown => 0,
        }
    }

    pub fn from_u8(value: u8) -> Self {
        match value {
            1 => ProcState::Running,
            2 => ProcState::Sleeping,
            3 => ProcState::Waiting,
            4 => ProcState::Idle,
            5 => ProcState::Stopped,
            6 => ProcState::Zombie,
            _ => ProcState::Unknown,
        }
    }
}

/// The identity of one process incarnation: a pid *and* the precise moment
/// it started, so a pid the kernel hands out again never continues a dead
/// process' history or receives a signal meant for it.
///
/// `start` is the OS's own precise start stamp, opaque to the UI: microseconds
/// since the epoch on macOS (`pbi_start_tvsec/usec`), boot time plus raw
/// start ticks on Linux, the full creation `FILETIME` on Windows. `0` means
/// the backend could not read a start time — an *unverified* identity, on
/// which no destructive action is taken.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ProcKey {
    pub pid: u32,
    pub start: u64,
}

impl ProcKey {
    pub fn verified(&self) -> bool {
        self.start != 0
    }
}

/// The descriptive side of one process incarnation. Interned by the backend
/// and shared (`Arc`) by every sample that mentions it, so a 0.1 s tick never
/// copies a thousand command lines. Most of it is fixed for the life of the
/// incarnation, but a parent can exit (reparenting) and a process can exec:
/// when any field changes the backend interns a NEW `Arc` for the same key,
/// so older samples keep the metadata that was true when they were taken.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct ProcMeta {
    pub key: ProcKey,
    pub ppid: u32,
    pub user: String,
    /// Short program name (basename of the executable, or the kernel's comm).
    pub name: String,
    /// Full command line where the OS lets us read it, else the exe path.
    pub cmdline: String,
    /// Wall-clock start, seconds since the epoch, 0 when unknown.
    pub started_secs: u64,
    /// A user-facing application: an `.app` bundle executable on macOS; on
    /// Linux and Windows the current user's own user-space processes.
    pub is_app: bool,
}

/// One process as every backend reports it, per tick.
#[derive(Clone, Debug)]
pub struct ProcInfo {
    pub meta: Arc<ProcMeta>,
    /// Percent of one core, so a busy 8-thread process reads ~800.
    pub cpu_pct: f64,
    pub mem_rss: u64,
    /// Cumulative user+system CPU time, when the OS lets us read it.
    pub cpu_time_ns: Option<u64>,
    pub state: ProcState,
    pub threads: u32,
    /// The rest of what the same OS records held, kept rather than dropped.
    pub extra: ProcExtra,
}

/// Figures the basic sample reads for every process anyway, from the same
/// OS record as its CPU and memory (`proc_taskinfo` + `kinfo_proc` on macOS,
/// `/proc/<pid>/stat` on Linux, `PROCESS_MEMORY_COUNTERS` + the Toolhelp
/// entry on Windows): no extra call per process. `None` where this OS's
/// record has no such field. Counters are cumulative for the process' life;
/// the Darwin task counters are 32-bit in the kernel and wrap, which a rate
/// treats like a reset.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ProcExtra {
    pub virtual_bytes: Option<u64>,
    pub faults: Option<u64>,
    pub pageins: Option<u64>,
    pub cow_faults: Option<u64>,
    pub context_switches: Option<u64>,
    pub syscalls: Option<u64>,
    pub priority: Option<i32>,
    pub nice: Option<i32>,
    pub running_threads: Option<u32>,
    pub commit_bytes: Option<u64>,
    pub peak_resident: Option<u64>,
    pub peak_commit: Option<u64>,
    /// Bytes the process read from and wrote to storage (`ri_diskio_*` on
    /// macOS, `/proc/<pid>/io` `read_bytes`/`write_bytes` on Linux,
    /// `IO_COUNTERS` transfer bytes on Windows, which include network
    /// and device I/O).
    pub disk_read: Option<u64>,
    pub disk_written: Option<u64>,
    /// The kernel's memory ledger for the process (macOS `ri_phys_footprint`).
    pub footprint: Option<u64>,
    /// Wake-ups from idle over the process' life (macOS
    /// `ri_pkg_idle_wkups`).
    pub idle_wakeups: Option<u64>,
    /// Network traffic of the process' sockets over its life: macOS
    /// `ntstat` per-socket counters summed per process (sockets that
    /// closed keep counting). `None` where the OS does not attribute
    /// traffic to processes.
    pub net_rx_bytes: Option<u64>,
    pub net_tx_bytes: Option<u64>,
    pub net_rx_packets: Option<u64>,
    pub net_tx_packets: Option<u64>,
}

/// What a detail read should include beyond the cheap per-process figures
/// (identity, memory ledger, counters). The descriptor walk and the
/// address-space walk are the expensive parts; a pinned process' metric tick
/// asks for neither.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Want {
    pub threads: bool,
    /// Descriptors and sockets (the Files and Ports tabs).
    pub files: bool,
    /// The mapped-file walk (the Libraries tab, region summary).
    pub libraries: bool,
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

#[derive(Clone, Copy, Debug, Default)]
pub struct DiskInfo {
    pub read_total: u64,
    pub write_total: u64,
    pub read_per_second: f64,
    pub write_per_second: f64,
}

/// A measurement the OS may or may not provide. `Unavailable` carries the
/// reason the UI prints in the metric's place.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Reading<T> {
    Value(T),
    Unavailable(&'static str),
}

impl<T: Copy> Reading<T> {
    pub fn value(&self) -> Option<T> {
        match self {
            Reading::Value(value) => Some(*value),
            Reading::Unavailable(_) => None,
        }
    }
}

/// Everything one sampler tick collected.
#[derive(Clone, Debug)]
pub struct Snapshot {
    /// Busy percent averaged over all cores (0..100).
    pub cpu_total: f64,
    /// Busy percent per core (0..100 each).
    pub cpu_cores: Vec<f64>,
    pub mem: MemInfo,
    pub net: NetInfo,
    pub disk: Reading<DiskInfo>,
    /// GPU busy percent as the driver reports it.
    pub gpu_pct: Reading<f64>,
    /// Package power in watts. No backend measures it yet; it is never
    /// estimated from CPU load.
    pub power_watts: Reading<f64>,
    pub processes: Vec<ProcInfo>,
    pub load_avg: [f64; 3],
    pub uptime_seconds: u64,
    /// Which backend produced this (shown in the status line).
    pub backend: &'static str,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            cpu_total: 0.0,
            cpu_cores: Vec::new(),
            mem: MemInfo::default(),
            net: NetInfo::default(),
            disk: Reading::Unavailable("no disk counters on this platform"),
            gpu_pct: Reading::Unavailable("no GPU counters on this platform"),
            power_watts: Reading::Unavailable("power is not measured"),
            processes: Vec::new(),
            load_avg: [0.0; 3],
            uptime_seconds: 0,
            backend: "unsupported",
        }
    }
}

// ---- per-process detail (the inspector) ----

/// A detail block the OS either handed over or refused. `Unavailable`
/// carries the reason ("permission denied", "not supported on windows"), and
/// the inspector prints that instead of zeros.
#[derive(Clone, Debug)]
pub enum Detail<T> {
    Ready(T),
    Unavailable(String),
}

impl<T> Detail<T> {
    pub fn denied() -> Self {
        Detail::Unavailable("permission denied: the OS refused to open this process".to_string())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThreadState {
    Running,
    Stopped,
    Waiting,
    Uninterruptible,
    Halted,
    #[default]
    Unknown,
}

impl ThreadState {
    pub fn as_str(self) -> &'static str {
        match self {
            ThreadState::Running => "Running",
            ThreadState::Stopped => "Stopped",
            ThreadState::Waiting => "Waiting",
            ThreadState::Uninterruptible => "Uninterruptible",
            ThreadState::Halted => "Halted",
            ThreadState::Unknown => "?",
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ThreadInfo {
    /// The OS's handle for the thread: the thread id on Linux and Windows,
    /// the kernel thread handle `PROC_PIDLISTTHREADS` returns on macOS.
    pub id: u64,
    pub name: String,
    pub state: ThreadState,
    /// Percent of one core, as the kernel's own recent estimate; `None`
    /// where the OS keeps no such estimate per thread.
    pub cpu_pct: Option<f64>,
    /// Cumulative user+system time of this thread.
    pub cpu_time_ns: Option<u64>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct RegionSummary {
    pub regions: u32,
    pub resident: u64,
    pub private_resident: u64,
    pub shared_resident: u64,
}

/// The memory part of a detail read beyond its numeric `measures` (which
/// carry every memory figure with its OS name, see `metrics::Measure`).
#[derive(Clone, Debug, Default)]
pub struct MemoryDetail {
    /// Resident pages summed over the address-space walk; a different
    /// measurement from the memory ledger (shared pages count in every
    /// process that maps them).
    pub regions: Option<RegionSummary>,
}

#[derive(Clone, Debug, Default)]
pub struct FileInfo {
    pub fd: i32,
    pub kind: &'static str,
    pub path: String,
}

#[derive(Clone, Debug, Default)]
pub struct PortInfo {
    pub protocol: &'static str,
    pub local: String,
    pub remote: String,
    pub state: String,
}

#[derive(Clone, Debug, Default)]
pub struct LibraryInfo {
    pub path: String,
    /// Bytes mapped from this file.
    pub mapped: u64,
}

/// Identity facts read straight from the OS for the inspected process.
#[derive(Clone, Debug, Default)]
pub struct IdentityDetail {
    pub path: String,
    pub status: String,
    /// Read in the same call as the thread list, so the recorder can compare
    /// the process with its threads over one interval.
    pub cpu_time_ns: Option<u64>,
    pub threads: u32,
    pub running_threads: u32,
}

/// Everything the inspector shows for one process, collected in one go on
/// the worker. `time_ms` is when it was collected: shown next to the data so
/// a historical view can say exactly how old it is.
#[derive(Clone, Debug)]
pub struct ProcDetail {
    pub key: ProcKey,
    pub time_ms: u64,
    pub identity: Detail<IdentityDetail>,
    pub memory: Detail<MemoryDetail>,
    pub threads: Detail<Vec<ThreadInfo>>,
    pub files: Detail<Vec<FileInfo>>,
    pub ports: Detail<Vec<PortInfo>>,
    /// `None` when this collection skipped the (expensive) library walk; the
    /// previous list stays valid.
    pub libraries: Option<Detail<Vec<LibraryInfo>>>,
    /// Every numeric reading this read produced that the basic sample does
    /// not already carry, in [`crate::metrics::Measure`] units.
    pub measures: Vec<(crate::metrics::Measure, i64)>,
    /// The thread list is whole: a thread missing from it has ended. False
    /// when the list was cut at its cap or a thread could not be read.
    pub threads_complete: bool,
    /// The descriptor list is whole: a descriptor missing from it was
    /// closed. False when the list was cut at its cap.
    pub files_complete: bool,
}

impl ProcDetail {
    /// Every tab says the same thing: why nothing could be read.
    pub fn unavailable(key: ProcKey, time_ms: u64, reason: &str) -> Self {
        fn says<T>(reason: &str) -> Detail<T> {
            Detail::Unavailable(reason.to_string())
        }
        Self {
            key,
            time_ms,
            identity: says(reason),
            memory: says(reason),
            threads: says(reason),
            files: says(reason),
            ports: says(reason),
            libraries: Some(says(reason)),
            measures: Vec::new(),
            threads_complete: false,
            files_complete: false,
        }
    }

    pub fn gone(key: ProcKey, time_ms: u64) -> Self {
        Self::unavailable(key, time_ms, "the process is gone (or its pid now belongs to another process)")
    }
}

/// One operating system's view of the machine.
pub trait SystemBackend: Send {
    /// Collect a full snapshot. Called about once a second, off the UI thread.
    fn sample(&mut self) -> Snapshot;
    /// Short backend name for the status line.
    fn name(&self) -> &'static str;
    /// Detail for `key`, read now; `want` says which of the walks to run.
    /// A part not asked for is `Unavailable("not collected")`.
    fn detail(&mut self, key: ProcKey, want: Want) -> ProcDetail;
    /// Re-read `key`'s start stamp and signal it only when it still matches.
    /// Unix re-reads then calls `kill`, which leaves the (tiny) window of a
    /// pid reused between the two calls; Windows checks and terminates
    /// through one handle, so there is none.
    fn signal_verified(&mut self, key: ProcKey, force: bool) -> Result<(), String>;
    /// A direct check of one incarnation: `Some(true)` it runs, `Some(false)`
    /// it is verifiably gone (no such pid, or the pid now has another start
    /// stamp), `None` the OS would not say. Used before a process missing
    /// from a list is recorded as exited: a list can miss a process it could
    /// not read.
    fn is_alive(&mut self, _key: ProcKey) -> Option<bool> {
        None
    }
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

/// Ask a process to exit, but only the process the row meant: the start
/// stamp is re-read first and a mismatch (the pid was reused, or the process
/// is gone) refuses instead of signalling whatever holds the number now.
///
/// `force` escalates: SIGTERM → SIGKILL on unix. Windows has no polite
/// equivalent — `TerminateProcess` is always immediate — so `force` changes
/// nothing there and the UI says as much.
pub fn terminate(backend: &mut dyn SystemBackend, key: ProcKey, force: bool) -> Result<(), String> {
    if is_protected(key.pid) {
        return Err(format!("PID {} is protected and will not be signalled", key.pid));
    }
    if !key.verified() {
        return Err(format!("PID {} has no verified start time; not signalled", key.pid));
    }
    backend.signal_verified(key, force)
}

/// Processes nothing in task will signal: the kernel, init/launchd, and task.
pub fn is_protected(pid: u32) -> bool {
    pid == 0 || pid == 1 || pid == std::process::id()
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
        Snapshot::default()
    }

    fn detail(&mut self, key: ProcKey, _want: Want) -> ProcDetail {
        ProcDetail::unavailable(key, now_ms(), "process detail: not available on this platform")
    }

    fn signal_verified(&mut self, _key: ProcKey, _force: bool) -> Result<(), String> {
        Err("no process backend for this platform".to_string())
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
pub mod unix_signal {
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

/// Percent of one core from two cumulative CPU-time readings over a wall
/// clock span. A counter that went *down* (the pid was reused and the cache
/// missed it) reads 0 rather than a spike.
pub fn cpu_pct_from_time(now_ns: u64, before_ns: u64, elapsed_ns: u64) -> f64 {
    if elapsed_ns == 0 || now_ns < before_ns {
        return 0.0;
    }
    (now_ns - before_ns) as f64 / elapsed_ns as f64 * 100.0
}

/// The placeholder for a part of a detail read that was not asked for.
pub fn not_collected<T>() -> Detail<T> {
    Detail::Unavailable(NOT_COLLECTED.to_string())
}

pub const NOT_COLLECTED: &str = "not collected in this read";

/// Unix time in milliseconds.
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis().min(u64::MAX as u128) as u64)
        .unwrap_or(0)
}

/// Clamp a string the OS handed us to a sane length, so one process with a
/// megabyte of arguments cannot make every sample and journal chunk huge.
pub fn bounded(mut text: String, max: usize) -> String {
    if text.len() > max {
        let mut cut = max;
        while cut > 0 && !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
        text.push('…');
    }
    text
}

pub const MAX_NAME_LEN: usize = 256;
pub const MAX_CMDLINE_LEN: usize = 4096;

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
