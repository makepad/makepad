//! The catalogue of per-process figures task records, and the arithmetic
//! that turns recorded readings into what the inspector graphs.
//!
//! Every figure is a [`Measure`] with one unit and one kind:
//!
//! * a **gauge** (resident bytes, thread count, priority) is graphed as read;
//! * a **counter** (CPU time, page faults, disk bytes) only grows while the
//!   process lives, so it is graphed as a *rate* between two real readings:
//!   `(v1 - v0) / (t1 - t0)` with the actual timestamps. A reading missing on
//!   either side, a recorded gap between them, or a counter that went down
//!   (a reset, a 32-bit kernel counter wrapping) gives no rate there, and the
//!   line breaks — nothing is interpolated.
//!
//! The codes are stored on disk; they never change meaning.

use crate::history::Point;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Measure {
    /// Percent of one core, as the sampler computed it.
    Cpu,
    Resident,
    CpuTime,
    Threads,
    Virtual,
    Faults,
    Pageins,
    CowFaults,
    ContextSwitches,
    Syscalls,
    Priority,
    Nice,
    RunningThreads,
    Commit,
    PeakResident,
    PeakCommit,
    Footprint,
    PeakFootprint,
    Wired,
    DiskRead,
    DiskWritten,
    IoRead,
    IoWritten,
    OpenFds,
    FdTable,
    AnonResident,
    FileResident,
    Swapped,
    IdleWakeups,
    NetReceived,
    NetSent,
    PacketsReceived,
    PacketsSent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unit {
    /// Percent of one core.
    Percent,
    Bytes,
    Count,
    /// Nanoseconds; as a rate, a share of one core.
    Nanos,
    /// A plain signed number (priority, nice).
    Plain,
}

/// Every measure, in the order the inspector lists them.
pub const MEASURES: [Measure; 33] = [
    Measure::Cpu,
    Measure::CpuTime,
    Measure::Footprint,
    Measure::Resident,
    Measure::Commit,
    Measure::Virtual,
    Measure::PeakFootprint,
    Measure::PeakResident,
    Measure::PeakCommit,
    Measure::Wired,
    Measure::AnonResident,
    Measure::FileResident,
    Measure::Swapped,
    Measure::Threads,
    Measure::RunningThreads,
    Measure::ContextSwitches,
    Measure::Syscalls,
    Measure::Faults,
    Measure::Pageins,
    Measure::CowFaults,
    Measure::DiskRead,
    Measure::DiskWritten,
    Measure::NetReceived,
    Measure::NetSent,
    Measure::PacketsReceived,
    Measure::PacketsSent,
    Measure::IoRead,
    Measure::IoWritten,
    Measure::OpenFds,
    Measure::FdTable,
    Measure::Priority,
    Measure::Nice,
    Measure::IdleWakeups,
];

impl Measure {
    /// The stable on-disk code.
    pub fn code(self) -> u8 {
        match self {
            Measure::Cpu => 1,
            Measure::Resident => 2,
            Measure::CpuTime => 3,
            Measure::Threads => 4,
            Measure::Virtual => 5,
            Measure::Faults => 6,
            Measure::Pageins => 7,
            Measure::CowFaults => 8,
            Measure::ContextSwitches => 9,
            Measure::Syscalls => 10,
            Measure::Priority => 11,
            Measure::Nice => 12,
            Measure::RunningThreads => 13,
            Measure::Commit => 14,
            Measure::PeakResident => 15,
            Measure::PeakCommit => 16,
            Measure::Footprint => 17,
            Measure::PeakFootprint => 18,
            Measure::Wired => 19,
            Measure::DiskRead => 20,
            Measure::DiskWritten => 21,
            Measure::IoRead => 22,
            Measure::IoWritten => 23,
            Measure::OpenFds => 24,
            Measure::FdTable => 25,
            Measure::AnonResident => 26,
            Measure::FileResident => 27,
            Measure::Swapped => 28,
            Measure::IdleWakeups => 29,
            Measure::NetReceived => 30,
            Measure::NetSent => 31,
            Measure::PacketsReceived => 32,
            Measure::PacketsSent => 33,
        }
    }

    pub fn from_code(code: u8) -> Option<Self> {
        MEASURES.iter().copied().find(|measure| measure.code() == code)
    }

    pub fn unit(self) -> Unit {
        match self {
            Measure::Cpu => Unit::Percent,
            Measure::CpuTime => Unit::Nanos,
            Measure::Resident
            | Measure::Virtual
            | Measure::Commit
            | Measure::PeakResident
            | Measure::PeakCommit
            | Measure::Footprint
            | Measure::PeakFootprint
            | Measure::Wired
            | Measure::DiskRead
            | Measure::DiskWritten
            | Measure::IoRead
            | Measure::IoWritten
            | Measure::AnonResident
            | Measure::FileResident
            | Measure::Swapped
            | Measure::NetReceived
            | Measure::NetSent => Unit::Bytes,
            Measure::Priority | Measure::Nice => Unit::Plain,
            _ => Unit::Count,
        }
    }

    /// Cumulative for the life of the process: graphed as a rate.
    pub fn is_counter(self) -> bool {
        matches!(
            self,
            Measure::CpuTime
                | Measure::Faults
                | Measure::Pageins
                | Measure::CowFaults
                | Measure::ContextSwitches
                | Measure::Syscalls
                | Measure::DiskRead
                | Measure::DiskWritten
                | Measure::IoRead
                | Measure::IoWritten
                | Measure::IdleWakeups
                | Measure::NetReceived
                | Measure::NetSent
                | Measure::PacketsReceived
                | Measure::PacketsSent
        )
    }

    /// The OS's own name for the figure.
    pub fn label(self) -> &'static str {
        match self {
            Measure::Cpu => "CPU",
            Measure::Resident => {
                if cfg!(windows) {
                    "Working set"
                } else if cfg!(target_os = "linux") {
                    "Resident (RSS)"
                } else {
                    "Resident"
                }
            }
            Measure::CpuTime => "CPU time",
            Measure::Threads => "Threads",
            Measure::Virtual => "Virtual size",
            Measure::Faults => "Page faults",
            Measure::Pageins => if cfg!(target_os = "linux") { "Major faults" } else { "Page-ins" },
            Measure::CowFaults => "Copy-on-write faults",
            Measure::ContextSwitches => "Context switches",
            Measure::Syscalls => "System calls",
            Measure::Priority => if cfg!(windows) { "Base priority" } else { "Priority" },
            Measure::Nice => "Nice",
            Measure::RunningThreads => "Running threads",
            Measure::Commit => "Private commit",
            Measure::PeakResident => if cfg!(windows) { "Peak working set" } else { "Peak resident" },
            Measure::PeakCommit => "Peak private commit",
            Measure::Footprint => "Physical footprint",
            Measure::PeakFootprint => "Peak footprint",
            Measure::Wired => "Wired",
            Measure::DiskRead => "Disk read",
            Measure::DiskWritten => "Disk written",
            Measure::IoRead => "I/O read",
            Measure::IoWritten => "I/O written",
            Measure::OpenFds => "Open descriptors",
            Measure::FdTable => "Descriptor table",
            Measure::AnonResident => "Anonymous resident",
            Measure::FileResident => "File-backed resident",
            Measure::Swapped => "Swapped",
            Measure::IdleWakeups => "Idle wake-ups",
            Measure::NetReceived => "Received bytes",
            Measure::NetSent => "Sent bytes",
            Measure::PacketsReceived => "Received packets",
            Measure::PacketsSent => "Sent packets",
        }
    }

    /// A short chip label for the History selector.
    pub fn short(self) -> &'static str {
        match self {
            Measure::Cpu => "CPU",
            Measure::Resident => if cfg!(windows) { "Working set" } else { "Resident" },
            Measure::CpuTime => "CPU time",
            Measure::Threads => "Threads",
            Measure::Virtual => "Virtual",
            Measure::Faults => "Faults",
            Measure::Pageins => if cfg!(target_os = "linux") { "Major faults" } else { "Page-ins" },
            Measure::CowFaults => "COW faults",
            Measure::ContextSwitches => "Switches",
            Measure::Syscalls => "Syscalls",
            Measure::Priority => "Priority",
            Measure::Nice => "Nice",
            Measure::RunningThreads => "Running",
            Measure::Commit => "Commit",
            Measure::PeakResident => "Peak resident",
            Measure::PeakCommit => "Peak commit",
            Measure::Footprint => "Footprint",
            Measure::PeakFootprint => "Peak footprint",
            Measure::Wired => "Wired",
            Measure::DiskRead => "Disk R",
            Measure::DiskWritten => "Disk W",
            Measure::IoRead => "I/O R",
            Measure::IoWritten => "I/O W",
            Measure::OpenFds => "Descriptors",
            Measure::FdTable => "FD table",
            Measure::AnonResident => "Anon",
            Measure::FileResident => "File-backed",
            Measure::Swapped => "Swapped",
            Measure::IdleWakeups => "Wake-ups",
            Measure::NetReceived => "Net in",
            Measure::NetSent => "Net out",
            Measure::PacketsReceived => "Pkts in",
            Measure::PacketsSent => "Pkts out",
        }
    }

    /// What the figure is, in a line.
    pub fn meaning(self) -> &'static str {
        match self {
            Measure::Cpu => "share of one core over the last sampling interval",
            Measure::Resident => "pages in RAM now, shared pages counted in every process that maps them",
            Measure::CpuTime => "user + system time over the process' life",
            Measure::Threads => "threads in the process",
            Measure::Virtual => "address space reserved, mostly not backed by RAM",
            Measure::Faults => if cfg!(target_os = "linux") { "minor + major faults over the process' life" } else { "page faults over the process' life" },
            Measure::Pageins => if cfg!(target_os = "linux") { "faults that read a page from storage" } else { "pages read in from storage" },
            Measure::CowFaults => "faults that copied a shared page on write",
            Measure::ContextSwitches => if cfg!(target_os = "linux") { "voluntary + involuntary switches" } else { "switches of the process' threads" },
            Measure::Syscalls => "Mach + BSD system calls",
            Measure::Priority => if cfg!(windows) { "the process' base priority class value" } else if cfg!(target_os = "linux") { "the kernel's scheduling priority (20 + nice for normal tasks)" } else { "the task's scheduling priority" },
            Measure::Nice => "the process' nice value",
            Measure::RunningThreads => "threads running at the moment of the read",
            Measure::Commit => "private memory the system has promised this process (Task Manager's Commit size)",
            Measure::PeakResident => if cfg!(windows) { "the highest working set over the process' life" } else { "the highest RSS over the process' life" },
            Measure::PeakCommit => "the highest private commit over the process' life",
            Measure::Footprint => "the kernel's ledger of memory charged to this process, including compressed pages (Activity Monitor's Memory)",
            Measure::PeakFootprint => "the highest physical footprint over the process' life",
            Measure::Wired => "memory the process has wired (locked in RAM)",
            Measure::DiskRead => "bytes read from storage",
            Measure::DiskWritten => "bytes written to storage",
            Measure::IoRead => "bytes read by all I/O: files, network and devices",
            Measure::IoWritten => "bytes written by all I/O: files, network and devices",
            Measure::OpenFds => "descriptors listed by the last descriptor walk",
            Measure::FdTable => "slots in the descriptor table (not the number open)",
            Measure::AnonResident => "heap, stacks and other private memory in RAM",
            Measure::FileResident => "mapped files and libraries in RAM",
            Measure::Swapped => "private memory currently in swap",
            Measure::IdleWakeups => "times the process woke the CPU from idle",
            Measure::NetReceived => "bytes received on the process' sockets",
            Measure::NetSent => "bytes sent on the process' sockets",
            Measure::PacketsReceived => "packets received on the process' sockets",
            Measure::PacketsSent => "packets sent on the process' sockets",
        }
    }

    /// The figure as read: a gauge's value, a counter's total.
    pub fn format_value(self, value: i64) -> String {
        match self.unit() {
            Unit::Percent => format!("{:.1}%", value as f64 / 100.0),
            Unit::Bytes => crate::format_bytes(value.max(0) as u64),
            Unit::Nanos => crate::format_duration_ns(value.max(0) as u64),
            Unit::Count => group_digits(value.max(0) as u64),
            Unit::Plain => value.to_string(),
        }
    }

    /// A graphed point: a gauge's value, or a counter's rate per second.
    pub fn format_point(self, value: f32) -> String {
        match (self.unit(), self.is_counter()) {
            (Unit::Percent, _) => format!("{value:.1}%"),
            (Unit::Nanos, true) => format!("{value:.1}% core"),
            (Unit::Bytes, true) => format!("{}/s", crate::format_bytes(value.max(0.0) as u64)),
            (Unit::Bytes, false) => crate::format_bytes(value.max(0.0) as u64),
            (Unit::Count, true) => format_rate(value as f64),
            (Unit::Count, false) | (Unit::Plain, _) | (Unit::Nanos, false) => format!("{value:.0}"),
        }
    }

    /// What the graphed value is, for a well's corner.
    pub fn graph_unit(self) -> &'static str {
        match (self.unit(), self.is_counter()) {
            (Unit::Percent, _) => "% of a core",
            (Unit::Nanos, true) => "% of a core, from CPU time",
            (Unit::Bytes, true) => "bytes/s",
            (Unit::Bytes, false) => "bytes",
            (Unit::Count, true) => "per second",
            _ => "",
        }
    }
}

