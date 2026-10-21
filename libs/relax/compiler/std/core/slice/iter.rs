//! Slice iterators (general path; Relax fuses `for x in s.iter()` chains into loops).

use crate::fmt;
use crate::iter::{DoubleEndedIterator, ExactSizeIterator, FusedIterator, Iterator};

fn take_mut<'a, T>(s: &mut &'a mut [T]) -> &'a mut [T] {
    crate::mem::replace(s, &mut [])
}

// ---------------------------------------------------------------- Iter / IterMut

// An iterator holds the next element's pointer and the end pointer (rustc's core does the same):
// a step is one compare and one add, where a (pointer, length) pair needed a test, an add and a
// subtract. For a zero-sized T both pointers stay put as addresses and `end` counts the
// remaining elements in bytes past `ptr` (the pointer never moves).

#[inline]
fn zst<T>() -> bool {
    crate::mem::size_of::<T>() == 0
}

#[inline]
fn end_of<T>(p: *const T, len: usize) -> *const T {
    if zst::<T>() {
        (p as *const u8).wrapping_add(len) as *const T
    } else {
        // SAFETY: one past the end of the slice
        unsafe { p.add(len) }
    }
}

#[inline]
fn len_between<T>(p: *const T, end: *const T) -> usize {
    let bytes = end as usize - p as usize;
    if zst::<T>() { bytes } else { bytes / crate::mem::size_of::<T>() }
}

pub struct Iter<'a, T> {
    ptr: *const T,
    end: *const T,
    _m: crate::marker::PhantomData<&'a T>,
}

// as the reference it stands for (rustc: core::slice::Iter)
unsafe impl<T: Sync> Send for Iter<'_, T> {}
unsafe impl<T: Sync> Sync for Iter<'_, T> {}

pub fn iter<T>(s: &[T]) -> Iter<'_, T> {
    Iter::new(s)
}

impl<'a, T> Iter<'a, T> {
    pub(crate) fn new(s: &'a [T]) -> Iter<'a, T> {
        let ptr = s.as_ptr();
        Iter { ptr, end: end_of(ptr, s.len()), _m: crate::marker::PhantomData }
    }
    pub fn as_slice(&self) -> &'a [T] {
        // SAFETY: ptr..end is the rest of the slice the iterator was made from
        unsafe { super::from_raw_parts(self.ptr, len_between(self.ptr, self.end)) }
    }
    #[inline]
    fn len_left(&self) -> usize {
        len_between(self.ptr, self.end)
    }
}

impl<'a, T> Clone for Iter<'a, T> {
    fn clone(&self) -> Iter<'a, T> {
        Iter { ptr: self.ptr, end: self.end, _m: crate::marker::PhantomData }
    }
}

impl<'a, T> Default for Iter<'a, T> {
    fn default() -> Iter<'a, T> {
        Iter::new(&[])
    }
}

impl<'a, T: fmt::Debug> fmt::Debug for Iter<'a, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Iter").field(&self.as_slice()).finish()
    }
}

impl<'a, T> Iterator for Iter<'a, T> {
    type Item = &'a T;
    #[inline]
    fn next(&mut self) -> Option<&'a T> {
        if self.ptr == self.end {
            return None;
        }
        // SAFETY: ptr < end: ptr points at the next element
        unsafe {
            let p = self.ptr;
            if zst::<T>() {
                self.end = (self.end as *const u8).wrapping_sub(1) as *const T;
            } else {
                self.ptr = p.add(1);
            }
            Some(&*p)
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = self.len_left();
        (n, Some(n))
    }
    fn count(self) -> usize {
        self.len_left()
    }
    fn nth(&mut self, n: usize) -> Option<&'a T> {
        if n >= self.len_left() {
            if zst::<T>() {
                self.end = self.ptr;
            } else {
                self.ptr = self.end;
            }
            None
        } else {
            // SAFETY: n < len: element n and the rest exist
            unsafe {
                if zst::<T>() {
                    self.end = (self.end as *const u8).wrapping_sub(n + 1) as *const T;
                    Some(&*self.ptr)
                } else {
                    let r = self.ptr.add(n);
                    self.ptr = r.add(1);
                    Some(&*r)
                }
            }
        }
    }
    fn last(self) -> Option<&'a T> {
        self.as_slice().last()
    }
    fn position<P: FnMut(&'a T) -> bool>(&mut self, predicate: P) -> Option<usize> {
        let mut predicate = predicate;
        let mut i = 0;
        while let Some(x) = self.next() {
            if predicate(x) {
                return Some(i);
            }
            i += 1;
        }
        None
    }
}

impl<'a, T> DoubleEndedIterator for Iter<'a, T> {
    #[inline]
    fn next_back(&mut self) -> Option<&'a T> {
        if self.ptr == self.end {
            return None;
        }
        // SAFETY: ptr < end: the last element is at end - 1
        unsafe {
            if zst::<T>() {
                self.end = (self.end as *const u8).wrapping_sub(1) as *const T;
                Some(&*self.ptr)
            } else {
                self.end = self.end.sub(1);
                Some(&*self.end)
            }
        }
    }
    fn nth_back(&mut self, n: usize) -> Option<&'a T> {
        let len = self.len_left();
        if n >= len {
            self.end = self.ptr;
            None
        } else {
            // SAFETY: n < len
            unsafe {
                if zst::<T>() {
                    self.end = (self.end as *const u8).wrapping_sub(n + 1) as *const T;
                    Some(&*self.ptr)
                } else {
                    self.end = self.end.sub(n + 1);
                    Some(&*self.end)
                }
            }
        }
    }
}

impl<'a, T> ExactSizeIterator for Iter<'a, T> {}
impl<'a, T> FusedIterator for Iter<'a, T> {}

pub struct IterMut<'a, T> {
    ptr: *mut T,
    end: *mut T,
    _m: crate::marker::PhantomData<&'a mut T>,
}

// as the reference it stands for (rustc: core::slice::IterMut)
unsafe impl<T: Send> Send for IterMut<'_, T> {}
unsafe impl<T: Sync> Sync for IterMut<'_, T> {}

pub fn iter_mut<T>(s: &mut [T]) -> IterMut<'_, T> {
    IterMut::new(s)
}

impl<'a, T> IterMut<'a, T> {
    pub(crate) fn new(s: &'a mut [T]) -> IterMut<'a, T> {
        let len = s.len();
        let ptr = s.as_mut_ptr();
        IterMut { ptr, end: end_of(ptr as *const T, len) as *mut T, _m: crate::marker::PhantomData }
    }
    pub fn into_slice(self) -> &'a mut [T] {
        // SAFETY: ptr..end is the rest of the slice, borrowed mutably for 'a
        unsafe { super::from_raw_parts_mut(self.ptr, len_between(self.ptr as *const T, self.end as *const T)) }
    }
    pub fn as_slice(&self) -> &[T] {
        // SAFETY: as into_slice, shared for the borrow of self
        unsafe { super::from_raw_parts(self.ptr as *const T, self.len_left()) }
    }
    #[inline]
    fn len_left(&self) -> usize {
        len_between(self.ptr as *const T, self.end as *const T)
    }
}

impl<'a, T: fmt::Debug> fmt::Debug for IterMut<'a, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("IterMut").field(&self.as_slice()).finish()
    }
}

impl<'a, T> Iterator for IterMut<'a, T> {
    type Item = &'a mut T;
    #[inline]
    fn next(&mut self) -> Option<&'a mut T> {
        if self.ptr == self.end {
            return None;
        }
        // SAFETY: ptr < end; each element is handed out once
        unsafe {
            let p = self.ptr;
            if zst::<T>() {
                self.end = (self.end as *mut u8).wrapping_sub(1) as *mut T;
            } else {
                self.ptr = p.add(1);
            }
            Some(&mut *p)
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = self.len_left();
        (n, Some(n))
    }
    fn count(self) -> usize {
        self.len_left()
    }
    fn nth(&mut self, n: usize) -> Option<&'a mut T> {
        if n >= self.len_left() {
            if zst::<T>() {
                self.end = self.ptr;
            } else {
                self.ptr = self.end;
            }
            None
        } else {
            // SAFETY: n < len
            unsafe {
                if zst::<T>() {
                    self.end = (self.end as *mut u8).wrapping_sub(n + 1) as *mut T;
                    Some(&mut *self.ptr)
                } else {
                    let r = self.ptr.add(n);
                    self.ptr = r.add(1);
                    Some(&mut *r)
                }
            }
        }
    }
}

