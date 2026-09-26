//! The history journal on disk, owned by the sampler worker. The UI thread
//! never touches a file.
//!
//! Layout, under the user's application-data directory
//! (`~/Library/Application Support/Makepad/task`, `$XDG_DATA_HOME/makepad/task`,
//! `%APPDATA%\Makepad\task`):
//!
//! ```text
//! history/lock                    pid of the instance that owns the journal
//! history/seg-<first_ms>-<tier>.bin   samples, oldest segment first
//! history/pins.txt                pinned process identities
//! history/columns.txt             the process table's columns, order and widths
//! ```
//!
//! A segment is a header (`MPTH`, version, tier) and a run of chunks
//! `[tag u8][len u32][fnv1a32 u32][payload]`: a `Meta` chunk per process
//! incarnation the segment mentions, then `Sample` chunks whose rows refer to
//! metas by index. Reading stops at the first chunk that is short, has a bad
//! hash or an unknown tag: what came before it is used, what comes after is
//! dropped. Every count and string on the way in is bounded.
//!
//! Version 3 adds each process row's packed extras (the figures the basic
//! sample reads beside CPU and memory, see `history::pack_extra`) as a
//! length byte and the bytes. Version 2 segments are still read — their rows
//! simply have no extras — and are rewritten as the current version when
//! compacted. Version 4 widens the extras' presence mask with disk bytes,
//! footprint, idle wake-ups and network traffic; a version 3 mask is a
//! subset of it, so version 3 rows read unchanged.
//!
//! Writes are capped at one sample a second whatever the UI interval; a
//! segment closes after ten minutes. Closed segments older than the raw
//! window are rewritten to one sample per 10 s, older than an hour to one
//! per 60 s, mirroring the in-memory tiers; segments older than 24 h are
//! removed, as are the oldest when the directory passes 96 MiB.
//!
//! Only one running instance writes: it holds an exclusive OS advisory lock
//! (`File::try_lock`) on `history/lock` for its whole life; a second
//! instance fails to take it and records in memory only. The lock dies with
//! the process, so a crash never leaves the journal owned.
//!
//! `--history-dir <path>` or `TASK_HISTORY_DIR=<path>` puts the journal
//! somewhere else, for isolated restart and corruption runs.

use crate::backend::{ProcKey, ProcMeta, ProcState, Reading, MAX_CMDLINE_LEN, MAX_NAME_LEN};
use crate::history::{unpack_extra, ProcRecord, Sample, SystemSample, Tier, FINE_WINDOW_MS, MAX_AGE_MS, MAX_PROCESSES_PER_SAMPLE, MID_WINDOW_MS, MINUTE_MS, NO_EXTRA, SECOND_MS};
use makepad_widgets::log;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const MAGIC: &[u8; 4] = b"MPTH";
/// Written; [`OLDEST_VERSION`]..=`VERSION` are read.
const VERSION: u32 = 4;
const OLDEST_VERSION: u32 = 2;
const TAG_META: u8 = 1;
const TAG_SAMPLE: u8 = 2;
/// A chunk longer than this is corruption, whatever its hash says.
const MAX_CHUNK_BYTES: usize = 8 * 1024 * 1024;
const MAX_CORES: usize = 4096;
/// Segment rollover.
const SEGMENT_SPAN_MS: u64 = 10 * MINUTE_MS;
const SEGMENT_MAX_BYTES: u64 = 24 * 1024 * 1024;
/// Journal write cadence cap.
const WRITE_EVERY_MS: u64 = SECOND_MS;
/// Directory cap.
const MAX_DIR_BYTES: u64 = 96 * 1024 * 1024;
/// Samples per `Loaded` batch handed to the UI.
pub const LOAD_BATCH: usize = 256;
/// Total samples loaded from disk, whatever is on it.
const MAX_LOADED_SAMPLES: usize = 20_000;
/// A segment file bigger than this is not ours; it is skipped unread.
const MAX_SEGMENT_FILE_BYTES: u64 = SEGMENT_MAX_BYTES + 8 * 1024 * 1024;
/// Segment files and bytes one restore reads at most.
const MAX_RESTORE_FILES: usize = 200;
const MAX_RESTORE_READ_BYTES: u64 = 256 * 1024 * 1024;
/// Metadata chunks one segment may declare.
const MAX_METAS_PER_SEGMENT: usize = 200_000;

/// The journal directory: `--history-dir`, then `TASK_HISTORY_DIR`, then
/// `<data dir>/history`.
pub fn history_dir() -> Option<PathBuf> {
    let mut args = std::env::args();
    while let Some(arg) = args.next() {
        if let Some(value) = arg.strip_prefix("--history-dir=") {
            return Some(PathBuf::from(value));
        }
        if arg == "--history-dir" {
            return args.next().map(PathBuf::from);
        }
    }
    if let Some(dir) = std::env::var_os("TASK_HISTORY_DIR").filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    data_dir().map(|dir| dir.join("history"))
}

/// Where task keeps its files for this user, or `None` when no home
/// directory is known.
pub fn data_dir() -> Option<PathBuf> {
    makepad_widgets::makepad_platform::home::app_data_dir("task")
}

fn fnv1a32(bytes: &[u8]) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in bytes {
        hash ^= *byte as u32;
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

// ---- encoding ----

struct Encoder {
    bytes: Vec<u8>,
}

impl Encoder {
    fn new() -> Self {
        Self { bytes: Vec::with_capacity(4096) }
    }
    fn u8(&mut self, value: u8) {
        self.bytes.push(value);
    }
    fn u16(&mut self, value: u16) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }
    fn u32(&mut self, value: u32) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }
    fn u64(&mut self, value: u64) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }
    fn f32(&mut self, value: f32) {
        self.bytes.extend_from_slice(&value.to_le_bytes());
    }
    fn str(&mut self, text: &str, max: usize) {
        let mut cut = text.len().min(max);
        while cut > 0 && !text.is_char_boundary(cut) {
            cut -= 1;
        }
        self.u16(cut as u16);
        self.bytes.extend_from_slice(&text.as_bytes()[..cut]);
    }
    fn reading_f32(&mut self, reading: Reading<f32>) {
        match reading {
            Reading::Value(value) => {
                self.u8(1);
                self.f32(value);
            }
            Reading::Unavailable(_) => {
                self.u8(0);
                self.f32(0.0);
            }
        }
    }
}

