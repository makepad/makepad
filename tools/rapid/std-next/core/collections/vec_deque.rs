//! VecDeque: a growable ring buffer.

use crate::vec::Vec;
use super::resolve_range;
use crate::cmp::Ordering;
use crate::fmt;
use crate::hash::{Hash, Hasher};
use crate::iter::{FromIterator, FusedIterator};
use crate::marker::PhantomData;
use crate::ops::{Index, IndexMut, RangeBounds};
use crate::ptr;

pub struct VecDeque<T> {
    buf: *mut T,
    cap: usize,
    head: usize,
    len: usize,
    _m: PhantomData<T>,
}

unsafe impl<T: Send> Send for VecDeque<T> {}
unsafe impl<T: Sync> Sync for VecDeque<T> {}

/// Smallest non-zero capacity, as real RawVec: 8 for 1-byte elements, 4 up to 1 KiB, else 1.
fn min_non_zero_cap(elem_size: usize) -> usize {
    if elem_size == 1 {
        8
    } else if elem_size <= 1024 {
        4
    } else {
        1
    }
}

impl<T> VecDeque<T> {
    pub const fn new() -> VecDeque<T> {
        VecDeque { buf: ptr::null_mut(), cap: 0, head: 0, len: 0, _m: PhantomData }
    }

    pub fn with_capacity(capacity: usize) -> VecDeque<T> {
        let mut d = VecDeque::new();
        if crate::mem::size_of::<T>() == 0 {
            d.cap = usize::MAX;
            d.buf = ptr::NonNull::<T>::dangling().as_ptr();
        } else if capacity > 0 {
            d.alloc_exact(capacity);
        }
        d
    }

    fn alloc_exact(&mut self, capacity: usize) {
        let mut v: Vec<T> = Vec::with_capacity(capacity);
        self.buf = v.as_mut_ptr();
        self.cap = v.capacity();
        crate::mem::forget(v);
    }

    fn is_zst() -> bool {
        crate::mem::size_of::<T>() == 0
    }

    /// Physical index of logical index `idx` (idx < cap).
    #[inline]
    fn phys(&self, idx: usize) -> usize {
        let i = self.head.wrapping_add(idx);
        if i >= self.cap {
            i.wrapping_sub(self.cap)
        } else {
            i
        }
    }

    #[inline]
    fn slot(&self, logical: usize) -> *mut T {
        unsafe { self.buf.add(self.phys(logical)) }
    }

    /// Moves the contents into a buffer of `new_cap` (>= len), head becomes 0.
    fn realloc(&mut self, new_cap: usize) {
        let mut v: Vec<T> = Vec::with_capacity(new_cap);
        let dst = v.as_mut_ptr();
        let new_cap = v.capacity();
        crate::mem::forget(v);
        if self.len > 0 {
            let (a, b) = self.slice_ranges();
            unsafe {
                ptr::copy_nonoverlapping(self.buf.add(a.0), dst, a.1 - a.0);
                ptr::copy_nonoverlapping(self.buf.add(b.0), dst.add(a.1 - a.0), b.1 - b.0);
            }
        }
        self.free_buf();
        self.buf = dst;
        self.cap = new_cap;
        self.head = 0;
    }

    fn free_buf(&mut self) {
        if !Self::is_zst() && self.cap > 0 {
            unsafe { drop(Vec::from_raw_parts(self.buf, 0, self.cap)) };
        }
        self.cap = 0;
        self.buf = ptr::null_mut();
    }

    /// Physical [start, end) ranges of the first and second contiguous parts.
    fn slice_ranges(&self) -> ((usize, usize), (usize, usize)) {
        if self.len == 0 {
            return ((0, 0), (0, 0));
        }
        let tail_room = self.cap - self.head;
        if self.len <= tail_room {
            ((self.head, self.head + self.len), (0, 0))
        } else {
            ((self.head, self.cap), (0, self.len - tail_room))
        }
    }

