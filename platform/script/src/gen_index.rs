/// Generational index system for detecting use-after-free bugs.
///
/// A GenVec stores data along with a generation counter per slot. When an item is freed
/// and its slot reused, the generation is incremented. Any stale references will
/// have a mismatched generation and can be detected.
///
/// The generation's high byte is the owning heap's tag, so a ref minted by another heap never
/// validates here either (see `crate::heap_tag`).
use crate::value::Generation;

/// Trait for types that can be used as generational indices into a GenVec.
/// The type must provide an index and a generation.
pub trait GenRef: Copy {
    fn index(&self) -> u32;
    fn generation(&self) -> Generation;
    fn new(index: u32, generation: Generation) -> Self;
}

/// Metadata stored per slot in a GenVec
#[derive(Debug, Clone)]
pub struct GenSlot<T> {
    pub generation: Generation,
    pub data: T,
}

impl<T: Default> Default for GenSlot<T> {
    fn default() -> Self {
        Self {
            generation: 0,
            data: T::default(),
        }
    }
}

impl<T> GenSlot<T> {
    pub fn new(data: T) -> Self {
        Self {
            generation: 0,
            data,
        }
    }

    /// Increment the generation counter (called when freeing a slot); the heap tag in the high
    /// byte stays.
    #[inline]
    pub fn increment_generation(&mut self) {
        self.generation = (self.generation & 0xff00) | (self.generation.wrapping_add(1) & 0x00ff);
    }
}

/// A Vec wrapper that tracks generations for each slot.
/// Provides checked access that validates generation on each lookup.
///
/// Use `Index<R>` where R implements `GenRef` for checked access.
/// Use `Index<usize>` for unchecked access (iteration, etc).
#[derive(Debug)]
pub struct GenVec<T> {
    slots: Vec<GenSlot<T>>,
    /// The owning heap's tag, shifted into the generation's high byte: every new slot but the
    /// null slot 0 starts at it.
    tag: Generation,
}

impl<T: Default> Default for GenVec<T> {
    fn default() -> Self {
        Self { slots: Vec::new(), tag: 0 }
    }
}

impl<T> GenVec<T> {
    pub fn new() -> Self {
        Self { slots: Vec::new(), tag: 0 }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            slots: Vec::with_capacity(capacity),
            tag: 0,
        }
    }

    /// Tag this table with its heap's tag (before any slot past the null slot is made).
    pub fn set_heap_tag(&mut self, tag: u8) {
        debug_assert!(self.slots.len() <= 1, "a table is tagged before it holds values");
        self.tag = (tag as Generation) << 8;
    }

    /// The generation a new slot at `index` starts at: the heap's tag, but 0 for the null slot.
    #[inline]
    fn first_generation(&self, index: usize) -> Generation {
        if index == 0 { 0 } else { self.tag }
    }

    /// Push a new item and return (index, generation)
    pub fn push(&mut self, data: T) -> (u32, Generation) {
        let index = self.slots.len();
        if index >= 1 << crate::value::HEAP_INDEX_BITS {
            table_full_panic::<T>();
        }
        let generation = self.first_generation(index);
        self.slots.push(GenSlot { generation, data });
        (index as u32, generation)
    }

    /// Get the length of the underlying vec
    #[inline]
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    /// Reclaim over-allocated backing capacity. This only shrinks spare capacity (it never
    /// changes `len`), so every existing raw index / `GenRef` remains valid. Useful after a
    /// large allocation spike followed by a GC: the slot count stays at its high-water mark
    /// (slots are reused via the free list), but the Vec's doubling overhead is returned.
    #[inline]
    pub fn shrink_to_fit(&mut self) {
        self.slots.shrink_to_fit();
    }

    /// Backing capacity, for deciding whether a `shrink_to_fit` is worthwhile.
    #[inline]
    pub fn capacity(&self) -> usize {
        self.slots.capacity()
    }

    /// Check if empty
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// Get the current generation for a slot by raw index
    #[inline]
    pub fn generation(&self, index: usize) -> Generation {
        self.slots[index].generation
    }

    /// Check if a reference is still valid (generation matches)
    #[inline]
    pub fn is_valid<R: GenRef>(&self, r: R) -> bool {
        let index = r.index() as usize;
        if index >= self.slots.len() {
            return false;
        }
        self.slots[index].generation == r.generation()
    }

    /// Increment the generation for a slot (call this when freeing)
    #[inline]
    pub fn free_slot(&mut self, index: u32) {
        if let Some(slot) = self.slots.get_mut(index as usize) {
            slot.increment_generation();
        }
    }

    /// Iter over the data items
    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.slots.iter().map(|s| &s.data)
    }

    /// Iter mutably over the data items
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut T> {
        self.slots.iter_mut().map(|s| &mut s.data)
    }

    /// Split the underlying slots at a given index for simultaneous mutable access.
    /// Returns references to GenSlot (with .data field) to allow accessing both data and generation.
    pub fn slots_split_at_mut(&mut self, mid: usize) -> (&mut [GenSlot<T>], &mut [GenSlot<T>]) {
        self.slots.split_at_mut(mid)
    }

    /// Get a reference to an item by raw index.
    /// INTERNAL USE ONLY - bypasses generation check. Use for GC iteration.
    #[inline]
    pub fn get_at(&self, index: usize) -> &T {
        &self.slots[index].data
    }

    /// Get a mutable reference to an item by raw index.
    /// INTERNAL USE ONLY - bypasses generation check. Use for GC iteration.
    #[inline]
    pub fn get_at_mut(&mut self, index: usize) -> &mut T {
        &mut self.slots[index].data
    }

    /// Set an item by raw index.
    /// INTERNAL USE ONLY - bypasses generation check. Use for GC sweep.
    #[inline]
    pub fn set_at(&mut self, index: usize, value: T) {
        self.slots[index].data = value;
    }
}