struct Decoder<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Decoder<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, at: 0 }
    }
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.at.checked_add(n)?;
        if end > self.bytes.len() {
            return None;
        }
        let slice = &self.bytes[self.at..end];
        self.at = end;
        Some(slice)
    }
    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|b| b[0])
    }
    fn u16(&mut self) -> Option<u16> {
        self.take(2).map(|b| u16::from_le_bytes([b[0], b[1]]))
    }
    fn u32(&mut self) -> Option<u32> {
        self.take(4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn u64(&mut self) -> Option<u64> {
        self.take(8).map(|b| {
            let mut a = [0u8; 8];
            a.copy_from_slice(b);
            u64::from_le_bytes(a)
        })
    }
    fn f32(&mut self) -> Option<f32> {
        self.u32().map(f32::from_bits).filter(|v| v.is_finite())
    }
    fn str(&mut self, max: usize) -> Option<String> {
        let len = self.u16()? as usize;
        if len > max {
            return None;
        }
        let bytes = self.take(len)?;
        Some(String::from_utf8_lossy(bytes).into_owned())
    }
    fn reading_f32(&mut self) -> Option<Reading<f32>> {
        let flag = self.u8()?;
        let value = self.f32()?;
        Some(if flag == 1 { Reading::Value(value) } else { Reading::Unavailable("not recorded in this sample") })
    }
}

fn encode_meta(meta: &ProcMeta) -> Vec<u8> {
    let mut e = Encoder::new();
    e.u32(meta.key.pid);
    e.u64(meta.key.start);
    e.u32(meta.ppid);
    e.u64(meta.started_secs);
    e.u8(meta.is_app as u8);
    e.str(&meta.user, MAX_NAME_LEN);
    e.str(&meta.name, MAX_NAME_LEN);
    e.str(&meta.cmdline, MAX_CMDLINE_LEN);
    e.bytes
}

fn decode_meta(bytes: &[u8]) -> Option<ProcMeta> {
    let mut d = Decoder::new(bytes);
    Some(ProcMeta {
        key: ProcKey { pid: d.u32()?, start: d.u64()? },
        ppid: d.u32()?,
        started_secs: d.u64()?,
        is_app: d.u8()? == 1,
        user: d.str(MAX_NAME_LEN)?,
        name: d.str(MAX_NAME_LEN)?,
        cmdline: d.str(MAX_CMDLINE_LEN)?,
    })
}

fn encode_sample(sample: &Sample, meta_index: &HashMap<usize, (u32, Arc<ProcMeta>)>) -> Vec<u8> {
    let mut e = Encoder::new();
    let s = &sample.system;
    e.u64(sample.time_ms);
    e.u32(sample.interval_ms);
    e.u32(sample.span_ms);
    e.u64(sample.session);
    // The spacing this record will have when read back: the journal keeps at
    // most one sample per WRITE_EVERY_MS, whatever the live interval was.
    e.u32(sample.stride_ms.max(WRITE_EVERY_MS as u32));
    e.f32(s.cpu_total);
    e.u16(s.cores.len().min(MAX_CORES) as u16);
    for core in s.cores.iter().take(MAX_CORES) {
        e.f32(*core);
    }
    for value in [s.mem_total, s.mem_used, s.mem_available, s.mem_cache, s.mem_free, s.swap_total, s.swap_used] {
        e.u64(value);
    }
    e.f32(s.net_rx);
    e.f32(s.net_tx);
    e.u64(s.net_rx_total);
    e.u64(s.net_tx_total);
    match s.disk {
        Reading::Value((read, write)) => {
            e.u8(1);
            e.f32(read);
            e.f32(write);
        }
        Reading::Unavailable(_) => {
            e.u8(0);
            e.f32(0.0);
            e.f32(0.0);
        }
    }
    e.u64(s.disk_read_total);
    e.u64(s.disk_write_total);
    e.reading_f32(s.gpu);
    e.reading_f32(s.power);
    for value in s.load {
        e.f32(value);
    }
    e.u64(s.uptime_secs);
    let index_of = |record: &ProcRecord| meta_index.get(&(Arc::as_ptr(&record.meta) as usize)).map(|(index, _)| *index);
    let rows: Vec<(u32, &ProcRecord)> = sample.processes.iter().filter_map(|r| index_of(r).map(|i| (i, r))).collect();
    e.u32(rows.len() as u32);
    for (index, record) in rows {
        e.u32(index);
        e.f32(record.cpu);
        e.u64(record.rss);
        e.u64(record.cpu_time_ns);
        e.u16(record.threads);
        e.u8(record.state.to_u8());
        let extra = sample.extra_bytes(record);
        let len = if extra.len() <= u8::MAX as usize { extra.len() } else { 0 };
        e.u8(len as u8);
        e.bytes.extend_from_slice(&extra[..len]);
    }
    e.bytes
}

fn decode_sample(bytes: &[u8], metas: &[Arc<ProcMeta>], version: u32) -> Option<Sample> {
    let mut d = Decoder::new(bytes);
    let time_ms = d.u64()?;
    let interval_ms = d.u32()?;
    let span_ms = d.u32()?;
    let session = d.u64()?;
    let stride_ms = d.u32()?.clamp(100, 3_600_000);
    let cpu_total = d.f32()?;
    let core_count = d.u16()? as usize;
    if core_count > MAX_CORES {
        return None;
    }
    let mut cores = Vec::with_capacity(core_count);
    for _ in 0..core_count {
        cores.push(d.f32()?);
    }
    let mut mem = [0u64; 7];
    for slot in mem.iter_mut() {
        *slot = d.u64()?;
    }
    let net_rx = d.f32()?;
    let net_tx = d.f32()?;
    let net_rx_total = d.u64()?;
    let net_tx_total = d.u64()?;
    let disk_flag = d.u8()?;
    let disk_read = d.f32()?;
    let disk_write = d.f32()?;
    let disk_read_total = d.u64()?;
    let disk_write_total = d.u64()?;
    let gpu = d.reading_f32()?;
    let power = d.reading_f32()?;
    let load = [d.f32()?, d.f32()?, d.f32()?];
    let uptime_secs = d.u64()?;
    let count = d.u32()? as usize;
    if count > MAX_PROCESSES_PER_SAMPLE {
        return None;
    }
    let mut processes = Vec::with_capacity(count);
    let mut extras: Vec<u8> = Vec::new();
    for _ in 0..count {
        let index = d.u32()? as usize;
        let cpu = d.f32()?;
        let rss = d.u64()?;
        let cpu_time_ns = d.u64()?;
        let threads = d.u16()?;
        let state = ProcState::from_u8(d.u8()?);
        let mut extra = NO_EXTRA;
        if version >= 3 {
            let len = d.u8()? as usize;
            let packed = d.take(len)?;
            if len > 0 {
                // Checked whole before it is kept: a bad record is corruption.
                let mut at = 0;
                unpack_extra(packed, &mut at)?;
                if at != len {
                    return None;
                }
                extra = extras.len() as u32;
                extras.extend_from_slice(packed);
            }
        }
        let meta = metas.get(index)?.clone();
        processes.push(ProcRecord { meta, cpu, rss, cpu_time_ns, threads, state, extra });
    }
    processes.sort_by_key(|record| record.key());
    Some(Sample {
        time_ms,
        interval_ms,
        span_ms,
        session,
        stride_ms,
        cost_us: 0,
        backend: "journal",
        system: SystemSample {
            cpu_total,
            cores: cores.into_boxed_slice(),
            mem_total: mem[0],
            mem_used: mem[1],
            mem_available: mem[2],
            mem_cache: mem[3],
            mem_free: mem[4],
            swap_total: mem[5],
            swap_used: mem[6],
            net_rx,
            net_tx,
            net_rx_total,
            net_tx_total,
            disk: if disk_flag == 1 { Reading::Value((disk_read, disk_write)) } else { Reading::Unavailable("not recorded in this sample") },
            disk_read_total,
            disk_write_total,
            gpu,
            power,
            load,
            uptime_secs,
        },
        processes,
        extras: extras.into_boxed_slice(),
    })
}

// ---- segments ----

fn tier_name(tier: Tier) -> &'static str {
    match tier {
        Tier::Raw => "raw",
        Tier::Mid => "mid",
        Tier::Coarse => "coarse",
    }
}

