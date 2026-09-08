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