    fn grow_for(&mut self, additional: usize) {
        let required = match self.len.checked_add(additional) {
            Some(r) => r,
            None => crate::panicking::panic_str("capacity overflow"),
        };
        if required <= self.cap {
            return;
        }
        let doubled = self.cap.saturating_mul(2);
        let mut new_cap = if doubled > required { doubled } else { required };
        let min = min_non_zero_cap(crate::mem::size_of::<T>());
        if new_cap < min {
            new_cap = min;
        }
        self.realloc(new_cap);
    }

    pub fn capacity(&self) -> usize {
        self.cap
    }
    pub fn reserve(&mut self, additional: usize) {
        self.grow_for(additional);
    }
    pub fn reserve_exact(&mut self, additional: usize) {
        let required = match self.len.checked_add(additional) {
            Some(r) => r,
            None => crate::panicking::panic_str("capacity overflow"),
        };
        if required > self.cap {
            self.realloc(required);
        }
    }
    pub fn try_reserve(&mut self, additional: usize) -> Result<(), super::TryReserveError> {
        self.reserve(additional);
        Ok(())
    }
    pub fn shrink_to_fit(&mut self) {
        self.shrink_to(0);
    }
    pub fn shrink_to(&mut self, min_capacity: usize) {
        if Self::is_zst() {
            return;
        }
        let target = if min_capacity > self.len { min_capacity } else { self.len };
        if target < self.cap {
            if target == 0 {
                self.free_buf();
                self.head = 0;
            } else {
                self.realloc(target);
            }
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn get(&self, index: usize) -> Option<&T> {
        if index < self.len {
            Some(unsafe { &*self.slot(index) })
        } else {
            None
        }
    }
    pub fn get_mut(&mut self, index: usize) -> Option<&mut T> {
        if index < self.len {
            Some(unsafe { &mut *self.slot(index) })
        } else {
            None
        }
    }
    pub fn front(&self) -> Option<&T> {
        self.get(0)
    }
    pub fn front_mut(&mut self) -> Option<&mut T> {
        self.get_mut(0)
    }
    pub fn back(&self) -> Option<&T> {
        if self.len == 0 {
            None
        } else {
            self.get(self.len - 1)
        }
    }
    pub fn back_mut(&mut self) -> Option<&mut T> {
        if self.len == 0 {
            None
        } else {
            let i = self.len - 1;
            self.get_mut(i)
        }
    }

    pub fn push_back(&mut self, value: T) {
        if self.len == self.cap {
            self.grow_for(1);
        }
        unsafe { ptr::write(self.slot(self.len), value) };
        self.len += 1;
    }
    pub fn push_front(&mut self, value: T) {
        if self.len == self.cap {
            self.grow_for(1);
        }
        self.head = if self.head == 0 { self.cap - 1 } else { self.head - 1 };
        unsafe { ptr::write(self.buf.add(self.head), value) };
        self.len += 1;
    }
    pub fn pop_front(&mut self) -> Option<T> {
        if self.len == 0 {
            return None;
        }
        let v = unsafe { ptr::read(self.buf.add(self.head)) };
        self.head = self.phys(1);
        self.len -= 1;
        Some(v)
    }
    pub fn pop_back(&mut self) -> Option<T> {
        if self.len == 0 {
            return None;
        }
        self.len -= 1;
        Some(unsafe { ptr::read(self.slot(self.len)) })
    }

    pub fn swap(&mut self, i: usize, j: usize) {
        if !(i < self.len) {
            crate::panicking::panic_str("assertion failed: i < self.len()");
        }
        if !(j < self.len) {
            crate::panicking::panic_str("assertion failed: j < self.len()");
        }
        if i != j {
            unsafe { ptr::swap(self.slot(i), self.slot(j)) };
        }
    }

    pub fn swap_remove_back(&mut self, index: usize) -> Option<T> {
        let length = self.len;
        if index < length && index != length - 1 {
            self.swap(index, length - 1);
        } else if index >= length {
            return None;
        }
        self.pop_back()
    }

    pub fn swap_remove_front(&mut self, index: usize) -> Option<T> {
        let length = self.len;
        if index < length && index != 0 {
            self.swap(index, 0);
        } else if index >= length {
            return None;
        }
        self.pop_front()
    }

    pub fn insert(&mut self, index: usize, value: T) {
        if index > self.len {
            crate::panicking::panic_str("index out of bounds");
        }
        if self.len == self.cap {
            self.grow_for(1);
        }
        if index < self.len - index {
            // shift the front part one to the left
            self.head = if self.head == 0 { self.cap - 1 } else { self.head - 1 };
            let mut i = 0;
            while i < index {
                unsafe { ptr::copy_nonoverlapping(self.slot(i + 1), self.slot(i), 1) };
                i += 1;
            }
        } else {
            let mut i = self.len;
            while i > index {
                unsafe { ptr::copy_nonoverlapping(self.slot(i - 1), self.slot(i), 1) };
                i -= 1;
            }
        }
        unsafe { ptr::write(self.slot(index), value) };
        self.len += 1;
    }

    pub fn remove(&mut self, index: usize) -> Option<T> {
        if index >= self.len {
            return None;
        }
        let v = unsafe { ptr::read(self.slot(index)) };
        if index < self.len - 1 - index {
            // close the gap from the front
            let mut i = index;
            while i > 0 {
                unsafe { ptr::copy_nonoverlapping(self.slot(i - 1), self.slot(i), 1) };
                i -= 1;
            }
            self.head = self.phys(1);
        } else {
            let mut i = index;
            while i + 1 < self.len {
                unsafe { ptr::copy_nonoverlapping(self.slot(i + 1), self.slot(i), 1) };
                i += 1;
            }
        }
        self.len -= 1;
        Some(v)
    }

    /// Drops the elements past `len`, front to back (as real std's slice drops do).
    pub fn truncate(&mut self, len: usize) {
        if len >= self.len {
            return;
        }
        let old = self.len;
        self.len = len;
        let mut i = len;
        while i < old {
            unsafe { ptr::drop_in_place(self.slot(i)) };
            i += 1;
        }
    }

    pub fn clear(&mut self) {
        self.truncate(0);
        self.head = 0;
    }

    pub fn contains(&self, x: &T) -> bool
    where
        T: PartialEq,
    {
        let (a, b) = self.as_slices();
        a.contains(x) || b.contains(x)
    }

    pub fn as_slices(&self) -> (&[T], &[T]) {
        let (a, b) = self.slice_ranges();
        unsafe {
            (
                crate::slice::from_raw_parts(self.buf_or_dangling().add(a.0), a.1 - a.0),
                crate::slice::from_raw_parts(self.buf_or_dangling().add(b.0), b.1 - b.0),
            )
        }
    }

    pub fn as_mut_slices(&mut self) -> (&mut [T], &mut [T]) {
        let (a, b) = self.slice_ranges();
        let p = self.buf_or_dangling();
        unsafe {
            (
                crate::slice::from_raw_parts_mut(p.add(a.0), a.1 - a.0),
                crate::slice::from_raw_parts_mut(p.add(b.0), b.1 - b.0),
            )
        }
    }

    fn buf_or_dangling(&self) -> *mut T {
        if self.buf.is_null() {
            ptr::NonNull::<T>::dangling().as_ptr()
        } else {
            self.buf
        }
    }

    pub fn make_contiguous(&mut self) -> &mut [T] {
        if self.head + self.len > self.cap && !Self::is_zst() {
            // read out in order, write back from the start of the buffer
            let mut tmp: Vec<T> = Vec::with_capacity(self.len);
            let mut i = 0;
            while i < self.len {
                tmp.push(unsafe { ptr::read(self.slot(i)) });
                i += 1;
            }
            unsafe {
                ptr::copy_nonoverlapping(tmp.as_ptr(), self.buf, self.len);
                tmp.set_len(0);
            }
            self.head = 0;
        }
        let p = self.buf_or_dangling();
        unsafe { crate::slice::from_raw_parts_mut(p.add(if self.len == 0 { 0 } else { self.head }), self.len) }
    }

    pub fn rotate_left(&mut self, n: usize) {
        if !(n <= self.len) {
            crate::panicking::panic_str("assertion failed: n <= self.len()");
        }
        let mut i = 0;
        while i < n {
            let v = self.pop_front();
            if let Some(v) = v {
                self.push_back(v);
            }
            i += 1;
        }
    }

    pub fn rotate_right(&mut self, n: usize) {
        if !(n <= self.len) {
            crate::panicking::panic_str("assertion failed: n <= self.len()");
        }
        let mut i = 0;
        while i < n {
            let v = self.pop_back();
            if let Some(v) = v {
                self.push_front(v);
            }
            i += 1;
        }
    }

    pub fn iter(&self) -> Iter<'_, T> {
        let (a, b) = self.as_slices();
        Iter { a: crate::slice::Iter::new(a), b: crate::slice::Iter::new(b) }
    }

    pub fn iter_mut(&mut self) -> IterMut<'_, T> {
        let (a, b) = self.as_mut_slices();
        IterMut { a: crate::slice::IterMut::new(a), b: crate::slice::IterMut::new(b) }
    }

