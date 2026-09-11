//! Opt-in presentation timeline. Producers only publish atomics and try_send;
//! the diagnostic worker aggregates at 1 Hz, with no paint clock of its own.
use std::{
    collections::VecDeque,
    fmt::Write,
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc::{self, SyncSender},
        Arc, OnceLock,
    },
    time::{Duration, Instant},
};

#[derive(Clone, Copy)]
#[repr(usize)]
pub enum Stage {
    Request,
    Draw,
    UploadBegin,
    UploadEnd,
    Encoded,
    Queued,
    Commit,
    CommitReturned,
    Scheduled,
    Completed,
    Presented,
    Callback,
    RetirementBegin,
    RetirementEnd,
    EvictionBegin,
    EvictionEnd,
    End,
}
const STAGES: usize = Stage::End as usize + 1;
const NAMES: [&str; STAGES] = [
    "request",
    "draw",
    "upload_begin",
    "upload_end",
    "encoded",
    "queued",
    "commit",
    "commit_returned",
    "scheduled",
    "completed",
    "presented",
    "callback",
    "retirement_begin",
    "retirement_end",
    "eviction_begin",
    "eviction_end",
    "request_end",
];

#[derive(Clone, Copy)]
#[repr(usize)]
pub enum Cause {
    RepaintsInFlight,
    PresentsInFlight,
    NoDrawable,
    DrawableWait,
    MetalLinkBeat,
    Occluded,
    SubmitterFull,
    CommandBufferCap,
    SetupPending,
    UniformsNotResident,
    PassAborted,
    Retirement,
    Eviction,
    CommitBlocked,
    LogBackpressure,
}
const CAUSES: usize = Cause::LogBackpressure as usize + 1;
const CAUSE_NAMES: [&str; CAUSES] = [
    "repaints_in_flight",
    "presents_in_flight",
    "no_drawable",
    "drawable_wait",
    "metal_link_beat",
    "occluded",
    "submitter_full",
    "command_buffer_cap",
    "setup_pending",
    "uniforms_not_resident",
    "pass_aborted",
    "retirement_running",
    "eviction_running",
    "commit_blocked",
    "log_backpressure",
];

static EPOCH: OnceLock<Instant> = OnceLock::new();
static DROPPED: AtomicU64 = AtomicU64::new(0);
fn now() -> u64 {
    EPOCH
        .get_or_init(Instant::now)
        .elapsed()
        .as_nanos()
        .min(u64::MAX as u128 - 1) as u64
        + 1
}

