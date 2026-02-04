/// Generational index system for detecting use-after-free bugs.
///
/// A GenVec stores data along with a generation counter per slot. When an item is freed
/// and its slot reused, the generation is incremented. Any stale references will
/// have a mismatched generation and can be detected.
///
/// For release builds, generation checking can be disabled via feature flag.
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

    /// Increment the generation counter (called when freeing a slot)
    #[inline]
    pub fn increment_generation(&mut self) {
        self.generation = self.generation.wrapping_add(1);
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
}

impl<T: Default> Default for GenVec<T> {
    fn default() -> Self {
        Self { slots: Vec::new() }
    }
}

impl<T> GenVec<T> {
    pub fn new() -> Self {
        Self { slots: Vec::new() }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            slots: Vec::with_capacity(capacity),
        }
    }

    /// Push a new item and return (index, generation)
    pub fn push(&mut self, data: T) -> (u32, Generation) {
        let index = self.slots.len() as u32;
        self.slots.push(GenSlot::new(data));
        (index, 0)
    }

    /// Get the length of the underlying vec
    #[inline]
    pub fn len(&self) -> usize {
        self.slots.len()
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
            let index = self.slots.len() as u32;
            self.slots.push(GenSlot::default());
            R::new(index, 0)
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
        panic!(
            "GenVec<{}> use-after-free detected! index={} ref_gen={} slot_gen={}",
            type_name, index, ref_gen, slots_gen
        );
    }
}

/// Index by a GenRef type - checked access
impl<T, R: GenRef> std::ops::Index<R> for GenVec<T> {
    type Output = T;

    #[inline]
    fn index(&self, r: R) -> &Self::Output {
        let slot = &self.slots[r.index() as usize];
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
        let slot = &mut self.slots[r.index() as usize];
        check_generation::<R>(
            slot.generation,
            r.generation(),
            r.index(),
            std::any::type_name::<T>(),
        );
        &mut slot.data
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Test reference type
    #[derive(Copy, Clone)]
    struct TestRef {
        index: u32,
        generation: Generation,
    }

    impl GenRef for TestRef {
        fn index(&self) -> u32 {
            self.index
        }
        fn generation(&self) -> Generation {
            self.generation
        }
        fn new(index: u32, generation: Generation) -> Self {
            Self { index, generation }
        }
    }
}
