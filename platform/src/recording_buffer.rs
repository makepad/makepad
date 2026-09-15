//! Recording admission is independent of prepared-publication residency.
//! Moving a buffer moves its capacity reservation too. Prepared CPU payloads
//! and their worker reservations have their own account; they are not staging.
use std::{
    ops::{Deref, DerefMut},
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
        Arc,
    },
};

/// An empty vector allocation whose elements have already been destroyed on
/// their owning thread. Only the allocator block travels to a retirement worker.
pub struct EmptyAllocation {
    pointer: std::ptr::NonNull<u8>,
    layout: std::alloc::Layout,
}
// No T values or references survive from_vec. The global allocator permits
// deallocation on another thread, using the original size and alignment.
unsafe impl Send for EmptyAllocation {}
impl EmptyAllocation {
    pub fn from_vec<T>(mut values: Vec<T>) -> Self {
        values.clear();
        let layout = std::alloc::Layout::array::<T>(values.capacity()).unwrap();
        let pointer = std::ptr::NonNull::new(values.as_mut_ptr().cast()).unwrap();
        std::mem::forget(values);
        Self { pointer, layout }
    }
}
impl Drop for EmptyAllocation {
    fn drop(&mut self) {
        if self.layout.size() != 0 {
            // from_vec transfers the unique allocation after dropping all
            // elements; its exact Vec layout is preserved, including alignment.
            unsafe { std::alloc::dealloc(self.pointer.as_ptr(), self.layout) };
        }
    }
}