    pub fn range<R: RangeBounds<usize>>(&self, range: R) -> Iter<'_, T> {
        let (start, end) = resolve_range(&range, self.len);
        let (a, b) = self.as_slices();
        let (a, b) = split_range(a, b, start, end);
        Iter { a: crate::slice::Iter::new(a), b: crate::slice::Iter::new(b) }
    }

    pub fn range_mut<R: RangeBounds<usize>>(&mut self, range: R) -> IterMut<'_, T> {
        let (start, end) = resolve_range(&range, self.len);
        let (a, b) = self.as_mut_slices();
        let alen = a.len();
        if end <= alen {
            let a = &mut a[start..end];
            IterMut { a: crate::slice::IterMut::new(a), b: crate::slice::IterMut::new(&mut []) }
        } else if start >= alen {
            let b = &mut b[start - alen..end - alen];
            IterMut { a: crate::slice::IterMut::new(b), b: crate::slice::IterMut::new(&mut []) }
        } else {
            let a = &mut a[start..];
            let b = &mut b[..end - alen];
            IterMut { a: crate::slice::IterMut::new(a), b: crate::slice::IterMut::new(b) }
        }
    }

    pub fn drain<R: RangeBounds<usize>>(&mut self, range: R) -> Drain<'_, T> {
        let (start, end) = resolve_range(&range, self.len);
        let orig_len = self.len;
        // until the Drain is dropped the deque only owns [0, start)
        self.len = start;
        Drain { deque: self as *mut VecDeque<T>, start, end, front: start, back: end, orig_len, _m: PhantomData }
    }

    pub fn append(&mut self, other: &mut VecDeque<T>) {
        self.reserve(other.len);
        while let Some(v) = other.pop_front() {
            self.push_back(v);
        }
    }

    pub fn retain<F: FnMut(&T) -> bool>(&mut self, f: F) {
        let mut f = f;
        self.retain_mut(|x| f(x));
    }

    pub fn retain_mut<F: FnMut(&mut T) -> bool>(&mut self, f: F) {
        let mut f = f;
        let len = self.len;
        let mut keep = 0;
        let mut i = 0;
        while i < len {
            let ok = f(unsafe { &mut *self.slot(i) });
            if ok {
                if i != keep {
                    unsafe { ptr::swap(self.slot(i), self.slot(keep)) };
                }
                keep += 1;
            }
            i += 1;
        }
        self.truncate(keep);
    }

    pub fn resize_with<F: FnMut() -> T>(&mut self, new_len: usize, generator: F) {
        let mut generator = generator;
        if new_len > self.len {
            self.reserve(new_len - self.len);
            while self.len < new_len {
                self.push_back(generator());
            }
        } else {
            self.truncate(new_len);
        }
    }

    pub fn split_off(&mut self, at: usize) -> VecDeque<T> {
        if at > self.len {
            crate::panicking::panic_str("`at` out of bounds");
        }
        let other_len = self.len - at;
        let mut other = VecDeque::with_capacity(other_len);
        let mut i = at;
        while i < self.len {
            other.push_back(unsafe { ptr::read(self.slot(i)) });
            i += 1;
        }
        self.len = at;
        other
    }

    /// As real std: search the back slice if its first element is <= the target.
    pub fn binary_search_by<F: FnMut(&T) -> Ordering>(&self, f: F) -> Result<usize, usize> {
        let mut f = f;
        let (front, back) = self.as_slices();
        let cmp_back = match back.first() {
            Some(elem) => Some(f(elem)),
            None => None,
        };
        match cmp_back {
            Some(Ordering::Equal) => Ok(front.len()),
            Some(Ordering::Less) => match back.binary_search_by(f) {
                Ok(i) => Ok(i + front.len()),
                Err(i) => Err(i + front.len()),
            },
            _ => front.binary_search_by(f),
        }
    }

    pub fn binary_search(&self, x: &T) -> Result<usize, usize>
    where
        T: Ord,
    {
        self.binary_search_by(|e| e.cmp(x))
    }

    pub fn binary_search_by_key<B: Ord, F: FnMut(&T) -> B>(&self, b: &B, f: F) -> Result<usize, usize> {
        let mut f = f;
        self.binary_search_by(|k| f(k).cmp(b))
    }

    pub fn partition_point<P: FnMut(&T) -> bool>(&self, pred: P) -> usize {
        let mut pred = pred;
        let (front, back) = self.as_slices();
        if let Some(b) = back.first() {
            if pred(b) {
                return back.partition_point(pred) + front.len();
            }
        }
        front.partition_point(pred)
    }
}

