//! Supplemental history: what task records about a process beyond the
//! basic per-tick sample, for the processes it looks at closely.
//!
//! # Scope
//!
//! The basic sample (CPU, memory, the `ProcExtra` counters) is kept for
//! every process. Richer reads — the memory ledger, storage bytes, thread
//! CPU, the descriptor list — cost a system call or a walk per process, so
//! they are collected only for the **inspected** process (at the inspector's
//! cadence) and for **pinned** processes (on a slower background cadence).
//! Whatever such a read returns is recorded here; nothing is read because a
//! graph is drawn or the cursor moved. Unpinning stops the extra reads; what
//! was recorded stays.
//!
//! # Honesty rules
//!
//! * Every observation carries the wall-clock time it was taken, and a
//!   `gap_before` when the one before it is further than its cadence allows
//!   (a stall, a deferred read, a restart): lines break there.
//! * Nothing is backfilled: a series starts at its first observation.
//! * Descriptor times are *observation* times, not kernel event times. A
//!   descriptor present at the first observation was "already open"; one
//!   absent from the previous complete observation was opened *between* the
//!   two. A close is only recorded when a complete list no longer has it,
//!   when the descriptor now names something else, or when the process
//!   verifiably exited — never from a refused, partial or cut-off read. Each
//!   close keeps the last time it was seen open and the first time it was
//!   seen gone.
//! * A thread missing from a complete list has ended; a thread whose CPU
//!   time went down is a new incarnation of the id, and its line breaks.
//!
//! The worker's [`Recorder`] turns reads into [`SuppEvent`]s; the same events
//! go to the journal's sidecar and to the UI's [`SuppStore`].

use crate::backend::{bounded, Detail, FileInfo, ProcDetail, ProcKey, ThreadInfo, ThreadState, Want};
use crate::history::{FINE_WINDOW_MS, MAX_AGE_MS, MID_BUCKET_MS, MID_WINDOW_MS, COARSE_BUCKET_MS, MINUTE_MS, SECOND_MS};
use crate::metrics::{Measure, Reading};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// A descriptor record's identity: the recording session and a counter.
pub type FileId = (u64, u32);

/// Longest path or socket description kept.
pub const MAX_PATH_LEN: usize = 1024;
pub const MAX_THREAD_NAME_LEN: usize = 128;
/// Threads recorded per observation; busier threads first beyond this.
pub const MAX_THREADS_RECORDED: usize = 512;
/// Descriptor records kept per process in memory (oldest closed go first).
pub const MAX_FILE_RECORDS: usize = 20_000;
/// Thread tracks kept per process (least recently seen go first).
pub const MAX_THREAD_TRACKS: usize = 2_048;
/// The most a descriptor checkpoint carries, so it fits one bounded UI
/// message and the sidecar's chunk and file limits.
pub const CHECKPOINT_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct ProcObs {
    pub key: ProcKey,
    pub session: u64,
    pub time_ms: u64,
    pub gap_before: bool,
    pub values: Vec<(Measure, i64)>,
}

#[derive(Clone, Debug)]
pub struct ThreadPoint {
    pub id: u64,
    pub name: Arc<str>,
    pub state: ThreadState,
    /// Percent of one core over this observation's interval (see
    /// [`ThreadObs::span_ms`]): the thread's CPU-time delta over the
    /// monotonic time between this read and the previous one. `None` for a
    /// new thread, a reset or after a gap — unknown, never 0.
    pub cpu_pct: Option<f32>,
    pub cpu_time_ns: Option<u64>,
    pub gap_before: bool,
    /// Its CPU time went down since the last observation: a new thread on a
    /// reused id, never a continuation of the old one.
    pub reset: bool,
}

#[derive(Clone, Debug)]
pub struct ThreadObs {
    pub key: ProcKey,
    pub session: u64,
    pub time_ms: u64,
    pub gap_before: bool,
    /// The list was whole, so the `ended` ids really ended.
    pub complete: bool,
    pub threads: Vec<ThreadPoint>,
    pub ended: Vec<u64>,
    /// The process' own cumulative CPU time, read in the same detail read as
    /// this thread list: the process side of a per-interval comparison over
    /// the same two observations.
    pub process_cpu_ns: Option<u64>,
    /// The process' CPU over the same interval as the threads' `cpu_pct`.
    pub process_pct: Option<f32>,
    /// That interval, monotonic milliseconds; `None` after a gap.
    pub span_ms: Option<u32>,
}

