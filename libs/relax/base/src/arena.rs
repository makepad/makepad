//! Arrays of the front end's per-file data that are either built in this process (owned) or
//! read from a mapped front-end image (plans/memory-use.md, "Image design"): a slice into the
//! mapping costs no heap and its pages stay clean (the OS can drop and re-read them).
//!
//! Reads go through `Deref<Target = [T]>`, so consumers do not care which one they have. A
//! mutable access to a mapped array copies it into the heap first (copy on write) and counts
//! the copy (`COPIES`): files loaded from an image are not meant to change.

use std::sync::atomic::{AtomicU64, Ordering};

/// mapped arrays copied into the heap by a mutable access, and their bytes
pub static COPIES: AtomicU64 = AtomicU64::new(0);
pub static COPIED_BYTES: AtomicU64 = AtomicU64::new(0);

pub enum Arena<T: 'static> {
    Owned(Vec<T>),
    /// into a mapping that lives as long as the process
    Mapped(&'static [T]),
}

impl<T: 'static> Default for Arena<T> {
    fn default() -> Self {
        Arena::Owned(Vec::new())
    }
}

impl<T: 'static> From<Vec<T>> for Arena<T> {
    fn from(v: Vec<T>) -> Self {
        Arena::Owned(v)
    }
}

impl<T: Clone + 'static> Clone for Arena<T> {
    fn clone(&self) -> Self {
        match self {
            Arena::Owned(v) => Arena::Owned(v.clone()),
            Arena::Mapped(s) => Arena::Mapped(s),
        }
    }
}

impl<T: std::fmt::Debug + 'static> std::fmt::Debug for Arena<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        (**self).fmt(f)
    }
}

impl<T: 'static> std::ops::Deref for Arena<T> {
    type Target = [T];
    #[inline]
    fn deref(&self) -> &[T] {
        match self {
            Arena::Owned(v) => v,
            Arena::Mapped(s) => s,
        }
    }
}

impl<T: Clone + 'static> std::ops::DerefMut for Arena<T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut [T] {
        self.vec_mut()
    }
}

impl<T: 'static> Arena<T> {
    pub fn is_mapped(&self) -> bool {
        matches!(self, Arena::Mapped(_))
    }

    /// Heap bytes this array holds (0 when mapped).
    pub fn heap_bytes(&self) -> usize {
        match self {
            Arena::Owned(v) => v.capacity() * std::mem::size_of::<T>(),
            Arena::Mapped(_) => 0,
        }
    }

    pub fn shrink_to_fit(&mut self) {
        if let Arena::Owned(v) = self {
            v.shrink_to_fit();
        }
    }

    /// The owned array (None when mapped).
    pub fn owned(&self) -> Option<&Vec<T>> {
        match self {
            Arena::Owned(v) => Some(v),
            Arena::Mapped(_) => None,
        }
    }
}

impl<T: Clone + 'static> Arena<T> {
    /// The array as a Vec to change (a mapped one is copied into the heap first).
    #[inline]
    pub fn vec_mut(&mut self) -> &mut Vec<T> {
        if let Arena::Mapped(s) = *self {
            COPIES.fetch_add(1, Ordering::Relaxed);
            COPIED_BYTES.fetch_add(std::mem::size_of_val(s) as u64, Ordering::Relaxed);
            *self = Arena::Owned(s.to_vec());
        }
        match self {
            Arena::Owned(v) => v,
            Arena::Mapped(_) => unreachable!(),
        }
    }

    #[inline]
    pub fn push(&mut self, x: T) {
        self.vec_mut().push(x)
    }

    pub fn extend_from_slice(&mut self, x: &[T]) {
        self.vec_mut().extend_from_slice(x)
    }

    pub fn truncate(&mut self, n: usize) {
        if n < self.len() {
            self.vec_mut().truncate(n)
        }
    }

    /// The owned Vec, copying a mapped array.
    pub fn into_vec(self) -> Vec<T> {
        match self {
            Arena::Owned(v) => v,
            Arena::Mapped(s) => s.to_vec(),
        }
    }
}

impl<T: Clone + 'static> Extend<T> for Arena<T> {
    fn extend<I: IntoIterator<Item = T>>(&mut self, it: I) {
        self.vec_mut().extend(it)
    }
}

impl<'a, T: 'static> IntoIterator for &'a Arena<T> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}
