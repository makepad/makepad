//! Immutable instance publications. Recording pins CPU storage; renderer-owned
//! buffers remain subject to each backend's real submission/completion law.
use std::{
    collections::{BTreeMap, HashMap},
    ops::Range,
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc, OnceLock,
    },
};

/// A one-time measurement of this machine's copy rate: the bytes one
/// 2 ms slice copies, reported by the atlas settle line as `probe=`. It
/// gates nothing — every owed copy is made in its frame (contract §7);
/// pacing is the producer's (`upload_pacing`, §8). Measured through the
/// touched destination pages, not a hot cache line.
pub fn retained_upload_limit() -> usize {
    const PROBE_BYTES: usize = 4 * 1024 * 1024;
    let source = vec![0x5au8; PROBE_BYTES];
    let mut destination = vec![0u8; PROBE_BYTES];
    let start = std::time::Instant::now();
    destination.copy_from_slice(std::hint::black_box(&source));
    std::hint::black_box(&destination);
    let nanos = start.elapsed().as_nanos().max(1);
    ((PROBE_BYTES as u128 * 2_000_000 / nanos).min(PROBE_BYTES as u128) as usize) & !3
}

#[derive(Clone, Copy, Debug, Default)]
pub struct RetainedUploadStats {
    pub bytes: usize,
    pub instances_uploaded: usize,
    pub install_us: u64,
    /// Immediate re-records whose bytes matched the resident copy: no upload.
    pub identical_skips: usize,
}

/// One frame's physical retirement allowance, in bytes: a Metal buffer
/// cannot be freed in pieces, so an oversized unit runs alone and is
/// reported. All passes share the same counter; a second call in the frame
/// grants no credit.
#[derive(Debug, Default, Clone, Copy)]
pub struct RetainedMaintenance {
    pub bytes: usize,
    pub buffers: usize,
    pub largest_unit: usize,
}
impl RetainedMaintenance {
    pub fn can_admit(&self, bytes: usize, limit: usize) -> bool {
        bytes <= limit.saturating_sub(self.bytes) || self.buffers == 0
    }
    pub fn admit(&mut self, bytes: usize, limit: usize) -> bool {
        if !self.can_admit(bytes, limit) { return false; }
        self.bytes = self.bytes.saturating_add(bytes);
        self.buffers += 1;
        self.largest_unit = self.largest_unit.max(bytes);
        true
    }
}

#[derive(Debug)]
pub struct RetainedUploadBudget {
    frame: Option<u64>,
    install_ns: u128,
    total_install_ns: u128,
    /// The copy probe's bytes-per-slice, a diagnostic the atlas prints
    /// (`upload_allowance=`); it bounds the retirement drain, never a copy.
    pub limit: usize,
    pub stats: RetainedUploadStats,
    pub totals: RetainedUploadStats,
    pub pending_bytes: usize,
    /// Retained items detached from the GPU so far (`evict_retained_item`):
    /// an application whose recorded content must stay resident re-walks
    /// its owners when this moves, never every frame.
    pub evictions: u64,
    pub upload_pending_max: usize,
    pub starved_frames: u64,
    pub max_frame_bytes: usize,
    pub max_install_us: u64,
    pub allocations: RetainedAllocationBudget,
    pub recordings: crate::recording_buffer::RecordingBudget,
    pub retirements: Arc<std::sync::atomic::AtomicUsize>,
    pub retirement_queued: Arc<std::sync::atomic::AtomicBool>,
    pub retirement_frame: Option<u64>,
    /// This frame's retirement allowance (the drain of buffers the GPU is
    /// done with): bounded by bytes derived from the copy probe.
    pub retirement: RetainedMaintenance,
    /// Publication ids whose backing lease a draw item released outside a
    /// collection (`detach_shared`, a consumer's eviction); the backend drops
    /// each lease at its next upload phase (O(released), no walk).
    pub backing_releases: Vec<u64>,
}

/// One process allowance, shared by Scope's prepared cache and the platform.
/// Preparation is peak headroom above the CPU cache, not resident recording
/// data. Physical GPU probes may further constrain the GPU share.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetainedMemoryBudgets {
    pub cpu: usize,
    pub preparation: usize,
    pub recording: usize,
    pub gpu: usize,
}
impl RetainedMemoryBudgets {
    pub fn from_process(process: usize) -> Self {
        let cpu = process / 8;
        Self { cpu, preparation: cpu / 3, recording: process / 16, gpu: process / 4 }
    }
}

/// Physical backing allocations, including allocator rounding, spare buffers
/// and buffers whose final CPU owner disappeared before command completion.
/// The UI alone owns the inventory. Allocation workers receive only leases;
/// neither admission nor completion polling takes a cross-thread lock.
#[derive(Debug)]
pub struct RetainedAllocationBudget {
    limit: usize,
    device_limit: Option<usize>,
    evicted_bytes: usize,
    bytes: usize,
    records: Vec<Arc<RetainedAllocationRecord>>,
    released: Arc<std::sync::atomic::AtomicUsize>,
    metadata_disposals: Vec<Arc<RetainedAllocationRecord>>,
    collect_cursor: usize,
    collected_frame: Option<u64>,
    collected_backlog_frame: Option<u64>,
    refusals: usize,
    cache_pressure: std::cell::Cell<bool>,
    pending_backend_retirements: usize,
}
#[derive(Debug)]
pub(crate) struct RetainedAllocationRecord {
    bytes: usize,
    submitted: AtomicU64,
    owners: std::sync::atomic::AtomicUsize,
    released: Arc<std::sync::atomic::AtomicUsize>,
}
#[derive(Debug)]
pub struct RetainedAllocation(Arc<RetainedAllocationRecord>);
impl Clone for RetainedAllocation {
    fn clone(&self) -> Self {
        self.0.owners.fetch_add(1, Ordering::Relaxed);
        Self(self.0.clone())
    }
}
impl Drop for RetainedAllocation {
    fn drop(&mut self) {
        if self.0.owners.fetch_sub(1, Ordering::AcqRel) == 1 {
            // Announce before dropping the last lease Arc. The UI collector
            // cannot remove this record until that Arc is gone.
            self.0.released.fetch_add(1, Ordering::Release);
        }
    }
}
impl RetainedAllocation {
    pub fn submitted(&self, serial: u64) {
        self.0.submitted.fetch_max(serial, Ordering::Release);
    }
    pub fn last_submission(&self) -> u64 {
        self.0.submitted.load(Ordering::Acquire)
    }
    pub fn unique_owner(&self) -> bool {
        self.0.owners.load(Ordering::Acquire) == 1
    }
}