impl<T: Clone> VecDeque<T> {
    pub fn resize(&mut self, new_len: usize, value: T) {
        if new_len > self.len {
            self.reserve(new_len - self.len);
            while self.len < new_len {
                self.push_back(value.clone());
            }
        } else {
            self.truncate(new_len);
        }
    }
}

/// Sub-slices [start, end) of the logical concatenation a ++ b.
fn split_range<'a, T>(a: &'a [T], b: &'a [T], start: usize, end: usize) -> (&'a [T], &'a [T]) {
    let alen = a.len();
    if end <= alen {
        (&a[start..end], &b[..0])
    } else if start >= alen {
        (&b[start - alen..end - alen], &a[..0])
    } else {
        (&a[start..], &b[..end - alen])
    }
}

impl<T> Drop for VecDeque<T> {
    fn drop(&mut self) {
        self.truncate(0);
        self.free_buf();
    }
}

impl<T: Clone> Clone for VecDeque<T> {
    fn clone(&self) -> VecDeque<T> {
        let mut d = VecDeque::with_capacity(self.len);
        for x in self.iter() {
            d.push_back(x.clone());
        }
        d
    }
}

impl<T> Default for VecDeque<T> {
    fn default() -> VecDeque<T> {
        VecDeque::new()
    }
}