fn tier_from_name(name: &str) -> Option<Tier> {
    match name {
        "raw" => Some(Tier::Raw),
        "mid" => Some(Tier::Mid),
        "coarse" => Some(Tier::Coarse),
        _ => None,
    }
}

#[derive(Clone, Debug)]
struct SegmentInfo {
    path: PathBuf,
    first_ms: u64,
    tier: Tier,
    bytes: u64,
}

fn segment_path(dir: &Path, first_ms: u64, tier: Tier) -> PathBuf {
    dir.join(format!("seg-{first_ms:015}-{}.bin", tier_name(tier)))
}

fn parse_segment_name(path: &Path) -> Option<(u64, Tier)> {
    let name = path.file_name()?.to_str()?;
    let rest = name.strip_prefix("seg-")?.strip_suffix(".bin")?;
    let (first, tier) = rest.split_once('-')?;
    Some((first.parse().ok()?, tier_from_name(tier)?))
}

fn list_segments(dir: &Path) -> Vec<SegmentInfo> {
    let mut segments: Vec<SegmentInfo> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let (first_ms, tier) = parse_segment_name(&path)?;
            let bytes = entry.metadata().ok()?.len();
            Some(SegmentInfo { path, first_ms, tier, bytes })
        })
        .collect();
    segments.sort_by_key(|segment| segment.first_ms);
    segments
}

/// An open segment being appended to.
struct SegmentWriter {
    path: PathBuf,
    file: BufWriter<File>,
    first_ms: u64,
    bytes: u64,
    /// Metadata `Arc` address → (index in this segment, the Arc). Holding the
    /// `Arc` keeps the address from being reused while the segment is open,
    /// and keying by the `Arc` rather than the process key journals every
    /// metadata version (exec, reparenting) of one incarnation.
    metas: HashMap<usize, (u32, Arc<ProcMeta>)>,
}

impl SegmentWriter {
    fn create(dir: &Path, first_ms: u64, tier: Tier) -> std::io::Result<Self> {
        Self::create_at(segment_path(dir, first_ms, tier), first_ms, tier)
    }

    fn create_at(path: PathBuf, first_ms: u64, tier: Tier) -> std::io::Result<Self> {
        let mut file = BufWriter::new(File::create(&path)?);
        file.write_all(MAGIC)?;
        file.write_all(&VERSION.to_le_bytes())?;
        file.write_all(&[match tier { Tier::Raw => 0, Tier::Mid => 1, Tier::Coarse => 2 }])?;
        Ok(Self { path, file, first_ms, bytes: 9, metas: HashMap::new() })
    }

    fn chunk(&mut self, tag: u8, payload: &[u8]) -> std::io::Result<()> {
        self.file.write_all(&[tag])?;
        self.file.write_all(&(payload.len() as u32).to_le_bytes())?;
        self.file.write_all(&fnv1a32(payload).to_le_bytes())?;
        self.file.write_all(payload)?;
        self.bytes += 9 + payload.len() as u64;
        Ok(())
    }

    fn write_sample(&mut self, sample: &Sample) -> std::io::Result<()> {
        for record in &sample.processes {
            let address = Arc::as_ptr(&record.meta) as usize;
            if !self.metas.contains_key(&address) {
                let index = self.metas.len() as u32;
                self.chunk(TAG_META, &encode_meta(&record.meta))?;
                self.metas.insert(address, (index, record.meta.clone()));
            }
        }
        let payload = encode_sample(sample, &self.metas);
        self.chunk(TAG_SAMPLE, &payload)
    }

    fn finish(mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

/// What reading one segment produced.
pub struct Loaded {
    pub samples: Vec<Arc<Sample>>,
    /// True when the file ended early or a chunk was corrupt.
    pub truncated: bool,
    /// True when reading stopped at the metadata budget.
    pub budget_hit: bool,
}

/// Metadata read back from disk, shared across segments: the same
/// incarnation mentioned by twenty segments is one `Arc`, not twenty. Its
/// bytes count against the restore budget AS they are interned, so a journal
/// full of metadata and few samples cannot bypass the budget.
pub struct MetaInterner {
    set: std::collections::HashSet<Arc<ProcMeta>>,
    pub bytes: usize,
    limit: usize,
}

impl MetaInterner {
    pub fn new(limit: usize) -> Self {
        Self { set: std::collections::HashSet::new(), bytes: 0, limit }
    }

    /// The shared copy of `meta`, or `None` once the byte limit is reached.
    fn intern(&mut self, meta: ProcMeta) -> Option<Arc<ProcMeta>> {
        if let Some(existing) = self.set.get(&meta) {
            return Some(existing.clone());
        }
        let size = std::mem::size_of::<ProcMeta>() + 32 + meta.name.len() + meta.cmdline.len() + meta.user.len();
        if self.bytes + size > self.limit {
            return None;
        }
        self.bytes += size;
        let shared = Arc::new(meta);
        self.set.insert(shared.clone());
        Some(shared)
    }
}

/// Read one segment, bounded in file size, metadata count and samples;
/// stops at the first bad chunk.
fn read_segment(path: &Path, max_samples: usize, intern: &mut MetaInterner) -> Loaded {
    let mut loaded = Loaded { samples: Vec::new(), truncated: false, budget_hit: false };
    let Ok(file) = File::open(path) else { return loaded };
    if file.metadata().map(|m| m.len() > MAX_SEGMENT_FILE_BYTES).unwrap_or(true) {
        loaded.truncated = true;
        return loaded;
    }
    let mut bytes = Vec::new();
    if file.take(MAX_SEGMENT_FILE_BYTES).read_to_end(&mut bytes).is_err() {
        loaded.truncated = true;
        return loaded;
    }
    let version = if bytes.len() >= 9 { u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) } else { 0 };
    if bytes.len() < 9 || &bytes[..4] != MAGIC || !(OLDEST_VERSION..=VERSION).contains(&version) {
        loaded.truncated = !bytes.is_empty();
        return loaded;
    }
    let mut metas: Vec<Arc<ProcMeta>> = Vec::new();
    let mut at = 9usize;
    while at + 9 <= bytes.len() {
        let tag = bytes[at];
        let len = u32::from_le_bytes([bytes[at + 1], bytes[at + 2], bytes[at + 3], bytes[at + 4]]) as usize;
        let hash = u32::from_le_bytes([bytes[at + 5], bytes[at + 6], bytes[at + 7], bytes[at + 8]]);
        let start = at + 9;
        let Some(end) = start.checked_add(len) else {
            loaded.truncated = true;
            break;
        };
        if len > MAX_CHUNK_BYTES || end > bytes.len() {
            loaded.truncated = true;
            break;
        }
        let payload = &bytes[start..end];
        if fnv1a32(payload) != hash {
            loaded.truncated = true;
            break;
        }
        match tag {
            TAG_META => match decode_meta(payload) {
                Some(meta) if metas.len() < MAX_METAS_PER_SEGMENT => match intern.intern(meta) {
                    Some(shared) => metas.push(shared),
                    None => {
                        loaded.budget_hit = true;
                        break;
                    }
                },
                _ => {
                    loaded.truncated = true;
                    break;
                }
            },
            TAG_SAMPLE => match decode_sample(payload, &metas, version) {
                Some(sample) => {
                    loaded.samples.push(Arc::new(sample));
                    if loaded.samples.len() >= max_samples {
                        break;
                    }
                }
                None => {
                    loaded.truncated = true;
                    break;
                }
            },
            _ => {
                loaded.truncated = true;
                break;
            }
        }
        at = end;
    }
    if at != bytes.len() && !loaded.truncated && loaded.samples.len() < max_samples {
        loaded.truncated = true;
    }
    loaded
}

/// Keep the last sample of each (session, `bucket_ms` bucket).
fn thin_samples(samples: Vec<Arc<Sample>>, bucket_ms: u64) -> Vec<Arc<Sample>> {
    let mut kept: Vec<Arc<Sample>> = Vec::with_capacity(samples.len() / 8 + 1);
    for sample in samples {
        if let Some(last) = kept.last() {
            // Two recording sessions never share a bucket: the restart
            // between them must survive compaction as a gap.
            if last.session == sample.session && last.time_ms / bucket_ms == sample.time_ms / bucket_ms {
                *kept.last_mut().expect("non-empty") = sample;
                continue;
            }
        }
        kept.push(sample);
    }
    kept
}

/// Write `samples` as a whole segment of `tier`, replacing `old`.
fn rewrite_segment(dir: &Path, old: &Path, samples: &[Arc<Sample>], tier: Tier) -> std::io::Result<()> {
    let Some(first) = samples.first() else {
        return fs::remove_file(old);
    };
    let temp = dir.join(format!("seg-{:015}-{}.tmp", first.time_ms, tier_name(tier)));
    {
        // Through a temp file, so a crash mid-rewrite leaves the old segment intact.
        let mut writer = SegmentWriter::create_at(temp.clone(), first.time_ms, tier)?;
        for sample in samples {
            writer.write_sample(sample)?;
        }
        writer.finish()?;
    }
    let final_path = segment_path(dir, first.time_ms, tier);
    fs::rename(&temp, &final_path)?;
    if old != final_path {
        let _ = fs::remove_file(old);
    }
    Ok(())
}

// ---- the journal ----

/// A pinned process as remembered across restarts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PinRecord {
    pub key: ProcKey,
    pub name: String,
}

