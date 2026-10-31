//! A kernel output: a ring of slots with generations and fences
//! (KERNELS.md §3.5.4a).
//!
//! A slot is in exactly one state:
//! - `Free`: its buffers may be handed to a writer;
//! - `Writing`: one job owns its buffers (they are moved into the
//!   [`WriteLease`], so nothing else can touch them);
//! - `Published`: the slot the draw uses, with its count and generation
//!   (at most one);
//! - `InFlight`: replaced by a newer publish, but a submitted frame still
//!   reads it; it returns to `Free` when that frame's fence has signalled.
//!
//! A writer only ever gets a `Free` slot, so a CPU write never touches
//! memory a frame in flight reads (deferred reclamation: more slots alone
//! would not be enough). When every slot is taken the request is refused
//! and counted as a dropped geometry frame; the last published output keeps
//! drawing.
//!
//! **Initialisation.** [`WriteLease::records`] hands out zero-filled
//! records, so every word of every record in `0..count` (padding included)
//! is written or zero before a publish. Records past `count` are never
//! drawn.
//!
//! **Hot reload.** [`OutputRing::set_layout`] names the new layout (a
//! shader edit made a new layout id). The published old-layout slot keeps
//! drawing until the first publish in the new layout; leases taken under
//! the old layout cannot publish; the old slot retires after its fence.
//! A draw is always one slot, so it is never mixed-layout.
//!
//! **Validation.** A publish checks the record count against the buffer and
//! the indices against the count ([`Topology::validate`]); a failure keeps
//! the previous slot published and reports the error.
//!
//! **High-water.** Slot buffers keep their allocation and are reserved to
//! the largest output seen, so steady state does not reallocate (Metal
//! buffer reallocation is expensive); growth is counted.

use crate::fences::FrameFences;
use crate::topology::{Topology, TopologyError};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotState {
    Free,
    Writing,
    Published,
    InFlight,
}

struct Slot {
    state: SlotState,
    /// The generation of its content (0: never published).
    generation: u64,
    layout: u64,
    stride: u32,
    count: u32,
    /// The last frame serial that reads it (0: never drawn).
    read: u64,
    data: Vec<u32>,
    indices: Vec<u32>,
}

/// Why a publish was refused. The previous published slot keeps drawing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PublishError {
    /// The lease was taken under a layout that has since been replaced.
    StaleLayout { lease: u64, current: u64 },
    /// The lease was taken before [`OutputRing::invalidate`] (its inputs
    /// changed while it ran).
    Stale,
    /// The buffer holds fewer words than `count` records.
    Short { words: usize, count: u32, stride: u32 },
    /// The indices do not fit the topology or the count.
    Topology(TopologyError),
    /// The lease belongs to another ring or was already returned.
    ForeignLease,
}

impl std::fmt::Display for PublishError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PublishError::StaleLayout { lease, current } => write!(f, "written in layout {:016x}, the draw now uses {:016x}", lease, current),
            PublishError::Stale => write!(f, "its inputs changed while it was written"),
            PublishError::Short { words, count, stride } => write!(f, "{} words hold fewer than {} records of {} words", words, count, stride),
            PublishError::Topology(e) => write!(f, "{}", e),
            PublishError::ForeignLease => write!(f, "the lease is not one of this ring's writes"),
        }
    }
}

/// Every slot is taken (writing, published or in flight): the frame's
/// output is dropped and the last published one keeps drawing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RingBusy;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RingStats {
    pub published: u64,
    /// Write requests refused because no slot was free (dropped geometry
    /// frames).
    pub busy: u64,
    /// Publishes refused (stale, short or invalid); the previous slot kept
    /// drawing.
    pub refused: u64,
    /// Layout swaps (hot reloads).
    pub layout_swaps: u64,
    /// Times a slot buffer grew past the high-water mark.
    pub grew: u64,
    /// Most slots in flight at once.
    pub max_in_flight: usize,
}

/// A slot's buffers, owned by one writer until it publishes or abandons.
pub struct WriteLease {
    ring: u64,
    slot: usize,
    ticket: u64,
    layout: u64,
    stride: u32,
    capacity: usize,
    data: Vec<u32>,
    indices: Vec<u32>,
}