/// Retained storage gets a share of the device working set, independently of
/// the CPU publication cache. Unified GPUs share the process's RAM allowance.
pub fn retained_device_envelope(recommended: u64, physical: u64, unified: bool) -> usize {
    let allowance = if unified || recommended == 0 {
        physical / 2
    } else {
        recommended
    };
    (allowance / 4).min(usize::MAX as u64) as usize
}

/// A bounded margin of completed buffers, shared across publication owners.
/// Prepare class inventories at startup/on a worker. Default is empty so
/// moving a backend state with mem::take never allocates on the UI thread.
pub struct RetainedBufferPool<P> {
    classes: [Vec<P>; usize::BITS as usize],
    bytes: usize,
    pub reuses: u64,
}
impl<P> Default for RetainedBufferPool<P> {
    fn default() -> Self {
        Self {
            classes: std::array::from_fn(|_| Vec::new()),
            bytes: 0,
            reuses: 0,
        }
    }
}
impl<P> RetainedBufferPool<P> {
    pub fn new(limit: usize) -> Self {
        Self {
            classes: std::array::from_fn(|class| {
                let count = if class < 8 {
                    0
                } else {
                    ((limit / 16) >> class).min(16_384)
                };
                Vec::with_capacity(count)
            }),
            ..Self::default()
        }
    }
    pub fn bytes(&self) -> usize {
        self.bytes
    }
    pub fn put(&mut self, bytes: usize, payload: P, limit: usize) -> Result<(), P> {
        if bytes < 256
            || !bytes.is_power_of_two()
            || bytes > (limit / 16).saturating_sub(self.bytes)
        {
            return Err(payload);
        }
        let class = &mut self.classes[bytes.trailing_zeros() as usize];
        if class.len() == class.capacity() {
            return Err(payload);
        }
        class.push(payload);
        self.bytes += bytes;
        Ok(())
    }
    pub fn take(&mut self, bytes: usize) -> Option<P> {
        let bytes = bytes.max(256).checked_next_power_of_two()?;
        let first = bytes.trailing_zeros() as usize;
        for class in first..=(first + 1).min(self.classes.len() - 1) {
            if let Some(payload) = self.classes[class].pop() {
                self.bytes -= 1usize << class;
                self.reuses += 1;
                return Some(payload);
            }
        }
        None
    }
    pub fn drain_one(&mut self) -> Option<P> {
        for (class, items) in self.classes.iter_mut().enumerate().rev() {
            if let Some(payload) = items.pop() {
                self.bytes -= 1usize << class;
                return Some(payload);
            }
        }
        None
    }
    pub fn largest_buffer_bytes(&self) -> Option<usize> {
        self.classes.iter().enumerate().rev().find(|(_, items)| !items.is_empty()).map(|(class, _)| 1usize << class)
    }
}
impl RetainedAllocationBudget {
    pub fn new(limit: usize) -> Self {
        Self {
            limit,
            device_limit: None,
            evicted_bytes: 0,
            bytes: 0,
            records: Vec::new(),
            released: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            metadata_disposals: Vec::with_capacity(4096),
            collect_cursor: 0,
            collected_frame: None,
            collected_backlog_frame: None,
            refusals: 0,
            cache_pressure: std::cell::Cell::new(false),
            pending_backend_retirements: 0,
        }
    }
    pub fn set_limit(&mut self, limit: usize) {
        self.limit = limit;
    }
    /// The renderer owns physical residency independently of a CPU cache's
    /// requested publication limit. Tests may explicitly supply a device cap.
    pub fn set_device_limit(&mut self, bytes: usize) {
        self.device_limit = Some(bytes);
    }
    pub fn has_device_limit(&self) -> bool {
        self.device_limit.is_some()
    }
    pub fn limit(&self) -> usize {
        self.device_limit.unwrap_or(self.limit)
    }
    pub fn evicted_bytes(&self) -> usize {
        self.evicted_bytes
    }
    pub fn evicted(&mut self, bytes: usize) {
        self.evicted_bytes = self.evicted_bytes.saturating_add(bytes);
    }
    pub fn pressure_high(&self) -> usize { self.limit() / 4 * 3 }
    pub fn pressure_low(&self) -> usize { self.limit() / 8 * 5 }
    pub fn has_pending_retirements(&self) -> bool {
        self.pending_backend_retirements != 0
            || self.released.load(Ordering::Acquire) != 0
            || !self.metadata_disposals.is_empty()
    }
    /// Backend-owned receipt-waiting storage participates in the same settle
    /// obligation as worker disposal and released allocation accounts.
    pub fn set_pending_backend_retirements(&mut self, count: usize) {
        self.pending_backend_retirements = count;
    }
    pub(crate) fn take_metadata_disposal(&mut self) -> Option<Arc<RetainedAllocationRecord>> {
        self.metadata_disposals.pop()
    }
    pub fn record_count(&self) -> usize {
        self.records.len()
    }
    pub fn bytes(&self) -> usize {
        self.bytes
    }
    pub fn refusals(&self) -> usize {
        self.refusals
    }
    /// Accounting only (the producer's own eviction policy reads it): the
    /// resident bytes are above three quarters of the limit, with a Schmitt
    /// band down to five eighths. Nothing in the platform evicts on it.
    pub fn reclaim_needed(&self) -> bool {
        if self.bytes > self.pressure_high() {
            self.cache_pressure.set(true);
        } else if self.bytes <= self.pressure_low() {
            self.cache_pressure.set(false);
        }
        self.cache_pressure.get()
    }
    pub fn reserve(&mut self, bytes: usize) -> Option<RetainedAllocation> {
        if bytes > self.limit().saturating_sub(self.bytes) {
            self.refusals = self.refusals.saturating_add(1);
            return None;
        }
        Some(self.reserve_visible(bytes))
    }
    /// The charge a draw item's own backing takes whatever the limit says: a
    /// refused allocation was a hole (residency by construction). The
    /// producer's policy brings the total back under its limit.
    pub fn reserve_visible(&mut self, bytes: usize) -> RetainedAllocation {
        let record = Arc::new(RetainedAllocationRecord {
            bytes,
            submitted: AtomicU64::new(0),
            owners: std::sync::atomic::AtomicUsize::new(1),
            released: self.released.clone(),
        });
        self.records.push(record.clone());
        self.bytes += bytes;
        RetainedAllocation(record)
    }
    pub fn collect(&mut self, completed: u64) {
        for _ in 0..self.records.len().min(64) {
            // A full worker-disposal inventory retains the record for retry.
            // Never free its last Arc (or grow this queue) on the UI thread.
            if self.metadata_disposals.len() == self.metadata_disposals.capacity() {
                break;
            }
            if self.collect_cursor >= self.records.len() {
                self.collect_cursor = 0;
            }
            let record = &self.records[self.collect_cursor];
            if Arc::strong_count(record) == 1
                && record.submitted.load(Ordering::Acquire) <= completed
            {
                self.bytes -= record.bytes;
                self.released.fetch_sub(1, Ordering::AcqRel);
                self.metadata_disposals
                    .push(self.records.swap_remove(self.collect_cursor));
            } else {
                self.collect_cursor += 1;
            }
        }
    }
    /// Extra bounded sweeps while released records are queued. Every upload of
    /// a draw item's instances leaves one released record behind and a pan
    /// re-uploads hundreds of items per frame, so one 64-record sweep per
    /// frame lets the backlog (and the retirement repaints it asks for) outlast
    /// the interaction by tens of seconds. Each call stays within `collect`'s
    /// bound; `max_calls` caps the frame's total.
    pub fn collect_backlog(&mut self, frame: u64, completed: u64, max_calls: usize) {
        if self.collected_backlog_frame == Some(frame) {
            return;
        }
        self.collected_backlog_frame = Some(frame);
        for _ in 0..max_calls {
            if self.released.load(Ordering::Acquire) == 0 {
                break;
            }
            self.collect(completed);
        }
    }
    pub fn collect_for_frame(&mut self, frame: u64, completed: u64) {
        if self.collected_frame == Some(frame) {
            return;
        }
        self.collected_frame = Some(frame);
        self.collect(completed);
    }
}