pub struct Journal {
    dir: PathBuf,
    /// Holds the exclusive advisory lock for as long as the journal lives.
    lock: File,
    current: Option<SegmentWriter>,
    last_written_ms: u64,
    last_maintained_ms: u64,
    /// Set once a write fails, so one full disk does not log every second.
    write_failed: bool,
}

/// Why the journal could not be opened.
#[derive(Debug)]
pub enum OpenError {
    NoDataDir,
    Locked,
    Io(std::io::Error),
}

impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpenError::NoDataDir => write!(f, "no user data directory"),
            OpenError::Locked => write!(f, "another task instance owns the journal"),
            OpenError::Io(error) => write!(f, "{error}"),
        }
    }
}

impl Journal {
    /// Take ownership of the journal directory, or say why not.
    pub fn open() -> Result<Self, OpenError> {
        let dir = history_dir().ok_or(OpenError::NoDataDir)?;
        fs::create_dir_all(&dir).map_err(OpenError::Io)?;
        let lock = fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(dir.join("lock"))
            .map_err(OpenError::Io)?;
        match lock.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => return Err(OpenError::Locked),
            Err(std::fs::TryLockError::Error(error)) => return Err(OpenError::Io(error)),
        }
        Ok(Self { dir, lock, current: None, last_written_ms: 0, last_maintained_ms: 0, write_failed: false })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// What is on disk and still within the retention window, newest first
    /// until `budget_bytes` (the in-memory history budget) is reached, then
    /// handed back oldest first in batches of [`LOAD_BATCH`]. Future-stamped
    /// samples (a clock that went back) are skipped. Also reports what was
    /// recovered from truncated files.
    pub fn load(&self, now_ms: u64, budget_bytes: usize) -> (Vec<Vec<Arc<Sample>>>, String) {
        let oldest_wanted = now_ms.saturating_sub(MAX_AGE_MS);
        let mut segments = list_segments(&self.dir);
        segments.retain(|segment| segment.first_ms + SEGMENT_SPAN_MS * 2 >= oldest_wanted && segment.first_ms <= now_ms + MINUTE_MS);
        let mut intern = MetaInterner::new(budget_bytes);
        let mut newest_first: Vec<Arc<Sample>> = Vec::new();
        let mut bytes = 0usize;
        let mut truncated = 0usize;
        let mut files = 0usize;
        let mut read_bytes = 0u64;
        let mut budgeted = false;
        'segments: for segment in segments.iter().rev() {
            if files >= MAX_RESTORE_FILES || read_bytes + segment.bytes > MAX_RESTORE_READ_BYTES {
                budgeted = true;
                break;
            }
            read_bytes += segment.bytes;
            let loaded = read_segment(&segment.path, MAX_LOADED_SAMPLES, &mut intern);
            files += 1;
            if loaded.truncated {
                truncated += 1;
            }
            budgeted |= loaded.budget_hit;
            for sample in loaded.samples.into_iter().rev() {
                if sample.time_ms < oldest_wanted || sample.time_ms > now_ms + MINUTE_MS {
                    continue;
                }
                bytes += sample.approx_bytes();
                if bytes + intern.bytes > budget_bytes || newest_first.len() >= MAX_LOADED_SAMPLES {
                    budgeted = true;
                    break 'segments;
                }
                newest_first.push(sample);
            }
            if loaded.budget_hit {
                break;
            }
        }
        let meta_bytes = intern.bytes;
        drop(intern);
        newest_first.reverse();
        let mut all = newest_first;
        all.sort_by_key(|sample| sample.time_ms);
        all.dedup_by_key(|sample| sample.time_ms);
        let count = all.len();
        let batches: Vec<Vec<Arc<Sample>>> = all.chunks(LOAD_BATCH).map(|chunk| chunk.to_vec()).collect();
        let mut recovered = if truncated > 0 { format!(", {truncated} truncated and recovered") } else { String::new() };
        if budgeted {
            recovered.push_str(", stopped at the memory budget; newest kept");
        }
        let status = format!(
            "history: loaded {count} samples from {files} files ({} KiB samples + {} KiB metadata{recovered})",
            bytes.min(budget_bytes) / 1024,
            meta_bytes / 1024
        );
        (batches, status)
    }

    /// Record a sample, at most once a second.
    pub fn append(&mut self, sample: &Sample) {
        if sample.time_ms.saturating_sub(self.last_written_ms) < WRITE_EVERY_MS {
            return;
        }
        if let Some(current) = &self.current {
            if sample.time_ms.saturating_sub(current.first_ms) >= SEGMENT_SPAN_MS || current.bytes >= SEGMENT_MAX_BYTES {
                if let Some(writer) = self.current.take() {
                    if let Err(error) = writer.finish() {
                        self.report_write_error(error);
                    }
                }
            }
        }
        if self.current.is_none() {
            match SegmentWriter::create(&self.dir, sample.time_ms, Tier::Raw) {
                Ok(writer) => self.current = Some(writer),
                Err(error) => {
                    self.report_write_error(error);
                    return;
                }
            }
        }
        if let Some(writer) = &mut self.current {
            match writer.write_sample(sample) {
                Ok(()) => self.last_written_ms = sample.time_ms,
                Err(error) => {
                    self.report_write_error(error);
                    self.current = None;
                }
            }
        }
    }

    fn report_write_error(&mut self, error: std::io::Error) {
        if !self.write_failed {
            self.write_failed = true;
            log!("task: history write failed: {error}");
        }
    }

    /// Flush the open segment so a crash loses at most the buffered tail.
    pub fn flush(&mut self) {
        if let Some(writer) = &mut self.current {
            let _ = writer.file.flush();
        }
    }

    /// Compact and cap the closed segments. Runs at most once a minute;
    /// returns whether it ran.
    pub fn maintain(&mut self, now_ms: u64) -> bool {
        if now_ms.saturating_sub(self.last_maintained_ms) < MINUTE_MS {
            return false;
        }
        self.last_maintained_ms = now_ms;
        let open_path = self.current.as_ref().map(|writer| writer.path.clone());
        let mut segments = list_segments(&self.dir);
        // Age cap.
        segments.retain(|segment| {
            let too_old = now_ms.saturating_sub(segment.first_ms) > MAX_AGE_MS + SEGMENT_SPAN_MS;
            if too_old && Some(&segment.path) != open_path.as_ref() {
                let _ = fs::remove_file(&segment.path);
                false
            } else {
                true
            }
        });
        // Tier rewrites.
        for segment in &segments {
            if Some(&segment.path) == open_path.as_ref() {
                continue;
            }
            let age = now_ms.saturating_sub(segment.first_ms + SEGMENT_SPAN_MS);
            let wanted = if age > MID_WINDOW_MS { Tier::Coarse } else if age > FINE_WINDOW_MS { Tier::Mid } else { Tier::Raw };
            let needs = matches!((segment.tier, wanted), (Tier::Raw, Tier::Mid | Tier::Coarse) | (Tier::Mid, Tier::Coarse));
            if !needs {
                continue;
            }
            let loaded = read_segment(&segment.path, MAX_LOADED_SAMPLES, &mut MetaInterner::new(usize::MAX));
            let bucket = wanted.bucket_ms().unwrap_or(SECOND_MS);
            let thinned = thin_samples(loaded.samples, bucket);
            if let Err(error) = rewrite_segment(&self.dir, &segment.path, &thinned, wanted) {
                log!("task: history compaction failed for {}: {error}", segment.path.display());
            }
        }
        // Size cap: oldest closed segments first.
        let mut segments = list_segments(&self.dir);
        let mut total: u64 = segments.iter().map(|segment| segment.bytes).sum();
        while total > MAX_DIR_BYTES && !segments.is_empty() {
            let oldest = segments.remove(0);
            if Some(&oldest.path) == open_path.as_ref() {
                break;
            }
            let _ = fs::remove_file(&oldest.path);
            total = total.saturating_sub(oldest.bytes);
        }
        true
    }

    /// Total bytes on disk right now.
    pub fn disk_bytes(&self) -> u64 {
        // The open segment is listed too (with its flushed size).
        list_segments(&self.dir).iter().map(|segment| segment.bytes).sum::<u64>()
    }

    pub fn load_pins(&self) -> Vec<PinRecord> {
        let Ok(text) = fs::read_to_string(self.dir.join("pins.txt")) else { return Vec::new() };
        text.lines()
            .take(256)
            .filter_map(|line| {
                let mut fields = line.splitn(3, '\t');
                let pid = fields.next()?.parse().ok()?;
                let start = fields.next()?.parse().ok()?;
                let name = fields.next().unwrap_or("").to_string();
                Some(PinRecord { key: ProcKey { pid, start }, name })
            })
            .collect()
    }

    /// The table's saved columns, as `GridColumns::serialize` wrote them.
    pub fn load_columns(&self) -> Option<String> {
        let path = self.dir.join("columns.txt");
        // A layout is a few hundred bytes; anything big is not ours.
        (fs::metadata(&path).ok()?.len() <= 64 * 1024).then(|| fs::read_to_string(&path).ok()).flatten()
    }

    pub fn save_columns(&self, text: &str) {
        if let Err(error) = fs::write(self.dir.join("columns.txt"), text) {
            log!("task: could not save columns: {error}");
        }
    }

    pub fn save_pins(&self, pins: &[PinRecord]) {
        let mut text = String::new();
        for pin in pins.iter().take(256) {
            text.push_str(&format!("{}\t{}\t{}\n", pin.key.pid, pin.key.start, pin.name.replace(['\t', '\n'], " ")));
        }
        if let Err(error) = fs::write(self.dir.join("pins.txt"), text) {
            log!("task: could not save pins: {error}");
        }
    }

    /// Close the open segment and release the lock.
    pub fn close(mut self) {
        if let Some(writer) = self.current.take() {
            let _ = writer.finish();
        }
        let _ = self.lock.unlock();
    }
}