impl<T: PartialEq> PartialEq for VecDeque<T> {
    fn eq(&self, other: &VecDeque<T>) -> bool {
        self.len == other.len && self.iter().eq(other.iter())
    }
}

impl<T: Eq> Eq for VecDeque<T> {}

impl<T: PartialEq> PartialEq<Vec<T>> for VecDeque<T> {
    fn eq(&self, other: &Vec<T>) -> bool {
        self.len == other.len() && self.iter().eq(other.iter())
    }
}

impl<'a, T: PartialEq> PartialEq<&'a [T]> for VecDeque<T> {
    fn eq(&self, other: &&'a [T]) -> bool {
        self.len == other.len() && self.iter().eq(other.iter())
    }
}

impl<T: PartialOrd> PartialOrd for VecDeque<T> {
    fn partial_cmp(&self, other: &VecDeque<T>) -> Option<Ordering> {
        self.iter().partial_cmp(other.iter())
    }
}

impl<T: Ord> Ord for VecDeque<T> {
    fn cmp(&self, other: &VecDeque<T>) -> Ordering {
        self.iter().cmp(other.iter())
    }
}

impl<T: Hash> Hash for VecDeque<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_usize(self.len);
        for x in self.iter() {
            x.hash(state);
        }
    }
}

impl<T> Index<usize> for VecDeque<T> {
    type Output = T;
    fn index(&self, index: usize) -> &T {
        match self.get(index) {
            Some(v) => v,
            None => crate::panicking::panic_str("Out of bounds access"),
        }
    }
}

impl<T> IndexMut<usize> for VecDeque<T> {
    fn index_mut(&mut self, index: usize) -> &mut T {
        match self.get_mut(index) {
            Some(v) => v,
            None => crate::panicking::panic_str("Out of bounds access"),
        }
    }
}