/// Stored values are integers: percent is kept in hundredths.
pub fn percent_to_stored(percent: f32) -> i64 {
    (percent as f64 * 100.0).round() as i64
}

pub fn format_rate(per_second: f64) -> String {
    if per_second >= 100.0 {
        format!("{}/s", group_digits(per_second.round() as u64))
    } else {
        format!("{per_second:.1}/s")
    }
}

/// `12 345 678`, with thin spaces the figure face keeps aligned.
pub fn group_digits(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, c) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            out.push('\u{2009}');
        }
        out.push(c);
    }
    out
}

/// One recorded reading of one measure.
#[derive(Clone, Copy, Debug)]
pub struct Reading {
    pub time_ms: u64,
    pub value: i64,
    /// The line must not join the previous reading.
    pub gap_before: bool,
}

/// Graph points for `measure` from its readings, oldest first: gauges as
/// they are (percent back from hundredths), counters as rates between
/// neighbouring readings with no gap and no decrease between them.
pub fn graph_points(measure: Measure, readings: &[Reading]) -> Vec<Point> {
    if !measure.is_counter() {
        let scale = if measure.unit() == Unit::Percent { 0.01 } else { 1.0 };
        return readings.iter().map(|r| Point { time_ms: r.time_ms, value: (r.value as f64 * scale) as f32, gap_before: r.gap_before }).collect();
    }
    let mut out = Vec::with_capacity(readings.len());
    let mut previous: Option<Reading> = None;
    let mut joined = false;
    for reading in readings {
        let rate = match previous {
            Some(then) if !reading.gap_before => rate_between(measure, then, *reading),
            _ => None,
        };
        match rate {
            Some(value) => {
                out.push(Point { time_ms: reading.time_ms, value, gap_before: !joined });
                joined = true;
            }
            None => joined = false,
        }
        previous = Some(*reading);
    }
    out
}