/// Human-readable span for status lines: `3h 12m`, `48s`.
pub fn format_span_ms(ms: u64) -> String {
    let secs = ms / 1000;
    if secs >= 3600 {
        format!("{}h {:02}m", secs / 3600, secs % 3600 / 60)
    } else if secs >= 60 {
        format!("{}m {:02}s", secs / 60, secs % 60)
    } else {
        format!("{secs}s")
    }
}


// ---- the supplemental sidecar ----
//
// `history/supp-<first_ms>.bin`: the supplemental events (see `supp.rs`) as
// they were recorded, one `Events` chunk per worker tick, in the same chunk
// framing as the sample segments (tag, length, FNV-1a, payload; reading
// stops at the first bad chunk). Paths and thread names are written once per
// segment into a string table (`Str` chunks) and referred to by index, so a
// descriptor list is never repeated per observation: only opens, closes and
// a small observation mark are. Written by the journal's owner only, under
// the same lock. Segments roll over every ten minutes or 8 MiB; the oldest
// go past 24 h or when the sidecar passes 48 MiB. Not thinned on disk.

use crate::metrics::Measure;
use crate::supp::{CloseReason, FileClose, FileObs, FileOpen, Opened, ProcObs, SuppEvent, ThreadObs, ThreadPoint};
use crate::backend::ThreadState;

const SUPP_MAGIC: &[u8; 4] = b"MPTS";
const SUPP_VERSION: u32 = 1;
const TAG_STR: u8 = 1;
const TAG_EVENTS: u8 = 2;
/// The descriptor records still open when the segment began (repeated
/// `Open`s), so a segment never needs an older one.
const TAG_CHECKPOINT: u8 = 3;
const SUPP_SEGMENT_SPAN_MS: u64 = 10 * MINUTE_MS;
const SUPP_SEGMENT_MAX_BYTES: u64 = 8 * 1024 * 1024;
pub const SUPP_MAX_DIR_BYTES: u64 = 48 * 1024 * 1024;
const SUPP_MAX_STRINGS: usize = 200_000;
const SUPP_MAX_EVENTS_PER_CHUNK: usize = 1_000_000;
const SUPP_MAX_ITEMS: usize = 65_535;
/// Events are written in chunks of about this many (estimated) bytes, well
/// under the reader's per-chunk limit.
const SUPP_CHUNK_TARGET: usize = 2 * 1024 * 1024;
/// A sidecar file larger than this is not read (a segment rolls over at
/// 8 MiB; one tick's events and a checkpoint can pass that, bounded).
const SUPP_READ_MAX_BYTES: u64 = 32 * 1024 * 1024;

fn thread_state_code(state: ThreadState) -> u8 {
    match state {
        ThreadState::Running => 1,
        ThreadState::Stopped => 2,
        ThreadState::Waiting => 3,
        ThreadState::Uninterruptible => 4,
        ThreadState::Halted => 5,
        ThreadState::Unknown => 0,
    }
}