impl<T: fmt::Debug> fmt::Debug for VecDeque<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

impl<T> FromIterator<T> for VecDeque<T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> VecDeque<T> {
        let iter = iter.into_iter();
        let (lower, _) = iter.size_hint();
        let mut d = VecDeque::with_capacity(lower);
        for x in iter {
            d.push_back(x);
        }
        d
    }
}

impl<T> Extend<T> for VecDeque<T> {
    fn extend<I: IntoIterator<Item = T>>(&mut self, iter: I) {
        let iter = iter.into_iter();
        let (lower, _) = iter.size_hint();
        self.reserve(lower);
        for x in iter {
            self.push_back(x);
        }
    }
}

impl<'a, T: 'a + Copy> Extend<&'a T> for VecDeque<T> {
    fn extend<I: IntoIterator<Item = &'a T>>(&mut self, iter: I) {
        for x in iter {
            self.push_back(*x);
        }
    }
}

impl<T> From<Vec<T>> for VecDeque<T> {
    fn from(other: Vec<T>) -> VecDeque<T> {
        let mut v = crate::mem::ManuallyDrop::new(other);
        let len = v.len();
        let cap = v.capacity();
        let buf = v.as_mut_ptr();
        if crate::mem::size_of::<T>() == 0 {
            VecDeque { buf: ptr::NonNull::<T>::dangling().as_ptr(), cap: usize::MAX, head: 0, len, _m: PhantomData }
        } else if cap == 0 {
            VecDeque::new()
        } else {
            VecDeque { buf, cap, head: 0, len, _m: PhantomData }
        }
    }
}

impl<T> From<VecDeque<T>> for Vec<T> {
    fn from(other: VecDeque<T>) -> Vec<T> {
        let mut other = other;
        if VecDeque::<T>::is_zst() {
            let mut v = Vec::new();
            while let Some(x) = other.pop_front() {
                v.push(x);
            }
            return v;
        }
        if other.cap == 0 {
            return Vec::new();
        }
        other.make_contiguous();
        // move the elements to the start of the buffer, hand the buffer to Vec
        let len = other.len;
        unsafe {
            if other.head != 0 {
                ptr::copy(other.buf.add(other.head), other.buf, len);
            }
            let v = Vec::from_raw_parts(other.buf, len, other.cap);
            other.buf = ptr::null_mut();
            other.cap = 0;
            other.len = 0;
            other.head = 0;
            v
        }
    }
}

// ---------------------------------------------------------------- iterators

pub struct Iter<'a, T: 'a> {
    a: crate::slice::Iter<'a, T>,
    b: crate::slice::Iter<'a, T>,
}

impl<'a, T> Clone for Iter<'a, T> {
    fn clone(&self) -> Iter<'a, T> {
        Iter { a: self.a.clone(), b: self.b.clone() }
    }
}

impl<'a, T> Iterator for Iter<'a, T> {
    type Item = &'a T;
    fn next(&mut self) -> Option<&'a T> {
        match self.a.next() {
            Some(x) => Some(x),
            None => {
                crate::mem::swap(&mut self.a, &mut self.b);
                self.a.next()
            }
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = self.a.len() + self.b.len();
        (n, Some(n))
    }
}

impl<'a, T> DoubleEndedIterator for Iter<'a, T> {
    fn next_back(&mut self) -> Option<&'a T> {
        match self.b.next_back() {
            Some(x) => Some(x),
            None => {
                crate::mem::swap(&mut self.a, &mut self.b);
                self.b.next_back()
            }
        }
    }
}

impl<'a, T> ExactSizeIterator for Iter<'a, T> {}
impl<'a, T> FusedIterator for Iter<'a, T> {}

impl<'a, T: fmt::Debug> fmt::Debug for Iter<'a, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Iter").field(&self.a.as_slice()).field(&self.b.as_slice()).finish()
    }
}

pub struct IterMut<'a, T: 'a> {
    a: crate::slice::IterMut<'a, T>,
    b: crate::slice::IterMut<'a, T>,
}