impl Default for RetainedUploadBudget {
    fn default() -> Self {
        Self::new(retained_upload_limit())
    }
}

impl RetainedUploadBudget {
    pub fn new(limit: usize) -> Self {
        Self {
            frame: None,
            install_ns: 0,
            total_install_ns: 0,
            limit,
            stats: Default::default(),
            totals: Default::default(),
            pending_bytes: 0,
            evictions: 0,
            upload_pending_max: 0,
            starved_frames: 0,
            max_frame_bytes: 0,
            max_install_us: 0,
            allocations: RetainedAllocationBudget::new(usize::MAX),
            recordings: Default::default(),
            retirements: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            retirement_queued: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            retirement_frame: None,
            retirement: RetainedMaintenance::default(),
            backing_releases: Vec::new(),
        }
    }

    pub fn begin_frame(&mut self, frame: u64) {
        if self.frame == Some(frame) {
            return;
        }
        if self.frame.is_some() && self.pending_bytes != 0 && self.stats.bytes == 0 {
            self.starved_frames = self.starved_frames.saturating_add(1);
        }
        self.upload_pending_max = self.upload_pending_max.max(self.pending_bytes);
        self.frame = Some(frame);
        self.install_ns = 0;
        self.stats = Default::default();
        self.retirement = RetainedMaintenance::default();
        self.pending_bytes = 0;
    }

    /// A backend reports every copy it made — whole copies, every frame.
    pub fn copied(&mut self, bytes: usize, stride_bytes: usize, elapsed: std::time::Duration) {
        let instances = bytes / stride_bytes;
        self.install_ns += elapsed.as_nanos();
        self.total_install_ns += elapsed.as_nanos();
        for stats in [&mut self.stats, &mut self.totals] {
            stats.bytes += bytes;
            stats.instances_uploaded += instances;
        }
        // Round after summing: thousands of sub-microsecond copies must not
        // disappear from the frame's install time.
        self.stats.install_us = (self.install_ns / 1000).min(u64::MAX as u128) as u64;
        self.totals.install_us = (self.total_install_ns / 1000).min(u64::MAX as u128) as u64;
        self.max_frame_bytes = self.max_frame_bytes.max(self.stats.bytes);
        self.max_install_us = self.max_install_us.max(self.stats.install_us);
    }
}

