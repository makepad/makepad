//! Descriptor sets kept across frames. A draw that binds what an earlier frame's draw bound (the
//! same program, buffers, images and samplers: an unchanged mesh chunk, a line of text) takes
//! that draw's set again, written once and bound with this draw's own dynamic offsets, so it
//! costs a bind and a draw. Only a draw that binds something new allocates and writes a set.
//!
//! Sets come from one descriptor pool at a time (a generation). A full pool is replaced by a new
//! one sized for the sets the recent frames bound; the old pool's sets leave the cache, and the
//! pool goes once the last batch that bound one of them is done with it (each batch holds the
//! generations it bound, `Batch::generations`).
use super::device::{error, Device};
use super::pipeline::Program;
use crate::FastMap;
use makepad_vulkan::vk;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

/// A new object's serial (`Buffer`, `Image`, `Program`): what a kept set's key names it by, never
/// a handle the driver may hand out again once the object is gone.
pub(super) fn serial() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// A draw's buffer binding: its slot, its type, the range it names and the serial of the buffer.
pub(super) type BufferBinding = (u32, vk::DescriptorType, vk::DescriptorBufferInfo, u64);
/// A draw's image or sampler binding: its slot, its type, what it names and the serial of the
/// image (0 for a sampler alone: the program's, or the device's, which outlive its sets).
pub(super) type ImageBinding = (u32, vk::DescriptorType, vk::DescriptorImageInfo, u64);

/// The sets a new pool holds at least.
const KEPT_MIN: u32 = 1024;
/// A set bound within this many frames is in use: a new pool has room for twice those.
const LIVE_FRAMES: u64 = 8;

/// One descriptor pool the kept sets come from.
pub(super) struct Generation {
    dev: Arc<Device>,
    pool: vk::DescriptorPool,
    live: Arc<AtomicU64>,
}

impl Drop for Generation {
    fn drop(&mut self) {
        unsafe { self.dev.raw.destroy_descriptor_pool(self.pool, None); }
        self.live.fetch_sub(1, Ordering::Relaxed);
    }
}

struct Kept {
    set: vk::DescriptorSet,
    key: Box<[u64]>,
    /// the frame (`KeptSets::tick`) that last bound it
    used: u64,
}

#[derive(Default)]
pub(super) struct KeptSets {
    /// by what the set holds (`key`)
    sets: FastMap<u64, Kept>,
    key: Vec<u64>,
    /// the generation new sets come from
    current: Option<Arc<Generation>>,
    /// what the current generation still has room for: sets, and descriptors per type
    room_sets: u32,
    room: Vec<(vk::DescriptorType, u32)>,
    /// the most descriptors of each type one set of any program drawn so far takes: a new pool
    /// has room for that many per set
    widest: Vec<(vk::DescriptorType, u32)>,
    need: Vec<(vk::DescriptorType, u32)>,
    clock: u64,
    /// generations alive: the current one and those batches still hold (the ledger's
    /// descriptor pools)
    live: Arc<AtomicU64>,
}

impl KeptSets {
    /// A frame begins (`encode_frame`): its draws' sets count as bound now.
    pub fn tick(&mut self) { self.clock += 1; }

    /// The descriptor pools alive.
    pub fn pools(&self) -> u64 { self.live.load(Ordering::Relaxed) }