/// The rate between two readings of a counter, or `None` when there is none
/// to give (no time between them, or the counter went down).
pub fn rate_between(measure: Measure, then: Reading, now: Reading) -> Option<f32> {
    let span_ms = now.time_ms.checked_sub(then.time_ms)?;
    if span_ms == 0 || now.value < then.value {
        return None;
    }
    let per_second = (now.value - then.value) as f64 / (span_ms as f64 / 1000.0);
    Some(match measure.unit() {
        // Nanoseconds of CPU per second of wall clock, as a share of a core.
        Unit::Nanos => (per_second / 1e9 * 100.0) as f32,
        _ => per_second as f32,
    })
}

/// Whether the basic sample carries `measure` (for every process).
pub fn is_basic(measure: Measure) -> bool {
    matches!(
        measure,
        Measure::Cpu
            | Measure::Resident
            | Measure::CpuTime
            | Measure::Threads
            | Measure::Virtual
            | Measure::Faults
            | Measure::Pageins
            | Measure::CowFaults
            | Measure::ContextSwitches
            | Measure::Syscalls
            | Measure::Priority
            | Measure::Nice
            | Measure::RunningThreads
            | Measure::Commit
            | Measure::PeakResident
            | Measure::PeakCommit
            | Measure::DiskRead
            | Measure::DiskWritten
            | Measure::Footprint
            | Measure::IdleWakeups
            | Measure::NetReceived
            | Measure::NetSent
            | Measure::PacketsReceived
            | Measure::PacketsSent
    )
}