/// When a descriptor record began, as far as observation can tell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Opened {
    /// Open at the first observation: opened at some earlier time.
    AlreadyOpen,
    /// Not open at this earlier observation (ms): opened after it.
    After(u64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseReason {
    /// A complete descriptor list no longer had it.
    Absent,
    /// The descriptor number now names another file or endpoint.
    Replaced,
    /// The process exited.
    Exited,
}

impl CloseReason {
    pub fn code(self) -> u8 {
        match self {
            CloseReason::Absent => 1,
            CloseReason::Replaced => 2,
            CloseReason::Exited => 3,
        }
    }

    pub fn from_code(code: u8) -> Option<Self> {
        match code {
            1 => Some(CloseReason::Absent),
            2 => Some(CloseReason::Replaced),
            3 => Some(CloseReason::Exited),
            _ => None,
        }
    }
}

/// A descriptor record began — or, with the same id, a checkpoint repeating
/// a still-open record at the start of a sidecar segment or after the UI
/// missed events (the first copy wins).
#[derive(Clone, Debug)]
pub struct FileOpen {
    pub key: ProcKey,
    pub id: FileId,
    pub fd: i32,
    pub kind: u8,
    pub path: Arc<str>,
    pub time_ms: u64,
    pub opened: Opened,
}

#[derive(Clone, Debug)]
pub struct FileClose {
    pub key: ProcKey,
    pub id: FileId,
    pub last_seen_ms: u64,
    pub gone_ms: u64,
    pub reason: CloseReason,
}

#[derive(Clone, Debug)]
pub struct FileObs {
    pub key: ProcKey,
    pub session: u64,
    pub time_ms: u64,
    /// Descriptors were not observed between the previous observation and
    /// this one: an unknown interval.
    pub gap_before: bool,
    pub complete: bool,
    /// Descriptors listed whose target could not be read.
    pub unreadable: u32,
    /// The records this observation affirmed, as runs `(first counter,
    /// count)` of this session's ids: explicit, so a record whose descriptor
    /// was unreadable or unlisted, or whose close event was lost, is never
    /// taken as seen.
    pub seen: Vec<(u32, u32)>,
}

#[derive(Clone, Debug)]
pub enum SuppEvent {
    Proc(ProcObs),
    Threads(ThreadObs),
    Open(FileOpen),
    Close(FileClose),
    Files(FileObs),
    /// Extra recording of `key` ended: unpinned, or the process exited.
    Stopped { key: ProcKey, session: u64, time_ms: u64, exited: bool },
}

impl SuppEvent {
    pub fn key(&self) -> ProcKey {
        match self {
            SuppEvent::Proc(o) => o.key,
            SuppEvent::Threads(o) => o.key,
            SuppEvent::Open(o) => o.key,
            SuppEvent::Close(o) => o.key,
            SuppEvent::Files(o) => o.key,
            SuppEvent::Stopped { key, .. } => *key,
        }
    }

    /// Conservative in-memory size.
    pub fn approx_bytes(&self) -> usize {
        match self {
            SuppEvent::Proc(o) => 64 + o.values.len() * 16,
            SuppEvent::Threads(o) => 64 + o.threads.len() * 64 + o.ended.len() * 8,
            SuppEvent::Open(o) => 96 + o.path.len(),
            SuppEvent::Close(_) => 64,
            SuppEvent::Files(o) => 64 + o.seen.len() * 8,
            SuppEvent::Stopped { .. } => 48,
        }
    }
}

/// Whether `counter` is in `runs` (sorted, disjoint).
pub fn runs_contain(runs: &[(u32, u32)], counter: u32) -> bool {
    let at = runs.partition_point(|(start, _)| *start <= counter);
    at > 0 && counter - runs[at - 1].0 < runs[at - 1].1
}

fn to_runs(mut counters: Vec<u32>) -> Vec<(u32, u32)> {
    counters.sort_unstable();
    counters.dedup();
    let mut runs: Vec<(u32, u32)> = Vec::new();
    for counter in counters {
        match runs.last_mut() {
            Some((start, len)) if start.wrapping_add(*len) == counter => *len += 1,
            _ => runs.push((counter, 1)),
        }
    }
    runs
}

// ---- descriptor kinds ----

const KINDS: [&str; 8] = ["file", "socket", "pipe", "shm", "sem", "kqueue", "anon", "other"];

pub fn kind_code(kind: &str) -> u8 {
    KINDS.iter().position(|k| *k == kind).map(|i| i as u8 + 1).unwrap_or(8)
}

pub fn kind_name(code: u8) -> &'static str {
    KINDS.get((code as usize).wrapping_sub(1)).copied().unwrap_or("other")
}

/// A backend's placeholder for a descriptor whose target it could not read.
fn unreadable_target(path: &str) -> bool {
    path == "(path not readable)" || path == "(not readable)"
}

// ---- the worker side ----

struct FileTrack {
    id: FileId,
    kind: u8,
    path: Arc<str>,
    first_seen: u64,
    opened: Opened,
    last_seen: u64,
}

struct ThreadTrack {
    cpu_time: Option<u64>,
    /// The next point must not join the previous one.
    fresh: bool,
}

#[derive(Default)]
struct Tracker {
    proc_last: Option<u64>,
    proc_cadence: u64,
    threads: HashMap<u64, ThreadTrack>,
    thread_last: Option<u64>,
    /// Monotonic time and the process' CPU time of the previous thread read.
    thread_mono: Option<std::time::Instant>,
    process_cpu_prev: Option<u64>,
    thread_cadence: u64,
    threads_broken: bool,
    files: HashMap<i32, FileTrack>,
    /// (time, complete) of the previous descriptor observation.
    file_last: Option<(u64, bool)>,
    file_cadence: u64,
    files_broken: bool,
}

/// Whether `now` is further from `last` than a cadence allows.
fn is_gap(last: Option<u64>, cadence_ms: u64, now: u64) -> bool {
    match last {
        None => true,
        Some(then) => now < then || now - then > cadence_ms.max(100) * 5 / 2 + 500,
    }
}

/// Turns detail reads into events, on the worker. Holds only what diffing
/// needs (the open descriptors and live threads of each tracked process),
/// never the history itself.
pub struct Recorder {
    session: u64,
    trackers: HashMap<ProcKey, Tracker>,
    next_file: u32,
    strings: HashSet<Arc<str>>,
    events: Vec<SuppEvent>,
}

impl Recorder {
    pub fn new(session: u64) -> Self {
        Self { session, trackers: HashMap::new(), next_file: 0, strings: HashSet::new(), events: Vec::new() }
    }

    pub fn tracked(&self) -> Vec<ProcKey> {
        self.trackers.keys().copied().collect()
    }

    pub fn take(&mut self) -> Vec<SuppEvent> {
        std::mem::take(&mut self.events)
    }