/// Segment payload ownership across current, queued, and backend-retained
/// publications. Clones and prefix views share one counted owner. These are
/// payload-owner bytes, not process RSS: separately wrapping the same external
/// Arc creates distinct owners, and external Arc references can outlive them.
#[derive(Clone, Copy, Debug, Default)]
pub struct InstanceSegmentMemoryStats {
    pub live_bytes: usize,
    pub peak_bytes: usize,
    pub live_segments: usize,
    pub peak_segments: usize,
}
static INSTANCE_SEGMENT_BYTES: AtomicUsize = AtomicUsize::new(0);
static INSTANCE_SEGMENT_PEAK_BYTES: AtomicUsize = AtomicUsize::new(0);
static INSTANCE_SEGMENTS: AtomicUsize = AtomicUsize::new(0);
static INSTANCE_SEGMENT_PEAK_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Lock-free diagnostic counters. Individual atomic values may reflect adjacent
/// instants while workers publish or retire; no renderer waits for a snapshot.
pub fn instance_segment_memory_stats() -> InstanceSegmentMemoryStats {
    InstanceSegmentMemoryStats {
        live_bytes: INSTANCE_SEGMENT_BYTES.load(Ordering::Relaxed),
        peak_bytes: INSTANCE_SEGMENT_PEAK_BYTES.load(Ordering::Relaxed),
        live_segments: INSTANCE_SEGMENTS.load(Ordering::Relaxed),
        peak_segments: INSTANCE_SEGMENT_PEAK_COUNT.load(Ordering::Relaxed),
    }
}

#[derive(Debug)]
struct InstanceSegmentStorage {
    id: u64,
    data: Arc<[f32]>,
}
impl Drop for InstanceSegmentStorage {
    fn drop(&mut self) {
        INSTANCE_SEGMENT_BYTES.fetch_sub(std::mem::size_of_val(&*self.data), Ordering::Relaxed);
        INSTANCE_SEGMENTS.fetch_sub(1, Ordering::Relaxed);
    }
}

/// An immutable piece of an instance stream. Clone this handle when republishing
/// a group: identity, CPU storage, and the corresponding GPU bytes stay reusable.
/// A prefix view keeps the same identity and only shortens the visible range.
#[derive(Clone, Debug)]
pub struct InstanceSegment {
    storage: Arc<InstanceSegmentStorage>,
    len: usize,
}
impl InstanceSegment {
    pub fn new(data: Arc<[f32]>) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let bytes = std::mem::size_of_val(&*data);
        let len = data.len();
        let storage = Arc::new(InstanceSegmentStorage {
            id: NEXT.fetch_add(1, Ordering::Relaxed),
            data,
        });
        let live_bytes = INSTANCE_SEGMENT_BYTES.fetch_add(bytes, Ordering::Relaxed) + bytes;
        let live_segments = INSTANCE_SEGMENTS.fetch_add(1, Ordering::Relaxed) + 1;
        INSTANCE_SEGMENT_PEAK_BYTES.fetch_max(live_bytes, Ordering::Relaxed);
        INSTANCE_SEGMENT_PEAK_COUNT.fetch_max(live_segments, Ordering::Relaxed);
        Self { storage, len }
    }
    pub fn id(&self) -> u64 {
        self.storage.id
    }
    pub fn data(&self) -> &[f32] {
        &self.storage.data[..self.len]
    }
    pub fn float_len(&self) -> usize {
        self.len
    }
    pub fn byte_len(&self) -> usize {
        self.len * 4
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

/// A copy from the previous complete GPU publication to the next one.
/// Offsets and lengths are in float slots, not bytes or instance records.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstanceCopy {
    pub source: Range<usize>,
    pub destination: usize,
}

/// Complete coverage of a new publication, in two disjoint classes: reusable
/// previous GPU ranges and changed CPU ranges. Copy ranges always refer to the
/// previous immutable snapshot. A backend must not overwrite that snapshot
/// while it is submitted, nor apply overlapping relocation copies in-place.
/// Use a separate target allocation for relocation, retaining the source until
/// its copies and earlier readers have completed. Publication becomes drawable
/// only once every range is ready; this contract never drops missing segments.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InstanceUploadPlan {
    pub copies: Vec<InstanceCopy>,
    pub writes: Vec<Range<usize>>,
}
impl InstanceUploadPlan {
    /// No old bytes need to move. The backend must additionally establish
    /// capacity and its own submission/completion rules before patching.
    pub fn can_update_in_place(&self) -> bool {
        self.copies
            .iter()
            .all(|copy| copy.source.start == copy.destination)
    }
    pub fn copy_float_len(&self) -> usize {
        self.copies.iter().map(|copy| copy.source.len()).sum()
    }
    pub fn write_float_len(&self) -> usize {
        self.writes.iter().map(|range| range.len()).sum()
    }
    fn copy(&mut self, source: Range<usize>, destination: usize) {
        if source.is_empty() {
            return;
        }
        if let Some(last) = self.copies.last_mut() {
            if last.source.end == source.start
                && last.destination + last.source.len() == destination
            {
                last.source.end = source.end;
                return;
            }
        }
        self.copies.push(InstanceCopy {
            source,
            destination,
        });
    }
    fn write(&mut self, range: Range<usize>) {
        if range.is_empty() {
            return;
        }
        if let Some(last) = self.writes.last_mut() {
            if last.end == range.start {
                last.end = range.end;
                return;
            }
        }
        self.writes.push(range);
    }
}

/// Segment identity for `upload_plan`: the storage allocation's address.
fn storage_key(segment: &InstanceSegment) -> usize {
    Arc::as_ptr(&segment.storage) as usize
}

/// Addresses are already unique; one multiply spreads them over the table's
/// control bits. `upload_plan` fills a map of thousands of these per call.
#[derive(Default)]
struct StorageKeyHasher(u64);
impl std::hash::Hasher for StorageKeyHasher {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.0 = (self.0 ^ byte as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        }
    }
    fn write_usize(&mut self, value: usize) {
        self.0 = (value as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    }
}
type StorageMap =
    HashMap<usize, (usize, usize), std::hash::BuildHasherDefault<StorageKeyHasher>>;