pub struct Frame {
    pub repaint: u64,
    times: [AtomicU64; STAGES],
    causes: AtomicU64,
    cb: AtomicU64,
    pending: AtomicU64,
    uploads: AtomicU64,
    retired: AtomicU64,
    evicted: AtomicU64,
    retirement_pending: AtomicU64,
    pool_bytes: AtomicU64,
    inflight: AtomicU64,
    drawable_wait: AtomicU64,
    /// The presented handler ran without a glass time (see `presented`).
    unconfirmed: AtomicU64,
    maintenance: [AtomicU64; 6],
    admission: [AtomicU64; 5],
    category_bytes: [AtomicU64; 8],
    logs: u64,
}
pub type Trace = Arc<Frame>;
impl Frame {
    pub fn mark(&self, stage: Stage) {
        self.times[stage as usize].store(now(), Ordering::Release);
    }
    pub fn first(&self, stage: Stage) {
        let _ = self.times[stage as usize].compare_exchange(
            0,
            now(),
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }
    pub fn cause(&self, cause: Cause) {
        self.causes.fetch_or(1 << cause as usize, Ordering::Relaxed);
    }
    pub fn command(&self, cb: u64) {
        self.cb.store(cb, Ordering::Relaxed);
    }
    pub fn inflight(&self, n: usize) {
        self.inflight.store(n as u64, Ordering::Relaxed);
    }
    pub fn drawable_wait(&self, ns: u64) {
        self.drawable_wait.store(ns, Ordering::Relaxed);
    }
    pub fn maintenance(&self, budget: &crate::retained_instances::RetainedUploadBudget) {
        for (slot, value) in self.category_bytes.iter().zip(budget.stats.category_bytes) {
            slot.store(value as u64, Ordering::Relaxed);
        }
        let values = [
            budget.limit,
            budget.eviction.bytes,
            budget.eviction.buffers,
            budget.retirement.bytes,
            budget.retirement.buffers,
            budget
                .eviction
                .largest_unit
                .max(budget.retirement.largest_unit),
        ];
        for (slot, value) in self.maintenance.iter().zip(values) {
            slot.store(value as u64, Ordering::Relaxed);
        }
    }
    pub fn missing_item(&self, list: usize, shader: usize, wanted: usize, progressive: bool) {
        if self.admission[0].fetch_add(1, Ordering::Relaxed) == 0 {
            self.admission[2].store(list as u64, Ordering::Relaxed);
            self.admission[3].store(shader as u64, Ordering::Relaxed);
            self.admission[4].store(wanted as u64, Ordering::Relaxed);
        }
        self.admission[1].fetch_add(u64::from(progressive), Ordering::Relaxed);
    }
    pub fn uploads(
        &self,
        bytes: usize,
        pending: usize,
        retired: usize,
        evicted: usize,
        queued: usize,
        pool_bytes: usize,
    ) {
        self.uploads.store(bytes as u64, Ordering::Relaxed);
        self.pending.store(pending as u64, Ordering::Relaxed);
        self.retired.fetch_add(retired as u64, Ordering::Relaxed);
        self.evicted.fetch_add(evicted as u64, Ordering::Relaxed);
        self.retirement_pending
            .store(queued as u64, Ordering::Relaxed);
        self.pool_bytes.store(pool_bytes as u64, Ordering::Relaxed);
        if evicted != 0 {
            self.cause(Cause::Eviction);
        }
    }
    /// Both Apple values use host-time seconds. Convert to the trace's epoch;
    /// callback delivery time remains separate from actual presentation time.
    pub fn presented(&self, host_now: f64, glass: f64) {
        let callback = now();
        self.times[Stage::Callback as usize].store(callback, Ordering::Release);
        if glass.is_finite() && glass > 0.0 && host_now >= glass {
            let lag = ((host_now - glass) * 1e9) as u64;
            self.times[Stage::Presented as usize]
                .store(callback.saturating_sub(lag).max(1), Ordering::Release);
        } else {
            // the handler ran but the drawable reports no glass time (a
            // window the compositor did not put on glass, or a layer that
            // does not report presentedTime): the present happened, `n` is
            // the confirmed ones — this counts the rest
            self.unconfirmed.store(1, Ordering::Release);
        }
    }
}

/// Marks even an early-returned repaint. The trace retains no UI/GPU objects.
pub struct RequestEnd(pub Option<Trace>);
impl Drop for RequestEnd {
    fn drop(&mut self) {
        if let Some(frame) = &self.0 {
            if crate::log::dropped_log_records() > frame.logs {
                frame.cause(Cause::LogBackpressure);
            }
            frame.mark(Stage::End);
        }
    }
}

pub fn begin(repaint: u64) -> Option<Trace> {
    if !crate::makepad_error_log::trace_enabled("present") {
        return None;
    }
    let frame = Arc::new(Frame {
        repaint,
        times: std::array::from_fn(|_| AtomicU64::new(0)),
        causes: AtomicU64::new(0),
        cb: AtomicU64::new(0),
        pending: AtomicU64::new(0),
        uploads: AtomicU64::new(0),
        retired: AtomicU64::new(0),
        evicted: AtomicU64::new(0),
        retirement_pending: AtomicU64::new(0),
        pool_bytes: AtomicU64::new(0),
        inflight: AtomicU64::new(0),
        drawable_wait: AtomicU64::new(0),
        unconfirmed: AtomicU64::new(0),
        maintenance: std::array::from_fn(|_| AtomicU64::new(0)),
        admission: std::array::from_fn(|_| AtomicU64::new(0)),
        category_bytes: std::array::from_fn(|_| AtomicU64::new(0)),
        logs: crate::log::dropped_log_records(),
    });
    frame.mark(Stage::Request);
    if sender().try_send(frame.clone()).is_err() {
        DROPPED.fetch_add(1, Ordering::Relaxed);
    }
    Some(frame)
}

fn sender() -> &'static SyncSender<Trace> {
    static SENDER: OnceLock<SyncSender<Trace>> = OnceLock::new();
    SENDER.get_or_init(|| {
        let (tx, rx) = mpsc::sync_channel(512);
        std::thread::Builder::new()
            .name("makepad-present-trace".into())
            .spawn(move || {
                let mut records = VecDeque::<Record>::with_capacity(2048);
                let mut last_report = Instant::now();
                let mut last_glass = None;
                let mut reported_drops = 0;
                let mut reported_logs = crate::log::dropped_log_records();
                loop {
                    let wait = Duration::from_secs(1).saturating_sub(last_report.elapsed());
                    let message = if records.is_empty() {
                        rx.recv().map_err(|_| mpsc::RecvTimeoutError::Disconnected)
                    } else {
                        rx.recv_timeout(wait)
                    };
                    match message {
                        Ok(frame) => {
                            if records.len() == 2048 {
                                records.pop_front();
                                DROPPED.fetch_add(1, Ordering::Relaxed);
                            }
                            records.push_back(Record {
                                frame,
                                reported_causes: 0,
                                reported_request: false,
                                reported_work: false,
                                reported_glass: false,
                            });
                        }
                        Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        Err(mpsc::RecvTimeoutError::Timeout) => (),
                    }
                    if last_report.elapsed() < Duration::from_secs(1) {
                        continue;
                    }
                    report(
                        &mut records,
                        &mut last_glass,
                        &mut reported_drops,
                        &mut reported_logs,
                    );
                    last_report = Instant::now();
                    // Keep enough history for both sides and every attempt in a
                    // long gap. Missing callbacks cannot grow the trace forever.
                    let cutoff = now().saturating_sub(8_000_000_000);
                    while records.front().is_some_and(|r| {
                        r.frame.times[Stage::Request as usize].load(Ordering::Acquire) < cutoff
                    }) {
                        records.pop_front();
                    }
                }
            })
            .expect("presentation trace worker");
        tx
    })
}
struct Record {
    frame: Trace,
    reported_causes: u64,
    reported_request: bool,
    reported_work: bool,
    reported_glass: bool,
}
#[derive(Clone)]
struct Snapshot {
    repaint: u64,
    times: [u64; STAGES],
    causes: u64,
    cb: u64,
    values: [u64; 8],
    maintenance: [u64; 6],
    admission: [u64; 5],
    category_bytes: [u64; 8],
}
impl Snapshot {
    fn take(f: &Frame) -> Self {
        Self {
            repaint: f.repaint,
            times: std::array::from_fn(|i| f.times[i].load(Ordering::Acquire)),
            causes: f.causes.load(Ordering::Relaxed),
            cb: f.cb.load(Ordering::Relaxed),
            values: [
                f.uploads.load(Ordering::Relaxed),
                f.pending.load(Ordering::Relaxed),
                f.retired.load(Ordering::Relaxed),
                f.evicted.load(Ordering::Relaxed),
                f.retirement_pending.load(Ordering::Relaxed),
                f.pool_bytes.load(Ordering::Relaxed),
                f.inflight.load(Ordering::Relaxed),
                f.drawable_wait.load(Ordering::Relaxed),
            ],
            maintenance: std::array::from_fn(|i| f.maintenance[i].load(Ordering::Relaxed)),
            admission: std::array::from_fn(|i| f.admission[i].load(Ordering::Relaxed)),
            category_bytes: std::array::from_fn(|i| f.category_bytes[i].load(Ordering::Relaxed)),
        }
    }
    fn glass(&self) -> u64 {
        self.times[Stage::Presented as usize]
    }
    fn line(&self, origin: u64) -> String {
        let mut s = format!("repaint={} cb={}", self.repaint, self.cb);
        for (i, t) in self.times.iter().enumerate() {
            if *t == 0 {
                let _ = write!(s, " {}=pending", NAMES[i]);
            } else {
                let _ = write!(
                    s,
                    " {}={:.3}ms",
                    NAMES[i],
                    (*t as f64 - origin as f64) / 1e6
                );
            }
        }
        for (i, name) in CAUSE_NAMES.iter().enumerate() {
            if self.causes & (1 << i) != 0 {
                let _ = write!(s, " cause={name}");
            }
        }
        let [uploads, pending, retired, evicted, queued, pool, inflight, wait] = self.values;
        let _=write!(s," uploads={uploads} pending={pending} retired_bytes={retired} evicted_bytes={evicted} retirement_pending={queued} pool_bytes={pool} inflight={inflight} drawable_wait_ms={:.3}",wait as f64/1e6);
        let [b, eviction_bytes, eviction_buffers, retirement_bytes, retirement_buffers, unit] =
            self.maintenance;
        let _=write!(s," B={b} eviction_service_bytes={eviction_bytes} eviction_buffers={eviction_buffers} retirement_service_bytes={retirement_bytes} retirement_buffers={retirement_buffers} largest_unit={unit}");
        let [missing, staged, list, shader, wanted] = self.admission;
        let _=write!(s," missing_items={missing} staged_items={staged} first_missing_list={list} first_missing_shader={shader} first_missing_wanted={wanted} category_bytes(Roofs,Walls,Labels,Outlines,Background,Structure,Code,Other)={:?}",self.category_bytes);
        s
    }
}
fn report(
    records: &mut VecDeque<Record>,
    last_glass: &mut Option<Snapshot>,
    reported_drops: &mut u64,
    reported_logs: &mut u64,
) {
    let mut causes = [0u64; CAUSES];
    let mut requests = 0;
    let mut glasses = Vec::new();
    let mut unconfirmed = 0usize;
    let mut uploads = 0;
    let mut retired = 0;
    let mut evicted = 0;
    let mut upload_max = 0;
    let mut maintenance_max = [0; 6];
    let mut staged_max = 0;
    let mut category_bytes = [0; 8];
    for r in records.iter_mut() {
        let mut s = Snapshot::take(&r.frame);
        let commit = s.times[Stage::Commit as usize];
        let returned = s.times[Stage::CommitReturned as usize];
        if commit != 0
            && (if returned == 0 { now() } else { returned }).saturating_sub(commit) > 1_000_000
        {
            s.causes |= 1 << Cause::CommitBlocked as usize;
        }
        for (i, count) in causes.iter_mut().enumerate() {
            if s.causes & !r.reported_causes & (1 << i) != 0 {
                *count += 1;
            }
        }
        r.reported_causes |= s.causes;
        if !r.reported_request {
            requests += 1;
            r.reported_request = true;
        }
        // A request can straddle the reporting boundary. Read its counters
        // only after the UI's release-published end stamp makes them final.
        if !r.reported_work && s.times[Stage::End as usize] != 0 {
            r.reported_work = true;
            uploads += s.values[0];
            retired += s.values[2];
            evicted += s.values[3];
            upload_max = upload_max.max(s.values[0]);
            staged_max = staged_max.max(s.admission[1]);
            for (total, value) in category_bytes.iter_mut().zip(s.category_bytes) {
                *total += value;
            }
            for (max, value) in maintenance_max.iter_mut().zip(s.maintenance) {
                *max = (*max).max(value);
            }
        }
        if s.glass() != 0 && !r.reported_glass {
            r.reported_glass = true;
            glasses.push(s);
        } else if s.glass() == 0 && !r.reported_glass && r.frame.unconfirmed.load(Ordering::Acquire) != 0 {
            r.reported_glass = true;
            unconfirmed += 1;
        }
    }
    glasses.sort_unstable_by_key(Snapshot::glass);
    let mut bins = [0; 8];
    let edges = [8.0, 10.0, 16.0, 25.0, 33.0, 40.0, 100.0];
    let mut worst = None;
    let mut max_gap = 0.0;
    let mut idle_gap: f64 = 0.0;
    for s in &glasses {
        if let Some(prev) = last_glass.as_ref().filter(|p| p.glass() < s.glass()) {
            let gap = (s.glass() - prev.glass()) as f64 / 1e6;
            // A deliberate rest is reported separately, never as a missed
            // animation frame. A request within two display periods starts
            // an active interval even if none subsequently reaches glass.
            let active = records.iter().any(|r| {
                let t = r.frame.times[Stage::Request as usize].load(Ordering::Acquire);
                t > prev.times[Stage::Request as usize] && t <= prev.glass() + 33_000_000
            });
            if active {
                bins[edges
                    .iter()
                    .position(|edge| gap < *edge)
                    .unwrap_or(edges.len())] += 1;
                if gap > max_gap {
                    max_gap = gap;
                    worst = Some((prev.clone(), s.clone()));
                }
            } else {
                idle_gap = idle_gap.max(gap);
            }
        }
        if last_glass.as_ref().is_none_or(|p| s.glass() > p.glass()) {
            *last_glass = Some(s.clone());
        }
    }
    let drops = DROPPED.load(Ordering::Relaxed);
    let logs = crate::log::dropped_log_records();
    let dropped = drops.saturating_sub(*reported_drops);
    let log_dropped = logs.saturating_sub(*reported_logs);
    *reported_drops = drops;
    *reported_logs = logs;
    if requests == 0
        && glasses.is_empty()
        && unconfirmed == 0
        && causes.iter().all(|n| *n == 0)
        && dropped == 0
        && log_dropped == 0
        && uploads == 0
        && retired == 0
        && evicted == 0
    {
        return;
    }
    let mut line=format!("n={} n_unconfirmed={unconfirmed} requests={requests} max_gap={max_gap:.3}ms idle_gap={idle_gap:.3}ms histogram(<8,<10,<16,<25,<33,<40,<100,100+)={bins:?} uploads={uploads} retired_bytes={retired} evicted_bytes={evicted} trace_dropped={dropped} log_dropped={log_dropped}",glasses.len());
    for (i, count) in causes.iter().enumerate() {
        let _ = write!(line, " cause={}:{}", CAUSE_NAMES[i], count);
    }
    let [b, eviction_bytes, eviction_buffers, retirement_bytes, retirement_buffers, unit] =
        maintenance_max;
    let _=write!(line," upload_frame_max={upload_max} B={b} eviction_frame_max={eviction_bytes} eviction_buffers_max={eviction_buffers} retirement_frame_max={retirement_bytes} retirement_buffers_max={retirement_buffers} largest_unit={unit}");
    let _=write!(line," staged_items_max={staged_max} category_bytes(Roofs,Walls,Labels,Outlines,Background,Structure,Code,Other)={category_bytes:?}");
    crate::trace!("present", "{line}");
    if let Some((prev, next)) = worst {
        let origin = prev.times[Stage::Request as usize];
        let mut timeline = format!(
            "worst_gap={max_gap:.3}ms epoch_ns={origin}\n  {}",
            prev.line(origin)
        );
        for r in records.iter() {
            let s = Snapshot::take(&r.frame);
            if s.times[0] > origin && s.times[0] <= next.times[0] {
                let _ = write!(timeline, "\n  {}", s.line(origin));
            }
        }
        crate::trace!("present", "{timeline}");
    }
}
