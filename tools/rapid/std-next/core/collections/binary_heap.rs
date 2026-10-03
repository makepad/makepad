//! BinaryHeap: a max-heap over a Vec, with real std's sift order (so pop order of equal
//! elements and `into_vec` layout match).

use crate::vec::Vec;
use crate::fmt;
use crate::iter::{FromIterator, FusedIterator};
use crate::ops::{Deref, DerefMut};

pub struct BinaryHeap<T> {
    data: Vec<T>,
}

impl<T: Ord> BinaryHeap<T> {
    pub fn new() -> BinaryHeap<T> {
        BinaryHeap { data: Vec::new() }
    }

    pub fn with_capacity(capacity: usize) -> BinaryHeap<T> {
        BinaryHeap { data: Vec::with_capacity(capacity) }
    }

    pub fn peek_mut(&mut self) -> Option<PeekMut<'_, T>> {
        if self.is_empty() {
            None
        } else {
            Some(PeekMut { heap: self })
        }
    }

    pub fn pop(&mut self) -> Option<T> {
        match self.data.pop() {
            Some(mut item) => {
                if !self.is_empty() {
                    crate::mem::swap(&mut item, &mut self.data[0]);
                    self.sift_down_to_bottom(0);
                }
                Some(item)
            }
            None => None,
        }
    }

    pub fn push(&mut self, item: T) {
        let old_len = self.len();
        self.data.push(item);
        self.sift_up(0, old_len);
    }

    pub fn into_sorted_vec(self) -> Vec<T> {
        let mut s = self;
        let mut end = s.len();
        while end > 1 {
            end -= 1;
            s.data.swap(0, end);
            s.sift_down_range(0, end);
        }
        s.into_vec()
    }

    /// Moves the element at `pos` up while it is greater than its parent.
    fn sift_up(&mut self, start: usize, pos: usize) -> usize {
        let mut pos = pos;
        while pos > start {
            let parent = (pos - 1) / 2;
            if self.data[pos] <= self.data[parent] {
                break;
            }
            self.data.swap(pos, parent);
            pos = parent;
        }
        pos
    }

    /// Moves the element at `pos` down within [pos, end).
    fn sift_down_range(&mut self, pos: usize, end: usize) {
        let mut pos = pos;
        let mut child = 2 * pos + 1;
        while child + 1 < end {
            // pick the greater child (the right one on ties, as real std)
            if self.data[child] <= self.data[child + 1] {
                child += 1;
            }
            if self.data[pos] >= self.data[child] {
                return;
            }
            self.data.swap(pos, child);
            pos = child;
            child = 2 * pos + 1;
        }
        if child == end - 1 && self.data[pos] < self.data[child] {
            self.data.swap(pos, child);
        }
    }

    fn sift_down(&mut self, pos: usize) {
        let len = self.len();
        self.sift_down_range(pos, len);
    }

    /// Moves the element at `pos` all the way to a leaf, then sifts it up (real std's pop).
    fn sift_down_to_bottom(&mut self, pos: usize) {
        let end = self.len();
        let start = pos;
        let mut pos = pos;
        let mut child = 2 * pos + 1;
        while child + 1 < end {
            if self.data[child] <= self.data[child + 1] {
                child += 1;
            }
            self.data.swap(pos, child);
            pos = child;
            child = 2 * pos + 1;
        }
        if child == end - 1 {
            self.data.swap(pos, child);
            pos = child;
        }
        self.sift_up(start, pos);
    }

    fn rebuild(&mut self) {
        let mut n = self.len() / 2;
        while n > 0 {
            n -= 1;
            self.sift_down(n);
        }
    }

    pub fn append(&mut self, other: &mut BinaryHeap<T>) {
        if self.len() < other.len() {
            crate::mem::swap(self, other);
        }
        let start = self.data.len();
        self.data.append(&mut other.data);
        self.rebuild_tail(start);
    }

    /// Restores the heap after elements were added (or changed) from `start` on, choosing a
    /// full rebuild or per-element sift-ups by real std's cost estimate.
    fn rebuild_tail(&mut self, start: usize) {
        if start == self.len() {
            return;
        }
        let tail_len = self.len() - start;
        let better_to_rebuild = if start < tail_len {
            true
        } else if self.len() <= 2048 {
            2 * self.len() < tail_len * log2_fast(start)
        } else {
            2 * self.len() < tail_len * 11
        };
        if better_to_rebuild {
            self.rebuild();
        } else {
            let mut i = start;
            while i < self.len() {
                self.sift_up(0, i);
                i += 1;
            }
        }
    }

    pub fn retain<F: FnMut(&T) -> bool>(&mut self, f: F) {
        let mut f = f;
        let mut first_removed = self.len();
        let mut i = 0;
        self.data.retain(|e| {
            let keep = f(e);
            if !keep && i < first_removed {
                first_removed = i;
            }
            i += 1;
            keep
        });
        self.rebuild_tail(first_removed);
    }
}