#[derive(Clone, Debug)]
pub struct RetainedInstances(Arc<InstancePublication>);
/// Observes storage held by independently retained draw recordings without
/// extending its lifetime or confusing it with a CodeView publication lease.
#[derive(Clone, Debug)]
pub struct WeakRetainedInstances {
    publication: std::sync::Weak<InstancePublication>,
    bytes: usize,
}
impl WeakRetainedInstances {
    pub fn live_bytes(&self) -> usize {
        if self.publication.strong_count() == 0 {
            0
        } else {
            self.bytes
        }
    }
}
#[derive(Debug)]
struct InstancePublication {
    id: u64,
    slots: usize,
    segments: Arc<[InstanceSegment]>,
    offsets: Arc<[usize]>,
    len: usize,
    // Only legacy contiguous consumers materialize this. Backend paths use
    // data_slices, so publication, append and replacement never copy a prefix.
    contiguous: OnceLock<Arc<[f32]>>,
    // Legacy id-only suffix receipts, bounded by the current segment count.
    // These store no publication/payload owners and form no ancestor chain.
    append_checkpoints: Arc<[(u64, usize)]>,
    // Prefix recordings still own the producer's entire CPU allocation.
    // Preserve its weak accounting receipt until the last view is retired.
    source: Option<Arc<InstancePublication>>,
}