    /// Every still-open descriptor record, as the `Open` that began it: what
    /// a new sidecar segment or a UI that missed events needs so that later
    /// observations and closes have their record. Bounded by
    /// [`CHECKPOINT_BYTES`]: records past it (tens of thousands of
    /// descriptors across pins) are left out, oldest first kept.
    pub fn checkpoint(&self) -> Vec<SuppEvent> {
        let mut open: Vec<(ProcKey, i32, &FileTrack)> = Vec::new();
        for (key, tracker) in &self.trackers {
            open.extend(tracker.files.iter().map(|(fd, track)| (*key, *fd, track)));
        }
        open.sort_by_key(|(key, _, track)| (*key, track.id.1));
        let mut out = Vec::with_capacity(open.len());
        let mut bytes = 0usize;
        for (key, fd, track) in open {
            let event = SuppEvent::Open(FileOpen { key, id: track.id, fd, kind: track.kind, path: track.path.clone(), time_ms: track.first_seen, opened: track.opened });
            bytes += event.approx_bytes();
            if bytes > CHECKPOINT_BYTES {
                break;
            }
            out.push(event);
        }
        out
    }

    fn intern(&mut self, text: &str, max: usize) -> Arc<str> {
        let text = bounded(text.to_string(), max);
        if let Some(shared) = self.strings.get(text.as_str()) {
            return shared.clone();
        }
        if self.strings.len() > 50_000 {
            self.strings.clear();
        }
        let shared: Arc<str> = Arc::from(text);
        self.strings.insert(shared.clone());
        shared
    }

    /// Record what `detail` read. `cadence_ms` is the spacing expected
    /// between metric/thread reads of this process, `files_cadence_ms`
    /// between descriptor walks.
    pub fn observe(&mut self, detail: &ProcDetail, want: Want, cadence_ms: u64, files_cadence_ms: u64) {
        let key = detail.key;
        let time = detail.time_ms;
        let session = self.session;
        if let Detail::Unavailable(_) = detail.identity {
            // Refused or gone: no observation, and the next one cannot be
            // compared with the last. A process never tracked is not
            // started on a failed read.
            if let Some(tracker) = self.trackers.get_mut(&key) {
                tracker.threads_broken = true;
                tracker.files_broken = true;
            }
            return;
        }
        let tracker = self.trackers.entry(key).or_default();
        if !detail.measures.is_empty() {
            let cadence = tracker.proc_cadence.max(cadence_ms);
            let gap_before = is_gap(tracker.proc_last, cadence, time);
            tracker.proc_last = Some(time);
            tracker.proc_cadence = cadence_ms;
            self.events.push(SuppEvent::Proc(ProcObs { key, session, time_ms: time, gap_before, values: detail.measures.clone() }));
        }
        if want.threads {
            let process_cpu_ns = match &detail.identity {
                Detail::Ready(identity) => identity.cpu_time_ns,
                Detail::Unavailable(_) => None,
            };
            self.observe_threads(key, time, &detail.threads, detail.threads_complete, cadence_ms, process_cpu_ns);
        }
        if want.files {
            self.observe_files(key, time, &detail.files, detail.files_complete, files_cadence_ms);
        }
    }

    fn observe_threads(&mut self, key: ProcKey, time: u64, threads: &Detail<Vec<ThreadInfo>>, complete: bool, cadence_ms: u64, process_cpu_ns: Option<u64>) {
        let list = match threads {
            Detail::Ready(list) => list,
            Detail::Unavailable(_) => {
                if let Some(tracker) = self.trackers.get_mut(&key) {
                    tracker.threads_broken = true;
                }
                return;
            }
        };
        // An id of 0 is not a thread identity any backend hands out for a
        // live thread (an unused list slot): never recorded, and the list
        // is not taken as whole.
        let mut complete = complete && list.iter().all(|t| t.id != 0);
        let mut chosen: Vec<&ThreadInfo> = list.iter().filter(|t| t.id != 0).collect();
        if chosen.len() > MAX_THREADS_RECORDED {
            chosen.sort_by(|a, b| b.cpu_time_ns.cmp(&a.cpu_time_ns).then(a.id.cmp(&b.id)));
            chosen.truncate(MAX_THREADS_RECORDED);
            complete = false;
        }
        let names: Vec<Arc<str>> = chosen.iter().map(|t| self.intern(&t.name, MAX_THREAD_NAME_LEN)).collect();
        let session = self.session;
        let Some(tracker) = self.trackers.get_mut(&key) else { return };
        let cadence = tracker.thread_cadence.max(cadence_ms);
        let gap = tracker.threads_broken || is_gap(tracker.thread_last, cadence, time);
        // One monotonic span for the process and every thread: taken right
        // after the read, so all deltas cover the same interval (a wall
        // clock step cannot change them).
        let mono = std::time::Instant::now();
        let span_ns = match tracker.thread_mono {
            Some(then) if !gap => Some(mono.duration_since(then).as_nanos() as f64).filter(|ns| *ns > 0.0),
            _ => None,
        };
        let pct = |now: Option<u64>, then: Option<u64>| -> Option<f32> {
            match (now, then, span_ns) {
                (Some(now), Some(then), Some(span)) if now >= then => Some(((now - then) as f64 / span * 100.0) as f32),
                _ => None,
            }
        };
        let process_pct = pct(process_cpu_ns, tracker.process_cpu_prev);
        let mut listed: HashSet<u64> = HashSet::with_capacity(chosen.len());
        let mut points = Vec::with_capacity(chosen.len());
        for (thread, name) in chosen.iter().zip(names) {
            listed.insert(thread.id);
            let previous = tracker.threads.get(&thread.id);
            let reset = match (previous.and_then(|p| p.cpu_time), thread.cpu_time_ns) {
                (Some(then), Some(now)) => now < then,
                _ => false,
            };
            let gap_before = gap || previous.is_none_or(|p| p.fresh) || reset;
            let cpu_pct = if gap_before { None } else { pct(thread.cpu_time_ns, previous.and_then(|p| p.cpu_time)) };
            tracker.threads.insert(thread.id, ThreadTrack { cpu_time: thread.cpu_time_ns, fresh: false });
            points.push(ThreadPoint {
                id: thread.id,
                name,
                state: thread.state,
                cpu_pct,
                cpu_time_ns: thread.cpu_time_ns,
                gap_before,
                reset,
            });
        }
        let mut ended = Vec::new();
        tracker.threads.retain(|id, track| {
            if listed.contains(id) {
                return true;
            }
            if complete {
                ended.push(*id);
                false
            } else {
                // Not in a partial list: unseen, not ended.
                track.fresh = true;
                true
            }
        });
        tracker.thread_last = Some(time);
        tracker.thread_mono = Some(mono);
        tracker.process_cpu_prev = process_cpu_ns;
        tracker.thread_cadence = cadence_ms;
        tracker.threads_broken = false;
        let span_ms = span_ns.map(|ns| (ns / 1e6).round().min(u32::MAX as f64) as u32);
        self.events.push(SuppEvent::Threads(ThreadObs { key, session, time_ms: time, gap_before: gap, complete, threads: points, ended, process_cpu_ns, process_pct, span_ms }));
    }