impl<'a, T> DoubleEndedIterator for IterMut<'a, T> {
    #[inline]
    fn next_back(&mut self) -> Option<&'a mut T> {
        if self.ptr == self.end {
            return None;
        }
        // SAFETY: ptr < end
        unsafe {
            if zst::<T>() {
                self.end = (self.end as *mut u8).wrapping_sub(1) as *mut T;
                Some(&mut *self.ptr)
            } else {
                self.end = self.end.sub(1);
                Some(&mut *self.end)
            }
        }
    }
}

impl<'a, T> ExactSizeIterator for IterMut<'a, T> {}
impl<'a, T> FusedIterator for IterMut<'a, T> {}

// ---------------------------------------------------------------- Windows

pub struct Windows<'a, T> {
    v: &'a [T],
    size: usize,
}

#[track_caller]
pub fn windows<T>(v: &[T], size: usize) -> Windows<'_, T> {
    if size == 0 {
        crate::panicking::panic_str("window size must be non-zero");
    }
    Windows { v, size }
}
impl<'a, T> Clone for Windows<'a, T> {
    fn clone(&self) -> Self {
        Windows {
            v: self.v,
            size: self.size,
        }
    }
}
impl<'a, T: fmt::Debug> fmt::Debug for Windows<'a, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Windows")
            .field("v", &self.v)
            .field("size", &self.size)
            .finish()
    }
}
impl<'a, T> Iterator for Windows<'a, T> {
    type Item = &'a [T];
    fn next(&mut self) -> Option<Self::Item> {
        if self.size > self.v.len() {
            None
        } else {
            let r = &self.v[..self.size];
            self.v = &self.v[1..];
            Some(r)
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = if self.size > self.v.len() {
            0
        } else {
            self.v.len() - self.size + 1
        };
        (n, Some(n))
    }
    fn count(self) -> usize {
        self.len()
    }
    fn nth(&mut self, n: usize) -> Option<Self::Item> {
        match n.checked_add(self.size) {
            Some(end) if end <= self.v.len() => {
                let r = &self.v[n..end];
                self.v = &self.v[n + 1..];
                Some(r)
            }
            _ => {
                self.v = &self.v[..0];
                None
            }
        }
    }
    fn last(mut self) -> Option<Self::Item> {
        self.next_back()
    }
}
impl<'a, T> DoubleEndedIterator for Windows<'a, T> {
    fn next_back(&mut self) -> Option<Self::Item> {
        self.nth_back(0)
    }
    fn nth_back(&mut self, n: usize) -> Option<Self::Item> {
        if n < self.len() {
            let end = self.v.len() - n;
            let r = &self.v[end - self.size..end];
            self.v = &self.v[..end - 1];
            Some(r)
        } else {
            self.v = &self.v[..0];
            None
        }
    }
}
impl<'a, T> ExactSizeIterator for Windows<'a, T> {}
impl<'a, T> FusedIterator for Windows<'a, T> {}

// ---------------------------------------------------------------- Chunks
// Keep mutable remaining slices disjoint from every yielded slice. Taking the
// remaining reference before splitting makes this invariant explicit without
// core's raw-pointer representation (needed there for unchecked random access).

// Core's mutable chunk iterators expose a raw slice pointer in Debug. Relax's
// generic pointer formatter currently prints only its address, so retain the
// slice metadata here as well.
struct SlicePointerDebug<T> {
    addr: *const T,
    metadata: usize,
}
impl<T> fmt::Debug for SlicePointerDebug<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pointer")
            .field("addr", &self.addr)
            .field("metadata", &self.metadata)
            .finish()
    }
}

pub struct Chunks<'a, T> {
    v: &'a [T],
    chunk_size: usize,
}
#[track_caller]
pub fn chunks<T>(v: &[T], chunk_size: usize) -> Chunks<'_, T> {
    if chunk_size == 0 {
        crate::panicking::panic_str("chunk size must be non-zero");
    }
    Chunks { v, chunk_size }
}
impl<'a, T> Clone for Chunks<'a, T> {
    fn clone(&self) -> Self {
        Chunks {
            v: self.v,
            chunk_size: self.chunk_size,
        }
    }
}
impl<'a, T: fmt::Debug> fmt::Debug for Chunks<'a, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Chunks")
            .field("v", &self.v)
            .field("chunk_size", &self.chunk_size)
            .finish()
    }
}
impl<'a, T> Iterator for Chunks<'a, T> {
    type Item = &'a [T];
    fn next(&mut self) -> Option<Self::Item> {
        if self.v.is_empty() {
            return None;
        }
        let size = crate::cmp::min(self.v.len(), self.chunk_size);
        let mid = size;
        let (a, b) = self.v.split_at(mid);
        self.v = b;
        Some(a)
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = self.v.len() / self.chunk_size
            + if self.v.len() % self.chunk_size != 0 {
                1
            } else {
                0
            };
        (n, Some(n))
    }
    fn count(self) -> usize {
        self.len()
    }
    fn nth(&mut self, n: usize) -> Option<Self::Item> {
        match n.checked_mul(self.chunk_size) {
            Some(offset) if offset < self.v.len() => {
                let start = offset;
                let end = start + crate::cmp::min(self.v.len() - start, self.chunk_size);
                let (_, v) = self.v.split_at(start);
                let (chunk, rest) = v.split_at(end - start);
                self.v = rest;
                Some(chunk)
            }
            _ => {
                self.v = &self.v[..0];
                None
            }
        }
    }
    fn last(mut self) -> Option<Self::Item> {
        self.next_back()
    }
}
impl<'a, T> DoubleEndedIterator for Chunks<'a, T> {
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.v.is_empty() {
            return None;
        }
        let remainder = self.v.len() % self.chunk_size;
        let size = if remainder != 0 {
            remainder
        } else {
            self.chunk_size
        };
        let mid = self.v.len() - size;
        let (a, b) = self.v.split_at(mid);
        self.v = a;
        Some(b)
    }
    fn nth_back(&mut self, n: usize) -> Option<Self::Item> {
        let len = self.len();
        if n >= len {
            self.v = &self.v[..0];
            return None;
        }
        let start = (len - 1 - n) * self.chunk_size;
        let end = start + crate::cmp::min(self.v.len() - start, self.chunk_size);
        let (v, _) = self.v.split_at(end);
        let (rest, chunk) = v.split_at(start);
        self.v = rest;
        Some(chunk)
    }
}
impl<'a, T> ExactSizeIterator for Chunks<'a, T> {}
impl<'a, T> FusedIterator for Chunks<'a, T> {}