pub struct InstanceDataSlices<'a> {
    publication: &'a InstancePublication,
    range: Range<usize>,
    index: usize,
}
impl<'a> Iterator for InstanceDataSlices<'a> {
    type Item = (usize, &'a [f32]);
    fn next(&mut self) -> Option<Self::Item> {
        if self.range.is_empty() {
            return None;
        }
        let segment = self.publication.segments.get(self.index)?;
        let offset = self.publication.offsets[self.index];
        let start = offset.max(self.range.start);
        let end = (offset + segment.len).min(self.range.end);
        if start >= end {
            return None;
        }
        self.index += 1;
        Some((start, &segment.data()[start - offset..end - offset]))
    }
}

impl RetainedInstances {
    /// Worker-side construction. No UI copy and no mutable alias of the payload.
    pub fn new(slots: usize, data: Arc<[f32]>) -> Result<Self, &'static str> {
        let result = Self::from_segments(slots, vec![InstanceSegment::new(data.clone())])?;
        // Preserve the original Arc for existing shared-instance publishers,
        // including the empty-publication case.
        let _ = result.0.contiguous.set(data);
        Ok(result)
    }
    /// Publish an ordered segment directory. Only metadata is inspected/copied;
    /// each segment must contain whole instance records of this stride.
    pub fn from_segments(
        slots: usize,
        mut segments: Vec<InstanceSegment>,
    ) -> Result<Self, &'static str> {
        if slots == 0 || segments.iter().any(|segment| segment.len % slots != 0) {
            return Err("invalid instance stride");
        }
        segments.retain(|segment| !segment.is_empty());
        let mut offsets = Vec::with_capacity(segments.len());
        let mut len = 0usize;
        for segment in &segments {
            offsets.push(len);
            len = len
                .checked_add(segment.len)
                .ok_or("instance stream too large")?;
        }
        if len > usize::MAX / 4 {
            return Err("instance stream too large");
        }
        let contiguous = OnceLock::new();
        if segments.len() == 1 {
            let _ = contiguous.set(segments[0].storage.data.clone());
        }
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Ok(Self(Arc::new(InstancePublication {
            id: NEXT.fetch_add(1, Ordering::Relaxed),
            slots,
            segments: segments.into(),
            offsets: offsets.into(),
            len,
            contiguous,
            append_checkpoints: Arc::from([]),
            source: None,
        })))
    }
    pub fn id(&self) -> u64 {
        self.0.id
    }
    pub fn downgrade(&self) -> WeakRetainedInstances {
        WeakRetainedInstances {
            publication: Arc::downgrade(&self.0),
            bytes: self.byte_len(),
        }
    }
    /// Compatibility for id-only append uploads. New backends retain the last
    /// publication and use upload_plan, which also handles replacement/reorder.
    pub fn can_continue_upload(&self, previous: u64, copied_bytes: usize) -> bool {
        copied_bytes <= self.byte_len()
            && (self.id() == previous
                || self.upload_since(previous).start.saturating_mul(4) >= copied_bytes)
    }
    /// Append copies only the caller's new suffix into immutable storage. The
    /// directory reuses every old segment; no CPU prefix or ancestor is copied.
    pub fn append(&self, suffix: &[f32]) -> Result<Self, &'static str> {
        if suffix.len() % self.slots() != 0 {
            return Err("invalid instance stride");
        }
        if suffix.is_empty() {
            return Ok(self.clone());
        }
        let mut segments = self.0.segments.to_vec();
        segments.push(InstanceSegment::new(Arc::from(suffix)));
        let mut result = Self::from_segments(self.slots(), segments)?;
        let mut checkpoints = self.0.append_checkpoints.to_vec();
        checkpoints.push((self.id(), self.float_len()));
        Arc::get_mut(&mut result.0).unwrap().append_checkpoints = checkpoints.into();
        Ok(result)
    }
    pub fn upload_since(&self, resident: u64) -> Range<usize> {
        if self.id() == resident {
            return self.float_len()..self.float_len();
        }
        match self
            .0
            .append_checkpoints
            .iter()
            .find(|(id, _)| *id == resident)
        {
            Some((_, len)) => *len..self.float_len(),
            None => 0..self.float_len(),
        }
    }
    pub fn slots(&self) -> usize {
        self.0.slots
    }
    pub fn segments(&self) -> &[InstanceSegment] {
        &self.0.segments
    }
    pub fn float_len(&self) -> usize {
        self.0.len
    }
    /// Iterate only the segment slices intersecting a global float range.
    /// Each item carries its destination offset in the complete publication.
    pub fn data_slices(&self, range: Range<usize>) -> InstanceDataSlices<'_> {
        assert!(range.start <= range.end && range.end <= self.float_len());
        let index = self
            .0
            .offsets
            .partition_point(|offset| *offset <= range.start)
            .saturating_sub(1);
        InstanceDataSlices {
            publication: &self.0,
            range,
            index,
        }
    }
    /// Diff immutable identities and lengths, never the float payloads. A
    /// repeated segment may reuse any valid previous occurrence; an occurrence
    /// at the same offset is preferred when it covers the reusable range.
    ///
    /// Identity is the storage allocation: an id is minted once per
    /// `InstanceSegmentStorage`, so two segments share an id exactly when they
    /// share the `Arc`. Comparing the pointers keeps this off the heap, which
    /// matters because renderers call it for every changed buffer every frame.
    /// A large map republishes thousands of segments of which a handful moved;
    /// reading each id through its `Arc` and SipHashing it into a map cost
    /// about 0.1 to 1.5 ms per plan. Segments that kept their slot and offset
    /// now settle with three compares. The few that did not are looked up by
    /// scanning pointers, and only a publication that moved many builds the
    /// by-identity map, keyed by pointer with a multiplicative hash.
    pub fn upload_plan(&self, previous: &Self) -> InstanceUploadPlan {
        let mut result = InstanceUploadPlan::default();
        if self.slots() != previous.slots() {
            result.write(0..self.float_len());
            return result;
        }
        if self.id() == previous.id() {
            result.copy(0..self.float_len(), 0);
            return result;
        }
        let previous_segments_list = &previous.0.segments;
        let previous_offsets = &previous.0.offsets;
        let mut previous_segments: Option<StorageMap> = None;
        const SCANNED_LOOKUPS: usize = 8;
        let mut unplaced = 0usize;
        for (index, (segment, &destination)) in
            self.0.segments.iter().zip(self.0.offsets.iter()).enumerate()
        {
            // Same storage, same offset, and at least as long as before: the
            // bytes are already in place, whichever other occurrences exist.
            if let (Some(before), Some(&before_offset)) =
                (previous_segments_list.get(index), previous_offsets.get(index))
            {
                if before_offset == destination
                    && before.len >= segment.len
                    && Arc::ptr_eq(&before.storage, &segment.storage)
                {
                    result.copy(destination..destination + segment.len, destination);
                    continue;
                }
            }
            // The longest previous occurrence of this storage, the first one
            // winning a tie. The first few lookups scan the list by pointer,
            // which is cheaper than hashing thousands of entries to answer a
            // handful of questions; a publication that moved more than that
            // pays for the map once.
            unplaced += 1;
            let longest = if unplaced <= SCANNED_LOOKUPS {
                let mut longest: Option<(usize, usize)> = None;
                for (before, &offset) in previous_segments_list.iter().zip(previous_offsets.iter())
                {
                    if Arc::ptr_eq(&before.storage, &segment.storage)
                        && longest.is_none_or(|(_, len)| before.len > len)
                    {
                        longest = Some((offset, before.len));
                    }
                }
                longest
            } else {
                previous_segments
                    .get_or_insert_with(|| {
                        let mut map = StorageMap::with_capacity_and_hasher(
                            previous_segments_list.len(),
                            Default::default(),
                        );
                        for (before, &offset) in
                            previous_segments_list.iter().zip(previous_offsets.iter())
                        {
                            let longest: &mut (usize, usize) =
                                map.entry(storage_key(before)).or_insert((offset, before.len));
                            if before.len > longest.1 {
                                *longest = (offset, before.len);
                            }
                        }
                        map
                    })
                    .get(&storage_key(segment))
                    .copied()
            };
            let Some((mut source, previous_len)) = longest else {
                result.write(destination..destination + segment.len);
                continue;
            };
            let reusable = previous_len.min(segment.len);
            if let Ok(index) = previous_offsets.binary_search(&destination) {
                let at_destination = &previous_segments_list[index];
                if Arc::ptr_eq(&at_destination.storage, &segment.storage)
                    && at_destination.len >= reusable
                {
                    source = destination;
                }
            }
            result.copy(source..source + reusable, destination);
            result.write(destination + reusable..destination + segment.len);
        }
        result
    }
    /// Legacy contiguous access. Single-segment publications return their
    /// original Arc. Multiple segments are explicitly materialized on first
    /// use and cached. Do not call this from a renderer/upload hot path: use
    /// data_slices instead to preserve incremental publication and memory use.
    pub fn shared_data(&self) -> Arc<[f32]> {
        self.contiguous_data().clone()
    }
    /// Legacy contiguous access, with the same materialization cost described
    /// by shared_data. Backend uploads and metadata queries must use slices and
    /// float_len; this compatibility accessor is not an incremental upload API.
    pub fn data(&self) -> &[f32] {
        &self.contiguous_data()[..self.float_len()]
    }
    fn contiguous_data(&self) -> &Arc<[f32]> {
        self.0.contiguous.get_or_init(|| {
            let mut data = Vec::with_capacity(self.float_len());
            for segment in self.segments() {
                data.extend_from_slice(segment.data());
            }
            data.into()
        })
    }
    /// A view of an immutable CPU prefix. Only segment descriptors are copied;
    /// the producer's entire storage remains pinned for existing accounting.
    pub fn prefix(&self, count: usize) -> Self {
        let len = count.min(self.count()) * self.slots();
        if len == self.float_len() {
            return self.clone();
        }
        let mut remaining = len;
        let mut segments = Vec::new();
        for segment in self.segments() {
            if remaining == 0 {
                break;
            }
            let mut part = segment.clone();
            part.len = part.len.min(remaining);
            remaining -= part.len;
            segments.push(part);
        }
        let mut result = Self::from_segments(self.slots(), segments).unwrap();
        let view = Arc::get_mut(&mut result.0).unwrap();
        view.source = Some(self.0.source.as_ref().unwrap_or(&self.0).clone());
        // A prefix is a new view, not a historical append receipt.
        result
    }
    pub fn count(&self) -> usize {
        self.float_len() / self.slots()
    }
    pub fn byte_len(&self) -> usize {
        self.float_len() * 4
    }
    pub fn readers(&self) -> usize {
        Arc::strong_count(&self.0)
    }
    /// Bounded upload ranges, in float slots; never split an instance record.
    pub fn upload_ranges(&self, max_bytes: usize) -> impl Iterator<Item = Range<usize>> + '_ {
        let batch = (max_bytes / (self.slots() * 4)).max(1) * self.slots();
        (0..self.float_len())
            .step_by(batch)
            .map(move |start| start..start.saturating_add(batch).min(self.float_len()))
    }
}