    /// The set for a draw's bindings: kept (`false`: nothing to write) or new (`true`: the caller
    /// writes it). `held`: the generations the draw's batch holds, given the set's.
    pub fn set(&mut self, dev: &Arc<Device>, held: &mut Vec<Arc<Generation>>, p: &Program, bins: &[BufferBinding], images: &[ImageBinding])
        -> Result<(vk::DescriptorSet, bool), String> {
        let mut key = std::mem::take(&mut self.key);
        key.clear();
        key.push(p.serial);
        for (slot, ty, info, serial) in bins { key.extend([*slot as u64 | (ty.0 as u64) << 32, *serial, info.offset, info.range]); }
        for (slot, ty, info, serial) in images {
            key.extend([*slot as u64 | (ty.0 as u64) << 32, *serial, info.sampler.0, info.image_view.0, info.image_layout.0 as u64]);
        }
        let hash = key.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, v| (h ^ *v).wrapping_mul(0x0100_0000_01b3).rotate_left(23));
        let clock = self.clock;
        let out = match self.sets.get_mut(&hash).filter(|k| *k.key == *key) {
            Some(kept) => { kept.used = clock; Ok((kept.set, false)) }
            None => self.make(dev, p).map(|set| {
                // a hash two different keys share keeps its first set; the second is not kept
                if !self.sets.contains_key(&hash) { self.sets.insert(hash, Kept { set, key: key.as_slice().into(), used: clock }); }
                (set, true)
            }),
        };
        if let Some(current) = &self.current {
            if !held.last().is_some_and(|g| Arc::ptr_eq(g, current)) { held.push(current.clone()); }
        }
        self.key = key;
        out
    }

    /// A new set for `p`, from the current generation or a new one.
    fn make(&mut self, dev: &Arc<Device>, p: &Program) -> Result<vk::DescriptorSet, String> {
        let mut need = std::mem::take(&mut self.need);
        need.clear();
        for (_, ty) in &p.descriptor_types {
            // (a combined image sampler through a Y'CbCr conversion may take up to three
            // descriptors: combinedImageSamplerDescriptorCount)
            let per = if *ty == vk::DescriptorType::COMBINED_IMAGE_SAMPLER { 3 } else { 1 };
            match need.iter_mut().find(|(t, _)| t == ty) { Some((_, n)) => *n += per, None => need.push((*ty, per)) }
        }
        for (ty, n) in &need {
            match self.widest.iter_mut().find(|(t, _)| t == ty) { Some((_, w)) => *w = (*w).max(*n), None => self.widest.push((*ty, *n)) }
        }
        let fits = self.current.is_some() && self.room_sets > 0
            && need.iter().all(|(ty, n)| self.room.iter().any(|(t, r)| t == ty && r >= n));
        let made = (|| {
            if !fits { self.rotate(dev)?; }
            match self.allocate(dev, p) {
                Ok(set) => Ok(set),
                // the driver's count of the pool's room differs from ours (fragmentation)
                Err(vk::Result::ERROR_OUT_OF_POOL_MEMORY | vk::Result::ERROR_FRAGMENTED_POOL) => {
                    self.rotate(dev)?;
                    self.allocate(dev, p).map_err(error)
                }
                Err(e) => Err(error(e)),
            }
        })();
        if made.is_ok() {
            self.room_sets -= 1;
            for (ty, n) in &need {
                if let Some((_, r)) = self.room.iter_mut().find(|(t, _)| t == ty) { *r -= n; }
            }
        }
        self.need = need;
        made
    }

    fn allocate(&self, dev: &Device, p: &Program) -> Result<vk::DescriptorSet, vk::Result> {
        let pool = self.current.as_ref().map_or(vk::DescriptorPool::null(), |g| g.pool);
        let layouts = [p.descriptors];
        // one set, written in place (the wrapper returns a Vec per call)
        let Some(allocate) = dev.raw.table.allocate_descriptor_sets else { return Err(vk::Result::ERROR_EXTENSION_NOT_PRESENT) };
        let mut set = vk::DescriptorSet::null();
        unsafe { allocate(dev.raw.handle(), &vk::DescriptorSetAllocateInfo::default().descriptor_pool(pool).set_layouts(&layouts), &mut set) }.result()?;
        Ok(set)
    }

    /// A new generation, with room for twice the sets the recent frames bound; the current one's
    /// sets leave the cache.
    fn rotate(&mut self, dev: &Arc<Device>) -> Result<(), String> {
        let live = self.sets.values().filter(|k| k.used + LIVE_FRAMES >= self.clock).count() as u32;
        let capacity = live.saturating_mul(2).max(KEPT_MIN).checked_next_power_of_two().unwrap_or(u32::MAX);
        let sizes: Vec<_> = self.widest.iter().map(|(ty, n)| vk::DescriptorPoolSize::default().ty(*ty).descriptor_count(capacity.saturating_mul(*n))).collect();
        let pool = unsafe { dev.raw.create_descriptor_pool(&vk::DescriptorPoolCreateInfo::default().max_sets(capacity).pool_sizes(&sizes), None) }.map_err(error)?;
        crate::profile::count("vk kept descriptor pool made");
        self.sets.clear();
        self.live.fetch_add(1, Ordering::Relaxed);
        self.current = Some(Arc::new(Generation { dev: dev.clone(), pool, live: self.live.clone() }));
        self.room_sets = capacity;
        self.room = self.widest.iter().map(|(ty, n)| (*ty, capacity.saturating_mul(*n))).collect();
        Ok(())
    }
}