impl<T: Default> GenVec<T> {
    /// Allocate a slot, either reusing from free list or growing.
    /// Returns a reference type R with the correct generation.
    pub fn allocate<R: GenRef>(&mut self, free_list: &mut Vec<u32>) -> R {
        if let Some(index) = free_list.pop() {
            // Reuse freed slot - generation was already incremented when freed
            let gen = self.slots[index as usize].generation;
            R::new(index, gen)
        } else {
            // Grow the vec
            let (index, generation) = self.push(T::default());
            R::new(index, generation)
        }
    }
}

/// Check generation and panic on mismatch
#[inline]
fn check_generation<R: GenRef>(
    slots_gen: Generation,
    ref_gen: Generation,
    index: u32,
    type_name: &str,
) {
    if slots_gen != ref_gen {
        use_after_free_panic(slots_gen, ref_gen, index, type_name);
    }
}

/// The panic itself stays out of line: every heap access inlines only the
/// generation compare.
#[cold]
#[inline(never)]
fn use_after_free_panic(slots_gen: Generation, ref_gen: Generation, index: u32, type_name: &str) -> ! {
    if slots_gen >> 8 != ref_gen >> 8 && index != 0 {
        panic!(
            "GenVec<{}> value from heap {} used in heap {} (index={}): a value is only meaningful in the heap that made it; resolve it in its own VM",
            type_name, ref_gen >> 8, slots_gen >> 8, index
        );
    }
    panic!(
        "GenVec<{}> use-after-free detected! index={} ref_gen={} slot_gen={}",
        type_name, index, ref_gen, slots_gen
    );
}

/// A heap table holds at most 2^24 slots: the index's bits in a `ScriptValue`.
#[cold]
#[inline(never)]
fn table_full_panic<T>() -> ! {
    panic!("GenVec<{}> is full: a heap table holds at most {} slots", std::any::type_name::<T>(), 1u32 << crate::value::HEAP_INDEX_BITS);
}

/// An index past len can't come from this GenVec (len never shrinks), so it is
/// a value minted by ANOTHER heap. Name the table and the numbers so the
/// cross-heap route can be identified from the panic alone.
#[cold]
#[inline(never)]
fn foreign_index_panic<T>(index: u32, len: usize) -> ! {
    panic!(
        "GenVec<{}> index {} out of bounds (len {}): value belongs to a different heap",
        std::any::type_name::<T>(),
        index,
        len
    );
}

/// Bounds-checked accessors for paths that must SURVIVE a foreign-heap ref
/// (e.g. the GC mark walk): a miss means the value was minted by another
/// heap, and the caller skips it instead of panicking.
impl<T> GenVec<T> {
    pub fn get_checked<R: GenRef>(&self, r: R) -> Option<&T> {
        let tag = self.tag;
        self.slots.get(r.index() as usize).filter(|slot| Self::same_heap(tag, slot, r)).map(|slot| &slot.data)
    }
    pub fn get_checked_mut<R: GenRef>(&mut self, r: R) -> Option<&mut T> {
        let tag = self.tag;
        self.slots.get_mut(r.index() as usize).filter(|slot| Self::same_heap(tag, slot, r)).map(|slot| &mut slot.data)
    }
    /// The ref was minted by this heap (a stale one of this heap counts: its slot is here).
    #[inline]
    fn same_heap<R: GenRef>(tag: Generation, slot: &GenSlot<T>, r: R) -> bool {
        r.generation() & 0xff00 == if slot.generation == 0 && r.index() == 0 { 0 } else { tag }
    }
}

/// Index by a GenRef type - checked access
impl<T, R: GenRef> std::ops::Index<R> for GenVec<T> {
    type Output = T;

    #[inline]
    fn index(&self, r: R) -> &Self::Output {
        let Some(slot) = self.slots.get(r.index() as usize) else {
            foreign_index_panic::<T>(r.index(), self.slots.len());
        };
        check_generation::<R>(
            slot.generation,
            r.generation(),
            r.index(),
            std::any::type_name::<T>(),
        );
        &slot.data
    }

}

/// IndexMut by a GenRef type - checked access
impl<T, R: GenRef> std::ops::IndexMut<R> for GenVec<T> {
    #[inline]
    fn index_mut(&mut self, r: R) -> &mut Self::Output {
        let len = self.slots.len();
        let Some(slot) = self.slots.get_mut(r.index() as usize) else {
            foreign_index_panic::<T>(r.index(), len);
        };
        check_generation::<R>(
            slot.generation,
            r.generation(),
            r.index(),
            std::any::type_name::<T>(),
        );
        &mut slot.data
    }

}