pub struct ChunksMut<'a, T> {
    v: &'a mut [T],
    chunk_size: usize,
}
#[track_caller]
pub fn chunks_mut<T>(v: &mut [T], chunk_size: usize) -> ChunksMut<'_, T> {
    if chunk_size == 0 {
        crate::panicking::panic_str("chunk size must be non-zero");
    }
    ChunksMut { v, chunk_size }
}
impl<'a, T: fmt::Debug> fmt::Debug for ChunksMut<'a, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let v = SlicePointerDebug {
            addr: self.v.as_ptr(),
            metadata: self.v.len(),
        };
        let marker: crate::marker::PhantomData<&'a mut T> = crate::marker::PhantomData;
        f.debug_struct("ChunksMut")
            .field("v", &v)
            .field("chunk_size", &self.chunk_size)
            .field("_marker", &marker)
            .finish()
    }
}
impl<'a, T> Iterator for ChunksMut<'a, T> {
    type Item = &'a mut [T];
    fn next(&mut self) -> Option<Self::Item> {
        if self.v.is_empty() {
            return None;
        }
        let size = crate::cmp::min(self.v.len(), self.chunk_size);
        let mid = size;
        let (a, b) = take_mut(&mut self.v).split_at_mut(mid);
        self.v = b;
        Some(a)
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = self.v.len() / self.chunk_size
            + if self.v.len() % self.chunk_size != 0 {
                1
            } else {
                0
            };
        (n, Some(n))
    }
    fn count(self) -> usize {
        self.len()
    }
    fn nth(&mut self, n: usize) -> Option<Self::Item> {
        match n.checked_mul(self.chunk_size) {
            Some(offset) if offset < self.v.len() => {
                let start = offset;
                let end = start + crate::cmp::min(self.v.len() - start, self.chunk_size);
                let (_, v) = take_mut(&mut self.v).split_at_mut(start);
                let (chunk, rest) = v.split_at_mut(end - start);
                self.v = rest;
                Some(chunk)
            }
            _ => {
                self.v = &mut [];
                None
            }
        }
    }
    fn last(mut self) -> Option<Self::Item> {
        self.next_back()
    }
}
impl<'a, T> DoubleEndedIterator for ChunksMut<'a, T> {
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.v.is_empty() {
            return None;
        }
        let remainder = self.v.len() % self.chunk_size;
        let size = if remainder != 0 {
            remainder
        } else {
            self.chunk_size
        };
        let mid = self.v.len() - size;
        let (a, b) = take_mut(&mut self.v).split_at_mut(mid);
        self.v = a;
        Some(b)
    }
    fn nth_back(&mut self, n: usize) -> Option<Self::Item> {
        let len = self.len();
        if n >= len {
            self.v = &mut [];
            return None;
        }
        let start = (len - 1 - n) * self.chunk_size;
        let end = start + crate::cmp::min(self.v.len() - start, self.chunk_size);
        let (v, _) = take_mut(&mut self.v).split_at_mut(end);
        let (rest, chunk) = v.split_at_mut(start);
        self.v = rest;
        Some(chunk)
    }
}
impl<'a, T> ExactSizeIterator for ChunksMut<'a, T> {}
impl<'a, T> FusedIterator for ChunksMut<'a, T> {}

pub struct ChunksExact<'a, T> {
    v: &'a [T],
    rem: &'a [T],
    chunk_size: usize,
}
#[track_caller]
pub fn chunks_exact<T>(v: &[T], chunk_size: usize) -> ChunksExact<'_, T> {
    if chunk_size == 0 {
        crate::panicking::panic_str("chunk size must be non-zero");
    }
    let rem_len = v.len() % chunk_size;
    let mid = v.len() - rem_len;
    let (a, b) = v.split_at(mid);
    ChunksExact {
        v: a,
        rem: b,
        chunk_size,
    }
}
impl<'a, T> ChunksExact<'a, T> {
    pub fn remainder(&self) -> &'a [T] {
        self.rem
    }
}
impl<'a, T> Clone for ChunksExact<'a, T> {
    fn clone(&self) -> Self {
        ChunksExact {
            v: self.v,
            rem: self.rem,
            chunk_size: self.chunk_size,
        }
    }
}
impl<'a, T: fmt::Debug> fmt::Debug for ChunksExact<'a, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChunksExact")
            .field("v", &self.v)
            .field("rem", &self.rem)
            .field("chunk_size", &self.chunk_size)
            .finish()
    }
}
impl<'a, T> Iterator for ChunksExact<'a, T> {
    type Item = &'a [T];
    // as core: a chunk is exactly chunk_size long (the remainder was split off at construction),
    // so `chunk[0..chunk_size]` indexing needs no bounds check once this is inlined
    fn next(&mut self) -> Option<Self::Item> {
        if self.v.len() < self.chunk_size {
            return None;
        }
        // The constructor separates the remainder; every yielded slice has
        // exactly chunk_size elements. Keep that length explicit for callers.
        let (a, b) = self.v.split_at(self.chunk_size);
        self.v = b;
        Some(a)
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = self.v.len() / self.chunk_size;
        (n, Some(n))
    }
    fn count(self) -> usize {
        self.len()
    }
    fn nth(&mut self, n: usize) -> Option<Self::Item> {
        match n.checked_mul(self.chunk_size) {
            Some(offset) if offset < self.v.len() => {
                let start = offset;
                let end = start + crate::cmp::min(self.v.len() - start, self.chunk_size);
                let (_, v) = self.v.split_at(start);
                let (chunk, rest) = v.split_at(end - start);
                self.v = rest;
                Some(chunk)
            }
            _ => {
                self.v = &self.v[..0];
                None
            }
        }
    }
    fn last(mut self) -> Option<Self::Item> {
        self.next_back()
    }
}
impl<'a, T> DoubleEndedIterator for ChunksExact<'a, T> {
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.v.is_empty() {
            return None;
        }
        let remainder = self.v.len() % self.chunk_size;
        let size = if remainder != 0 {
            remainder
        } else {
            self.chunk_size
        };
        let mid = self.v.len() - size;
        let (a, b) = self.v.split_at(mid);
        self.v = a;
        Some(b)
    }
    fn nth_back(&mut self, n: usize) -> Option<Self::Item> {
        let len = self.len();
        if n >= len {
            self.v = &self.v[..0];
            return None;
        }
        let start = (len - 1 - n) * self.chunk_size;
        let end = start + crate::cmp::min(self.v.len() - start, self.chunk_size);
        let (v, _) = self.v.split_at(end);
        let (rest, chunk) = v.split_at(start);
        self.v = rest;
        Some(chunk)
    }
}
impl<'a, T> ExactSizeIterator for ChunksExact<'a, T> {}
impl<'a, T> FusedIterator for ChunksExact<'a, T> {}

pub struct ChunksExactMut<'a, T> {
    v: &'a mut [T],
    rem: &'a mut [T],
    chunk_size: usize,
}
#[track_caller]
pub fn chunks_exact_mut<T>(v: &mut [T], chunk_size: usize) -> ChunksExactMut<'_, T> {
    if chunk_size == 0 {
        crate::panicking::panic_str("chunk size must be non-zero");
    }
    let rem_len = v.len() % chunk_size;
    let mid = v.len() - rem_len;
    let (a, b) = v.split_at_mut(mid);
    ChunksExactMut {
        v: a,
        rem: b,
        chunk_size,
    }
}
impl<'a, T> ChunksExactMut<'a, T> {
    pub fn into_remainder(self) -> &'a mut [T] {
        self.rem
    }
}
impl<'a, T: fmt::Debug> fmt::Debug for ChunksExactMut<'a, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let v = SlicePointerDebug {
            addr: self.v.as_ptr(),
            metadata: self.v.len(),
        };
        let marker: crate::marker::PhantomData<&'a mut T> = crate::marker::PhantomData;
        f.debug_struct("ChunksExactMut")
            .field("v", &v)
            .field("rem", &self.rem)
            .field("chunk_size", &self.chunk_size)
            .field("_marker", &marker)
            .finish()
    }
}
impl<'a, T> Iterator for ChunksExactMut<'a, T> {
    type Item = &'a mut [T];
    fn next(&mut self) -> Option<Self::Item> {
        if self.v.len() < self.chunk_size {
            return None;
        }
        // The constructor separates the remainder; every yielded slice has
        // exactly chunk_size elements. Keep that length explicit for callers.
        let (a, b) = take_mut(&mut self.v).split_at_mut(self.chunk_size);
        self.v = b;
        Some(a)
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = self.v.len() / self.chunk_size;
        (n, Some(n))
    }
    fn count(self) -> usize {
        self.len()
    }
    fn nth(&mut self, n: usize) -> Option<Self::Item> {
        match n.checked_mul(self.chunk_size) {
            Some(offset) if offset < self.v.len() => {
                let start = offset;
                let end = start + crate::cmp::min(self.v.len() - start, self.chunk_size);
                let (_, v) = take_mut(&mut self.v).split_at_mut(start);
                let (chunk, rest) = v.split_at_mut(end - start);
                self.v = rest;
                Some(chunk)
            }
            _ => {
                self.v = &mut [];
                None
            }
        }
    }
    fn last(mut self) -> Option<Self::Item> {
        self.next_back()
    }
}
impl<'a, T> DoubleEndedIterator for ChunksExactMut<'a, T> {
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.v.is_empty() {
            return None;
        }
        let remainder = self.v.len() % self.chunk_size;
        let size = if remainder != 0 {
            remainder
        } else {
            self.chunk_size
        };
        let mid = self.v.len() - size;
        let (a, b) = take_mut(&mut self.v).split_at_mut(mid);
        self.v = a;
        Some(b)
    }
    fn nth_back(&mut self, n: usize) -> Option<Self::Item> {
        let len = self.len();
        if n >= len {
            self.v = &mut [];
            return None;
        }
        let start = (len - 1 - n) * self.chunk_size;
        let end = start + crate::cmp::min(self.v.len() - start, self.chunk_size);
        let (v, _) = take_mut(&mut self.v).split_at_mut(end);
        let (rest, chunk) = v.split_at_mut(start);
        self.v = rest;
        Some(chunk)
    }
}
impl<'a, T> ExactSizeIterator for ChunksExactMut<'a, T> {}
impl<'a, T> FusedIterator for ChunksExactMut<'a, T> {}

