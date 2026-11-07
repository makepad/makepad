//! Device memory for retained buffers, many buffers to one allocation. Allocating, mapping and
//! freeing device memory are driver round trips (a desktop driver: ~100 µs to make a buffer and
//! about as long to free one): an edit that remade a part's mesh chunks made and dropped up to
//! ~1900 buffers a second, ~400 ms of the UI thread's second. A buffer binds a range of a block
//! instead: the block is allocated and mapped once, its ranges are handed out and taken back
//! under the device's lock (a buffer goes on whichever thread drops it last, once the GPU is done
//! with it), and writes go through the mapping with no map or unmap of their own.
use super::device::Device;
use makepad_vulkan::vk;
use std::collections::{BTreeMap, BTreeSet};

/// A block's size. A buffer over a quarter of it gets an allocation of its own.
const BLOCK: u64 = 32 << 20;

/// Where a buffer's memory is: a range of a block (`block` its id) or an allocation of its own
/// (`block` 0), and the host's view of its first byte.
#[derive(Clone, Copy)]
pub(super) struct Placement {
    pub block: u64,
    pub memory: vk::DeviceMemory,
    pub offset: u64,
    pub size: u64,
    pub mapped: usize,
}

#[derive(Default)]
pub(super) struct Blocks {
    blocks: Vec<Block>,
    next: u64,
}

struct Block {
    id: u64,
    memory: vk::DeviceMemory,
    ty: u32,
    /// allocated with `DEVICE_ADDRESS` (for buffers a shader reaches by address)
    address: bool,
    mapped: usize,
    used: u64,
    /// the free ranges by offset (a range given back merges with its neighbours) and by size
    /// (the smallest that fits is taken)
    by_offset: BTreeMap<u64, u64>,
    by_size: BTreeSet<(u64, u64)>,
}

impl Block {
    fn take(&mut self, size: u64, align: u64) -> Option<u64> {
        let (len, at) = self.by_size.range((size, 0)..).find(|(len, at)| at.next_multiple_of(align) + size <= at + len).copied()?;
        self.by_size.remove(&(len, at));
        self.by_offset.remove(&at);
        let start = at.next_multiple_of(align);
        if start > at { self.free(at, start - at); }
        if start + size < at + len { self.free(start + size, at + len - start - size); }
        self.used += size;
        Some(start)
    }

    fn free(&mut self, at: u64, len: u64) {
        self.by_offset.insert(at, len);
        self.by_size.insert((len, at));
    }

    fn give(&mut self, mut at: u64, mut len: u64) {
        self.used -= len;
        if let Some((&before, &before_len)) = self.by_offset.range(..at).next_back().filter(|(b, l)| **b + **l == at) {
            self.by_offset.remove(&before);
            self.by_size.remove(&(before_len, before));
            at = before;
            len += before_len;
        }
        if let Some(after_len) = self.by_offset.remove(&(at + len)) {
            self.by_size.remove(&(after_len, at + len));
            len += after_len;
        }
        self.free(at, len);
    }
}

fn allocate(dev: &Device, ty: u32, address: bool, size: u64) -> Result<(vk::DeviceMemory, usize), vk::Result> {
    let mut flags = vk::MemoryAllocateFlagsInfo::default().flags(vk::MemoryAllocateFlags::DEVICE_ADDRESS);
    let mut info = vk::MemoryAllocateInfo::default().allocation_size(size).memory_type_index(ty);
    if address { info = info.next_memory_allocate_flags_info(&mut flags); }
    let memory = unsafe { dev.raw.allocate_memory(&info, None) }?;
    match unsafe { dev.raw.map_memory(memory, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty()) } {
        Ok(ptr) => Ok((memory, ptr as usize)),
        Err(e) => { unsafe { dev.raw.free_memory(memory, None); } Err(e) }
    }
}

/// An allocation of its own, mapped: a large buffer's, or a scratch buffer's (arenas and
/// staging, made rarely and charged to their own pools).
pub(super) fn own(dev: &Device, ty: u32, address: bool, size: u64) -> Result<Placement, vk::Result> {
    let (memory, mapped) = allocate(dev, ty, address, size)?;
    Ok(Placement { block: 0, memory, offset: 0, size, mapped })
}

impl Blocks {
    /// Memory of host-visible type `ty` for a buffer with requirements `req`, mapped.
    /// `coherent`: the type is host-coherent (otherwise a range is whole non-coherent atoms, so
    /// flushing or invalidating one buffer's bytes never touches another's).
    pub(super) fn place(&mut self, dev: &Device, ty: u32, address: bool, coherent: bool, req: &vk::MemoryRequirements) -> Result<Placement, vk::Result> {
        let atom = if coherent { 1 } else { dev.properties.limits.non_coherent_atom_size.max(1) };
        let (size, align) = (req.size.next_multiple_of(atom), req.alignment.max(atom).max(1));
        if size > BLOCK / 4 { return own(dev, ty, address, req.size); }
        let fits = |b: &mut Block| (b.ty == ty && b.address == address).then(|| b.take(size, align)).flatten();
        let placed = self.blocks.iter_mut().find_map(|b| fits(b).map(|at| (b.id, b.memory, b.mapped, at)));
        let (block, memory, mapped, offset) = match placed {
            Some(p) => p,
            None => {
                let (memory, mapped) = allocate(dev, ty, address, BLOCK)?;
                crate::profile::count("vk buffer block made");
                self.next += 1;
                let mut b = Block { id: self.next, memory, ty, address, mapped, used: 0, by_offset: BTreeMap::new(), by_size: BTreeSet::new() };
                b.free(0, BLOCK);
                let at = b.take(size, align).expect("a new block holds a quarter of its size");
                self.blocks.push(b);
                (self.next, memory, mapped, at)
            }
        };
        self.report(dev);
        Ok(Placement { block, memory, offset, size, mapped: mapped + offset as usize })
    }

    /// A buffer's memory goes back. A block left empty goes too, unless it is the last of its
    /// kind (the next buffer would make it again).
    pub(super) fn give(&mut self, dev: &Device, p: &Placement) {
        if p.block == 0 {
            unsafe { dev.raw.unmap_memory(p.memory); dev.raw.free_memory(p.memory, None); }
            return;
        }
        let Some(i) = self.blocks.iter().position(|b| b.id == p.block) else { return };
        self.blocks[i].give(p.offset, p.size);
        let (ty, address) = (self.blocks[i].ty, self.blocks[i].address);
        if self.blocks[i].used == 0 && self.blocks.iter().filter(|b| b.ty == ty && b.address == address).count() > 1 {
            let b = self.blocks.swap_remove(i);
            unsafe { dev.raw.unmap_memory(b.memory); dev.raw.free_memory(b.memory, None); }
        }
        self.report(dev);
    }

    /// The blocks' room no buffer holds, on the ledger's backend rows.
    fn report(&self, dev: &Device) {
        let free: u64 = self.blocks.iter().map(|b| BLOCK - b.used).sum();
        dev.pools.set(crate::ledger::Pool::BufferBlocks, free, self.blocks.len() as u64);
    }

    /// The device goes: every block with it (no buffer is left).
    pub(super) fn release(&mut self, dev: &Device) {
        for b in self.blocks.drain(..) { unsafe { dev.raw.unmap_memory(b.memory); dev.raw.free_memory(b.memory, None); } }
    }
}