impl WriteLease {
    /// The layout the records are written in.
    pub fn layout(&self) -> u64 {
        self.layout
    }

    /// Words per record.
    pub fn stride(&self) -> u32 {
        self.stride
    }

    /// `count` zero-filled records to write.
    pub fn records(&mut self, count: usize) -> &mut [u32] {
        self.data.clear();
        self.data.resize(count * self.stride as usize, 0);
        &mut self.data
    }

    /// The record buffer itself (a compaction target, or moved into a
    /// kernel job as its output and put back with [`Self::set_data`]).
    pub fn take_data(&mut self) -> Vec<u32> {
        std::mem::take(&mut self.data)
    }

    pub fn set_data(&mut self, data: Vec<u32>) {
        self.data = data;
    }

    pub fn data(&self) -> &[u32] {
        &self.data
    }

    pub fn data_mut(&mut self) -> &mut Vec<u32> {
        &mut self.data
    }

    pub fn indices_mut(&mut self) -> &mut Vec<u32> {
        &mut self.indices
    }
}

/// What one frame draws: one slot, one layout.
pub struct DrawView<'a> {
    pub generation: u64,
    pub layout: u64,
    pub stride: u32,
    pub count: u32,
    pub topology: Topology,
    pub data: &'a [u32],
    pub indices: &'a [u32],
}

/// A kernel output buffer ring (see the module docs).
pub struct OutputRing {
    id: u64,
    slots: Vec<Slot>,
    topology: Topology,
    layout: u64,
    stride: u32,
    published: Option<usize>,
    next_ticket: u64,
    stale_before: u64,
    next_generation: u64,
    high_water: usize,
    stats: RingStats,
    last_error: Option<PublishError>,
}

fn ring_id() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

impl OutputRing {
    /// `slots` buffers (2 on the CPU path, 3 with GPU compute) of records of
    /// `stride` words in layout `layout`.
    pub fn new(slots: usize, topology: Topology, layout: u64, stride: u32) -> Self {
        OutputRing {
            id: ring_id(),
            slots: (0..slots.max(2))
                .map(|_| Slot { state: SlotState::Free, generation: 0, layout, stride, count: 0, read: 0, data: Vec::new(), indices: Vec::new() })
                .collect(),
            topology,
            layout,
            stride: stride.max(1),
            published: None,
            next_ticket: 1,
            stale_before: 0,
            next_generation: 1,
            high_water: 0,
            stats: RingStats::default(),
            last_error: None,
        }
    }

    pub fn stats(&self) -> RingStats {
        self.stats
    }

    /// The last refused publish (cleared by the next good one).
    pub fn last_error(&self) -> Option<&PublishError> {
        self.last_error.as_ref()
    }

    pub fn layout(&self) -> u64 {
        self.layout
    }

    pub fn topology(&self) -> Topology {
        self.topology
    }

    pub fn slot_states(&self) -> Vec<SlotState> {
        self.slots.iter().map(|s| s.state).collect()
    }

    /// The published generation (0: nothing yet).
    pub fn generation(&self) -> u64 {
        self.published.map_or(0, |k| self.slots[k].generation)
    }

    /// Returns every in-flight slot whose last frame has completed to
    /// `Free`.
    pub fn collect(&mut self, fences: &dyn FrameFences) {
        let done = fences.completed();
        for s in &mut self.slots {
            if s.state == SlotState::InFlight && s.read <= done {
                s.state = SlotState::Free;
            }
        }
    }

    /// A free slot's buffers for one writer, or [`RingBusy`] (counted).
    pub fn begin_write(&mut self, fences: &dyn FrameFences) -> Result<WriteLease, RingBusy> {
        self.collect(fences);
        // The least recently published free slot, so buffers rotate.
        let Some(k) = (0..self.slots.len()).filter(|&k| self.slots[k].state == SlotState::Free).min_by_key(|&k| self.slots[k].generation) else {
            self.stats.busy += 1;
            return Err(RingBusy);
        };
        let s = &mut self.slots[k];
        s.state = SlotState::Writing;
        let mut data = std::mem::take(&mut s.data);
        data.clear();
        if data.capacity() < self.high_water {
            data.reserve(self.high_water);
        }
        let mut indices = std::mem::take(&mut s.indices);
        indices.clear();
        let ticket = self.next_ticket;
        self.next_ticket += 1;
        Ok(WriteLease { ring: self.id, slot: k, ticket, layout: self.layout, stride: self.stride, capacity: data.capacity(), data, indices })
    }