fn thread_state_from(code: u8) -> ThreadState {
    match code {
        1 => ThreadState::Running,
        2 => ThreadState::Stopped,
        3 => ThreadState::Waiting,
        4 => ThreadState::Uninterruptible,
        5 => ThreadState::Halted,
        _ => ThreadState::Unknown,
    }
}

impl Encoder {
    fn key(&mut self, key: ProcKey) {
        self.u32(key.pid);
        self.u64(key.start);
    }
    fn varint(&mut self, value: u64) {
        crate::history::put_varint(&mut self.bytes, value);
    }
}

impl<'a> Decoder<'a> {
    fn key(&mut self) -> Option<ProcKey> {
        Some(ProcKey { pid: self.u32()?, start: self.u64()? })
    }
    fn varint(&mut self) -> Option<u64> {
        crate::history::get_varint(self.bytes, &mut self.at)
    }
    fn i32(&mut self) -> Option<i32> {
        self.u32().map(|v| v as i32)
    }
}

/// Encode one tick's events; `string` interns a path or name into the
/// segment's table, returning its index.
fn encode_events(events: &[SuppEvent], mut string: impl FnMut(&Arc<str>) -> u32) -> Vec<u8> {
    let mut e = Encoder::new();
    e.u32(events.len() as u32);
    for event in events {
        match event {
            SuppEvent::Proc(o) => {
                e.u8(1);
                e.key(o.key);
                e.u64(o.session);
                e.u64(o.time_ms);
                e.u8(o.gap_before as u8);
                let values = &o.values[..o.values.len().min(255)];
                e.u8(values.len() as u8);
                for (measure, value) in values {
                    e.u8(measure.code());
                    e.varint(crate::history::zigzag(*value));
                }
            }
            SuppEvent::Threads(o) => {
                e.u8(2);
                e.key(o.key);
                e.u64(o.session);
                e.u64(o.time_ms);
                e.u8(o.gap_before as u8 | (o.complete as u8) << 1);
                let threads = &o.threads[..o.threads.len().min(SUPP_MAX_ITEMS)];
                e.u16(threads.len() as u16);
                for t in threads {
                    e.u64(t.id);
                    let name = string(&t.name);
                    e.u32(name);
                    e.u8(thread_state_code(t.state) | (t.reset as u8) << 6 | (t.gap_before as u8) << 7);
                    match t.cpu_pct {
                        Some(p) => {
                            e.u8(1);
                            e.f32(p);
                        }
                        None => e.u8(0),
                    }
                    // 0 = unknown, else ns + 1.
                    e.varint(t.cpu_time_ns.map(|ns| ns.saturating_add(1)).unwrap_or(0));
                }
                let ended = &o.ended[..o.ended.len().min(SUPP_MAX_ITEMS)];
                e.u16(ended.len() as u16);
                for id in ended {
                    e.u64(*id);
                }
                e.varint(o.process_cpu_ns.map(|ns| ns.saturating_add(1)).unwrap_or(0));
                match o.process_pct {
                    Some(p) => {
                        e.u8(1);
                        e.f32(p);
                    }
                    None => e.u8(0),
                }
                e.varint(o.span_ms.map(|ms| ms as u64 + 1).unwrap_or(0));
            }
            SuppEvent::Open(o) => {
                e.u8(3);
                e.key(o.key);
                e.u64(o.id.0);
                e.u32(o.id.1);
                e.u32(o.fd as u32);
                e.u8(o.kind);
                let path = string(&o.path);
                e.u32(path);
                e.u64(o.time_ms);
                match o.opened {
                    Opened::AlreadyOpen => {
                        e.u8(0);
                        e.u64(0);
                    }
                    Opened::After(then) => {
                        e.u8(1);
                        e.u64(then);
                    }
                }
            }
            SuppEvent::Close(o) => {
                e.u8(4);
                e.key(o.key);
                e.u64(o.id.0);
                e.u32(o.id.1);
                e.u64(o.last_seen_ms);
                e.u64(o.gone_ms);
                e.u8(o.reason.code());
            }
            SuppEvent::Files(o) => {
                e.u8(5);
                e.key(o.key);
                e.u64(o.session);
                e.u64(o.time_ms);
                e.u8(o.gap_before as u8 | (o.complete as u8) << 1);
                e.u32(o.unreadable);
                let seen = &o.seen[..o.seen.len().min(SUPP_MAX_ITEMS)];
                e.u16(seen.len() as u16);
                for (start, len) in seen {
                    e.varint(*start as u64);
                    e.varint(*len as u64);
                }
            }
            SuppEvent::Stopped { key, session, time_ms, exited } => {
                e.u8(6);
                e.key(*key);
                e.u64(*session);
                e.u64(*time_ms);
                e.u8(*exited as u8);
            }
        }
    }
    e.bytes
}

fn decode_events(bytes: &[u8], strings: &[Arc<str>]) -> Option<Vec<SuppEvent>> {
    let mut d = Decoder::new(bytes);
    let count = d.u32()? as usize;
    if count > SUPP_MAX_EVENTS_PER_CHUNK {
        return None;
    }
    let string = |index: u32| strings.get(index as usize).cloned();
    let mut events = Vec::with_capacity(count.min(4096));
    for _ in 0..count {
        let event = match d.u8()? {
            1 => {
                let key = d.key()?;
                let session = d.u64()?;
                let time_ms = d.u64()?;
                let gap_before = d.u8()? & 1 != 0;
                let n = d.u8()? as usize;
                let mut values = Vec::with_capacity(n);
                for _ in 0..n {
                    let code = d.u8()?;
                    let value = crate::history::unzigzag(d.varint()?);
                    // A code this build does not know is skipped, not fatal.
                    if let Some(measure) = Measure::from_code(code) {
                        values.push((measure, value));
                    }
                }
                SuppEvent::Proc(ProcObs { key, session, time_ms, gap_before, values })
            }
            2 => {
                let key = d.key()?;
                let session = d.u64()?;
                let time_ms = d.u64()?;
                let flags = d.u8()?;
                let n = d.u16()? as usize;
                let mut threads = Vec::with_capacity(n);
                for _ in 0..n {
                    let id = d.u64()?;
                    let name = string(d.u32()?)?;
                    let state = d.u8()?;
                    let cpu_pct = if d.u8()? == 1 { Some(d.f32()?) } else { None };
                    let cpu_time = d.varint()?;
                    threads.push(ThreadPoint {
                        id,
                        name,
                        state: thread_state_from(state & 0x3f),
                        cpu_pct,
                        cpu_time_ns: cpu_time.checked_sub(1),
                        gap_before: state & 0x80 != 0,
                        reset: state & 0x40 != 0,
                    });
                }
                let n = d.u16()? as usize;
                let mut ended = Vec::with_capacity(n);
                for _ in 0..n {
                    ended.push(d.u64()?);
                }
                let process_cpu_ns = d.varint()?.checked_sub(1);
                let process_pct = if d.u8()? == 1 { Some(d.f32()?) } else { None };
                let span_ms = d.varint()?.checked_sub(1).map(|ms| ms.min(u32::MAX as u64) as u32);
                SuppEvent::Threads(ThreadObs { key, session, time_ms, gap_before: flags & 1 != 0, complete: flags & 2 != 0, threads, ended, process_cpu_ns, process_pct, span_ms })
            }
            3 => {
                let key = d.key()?;
                let id = (d.u64()?, d.u32()?);
                let fd = d.i32()?;
                let kind = d.u8()?;
                let path = string(d.u32()?)?;
                let time_ms = d.u64()?;
                let opened = match (d.u8()?, d.u64()?) {
                    (0, _) => Opened::AlreadyOpen,
                    (1, then) => Opened::After(then),
                    _ => return None,
                };
                SuppEvent::Open(FileOpen { key, id, fd, kind, path, time_ms, opened })
            }
            4 => {
                let key = d.key()?;
                let id = (d.u64()?, d.u32()?);
                let last_seen_ms = d.u64()?;
                let gone_ms = d.u64()?;
                let reason = CloseReason::from_code(d.u8()?)?;
                SuppEvent::Close(FileClose { key, id, last_seen_ms, gone_ms, reason })
            }
            5 => {
                let key = d.key()?;
                let session = d.u64()?;
                let time_ms = d.u64()?;
                let flags = d.u8()?;
                let unreadable = d.u32()?;
                let n = d.u16()? as usize;
                let mut seen = Vec::with_capacity(n);
                for _ in 0..n {
                    let start = d.varint()?.min(u32::MAX as u64) as u32;
                    let len = d.varint()?.min(u32::MAX as u64) as u32;
                    seen.push((start, len));
                }
                SuppEvent::Files(FileObs { key, session, time_ms, gap_before: flags & 1 != 0, complete: flags & 2 != 0, unreadable, seen })
            }
            6 => {
                let key = d.key()?;
                let session = d.u64()?;
                let time_ms = d.u64()?;
                let exited = d.u8()? == 1;
                SuppEvent::Stopped { key, session, time_ms, exited }
            }
            _ => return None,
        };
        events.push(event);
    }
    Some(events)
}