#[derive(Clone, Debug)]
pub struct RecordingBudget(Arc<BudgetState>);
#[derive(Debug)]
struct BudgetState {
    limit: AtomicUsize,
    resident: Arc<AtomicUsize>,
    reserved: Arc<AtomicUsize>,
    prepared: Arc<AtomicUsize>,
    changes: Arc<StorageChanges>,
    refused: AtomicUsize,
    reasons: [AtomicUsize; 3],
    needed: AtomicUsize,
    available: AtomicUsize,
}
#[derive(Debug, Default)]
struct StorageChanges {
    revision: AtomicU64,
    waiting: AtomicBool,
}
impl StorageChanges {
    fn released(&self) {
        self.revision.fetch_add(1, Ordering::Release);
        if self.waiting.swap(false, Ordering::AcqRel) {
            crate::thread::SignalToUI::set_ui_signal();
        }
    }
}
impl Default for RecordingBudget {
    fn default() -> Self {
        Self::new(usize::MAX)
    }
}
impl RecordingBudget {
    pub fn new(limit: usize) -> Self {
        Self(Arc::new(BudgetState {
            limit: AtomicUsize::new(limit),
            resident: Arc::new(AtomicUsize::new(0)),
            reserved: Arc::new(AtomicUsize::new(0)),
            prepared: Arc::new(AtomicUsize::new(0)),
            changes: Arc::new(StorageChanges::default()),
            refused: AtomicUsize::new(0),
            reasons: std::array::from_fn(|_| AtomicUsize::new(0)),
            needed: AtomicUsize::new(0),
            available: AtomicUsize::new(0),
        }))
    }
    pub fn configure(&self, limit: usize) {
        if self.0.limit.swap(limit, Ordering::AcqRel) < limit {
            self.0.changes.released();
        }
    }
    /// Preparation workers share this counter with the CPU cache, never with
    /// recording vectors. The caller supplies the CPU preparation allowance.
    pub fn reservations(&self) -> Arc<AtomicUsize> {
        self.0.prepared.clone()
    }
    pub fn residency(&self) -> Arc<AtomicUsize> {
        self.0.resident.clone()
    }
    pub fn bytes(&self) -> usize {
        self.0.reserved.load(Ordering::Acquire)
    }
    pub fn limit(&self) -> usize {
        self.0.limit.load(Ordering::Acquire)
    }
    pub fn available(&self) -> usize {
        self.limit().saturating_sub(self.bytes())
    }
    pub fn prepared_bytes(&self) -> usize {
        self.0.prepared.load(Ordering::Acquire)
    }
    pub fn resident_bytes(&self) -> usize {
        self.0.resident.load(Ordering::Acquire)
    }
    pub fn revision(&self) -> u64 {
        self.0.changes.revision.load(Ordering::Acquire)
    }
    /// Arm before observing the revision: a racing release is either included
    /// in this snapshot or sends the UI signal. No timer or polling at rest.
    pub fn watch_releases(&self) -> u64 {
        self.0.changes.waiting.store(true, Ordering::Release);
        self.revision()
    }
    pub fn refusals(&self) -> usize {
        self.0.refused.load(Ordering::Acquire)
    }
    /// Cumulative capacity, contention and allocator failures. Owner-state
    /// counts are separate; their sum is this account's historical refusals.
    pub fn refusal_reasons(&self) -> [usize;3] {
        std::array::from_fn(|i|self.0.reasons[i].load(Ordering::Acquire))
    }
    pub fn last_refusal(&self) -> (usize,usize) {
        (self.0.needed.load(Ordering::Acquire),self.0.available.load(Ordering::Acquire))
    }
    fn refused(&self, reason: usize, needed: usize, available: usize) {
        self.0.reasons[reason].fetch_add(1,Ordering::Relaxed);
        self.0.refused.fetch_add(1,Ordering::Relaxed);
        self.0.needed.store(needed,Ordering::Release);
        self.0.available.store(available,Ordering::Release);
    }
    pub fn reserve(&self, bytes: usize) -> Option<CpuReservation> {
        let available = self.limit();
        match CpuReservation::try_reserve(self.0.reserved.clone(),bytes,available) {
            Ok(mut credit) => {
                credit.changes = Some(self.0.changes.clone());
                Some(credit)
            }
            Err(reason) => {
                self.refused(reason,bytes,available.saturating_sub(self.bytes()));
                None
            }
        }
    }
}
#[derive(Debug)]
pub struct CpuReservation {
    reserved: Arc<AtomicUsize>,
    bytes: usize,
    changes: Option<Arc<StorageChanges>>,
}
impl CpuReservation {
    /// Bounded optimistic admission. Capacity pressure returns immediately;
    /// concurrent counter updates get at most eight atomic comparisons, never
    /// a wait for a producer to finish, a lock, or an until-success loop.
    pub fn reserve(reserved: Arc<AtomicUsize>, bytes: usize, available: usize) -> Option<Self> {
        Self::try_reserve(reserved,bytes,available).ok()
    }
    fn try_reserve(reserved: Arc<AtomicUsize>, bytes: usize, available: usize) -> Result<Self,usize> {
        // Linking an empty/retained stream needs no capacity transaction and
        // cannot fail because another producer changed the shared counter.
        if bytes == 0 { return Ok(Self { reserved,bytes, changes: None }); }
        let mut used = reserved.load(Ordering::Acquire);
        for _ in 0..8 {
            if bytes > available.saturating_sub(used) { return Err(0); }
            match reserved.compare_exchange(used, used + bytes, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => return Ok(Self { reserved, bytes, changes: None }),
                Err(current) => used = current,
            }
        }
        Err(1)
    }
    pub fn bytes(&self) -> usize {
        self.bytes
    }
    /// Release scratch capacity after the producer has destroyed its scratch.
    pub fn shrink_to(&mut self, bytes: usize) {
        assert!(bytes <= self.bytes);
        let released = self.bytes - bytes;
        self.reserved
            .fetch_sub(released, Ordering::AcqRel);
        self.bytes = bytes;
        if released != 0 {
            if let Some(changes) = &self.changes { changes.released(); }
        }
    }
}
impl Drop for CpuReservation {
    fn drop(&mut self) {
        self.reserved.fetch_sub(self.bytes, Ordering::AcqRel);
        if self.bytes != 0 {
            if let Some(changes) = &self.changes { changes.released(); }
        }
    }
}