pub struct RChunks<'a, T> {
    v: &'a [T],
    chunk_size: usize,
}
#[track_caller]
pub fn rchunks<T>(v: &[T], chunk_size: usize) -> RChunks<'_, T> {
    if chunk_size == 0 {
        crate::panicking::panic_str("chunk size must be non-zero");
    }
    RChunks { v, chunk_size }
}
impl<'a, T> Clone for RChunks<'a, T> {
    fn clone(&self) -> Self {
        RChunks {
            v: self.v,
            chunk_size: self.chunk_size,
        }
    }
}
impl<'a, T: fmt::Debug> fmt::Debug for RChunks<'a, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RChunks")
            .field("v", &self.v)
            .field("chunk_size", &self.chunk_size)
            .finish()
    }
}
impl<'a, T> Iterator for RChunks<'a, T> {
    type Item = &'a [T];
    fn next(&mut self) -> Option<Self::Item> {
        if self.v.is_empty() {
            return None;
        }
        let size = crate::cmp::min(self.v.len(), self.chunk_size);
        let mid = self.v.len() - size;
        let (a, b) = self.v.split_at(mid);
        self.v = a;
        Some(b)
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = self.v.len() / self.chunk_size
            + if self.v.len() % self.chunk_size != 0 {
                1
            } else {
                0
            };
        (n, Some(n))
    }
    fn count(self) -> usize {
        self.len()
    }
    fn nth(&mut self, n: usize) -> Option<Self::Item> {
        match n.checked_mul(self.chunk_size) {
            Some(offset) if offset < self.v.len() => {
                let end = self.v.len() - offset;
                let start = end.saturating_sub(self.chunk_size);
                let (v, _) = self.v.split_at(end);
                let (rest, chunk) = v.split_at(start);
                self.v = rest;
                Some(chunk)
            }
            _ => {
                self.v = &self.v[..0];
                None
            }
        }
    }
    fn last(mut self) -> Option<Self::Item> {
        self.next_back()
    }
}
impl<'a, T> DoubleEndedIterator for RChunks<'a, T> {
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.v.is_empty() {
            return None;
        }
        let remainder = self.v.len() % self.chunk_size;
        let size = if remainder != 0 {
            remainder
        } else {
            self.chunk_size
        };
        let mid = size;
        let (a, b) = self.v.split_at(mid);
        self.v = b;
        Some(a)
    }
    fn nth_back(&mut self, n: usize) -> Option<Self::Item> {
        let len = self.len();
        if n >= len {
            self.v = &self.v[..0];
            return None;
        }
        let end = self.v.len() - (len - 1 - n) * self.chunk_size;
        let start = end.saturating_sub(self.chunk_size);
        let (v, rest) = self.v.split_at(end);
        let (_, chunk) = v.split_at(start);
        self.v = rest;
        Some(chunk)
    }
}
impl<'a, T> ExactSizeIterator for RChunks<'a, T> {}
impl<'a, T> FusedIterator for RChunks<'a, T> {}

pub struct RChunksMut<'a, T> {
    v: &'a mut [T],
    chunk_size: usize,
}
#[track_caller]
pub fn rchunks_mut<T>(v: &mut [T], chunk_size: usize) -> RChunksMut<'_, T> {
    if chunk_size == 0 {
        crate::panicking::panic_str("chunk size must be non-zero");
    }
    RChunksMut { v, chunk_size }
}
impl<'a, T: fmt::Debug> fmt::Debug for RChunksMut<'a, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let v = SlicePointerDebug {
            addr: self.v.as_ptr(),
            metadata: self.v.len(),
        };
        let marker: crate::marker::PhantomData<&'a mut T> = crate::marker::PhantomData;
        f.debug_struct("RChunksMut")
            .field("v", &v)
            .field("chunk_size", &self.chunk_size)
            .field("_marker", &marker)
            .finish()
    }
}
impl<'a, T> Iterator for RChunksMut<'a, T> {
    type Item = &'a mut [T];
    fn next(&mut self) -> Option<Self::Item> {
        if self.v.is_empty() {
            return None;
        }
        let size = crate::cmp::min(self.v.len(), self.chunk_size);
        let mid = self.v.len() - size;
        let (a, b) = take_mut(&mut self.v).split_at_mut(mid);
        self.v = a;
        Some(b)
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = self.v.len() / self.chunk_size
            + if self.v.len() % self.chunk_size != 0 {
                1
            } else {
                0
            };
        (n, Some(n))
    }
    fn count(self) -> usize {
        self.len()
    }
    fn nth(&mut self, n: usize) -> Option<Self::Item> {
        match n.checked_mul(self.chunk_size) {
            Some(offset) if offset < self.v.len() => {
                let end = self.v.len() - offset;
                let start = end.saturating_sub(self.chunk_size);
                let (v, _) = take_mut(&mut self.v).split_at_mut(end);
                let (rest, chunk) = v.split_at_mut(start);
                self.v = rest;
                Some(chunk)
            }
            _ => {
                self.v = &mut [];
                None
            }
        }
    }
    fn last(mut self) -> Option<Self::Item> {
        self.next_back()
    }
}
impl<'a, T> DoubleEndedIterator for RChunksMut<'a, T> {
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.v.is_empty() {
            return None;
        }
        let remainder = self.v.len() % self.chunk_size;
        let size = if remainder != 0 {
            remainder
        } else {
            self.chunk_size
        };
        let mid = size;
        let (a, b) = take_mut(&mut self.v).split_at_mut(mid);
        self.v = b;
        Some(a)
    }
    fn nth_back(&mut self, n: usize) -> Option<Self::Item> {
        let len = self.len();
        if n >= len {
            self.v = &mut [];
            return None;
        }
        let end = self.v.len() - (len - 1 - n) * self.chunk_size;
        let start = end.saturating_sub(self.chunk_size);
        let (v, rest) = take_mut(&mut self.v).split_at_mut(end);
        let (_, chunk) = v.split_at_mut(start);
        self.v = rest;
        Some(chunk)
    }
}
impl<'a, T> ExactSizeIterator for RChunksMut<'a, T> {}
impl<'a, T> FusedIterator for RChunksMut<'a, T> {}

