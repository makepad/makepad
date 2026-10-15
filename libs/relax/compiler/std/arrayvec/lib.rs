//! Relax's built-in `arrayvec` crate: under Relax the crate path `arrayvec` resolves here
//! (crate substitution, coord.md "SV1"). Behaviour follows arrayvec 0.7.6 (fixed capacity
//! CAP, u32 length, panic messages, CapacityError); ported from it (Copyright (c) Ulrik Sverdrup
//! "bluss" 2015-2023, MIT/Apache-2.0; see libs/relax/compiler/std/CREDITS.md). ArrayString is not
//! provided (no supported client uses it). Needs const generics in Relax (coord.md F-STD2);
//! the rustc path keeps the real crate and std/check_ext tests this file against it.

use core::borrow::{Borrow, BorrowMut};
use core::cmp;
use core::fmt;
use core::hash::{Hash, Hasher};
use core::iter;
use core::mem::{self, ManuallyDrop, MaybeUninit};
use core::ops::{Bound, Deref, DerefMut, RangeBounds};
use core::ptr;
use core::slice;

type LenUint = u32;

/// Error value indicating insufficient capacity
#[derive(Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
pub struct CapacityError<T = ()> {
    element: T,
}

impl<T> CapacityError<T> {
    /// Create a new `CapacityError` from `element`.
    pub const fn new(element: T) -> CapacityError<T> {
        CapacityError { element }
    }
    /// Extract the overflowing element
    pub fn element(self) -> T {
        self.element
    }
    /// Convert into a `CapacityError` that does not carry an element.
    pub fn simplify(self) -> CapacityError {
        CapacityError { element: () }
    }
}

const CAPERROR: &str = "insufficient capacity";

impl<T: 'static> core::error::Error for CapacityError<T> {}

impl<T> fmt::Display for CapacityError<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", CAPERROR)
    }
}

impl<T> fmt::Debug for CapacityError<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", "CapacityError", CAPERROR)
    }
}

/// A vector with a fixed capacity.
#[repr(C)]
pub struct ArrayVec<T, const CAP: usize> {
    len: LenUint,
    xs: [MaybeUninit<T>; CAP],
}

impl<T, const CAP: usize> Drop for ArrayVec<T, CAP> {
    fn drop(&mut self) {
        self.clear();
    }
}

#[cold]
#[track_caller]
fn panic_oob(method_name: &str, index: usize, len: usize) -> ! {
    panic!("ArrayVec::{}: index {} is out of bounds in vector of length {}", method_name, index, len)
}

#[cold]
#[track_caller]
fn extend_panic() -> ! {
    panic!("ArrayVec: capacity exceeded in extend/from_iter");
}

impl<T, const CAP: usize> ArrayVec<T, CAP> {
    const CAPACITY: usize = CAP;

    /// Create a new empty `ArrayVec`.
    #[track_caller]
    pub fn new() -> ArrayVec<T, CAP> {
        if mem::size_of::<usize>() > mem::size_of::<LenUint>() && CAP > LenUint::MAX as usize {
            panic!("ArrayVec: largest supported capacity is u32::MAX")
        }
        unsafe { ArrayVec { xs: MaybeUninit::uninit().assume_init(), len: 0 } }
    }

    /// Return the number of elements in the `ArrayVec`.
    pub const fn len(&self) -> usize {
        self.len as usize
    }

    /// Returns whether the `ArrayVec` is empty.
    pub const fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Return the capacity of the `ArrayVec`.
    pub const fn capacity(&self) -> usize {
        CAP
    }

    /// Return true if the `ArrayVec` is completely filled to its capacity, false otherwise.
    pub const fn is_full(&self) -> bool {
        self.len() == self.capacity()
    }

    /// Returns the capacity left in the `ArrayVec`.
    pub const fn remaining_capacity(&self) -> usize {
        self.capacity() - self.len()
    }

    /// Push `element` to the end of the vector; panics if the vector is full.
    #[track_caller]
    pub fn push(&mut self, element: T) {
        self.try_push(element).unwrap()
    }

    /// Push `element` to the end of the vector, or return it in an error if full.
    pub fn try_push(&mut self, element: T) -> Result<(), CapacityError<T>> {
        if self.len() < Self::CAPACITY {
            unsafe {
                self.push_unchecked(element);
            }
            Ok(())
        } else {
            Err(CapacityError::new(element))
        }
    }

