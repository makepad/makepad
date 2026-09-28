//! The process table's columns: every figure and graph the table can show,
//! and how each one is read from the recorded samples. Which of them are
//! shown, in what order, how wide and sorted how is the shared
//! `makepad_widgets::GridColumns`; this module describes the columns to it
//! (`GridColumn`).
//!
//! A figure column reads its value through the same [`Measure`] catalogue
//! the inspector graphs: a gauge as read, a counter either as its total over
//! the process' life or as a rate. A rate is taken against the sample about
//! [`RATE_SPAN_MS`] older in the same recording session, so it reads the
//! same at a 100 ms interval as at 2 s, and a recording gap gives none.
//!
//! Graph columns are separate from the figures they plot: the last minute of
//! the process' CPU, memory, disk or network traffic, drawn by time.

use crate::backend::ProcExtra;
use crate::history::{ProcRecord, Sample, Store, SECOND_MS};
use crate::metrics::{self, Measure, Reading};
use makepad_widgets::{ColumnWidth, GridColumn};
use std::sync::Arc;

/// How far back a rate column looks for its earlier reading.
pub const RATE_SPAN_MS: u64 = SECOND_MS;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Column {
    Pin,
    Name,
    Pid,
    Ppid,
    User,
    State,
    Priority,
    Nice,
    Cpu,
    CpuHistory,
    CpuTime,
    Threads,
    IdleWakeups,
    ContextSwitches,
    Syscalls,
    Mem,
    MemHistory,
    Footprint,
    Virtual,
    Faults,
    Pageins,
    DiskRead,
    DiskWrite,
    BytesRead,
    BytesWritten,
    DiskHistory,
    NetIn,
    NetOut,
    BytesReceived,
    BytesSent,
    PacketsReceived,
    PacketsSent,
    NetHistory,
}

/// The column drawn before the chosen ones, always: the pin.
pub const LEADING: [Column; 1] = [Column::Pin];

/// Every column after the pin, in the chooser's order.
pub const CHOOSABLE: [Column; 32] = [
    Column::Name,
    Column::Pid,
    Column::Ppid,
    Column::User,
    Column::State,
    Column::Priority,
    Column::Nice,
    Column::Cpu,
    Column::CpuHistory,
    Column::CpuTime,
    Column::Threads,
    Column::IdleWakeups,
    Column::ContextSwitches,
    Column::Syscalls,
    Column::Mem,
    Column::MemHistory,
    Column::Footprint,
    Column::Virtual,
    Column::Faults,
    Column::Pageins,
    Column::DiskRead,
    Column::DiskWrite,
    Column::BytesRead,
    Column::BytesWritten,
    Column::DiskHistory,
    Column::NetIn,
    Column::NetOut,
    Column::BytesReceived,
    Column::BytesSent,
    Column::PacketsReceived,
    Column::PacketsSent,
    Column::NetHistory,
];

/// A graph column's traces.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Graph {
    Cpu,
    Memory,
    Disk,
    Network,
}

/// How a counter column reads its measure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Read {
    /// The value as recorded: a gauge, or a counter's total.
    Value,
    /// A counter per second.
    Rate,
}

/// The columns a first start shows, after the pin.
pub fn default_columns() -> Vec<Column> {
    use Column::*;
    let mut columns = vec![Name, Cpu, CpuHistory, Mem, MemHistory];
    // Only macOS attributes network traffic to processes.
    if cfg!(target_os = "macos") {
        columns.push(NetHistory);
    }
    columns.extend([Threads, Pid, User, State]);
    columns
}