    /// Returns a lease unpublished (its job was cancelled or failed).
    pub fn abandon(&mut self, lease: WriteLease) {
        if lease.ring != self.id {
            return;
        }
        let s = &mut self.slots[lease.slot];
        if s.state == SlotState::Writing {
            s.state = SlotState::Free;
            s.data = lease.data;
            s.indices = lease.indices;
        }
    }

    /// Publishes `count` records of the lease: validated, then the draw's.
    /// On an error the slot goes back to `Free` and the previous publish
    /// keeps drawing.
    pub fn publish(&mut self, mut lease: WriteLease, count: u32, fences: &dyn FrameFences) -> Result<u64, PublishError> {
        if lease.ring != self.id || self.slots.get(lease.slot).is_none_or(|s| s.state != SlotState::Writing) {
            return Err(PublishError::ForeignLease);
        }
        let words = count as usize * lease.stride as usize;
        let check = if lease.layout != self.layout {
            Err(PublishError::StaleLayout { lease: lease.layout, current: self.layout })
        } else if lease.ticket < self.stale_before {
            Err(PublishError::Stale)
        } else if lease.data.len() < words {
            Err(PublishError::Short { words: lease.data.len(), count, stride: lease.stride })
        } else {
            self.topology.validate(count, &lease.indices).map_err(PublishError::Topology)
        };
        if lease.data.capacity() > lease.capacity {
            self.stats.grew += 1;
        }
        // Records past the count are never drawn.
        lease.data.truncate(words);
        self.high_water = self.high_water.max(lease.data.capacity().max(words));
        let k = lease.slot;
        {
            let s = &mut self.slots[k];
            s.data = lease.data;
            s.indices = lease.indices;
        }
        if let Err(e) = check {
            self.slots[k].state = SlotState::Free;
            self.stats.refused += 1;
            self.last_error = Some(e.clone());
            return Err(e);
        }
        self.collect(fences);
        let done = fences.completed();
        if let Some(old) = self.published.take() {
            let o = &mut self.slots[old];
            o.state = if o.read > done { SlotState::InFlight } else { SlotState::Free };
            if o.layout != lease.layout {
                self.stats.layout_swaps += 1;
            }
        }
        let generation = self.next_generation;
        self.next_generation += 1;
        let s = &mut self.slots[k];
        s.state = SlotState::Published;
        s.generation = generation;
        s.layout = lease.layout;
        s.stride = lease.stride;
        s.count = count;
        s.read = 0;
        self.published = Some(k);
        self.stats.published += 1;
        let in_flight = self.slots.iter().filter(|s| s.state == SlotState::InFlight).count();
        self.stats.max_in_flight = self.stats.max_in_flight.max(in_flight);
        self.last_error = None;
        Ok(generation)
    }