    /// Push `element` to the end of the vector without checking the capacity.
    pub unsafe fn push_unchecked(&mut self, element: T) {
        let len = self.len();
        ptr::write(self.as_mut_ptr().add(len), element);
        self.set_len(len + 1);
    }

    /// Shortens the vector, keeping the first `len` elements and dropping the rest.
    pub fn truncate(&mut self, new_len: usize) {
        unsafe {
            let len = self.len();
            if new_len < len {
                self.set_len(new_len);
                let tail = slice::from_raw_parts_mut(self.as_mut_ptr().add(new_len), len - new_len);
                ptr::drop_in_place(tail as *mut [T]);
            }
        }
    }

    /// Remove all elements in the vector.
    pub fn clear(&mut self) {
        self.truncate(0)
    }

    unsafe fn get_unchecked_ptr(&mut self, index: usize) -> *mut T {
        self.as_mut_ptr().add(index)
    }

    /// Insert `element` at position `index`; panics if full or out of bounds.
    #[track_caller]
    pub fn insert(&mut self, index: usize, element: T) {
        self.try_insert(index, element).unwrap()
    }

    /// Insert `element` at position `index`, or return it in an error if full.
    #[track_caller]
    pub fn try_insert(&mut self, index: usize, element: T) -> Result<(), CapacityError<T>> {
        if index > self.len() {
            panic_oob("try_insert", index, self.len())
        }
        if self.len() == self.capacity() {
            return Err(CapacityError::new(element));
        }
        let len = self.len();
        unsafe {
            let p: *mut T = self.get_unchecked_ptr(index);
            ptr::copy(p, p.add(1), len - index);
            ptr::write(p, element);
            self.set_len(len + 1);
        }
        Ok(())
    }

    /// Remove the last element in the vector and return it.
    pub fn pop(&mut self) -> Option<T> {
        if self.len() == 0 {
            return None;
        }
        unsafe {
            let new_len = self.len() - 1;
            self.set_len(new_len);
            Some(ptr::read(self.as_ptr().add(new_len)))
        }
    }

    /// Remove the element at `index` and swap the last element into its place.
    #[track_caller]
    pub fn swap_remove(&mut self, index: usize) -> T {
        match self.swap_pop(index) {
            Some(v) => v,
            None => panic_oob("swap_remove", index, self.len()),
        }
    }

    /// Like swap_remove, returning None when out of bounds.
    pub fn swap_pop(&mut self, index: usize) -> Option<T> {
        let len = self.len();
        if index >= len {
            return None;
        }
        self.as_mut_slice().swap(index, len - 1);
        self.pop()
    }

    /// Remove the element at `index` and shift down the following elements.
    #[track_caller]
    pub fn remove(&mut self, index: usize) -> T {
        match self.pop_at(index) {
            Some(v) => v,
            None => panic_oob("remove", index, self.len()),
        }
    }

    /// Like remove, returning None when out of bounds.
    pub fn pop_at(&mut self, index: usize) -> Option<T> {
        if index >= self.len() {
            None
        } else {
            self.drain(index..index + 1).next()
        }
    }

    /// Retains only the elements specified by the predicate (order preserved).
    pub fn retain<F: FnMut(&mut T) -> bool>(&mut self, f: F) {
        let mut f = f;
        let original_len = self.len();
        unsafe { self.set_len(0) };
        let mut processed = 0;
        let mut deleted = 0;
        while processed != original_len {
            unsafe {
                let cur = self.as_mut_ptr().add(processed);
                if !f(&mut *cur) {
                    processed += 1;
                    deleted += 1;
                    ptr::drop_in_place(cur);
                } else {
                    if deleted > 0 {
                        ptr::copy_nonoverlapping(cur as *const T, cur.sub(deleted), 1);
                    }
                    processed += 1;
                }
            }
        }
        unsafe { self.set_len(original_len - deleted) };
    }

    /// Set the vector's length without dropping or moving out elements.
    pub unsafe fn set_len(&mut self, length: usize) {
        self.len = length as LenUint;
    }

    /// Copy all elements from the slice and append to the `ArrayVec`.
    pub fn try_extend_from_slice(&mut self, other: &[T]) -> Result<(), CapacityError>
    where
        T: Copy,
    {
        if self.remaining_capacity() < other.len() {
            return Err(CapacityError::new(()));
        }
        let self_len = self.len();
        let other_len = other.len();
        unsafe {
            let dst = self.get_unchecked_ptr(self_len);
            ptr::copy_nonoverlapping(other.as_ptr(), dst, other_len);
            self.set_len(self_len + other_len);
        }
        Ok(())
    }

