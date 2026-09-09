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

#[derive(Debug)]
pub struct RetainedUploadBudget {
    frame: Option<u64>,
    install_ns: u128,
    total_install_ns: u128,
    pub limit: usize,
    pub stats: RetainedUploadStats,
    pub totals: RetainedUploadStats,
    pub pending_bytes: usize,
    pub max_frame_bytes: usize,
    pub max_install_us: u64,
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
            max_frame_bytes: 0,
            max_install_us: 0,
        }
    }

    pub fn begin_frame(&mut self, frame: u64) {
        if self.frame == Some(frame) {
            return;
        }
        self.frame = Some(frame);
        self.install_ns = 0;
        self.stats = Default::default();
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

    pub fn copied(&mut self, bytes: usize, stride_bytes: usize, category: UploadCategory, elapsed: std::time::Duration) {
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
#[derive(Debug)]
struct InstancePublication {
    id: u64,
    parent: Option<Arc<InstanceStamp>>,
    slots: usize,
    data: Arc<[f32]>,
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
            data,
        })))
    }
    pub fn id(&self) -> u64 {
        self.0.id
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
        &self.0.data
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
    fn fifty_mib_copies_are_bounded_complete_and_frame_shared() {
        let publication = RetainedInstances::new(16, vec![0.25; 50 * 1024 * 1024 / 4].into()).unwrap();
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
        assert_eq!(budget.totals.category_bytes[UploadCategory::Code as usize], publication.byte_len());
        assert!(budget.max_frame_bytes <= budget.limit);
        let small = RetainedUploadBudget::new(63);
        assert!(small.range(0..128, 64).is_empty(), "never round an oversized record up");
        let mut short = RetainedUploadBudget::new(1024);
        for frame in 0..2 {
            short.begin_frame(frame);
            for _ in 0..4 {
                short.copied(64, 64, UploadCategory::Code, std::time::Duration::from_nanos(250));
            }
            assert_eq!(short.stats.install_us, 1, "count every short copy before rounding");
            assert_eq!(short.totals.install_us, frame + 1);
        }
    }
}