pub struct RChunksExact<'a, T> {
    v: &'a [T],
    rem: &'a [T],
    chunk_size: usize,
}
#[track_caller]
pub fn rchunks_exact<T>(v: &[T], chunk_size: usize) -> RChunksExact<'_, T> {
    if chunk_size == 0 {
        crate::panicking::panic_str("chunk size must be non-zero");
    }
    let rem_len = v.len() % chunk_size;
    let mid = rem_len;
    let (a, b) = v.split_at(mid);
    RChunksExact {
        v: b,
        rem: a,
        chunk_size,
    }
}
impl<'a, T> RChunksExact<'a, T> {
    pub const fn remainder(&self) -> &'a [T] {
        self.rem
    }
}
impl<'a, T> Clone for RChunksExact<'a, T> {
    fn clone(&self) -> Self {
        RChunksExact {
            v: self.v,
            rem: self.rem,
            chunk_size: self.chunk_size,
        }
    }
}
impl<'a, T: fmt::Debug> fmt::Debug for RChunksExact<'a, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RChunksExact")
            .field("v", &self.v)
            .field("rem", &self.rem)
            .field("chunk_size", &self.chunk_size)
            .finish()
    }
}
impl<'a, T> Iterator for RChunksExact<'a, T> {
    type Item = &'a [T];
    fn next(&mut self) -> Option<Self::Item> {
        if self.v.is_empty() {
            return None;
        }
        let size = crate::cmp::min(self.v.len(), self.chunk_size);
        let mid = self.v.len() - size;
        let (a, b) = self.v.split_at(mid);
        self.v = a;
        Some(b)
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = self.v.len() / self.chunk_size;
        (n, Some(n))
    }
    fn count(self) -> usize {
        self.len()
    }
    fn nth(&mut self, n: usize) -> Option<Self::Item> {
        match n.checked_mul(self.chunk_size) {
            Some(offset) if offset < self.v.len() => {
                let end = self.v.len() - offset;
                let start = end.saturating_sub(self.chunk_size);
                let (v, _) = self.v.split_at(end);
                let (rest, chunk) = v.split_at(start);
                self.v = rest;
                Some(chunk)
            }
            _ => {
                self.v = &self.v[..0];
                None
            }
        }
    }
    fn last(mut self) -> Option<Self::Item> {
        self.next_back()
    }
}
impl<'a, T> DoubleEndedIterator for RChunksExact<'a, T> {
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.v.is_empty() {
            return None;
        }
        let remainder = self.v.len() % self.chunk_size;
        let size = if remainder != 0 {
            remainder
        } else {
            self.chunk_size
        };
        let mid = size;
        let (a, b) = self.v.split_at(mid);
        self.v = b;
        Some(a)
    }
    fn nth_back(&mut self, n: usize) -> Option<Self::Item> {
        let len = self.len();
        if n >= len {
            self.v = &self.v[..0];
            return None;
        }
        let end = self.v.len() - (len - 1 - n) * self.chunk_size;
        let start = end.saturating_sub(self.chunk_size);
        let (v, rest) = self.v.split_at(end);
        let (_, chunk) = v.split_at(start);
        self.v = rest;
        Some(chunk)
    }
}
impl<'a, T> ExactSizeIterator for RChunksExact<'a, T> {}
impl<'a, T> FusedIterator for RChunksExact<'a, T> {}

pub struct RChunksExactMut<'a, T> {
    v: &'a mut [T],
    rem: &'a mut [T],
    chunk_size: usize,
}
#[track_caller]
pub fn rchunks_exact_mut<T>(v: &mut [T], chunk_size: usize) -> RChunksExactMut<'_, T> {
    if chunk_size == 0 {
        crate::panicking::panic_str("chunk size must be non-zero");
    }
    let rem_len = v.len() % chunk_size;
    let mid = rem_len;
    let (a, b) = v.split_at_mut(mid);
    RChunksExactMut {
        v: b,
        rem: a,
        chunk_size,
    }
}
impl<'a, T> RChunksExactMut<'a, T> {
    pub const fn into_remainder(self) -> &'a mut [T] {
        self.rem
    }
}
impl<'a, T: fmt::Debug> fmt::Debug for RChunksExactMut<'a, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let v = SlicePointerDebug {
            addr: self.v.as_ptr(),
            metadata: self.v.len(),
        };
        f.debug_struct("RChunksExactMut")
            .field("v", &v)
            .field("rem", &self.rem)
            .field("chunk_size", &self.chunk_size)
            .finish()
    }
}
impl<'a, T> Iterator for RChunksExactMut<'a, T> {
    type Item = &'a mut [T];
    fn next(&mut self) -> Option<Self::Item> {
        if self.v.is_empty() {
            return None;
        }
        let size = crate::cmp::min(self.v.len(), self.chunk_size);
        let mid = self.v.len() - size;
        let (a, b) = take_mut(&mut self.v).split_at_mut(mid);
        self.v = a;
        Some(b)
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = self.v.len() / self.chunk_size;
        (n, Some(n))
    }
    fn count(self) -> usize {
        self.len()
    }
    fn nth(&mut self, n: usize) -> Option<Self::Item> {
        match n.checked_mul(self.chunk_size) {
            Some(offset) if offset < self.v.len() => {
                let end = self.v.len() - offset;
                let start = end.saturating_sub(self.chunk_size);
                let (v, _) = take_mut(&mut self.v).split_at_mut(end);
                let (rest, chunk) = v.split_at_mut(start);
                self.v = rest;
                Some(chunk)
            }
            _ => {
                self.v = &mut [];
                None
            }
        }
    }
    fn last(mut self) -> Option<Self::Item> {
        self.next_back()
    }
}
impl<'a, T> DoubleEndedIterator for RChunksExactMut<'a, T> {
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.v.is_empty() {
            return None;
        }
        let remainder = self.v.len() % self.chunk_size;
        let size = if remainder != 0 {
            remainder
        } else {
            self.chunk_size
        };
        let mid = size;
        let (a, b) = take_mut(&mut self.v).split_at_mut(mid);
        self.v = b;
        Some(a)
    }
    fn nth_back(&mut self, n: usize) -> Option<Self::Item> {
        let len = self.len();
        if n >= len {
            self.v = &mut [];
            return None;
        }
        let end = self.v.len() - (len - 1 - n) * self.chunk_size;
        let start = end.saturating_sub(self.chunk_size);
        let (v, rest) = take_mut(&mut self.v).split_at_mut(end);
        let (_, chunk) = v.split_at_mut(start);
        self.v = rest;
        Some(chunk)
    }
}
impl<'a, T> ExactSizeIterator for RChunksExactMut<'a, T> {}
impl<'a, T> FusedIterator for RChunksExactMut<'a, T> {}

// ---------------------------------------------------------------- Split family

pub struct Split<'a, T, P> {
    v: &'a [T],
    pred: P,
    finished: bool,
}

pub fn split<T, P: FnMut(&T) -> bool>(v: &[T], pred: P) -> Split<'_, T, P> {
    Split {
        v,
        pred,
        finished: false,
    }
}

impl<'a, T, P: FnMut(&T) -> bool> Split<'a, T, P> {
    fn finish(&mut self) -> Option<&'a [T]> {
        if self.finished {
            None
        } else {
            self.finished = true;
            Some(self.v)
        }
    }
    pub fn as_slice(&self) -> &'a [T] {
        if self.finished {
            &[]
        } else {
            self.v
        }
    }
}

impl<'a, T, P: FnMut(&T) -> bool> Iterator for Split<'a, T, P> {
    type Item = &'a [T];
    fn next(&mut self) -> Option<&'a [T]> {
        if self.finished {
            return None;
        }
        let mut i = 0;
        while i < self.v.len() {
            if (self.pred)(&self.v[i]) {
                let ret = &self.v[..i];
                self.v = &self.v[i + 1..];
                return Some(ret);
            }
            i += 1;
        }
        self.finish()
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        if self.finished {
            (0, Some(0))
        } else {
            (1, Some(self.v.len() + 1))
        }
    }
}

impl<'a, T, P: FnMut(&T) -> bool> DoubleEndedIterator for Split<'a, T, P> {
    fn next_back(&mut self) -> Option<&'a [T]> {
        if self.finished {
            return None;
        }
        let mut i = self.v.len();
        while i > 0 {
            if (self.pred)(&self.v[i - 1]) {
                let ret = &self.v[i..];
                self.v = &self.v[..i - 1];
                return Some(ret);
            }
            i -= 1;
        }
        self.finish()
    }
}

impl<'a, T, P: FnMut(&T) -> bool> FusedIterator for Split<'a, T, P> {}

pub struct SplitInclusive<'a, T, P> {
    v: &'a [T],
    pred: P,
    finished: bool,
}