    /// Create a draining iterator that removes the specified range.
    #[track_caller]
    pub fn drain<R: RangeBounds<usize>>(&mut self, range: R) -> Drain<'_, T, CAP> {
        let len = self.len();
        let start = match range.start_bound() {
            Bound::Unbounded => 0,
            Bound::Included(&i) => i,
            Bound::Excluded(&i) => i.saturating_add(1),
        };
        let end = match range.end_bound() {
            Bound::Excluded(&j) => j,
            Bound::Included(&j) => j.saturating_add(1),
            Bound::Unbounded => len,
        };
        self.drain_range(start, end)
    }

    #[track_caller]
    fn drain_range(&mut self, start: usize, end: usize) -> Drain<'_, T, CAP> {
        let len = self.len();
        let range_slice: *const [T] = &self.as_slice()[start..end];
        self.len = start as LenUint;
        unsafe { Drain { tail_start: end, tail_len: len - end, iter: (*range_slice).iter(), vec: self as *mut ArrayVec<T, CAP> } }
    }

    /// Return the inner fixed size array, if it is full to its capacity.
    pub fn into_inner(self) -> Result<[T; CAP], ArrayVec<T, CAP>> {
        if self.len() < self.capacity() {
            Err(self)
        } else {
            unsafe { Ok(self.into_inner_unchecked()) }
        }
    }

    /// Return the inner fixed size array without checking that it is full.
    pub unsafe fn into_inner_unchecked(self) -> [T; CAP] {
        let self_ = ManuallyDrop::new(self);
        ptr::read(self_.as_ptr() as *const [T; CAP])
    }

    /// Returns the ArrayVec, replacing the original with a new empty ArrayVec.
    pub fn take(&mut self) -> ArrayVec<T, CAP> {
        mem::replace(self, ArrayVec::new())
    }

    /// Return a slice containing all elements of the vector.
    pub fn as_slice(&self) -> &[T] {
        let len = self.len();
        unsafe { slice::from_raw_parts(self.as_ptr(), len) }
    }

    /// Return a mutable slice containing all elements of the vector.
    pub fn as_mut_slice(&mut self) -> &mut [T] {
        let len = self.len();
        unsafe { slice::from_raw_parts_mut(self.as_mut_ptr(), len) }
    }

    /// Return a raw pointer to the vector's buffer.
    pub fn as_ptr(&self) -> *const T {
        self.xs.as_ptr() as *const T
    }

    /// Return a raw mutable pointer to the vector's buffer.
    pub fn as_mut_ptr(&mut self) -> *mut T {
        self.xs.as_mut_ptr() as *mut T
    }

    #[track_caller]
    fn extend_from_iter<I: IntoIterator<Item = T>>(&mut self, iterable: I, check: bool) {
        let take = self.capacity() - self.len();
        let mut len = self.len();
        let end = len + take;
        let mut iter = iterable.into_iter();
        loop {
            match iter.next() {
                Some(elt) => {
                    if len == end && check {
                        unsafe { self.set_len(len) };
                        extend_panic();
                    }
                    if mem::size_of::<T>() != 0 {
                        unsafe { ptr::write(self.as_mut_ptr().add(len), elt) };
                    } else {
                        mem::forget(elt);
                    }
                    len += 1;
                }
                None => {
                    unsafe { self.set_len(len) };
                    return;
                }
            }
        }
    }

    fn extend_from_slice(&mut self, slice: &[T])
    where
        T: Clone,
    {
        let take = self.capacity() - self.len();
        let slice = if take < slice.len() { &slice[..take] } else { slice };
        self.extend_from_iter(slice.iter().cloned(), false);
    }
}

impl<T, const CAP: usize> Deref for ArrayVec<T, CAP> {
    type Target = [T];
    fn deref(&self) -> &[T] {
        self.as_slice()
    }
}

impl<T, const CAP: usize> DerefMut for ArrayVec<T, CAP> {
    fn deref_mut(&mut self) -> &mut [T] {
        self.as_mut_slice()
    }
}

impl<T, const CAP: usize> From<[T; CAP]> for ArrayVec<T, CAP> {
    #[track_caller]
    fn from(array: [T; CAP]) -> ArrayVec<T, CAP> {
        let array = ManuallyDrop::new(array);
        let mut vec = ArrayVec::<T, CAP>::new();
        unsafe {
            ptr::copy_nonoverlapping(&*array as *const [T; CAP] as *const T, vec.as_mut_ptr(), CAP);
            vec.set_len(CAP);
        }
        vec
    }
}