/// Byte LRU for immutable publications. Pins and retained draw readers prevent
/// eviction. Retired entries stay charged until the caller's *actual* renderer
/// completion serial covers their last submission. WebGL must not advance it.
#[derive(Default)]
pub struct RetainedInstancePool {
    entries: BTreeMap<u64, PoolEntry>,
    clock: u64,
    budget: usize,
    bytes: usize,
}
struct PoolEntry {
    value: RetainedInstances,
    touched: u64,
    pins: usize,
    submitted: u64,
}
impl RetainedInstancePool {
    pub fn new(budget: usize) -> Self {
        Self {
            budget,
            ..Self::default()
        }
    }
    pub fn bytes(&self) -> usize {
        self.bytes
    }
    pub fn set_budget(&mut self, bytes: usize) {
        self.budget = bytes;
    }
    pub fn admit(
        &mut self,
        value: RetainedInstances,
        completed: u64,
    ) -> Result<(), RetainedInstances> {
        self.clock += 1;
        if let Some(entry) = self.entries.get_mut(&value.id()) {
            entry.touched = self.clock;
            return Ok(());
        }
        while self.bytes.saturating_add(value.byte_len()) > self.budget {
            let victim = self
                .entries
                .iter()
                .filter(|(_, e)| e.pins == 0 && e.value.readers() == 1 && e.submitted <= completed)
                .min_by_key(|(_, e)| e.touched)
                .map(|(&id, _)| id);
            let Some(id) = victim else {
                return Err(value);
            };
            self.bytes -= self.entries.remove(&id).unwrap().value.byte_len();
        }
        self.bytes += value.byte_len();
        self.entries.insert(
            value.id(),
            PoolEntry {
                value,
                touched: self.clock,
                pins: 0,
                submitted: 0,
            },
        );
        Ok(())
    }
    pub fn get(&mut self, id: u64) -> Option<RetainedInstances> {
        self.clock += 1;
        let entry = self.entries.get_mut(&id)?;
        entry.touched = self.clock;
        Some(entry.value.clone())
    }
    pub fn pin(&mut self, id: u64, pin: bool) {
        if let Some(e) = self.entries.get_mut(&id) {
            if pin {
                e.pins += 1;
            } else {
                e.pins = e.pins.saturating_sub(1);
            }
        }
    }
    pub fn submitted(&mut self, id: u64, serial: u64) {
        if let Some(e) = self.entries.get_mut(&id) {
            e.submitted = e.submitted.max(serial);
        }
    }
}

#[cfg(test)]
mod upload_tests {
    use super::*;

    #[test]
    fn machine_residency_pressure_has_fractional_hysteresis() {
        for physical in [8u64 << 30, 128u64 << 30] {
            let limit = retained_device_envelope(physical * 9 / 10, physical, true);
            assert_eq!(limit, physical as usize / 8);
            let mut budget = RetainedAllocationBudget::new(limit);
            assert_eq!(budget.pressure_high(), limit * 3 / 4);
            assert_eq!(budget.pressure_low(), limit * 5 / 8);
            let near = budget.reserve(limit * 11 / 16).unwrap();
            assert!(!budget.reclaim_needed(), "below 3/4 is a cache, not garbage");
            let crossing = budget.reserve(limit / 8).unwrap();
            assert!(budget.reclaim_needed());
            drop(crossing);
            budget.collect(0);
            assert!(budget.reclaim_needed(), "hold pressure through the hysteresis band");
            drop(near);
            budget.collect(0);
            assert!(!budget.reclaim_needed());
        }
    }

    #[test]
    fn append_publications_preserve_a_partially_copied_prefix() {
        let a = RetainedInstances::new(4, vec![1.0; 4096].into()).unwrap();
        let b = a.append(&[2.0; 1024]).unwrap();
        let c = b.append(&[3.0; 1024]).unwrap();
        let replacement = RetainedInstances::new(4, vec![9.0; 6144].into()).unwrap();
        let copied = 2048;
        assert!(b.can_continue_upload(a.id(), copied));
        assert!(c.can_continue_upload(a.id(), copied));
        assert!(c.can_continue_upload(b.id(), b.byte_len()));
        assert!(!a.can_continue_upload(b.id(), copied));
        assert!(!replacement.can_continue_upload(a.id(), copied));
        let mut destination = vec![0.0; c.data().len()];
        destination[..copied / 4].copy_from_slice(&a.data()[..copied / 4]);
        destination[copied / 4..].copy_from_slice(&c.data()[copied / 4..]);
        assert_eq!(destination.as_slice(), c.data());
    }

