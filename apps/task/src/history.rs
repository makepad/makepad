//! The retained history: every sample the worker took, kept in memory with a
//! time budget and a byte budget, and looked up by time for the band, the
//! tiles, the table and the inspector.
//!
//! A [`Sample`] is immutable and `Arc`-shared: the worker builds it, the
//! journal writes it, the store keeps it. Per-process rows are compact
//! numbers plus an `Arc<ProcMeta>` — the strings live once per process
//! incarnation, not once per tick.
//!
//! # Tiers
//!
//! Everything younger than 15 minutes is kept raw. Between 15 minutes and an
//! hour one real sample per 10 s survives; beyond an hour one per 60 s, up to
//! 24 h. Thinning keeps the *last actual sample* of each bucket rather than
//! averaging a synthetic one, so a historical view is always a real
//! snapshot at a real timestamp, with the processes that existed then.
//! The retained resolution is stated next to the cursor.
//!
//! # Gaps
//!
//! Nothing is interpolated. Every sample carries the recording session it
//! belongs to (one per worker start) and the spacing it was retained at.
//! Neighbours from different sessions, or further apart than their spacing
//! (plus slack), are a gap — app downtime, sleep, a stalled worker — and
//! every graph breaks its line there. Thinning never merges two sessions
//! into one bucket, so a restart survives compaction. A process that was not
//! in a sample has no point there.

use crate::backend::{ProcExtra, ProcKey, ProcMeta, ProcState, Reading, Snapshot};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

pub const SECOND_MS: u64 = 1_000;
pub const MINUTE_MS: u64 = 60 * SECOND_MS;
pub const HOUR_MS: u64 = 60 * MINUTE_MS;
/// Raw samples are kept this long.
pub const FINE_WINDOW_MS: u64 = 15 * MINUTE_MS;
/// One sample per [`MID_BUCKET_MS`] survives this long.
pub const MID_WINDOW_MS: u64 = HOUR_MS;
/// Nothing older than this is kept.
pub const MAX_AGE_MS: u64 = 24 * HOUR_MS;
pub const MID_BUCKET_MS: u64 = 10 * SECOND_MS;
pub const COARSE_BUCKET_MS: u64 = MINUTE_MS;
/// Byte budget bounds; the actual budget is a share of the machine's RAM.
pub const MIN_BUDGET_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_BUDGET_BYTES: usize = 128 * 1024 * 1024;
/// Per-process rows per sample the store will accept from disk or the worker.
pub const MAX_PROCESSES_PER_SAMPLE: usize = 65_535;

/// The history byte budget for a machine with this much RAM: 1/256th,
/// clamped to 16..128 MiB.
pub fn budget_for_ram(total_bytes: u64) -> usize {
    if total_bytes == 0 {
        return 64 * 1024 * 1024;
    }
    ((total_bytes / 256) as usize).clamp(MIN_BUDGET_BYTES, MAX_BUDGET_BYTES)
}

/// The unknown marker for `ProcRecord::cpu_time_ns`.
pub const CPU_TIME_UNKNOWN: u64 = u64::MAX;

/// One process in one sample. 40 bytes, plus its packed extras.
#[derive(Clone, Debug)]
pub struct ProcRecord {
    pub meta: Arc<ProcMeta>,
    /// Percent of one core.
    pub cpu: f32,
    pub rss: u64,
    /// Cumulative CPU nanoseconds, or [`CPU_TIME_UNKNOWN`].
    pub cpu_time_ns: u64,
    pub threads: u16,
    pub state: ProcState,
    /// Where this record's [`ProcExtra`] starts in its sample's `extras`, or
    /// [`NO_EXTRA`] (a record read back from a version 2 journal).
    pub extra: u32,
}

pub const NO_EXTRA: u32 = u32::MAX;

// ---- packed extras ----
//
// A record's `ProcExtra` is stored as a presence mask then one LEB128 varint
// per present field (signed fields zig-zagged), so the figures an OS does not
// have cost nothing and small counters cost a byte or two: about 25 bytes
// per process on macOS, 10 on Linux, 12 on Windows. The same bytes go to the
// journal unchanged.