pub fn split_inclusive<T, P: FnMut(&T) -> bool>(v: &[T], pred: P) -> SplitInclusive<'_, T, P> {
    let finished = v.is_empty();
    SplitInclusive { v, pred, finished }
}

impl<'a, T, P: FnMut(&T) -> bool> Iterator for SplitInclusive<'a, T, P> {
    type Item = &'a [T];
    fn next(&mut self) -> Option<&'a [T]> {
        if self.finished {
            return None;
        }
        let mut idx = self.v.len();
        let mut i = 0;
        while i < self.v.len() {
            if (self.pred)(&self.v[i]) {
                idx = i + 1;
                break;
            }
            i += 1;
        }
        if idx == self.v.len() {
            self.finished = true;
        }
        let ret = &self.v[..idx];
        self.v = &self.v[idx..];
        Some(ret)
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        if self.finished {
            (0, Some(0))
        } else {
            (1, Some(crate::cmp::max(1, self.v.len())))
        }
    }
}

impl<'a, T, P: FnMut(&T) -> bool> DoubleEndedIterator for SplitInclusive<'a, T, P> {
    fn next_back(&mut self) -> Option<&'a [T]> {
        if self.finished {
            return None;
        }
        // the last element is never a split point going backwards
        let remainder = if self.v.is_empty() {
            &[][..]
        } else {
            &self.v[..self.v.len() - 1]
        };
        let mut idx = 0;
        let mut i = remainder.len();
        while i > 0 {
            if (self.pred)(&remainder[i - 1]) {
                idx = i;
                break;
            }
            i -= 1;
        }
        if idx == 0 {
            self.finished = true;
        }
        let ret = &self.v[idx..];
        self.v = &self.v[..idx];
        Some(ret)
    }
}

pub struct SplitMut<'a, T, P> {
    v: &'a mut [T],
    pred: P,
    finished: bool,
}

pub fn split_mut<T, P: FnMut(&T) -> bool>(v: &mut [T], pred: P) -> SplitMut<'_, T, P> {
    SplitMut {
        v,
        pred,
        finished: false,
    }
}

impl<'a, T, P: FnMut(&T) -> bool> Iterator for SplitMut<'a, T, P> {
    type Item = &'a mut [T];
    fn next(&mut self) -> Option<&'a mut [T]> {
        if self.finished {
            return None;
        }
        let mut found = None;
        let mut i = 0;
        while i < self.v.len() {
            if (self.pred)(&self.v[i]) {
                found = Some(i);
                break;
            }
            i += 1;
        }
        let v = take_mut(&mut self.v);
        match found {
            None => {
                self.finished = true;
                Some(v)
            }
            Some(idx) => {
                let (head, tail) = v.split_at_mut(idx);
                self.v = &mut tail[1..];
                Some(head)
            }
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        if self.finished {
            (0, Some(0))
        } else {
            (1, Some(self.v.len() + 1))
        }
    }
}

pub struct SplitInclusiveMut<'a, T, P> {
    v: &'a mut [T],
    pred: P,
    finished: bool,
}

pub fn split_inclusive_mut<T, P: FnMut(&T) -> bool>(
    v: &mut [T],
    pred: P,
) -> SplitInclusiveMut<'_, T, P> {
    let finished = v.is_empty();
    SplitInclusiveMut { v, pred, finished }
}

impl<'a, T, P: FnMut(&T) -> bool> Iterator for SplitInclusiveMut<'a, T, P> {
    type Item = &'a mut [T];
    fn next(&mut self) -> Option<&'a mut [T]> {
        if self.finished {
            return None;
        }
        let mut idx = self.v.len();
        let mut i = 0;
        while i < self.v.len() {
            if (self.pred)(&self.v[i]) {
                idx = i + 1;
                break;
            }
            i += 1;
        }
        if idx == self.v.len() {
            self.finished = true;
        }
        let (ret, rest) = take_mut(&mut self.v).split_at_mut(idx);
        self.v = rest;
        Some(ret)
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        if self.finished {
            (0, Some(0))
        } else {
            (1, Some(crate::cmp::max(1, self.v.len())))
        }
    }
}

impl<'a, T, P: FnMut(&T) -> bool> DoubleEndedIterator for SplitInclusiveMut<'a, T, P> {
    fn next_back(&mut self) -> Option<&'a mut [T]> {
        if self.finished {
            return None;
        }
        // the last element is never a split point going backwards
        let remainder = if self.v.is_empty() {
            &[][..]
        } else {
            &self.v[..self.v.len() - 1]
        };
        let mut idx = 0;
        let mut i = remainder.len();
        while i > 0 {
            if (self.pred)(&remainder[i - 1]) {
                idx = i;
                break;
            }
            i -= 1;
        }
        if idx == 0 {
            self.finished = true;
        }
        let (rest, ret) = take_mut(&mut self.v).split_at_mut(idx);
        self.v = rest;
        Some(ret)
    }
}