    /// The published slot for the frame about to be submitted (serial
    /// `submitted + 1`); the slot is held until that frame's fence.
    pub fn draw(&mut self, fences: &dyn FrameFences) -> Option<DrawView<'_>> {
        let k = self.published?;
        let serial = fences.submitted() + 1;
        let topology = self.topology;
        let s = &mut self.slots[k];
        s.read = s.read.max(serial);
        Some(DrawView { generation: s.generation, layout: s.layout, stride: s.stride, count: s.count, topology, data: &s.data, indices: &s.indices })
    }

    /// A shader or layout change: later writes use `layout` (`stride` words
    /// per record). The published slot keeps drawing until the first
    /// publish in the new layout; leases taken before cannot publish.
    pub fn set_layout(&mut self, layout: u64, stride: u32) {
        if layout == self.layout && stride.max(1) == self.stride {
            return;
        }
        self.layout = layout;
        self.stride = stride.max(1);
    }

    /// The inputs changed: every lease taken so far is stale (its job is
    /// computing an outdated result) and cannot publish.
    pub fn invalidate(&mut self) {
        self.stale_before = self.next_ticket;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fences::ManualFences;

    fn write(ring: &mut OutputRing, f: &ManualFences, value: u32, n: usize) -> Result<u64, PublishError> {
        let mut lease = ring.begin_write(f).map_err(|_| PublishError::Stale)?;
        for w in lease.records(n) {
            *w = value;
        }
        ring.publish(lease, n as u32, f)
    }

    #[test]
    fn a_slot_in_flight_is_never_written() {
        let f = ManualFences::new();
        let mut ring = OutputRing::new(2, Topology::Instances, 1, 4);
        assert_eq!(write(&mut ring, &f, 1, 8), Ok(1));
        // Frame 1 draws generation 1, and is submitted.
        assert_eq!(ring.draw(&f).unwrap().generation, 1);
        let s1 = f.submit();
        // Generation 2 replaces it; slot 0 is in flight until frame 1's fence.
        assert_eq!(write(&mut ring, &f, 2, 8), Ok(2));
        assert_eq!(ring.slot_states(), vec![SlotState::InFlight, SlotState::Published]);
        assert!(ring.begin_write(&f).is_err(), "no free slot while frame 1 runs");
        assert_eq!(ring.stats().busy, 1);
        f.complete(s1);
        let lease = ring.begin_write(&f).expect("frame 1 completed");
        assert_eq!(lease.slot, 0);
        ring.abandon(lease);
        assert_eq!(ring.slot_states(), vec![SlotState::Free, SlotState::Published]);
    }

    #[test]
    fn a_bad_publish_keeps_the_previous_slot() {
        let f = ManualFences::new();
        let mut ring = OutputRing::new(2, Topology::Triangles { indexed: true }, 1, 3);
        let mut lease = ring.begin_write(&f).unwrap();
        lease.records(3);
        lease.indices_mut().extend_from_slice(&[0, 1, 2]);
        assert_eq!(ring.publish(lease, 3, &f), Ok(1));
        let mut lease = ring.begin_write(&f).unwrap();
        lease.records(3);
        lease.indices_mut().extend_from_slice(&[0, 1, 3]);
        assert!(matches!(ring.publish(lease, 3, &f), Err(PublishError::Topology(_))));
        assert_eq!(ring.generation(), 1);
        assert!(ring.last_error().is_some());
        let mut lease = ring.begin_write(&f).unwrap();
        lease.records(2);
        assert!(matches!(ring.publish(lease, 3, &f), Err(PublishError::Short { .. })));
        assert_eq!(ring.draw(&f).unwrap().indices, &[0, 1, 2]);
    }

    #[test]
    fn a_layout_swap_never_mixes_and_refuses_old_leases() {
        let f = ManualFences::with_delay(2);
        let mut ring = OutputRing::new(3, Topology::Instances, 10, 4);
        write(&mut ring, &f, 1, 4).unwrap();
        let old = ring.begin_write(&f).unwrap();
        ring.set_layout(11, 6);
        // The old layout keeps drawing until the new one publishes.
        let v = ring.draw(&f).unwrap();
        assert_eq!((v.layout, v.stride, v.data.len()), (10, 4, 16));
        f.submit();
        assert!(matches!(ring.publish(old, 4, &f), Err(PublishError::StaleLayout { .. })));
        write(&mut ring, &f, 2, 4).unwrap();
        let v = ring.draw(&f).unwrap();
        assert_eq!((v.layout, v.stride, v.data.len()), (11, 6, 24));
        assert_eq!(ring.stats().layout_swaps, 1);
        ring.invalidate();
        let mut stale = ring.begin_write(&f);
        ring.invalidate();
        let lease = stale.as_mut().map(|l| l.records(1).len()).unwrap();
        assert_eq!(lease, 6);
        assert_eq!(ring.publish(stale.unwrap(), 1, &f), Err(PublishError::Stale));
    }
}