fn log2_fast(x: usize) -> usize {
    (usize::BITS - x.leading_zeros() - 1) as usize
}

impl<T> BinaryHeap<T> {
    pub fn peek(&self) -> Option<&T> {
        self.data.first()
    }
    pub fn capacity(&self) -> usize {
        self.data.capacity()
    }
    pub fn reserve(&mut self, additional: usize) {
        self.data.reserve(additional);
    }
    pub fn reserve_exact(&mut self, additional: usize) {
        self.data.reserve_exact(additional);
    }
    pub fn shrink_to_fit(&mut self) {
        self.data.shrink_to_fit();
    }
    pub fn into_vec(self) -> Vec<T> {
        self.data
    }
    pub fn as_slice(&self) -> &[T] {
        self.data.as_slice()
    }
    pub fn len(&self) -> usize {
        self.data.len()
    }
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }
    pub fn iter(&self) -> crate::slice::Iter<'_, T> {
        crate::slice::Iter::new(self.data.as_slice())
    }
    pub fn drain(&mut self) -> crate::vec::Drain<'_, T> {
        self.data.drain(..)
    }
    pub fn clear(&mut self) {
        self.data.clear();
    }
}

/// `heap.peek_mut()`: re-sifts the top element when dropped.
pub struct PeekMut<'a, T: 'a + Ord> {
    heap: &'a mut BinaryHeap<T>,
}

impl<'a, T: Ord> PeekMut<'a, T> {
    pub fn pop(this: PeekMut<'a, T>) -> T {
        let heap: *mut BinaryHeap<T> = this.heap as *mut BinaryHeap<T>;
        crate::mem::forget(this);
        match unsafe { &mut *heap }.pop() {
            Some(v) => v,
            None => crate::panicking::panic_str("PeekMut on an empty heap"),
        }
    }
}

impl<'a, T: Ord> Deref for PeekMut<'a, T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.heap.data[0]
    }
}

impl<'a, T: Ord> DerefMut for PeekMut<'a, T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.heap.data[0]
    }
}

impl<'a, T: Ord> Drop for PeekMut<'a, T> {
    fn drop(&mut self) {
        self.heap.sift_down(0);
    }
}

impl<T: Clone> Clone for BinaryHeap<T> {
    fn clone(&self) -> BinaryHeap<T> {
        BinaryHeap { data: self.data.clone() }
    }
}

impl<T: Ord> Default for BinaryHeap<T> {
    fn default() -> BinaryHeap<T> {
        BinaryHeap::new()
    }
}

impl<T: fmt::Debug> fmt::Debug for BinaryHeap<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.data.iter()).finish()
    }
}

impl<T: Ord> From<Vec<T>> for BinaryHeap<T> {
    fn from(vec: Vec<T>) -> BinaryHeap<T> {
        let mut heap = BinaryHeap { data: vec };
        heap.rebuild();
        heap
    }
}

impl<T> From<BinaryHeap<T>> for Vec<T> {
    fn from(heap: BinaryHeap<T>) -> Vec<T> {
        heap.data
    }
}

impl<T: Ord> FromIterator<T> for BinaryHeap<T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> BinaryHeap<T> {
        let v: Vec<T> = iter.into_iter().collect();
        BinaryHeap::from(v)
    }
}

impl<T: Ord> Extend<T> for BinaryHeap<T> {
    fn extend<I: IntoIterator<Item = T>>(&mut self, iter: I) {
        for x in iter {
            self.push(x);
        }
    }
}

pub struct IntoIter<T> {
    iter: crate::vec::IntoIter<T>,
}

impl<T> Iterator for IntoIter<T> {
    type Item = T;
    fn next(&mut self) -> Option<T> {
        self.iter.next()
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.iter.size_hint()
    }
}

impl<T> DoubleEndedIterator for IntoIter<T> {
    fn next_back(&mut self) -> Option<T> {
        self.iter.next_back()
    }
}

impl<T> ExactSizeIterator for IntoIter<T> {}
impl<T> FusedIterator for IntoIter<T> {}

impl<T> IntoIterator for BinaryHeap<T> {
    type Item = T;
    type IntoIter = IntoIter<T>;
    fn into_iter(self) -> IntoIter<T> {
        IntoIter { iter: self.data.into_iter() }
    }
}

impl<'a, T> IntoIterator for &'a BinaryHeap<T> {
    type Item = &'a T;
    type IntoIter = crate::slice::Iter<'a, T>;
    fn into_iter(self) -> crate::slice::Iter<'a, T> {
        crate::slice::Iter::new(self.data.as_slice())
    }
}

#[cfg(any(rapid_check, rapid_const_generics))]
impl<T: Ord, const N: usize> From<[T; N]> for BinaryHeap<T> {
    fn from(arr: [T; N]) -> BinaryHeap<T> {
        let mut v = Vec::with_capacity(N);
        for x in arr {
            v.push(x);
        }
        BinaryHeap::from(v)
    }
}