#[derive(Debug, Default)]
pub struct RecordingBuffer {
    // Field order matters: storage drops before its reservation.
    data: Vec<f32>,
    credit: Option<CpuReservation>,
    budget: Option<RecordingBudget>,
    refused: bool,
}
impl From<Vec<f32>> for RecordingBuffer {
    fn from(data: Vec<f32>) -> Self {
        Self {
            data,
            ..Self::default()
        }
    }
}
impl Deref for RecordingBuffer {
    type Target = [f32];
    fn deref(&self) -> &[f32] {
        &self.data
    }
}
impl DerefMut for RecordingBuffer {
    fn deref_mut(&mut self) -> &mut [f32] {
        &mut self.data
    }
}
impl RecordingBuffer {
    pub fn new(budget: RecordingBudget) -> Self {
        Self {
            budget: Some(budget),
            ..Self::default()
        }
    }
    pub fn with_capacity_in(capacity: usize, budget: RecordingBudget) -> Self {
        let mut buffer = Self::new(budget);
        buffer.reserve(capacity);
        buffer
    }
    pub fn bind_budget(&mut self, budget: &RecordingBudget) -> bool {
        if self
            .budget
            .as_ref()
            .is_some_and(|b| Arc::ptr_eq(&b.0, &budget.0))
        {
            return !self.refused;
        }
        let Some(credit) = budget.reserve(self.data.capacity().saturating_mul(4)) else {
            self.refused = true;
            return false;
        };
        self.credit = Some(credit);
        self.budget = Some(budget.clone());
        true
    }
    pub fn swap_storage(&mut self, other: &mut Self) -> bool {
        if let Some(budget) = &self.budget {
            if !other.bind_budget(budget) {
                return false;
            }
        }
        std::mem::swap(self, other);
        true
    }
    pub fn capacity(&self) -> usize {
        self.data.capacity()
    }
    pub fn as_slice(&self) -> &[f32] {
        &self.data
    }
    pub fn refused(&self) -> bool {
        self.refused
    }
    pub fn clear(&mut self) {
        self.data.clear();
        self.refused = false;
    }
    pub fn truncate(&mut self, len: usize) {
        self.data.truncate(len);
    }
    fn grow(&mut self, wanted: usize) -> bool {
        if self.refused {
            return false;
        }
        if wanted <= self.data.capacity() {
            return true;
        }
        let capacity = wanted.max(self.data.capacity().saturating_mul(2)).max(4);
        let credit = if let Some(budget) = &self.budget {
            let Some(credit) = budget.reserve(capacity.saturating_mul(4)) else {
                self.refused = true;
                return false;
            };
            Some(credit)
        } else {
            None
        };
        let mut data = Vec::new();
        if data.try_reserve_exact(capacity).is_err() {
            if let Some(budget) = &self.budget { budget.refused(2,capacity.saturating_mul(4),0); }
            self.refused = true;
            return false;
        }
        data.extend_from_slice(&self.data);
        // Both old and replacement allocations stay charged during copying.
        let old = std::mem::replace(&mut self.data, data);
        drop(old);
        self.credit = credit;
        true
    }
    pub fn reserve(&mut self, additional: usize) {
        self.grow(self.data.len().saturating_add(additional));
    }
    pub fn reserve_exact(&mut self, additional: usize) {
        self.reserve(additional);
    }
    pub fn extend_from_slice(&mut self, data: &[f32]) {
        if self.grow(self.data.len().saturating_add(data.len())) {
            self.data.extend_from_slice(data);
        }
    }
    pub fn push(&mut self, value: f32) {
        if self.grow(self.data.len().saturating_add(1)) {
            self.data.push(value);
        }
    }
    pub fn resize(&mut self, len: usize, value: f32) {
        if self.grow(len) {
            self.data.resize(len, value);
        }
    }
}
impl Extend<f32> for RecordingBuffer {
    fn extend<T: IntoIterator<Item = f32>>(&mut self, values: T) {
        for value in values {
            self.push(value);
            if self.refused {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::retained_instances::RetainedMemoryBudgets;

    #[test]
    fn eight_gib_prepared_wander_cannot_consume_recording_admission() {
        let pools = RetainedMemoryBudgets::from_process((8usize << 30) / 2);
        assert_eq!(pools.cpu, 512 << 20);
        assert_eq!(pools.recording, 256 << 20);
        assert_eq!(pools.gpu, 1 << 30);
        let budget = RecordingBudget::new(pools.recording);
        let prepared = budget.reservations();
        let residency = budget.residency();
        // Scaled 8,192-file corpus. CPU cache grows past the smaller recording
        // allowance. Upload scratch returns on each receipt; a bounded next
        // preparation is concurrently charged to the preparation allowance.
        let mut peak_recording = 0;
        for file in 0..8192 {
            residency.store((file + 1) * (64 << 10), Ordering::Release);
            let job = CpuReservation::reserve(prepared.clone(), 1 << 20, pools.preparation).unwrap();
            let mut recording = RecordingBuffer::new(budget.clone());
            recording.resize(16 << 10, 1.0);
            assert!(!recording.refused());
            assert_eq!(budget.prepared_bytes(), job.bytes());
            peak_recording = peak_recording.max(budget.bytes());
            assert!(budget.available() > 0);
            let revision = budget.watch_releases();
            // The disposal owns the allocation until after the copy receipt.
            drop(recording);
            assert!(budget.revision() > revision);
            assert_eq!(budget.bytes(), 0);
            drop(job);
        }
        assert_eq!(residency.load(Ordering::Acquire), pools.cpu);
        assert_eq!(peak_recording, 64 << 10);
        assert_eq!(budget.refusals(), 0);
        assert_eq!(budget.prepared_bytes(), 0);
        assert_eq!(budget.available(), pools.recording);
    }
}