struct SuppWriter {
    path: PathBuf,
    file: BufWriter<File>,
    first_ms: u64,
    bytes: u64,
    strings: HashMap<Arc<str>, u32>,
}

impl SuppWriter {
    fn create(dir: &Path, first_ms: u64) -> std::io::Result<Self> {
        let path = dir.join(format!("supp-{first_ms:015}.bin"));
        let mut file = BufWriter::new(File::create(&path)?);
        file.write_all(SUPP_MAGIC)?;
        file.write_all(&SUPP_VERSION.to_le_bytes())?;
        Ok(Self { path, file, first_ms, bytes: 8, strings: HashMap::new() })
    }

    fn chunk(&mut self, tag: u8, payload: &[u8]) -> std::io::Result<()> {
        self.file.write_all(&[tag])?;
        self.file.write_all(&(payload.len() as u32).to_le_bytes())?;
        self.file.write_all(&fnv1a32(payload).to_le_bytes())?;
        self.file.write_all(payload)?;
        self.bytes += 9 + payload.len() as u64;
        Ok(())
    }

    fn write(&mut self, events: &[SuppEvent]) -> std::io::Result<()> {
        self.write_tagged(TAG_EVENTS, events)
    }

    /// `events` as one or more chunks of at most about
    /// [`SUPP_CHUNK_TARGET`] bytes each, every one self-contained after the
    /// string chunks it needs.
    fn write_tagged(&mut self, tag: u8, events: &[SuppEvent]) -> std::io::Result<()> {
        let mut start = 0;
        while start < events.len() {
            let mut end = start;
            let mut bytes = 0;
            while end < events.len() && (end == start || bytes + events[end].approx_bytes() <= SUPP_CHUNK_TARGET) {
                bytes += events[end].approx_bytes();
                end += 1;
            }
            self.write_chunk(tag, &events[start..end])?;
            start = end;
        }
        Ok(())
    }

    fn write_chunk(&mut self, tag: u8, events: &[SuppEvent]) -> std::io::Result<()> {
        // New strings first, each its own chunk, so the events chunk only
        // refers to strings already on disk before it.
        let mut fresh: Vec<Arc<str>> = Vec::new();
        let mut next = self.strings.len() as u32;
        let payload = encode_events(events, |text| {
            if let Some(index) = self.strings.get(text) {
                return *index;
            }
            let index = next;
            next += 1;
            self.strings.insert(text.clone(), index);
            fresh.push(text.clone());
            index
        });
        for (offset, text) in fresh.iter().enumerate() {
            let mut e = Encoder::new();
            e.u32(next - fresh.len() as u32 + offset as u32);
            e.str(text, crate::supp::MAX_PATH_LEN * 4);
            self.chunk(TAG_STR, &e.bytes)?;
        }
        self.chunk(tag, &payload)
    }
}

/// One sidecar file read back: its checkpoint, its event batches oldest
/// first with their decoded sizes, the total, and whether it was cut short.
struct SuppSegment {
    checkpoint: Vec<SuppEvent>,
    batches: Vec<(Vec<SuppEvent>, usize)>,
    bytes: usize,
    truncated: bool,
}

fn read_supp_segment(path: &Path) -> SuppSegment {
    let (batches, bytes, truncated, checkpoint) = read_supp_chunks(path);
    SuppSegment { checkpoint, batches, bytes, truncated }
}

fn read_supp_chunks(path: &Path) -> (Vec<(Vec<SuppEvent>, usize)>, usize, bool, Vec<SuppEvent>) {
    let mut checkpoint = Vec::new();
    let mut batches = Vec::new();
    let mut bytes_read = 0usize;
    let Ok(file) = File::open(path) else { return (batches, 0, false, checkpoint) };
    if file.metadata().map(|m| m.len() > SUPP_READ_MAX_BYTES).unwrap_or(true) {
        return (batches, 0, true, checkpoint);
    }
    let mut bytes = Vec::new();
    if file.take(SUPP_READ_MAX_BYTES).read_to_end(&mut bytes).is_err() {
        return (batches, 0, true, checkpoint);
    }
    if bytes.len() < 8 || &bytes[..4] != SUPP_MAGIC || u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) != SUPP_VERSION {
        return (batches, 0, !bytes.is_empty(), checkpoint);
    }
    let mut strings: Vec<Arc<str>> = Vec::new();
    let mut at = 8usize;
    let mut truncated = false;
    while at + 9 <= bytes.len() {
        let tag = bytes[at];
        let len = u32::from_le_bytes([bytes[at + 1], bytes[at + 2], bytes[at + 3], bytes[at + 4]]) as usize;
        let hash = u32::from_le_bytes([bytes[at + 5], bytes[at + 6], bytes[at + 7], bytes[at + 8]]);
        let start = at + 9;
        let Some(end) = start.checked_add(len) else {
            truncated = true;
            break;
        };
        if len > MAX_CHUNK_BYTES || end > bytes.len() || fnv1a32(&bytes[start..end]) != hash {
            truncated = true;
            break;
        }
        let payload = &bytes[start..end];
        match tag {
            TAG_STR => {
                let mut d = Decoder::new(payload);
                let (Some(index), Some(text)) = (d.u32(), d.str(crate::supp::MAX_PATH_LEN * 4)) else {
                    truncated = true;
                    break;
                };
                if index as usize != strings.len() || strings.len() >= SUPP_MAX_STRINGS {
                    truncated = true;
                    break;
                }
                bytes_read += 32 + text.len();
                strings.push(Arc::from(text));
            }
            TAG_EVENTS | TAG_CHECKPOINT => match decode_events(payload, &strings) {
                Some(events) => {
                    let size: usize = events.iter().map(|e| e.approx_bytes()).sum();
                    bytes_read += size;
                    if tag == TAG_CHECKPOINT {
                        checkpoint.extend(events);
                    } else {
                        batches.push((events, size));
                    }
                }
                None => {
                    truncated = true;
                    break;
                }
            },
            _ => {
                truncated = true;
                break;
            }
        }
        at = end;
    }
    if at != bytes.len() {
        truncated = true;
    }
    (batches, bytes_read, truncated, checkpoint)
}