/// `measure` in one basic-sample record, in stored units.
pub fn basic_value(sample: &crate::history::Sample, record: &crate::history::ProcRecord, measure: Measure) -> Option<i64> {
    // Resident size and (on macOS) the thread count come from the same task
    // read as the CPU time; a process that read refused has neither, and
    // its zeros are not readings.
    let task_read = record.cpu_time().is_some();
    match measure {
        Measure::Cpu => Some(percent_to_stored(record.cpu)),
        Measure::Resident => task_read.then_some(record.rss as i64),
        Measure::CpuTime => record.cpu_time().map(|ns| ns.min(i64::MAX as u64) as i64),
        Measure::Threads => (task_read || !cfg!(target_os = "macos")).then_some(record.threads as i64),
        _ => extra_value(&sample.extra(record), measure),
    }
}

pub fn extra_value(extra: &crate::backend::ProcExtra, measure: Measure) -> Option<i64> {
    let bytes = |v: Option<u64>| v.map(|v| v.min(i64::MAX as u64) as i64);
    match measure {
        Measure::Virtual => bytes(extra.virtual_bytes),
        Measure::Faults => bytes(extra.faults),
        Measure::Pageins => bytes(extra.pageins),
        Measure::CowFaults => bytes(extra.cow_faults),
        Measure::ContextSwitches => bytes(extra.context_switches),
        Measure::Syscalls => bytes(extra.syscalls),
        Measure::Priority => extra.priority.map(|v| v as i64),
        Measure::Nice => extra.nice.map(|v| v as i64),
        Measure::RunningThreads => extra.running_threads.map(|v| v as i64),
        Measure::Commit => bytes(extra.commit_bytes),
        Measure::PeakResident => bytes(extra.peak_resident),
        Measure::PeakCommit => bytes(extra.peak_commit),
        Measure::DiskRead => bytes(extra.disk_read),
        Measure::DiskWritten => bytes(extra.disk_written),
        Measure::Footprint => bytes(extra.footprint),
        Measure::IdleWakeups => bytes(extra.idle_wakeups),
        Measure::NetReceived => bytes(extra.net_rx_bytes),
        Measure::NetSent => bytes(extra.net_tx_bytes),
        Measure::PacketsReceived => bytes(extra.net_rx_packets),
        Measure::PacketsSent => bytes(extra.net_tx_packets),
        _ => None,
    }
}