impl GridColumn for Column {
    fn code(self) -> &'static str {
        match self {
            Column::Pin => "pin",
            Column::Name => "name",
            Column::Pid => "pid",
            Column::Ppid => "ppid",
            Column::User => "user",
            Column::State => "state",
            Column::Priority => "priority",
            Column::Nice => "nice",
            Column::Cpu => "cpu",
            Column::CpuHistory => "cpu_graph",
            Column::CpuTime => "cpu_time",
            Column::Threads => "threads",
            Column::IdleWakeups => "idle_wakeups",
            Column::ContextSwitches => "context_switches",
            Column::Syscalls => "syscalls",
            Column::Mem => "memory",
            Column::MemHistory => "memory_graph",
            Column::Footprint => "footprint",
            Column::Virtual => "virtual",
            Column::Faults => "faults",
            Column::Pageins => "pageins",
            Column::DiskRead => "disk_read",
            Column::DiskWrite => "disk_write",
            Column::BytesRead => "bytes_read",
            Column::BytesWritten => "bytes_written",
            Column::DiskHistory => "disk_graph",
            Column::NetIn => "net_in",
            Column::NetOut => "net_out",
            Column::BytesReceived => "bytes_received",
            Column::BytesSent => "bytes_sent",
            Column::PacketsReceived => "packets_received",
            Column::PacketsSent => "packets_sent",
            Column::NetHistory => "net_graph",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Column::Pin => "",
            Column::Name => "Process",
            Column::Pid => "PID",
            Column::Ppid => "PPID",
            Column::User => "User",
            Column::State => "State",
            Column::Priority => "Pri",
            Column::Nice => "Nice",
            Column::Cpu => "% CPU",
            Column::CpuHistory => "CPU 60 s",
            Column::CpuTime => "CPU Time",
            Column::Threads => "Thr",
            Column::IdleWakeups => "Wakes/s",
            Column::ContextSwitches => "Switches/s",
            Column::Syscalls => "Syscalls/s",
            Column::Mem => "Memory",
            Column::MemHistory => "Memory 60 s",
            Column::Footprint => "Footprint",
            Column::Virtual => "Virtual",
            Column::Faults => "Faults/s",
            Column::Pageins => if cfg!(target_os = "linux") { "Maj faults/s" } else { "Page-ins/s" },
            Column::DiskRead => "Read/s",
            Column::DiskWrite => "Write/s",
            Column::BytesRead => "Bytes Read",
            Column::BytesWritten => "Bytes Written",
            Column::DiskHistory => "Disk 60 s",
            Column::NetIn => "Recv/s",
            Column::NetOut => "Sent/s",
            Column::BytesReceived => "Rcvd Bytes",
            Column::BytesSent => "Sent Bytes",
            Column::PacketsReceived => "Rcvd Pkts",
            Column::PacketsSent => "Sent Pkts",
            Column::NetHistory => "Network 60 s",
        }
    }

    /// The chooser's row: the heading spelled out.
    fn menu_label(self) -> &'static str {
        match self {
            Column::Name => "Process Name",
            Column::Pid => "Process ID",
            Column::Ppid => "Parent Process ID",
            Column::Priority => "Priority",
            Column::Cpu => "% CPU",
            Column::CpuHistory => "CPU Graph",
            Column::Threads => "Threads",
            Column::IdleWakeups => "Idle Wake-ups per Second",
            Column::ContextSwitches => "Context Switches per Second",
            Column::Syscalls => "System Calls per Second",
            Column::Mem => if cfg!(windows) { "Memory (Working Set)" } else { "Memory (Resident)" },
            Column::MemHistory => "Memory Graph",
            Column::Footprint => "Physical Footprint",
            Column::Virtual => "Virtual Size",
            Column::Faults => "Page Faults per Second",
            Column::Pageins => if cfg!(target_os = "linux") { "Major Faults per Second" } else { "Page-ins per Second" },
            Column::DiskRead => "Disk Read per Second",
            Column::DiskWrite => "Disk Written per Second",
            Column::DiskHistory => "Disk Graph",
            Column::NetIn => "Received per Second",
            Column::NetOut => "Sent per Second",
            Column::BytesReceived => "Received Bytes",
            Column::PacketsReceived => "Received Packets",
            Column::PacketsSent => "Sent Packets",
            Column::NetHistory => "Network Graph",
            other => other.label(),
        }
    }

    fn section(self) -> Option<&'static str> {
        Some(match self {
            Column::Pin | Column::Name | Column::Pid | Column::Ppid | Column::User | Column::State | Column::Priority | Column::Nice => "General",
            Column::Cpu | Column::CpuHistory | Column::CpuTime | Column::Threads | Column::IdleWakeups | Column::ContextSwitches | Column::Syscalls => "CPU",
            Column::Mem | Column::MemHistory | Column::Footprint | Column::Virtual | Column::Faults | Column::Pageins => "Memory",
            Column::DiskRead | Column::DiskWrite | Column::BytesRead | Column::BytesWritten | Column::DiskHistory => "Disk",
            Column::NetIn | Column::NetOut | Column::BytesReceived | Column::BytesSent | Column::PacketsReceived | Column::PacketsSent | Column::NetHistory => "Network",
        })
    }

    /// The name takes the room the figures leave, 120 to 360; the graphs
    /// widen a little on a wide table; everything else is fixed.
    fn width(self) -> ColumnWidth {
        match self {
            Column::Name => ColumnWidth::Flex { share: 1.0, min: 120.0, max: 360.0 },
            Column::CpuHistory | Column::MemHistory | Column::DiskHistory | Column::NetHistory => ColumnWidth::Stretch { base: 104.0, max_extra: 40.0 },
            Column::Pin => ColumnWidth::Fixed(28.0),
            Column::Pid | Column::Ppid => ColumnWidth::Fixed(80.0),
            Column::User => ColumnWidth::Fixed(110.0),
            Column::State => ColumnWidth::Fixed(112.0),
            Column::Priority | Column::Nice => ColumnWidth::Fixed(60.0),
            Column::Cpu => ColumnWidth::Fixed(92.0),
            Column::Threads => ColumnWidth::Fixed(66.0),
            Column::CpuTime => ColumnWidth::Fixed(112.0),
            Column::Mem | Column::Footprint | Column::Virtual | Column::BytesRead | Column::BytesWritten | Column::BytesReceived | Column::BytesSent => ColumnWidth::Fixed(112.0),
            Column::DiskRead | Column::DiskWrite | Column::NetIn | Column::NetOut => ColumnWidth::Fixed(124.0),
            Column::IdleWakeups | Column::ContextSwitches | Column::Syscalls | Column::Faults | Column::Pageins | Column::PacketsReceived | Column::PacketsSent => ColumnWidth::Fixed(104.0),
        }
    }

    /// The pin and the name are always there.
    fn hideable(self) -> bool {
        !matches!(self, Column::Pin | Column::Name)
    }

    fn movable(self) -> bool {
        self != Column::Pin
    }

    fn sortable(self) -> bool {
        self != Column::Pin
    }

    /// Figures read best biggest first; names, ids and states A-Z.
    fn descending_first(self) -> bool {
        !matches!(self, Column::Name | Column::User | Column::State | Column::Pid | Column::Ppid | Column::Priority | Column::Nice)
    }

}