impl<'a, T: Clone, const CAP: usize> TryFrom<&'a [T]> for ArrayVec<T, CAP> {
    type Error = CapacityError;
    fn try_from(slice: &'a [T]) -> Result<ArrayVec<T, CAP>, CapacityError> {
        if Self::CAPACITY < slice.len() {
            Err(CapacityError::new(()))
        } else {
            let mut array = ArrayVec::new();
            array.extend_from_slice(slice);
            Ok(array)
        }
    }
}

impl<'a, T: 'a, const CAP: usize> IntoIterator for &'a ArrayVec<T, CAP> {
    type Item = &'a T;
    type IntoIter = slice::Iter<'a, T>;
    fn into_iter(self) -> slice::Iter<'a, T> {
        self.as_slice().iter()
    }
}

impl<'a, T: 'a, const CAP: usize> IntoIterator for &'a mut ArrayVec<T, CAP> {
    type Item = &'a mut T;
    type IntoIter = slice::IterMut<'a, T>;
    fn into_iter(self) -> slice::IterMut<'a, T> {
        self.as_mut_slice().iter_mut()
    }
}

impl<T, const CAP: usize> IntoIterator for ArrayVec<T, CAP> {
    type Item = T;
    type IntoIter = IntoIter<T, CAP>;
    fn into_iter(self) -> IntoIter<T, CAP> {
        IntoIter { index: 0, v: self }
    }
}

/// By-value iterator for `ArrayVec`.
pub struct IntoIter<T, const CAP: usize> {
    index: usize,
    v: ArrayVec<T, CAP>,
}

impl<T, const CAP: usize> IntoIter<T, CAP> {
    /// Returns the remaining items of this iterator as a slice.
    pub fn as_slice(&self) -> &[T] {
        &self.v.as_slice()[self.index..]
    }
    /// Returns the remaining items of this iterator as a mutable slice.
    pub fn as_mut_slice(&mut self) -> &mut [T] {
        let i = self.index;
        &mut self.v.as_mut_slice()[i..]
    }
}

impl<T, const CAP: usize> Iterator for IntoIter<T, CAP> {
    type Item = T;
    fn next(&mut self) -> Option<T> {
        if self.index == self.v.len() {
            None
        } else {
            unsafe {
                let index = self.index;
                self.index = index + 1;
                Some(ptr::read(self.v.get_unchecked_ptr(index)))
            }
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let len = self.v.len() - self.index;
        (len, Some(len))
    }
}

impl<T, const CAP: usize> DoubleEndedIterator for IntoIter<T, CAP> {
    fn next_back(&mut self) -> Option<T> {
        if self.index == self.v.len() {
            None
        } else {
            unsafe {
                let new_len = self.v.len() - 1;
                self.v.set_len(new_len);
                Some(ptr::read(self.v.get_unchecked_ptr(new_len)))
            }
        }
    }
}

impl<T, const CAP: usize> ExactSizeIterator for IntoIter<T, CAP> {}

impl<T, const CAP: usize> Drop for IntoIter<T, CAP> {
    fn drop(&mut self) {
        let index = self.index;
        let len = self.v.len();
        unsafe {
            self.v.set_len(0);
            let elements = slice::from_raw_parts_mut(self.v.get_unchecked_ptr(index), len - index);
            ptr::drop_in_place(elements as *mut [T]);
        }
    }
}

impl<T: Clone, const CAP: usize> Clone for IntoIter<T, CAP> {
    fn clone(&self) -> IntoIter<T, CAP> {
        let mut v = ArrayVec::new();
        v.extend_from_slice(&self.v.as_slice()[self.index..]);
        v.into_iter()
    }
}

impl<T: fmt::Debug, const CAP: usize> fmt::Debug for IntoIter<T, CAP> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(&self.v.as_slice()[self.index..]).finish()
    }
}

/// A draining iterator for `ArrayVec`.
pub struct Drain<'a, T: 'a, const CAP: usize> {
    tail_start: usize,
    tail_len: usize,
    iter: slice::Iter<'a, T>,
    vec: *mut ArrayVec<T, CAP>,
}

unsafe impl<'a, T: Sync, const CAP: usize> Sync for Drain<'a, T, CAP> {}
unsafe impl<'a, T: Send, const CAP: usize> Send for Drain<'a, T, CAP> {}