/// Readings of `measure` for `key`: from the basic history when it carries
/// the figure for this process, else from the supplemental record.
pub fn readings(store: &crate::history::Store, supp: &crate::supp::SuppStore, key: crate::backend::ProcKey, measure: Measure, from_ms: u64, to_ms: u64) -> Vec<Reading> {
    if is_basic(measure) {
        let basic = store.readings(key, measure, from_ms, to_ms);
        if !basic.is_empty() {
            return basic;
        }
    }
    supp.readings(key, measure, from_ms, to_ms)
}

/// The measures recorded for `key`: the basic figures present in `sample`
/// (the view sample), and every supplemental one ever recorded for it.
pub fn available(sample: Option<&crate::history::Sample>, supp: &crate::supp::SuppStore, key: crate::backend::ProcKey) -> Vec<Measure> {
    let record = sample.and_then(|s| s.process(key).map(|r| (s, r)));
    let seen = supp.get(key).map(|p| p.measures_seen).unwrap_or(0);
    MEASURES
        .iter()
        .copied()
        .filter(|m| record.is_some_and(|(s, r)| basic_value(s, r, *m).is_some()) || seen & (1u64 << m.code()) != 0)
        .collect()
}

/// The reading of `measure` for `key` at `time_ms` (basic history first,
/// then the supplemental record within `tolerance_ms`), with the previous
/// joined reading for a rate.
pub fn reading_at(
    store: &crate::history::Store,
    supp: &crate::supp::SuppStore,
    key: crate::backend::ProcKey,
    measure: Measure,
    time_ms: u64,
    tolerance_ms: u64,
) -> Option<(Reading, Option<Reading>)> {
    if is_basic(measure) {
        if let Some(pair) = store.reading_pair(key, measure, time_ms).filter(|(now, _)| now.time_ms.abs_diff(time_ms) <= tolerance_ms) {
            return Some(pair);
        }
    }
    supp.reading_at(key, measure, time_ms, tolerance_ms)
}