impl<'a, T, P: FnMut(&T) -> bool> SplitMut<'a, T, P> {
    fn finish(&mut self) -> Option<&'a mut [T]> {
        if self.finished {
            None
        } else {
            self.finished = true;
            Some(take_mut(&mut self.v))
        }
    }
}
impl<'a, T, P: FnMut(&T) -> bool> DoubleEndedIterator for SplitMut<'a, T, P> {
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.finished {
            return None;
        }
        let mut i = self.v.len();
        while i > 0 {
            i -= 1;
            if (self.pred)(&self.v[i]) {
                let (head, tail) = take_mut(&mut self.v).split_at_mut(i);
                self.v = head;
                return Some(&mut tail[1..]);
            }
        }
        self.finish()
    }
}
impl<'a, T: fmt::Debug, P: FnMut(&T) -> bool> fmt::Debug for Split<'a, T, P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Split")
            .field("v", &self.v)
            .field("finished", &self.finished)
            .finish()
    }
}
impl<'a, T, P: Clone + FnMut(&T) -> bool> Clone for Split<'a, T, P> {
    fn clone(&self) -> Self {
        Split {
            v: self.v,
            pred: self.pred.clone(),
            finished: self.finished,
        }
    }
}
impl<'a, T: fmt::Debug, P: FnMut(&T) -> bool> fmt::Debug for SplitMut<'a, T, P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SplitMut")
            .field("v", &self.v)
            .field("finished", &self.finished)
            .finish()
    }
}
impl<'a, T, P: FnMut(&T) -> bool> FusedIterator for SplitMut<'a, T, P> {}
impl<'a, T: fmt::Debug, P: FnMut(&T) -> bool> fmt::Debug for SplitInclusive<'a, T, P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SplitInclusive")
            .field("v", &self.v)
            .field("finished", &self.finished)
            .finish()
    }
}
impl<'a, T, P: FnMut(&T) -> bool> FusedIterator for SplitInclusive<'a, T, P> {}
impl<'a, T, P: Clone + FnMut(&T) -> bool> Clone for SplitInclusive<'a, T, P> {
    fn clone(&self) -> Self {
        SplitInclusive {
            v: self.v,
            pred: self.pred.clone(),
            finished: self.finished,
        }
    }
}
impl<'a, T: fmt::Debug, P: FnMut(&T) -> bool> fmt::Debug for SplitInclusiveMut<'a, T, P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SplitInclusiveMut")
            .field("v", &self.v)
            .field("finished", &self.finished)
            .finish()
    }
}
impl<'a, T, P: FnMut(&T) -> bool> FusedIterator for SplitInclusiveMut<'a, T, P> {}
pub struct RSplit<'a, T, P> {
    inner: Split<'a, T, P>,
}
pub fn rsplit<T, P: FnMut(&T) -> bool>(v: &[T], pred: P) -> RSplit<'_, T, P> {
    RSplit {
        inner: split(v, pred),
    }
}
impl<'a, T, P: FnMut(&T) -> bool> RSplit<'a, T, P> {
    fn finish(&mut self) -> Option<&'a [T]> {
        self.inner.finish()
    }
}
impl<'a, T, P: FnMut(&T) -> bool> Iterator for RSplit<'a, T, P> {
    type Item = &'a [T];
    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next_back()
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}
impl<'a, T, P: FnMut(&T) -> bool> DoubleEndedIterator for RSplit<'a, T, P> {
    fn next_back(&mut self) -> Option<Self::Item> {
        self.inner.next()
    }
}
impl<'a, T, P: FnMut(&T) -> bool> FusedIterator for RSplit<'a, T, P> {}
impl<'a, T: fmt::Debug, P: FnMut(&T) -> bool> fmt::Debug for RSplit<'a, T, P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RSplit")
            .field("v", &self.inner.v)
            .field("finished", &self.inner.finished)
            .finish()
    }
}
impl<'a, T, P: Clone + FnMut(&T) -> bool> Clone for RSplit<'a, T, P> {
    fn clone(&self) -> Self {
        RSplit {
            inner: self.inner.clone(),
        }
    }
}
pub struct RSplitMut<'a, T, P> {
    inner: SplitMut<'a, T, P>,
}
pub fn rsplit_mut<T, P: FnMut(&T) -> bool>(v: &mut [T], pred: P) -> RSplitMut<'_, T, P> {
    RSplitMut {
        inner: split_mut(v, pred),
    }
}
impl<'a, T, P: FnMut(&T) -> bool> RSplitMut<'a, T, P> {
    fn finish(&mut self) -> Option<&'a mut [T]> {
        self.inner.finish()
    }
}
impl<'a, T, P: FnMut(&T) -> bool> Iterator for RSplitMut<'a, T, P> {
    type Item = &'a mut [T];
    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next_back()
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}
impl<'a, T, P: FnMut(&T) -> bool> DoubleEndedIterator for RSplitMut<'a, T, P> {
    fn next_back(&mut self) -> Option<Self::Item> {
        self.inner.next()
    }
}
impl<'a, T, P: FnMut(&T) -> bool> FusedIterator for RSplitMut<'a, T, P> {}
impl<'a, T: fmt::Debug, P: FnMut(&T) -> bool> fmt::Debug for RSplitMut<'a, T, P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RSplitMut")
            .field("v", &self.inner.v)
            .field("finished", &self.inner.finished)
            .finish()
    }
}
// The wrapper also preserves core's nested Debug representation.
struct GenericSplitN<I> {
    iter: I,
    count: usize,
}
impl<I: fmt::Debug> fmt::Debug for GenericSplitN<I> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GenericSplitN")
            .field("iter", &self.iter)
            .field("count", &self.count)
            .finish()
    }
}
pub struct SplitN<'a, T, P> {
    inner: GenericSplitN<Split<'a, T, P>>,
}
pub fn splitn<T, P: FnMut(&T) -> bool>(v: &[T], n: usize, pred: P) -> SplitN<'_, T, P> {
    SplitN {
        inner: GenericSplitN {
            iter: split(v, pred),
            count: n,
        },
    }
}
impl<'a, T, P: FnMut(&T) -> bool> Iterator for SplitN<'a, T, P> {
    type Item = &'a [T];
    fn next(&mut self) -> Option<Self::Item> {
        match self.inner.count {
            0 => None,
            1 => {
                self.inner.count -= 1;
                self.inner.iter.finish()
            }
            _ => {
                self.inner.count -= 1;
                self.inner.iter.next()
            }
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let (lower, upper) = self.inner.iter.size_hint();
        let count = self.inner.count;
        let upper = match upper {
            Some(n) => crate::cmp::min(count, n),
            None => count,
        };
        (crate::cmp::min(count, lower), Some(upper))
    }
}
impl<'a, T, P: FnMut(&T) -> bool> FusedIterator for SplitN<'a, T, P> {}
impl<'a, T: fmt::Debug, P: FnMut(&T) -> bool> fmt::Debug for SplitN<'a, T, P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SplitN")
            .field("inner", &self.inner)
            .finish()
    }
}
pub struct SplitNMut<'a, T, P> {
    inner: GenericSplitN<SplitMut<'a, T, P>>,
}
pub fn splitn_mut<T, P: FnMut(&T) -> bool>(v: &mut [T], n: usize, pred: P) -> SplitNMut<'_, T, P> {
    SplitNMut {
        inner: GenericSplitN {
            iter: split_mut(v, pred),
            count: n,
        },
    }
}
impl<'a, T, P: FnMut(&T) -> bool> Iterator for SplitNMut<'a, T, P> {
    type Item = &'a mut [T];
    fn next(&mut self) -> Option<Self::Item> {
        match self.inner.count {
            0 => None,
            1 => {
                self.inner.count -= 1;
                self.inner.iter.finish()
            }
            _ => {
                self.inner.count -= 1;
                self.inner.iter.next()
            }
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let (lower, upper) = self.inner.iter.size_hint();
        let count = self.inner.count;
        let upper = match upper {
            Some(n) => crate::cmp::min(count, n),
            None => count,
        };
        (crate::cmp::min(count, lower), Some(upper))
    }
}
impl<'a, T, P: FnMut(&T) -> bool> FusedIterator for SplitNMut<'a, T, P> {}
impl<'a, T: fmt::Debug, P: FnMut(&T) -> bool> fmt::Debug for SplitNMut<'a, T, P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SplitNMut")
            .field("inner", &self.inner)
            .finish()
    }
}
pub struct RSplitN<'a, T, P> {
    inner: GenericSplitN<RSplit<'a, T, P>>,
}
pub fn rsplitn<T, P: FnMut(&T) -> bool>(v: &[T], n: usize, pred: P) -> RSplitN<'_, T, P> {
    RSplitN {
        inner: GenericSplitN {
            iter: rsplit(v, pred),
            count: n,
        },
    }
}
impl<'a, T, P: FnMut(&T) -> bool> Iterator for RSplitN<'a, T, P> {
    type Item = &'a [T];
    fn next(&mut self) -> Option<Self::Item> {
        match self.inner.count {
            0 => None,
            1 => {
                self.inner.count -= 1;
                self.inner.iter.finish()
            }
            _ => {
                self.inner.count -= 1;
                self.inner.iter.next()
            }
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let (lower, upper) = self.inner.iter.size_hint();
        let count = self.inner.count;
        let upper = match upper {
            Some(n) => crate::cmp::min(count, n),
            None => count,
        };
        (crate::cmp::min(count, lower), Some(upper))
    }
}
impl<'a, T, P: FnMut(&T) -> bool> FusedIterator for RSplitN<'a, T, P> {}
impl<'a, T: fmt::Debug, P: FnMut(&T) -> bool> fmt::Debug for RSplitN<'a, T, P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RSplitN")
            .field("inner", &self.inner)
            .finish()
    }
}
pub struct RSplitNMut<'a, T, P> {
    inner: GenericSplitN<RSplitMut<'a, T, P>>,
}
pub fn rsplitn_mut<T, P: FnMut(&T) -> bool>(
    v: &mut [T],
    n: usize,
    pred: P,
) -> RSplitNMut<'_, T, P> {
    RSplitNMut {
        inner: GenericSplitN {
            iter: rsplit_mut(v, pred),
            count: n,
        },
    }
}
impl<'a, T, P: FnMut(&T) -> bool> Iterator for RSplitNMut<'a, T, P> {
    type Item = &'a mut [T];
    fn next(&mut self) -> Option<Self::Item> {
        match self.inner.count {
            0 => None,
            1 => {
                self.inner.count -= 1;
                self.inner.iter.finish()
            }
            _ => {
                self.inner.count -= 1;
                self.inner.iter.next()
            }
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let (lower, upper) = self.inner.iter.size_hint();
        let count = self.inner.count;
        let upper = match upper {
            Some(n) => crate::cmp::min(count, n),
            None => count,
        };
        (crate::cmp::min(count, lower), Some(upper))
    }
}
impl<'a, T, P: FnMut(&T) -> bool> FusedIterator for RSplitNMut<'a, T, P> {}
impl<'a, T: fmt::Debug, P: FnMut(&T) -> bool> fmt::Debug for RSplitNMut<'a, T, P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RSplitNMut")
            .field("inner", &self.inner)
            .finish()
    }
}
pub struct ChunkBy<'a, T, P> {
    slice: &'a [T],
    predicate: P,
}
pub fn chunk_by<T, P: FnMut(&T, &T) -> bool>(slice: &[T], predicate: P) -> ChunkBy<'_, T, P> {
    ChunkBy { slice, predicate }
}
impl<'a, T, P: FnMut(&T, &T) -> bool> Iterator for ChunkBy<'a, T, P> {
    type Item = &'a [T];
    fn next(&mut self) -> Option<Self::Item> {
        if self.slice.is_empty() {
            return None;
        }
        let mut len = 1;
        while len < self.slice.len() {
            if !(self.predicate)(&self.slice[len - 1], &self.slice[len]) {
                break;
            }
            len += 1;
        }
        let (head, tail) = self.slice.split_at(len);
        self.slice = tail;
        Some(head)
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        if self.slice.is_empty() {
            (0, Some(0))
        } else {
            (1, Some(self.slice.len()))
        }
    }
    fn last(mut self) -> Option<Self::Item> {
        self.next_back()
    }
}
impl<'a, T, P: FnMut(&T, &T) -> bool> DoubleEndedIterator for ChunkBy<'a, T, P> {
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.slice.is_empty() {
            return None;
        }
        let mut start = self.slice.len() - 1;
        while start > 0 {
            if !(self.predicate)(&self.slice[start - 1], &self.slice[start]) {
                break;
            }
            start -= 1;
        }
        let (head, tail) = self.slice.split_at(start);
        self.slice = head;
        Some(tail)
    }
}
impl<'a, T, P: FnMut(&T, &T) -> bool> FusedIterator for ChunkBy<'a, T, P> {}
impl<'a, T: fmt::Debug, P> fmt::Debug for ChunkBy<'a, T, P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChunkBy")
            .field("slice", &self.slice)
            .finish()
    }
}
impl<'a, T, P: Clone> Clone for ChunkBy<'a, T, P> {
    fn clone(&self) -> Self {
        ChunkBy {
            slice: self.slice,
            predicate: self.predicate.clone(),
        }
    }
}
pub struct ChunkByMut<'a, T, P> {
    slice: &'a mut [T],
    predicate: P,
}
pub fn chunk_by_mut<T, P: FnMut(&T, &T) -> bool>(
    slice: &mut [T],
    predicate: P,
) -> ChunkByMut<'_, T, P> {
    ChunkByMut { slice, predicate }
}
impl<'a, T, P: FnMut(&T, &T) -> bool> Iterator for ChunkByMut<'a, T, P> {
    type Item = &'a mut [T];
    fn next(&mut self) -> Option<Self::Item> {
        if self.slice.is_empty() {
            return None;
        }
        let mut len = 1;
        while len < self.slice.len() {
            if !(self.predicate)(&self.slice[len - 1], &self.slice[len]) {
                break;
            }
            len += 1;
        }
        let (head, tail) = take_mut(&mut self.slice).split_at_mut(len);
        self.slice = tail;
        Some(head)
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        if self.slice.is_empty() {
            (0, Some(0))
        } else {
            (1, Some(self.slice.len()))
        }
    }
    fn last(mut self) -> Option<Self::Item> {
        self.next_back()
    }
}
impl<'a, T, P: FnMut(&T, &T) -> bool> DoubleEndedIterator for ChunkByMut<'a, T, P> {
    fn next_back(&mut self) -> Option<Self::Item> {
        if self.slice.is_empty() {
            return None;
        }
        let mut start = self.slice.len() - 1;
        while start > 0 {
            if !(self.predicate)(&self.slice[start - 1], &self.slice[start]) {
                break;
            }
            start -= 1;
        }
        let (head, tail) = take_mut(&mut self.slice).split_at_mut(start);
        self.slice = head;
        Some(tail)
    }
}
impl<'a, T, P: FnMut(&T, &T) -> bool> FusedIterator for ChunkByMut<'a, T, P> {}
impl<'a, T: fmt::Debug, P> fmt::Debug for ChunkByMut<'a, T, P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ChunkByMut")
            .field("slice", &self.slice)
            .finish()
    }
}