    #[test]
    fn fifty_mib_copies_are_bounded_complete_and_frame_shared() {
        let publication =
            RetainedInstances::new(16, vec![0.25; 50 * 1024 * 1024 / 4].into()).unwrap();
        let mut copied = vec![0.0; publication.data().len()];
        let mut budget = RetainedUploadBudget::default();
        let mut frames = 0;
        // The whole block is copied in the frame that owes it (contract §7):
        // one copy, complete, counted once. The bounded, frame-shared pacing
        // the old allowance provided is the producer's through the async
        // publication facility (`upload_pacing`, §8).
        {
            budget.begin_frame(frames);
            let bytes = publication.byte_len();
            let start = std::time::Instant::now();
            copied.copy_from_slice(publication.data());
            budget.copied(bytes, 64, start.elapsed());
            budget.begin_frame(frames);
            assert_eq!(budget.stats.bytes, bytes);
            assert_eq!(budget.stats.instances_uploaded * 64, budget.stats.bytes);
            frames += 1;
        }
        assert_eq!(frames, 1);
        assert_eq!(copied.as_slice(), publication.data());
        assert_eq!(budget.totals.bytes, publication.byte_len());
        assert_eq!(budget.max_frame_bytes, publication.byte_len());
        let mut short = RetainedUploadBudget::new(1024);
        short.begin_frame(99);
        assert!(short.retirement.admit(1024, short.limit));
        short.begin_frame(99);
        assert!(!short.retirement.admit(1, short.limit), "another pass cannot renew maintenance credit");
        short.begin_frame(100);
        assert!(short.retirement.admit(4096, short.limit), "indivisible oversized allocation must eventually retire");
        assert!(!short.retirement.admit(1, short.limit), "oversized allocation must run alone");
        assert_eq!(short.retirement.largest_unit, 4096);
        for frame in 0..2 {
            short.begin_frame(frame);
            for _ in 0..4 {
                short.copied(64, 64, std::time::Duration::from_nanos(250));
            }
            assert_eq!(
                short.stats.install_us, 1,
                "count every short copy before rounding"
            );
            assert_eq!(short.totals.install_us, frame + 1);
        }
        let mut allocations = RetainedAllocationBudget::new(128);
        let first = allocations.reserve(128).unwrap();
        let spare = first.clone();
        first.submitted(7);
        assert!(allocations.reserve(1).is_none());
        assert!(
            allocations.reclaim_needed(),
            "a refused allocation must request spare reclamation"
        );
        drop(first);
        allocations.collect(7);
        assert_eq!(
            allocations.bytes(),
            128,
            "a spare still owns physical capacity"
        );
        drop(spare);
        allocations.collect(6);
        assert_eq!(
            allocations.bytes(),
            128,
            "command completion gates physical retirement"
        );
        allocations.collect(7);
        assert_eq!(allocations.bytes(), 0);
        assert!(
            !allocations.reclaim_needed(),
            "recovery ends pressure without raising the budget"
        );
        let mut stalled = RetainedUploadBudget::new(4 << 20);
        stalled.begin_frame(0);
        stalled.pending_bytes = 6 << 20;
        stalled.begin_frame(1);
        assert_eq!(stalled.upload_pending_max, 6 << 20);
        assert_eq!(stalled.starved_frames, 1);
        stalled.begin_frame(1);
        assert_eq!(
            stalled.starved_frames, 1,
            "multiple passes do not count a frame twice"
        );
        stalled.pending_bytes = 2 << 20;
        stalled.copied(4 << 20, 64, std::time::Duration::ZERO);
        stalled.begin_frame(2);
        assert_eq!(
            stalled.starved_frames, 1,
            "a partial copy makes real progress"
        );
        let cpu = crate::recording_buffer::RecordingBudget::new(128);
        let mut recording =
            crate::recording_buffer::RecordingBuffer::with_capacity_in(16, cpu.clone());
        recording.extend_from_slice(&[1.0; 16]);
        assert_eq!(cpu.bytes(), 64);
        recording.extend_from_slice(&[2.0; 16]);
        assert!(
            recording.refused(),
            "replacement admission includes the live old capacity"
        );
        assert_eq!(
            recording.len(),
            16,
            "refusal preserves the previous recording bytes"
        );
        assert_eq!(cpu.refusals(), 1);
        assert_eq!(cpu.refusal_reasons(), [1, 0, 0]);
        assert_eq!(cpu.last_refusal(), (128, 64));
        let empty = cpu
            .reserve(0)
            .expect("an empty retained link does not contend for capacity");
        drop(empty);
        assert_eq!(cpu.refusals(), cpu.refusal_reasons().iter().sum());
        recording.clear();
        assert_eq!(
            cpu.bytes(),
            64,
            "clearing length does not free backing capacity"
        );
        drop(recording);
        assert_eq!(
            cpu.bytes(),
            0,
            "retirement releases the charge with the buffer"
        );
        let recording = crate::recording_buffer::RecordingBuffer::with_capacity_in(32, cpu.clone());
        assert!(!recording.refused());
        assert_eq!(cpu.bytes(), 128);
        drop(recording);
        let replacement = allocations.reserve(128).unwrap();
        let ui_reader = std::rc::Rc::new(());
        let storage = vec![ui_reader.clone(); 1024];
        let empty = crate::recording_buffer::EmptyAllocation::from_vec(storage);
        assert_eq!(
            std::rc::Rc::strong_count(&ui_reader),
            1,
            "UI-only elements are destroyed before their allocation leaves the UI"
        );
        std::thread::spawn(move || drop(empty)).join().unwrap();
        drop(crate::recording_buffer::EmptyAllocation::from_vec(vec![
            ();
            1024
        ]));
        drop(crate::recording_buffer::EmptyAllocation::from_vec(
            Vec::<u8>::new(),
        ));
        assert_eq!(
            allocations.bytes(),
            128,
            "capacity recovery admits the replacement"
        );
        drop(replacement);
        allocations.collect(7);
        assert_eq!(allocations.bytes(), 0);
        allocations.set_limit(1024);
        for _ in 0..1024 {
            drop(allocations.reserve(1).unwrap());
        }
        allocations.collect(7);
        assert_eq!(
            allocations.bytes(),
            960,
            "one collection services at most 64 allocation records"
        );
        for _ in 0..15 {
            allocations.collect(7);
        }
        assert_eq!(
            allocations.bytes(),
            0,
            "bounded collection eventually visits every retired allocation"
        );
    }
}