    fn observe_files(&mut self, key: ProcKey, time: u64, files: &Detail<Vec<FileInfo>>, complete: bool, cadence_ms: u64) {
        let list = match files {
            Detail::Ready(list) => list,
            Detail::Unavailable(_) => {
                if let Some(tracker) = self.trackers.get_mut(&key) {
                    tracker.files_broken = true;
                }
                return;
            }
        };
        let mut targets: Vec<(i32, u8, Option<Arc<str>>)> = Vec::with_capacity(list.len());
        for file in list {
            let path = (!unreadable_target(&file.path)).then(|| self.intern(&file.path, MAX_PATH_LEN));
            targets.push((file.fd, kind_code(file.kind), path));
        }
        let session = self.session;
        let mut next_file = self.next_file;
        let mut events = Vec::new();
        let Some(tracker) = self.trackers.get_mut(&key) else { return };
        let cadence = tracker.file_cadence.max(cadence_ms);
        let gap = tracker.files_broken || is_gap(tracker.file_last.map(|(t, _)| t), cadence, time);
        // "Opened after the previous observation" needs that observation to
        // have been whole and recent.
        let previous = match tracker.file_last {
            Some((then, true)) if !gap => Some(then),
            _ => None,
        };
        let mut listed: HashSet<i32> = HashSet::with_capacity(targets.len());
        let mut seen: Vec<u32> = Vec::with_capacity(targets.len());
        let mut unreadable = 0u32;
        for (fd, kind, path) in targets {
            listed.insert(fd);
            // A target that could not be read proves the descriptor is
            // open and nothing about what it is: the record on that number
            // is neither affirmed (not in `seen`) nor closed.
            let Some(path) = path else {
                unreadable += 1;
                continue;
            };
            let mut new_record = |events: &mut Vec<SuppEvent>, opened: Opened| {
                let id = (session, next_file);
                next_file = next_file.wrapping_add(1);
                events.push(SuppEvent::Open(FileOpen { key, id, fd, kind, path: path.clone(), time_ms: time, opened }));
                FileTrack { id, kind, path: path.clone(), first_seen: time, opened, last_seen: time }
            };
            match tracker.files.get_mut(&fd) {
                Some(track) if track.kind == kind && track.path == path => {
                    track.last_seen = time;
                    seen.push(track.id.1);
                }
                Some(track) => {
                    // Same number, another target: the old one is gone, and
                    // the new one was opened after the old was last seen.
                    events.push(SuppEvent::Close(FileClose { key, id: track.id, last_seen_ms: track.last_seen, gone_ms: time, reason: CloseReason::Replaced }));
                    let after = track.last_seen;
                    *track = new_record(&mut events, Opened::After(after));
                    seen.push(track.id.1);
                }
                None => {
                    let opened = match previous {
                        Some(then) => Opened::After(then),
                        None => Opened::AlreadyOpen,
                    };
                    let track = new_record(&mut events, opened);
                    seen.push(track.id.1);
                    tracker.files.insert(fd, track);
                }
            }
        }
        tracker.files.retain(|fd, track| {
            if listed.contains(fd) || !complete {
                // Unlisted in a partial list: unseen, not closed.
                return true;
            }
            events.push(SuppEvent::Close(FileClose { key, id: track.id, last_seen_ms: track.last_seen, gone_ms: time, reason: CloseReason::Absent }));
            false
        });
        events.push(SuppEvent::Files(FileObs { key, session, time_ms: time, gap_before: gap, complete, unreadable, seen: to_runs(seen) }));
        tracker.file_last = Some((time, complete));
        tracker.file_cadence = cadence_ms;
        tracker.files_broken = false;
        self.next_file = next_file;
        self.events.extend(events);
    }

    /// `key` verifiably exited (see `SystemBackend::is_alive`), noticed at
    /// `time_ms`: its descriptors closed with it; stop tracking.
    pub fn exited(&mut self, key: ProcKey, time_ms: u64) {
        let Some(tracker) = self.trackers.remove(&key) else { return };
        let mut open: Vec<&FileTrack> = tracker.files.values().collect();
        open.sort_by_key(|track| track.id.1);
        for track in open {
            self.events.push(SuppEvent::Close(FileClose { key, id: track.id, last_seen_ms: track.last_seen, gone_ms: time_ms, reason: CloseReason::Exited }));
        }
        self.events.push(SuppEvent::Stopped { key, session: self.session, time_ms, exited: true });
    }

    /// Stop extra recording of `key` (unpinned, no longer inspected, or its
    /// exit could not be confirmed). What was open stays open in the record,
    /// last seen when it was last seen.
    pub fn stop(&mut self, key: ProcKey, time_ms: u64) {
        if self.trackers.remove(&key).is_some() {
            self.events.push(SuppEvent::Stopped { key, session: self.session, time_ms, exited: false });
        }
    }
}

// ---- the UI side ----