fn list_supp_segments(dir: &Path) -> Vec<(PathBuf, u64, u64)> {
    let mut segments: Vec<(PathBuf, u64, u64)> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            let name = path.file_name()?.to_str()?;
            let first: u64 = name.strip_prefix("supp-")?.strip_suffix(".bin")?.parse().ok()?;
            let bytes = entry.metadata().ok()?.len();
            Some((path, first, bytes))
        })
        .collect();
    segments.sort_by_key(|(_, first, _)| *first);
    segments
}

/// The supplemental sidecar, owned by the journal's owner.
pub struct SuppJournal {
    dir: PathBuf,
    current: Option<SuppWriter>,
    last_maintained_ms: u64,
    write_failed: bool,
}

impl Journal {
    /// The sidecar in this journal's directory, under the same lock.
    pub fn supp(&self) -> SuppJournal {
        SuppJournal { dir: self.dir.clone(), current: None, last_maintained_ms: 0, write_failed: false }
    }
}

impl SuppJournal {
    /// At the start of a worker tick: roll the segment over when it is due,
    /// beginning the next one with a checkpoint of what is still open.
    pub fn begin_tick(&mut self, time_ms: u64, checkpoint: impl FnOnce() -> Vec<SuppEvent>) {
        let due = self.current.as_ref().is_some_and(|current| time_ms.saturating_sub(current.first_ms) >= SUPP_SEGMENT_SPAN_MS || current.bytes >= SUPP_SEGMENT_MAX_BYTES);
        if due {
            if let Some(mut writer) = self.current.take() {
                let _ = writer.file.flush();
            }
            self.open(time_ms, checkpoint());
        }
    }

    fn open(&mut self, time_ms: u64, checkpoint: Vec<SuppEvent>) {
        match SuppWriter::create(&self.dir, time_ms) {
            Ok(mut writer) => {
                if !checkpoint.is_empty() {
                    if let Err(error) = writer.write_tagged(TAG_CHECKPOINT, &checkpoint) {
                        self.report(error);
                        return;
                    }
                }
                self.current = Some(writer);
            }
            Err(error) => self.report(error),
        }
    }

    /// Write one tick's events. A first segment (or one after a failed
    /// write) starts with `checkpoint`.
    pub fn append(&mut self, events: &[SuppEvent], time_ms: u64, checkpoint: impl FnOnce() -> Vec<SuppEvent>) {
        if events.is_empty() {
            return;
        }
        if self.current.is_none() {
            self.open(time_ms, checkpoint());
            if self.current.is_none() {
                return;
            }
        }
        if let Some(writer) = &mut self.current {
            if let Err(error) = writer.write(events) {
                self.report(error);
                self.current = None;
            }
        }
    }

    fn report(&mut self, error: std::io::Error) {
        if !self.write_failed {
            self.write_failed = true;
            log!("task: supplemental history write failed: {error}");
        }
    }

    pub fn flush(&mut self) {
        if let Some(writer) = &mut self.current {
            let _ = writer.file.flush();
        }
    }

    /// What is on disk within 24 h, newest segments first until
    /// `budget_bytes` of decoded events, handed back oldest first.
    pub fn load(&self, now_ms: u64, budget_bytes: usize) -> (Vec<Vec<SuppEvent>>, String) {
        let oldest_wanted = now_ms.saturating_sub(MAX_AGE_MS);
        let mut segments = list_supp_segments(&self.dir);
        segments.retain(|(_, first, _)| first + SUPP_SEGMENT_SPAN_MS * 2 >= oldest_wanted && *first <= now_ms + MINUTE_MS);
        let mut newest_first: Vec<Vec<Vec<SuppEvent>>> = Vec::new();
        let (mut bytes, mut files, mut truncated, mut read) = (0usize, 0usize, 0usize, 0u64);
        let mut budgeted = false;
        for (path, _, size) in segments.iter().rev() {
            if files >= MAX_RESTORE_FILES * 2 || read + size > MAX_RESTORE_READ_BYTES {
                budgeted = true;
                break;
            }
            read += size;
            let segment = read_supp_segment(path);
            files += 1;
            truncated += segment.truncated as usize;
            let checkpoint_bytes: usize = segment.checkpoint.iter().map(|e| e.approx_bytes()).sum();
            if bytes + segment.bytes > budget_bytes {
                // Over budget: the newest segment still gives its checkpoint
                // and its newest batches that fit; older segments stop here.
                if newest_first.is_empty() {
                    let mut room = budget_bytes.saturating_sub(checkpoint_bytes);
                    let mut tail = Vec::new();
                    for (batch, size) in segment.batches.into_iter().rev() {
                        if size > room {
                            break;
                        }
                        room -= size;
                        tail.push(batch);
                    }
                    tail.reverse();
                    let mut kept = vec![segment.checkpoint];
                    kept.extend(tail);
                    bytes = budget_bytes - room;
                    newest_first.push(kept);
                }
                budgeted = true;
                break;
            }
            bytes += segment.bytes;
            let mut kept = vec![segment.checkpoint];
            kept.extend(segment.batches.into_iter().map(|(batch, _)| batch));
            newest_first.push(kept);
        }
        newest_first.reverse();
        let batches: Vec<Vec<SuppEvent>> = newest_first.into_iter().flatten().collect();
        let mut note = String::new();
        if truncated > 0 {
            note.push_str(&format!(", {truncated} truncated and recovered"));
        }
        if budgeted {
            note.push_str(", stopped at the memory budget; newest kept");
        }
        let status = format!("supplemental history: {} batches from {files} files ({} KiB{note})", batches.len(), bytes / 1024);
        (batches, status)
    }

    /// Age and size caps; at most once a minute.
    pub fn maintain(&mut self, now_ms: u64) {
        if now_ms.saturating_sub(self.last_maintained_ms) < MINUTE_MS {
            return;
        }
        self.last_maintained_ms = now_ms;
        let open = self.current.as_ref().map(|w| w.path.clone());
        let mut segments = list_supp_segments(&self.dir);
        segments.retain(|(path, first, _)| {
            let too_old = now_ms.saturating_sub(*first) > MAX_AGE_MS + SUPP_SEGMENT_SPAN_MS;
            if too_old && Some(path) != open.as_ref() {
                let _ = fs::remove_file(path);
                false
            } else {
                true
            }
        });
        let mut total: u64 = segments.iter().map(|(_, _, bytes)| bytes).sum();
        while total > SUPP_MAX_DIR_BYTES && !segments.is_empty() {
            let (path, _, bytes) = segments.remove(0);
            if Some(&path) == open.as_ref() {
                break;
            }
            let _ = fs::remove_file(&path);
            total = total.saturating_sub(bytes);
        }
    }

    pub fn close(mut self) {
        if let Some(mut writer) = self.current.take() {
            let _ = writer.file.flush();
        }
    }
}