impl<'a, T: 'a, const CAP: usize> Iterator for Drain<'a, T, CAP> {
    type Item = T;
    fn next(&mut self) -> Option<T> {
        match self.iter.next() {
            Some(elt) => Some(unsafe { ptr::read(elt as *const T) }),
            None => None,
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.iter.size_hint()
    }
}

impl<'a, T: 'a, const CAP: usize> DoubleEndedIterator for Drain<'a, T, CAP> {
    fn next_back(&mut self) -> Option<T> {
        match self.iter.next_back() {
            Some(elt) => Some(unsafe { ptr::read(elt as *const T) }),
            None => None,
        }
    }
}

impl<'a, T: 'a, const CAP: usize> ExactSizeIterator for Drain<'a, T, CAP> {}

impl<'a, T: 'a, const CAP: usize> Drop for Drain<'a, T, CAP> {
    fn drop(&mut self) {
        while let Some(x) = self.next() {
            drop(x);
        }
        if self.tail_len > 0 {
            unsafe {
                let source_vec = &mut *self.vec;
                let start = source_vec.len();
                let tail = self.tail_start;
                let ptr = source_vec.as_mut_ptr();
                ptr::copy(ptr.add(tail), ptr.add(start), self.tail_len);
                source_vec.set_len(start + self.tail_len);
            }
        }
    }
}

impl<T, const CAP: usize> Extend<T> for ArrayVec<T, CAP> {
    #[track_caller]
    fn extend<I: IntoIterator<Item = T>>(&mut self, iter: I) {
        self.extend_from_iter(iter, true)
    }
}

impl<T, const CAP: usize> iter::FromIterator<T> for ArrayVec<T, CAP> {
    #[track_caller]
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> ArrayVec<T, CAP> {
        let mut array = ArrayVec::new();
        array.extend(iter);
        array
    }
}

impl<T: Clone, const CAP: usize> Clone for ArrayVec<T, CAP> {
    fn clone(&self) -> ArrayVec<T, CAP> {
        self.iter().cloned().collect()
    }
    fn clone_from(&mut self, rhs: &ArrayVec<T, CAP>) {
        let prefix = cmp::min(self.len(), rhs.len());
        self.as_mut_slice()[..prefix].clone_from_slice(&rhs.as_slice()[..prefix]);
        if prefix < self.len() {
            self.truncate(prefix);
        } else {
            let rhs_elems = &rhs.as_slice()[self.len()..];
            self.extend_from_slice(rhs_elems);
        }
    }
}

impl<T: Hash, const CAP: usize> Hash for ArrayVec<T, CAP> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        Hash::hash(&**self, state)
    }
}

impl<T: PartialEq, const CAP: usize> PartialEq for ArrayVec<T, CAP> {
    fn eq(&self, other: &ArrayVec<T, CAP>) -> bool {
        **self == **other
    }
}

impl<T: PartialEq, const CAP: usize> PartialEq<[T]> for ArrayVec<T, CAP> {
    fn eq(&self, other: &[T]) -> bool {
        **self == *other
    }
}

impl<T: Eq, const CAP: usize> Eq for ArrayVec<T, CAP> {}

impl<T, const CAP: usize> Borrow<[T]> for ArrayVec<T, CAP> {
    fn borrow(&self) -> &[T] {
        self
    }
}

impl<T, const CAP: usize> BorrowMut<[T]> for ArrayVec<T, CAP> {
    fn borrow_mut(&mut self) -> &mut [T] {
        self
    }
}

impl<T, const CAP: usize> AsRef<[T]> for ArrayVec<T, CAP> {
    fn as_ref(&self) -> &[T] {
        self
    }
}

impl<T, const CAP: usize> AsMut<[T]> for ArrayVec<T, CAP> {
    fn as_mut(&mut self) -> &mut [T] {
        self
    }
}

impl<T: fmt::Debug, const CAP: usize> fmt::Debug for ArrayVec<T, CAP> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        (**self).fmt(f)
    }
}

impl<T, const CAP: usize> Default for ArrayVec<T, CAP> {
    fn default() -> ArrayVec<T, CAP> {
        ArrayVec::new()
    }
}

impl<T: PartialOrd, const CAP: usize> PartialOrd for ArrayVec<T, CAP> {
    fn partial_cmp(&self, other: &ArrayVec<T, CAP>) -> Option<cmp::Ordering> {
        (**self).partial_cmp(other)
    }
}

impl<T: Ord, const CAP: usize> Ord for ArrayVec<T, CAP> {
    fn cmp(&self, other: &ArrayVec<T, CAP>) -> cmp::Ordering {
        (**self).cmp(other)
    }
}