#[derive(Clone, Debug)]
pub struct SuppPoint {
    pub time_ms: u64,
    pub gap_before: bool,
    pub values: Box<[(Measure, i64)]>,
}

impl SuppPoint {
    pub fn value(&self, measure: Measure) -> Option<i64> {
        self.values.iter().find(|(m, _)| *m == measure).map(|(_, v)| *v)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ThreadSample {
    pub time_ms: u64,
    pub cpu_time_ns: Option<u64>,
    pub cpu_pct: Option<f32>,
    pub state: ThreadState,
    pub gap_before: bool,
}

/// One observed incarnation of a thread id: an id seen again after it
/// ended, or with less CPU time, is the next incarnation, with its own line,
/// name and end.
#[derive(Clone, Debug)]
pub struct ThreadHistory {
    pub id: u64,
    pub incarnation: u32,
    pub name: Arc<str>,
    pub samples: Vec<ThreadSample>,
    /// Observed ended at this time (absent from a complete list).
    pub ended_ms: Option<u64>,
}

/// One observation: of the thread list, or of the descriptor list (then
/// with what it affirmed).
#[derive(Clone, Debug)]
pub struct ObsMark {
    pub time_ms: u64,
    pub session: u64,
    pub gap_before: bool,
    pub complete: bool,
    pub unreadable: u32,
    pub seen: Box<[(u32, u32)]>,
    /// Thread observations: the process' CPU over the interval ending here,
    /// and that interval (monotonic ms), as recorded with the thread rates.
    pub process_pct: Option<f32>,
    pub span_ms: Option<u32>,
}

#[derive(Clone, Debug)]
pub struct FileRecord {
    pub id: FileId,
    pub fd: i32,
    pub kind: u8,
    pub path: Arc<str>,
    pub first_seen_ms: u64,
    pub opened: Opened,
    /// The last observation that affirmed it, of everything recorded (see
    /// [`ProcSupp::last_seen_as_of`] for a past time).
    pub last_seen_ms: u64,
    /// (first seen gone, why).
    pub closed: Option<(u64, CloseReason)>,
}

pub type ThreadKey = (u64, u32);

#[derive(Default)]
pub struct ProcSupp {
    pub points: Vec<SuppPoint>,
    pub threads: HashMap<ThreadKey, ThreadHistory>,
    /// Thread id → incarnations seen.
    incarnations: HashMap<u64, u32>,
    pub thread_obs: Vec<ObsMark>,
    pub files: Vec<FileRecord>,
    index: HashMap<FileId, usize>,
    pub file_obs: Vec<ObsMark>,
    /// Measures seen in any point, as a bit per code.
    pub measures_seen: u64,
    /// The last time extra recording stopped, and whether by exit.
    pub stopped: Option<(u64, bool)>,
}

impl ProcSupp {
    /// The last observation at or before `time_ms` that affirmed `record` —
    /// what was known *then*, never a later sighting.
    pub fn last_seen_as_of(&self, record: &FileRecord, time_ms: u64) -> Option<u64> {
        if record.first_seen_ms > time_ms {
            return None;
        }
        if record.last_seen_ms <= time_ms {
            return Some(record.last_seen_ms);
        }
        let end = self.file_obs.partition_point(|m| m.time_ms <= time_ms);
        for mark in self.file_obs[..end].iter().rev() {
            if mark.time_ms < record.first_seen_ms {
                break;
            }
            if mark.session == record.id.0 && runs_contain(&mark.seen, record.id.1) {
                return Some(mark.time_ms);
            }
        }
        // Thinned or evicted marks: its own first sighting is still true.
        Some(record.first_seen_ms)
    }

    /// The incarnation a sample at `time_ms` belongs to.
    fn incarnation_for(&mut self, point: &ThreadPoint, time_ms: u64) -> ThreadKey {
        let Some(&count) = self.incarnations.get(&point.id) else {
            self.incarnations.insert(point.id, 1);
            return (point.id, 0);
        };
        let latest = count - 1;
        let (last_time, ended) = match self.threads.get(&(point.id, latest)) {
            Some(track) => (track.samples.last().map(|s| s.time_ms).unwrap_or(0), track.ended_ms),
            None => return (point.id, latest),
        };
        if time_ms >= last_time {
            if ended.is_some() || point.reset {
                self.incarnations.insert(point.id, count + 1);
                return (point.id, count);
            }
            return (point.id, latest);
        }
        // Restored history, older than what is kept: the incarnation it
        // falls in by time.
        (0..count)
            .rev()
            .find(|n| self.threads.get(&(point.id, *n)).and_then(|t| t.samples.first()).is_some_and(|s| s.time_ms <= time_ms))
            .map(|n| (point.id, n))
            .unwrap_or((point.id, 0))
    }
}

fn insert_sorted<T>(items: &mut Vec<T>, item: T, time: impl Fn(&T) -> u64) {
    let at = time(&item);
    if items.last().is_none_or(|last| time(last) < at) {
        items.push(item);
        return;
    }
    let index = items.partition_point(|existing| time(existing) < at);
    if items.get(index).is_some_and(|existing| time(existing) == at) {
        return;
    }
    items.insert(index, item);
}

/// Keep the last item per bucket of the tiers (15 min raw, then one per
/// 10 s, then one per 60 s, nothing past 24 h). A dropped item's
/// `gap_before` passes to the item that replaces it, so a break is never
/// joined over.
fn thin_items<T>(items: &mut Vec<T>, now_ms: u64, time: impl Fn(&T) -> u64, gap: impl Fn(&mut T) -> &mut bool) {
    let bucket_of = |t: u64| -> Option<(u64, u64)> {
        let age = now_ms.saturating_sub(t);
        if age <= FINE_WINDOW_MS {
            None
        } else if age <= MID_WINDOW_MS {
            Some((MID_BUCKET_MS, t / MID_BUCKET_MS))
        } else {
            Some((COARSE_BUCKET_MS, t / COARSE_BUCKET_MS))
        }
    };
    let old = std::mem::take(items);
    for mut item in old {
        let t = time(&item);
        if now_ms.saturating_sub(t) > MAX_AGE_MS {
            continue;
        }
        if let (Some(bucket), Some(last)) = (bucket_of(t), items.last_mut()) {
            if bucket_of(time(last)) == Some(bucket) {
                let carried = *gap(last);
                *gap(&mut item) |= carried;
                *last = item;
                continue;
            }
        }
        items.push(item);
    }
}

/// The UI's copy of the supplemental history, bounded by bytes and age.
pub struct SuppStore {
    procs: HashMap<ProcKey, ProcSupp>,
    pub generation: u64,
    bytes: usize,
    budget: usize,
    last_thinned_ms: u64,
    /// Data before this time was evicted by the byte budget (0: none).
    pub evicted_before_ms: u64,
}

impl Default for SuppStore {
    fn default() -> Self {
        Self::new()
    }
}

/// The in-memory budget for supplemental history on a machine with this
/// much RAM: a quarter of the basic history's, 8..32 MiB.
pub fn budget_for_ram(total_bytes: u64) -> usize {
    (crate::history::budget_for_ram(total_bytes) / 4).clamp(8 << 20, 32 << 20)
}

impl SuppStore {
    pub fn new() -> Self {
        Self { procs: HashMap::new(), generation: 0, bytes: 0, budget: 16 << 20, last_thinned_ms: 0, evicted_before_ms: 0 }
    }

    pub fn set_budget(&mut self, bytes: usize) {
        self.budget = bytes;
    }

    pub fn get(&self, key: ProcKey) -> Option<&ProcSupp> {
        self.procs.get(&key)
    }

    pub fn apply(&mut self, events: Vec<SuppEvent>, now_ms: u64) {
        for event in events {
            if event_time(&event) > now_ms + MINUTE_MS {
                continue;
            }
            self.bytes += event.approx_bytes();
            let proc = self.procs.entry(event.key()).or_default();
            match event {
                SuppEvent::Proc(obs) => {
                    for (measure, _) in &obs.values {
                        proc.measures_seen |= 1u64 << measure.code();
                    }
                    proc.stopped = None;
                    insert_sorted(&mut proc.points, SuppPoint { time_ms: obs.time_ms, gap_before: obs.gap_before, values: obs.values.into_boxed_slice() }, |p| p.time_ms);
                }
                SuppEvent::Threads(obs) => {
                    let mark = ObsMark {
                        time_ms: obs.time_ms,
                        session: obs.session,
                        gap_before: obs.gap_before,
                        complete: obs.complete,
                        unreadable: 0,
                        seen: Box::new([]),
                        process_pct: obs.process_pct,
                        span_ms: obs.span_ms,
                    };
                    insert_sorted(&mut proc.thread_obs, mark, |m| m.time_ms);
                    for point in obs.threads {
                        if point.id == 0 {
                            continue;
                        }
                        let thread_key = proc.incarnation_for(&point, obs.time_ms);
                        let track = proc.threads.entry(thread_key).or_insert_with(|| ThreadHistory {
                            id: point.id,
                            incarnation: thread_key.1,
                            name: point.name.clone(),
                            samples: Vec::new(),
                            ended_ms: None,
                        });
                        if track.samples.last().is_none_or(|last| last.time_ms <= obs.time_ms) {
                            track.name = point.name.clone();
                        }
                        let sample = ThreadSample { time_ms: obs.time_ms, cpu_time_ns: point.cpu_time_ns, cpu_pct: point.cpu_pct, state: point.state, gap_before: point.gap_before };
                        insert_sorted(&mut track.samples, sample, |s| s.time_ms);
                    }
                    for id in obs.ended {
                        let Some(&count) = proc.incarnations.get(&id) else { continue };
                        if let Some(track) = proc.threads.get_mut(&(id, count - 1)) {
                            if track.ended_ms.is_none() && track.samples.last().is_some_and(|last| last.time_ms <= obs.time_ms) {
                                track.ended_ms = Some(obs.time_ms);
                            }
                        }
                    }
                }
                SuppEvent::Open(open) => {
                    if !proc.index.contains_key(&open.id) {
                        proc.index.insert(open.id, proc.files.len());
                        proc.files.push(FileRecord {
                            id: open.id,
                            fd: open.fd,
                            kind: open.kind,
                            path: open.path,
                            first_seen_ms: open.time_ms,
                            opened: open.opened,
                            last_seen_ms: open.time_ms,
                            closed: None,
                        });
                    }
                }
                SuppEvent::Close(close) => {
                    if let Some(&index) = proc.index.get(&close.id) {
                        let record = &mut proc.files[index];
                        record.last_seen_ms = record.last_seen_ms.max(close.last_seen_ms);
                        record.closed = Some((close.gone_ms, close.reason));
                    }
                }
                SuppEvent::Files(obs) => {
                    for record in proc.files.iter_mut() {
                        if record.closed.is_none() && record.id.0 == obs.session && record.first_seen_ms <= obs.time_ms && runs_contain(&obs.seen, record.id.1) {
                            record.last_seen_ms = record.last_seen_ms.max(obs.time_ms);
                        }
                    }
                    let mark = ObsMark {
                        time_ms: obs.time_ms,
                        session: obs.session,
                        gap_before: obs.gap_before,
                        complete: obs.complete,
                        unreadable: obs.unreadable,
                        seen: obs.seen.into_boxed_slice(),
                        process_pct: None,
                        span_ms: None,
                    };
                    insert_sorted(&mut proc.file_obs, mark, |m| m.time_ms);
                }
                SuppEvent::Stopped { time_ms, exited, .. } => {
                    proc.stopped = Some((time_ms, exited));
                    if exited {
                        for track in proc.threads.values_mut() {
                            if track.ended_ms.is_none() && track.samples.last().is_some_and(|last| last.time_ms <= time_ms) {
                                track.ended_ms = Some(time_ms);
                            }
                        }
                    }
                }
            }
        }
        self.generation = self.generation.wrapping_add(1);
        self.maintain(now_ms, self.bytes > self.budget);
    }

    /// Tiers, age, per-process caps and the byte budget. At most every few
    /// seconds unless `force`d (over budget).
    pub fn maintain(&mut self, now_ms: u64, force: bool) {
        if !force && now_ms.saturating_sub(self.last_thinned_ms) < 5 * SECOND_MS {
            return;
        }
        self.last_thinned_ms = now_ms;
        for proc in self.procs.values_mut() {
            thin_items(&mut proc.points, now_ms, |p| p.time_ms, |p| &mut p.gap_before);
            thin_items(&mut proc.thread_obs, now_ms, |m| m.time_ms, |m| &mut m.gap_before);
            thin_items(&mut proc.file_obs, now_ms, |m| m.time_ms, |m| &mut m.gap_before);
            for track in proc.threads.values_mut() {
                thin_items(&mut track.samples, now_ms, |s| s.time_ms, |s| &mut s.gap_before);
            }
            proc.threads.retain(|_, track| !track.samples.is_empty());
            if proc.threads.len() > MAX_THREAD_TRACKS {
                // Least recently seen go first, ended or not.
                let mut by_age: Vec<(u64, ThreadKey)> = proc.threads.iter().map(|(k, t)| (t.samples.last().map(|s| s.time_ms).unwrap_or(0), *k)).collect();
                by_age.sort_unstable();
                for (_, k) in by_age.into_iter().take(proc.threads.len() - MAX_THREAD_TRACKS) {
                    proc.threads.remove(&k);
                }
            }
            // A record no observation mentions for a day goes (never with
            // an invented close); past the cap, least recently seen first.
            let before = proc.files.len();
            proc.files.retain(|record| now_ms.saturating_sub(record_time(record)) <= MAX_AGE_MS);
            if proc.files.len() > MAX_FILE_RECORDS {
                let mut by_age: Vec<(u64, FileId)> = proc.files.iter().map(|r| (record_time(r), r.id)).collect();
                by_age.sort_unstable();
                let drop: HashSet<FileId> = by_age.into_iter().take(proc.files.len() - MAX_FILE_RECORDS).map(|(_, id)| id).collect();
                proc.files.retain(|record| !drop.contains(&record.id));
            }
            if proc.files.len() != before {
                proc.index = proc.files.iter().enumerate().map(|(i, r)| (r.id, i)).collect();
            }
        }
        self.procs.retain(|_, proc| !is_empty(proc));
        self.bytes = self.procs.values().map(proc_bytes).sum();
        // The byte budget: the oldest data goes first, in steps of an
        // eighth of what is kept (at least a second), open descriptor records
        // by when they were last seen. The cutoff never passes the newest
        // observation: an overrun thins history, it never empties it.
        let newest = self.procs.values().filter_map(newest_time).max().unwrap_or(0);
        while self.bytes > self.budget {
            let Some(oldest) = self.procs.values().filter_map(oldest_time).min() else { break };
            let cutoff = oldest + ((newest.saturating_sub(oldest)) / 8).max(SECOND_MS);
            if cutoff > newest {
                break;
            }
            for proc in self.procs.values_mut() {
                proc.points.retain(|p| p.time_ms >= cutoff);
                proc.thread_obs.retain(|m| m.time_ms >= cutoff);
                proc.file_obs.retain(|m| m.time_ms >= cutoff);
                for track in proc.threads.values_mut() {
                    track.samples.retain(|s| s.time_ms >= cutoff);
                }
                proc.threads.retain(|_, track| !track.samples.is_empty());
                proc.files.retain(|r| record_time(r) >= cutoff);
                proc.index = proc.files.iter().enumerate().map(|(i, r)| (r.id, i)).collect();
            }
            self.procs.retain(|_, proc| !is_empty(proc));
            self.evicted_before_ms = self.evicted_before_ms.max(cutoff);
            let bytes: usize = self.procs.values().map(proc_bytes).sum();
            let stuck = bytes >= self.bytes;
            self.bytes = bytes;
            if stuck {
                break;
            }
        }
        self.generation = self.generation.wrapping_add(1);
    }

    /// The newest supplemental observation of `key`, of any kind.
    pub fn newest_ms(&self, key: ProcKey) -> Option<u64> {
        let proc = self.procs.get(&key)?;
        [proc.points.last().map(|p| p.time_ms), proc.thread_obs.last().map(|m| m.time_ms), proc.file_obs.last().map(|m| m.time_ms)].into_iter().flatten().max()
    }

    /// Readings of `measure` for `key` over `[from, to]` plus one either
    /// side. A point without the measure is a hole: the next reading breaks.
    pub fn readings(&self, key: ProcKey, measure: Measure, from_ms: u64, to_ms: u64) -> Vec<Reading> {
        let Some(proc) = self.procs.get(&key) else { return Vec::new() };
        let start = proc.points.partition_point(|p| p.time_ms < from_ms).saturating_sub(1);
        let end = (proc.points.partition_point(|p| p.time_ms <= to_ms) + 1).min(proc.points.len());
        let mut out = Vec::new();
        let mut hole = true;
        for point in &proc.points[start..end.max(start)] {
            match point.value(measure) {
                Some(value) => {
                    out.push(Reading { time_ms: point.time_ms, value, gap_before: point.gap_before || hole });
                    hole = false;
                }
                None => hole = true,
            }
        }
        out
    }

    /// The newest reading of `measure` at or before `time_ms`, not older
    /// than `tolerance_ms`, with the one before it when they join (for a
    /// rate).
    pub fn reading_at(&self, key: ProcKey, measure: Measure, time_ms: u64, tolerance_ms: u64) -> Option<(Reading, Option<Reading>)> {
        let proc = self.procs.get(&key)?;
        let end = proc.points.partition_point(|p| p.time_ms <= time_ms);
        let (index, point) = proc.points[..end].iter().enumerate().rev().find(|(_, p)| p.value(measure).is_some())?;
        if time_ms - point.time_ms > tolerance_ms {
            return None;
        }
        let now = Reading { time_ms: point.time_ms, value: point.value(measure)?, gap_before: point.gap_before };
        let before = (index > 0 && !point.gap_before)
            .then(|| &proc.points[index - 1])
            .and_then(|p| p.value(measure).map(|value| Reading { time_ms: p.time_ms, value, gap_before: p.gap_before }));
        Some((now, before))
    }

    /// The thread observation at or before `time_ms` within `tolerance_ms`,
    /// and every thread incarnation recorded in it with its sample index.
    pub fn threads_at(&self, key: ProcKey, time_ms: u64, tolerance_ms: u64) -> Option<(ObsMark, Vec<(&ThreadHistory, usize)>)> {
        let proc = self.procs.get(&key)?;
        let end = proc.thread_obs.partition_point(|m| m.time_ms <= time_ms);
        let mark = proc.thread_obs[..end].last()?.clone();
        if time_ms - mark.time_ms > tolerance_ms {
            return None;
        }
        let mut rows = Vec::new();
        for track in proc.threads.values() {
            if let Ok(index) = track.samples.binary_search_by_key(&mark.time_ms, |s| s.time_ms) {
                rows.push((track, index));
            }
        }
        Some((mark, rows))
    }
}

fn event_time(event: &SuppEvent) -> u64 {
    match event {
        SuppEvent::Proc(o) => o.time_ms,
        SuppEvent::Threads(o) => o.time_ms,
        SuppEvent::Open(o) => o.time_ms,
        SuppEvent::Close(o) => o.gone_ms,
        SuppEvent::Files(o) => o.time_ms,
        SuppEvent::Stopped { time_ms, .. } => *time_ms,
    }
}

/// When a descriptor record was last known about: its close, else its last
/// sighting.
fn record_time(record: &FileRecord) -> u64 {
    record.closed.map(|(gone, _)| gone).unwrap_or(record.last_seen_ms).max(record.last_seen_ms)
}

fn is_empty(proc: &ProcSupp) -> bool {
    proc.points.is_empty() && proc.threads.is_empty() && proc.files.is_empty() && proc.file_obs.is_empty() && proc.thread_obs.is_empty()
}

fn newest_time(proc: &ProcSupp) -> Option<u64> {
    let points = proc.points.last().map(|p| p.time_ms);
    let marks = proc.file_obs.last().map(|m| m.time_ms).into_iter().chain(proc.thread_obs.last().map(|m| m.time_ms)).max();
    [points, marks].into_iter().flatten().max()
}

fn oldest_time(proc: &ProcSupp) -> Option<u64> {
    let points = proc.points.first().map(|p| p.time_ms);
    let threads = proc.threads.values().filter_map(|t| t.samples.first().map(|s| s.time_ms)).min();
    let marks = proc.file_obs.first().map(|m| m.time_ms).into_iter().chain(proc.thread_obs.first().map(|m| m.time_ms)).min();
    let files = proc.files.iter().map(record_time).min();
    [points, threads, marks, files].into_iter().flatten().min()
}

/// Conservative: capacities, not lengths, plus map and allocation overhead.
fn proc_bytes(proc: &ProcSupp) -> usize {
    let points: usize = proc.points.capacity() * std::mem::size_of::<SuppPoint>() + proc.points.iter().map(|p| 16 + p.values.len() * 16).sum::<usize>();
    let threads: usize = proc
        .threads
        .values()
        .map(|t| 96 + std::mem::size_of::<ThreadHistory>() + t.name.len() + t.samples.capacity() * std::mem::size_of::<ThreadSample>())
        .sum::<usize>()
        + proc.incarnations.len() * 32;
    let files: usize = proc.files.capacity() * std::mem::size_of::<FileRecord>() + proc.files.iter().map(|r| 32 + r.path.len()).sum::<usize>() + proc.index.len() * 48;
    let marks = (proc.thread_obs.capacity() + proc.file_obs.capacity()) * std::mem::size_of::<ObsMark>() + proc.file_obs.iter().map(|m| 16 + m.seen.len() * 8).sum::<usize>();
    256 + points + threads + files + marks
}

/// A thread's CPU over time as graph points: each observation's recorded
/// interval rate (its CPU-time delta over the monotonic span since the
/// previous read — the same arithmetic as the process' own CPU, never the
/// scheduler's aged estimate). An observation without a rate (new thread,
/// reset, gap) is a hole.
pub fn thread_points(track: &ThreadHistory, from_ms: u64, to_ms: u64) -> Vec<crate::history::Point> {
    use crate::history::Point;
    let start = track.samples.partition_point(|s| s.time_ms < from_ms).saturating_sub(1);
    let end = (track.samples.partition_point(|s| s.time_ms <= to_ms) + 1).min(track.samples.len());
    let mut out = Vec::new();
    let mut hole = true;
    for s in &track.samples[start..end.max(start)] {
        match s.cpu_pct {
            Some(value) => {
                out.push(Point { time_ms: s.time_ms, value, gap_before: s.gap_before || hole });
                hole = false;
            }
            None => hole = true,
        }
    }
    out
}

/// A thread's CPU over the interval ending at one of its samples, as
/// recorded; `None` (unknown, not 0) for a new thread, a reset or a gap.
pub fn thread_cpu_at(track: &ThreadHistory, index: usize) -> Option<f32> {
    track.samples.get(index)?.cpu_pct
}
