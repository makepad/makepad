//! Write-once instance regions. No allocation returns bytes an uncompleted frame may read.
//! Bytes beyond an open slab's bump pointer are fresh. A closed slab starts over at zero
//! only after its last reader completes. Destroy/Create names a new backing buffer.

use crate::{
    draw_list::{CxDrawItem, CxDrawItems, ItemRegion},
    retained_instances::RetainedInstances,
    shared_instances::SharedInstances,
};
use std::{collections::HashMap, ops::Range};

const MIN_SLAB: u64 = 64 * 1024;
const GROW_TO: u64 = 4 * 1024 * 1024;
const SPARE_FRAMES: u8 = 120;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RegionRef {
    pub(crate) index: u32,
    pub(crate) gen: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Generation {
    Young,
    Old,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Alloc {
    pub(crate) region: RegionRef,
    pub(crate) slab: u32,
    pub(crate) byte_off: u64,
    pub(crate) first_row: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SlabEvent {
    Create { slab: u32, bytes: u64 },
    Destroy { slab: u32 },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct StoreStats {
    pub(crate) slab_bytes: u64,
    pub(crate) live_bytes: u64,
    pub(crate) evacuated_bytes: u64,
    pub(crate) regions_dropped: u64,
    pub(crate) slabs: u64,
    pub(crate) spare_slabs: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Open,
    Closed,
    Evacuating,
    Spare { idle: u8 },
}

struct Slab {
    size: u64,
    bump: u64,
    live: u64,
    last_read: u64,
    state: State,
    members: Vec<u32>,
}

impl Slab {
    fn placement(&self, bytes: u64, stride: u32) -> Option<(u64, u32)> {
        let row = u32::try_from(self.bump.div_ceil(u64::from(stride))).ok()?;
        let base = u64::from(row) * u64::from(stride);
        (base.checked_add(bytes)? <= self.size).then_some((base, row))
    }
}

struct Region {
    slab: u32,
    range: Range<u64>,
    member: usize,
}

struct Slot {
    gen: u32,
    region: Option<Region>,
}

impl Slot {
    fn vacate(&mut self, index: u32, free: &mut Vec<u32>) {
        self.region = None;
        if let Some(next) = self.gen.checked_add(1) {
            self.gen = next;
            free.push(index);
        }
    }
}

#[derive(Default)]
pub(crate) struct InstanceStore {
    slabs: Vec<Option<Slab>>,
    open: [Option<u32>; 2],
    last_size: [u64; 2],
    regions: Vec<Slot>,
    free_regions: Vec<u32>,
    evacuated_bytes: u64,
    regions_dropped: u64,
    events: Vec<SlabEvent>,
}

impl InstanceStore {
    /// Whether a region of `bytes` gets a slab of its own (nothing else is appended into it).
    pub(crate) fn dedicated(bytes: u64) -> bool {
        bytes > GROW_TO
    }

    fn take_slab(&mut self, size: u64, state: State) -> u32 {
        let spare = self.slabs.iter().position(|s| s.as_ref().is_some_and(|s| {
            s.size == size && matches!(s.state, State::Spare { .. })
        }));
        match spare {
            Some(index) => {
                let slab = self.slabs[index].as_mut().unwrap();
                slab.state = state;
                slab.bump = 0;
                slab.last_read = 0;
                index as u32
            }
            None => {
                let index = self.slabs.iter().position(Option::is_none).unwrap_or(self.slabs.len());
                let id = u32::try_from(index).expect("slab index must fit in u32");
                let slab = Some(Slab { size, bump: 0, live: 0, last_read: 0, state, members: Vec::new() });
                if index == self.slabs.len() {
                    self.slabs.push(slab);
                } else {
                    self.slabs[index] = slab;
                }
                self.events.push(SlabEvent::Create { slab: id, bytes: size });
                id
            }
        }
    }

    fn next_slab(&mut self, bytes: u64, generation: Generation) -> u32 {
        let size = bytes.checked_next_power_of_two().expect("slab size must fit in u64");
        if Self::dedicated(bytes) {
            return self.take_slab(size, State::Closed);
        }
        let g = generation as usize;
        if let Some(id) = self.open[g] {
            self.slabs[id as usize].as_mut().unwrap().state = State::Closed;
        }
        let growth = self.last_size[g].min(GROW_TO / 2) * 2;
        let size = size.max(MIN_SLAB).max(growth);
        let index = self.take_slab(size, State::Open);
        self.open[g] = Some(index);
        self.last_size[g] = size;
        index
    }

    pub(crate) fn alloc(&mut self, bytes: u64, stride: u32, generation: Generation) -> Alloc {
        assert_ne!(stride, 0, "instance rows need a nonzero stride");
        let placement = self.open[generation as usize].and_then(|id| {
            let slab = self.slabs[id as usize].as_ref().unwrap();
            let (off, row) = slab.placement(bytes, stride)?;
            Some((id, off, row))
        });
        let (id, byte_off, first_row) = placement.unwrap_or_else(|| (self.next_slab(bytes, generation), 0, 0));
        let index = self.free_regions.pop().unwrap_or_else(|| {
            let index = u32::try_from(self.regions.len()).expect("region index must fit in u32");
            self.regions.push(Slot { gen: 0, region: None });
            index
        });
        let slab = self.slabs[id as usize].as_mut().unwrap();
        slab.bump = byte_off + bytes;
        slab.live += bytes;
        let slot = &mut self.regions[index as usize];
        slot.region = Some(Region { slab: id, range: byte_off..slab.bump, member: slab.members.len() });
        slab.members.push(index);
        Alloc { region: RegionRef { index, gen: slot.gen }, slab: id, byte_off, first_row }
    }

    pub(crate) fn alloc_move(&mut self, bytes: u64, stride: u32) -> Alloc {
        let alloc = self.alloc(bytes, stride, Generation::Old);
        self.evacuated_bytes += bytes;
        alloc
    }

    fn region(&self, r: RegionRef) -> Option<&Region> {
        let slot = self.regions.get(r.index as usize)?;
        if r.gen != slot.gen { return None; }
        slot.region.as_ref()
    }

    pub(crate) fn is_valid(&self, r: RegionRef) -> bool {
        self.region(r).is_some()
    }

    pub(crate) fn drawn(&mut self, r: RegionRef, serial: u64) {
        if let Some(region) = self.region(r) {
            let id = region.slab as usize;
            let slab = self.slabs[id].as_mut().unwrap();
            slab.last_read = slab.last_read.max(serial);
        }
    }

    pub(crate) fn needs_move(&self, r: RegionRef) -> bool {
        self.region(r).is_some_and(|r| self.slabs[r.slab as usize].as_ref().unwrap().state == State::Evacuating)
    }

    pub(crate) fn release(&mut self, r: RegionRef) {
        let Some(region) = self.region(r) else { return };
        let (id, member, bytes) = (region.slab, region.member, region.range.end - region.range.start);
        let slab = self.slabs[id as usize].as_mut().unwrap();
        slab.live -= bytes;
        slab.members.swap_remove(member);
        if let Some(&moved) = slab.members.get(member) {
            self.regions[moved as usize].region.as_mut().unwrap().member = member;
        }
        self.regions[r.index as usize].vacate(r.index, &mut self.free_regions);
    }

    fn drop_regions(&mut self, id: u32) {
        let slab = self.slabs[id as usize].as_mut().unwrap();
        self.regions_dropped += slab.members.len() as u64;
        for index in slab.members.drain(..) {
            self.regions[index as usize].vacate(index, &mut self.free_regions);
        }
        slab.live = 0;
    }

    pub(crate) fn end_frame(&mut self, _submitted: u64, completed: u64) {
        let mut candidate = None;
        for index in 0..self.slabs.len() {
            let Some(slab) = &mut self.slabs[index] else { continue };
            let id = index as u32;
            match slab.state {
                State::Closed if slab.live == 0 && slab.last_read <= completed => {
                    slab.state = State::Spare { idle: 0 };
                }
                State::Spare { idle } if idle + 1 == SPARE_FRAMES => self.drop_slab(id),
                State::Spare { idle } => slab.state = State::Spare { idle: idle + 1 },
                State::Evacuating => {
                    slab.state = State::Closed;
                    self.drop_regions(id);
                }
                State::Closed if slab.live > 0 && slab.live < slab.size / 4 && candidate.is_none() => {
                    candidate = Some(index);
                }
                _ => {}
            }
        }
        if let Some(index) = candidate {
            self.slabs[index].as_mut().unwrap().state = State::Evacuating;
        }
    }

    pub(crate) fn drop_slab(&mut self, id: u32) {
        let index = id as usize;
        if self.slabs.get(index).is_none_or(Option::is_none) { return; }
        self.drop_regions(id);
        self.slabs[index] = None;
        for open in &mut self.open {
            if *open == Some(id) { *open = None; }
        }
        self.events.push(SlabEvent::Destroy { slab: id });
    }

    pub(crate) fn drop_all(&mut self) {
        for index in 0..self.slabs.len() {
            self.drop_slab(index as u32);
        }
    }

    pub(crate) fn take_events(&mut self) -> Vec<SlabEvent> {
        std::mem::take(&mut self.events)
    }

    pub(crate) fn stats(&self) -> StoreStats {
        let mut stats = StoreStats { evacuated_bytes: self.evacuated_bytes, regions_dropped: self.regions_dropped, ..Default::default() };
        for slab in self.slabs.iter().flatten() {
            stats.slabs += 1;
            stats.slab_bytes += slab.size;
            stats.live_bytes += slab.live;
            stats.spare_slabs += u64::from(matches!(slab.state, State::Spare { .. }));
        }
        stats
    }
}

type ItemSlot = (usize, usize);
type InstKey = (u64, u64); // Retained publication id and prefix size in bytes.

pub(crate) struct InstRec {
    pub(crate) held: ItemRegion,
    /// The list that first drew this prefix, for memory inventory.
    pub(crate) list: usize,
    pub(crate) users: u32,
}

pub(crate) enum RegionUse {
    Reuse(ItemRegion),
    /// Write the CPU rows to this fresh range, including when evacuating a slab.
    Write { held: ItemRegion, byte_off: u64, bytes: u64 },
}

/// Each draw executor owns one of these. It decides when to reuse, write, or release
/// a recording's region or a publication prefix shared by several draw items.
#[derive(Default)]
pub(crate) struct InstanceRegions {
    pub(crate) store: InstanceStore,
    pub(crate) inst: HashMap<InstKey, InstRec>,
    /// Physical draw slot -> (list generation, publication prefix).
    pub(crate) inst_items: HashMap<ItemSlot, (u64, InstKey)>,
    /// One past each list's highest slot with a publication binding.
    pub(crate) inst_top: HashMap<usize, usize>,
}

impl InstanceRegions {
    /// Resident publication bytes attributed to the draw list that first used the prefix.
    pub(crate) fn instance_bytes(&self) -> impl Iterator<Item = (usize, u64)> + '_ {
        self.inst.values().filter(|rec| self.store.is_valid(rec.held.region)).map(|rec| {
            (rec.list, u64::from(rec.held.rows) * u64::from(rec.held.stride))
        })
    }

    /// Bind a prefix of an ordinary publication or all rows of a shared block.
    #[inline]
    pub(crate) fn publication(
        &mut self, item: ItemSlot, list_gen: u64, publication: &RetainedInstances,
        block: Option<&SharedInstances>, count: usize, slots: usize,
    ) -> RegionUse {
        let rows = block.map_or_else(
            || count.min(publication.float_len().checked_div(slots).unwrap_or(0)),
            SharedInstances::count,
        );
        let bytes = (rows * slots * 4) as u64;
        let want = (publication.id(), bytes);
        let binding = self.inst_items.get(&item).copied();
        let new_user = binding != Some((list_gen, want));
        if new_user {
            if binding.is_some() { self.release_publication(item); }
            self.inst_items.insert(item, (list_gen, want));
            let top = self.inst_top.entry(item.0).or_default();
            *top = (*top).max(item.1 + 1);
        }
        let mut moving = false;
        if let Some(rec) = self.inst.get_mut(&want) {
            if new_user { rec.users += 1; }
            moving = self.store.needs_move(rec.held.region);
            if moving {
                self.store.release(rec.held.region);
            } else if self.store.is_valid(rec.held.region) {
                return RegionUse::Reuse(rec.held);
            }
        }
        let stride = (slots * 4) as u32;
        let alloc = if moving { self.store.alloc_move(bytes, stride) } else { self.store.alloc(bytes, stride, Generation::Old) };
        let held = ItemRegion { region: alloc.region, slab: alloc.slab, first_row: alloc.first_row, rows: rows as u32, stride };
        self.inst.entry(want).and_modify(|rec| rec.held = held).or_insert(InstRec { held, list: item.0, users: 1 });
        RegionUse::Write { held, byte_off: alloc.byte_off, bytes }
    }

    /// Select the region for a plain recording and install a new handle on its slot.
    #[inline]
    pub(crate) fn recording(
        &mut self, items: &mut CxDrawItems, index: usize, count: usize, slots: usize,
    ) -> Option<RegionUse> {
        let item = &mut items.buffer[index];
        if count == 0 || slots == 0 || item.instances.as_ref().is_none_or(|r| r.refused()) {
            self.release_plain(item);
            return None;
        }
        let rows = count as u32;
        let stride = (slots * 4) as u32;
        let old = item.instance_region.filter(|r| self.store.is_valid(r.region));
        let moving = old.is_some_and(|r| self.store.needs_move(r.region));
        if let Some(r) = old {
            if !moving && r.rows == rows && r.stride == stride && item.kind.draw_call().is_some_and(|call| !call.instance_dirty) {
                return Some(RegionUse::Reuse(r));
            }
            self.store.release(r.region);
        }
        let bytes = (count * slots * 4) as u64;
        let alloc = if moving { self.store.alloc_move(bytes, stride) } else { self.store.alloc(bytes, stride, Generation::Young) };
        let held = ItemRegion { region: alloc.region, slab: alloc.slab, first_row: alloc.first_row, rows, stride };
        items.set_instance_region(index, held);
        Some(RegionUse::Write { held, byte_off: alloc.byte_off, bytes })
    }

    pub(crate) fn release_plain(&mut self, item: &mut CxDrawItem) {
        if let Some(old) = item.instance_region.take() {
            self.store.release(old.region);
        }
    }

    /// Drop this slot's reference. The last user releases the region.
    pub(crate) fn release_publication(&mut self, item: ItemSlot) {
        let Some((_, key)) = self.inst_items.remove(&item) else { return };
        let Some(rec) = self.inst.get_mut(&key) else { return };
        rec.users -= 1;
        if rec.users == 0 {
            self.store.release(rec.held.region);
            self.inst.remove(&key);
        }
    }

    pub(crate) fn release_item(&mut self, key: ItemSlot, item: &mut CxDrawItem) {
        self.release_publication(key);
        self.release_plain(item);
    }

    /// Release just the discarded tail when a list records fewer items.
    pub(crate) fn release_spares(&mut self, list: usize, items: &mut CxDrawItems) {
        for item in items.spare_region_slots() {
            self.release_plain(item);
        }
        let len = items.len();
        let Some(top) = self.inst_top.get_mut(&list) else { return };
        if *top > len {
            let top = std::mem::replace(top, len);
            for item in len..top {
                self.release_publication((list, item));
            }
        }
    }

    /// Invalidate the slab and report the publication slots that lost their backing.
    pub(crate) fn evict_slab(&mut self, slab: u32) -> Vec<ItemSlot> {
        self.store.drop_slab(slab);
        self.inst.retain(|_, rec| self.store.is_valid(rec.held.region));
        let mut users = Vec::new();
        self.inst_items.retain(|item, (_, key)| {
            let live = self.inst.contains_key(key);
            if !live { users.push(*item); }
            live
        });
        users
    }

    /// Forget publication bindings after the store has dropped all slabs.
    pub(crate) fn clear_publications(&mut self) -> Vec<ItemSlot> {
        self.inst.clear();
        self.inst_top.clear();
        self.inst_items.drain().map(|(item, _)| item).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    type Reads = BTreeMap<u64, Vec<(u32, Range<u64>)>>;

    fn check_fresh(reads: &Reads, a: Alloc, bytes: u64, stride: u32) {
        assert_eq!(u64::from(a.first_row) * u64::from(stride), a.byte_off, "first row addresses the upload");
        for (&serial, ranges) in reads {
            for (slab, range) in ranges {
                assert!(
                    a.slab != *slab || a.byte_off + bytes <= range.start || range.end <= a.byte_off,
                    "{a:?} overlaps frame {serial}'s pending read of slab {slab} at {range:?}"
                );
            }
        }
    }

    fn draw(store: &mut InstanceStore, reads: &mut Reads, serial: u64, a: Alloc, bytes: u64) {
        assert!(store.is_valid(a.region), "draw uses a live region");
        store.drawn(a.region, serial);
        reads.entry(serial).or_default().push((a.slab, a.byte_off..a.byte_off + bytes));
    }

    #[test]
    fn rerecord_with_two_frames_in_flight() {
        const LIST: u64 = 50_000 * 48;
        let mut store = InstanceStore::default();
        let mut reads = Reads::new();
        let first = store.alloc(48, 48, Generation::Young);
        draw(&mut store, &mut reads, 1, first, 48);
        store.release(first.region);
        let mut released = 48;
        let mut painted = store.alloc(32, 16, Generation::Young);
        check_fresh(&reads, painted, 32, 16);
        let hidden = store.alloc(96, 48, Generation::Young);
        check_fresh(&reads, hidden, 96, 48);
        assert_eq!(hidden.byte_off, 96, "mixed strides round up to the next whole row");
        draw(&mut store, &mut reads, 1, hidden, 96);
        let mut previous: Option<Alloc> = None;
        let mut nursery_destroyed = None;

        for serial in 1u64..=2048 {
            if store.needs_move(painted.region) {
                let fresh = store.alloc_move(32, 16);
                check_fresh(&reads, fresh, 32, 16);
                store.release(painted.region);
                released += 32;
                painted = fresh;
            }
            draw(&mut store, &mut reads, serial, painted, 32);
            let list = store.alloc(LIST, 48, Generation::Young);
            check_fresh(&reads, list, LIST, 48);
            draw(&mut store, &mut reads, serial, list, LIST);
            if let Some(last) = previous.replace(list) {
                store.release(last.region);
                released += LIST;
                assert!(!store.is_valid(last.region), "release invalidates the previous recording");
            }
            let completed = serial.saturating_sub(2);
            store.end_frame(serial, completed);
            reads.retain(|&frame, _| frame > completed);

            for event in store.take_events() {
                match event {
                    SlabEvent::Create { .. } => assert!(serial <= 3, "re-recording uses only recycled slabs after warm-up"),
                    SlabEvent::Destroy { slab } => {
                        assert_eq!(slab, hidden.slab, "only the idle nursery is destroyed during churn");
                        nursery_destroyed = Some(serial);
                    }
                }
            }
            let stats = store.stats();
            let open_and_spare: u64 = store.slabs.iter().flatten()
                .filter(|s| matches!(s.state, State::Open | State::Spare { .. })).map(|s| s.size).sum();
            assert!(stats.slab_bytes <= 2 * stats.live_bytes + open_and_spare, "bounded capacity at frame {serial}: {stats:?}");
            assert!(stats.evacuated_bytes <= released, "evacuation never exceeds released bytes");
            if serial == 1 {
                assert!(store.needs_move(painted.region) && store.needs_move(hidden.region), "survivors get one frame to move");
                assert_eq!(stats.evacuated_bytes, 0, "marking a slab for evacuation uploads nothing");
            } else {
                assert!(!store.is_valid(hidden.region), "an unpainted survivor loses its region");
                assert_eq!(stats.regions_dropped, 1, "only the unpainted survivor was dropped");
                assert_eq!(stats.evacuated_bytes, 32, "the painted survivor moved once");
                assert_eq!(stats.live_bytes, LIST + 32, "live bytes count just the current rows");
            }
        }
        assert_eq!(nursery_destroyed, Some(3 + u64::from(SPARE_FRAMES)), "an unused spare expires after 120 idle frames");
    }

    #[test]
    fn slab_eviction_and_session_reset() {
        let mut store = InstanceStore::default();
        let young = [store.alloc(32, 16, Generation::Young), store.alloc(96, 48, Generation::Young)];
        let old = [store.alloc(80, 40, Generation::Old), store.alloc(48, 48, Generation::Old)];
        let large = store.alloc(GROW_TO + 1, 1, Generation::Old);
        assert_eq!(store.take_events(), [
            SlabEvent::Create { slab: young[0].slab, bytes: MIN_SLAB },
            SlabEvent::Create { slab: old[0].slab, bytes: MIN_SLAB },
            SlabEvent::Create { slab: large.slab, bytes: 2 * GROW_TO },
        ], "each generation grows independently and an oversized region gets its size class");

        let after = store.alloc(16, 16, Generation::Old);
        assert_eq!(after.slab, old[0].slab, "a dedicated allocation preserves the open slab for small rows");
        let dedicated = store.slabs[large.slab as usize].as_ref().unwrap();
        assert_eq!(dedicated.members, [large.region.index], "only the oversized region occupies its slab");
        store.drawn(large.region, 2);
        store.release(large.region);
        store.end_frame(2, 1);
        assert_eq!(store.stats().spare_slabs, 0, "the dedicated slab waits for its reader");
        store.end_frame(3, 2);
        assert_eq!(store.stats().spare_slabs, 1, "the completed dedicated slab becomes spare");
        let reused = store.alloc(GROW_TO + 2, 1, Generation::Old);
        assert_eq!(reused.slab, large.slab, "another oversized region reuses its exact size class");
        assert!(store.take_events().is_empty(), "reusing the dedicated slab creates no buffer");
        store.release(after.region);
        let next_old = store.alloc(MIN_SLAB, 16, Generation::Old);
        assert_eq!(store.take_events(), [SlabEvent::Create { slab: next_old.slab, bytes: 2 * MIN_SLAB }],
            "the dedicated slab leaves the generation's growth schedule unchanged");
        store.release(next_old.region);

        store.drop_slab(young[0].slab);
        let fresh = store.alloc(16, 16, Generation::Young);
        assert_eq!(store.take_events(), [
            SlabEvent::Destroy { slab: young[0].slab },
            SlabEvent::Create { slab: fresh.slab, bytes: 2 * MIN_SLAB },
        ], "destroy precedes opening a replacement buffer");
        for a in young {
            assert!(!store.is_valid(a.region), "all handles of the evicted slab stay stale after allocation");
            store.drawn(a.region, u64::MAX);
            store.release(a.region);
        }
        assert_eq!(store.stats().regions_dropped, 2, "eviction counts each live member once");
        for a in [old[0], old[1], reused, fresh] {
            let r = store.region(a.region).expect("other slabs retain their handles");
            assert_eq!((r.slab, r.range.start), (a.slab, a.byte_off), "eviction preserves the address of every other region");
        }
        store.end_frame(4, 3);
        assert!(store.needs_move(old[1].region), "closed old slabs also evacuate");
        store.release(old[0].region);
        store.release(old[1].region);
        store.drop_all();
        assert_eq!(store.stats(), StoreStats { regions_dropped: 4, ..Default::default() }, "reset frees every slab and keeps running totals");
        let destroys = store.take_events();
        for slab in [old[0].slab, reused.slab, next_old.slab, fresh.slab] {
            assert!(destroys.contains(&SlabEvent::Destroy { slab }), "reset destroys slab {slab}");
        }
        assert_eq!(destroys.len(), 4, "reset emits one destroy per remaining buffer");
        store.drop_all();
        assert!(store.take_events().is_empty(), "repeated reset emits no duplicate destroys");
        let next = store.alloc(16, 16, Generation::Young);
        assert!(store.is_valid(next.region) && !store.is_valid(large.region) && !store.is_valid(fresh.region),
            "pre-reset handles stay stale in the next session");
    }
}