pub fn put_varint(out: &mut Vec<u8>, mut value: u64) {
    while value >= 0x80 {
        out.push((value as u8) | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

/// A varint at `bytes[*at..]`, advancing `at`; `None` when truncated or
/// longer than a u64.
pub fn get_varint(bytes: &[u8], at: &mut usize) -> Option<u64> {
    let mut value = 0u64;
    for shift in (0..64).step_by(7) {
        let byte = *bytes.get(*at)?;
        *at += 1;
        value |= ((byte & 0x7f) as u64) << shift;
        if byte & 0x80 == 0 {
            return Some(value);
        }
    }
    None
}

pub fn zigzag(value: i64) -> u64 {
    ((value << 1) ^ (value >> 63)) as u64
}

pub fn unzigzag(value: u64) -> i64 {
    ((value >> 1) as i64) ^ -((value & 1) as i64)
}

/// Pack `extra` onto `out`; returns nothing when no field is present.
pub fn pack_extra(out: &mut Vec<u8>, extra: &ProcExtra) {
    let unsigned = [
        extra.virtual_bytes,
        extra.faults,
        extra.pageins,
        extra.cow_faults,
        extra.context_switches,
        extra.syscalls,
    ];
    let signed = [extra.priority, extra.nice];
    let tail = [
        extra.running_threads.map(|v| v as u64),
        extra.commit_bytes,
        extra.peak_resident,
        extra.peak_commit,
        extra.disk_read,
        extra.disk_written,
        extra.footprint,
        extra.idle_wakeups,
        extra.net_rx_bytes,
        extra.net_tx_bytes,
        extra.net_rx_packets,
        extra.net_tx_packets,
    ];
    let mut mask = 0u64;
    for (bit, value) in unsigned.iter().enumerate() {
        mask |= (value.is_some() as u64) << bit;
    }
    for (bit, value) in signed.iter().enumerate() {
        mask |= (value.is_some() as u64) << (6 + bit);
    }
    for (bit, value) in tail.iter().enumerate() {
        mask |= (value.is_some() as u64) << (8 + bit);
    }
    put_varint(out, mask);
    for value in unsigned.iter().flatten() {
        put_varint(out, *value);
    }
    for value in signed.iter().flatten() {
        put_varint(out, zigzag(*value as i64));
    }
    for value in tail.iter().flatten() {
        put_varint(out, *value);
    }
}

/// Unpack one extras record at `bytes[*at..]`.
pub fn unpack_extra(bytes: &[u8], at: &mut usize) -> Option<ProcExtra> {
    let mask = get_varint(bytes, at)?;
    if mask >> 20 != 0 {
        return None;
    }
    let mut next = |bit: u32| -> Option<Option<u64>> { if mask & (1 << bit) != 0 { get_varint(bytes, at).map(Some) } else { Some(None) } };
    let virtual_bytes = next(0)?;
    let faults = next(1)?;
    let pageins = next(2)?;
    let cow_faults = next(3)?;
    let context_switches = next(4)?;
    let syscalls = next(5)?;
    let priority = next(6)?.map(|v| unzigzag(v) as i32);
    let nice = next(7)?.map(|v| unzigzag(v) as i32);
    let running_threads = next(8)?.map(|v| v.min(u32::MAX as u64) as u32);
    let commit_bytes = next(9)?;
    let peak_resident = next(10)?;
    let peak_commit = next(11)?;
    let disk_read = next(12)?;
    let disk_written = next(13)?;
    let footprint = next(14)?;
    let idle_wakeups = next(15)?;
    let net_rx_bytes = next(16)?;
    let net_tx_bytes = next(17)?;
    let net_rx_packets = next(18)?;
    let net_tx_packets = next(19)?;
    Some(ProcExtra {
        virtual_bytes,
        faults,
        pageins,
        cow_faults,
        context_switches,
        syscalls,
        priority,
        nice,
        running_threads,
        commit_bytes,
        peak_resident,
        peak_commit,
        disk_read,
        disk_written,
        footprint,
        idle_wakeups,
        net_rx_bytes,
        net_tx_bytes,
        net_rx_packets,
        net_tx_packets,
    })
}

impl ProcRecord {
    pub fn key(&self) -> ProcKey {
        self.meta.key
    }
    pub fn cpu_time(&self) -> Option<u64> {
        (self.cpu_time_ns != CPU_TIME_UNKNOWN).then_some(self.cpu_time_ns)
    }
}

/// The machine-wide figures of one sample.
#[derive(Clone, Debug)]
pub struct SystemSample {
    pub cpu_total: f32,
    pub cores: Box<[f32]>,
    pub mem_total: u64,
    pub mem_used: u64,
    pub mem_available: u64,
    pub mem_cache: u64,
    pub mem_free: u64,
    pub swap_total: u64,
    pub swap_used: u64,
    /// Bytes per second.
    pub net_rx: f32,
    pub net_tx: f32,
    pub net_rx_total: u64,
    pub net_tx_total: u64,
    /// (read, write) bytes per second.
    pub disk: Reading<(f32, f32)>,
    pub disk_read_total: u64,
    pub disk_write_total: u64,
    pub gpu: Reading<f32>,
    pub power: Reading<f32>,
    pub load: [f32; 3],
    pub uptime_secs: u64,
}

/// Everything one tick recorded, immutable once built.
#[derive(Clone, Debug)]
pub struct Sample {
    /// Wall clock, milliseconds since the epoch.
    pub time_ms: u64,
    /// The sampling interval the worker was asked for at the time.
    pub interval_ms: u32,
    /// Wall-clock span since the previous sample the worker took (0 for the
    /// first one after start-up).
    pub span_ms: u32,
    /// The recording session (worker start time, ms): samples of different
    /// sessions are never joined by a line.
    pub session: u64,
    /// The spacing this record was retained at: the interval while live, the
    /// journal cadence once read back from disk.
    pub stride_ms: u32,
    /// How long the backend took to collect this sample (0 when read back
    /// from disk, where it is not recorded).
    pub cost_us: u32,
    pub backend: &'static str,
    pub system: SystemSample,
    /// Sorted by key so a process is found by binary search.
    pub processes: Vec<ProcRecord>,
    /// Every record's packed [`ProcExtra`], addressed by `ProcRecord::extra`.
    pub extras: Box<[u8]>,
}

impl Sample {
    /// Build from a backend snapshot. `processes` are sorted by key here.
    pub fn from_snapshot(snapshot: Snapshot, time_ms: u64, interval_ms: u32, span_ms: u32, session: u64) -> Self {
        let mut extras: Vec<u8> = Vec::with_capacity(snapshot.processes.len() * 16);
        let mut processes: Vec<ProcRecord> = snapshot
            .processes
            .into_iter()
            .take(MAX_PROCESSES_PER_SAMPLE)
            .map(|process| {
                let at = extras.len() as u32;
                pack_extra(&mut extras, &process.extra);
                ProcRecord {
                    meta: process.meta,
                    cpu: process.cpu_pct as f32,
                    rss: process.mem_rss,
                    cpu_time_ns: process.cpu_time_ns.unwrap_or(CPU_TIME_UNKNOWN),
                    threads: process.threads.min(u16::MAX as u32) as u16,
                    state: process.state,
                    extra: at,
                }
            })
            .collect();
        processes.sort_by_key(|record| record.key());
        let disk = match snapshot.disk {
            Reading::Value(disk) => Reading::Value((disk.read_per_second as f32, disk.write_per_second as f32)),
            Reading::Unavailable(reason) => Reading::Unavailable(reason),
        };
        let (disk_read_total, disk_write_total) = match snapshot.disk {
            Reading::Value(disk) => (disk.read_total, disk.write_total),
            Reading::Unavailable(_) => (0, 0),
        };
        Self {
            time_ms,
            interval_ms,
            span_ms,
            session,
            stride_ms: interval_ms.max(100),
            cost_us: 0,
            backend: snapshot.backend,
            system: SystemSample {
                cpu_total: snapshot.cpu_total as f32,
                cores: snapshot.cpu_cores.iter().map(|core| *core as f32).collect(),
                mem_total: snapshot.mem.total,
                mem_used: snapshot.mem.used,
                mem_available: snapshot.mem.available,
                mem_cache: snapshot.mem.cache,
                mem_free: snapshot.mem.free,
                swap_total: snapshot.mem.swap_total,
                swap_used: snapshot.mem.swap_used,
                net_rx: snapshot.net.rx_per_second as f32,
                net_tx: snapshot.net.tx_per_second as f32,
                net_rx_total: snapshot.net.rx_total,
                net_tx_total: snapshot.net.tx_total,
                disk,
                disk_read_total,
                disk_write_total,
                gpu: match snapshot.gpu_pct {
                    Reading::Value(value) => Reading::Value(value as f32),
                    Reading::Unavailable(reason) => Reading::Unavailable(reason),
                },
                power: match snapshot.power_watts {
                    Reading::Value(value) => Reading::Value(value as f32),
                    Reading::Unavailable(reason) => Reading::Unavailable(reason),
                },
                load: [snapshot.load_avg[0] as f32, snapshot.load_avg[1] as f32, snapshot.load_avg[2] as f32],
                uptime_secs: snapshot.uptime_seconds,
            },
            processes,
            extras: extras.into_boxed_slice(),
        }
    }

    /// The extras of `record` (a record of this sample), all `None` when
    /// none were recorded.
    pub fn extra(&self, record: &ProcRecord) -> ProcExtra {
        if record.extra == NO_EXTRA {
            return ProcExtra::default();
        }
        let mut at = record.extra as usize;
        unpack_extra(&self.extras, &mut at).unwrap_or_default()
    }

    /// The packed bytes of `record`'s extras, for the journal.
    pub fn extra_bytes(&self, record: &ProcRecord) -> &[u8] {
        if record.extra == NO_EXTRA {
            return &[];
        }
        let start = record.extra as usize;
        let mut at = start;
        match unpack_extra(&self.extras, &mut at) {
            Some(_) => &self.extras[start..at],
            None => &[],
        }
    }

    /// The record for `key`, by binary search.
    pub fn process(&self, key: ProcKey) -> Option<&ProcRecord> {
        self.processes
            .binary_search_by_key(&key, |record| record.key())
            .ok()
            .map(|index| &self.processes[index])
    }

    /// Approximate resident size of the sample itself (its metadata is
    /// accounted separately, once per shared `Arc`).
    pub fn approx_bytes(&self) -> usize {
        160 + self.system.cores.len() * 4 + self.processes.len() * std::mem::size_of::<ProcRecord>() + self.extras.len()
    }

    /// Memory used as a percentage of the total.
    pub fn mem_pct(&self) -> f32 {
        if self.system.mem_total == 0 {
            0.0
        } else {
            (self.system.mem_used as f64 / self.system.mem_total as f64 * 100.0) as f32
        }
    }
}

/// A stored sample with the spacing it is retained at, for gap detection.
#[derive(Clone, Debug)]
pub struct Entry {
    pub sample: Arc<Sample>,
    /// Expected distance to the neighbouring samples at this tier: the
    /// sampling interval while raw, the bucket width once thinned.
    pub stride_ms: u32,
}

/// The retention tier a sample of this age sits in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    Raw,
    /// One real sample per 10 s.
    Mid,
    /// One real sample per 60 s.
    Coarse,
}

impl Tier {
    pub fn for_age(age_ms: u64) -> Self {
        if age_ms <= FINE_WINDOW_MS {
            Tier::Raw
        } else if age_ms <= MID_WINDOW_MS {
            Tier::Mid
        } else {
            Tier::Coarse
        }
    }

    pub fn bucket_ms(self) -> Option<u64> {
        match self {
            Tier::Raw => None,
            Tier::Mid => Some(MID_BUCKET_MS),
            Tier::Coarse => Some(COARSE_BUCKET_MS),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Tier::Raw => "raw samples",
            Tier::Mid => "1 sample / 10 s",
            Tier::Coarse => "1 sample / 60 s",
        }
    }
}

/// Whether the distance between two neighbouring entries is a gap rather
/// than the recorded spacing: another recording session, the first sample
/// after a start, or more than 2.5× the larger stride plus slack.
pub fn is_gap(previous: &Entry, next: &Entry) -> bool {
    if previous.sample.session != next.sample.session || next.sample.span_ms == 0 {
        return true;
    }
    let stride = previous.stride_ms.max(next.stride_ms).max(100) as u64;
    next.sample.time_ms.saturating_sub(previous.sample.time_ms) > stride * 5 / 2 + 500
}

/// One point of a system series, positioned by its real timestamp.
#[derive(Clone, Copy, Debug)]
pub struct Point {
    pub time_ms: u64,
    pub value: f32,
    /// True when the line must not connect from the previous point.
    pub gap_before: bool,
}

/// One point of a process' own series.
#[derive(Clone, Copy, Debug)]
pub struct ProcPoint {
    pub time_ms: u64,
    pub cpu: f32,
    pub rss: u64,
    pub gap_before: bool,
}

/// The in-memory history. Samples are kept oldest first.
pub struct Store {
    entries: VecDeque<Entry>,
    /// Bytes of the samples themselves plus every distinct metadata `Arc`
    /// they reference, counted once.
    bytes: usize,
    /// Metadata `Arc` address → (samples referencing it, its bytes). The
    /// `Arc`s are alive while any stored sample holds them, so an address
    /// is never reused for another metadata while it is in this map.
    meta_refs: HashMap<usize, (u32, usize)>,
    budget: usize,
    last_thinned_ms: u64,
    /// Samples evicted by the byte budget rather than by age, so the status
    /// can say the window is memory-bound.
    pub budget_evictions: u64,
    /// Bumped on every change to the stored samples: a view caches geometry
    /// against it and rebuilds only when the data changed, not per frame.
    pub generation: u64,
}

impl Default for Store {
    fn default() -> Self {
        Self::new()
    }
}

impl Store {
    pub fn new() -> Self {
        Self {
            entries: VecDeque::new(),
            bytes: 0,
            meta_refs: HashMap::new(),
            budget: 64 * 1024 * 1024,
            last_thinned_ms: 0,
            budget_evictions: 0,
            generation: 0,
        }
    }

    pub fn set_budget_from_ram(&mut self, total_bytes: u64) {
        self.budget = budget_for_ram(total_bytes);
    }

    fn meta_bytes(meta: &ProcMeta) -> usize {
        std::mem::size_of::<ProcMeta>() + 32 + meta.user.len() + meta.name.len() + meta.cmdline.len()
    }

    /// Count a sample in: its own bytes and any metadata not yet referenced.
    fn account_in(&mut self, sample: &Sample) {
        self.bytes += sample.approx_bytes();
        for record in &sample.processes {
            let address = Arc::as_ptr(&record.meta) as usize;
            let entry = self.meta_refs.entry(address).or_insert_with(|| (0, Self::meta_bytes(&record.meta)));
            if entry.0 == 0 {
                self.bytes += entry.1;
            }
            entry.0 += 1;
        }
    }

    /// Count a sample out; metadata no stored sample references is freed.
    fn account_out(&mut self, sample: &Sample) {
        self.bytes = self.bytes.saturating_sub(sample.approx_bytes());
        for record in &sample.processes {
            let address = Arc::as_ptr(&record.meta) as usize;
            if let Some(entry) = self.meta_refs.get_mut(&address) {
                entry.0 = entry.0.saturating_sub(1);
                if entry.0 == 0 {
                    self.bytes = self.bytes.saturating_sub(entry.1);
                    self.meta_refs.remove(&address);
                }
            }
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn latest(&self) -> Option<&Arc<Sample>> {
        self.entries.back().map(|entry| &entry.sample)
    }

    pub fn oldest_ms(&self) -> Option<u64> {
        self.entries.front().map(|entry| entry.sample.time_ms)
    }

    pub fn newest_ms(&self) -> Option<u64> {
        self.entries.back().map(|entry| entry.sample.time_ms)
    }

    /// Append a sample. Out-of-order samples (a late journal batch) are
    /// inserted in place so the time order always holds; a sample whose
    /// timestamp is already present is ignored.
    pub fn push(&mut self, sample: Arc<Sample>) {
        let time = sample.time_ms;
        let at = self.entries.partition_point(|existing| existing.sample.time_ms < time);
        if self.entries.get(at).is_some_and(|existing| existing.sample.time_ms == time) {
            return;
        }
        self.account_in(&sample);
        let entry = Entry { stride_ms: sample.stride_ms.max(100), sample };
        if at == self.entries.len() {
            self.entries.push_back(entry);
        } else {
            self.entries.insert(at, entry);
        }
        self.generation = self.generation.wrapping_add(1);
    }

    /// Samples read back from disk, oldest first. Anything stamped in the
    /// future (a clock that jumped back) is refused.
    pub fn ingest_loaded(&mut self, samples: Vec<Arc<Sample>>, now_ms: u64) {
        for sample in samples {
            if sample.time_ms > now_ms + MINUTE_MS {
                continue;
            }
            self.push(sample);
        }
        self.thin(now_ms, true);
    }

    /// Apply the tiers relative to `now_ms`, the age cap and the byte
    /// budget. Runs at most every few seconds unless `force`d.
    pub fn thin(&mut self, now_ms: u64, force: bool) {
        if !force && now_ms.saturating_sub(self.last_thinned_ms) < 5 * SECOND_MS {
            return;
        }
        self.last_thinned_ms = now_ms;
        self.generation = self.generation.wrapping_add(1);
        // One real sample per bucket in the thinned tiers: the LAST sample
        // of each (session, bucket), so its timestamp and process list are
        // exactly what was recorded and a restart is never merged away.
        let old: Vec<Entry> = self.entries.drain(..).collect();
        let mut kept: VecDeque<Entry> = VecDeque::with_capacity(old.len());
        let mut dropped: Vec<Arc<Sample>> = Vec::new();
        let bucket_of = |entry: &Entry| -> Option<(u64, u64, u64)> {
            let tier = Tier::for_age(now_ms.saturating_sub(entry.sample.time_ms));
            tier.bucket_ms().map(|bucket| (bucket, entry.sample.time_ms / bucket, entry.sample.session))
        };
        let count = old.len();
        for (index, mut entry) in old.into_iter().enumerate() {
            if now_ms.saturating_sub(entry.sample.time_ms) > MAX_AGE_MS && index + 1 < count {
                dropped.push(entry.sample);
                continue;
            }
            let bucket = bucket_of(&entry);
            if let Some((width, _, _)) = bucket {
                entry.stride_ms = entry.stride_ms.max(width as u32);
            }
            if let (Some(bucket), Some(last)) = (bucket, kept.back()) {
                if bucket_of(last) == Some(bucket) {
                    let replaced = kept.pop_back().expect("checked");
                    dropped.push(replaced.sample);
                }
            }
            kept.push_back(entry);
        }
        self.entries = kept;
        for sample in dropped {
            self.account_out(&sample);
        }
        // Finally the byte budget: oldest out first.
        while self.bytes > self.budget && self.entries.len() > 2 {
            if let Some(front) = self.entries.pop_front() {
                self.account_out(&front.sample);
                self.budget_evictions += 1;
            }
        }
    }

    /// Index of the sample nearest to `time_ms`.
    pub fn index_at(&self, time_ms: u64) -> Option<usize> {
        if self.entries.is_empty() {
            return None;
        }
        let after = self.entries.partition_point(|entry| entry.sample.time_ms < time_ms);
        if after == 0 {
            return Some(0);
        }
        if after >= self.entries.len() {
            return Some(self.entries.len() - 1);
        }
        let before = after - 1;
        let d_before = time_ms - self.entries[before].sample.time_ms;
        let d_after = self.entries[after].sample.time_ms - time_ms;
        Some(if d_before <= d_after { before } else { after })
    }

    pub fn entry(&self, index: usize) -> Option<&Entry> {
        self.entries.get(index)
    }

    pub fn at(&self, time_ms: u64) -> Option<&Arc<Sample>> {
        self.index_at(time_ms).map(|index| &self.entries[index].sample)
    }

    /// Entries with `from_ms <= time <= to_ms`, plus one on each side so
    /// lines reach the edges, as an index range.
    pub fn range_indices(&self, from_ms: u64, to_ms: u64) -> std::ops::Range<usize> {
        let start = self.entries.partition_point(|entry| entry.sample.time_ms < from_ms);
        let end = self.entries.partition_point(|entry| entry.sample.time_ms <= to_ms);
        let start = start.saturating_sub(1);
        let end = (end + 1).min(self.entries.len());
        start..end.max(start)
    }

    /// A system series over the window: `pick` reads the value from each
    /// sample, `None` leaves a hole.
    pub fn series(&self, from_ms: u64, to_ms: u64, pick: impl Fn(&Sample) -> Option<f32>) -> Vec<Point> {
        let range = self.range_indices(from_ms, to_ms);
        let mut points = Vec::with_capacity(range.len());
        let mut previous: Option<&Entry> = None;
        for index in range {
            let entry = &self.entries[index];
            match pick(&entry.sample) {
                Some(value) => {
                    let gap_before = previous.is_none_or(|p| is_gap(p, entry));
                    points.push(Point { time_ms: entry.sample.time_ms, value, gap_before });
                    previous = Some(entry);
                }
                None => previous = None,
            }
        }
        points
    }

    /// A process' own cpu/rss over the window; a sample without the process
    /// is a hole.
    pub fn process_series(&self, key: ProcKey, from_ms: u64, to_ms: u64) -> Vec<ProcPoint> {
        let range = self.range_indices(from_ms, to_ms);
        let mut points = Vec::with_capacity(range.len());
        let mut previous: Option<&Entry> = None;
        for index in range {
            let entry = &self.entries[index];
            match entry.sample.process(key) {
                Some(record) => {
                    let gap_before = previous.is_none_or(|p| is_gap(p, entry));
                    points.push(ProcPoint { time_ms: entry.sample.time_ms, cpu: record.cpu, rss: record.rss, gap_before });
                    previous = Some(entry);
                }
                None => previous = None,
            }
        }
        points
    }

    /// The newest retained sample that has `key`, looking back from
    /// `time_ms`: a process that exited, or was not in the view sample,
    /// still has the history it had.
    pub fn latest_with(&self, key: ProcKey, time_ms: u64) -> Option<&Arc<Sample>> {
        let end = self.entries.partition_point(|entry| entry.sample.time_ms <= time_ms);
        self.entries.range(..end).rev().map(|entry| &entry.sample).find(|sample| sample.process(key).is_some())
    }

    /// Readings of one basic-sample `measure` of `key` over the window; a
    /// sample without the process or the figure is a hole.
    pub fn readings(&self, key: ProcKey, measure: crate::metrics::Measure, from_ms: u64, to_ms: u64) -> Vec<crate::metrics::Reading> {
        let range = self.range_indices(from_ms, to_ms);
        let mut out = Vec::with_capacity(range.len());
        let mut previous: Option<&Entry> = None;
        for index in range {
            let entry = &self.entries[index];
            match entry.sample.process(key).and_then(|record| crate::metrics::basic_value(&entry.sample, record, measure)) {
                Some(value) => {
                    let gap_before = previous.is_none_or(|p| is_gap(p, entry));
                    out.push(crate::metrics::Reading { time_ms: entry.sample.time_ms, value, gap_before });
                    previous = Some(entry);
                }
                None => previous = None,
            }
        }
        out
    }

    /// The basic-sample reading of `measure` for `key` in the sample nearest
    /// `time_ms`, with the previous sample's when the two join (for a rate).
    pub fn reading_pair(&self, key: ProcKey, measure: crate::metrics::Measure, time_ms: u64) -> Option<(crate::metrics::Reading, Option<crate::metrics::Reading>)> {
        use crate::metrics::{basic_value, Reading};
        let index = self.index_at(time_ms)?;
        let entry = &self.entries[index];
        let value = entry.sample.process(key).and_then(|record| basic_value(&entry.sample, record, measure))?;
        let now = Reading { time_ms: entry.sample.time_ms, value, gap_before: false };
        let before = index
            .checked_sub(1)
            .map(|i| &self.entries[i])
            .filter(|previous| !is_gap(previous, entry))
            .and_then(|previous| {
                let value = previous.sample.process(key).and_then(|record| basic_value(&previous.sample, record, measure))?;
                Some(Reading { time_ms: previous.sample.time_ms, value, gap_before: false })
            });
        Some((now, before))
    }

    /// The retained tiers actually present, for the status line.
    pub fn retained_ms(&self) -> u64 {
        match (self.oldest_ms(), self.newest_ms()) {
            (Some(oldest), Some(newest)) => newest.saturating_sub(oldest),
            _ => 0,
        }
    }
}

/// The time bucket a trace of `span_ms` drawn across `px` pixels folds its
/// samples into, or 0 to draw every sample. Chosen from a fixed ladder so it
/// only changes with the range or the plot's width, never as the window
/// slides.
pub fn fold_bucket_ms(span_ms: f64, px: f64) -> u64 {
    const LADDER: [u64; 16] = [
        100, 200, 250, 500, 1_000, 2_000, 5_000, 10_000, 15_000, 30_000, 60_000, 120_000, 300_000, 600_000, 1_800_000, 3_600_000,
    ];
    // Fold only below about 1.5 px per sample.
    let raw = span_ms / (px / 1.5).max(1.0);
    if raw < 100.0 {
        return 0;
    }
    LADDER.iter().copied().find(|bucket| *bucket as f64 >= raw).unwrap_or(3_600_000)
}

/// Fold `points` into world-anchored time buckets (`time / bucket_ms`), one
/// point per bucket: the bucket's highest value at that sample's own
/// timestamp, so peaks survive and keep their real time. Because the buckets
/// are anchored to absolute time, not to the window, old samples fold the
/// same way however the window moves; only the newest bucket can change, as
/// samples arrive in it.
///
/// Nothing is interpolated. A recorded gap (another session, a stall, a
/// process absent from a sample) starts a new point with `gap_before` even
/// inside one bucket, so the line still breaks there.
pub fn fold_by_time(points: &[Point], bucket_ms: u64) -> Vec<Point> {
    if bucket_ms <= 1 {
        return points.to_vec();
    }
    let mut out: Vec<Point> = Vec::with_capacity(points.len().min(4096));
    let mut current: Option<(u64, Point)> = None;
    for point in points {
        let id = point.time_ms / bucket_ms;
        match current.as_mut() {
            Some((bucket, best)) if *bucket == id && !point.gap_before => {
                if point.value > best.value {
                    let gap_before = best.gap_before;
                    *best = Point { gap_before, ..*point };
                }
            }
            _ => {
                if let Some((_, done)) = current.take() {
                    out.push(done);
                }
                current = Some((id, *point));
            }
        }
    }
    if let Some((_, done)) = current {
        out.push(done);
    }
    out
}

/// Bounded ring of the detail blocks collected for inspected processes, so a
/// scrub within the recorded window shows what was recorded *then* rather
/// than a live lookup on an old identity.
pub struct DetailRing {
    items: VecDeque<Arc<crate::backend::ProcDetail>>,
    bytes: usize,
}

/// Detail is kept this long and this large.
const DETAIL_WINDOW_MS: u64 = FINE_WINDOW_MS;
const DETAIL_BUDGET_BYTES: usize = 8 * 1024 * 1024;

impl Default for DetailRing {
    fn default() -> Self {
        Self::new()
    }
}

impl DetailRing {
    pub fn new() -> Self {
        Self { items: VecDeque::new(), bytes: 0 }
    }

    fn approx_bytes(detail: &crate::backend::ProcDetail) -> usize {
        use crate::backend::Detail;
        let mut bytes = 256;
        if let Detail::Ready(threads) = &detail.threads {
            bytes += threads.len() * 96;
        }
        if let Detail::Ready(files) = &detail.files {
            bytes += files.iter().map(|f| 48 + f.path.len()).sum::<usize>();
        }
        if let Detail::Ready(ports) = &detail.ports {
            bytes += ports.iter().map(|p| 64 + p.local.len() + p.remote.len()).sum::<usize>();
        }
        if let Some(Detail::Ready(libraries)) = &detail.libraries {
            bytes += libraries.iter().map(|l| 32 + l.path.len()).sum::<usize>();
        }
        bytes
    }

    pub fn push(&mut self, detail: Arc<crate::backend::ProcDetail>, now_ms: u64) {
        self.bytes += Self::approx_bytes(&detail);
        self.items.push_back(detail);
        while let Some(front) = self.items.front() {
            let too_old = now_ms.saturating_sub(front.time_ms) > DETAIL_WINDOW_MS;
            if (too_old || self.bytes > DETAIL_BUDGET_BYTES) && self.items.len() > 1 {
                self.bytes = self.bytes.saturating_sub(Self::approx_bytes(front));
                self.items.pop_front();
            } else {
                break;
            }
        }
    }

    /// The newest detail for `key`.
    pub fn latest(&self, key: ProcKey) -> Option<&Arc<crate::backend::ProcDetail>> {
        self.items.iter().rev().find(|detail| detail.key == key)
    }

    /// The detail for `key` collected at or before `time_ms` and no more
    /// than `max_age_ms` before it: what was true then, never a later read.
    pub fn at_or_before(&self, key: ProcKey, time_ms: u64, max_age_ms: u64) -> Option<&Arc<crate::backend::ProcDetail>> {
        self.items
            .iter()
            .rev()
            .filter(|detail| detail.key == key && detail.time_ms <= time_ms)
            .find(|detail| time_ms - detail.time_ms <= max_age_ms)
    }

    /// The library list for `key` collected at or before `time_ms`, within
    /// `max_age_ms` (libraries are walked less often than the rest).
    pub fn libraries_at_or_before(&self, key: ProcKey, time_ms: u64, max_age_ms: u64) -> Option<&Arc<crate::backend::ProcDetail>> {
        self.items
            .iter()
            .rev()
            .filter(|detail| detail.key == key && detail.time_ms <= time_ms && detail.libraries.is_some())
            .find(|detail| time_ms - detail.time_ms <= max_age_ms)
    }
}
