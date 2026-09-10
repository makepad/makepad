//! Immutable instance publications. Recording pins CPU storage; renderer-owned
//! buffers remain subject to each backend's real submission/completion law.
use std::{
    collections::BTreeMap,
    ops::Range,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

/// One allowance for all retained/page copies in a presented frame, shared by
/// every pass. The startup probe includes touched destination pages, rather
/// than measuring a copy into an already hot cache line.
pub const MAX_RETAINED_UPLOAD_BYTES: usize = 4 * 1024 * 1024;

pub fn retained_upload_limit() -> usize {
    static LIMIT: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *LIMIT.get_or_init(|| {
        let source = vec![0x5au8; MAX_RETAINED_UPLOAD_BYTES];
        let mut destination = vec![0u8; MAX_RETAINED_UPLOAD_BYTES];
        let start = std::time::Instant::now();
        destination.copy_from_slice(std::hint::black_box(&source));
        std::hint::black_box(&destination);
        let nanos = start.elapsed().as_nanos().max(1);
        ((MAX_RETAINED_UPLOAD_BYTES as u128 * 2_000_000 / nanos)
            .min(MAX_RETAINED_UPLOAD_BYTES as u128) as usize)
            & !3
    })
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(usize)]
pub enum UploadCategory {
    Roofs,
    Walls,
    Labels,
    Outlines,
    Background,
    Structure,
    Code,
    #[default]
    Other,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct RetainedUploadStats {
    pub bytes: usize,
    pub instances_uploaded: usize,
    pub install_us: u64,
    pub category_bytes: [usize; 8],
    pub category_instances: [usize; 8],
}

/// One frame's physical maintenance allowance. A Metal buffer cannot be
/// freed in pieces: an oversized unit runs alone, and is explicitly reported.
/// All passes share the same counters; calling service twice grants no credit.
#[derive(Debug, Default, Clone, Copy)]
pub struct RetainedMaintenance {
    pub bytes: usize,
    pub buffers: usize,
    pub largest_unit: usize,
}
impl RetainedMaintenance {
    pub const MAX_BUFFERS: usize = 64;
    pub fn can_admit(&self, bytes: usize, buffers: usize, limit: usize) -> bool {
        buffers <= Self::MAX_BUFFERS.saturating_sub(self.buffers)
            && (bytes <= limit.saturating_sub(self.bytes) || self.buffers == 0)
    }
    pub fn admit(&mut self, bytes: usize, buffers: usize, limit: usize) -> bool {
        if !self.can_admit(bytes, buffers, limit) { return false; }
        self.bytes = self.bytes.saturating_add(bytes);
        self.buffers += buffers;
        self.largest_unit = self.largest_unit.max(bytes);
        true
    }
}

#[derive(Debug)]
pub struct RetainedUploadBudget {
    frame: Option<u64>,
    install_ns: u128,
    total_install_ns: u128,
    pub limit: usize,
    pub stats: RetainedUploadStats,
    pub totals: RetainedUploadStats,
    pub pending_bytes: usize,
    pub upload_pending_max: usize,
    pub starved_frames: u64,
    pub max_frame_bytes: usize,
    pub max_install_us: u64,
    pub allocations: RetainedAllocationBudget,
    pub recordings: crate::recording_buffer::RecordingBudget,
    pub demand_epoch: u64,
    pub working_set: Vec<bool>,
    pub working_set_valid: bool,
    pub working_set_scan_passes: u8,
    /// The backend exhausted off-demand victims and completed retirement.
    pub allow_visible_overflow: bool,
    pub retirements: Arc<std::sync::atomic::AtomicUsize>,
    pub retirement_queued: Arc<std::sync::atomic::AtomicBool>,
    pub retirement_frame: Option<u64>,
    pub eviction: RetainedMaintenance,
    pub retirement: RetainedMaintenance,
    pub eviction_cursor: usize,
    pub eviction_item_cursor: usize,
    pub eviction_scanned: usize,
    pub eviction_items_scanned: usize,
    pub eviction_cycle_has_victims: bool,
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
    waits: usize,
    bytes: usize,
    records: Vec<Arc<RetainedAllocationRecord>>,
    released: Arc<std::sync::atomic::AtomicUsize>,
    metadata_disposals: Vec<Arc<RetainedAllocationRecord>>,
    collect_cursor: usize,
    collected_frame: Option<u64>,
    refusals: usize,
    reclaim_bytes: usize,
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
            waits: 0,
            bytes: 0,
            records: Vec::new(),
            released: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            metadata_disposals: Vec::with_capacity(4096),
            collect_cursor: 0,
            collected_frame: None,
            refusals: 0,
            reclaim_bytes: 0,
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
    /// Existing Metal small-allocation threshold, shared with the recorder.
    pub fn sync_allocation_bytes(&self) -> usize {
        (self.limit() / 512).clamp(1 << 20, 16 << 20)
    }
    pub fn evicted_bytes(&self) -> usize {
        self.evicted_bytes
    }
    pub fn evicted(&mut self, bytes: usize) {
        self.evicted_bytes = self.evicted_bytes.saturating_add(bytes);
    }
    pub fn waits(&self) -> usize {
        self.waits
    }
    pub fn reclaim_bytes(&self) -> usize {
        self.reclaim_bytes
    }
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
    pub fn can_reserve(&self, bytes: usize) -> bool {
        bytes <= self.limit().saturating_sub(self.bytes)
    }
    /// Deferral while reclaiming is counted but is not an allocation refusal.
    /// The caller retains the publication and retries after completion.
    pub fn wait_for_reclaim(&mut self, bytes: usize) {
        self.waits = self.waits.saturating_add(1);
        self.reclaim_bytes = self.reclaim_bytes.max(bytes);
    }
    pub fn bytes(&self) -> usize {
        self.bytes
    }
    pub fn refusals(&self) -> usize {
        self.refusals
    }
    pub fn reclaim_needed(&self) -> bool {
        self.reclaim_bytes > self.limit().saturating_sub(self.bytes)
    }
    pub fn take_reclaim_request(&mut self) -> bool {
        let needed = self.reclaim_needed();
        self.reclaim_bytes = 0;
        needed
    }
    pub fn reserve(&mut self, bytes: usize) -> Option<RetainedAllocation> {
        if bytes > self.limit().saturating_sub(self.bytes) {
            self.refusals = self.refusals.saturating_add(1);
            self.reclaim_bytes = self.reclaim_bytes.max(bytes);
            return None;
        }
        Some(self.reserve_visible(bytes))
    }
    /// A soft device target cannot veto the current visible working set after
    /// every off-demand victim has retired. Also supplies emergency small-draw
    /// service while bulk retirement is in flight. Actual driver failure still
    /// retains the caller's pending publication for retry.
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
            limit: limit.min(MAX_RETAINED_UPLOAD_BYTES),
            stats: Default::default(),
            totals: Default::default(),
            pending_bytes: 0,
            upload_pending_max: 0,
            starved_frames: 0,
            max_frame_bytes: 0,
            max_install_us: 0,
            allocations: RetainedAllocationBudget::new(usize::MAX),
            recordings: Default::default(),
            demand_epoch: 0,
            working_set: Vec::new(),
            working_set_valid: false,
            working_set_scan_passes: 0,
            allow_visible_overflow: false,
            retirements: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            retirement_queued: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            retirement_frame: None,
            eviction: RetainedMaintenance::default(),
            retirement: RetainedMaintenance::default(),
            eviction_cursor: 0,
            eviction_item_cursor: 0,
            eviction_scanned: 0,
            eviction_items_scanned: 0,
            eviction_cycle_has_victims: false,
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
        self.eviction = RetainedMaintenance::default();
        self.retirement = RetainedMaintenance::default();
        self.eviction_scanned = 0;
        self.eviction_items_scanned = 0;
        self.pending_bytes = 0;
    }

    /// A backend copies precisely this range, then reports the actual copy.
    /// No rounding up, including when one complete record does not fit.
    pub fn range(&self, remaining: Range<usize>, stride_bytes: usize) -> Range<usize> {
        assert!(stride_bytes > 0);
        let available = self.limit.saturating_sub(self.stats.bytes);
        let bytes = remaining.len().min(available) / stride_bytes * stride_bytes;
        remaining.start..remaining.start + bytes
    }

    pub fn copied(
        &mut self,
        bytes: usize,
        stride_bytes: usize,
        category: UploadCategory,
        elapsed: std::time::Duration,
    ) {
        assert!(bytes <= self.limit.saturating_sub(self.stats.bytes));
        let instances = bytes / stride_bytes;
        self.install_ns += elapsed.as_nanos();
        self.total_install_ns += elapsed.as_nanos();
        for stats in [&mut self.stats, &mut self.totals] {
            stats.bytes += bytes;
            stats.instances_uploaded += instances;
            stats.category_bytes[category as usize] += bytes;
            stats.category_instances[category as usize] += instances;
        }
        // Round after summing: thousands of sub-microsecond copies must not
        // disappear from the frame's install time.
        self.stats.install_us = (self.install_ns / 1000).min(u64::MAX as u128) as u64;
        self.totals.install_us = (self.total_install_ns / 1000).min(u64::MAX as u128) as u64;
        self.max_frame_bytes = self.max_frame_bytes.max(self.stats.bytes);
        self.max_install_us = self.max_install_us.max(self.stats.install_us);
    }
}

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
    parent: Option<Arc<InstanceStamp>>,
    slots: usize,
    data: Arc<[f32]>,
    len: usize,
    // Prefix recordings still own the producer's entire CPU allocation.
    // Preserve its weak accounting receipt until the last view is retired.
    source: Option<Arc<InstancePublication>>,
}
#[derive(Debug)]
struct InstanceStamp {
    id: u64,
    len: usize,
    parent: Option<Arc<InstanceStamp>>,
}
impl RetainedInstances {
    /// Worker-side construction. No UI copy and no mutable alias of the payload.
    pub fn new(slots: usize, data: Arc<[f32]>) -> Result<Self, &'static str> {
        if slots == 0 || data.len() % slots != 0 {
            return Err("invalid instance stride");
        }
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Ok(Self(Arc::new(InstancePublication {
            id: NEXT.fetch_add(1, Ordering::Relaxed),
            parent: None,
            slots,
            len: data.len(),
            data,
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
    /// A pending copy can follow an append without restarting its immutable
    /// prefix. Unrelated replacements must never inherit partially copied data.
    pub fn can_continue_upload(&self, previous: u64, copied_bytes: usize) -> bool {
        copied_bytes <= self.byte_len()
            && (self.id() == previous
                || self.upload_since(previous).start.saturating_mul(4) >= copied_bytes)
    }
    /// Worker-side append. Existing records are immutable. Backends may upload
    /// only the suffix when their last resident publication is an ancestor.
    pub fn append(&self, suffix: &[f32]) -> Result<Self, &'static str> {
        if suffix.len() % self.slots() != 0 {
            return Err("invalid instance stride");
        }
        let mut data = Vec::with_capacity(self.data().len() + suffix.len());
        data.extend_from_slice(self.data());
        data.extend_from_slice(suffix);
        let mut result = Self::new(self.slots(), data.into())?;
        Arc::get_mut(&mut result.0).unwrap().parent = Some(Arc::new(InstanceStamp {
            id: self.id(),
            len: self.data().len(),
            parent: self.0.parent.clone(),
        }));
        Ok(result)
    }
    pub fn upload_since(&self, resident: u64) -> Range<usize> {
        if self.id() == resident {
            return self.data().len()..self.data().len();
        }
        let mut current = self.0.parent.as_deref();
        while let Some(stamp) = current {
            if stamp.id == resident {
                return stamp.len..self.data().len();
            }
            current = stamp.parent.as_deref();
        }
        0..self.data().len()
    }
    pub fn slots(&self) -> usize {
        self.0.slots
    }
    pub fn data(&self) -> &[f32] {
        &self.0.data[..self.0.len]
    }
    /// A view of an immutable CPU prefix. The backend sees only these records;
    /// creating an LOD view never copies the file's geometry on the UI thread.
    /// Keep the full publication with the producer until retirement.
    pub fn prefix(&self, count: usize) -> Self {
        let len = count.min(self.count()) * self.slots();
        if len == self.0.len { return self.clone(); }
        let mut result = Self::new(self.slots(), self.0.data.clone()).unwrap();
        let view = Arc::get_mut(&mut result.0).unwrap();
        view.len = len;
        view.source = Some(self.0.source.as_ref().unwrap_or(&self.0).clone());
        result
    }
    pub fn count(&self) -> usize {
        self.data().len() / self.slots()
    }
    pub fn byte_len(&self) -> usize {
        std::mem::size_of_val(self.data())
    }
    pub fn readers(&self) -> usize {
        Arc::strong_count(&self.0)
    }
    /// Bounded upload ranges, in float slots; never split an instance record.
    pub fn upload_ranges(&self, max_bytes: usize) -> impl Iterator<Item = Range<usize>> + '_ {
        let batch = (max_bytes / (self.slots() * 4)).max(1) * self.slots();
        (0..self.data().len())
            .step_by(batch)
            .map(move |start| start..(start + batch).min(self.data().len()))
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
        let mut offset = 0;
        let mut frames = 0;
        while offset != publication.byte_len() {
            budget.begin_frame(frames);
            let range = budget.range(offset..publication.byte_len(), 64);
            assert!(!range.is_empty());
            let start = std::time::Instant::now();
            copied[range.start / 4..range.end / 4]
                .copy_from_slice(&publication.data()[range.start / 4..range.end / 4]);
            budget.copied(range.len(), 64, UploadCategory::Code, start.elapsed());
            // A second pass in this same frame cannot replenish the allowance.
            budget.begin_frame(frames);
            assert!(budget.stats.bytes <= budget.limit);
            assert_eq!(budget.stats.instances_uploaded * 64, budget.stats.bytes);
            offset = range.end;
            frames += 1;
        }
        assert!(frames >= 12);
        assert_eq!(copied.as_slice(), publication.data());
        assert_eq!(budget.totals.bytes, publication.byte_len());
        assert_eq!(
            budget.totals.category_bytes[UploadCategory::Code as usize],
            publication.byte_len()
        );
        assert!(budget.max_frame_bytes <= budget.limit);
        let small = RetainedUploadBudget::new(63);
        assert!(
            small.range(0..128, 64).is_empty(),
            "never round an oversized record up"
        );
        let mut short = RetainedUploadBudget::new(1024);
        short.begin_frame(99);
        assert!(short.eviction.admit(1024, 1, short.limit));
        short.begin_frame(99);
        assert!(!short.eviction.admit(1, 1, short.limit), "another pass cannot renew maintenance credit");
        short.begin_frame(100);
        assert!(short.eviction.admit(4096, 1, short.limit), "indivisible oversized allocation must eventually retire");
        assert!(!short.eviction.admit(1, 1, short.limit), "oversized allocation must run alone");
        assert_eq!(short.eviction.largest_unit, 4096);
        for _ in 0..RetainedMaintenance::MAX_BUFFERS {assert!(short.retirement.admit(1, 1, short.limit));}
        assert!(!short.retirement.admit(1, 1, short.limit), "small buffers still obey the handle bound");
        for frame in 0..2 {
            short.begin_frame(frame);
            for _ in 0..4 {
                short.copied(
                    64,
                    64,
                    UploadCategory::Code,
                    std::time::Duration::from_nanos(250),
                );
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
        stalled.copied(4 << 20, 64, UploadCategory::Code, std::time::Duration::ZERO);
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