impl Column {
    pub fn graph(self) -> Option<Graph> {
        match self {
            Column::CpuHistory => Some(Graph::Cpu),
            Column::MemHistory => Some(Graph::Memory),
            Column::DiskHistory => Some(Graph::Disk),
            Column::NetHistory => Some(Graph::Network),
            _ => None,
        }
    }

    /// Characters a figure is padded to in the monospace face, so a column's
    /// figures end on one edge (the grid aligns every heading one way).
    pub fn chars(self) -> usize {
        match self {
            Column::Cpu => 5,
            Column::Threads => 4,
            Column::Pid | Column::Ppid => 6,
            Column::Priority | Column::Nice => 4,
            Column::CpuTime => 11,
            Column::DiskRead | Column::DiskWrite | Column::NetIn | Column::NetOut => 12,
            Column::IdleWakeups | Column::ContextSwitches | Column::Syscalls | Column::Faults | Column::Pageins => 9,
            Column::PacketsReceived | Column::PacketsSent => 11,
            _ => 10,
        }
    }

    /// The figure the column shows, as a measure and how it is read.
    fn source(self) -> Option<(Measure, Read)> {
        Some(match self {
            Column::Priority => (Measure::Priority, Read::Value),
            Column::Nice => (Measure::Nice, Read::Value),
            Column::CpuTime => (Measure::CpuTime, Read::Value),
            Column::IdleWakeups => (Measure::IdleWakeups, Read::Rate),
            Column::ContextSwitches => (Measure::ContextSwitches, Read::Rate),
            Column::Syscalls => (Measure::Syscalls, Read::Rate),
            Column::Footprint => (Measure::Footprint, Read::Value),
            Column::Virtual => (Measure::Virtual, Read::Value),
            Column::Faults => (Measure::Faults, Read::Rate),
            Column::Pageins => (Measure::Pageins, Read::Rate),
            Column::DiskRead => (Measure::DiskRead, Read::Rate),
            Column::DiskWrite => (Measure::DiskWritten, Read::Rate),
            Column::BytesRead => (Measure::DiskRead, Read::Value),
            Column::BytesWritten => (Measure::DiskWritten, Read::Value),
            Column::NetIn => (Measure::NetReceived, Read::Rate),
            Column::NetOut => (Measure::NetSent, Read::Rate),
            Column::BytesReceived => (Measure::NetReceived, Read::Value),
            Column::BytesSent => (Measure::NetSent, Read::Value),
            Column::PacketsReceived => (Measure::PacketsReceived, Read::Value),
            Column::PacketsSent => (Measure::PacketsSent, Read::Value),
            _ => return None,
        })
    }
}