impl<'a, T> Iterator for IterMut<'a, T> {
    type Item = &'a mut T;
    fn next(&mut self) -> Option<&'a mut T> {
        match self.a.next() {
            Some(x) => Some(x),
            None => {
                crate::mem::swap(&mut self.a, &mut self.b);
                self.a.next()
            }
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = self.a.len() + self.b.len();
        (n, Some(n))
    }
}

impl<'a, T> DoubleEndedIterator for IterMut<'a, T> {
    fn next_back(&mut self) -> Option<&'a mut T> {
        match self.b.next_back() {
            Some(x) => Some(x),
            None => {
                crate::mem::swap(&mut self.a, &mut self.b);
                self.b.next_back()
            }
        }
    }
}

impl<'a, T> ExactSizeIterator for IterMut<'a, T> {}
impl<'a, T> FusedIterator for IterMut<'a, T> {}

pub struct IntoIter<T> {
    inner: VecDeque<T>,
}

impl<T> Iterator for IntoIter<T> {
    type Item = T;
    fn next(&mut self) -> Option<T> {
        self.inner.pop_front()
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.inner.len, Some(self.inner.len))
    }
}

impl<T> DoubleEndedIterator for IntoIter<T> {
    fn next_back(&mut self) -> Option<T> {
        self.inner.pop_back()
    }
}

impl<T> ExactSizeIterator for IntoIter<T> {}
impl<T> FusedIterator for IntoIter<T> {}

pub struct Drain<'a, T: 'a> {
    deque: *mut VecDeque<T>,
    start: usize,
    end: usize,
    front: usize,
    back: usize,
    orig_len: usize,
    _m: PhantomData<&'a mut VecDeque<T>>,
}

impl<'a, T> Iterator for Drain<'a, T> {
    type Item = T;
    fn next(&mut self) -> Option<T> {
        if self.front == self.back {
            return None;
        }
        let d = unsafe { &*self.deque };
        let v = unsafe { ptr::read(d.slot(self.front)) };
        self.front += 1;
        Some(v)
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = self.back - self.front;
        (n, Some(n))
    }
}

impl<'a, T> DoubleEndedIterator for Drain<'a, T> {
    fn next_back(&mut self) -> Option<T> {
        if self.front == self.back {
            return None;
        }
        self.back -= 1;
        let d = unsafe { &*self.deque };
        Some(unsafe { ptr::read(d.slot(self.back)) })
    }
}

impl<'a, T> ExactSizeIterator for Drain<'a, T> {}
impl<'a, T> FusedIterator for Drain<'a, T> {}

impl<'a, T> Drop for Drain<'a, T> {
    fn drop(&mut self) {
        let d = unsafe { &mut *self.deque };
        // drop what was not yielded
        while self.front < self.back {
            unsafe { ptr::drop_in_place(d.slot(self.front)) };
            self.front += 1;
        }
        // move the tail down over the gap
        let tail = self.orig_len - self.end;
        let mut i = 0;
        while i < tail {
            unsafe { ptr::copy_nonoverlapping(d.slot(self.end + i), d.slot(self.start + i), 1) };
            i += 1;
        }
        d.len = self.start + tail;
    }
}

impl<'a, T> IntoIterator for &'a VecDeque<T> {
    type Item = &'a T;
    type IntoIter = Iter<'a, T>;
    fn into_iter(self) -> Iter<'a, T> {
        self.iter()
    }
}

impl<'a, T> IntoIterator for &'a mut VecDeque<T> {
    type Item = &'a mut T;
    type IntoIter = IterMut<'a, T>;
    fn into_iter(self) -> IterMut<'a, T> {
        self.iter_mut()
    }
}

impl<T> IntoIterator for VecDeque<T> {
    type Item = T;
    type IntoIter = IntoIter<T>;
    fn into_iter(self) -> IntoIter<T> {
        IntoIter { inner: self }
    }
}

#[cfg(any(rapid_check, rapid_const_generics))]
impl<T, const N: usize> From<[T; N]> for VecDeque<T> {
    fn from(arr: [T; N]) -> VecDeque<T> {
        let mut d = VecDeque::with_capacity(N);
        for x in arr {
            d.push_back(x);
        }
        d
    }
}