#[cfg(not(relax_check))]
impl<'a, T> crate::iter::IntoIterator for &'a [T] {
    type Item = &'a T;
    type IntoIter = Iter<'a, T>;
    fn into_iter(self) -> Iter<'a, T> {
        Iter::new(self)
    }
}

#[cfg(not(relax_check))]
impl<'a, T> crate::iter::IntoIterator for &'a mut [T] {
    type Item = &'a mut T;
    type IntoIter = IterMut<'a, T>;
    fn into_iter(self) -> IterMut<'a, T> {
        IterMut::new(self)
    }
}

// ---------------------------------------------------------------- Array chunk views
#[track_caller]
pub fn as_chunks<T, const N: usize>(s: &[T]) -> (&[[T; N]], &[T]) {
    if N == 0 {
        crate::panicking::panic_str("chunk size must be non-zero");
    }
    let n = s.len() / N;
    let mid = n * N;
    let (a, b) = s.split_at(mid);
    // SAFETY: the array region has n * N elements with the same alignment as T.
    (
        unsafe { crate::intrinsics_mem::slice_from_raw(a.as_ptr() as *const [T; N], n) },
        b,
    )
}
#[track_caller]
pub fn as_chunks_mut<T, const N: usize>(s: &mut [T]) -> (&mut [[T; N]], &mut [T]) {
    if N == 0 {
        crate::panicking::panic_str("chunk size must be non-zero");
    }
    let n = s.len() / N;
    let mid = n * N;
    let (a, b) = s.split_at_mut(mid);
    // SAFETY: splitting preserves disjointness; the cast preserves alignment and extent.
    (
        unsafe { crate::intrinsics_mem::slice_from_raw_mut(a.as_mut_ptr() as *mut [T; N], n) },
        b,
    )
}
#[track_caller]
pub fn as_rchunks<T, const N: usize>(s: &[T]) -> (&[T], &[[T; N]]) {
    if N == 0 {
        crate::panicking::panic_str("chunk size must be non-zero");
    }
    let n = s.len() / N;
    let mid = s.len() - n * N;
    let (a, b) = s.split_at(mid);
    // SAFETY: the array region has n * N elements with the same alignment as T.
    (a, unsafe {
        crate::intrinsics_mem::slice_from_raw(b.as_ptr() as *const [T; N], n)
    })
}
#[track_caller]
pub fn as_rchunks_mut<T, const N: usize>(s: &mut [T]) -> (&mut [T], &mut [[T; N]]) {
    if N == 0 {
        crate::panicking::panic_str("chunk size must be non-zero");
    }
    let n = s.len() / N;
    let mid = s.len() - n * N;
    let (a, b) = s.split_at_mut(mid);
    // SAFETY: splitting preserves disjointness; the cast preserves alignment and extent.
    (a, unsafe {
        crate::intrinsics_mem::slice_from_raw_mut(b.as_mut_ptr() as *mut [T; N], n)
    })
}
#[track_caller]
pub fn split_first_chunk_mut<T, const N: usize>(s: &mut [T]) -> Option<(&mut [T; N], &mut [T])> {
    if s.len() < N {
        return None;
    }
    let mid = N;
    let (a, b) = s.split_at_mut(mid);
    // SAFETY: splitting preserves disjointness; the cast preserves alignment and extent.
    Some((unsafe { &mut *(a.as_mut_ptr() as *mut [T; N]) }, b))
}
#[track_caller]
pub fn split_last_chunk_mut<T, const N: usize>(s: &mut [T]) -> Option<(&mut [T], &mut [T; N])> {
    if s.len() < N {
        return None;
    }
    let mid = s.len() - N;
    let (a, b) = s.split_at_mut(mid);
    // SAFETY: splitting preserves disjointness; the cast preserves alignment and extent.
    Some((a, unsafe { &mut *(b.as_mut_ptr() as *mut [T; N]) }))
}