/// Reads the table's figures from the view sample and, for rates, the
/// sample about [`RATE_SPAN_MS`] before it in the same session.
pub struct Figures {
    base: Option<Arc<Sample>>,
}

impl Figures {
    pub fn new(store: &Store, sample: &Sample) -> Self {
        let base = store.index_at(sample.time_ms.saturating_sub(RATE_SPAN_MS)).and_then(|mut index| {
            // Strictly older than the view, however close the samples are.
            loop {
                let entry = store.entry(index)?;
                if entry.sample.time_ms < sample.time_ms {
                    break;
                }
                index = index.checked_sub(1)?;
            }
            let base = &store.entry(index)?.sample;
            // A rate across a restart or a long pause would be an average
            // over time nobody recorded.
            let recent = sample.time_ms - base.time_ms <= RATE_SPAN_MS * 5 + SECOND_MS;
            (base.session == sample.session && recent).then(|| base.clone())
        });
        Self { base }
    }

    /// The number a column sorts by and shows, `None` where the OS did not
    /// record it (or, for a rate, there is no earlier reading to take it
    /// against). Rates are per second; CPU is percent of one core; the
    /// rest are in their measure's unit.
    pub fn value(&self, sample: &Sample, record: &ProcRecord, column: Column) -> Option<f64> {
        let extra = || sample.extra(record);
        self.value_with(sample, record, &extra, column)
    }

    fn value_with(&self, sample: &Sample, record: &ProcRecord, extra: &dyn Fn() -> ProcExtra, column: Column) -> Option<f64> {
        match column {
            Column::Cpu | Column::CpuHistory => Some(record.cpu as f64),
            Column::Mem | Column::MemHistory => Some(record.rss as f64),
            Column::Threads => Some(record.threads as f64),
            Column::Pid => Some(record.meta.key.pid as f64),
            Column::Ppid => Some(record.meta.ppid as f64),
            Column::DiskHistory => sum(self.value_with(sample, record, extra, Column::DiskRead), self.value_with(sample, record, extra, Column::DiskWrite)),
            Column::NetHistory => sum(self.value_with(sample, record, extra, Column::NetIn), self.value_with(sample, record, extra, Column::NetOut)),
            _ => {
                let (measure, read) = column.source()?;
                let now = match measure {
                    Measure::CpuTime => record.cpu_time().map(|ns| ns.min(i64::MAX as u64) as i64),
                    _ => metrics::extra_value(&extra(), measure),
                }?;
                match read {
                    Read::Value => Some(now as f64),
                    Read::Rate => {
                        let base = self.base.as_ref()?;
                        let then = metrics::basic_value(base, base.process(record.key())?, measure)?;
                        let then = Reading { time_ms: base.time_ms, value: then, gap_before: false };
                        let now = Reading { time_ms: sample.time_ms, value: now, gap_before: false };
                        metrics::rate_between(measure, then, now).map(|rate| rate as f64)
                    }
                }
            }
        }
    }

    /// The figure as the cell prints it, unpadded; `None` when there is
    /// none (the cell prints a quiet dash).
    pub fn text(&self, sample: &Sample, record: &ProcRecord, column: Column) -> Option<String> {
        let value = self.value(sample, record, column)?;
        Some(match column {
            Column::Cpu => format!("{value:.1}"),
            Column::Mem => crate::format_bytes(value as u64),
            Column::Threads | Column::Pid | Column::Ppid => format!("{value:.0}"),
            _ => {
                let (measure, read) = column.source()?;
                match read {
                    Read::Value => measure.format_value(value as i64),
                    Read::Rate => measure.format_point(value as f32),
                }
            }
        })
    }
}

fn sum(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    match (a, b) {
        (None, None) => None,
        (a, b) => Some(a.unwrap_or(0.0) + b.unwrap_or(0.0)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_column_has_a_distinct_code() {
        let mut seen = std::collections::HashSet::new();
        for column in LEADING.into_iter().chain(CHOOSABLE) {
            assert!(seen.insert(column.code()), "{column:?}");
        }
    }
}
