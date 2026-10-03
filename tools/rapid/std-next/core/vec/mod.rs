//! Vec<T>: a contiguous growable array (alloc::vec).

mod drain;
mod into_iter;

pub use drain::Drain;
pub use into_iter::IntoIter;

use crate::borrow::{Borrow, BorrowMut, Cow, ToOwned};
use crate::cmp::Ordering;
use crate::fmt;
use crate::hash::{Hash, Hasher};
use crate::heap::{self, Layout};
use crate::intrinsics_mem as im;
use crate::ops::{Deref, DerefMut, Index, IndexMut, RangeBounds};
use crate::slice::SliceIndex;

pub struct Vec<T> {
    ptr: *mut T,
    cap: usize,
    len: usize,
}

unsafe impl<T: Send> Send for Vec<T> {}
unsafe impl<T: Sync> Sync for Vec<T> {}

fn is_zst<T>() -> bool {
    crate::mem::size_of::<T>() == 0
}

fn min_non_zero_cap(elem_size: usize) -> usize {
    if elem_size == 1 {
        8
    } else if elem_size <= 1024 {
        4
    } else {
        1
    }
}

#[cold]
fn capacity_overflow() -> ! {
    crate::panicking::panic_str("capacity overflow")
}

impl<T> Vec<T> {
    pub const fn new() -> Vec<T> {
        Vec { ptr: crate::ptr::dangling_mut::<T>(), cap: 0, len: 0 }
    }

    pub fn with_capacity(capacity: usize) -> Vec<T> {
        let mut v = Vec::new();
        if capacity > 0 && !is_zst::<T>() {
            v.grow_to(capacity);
        }
        v
    }

    pub unsafe fn from_raw_parts(ptr: *mut T, length: usize, capacity: usize) -> Vec<T> {
        Vec { ptr, cap: capacity, len: length }
    }

    pub fn into_raw_parts(self) -> (*mut T, usize, usize) {
        let me = crate::mem::ManuallyDrop::new(self);
        (me.ptr, me.len, me.cap)
    }

    pub fn capacity(&self) -> usize {
        if is_zst::<T>() {
            usize::MAX
        } else {
            self.cap
        }
    }

    fn layout_for(cap: usize) -> Layout {
        match Layout::array::<T>(cap) {
            Ok(l) => l,
            Err(_) => capacity_overflow(),
        }
    }

    /// Reallocates to exactly `new_cap` (>= len). Not for ZSTs.
    fn grow_to(&mut self, new_cap: usize) {
        let new_layout = Vec::<T>::layout_for(new_cap);
        if new_layout.size() > isize::MAX as usize {
            capacity_overflow();
        }
        let p = if self.cap == 0 {
            heap::alloc_or_abort(new_layout)
        } else {
            heap::realloc_or_abort(self.ptr as *mut u8, Vec::<T>::layout_for(self.cap), new_layout.size())
        };
        self.ptr = p as *mut T;
        self.cap = new_cap;
    }

    pub fn reserve(&mut self, additional: usize) {
        if is_zst::<T>() {
            return;
        }
        if self.cap - self.len >= additional {
            return;
        }
        let required = match self.len.checked_add(additional) {
            Some(r) => r,
            None => capacity_overflow(),
        };
        let mut cap = self.cap * 2;
        if cap < required {
            cap = required;
        }
        let min = min_non_zero_cap(crate::mem::size_of::<T>());
        if cap < min {
            cap = min;
        }
        self.grow_to(cap);
    }

    pub fn reserve_exact(&mut self, additional: usize) {
        if is_zst::<T>() {
            return;
        }
        if self.cap - self.len >= additional {
            return;
        }
        let required = match self.len.checked_add(additional) {
            Some(r) => r,
            None => capacity_overflow(),
        };
        self.grow_to(required);
    }

    pub fn try_reserve(&mut self, additional: usize) -> Result<(), crate::collections::TryReserveError> {
        if self.len.checked_add(additional).is_none() {
            return Err(crate::collections::TryReserveError::capacity_overflow());
        }
        self.reserve(additional);
        Ok(())
    }

    pub fn try_reserve_exact(&mut self, additional: usize) -> Result<(), crate::collections::TryReserveError> {
        if self.len.checked_add(additional).is_none() {
            return Err(crate::collections::TryReserveError::capacity_overflow());
        }
        self.reserve_exact(additional);
        Ok(())
    }

    pub fn shrink_to_fit(&mut self) {
        self.shrink_to(0);
    }

    pub fn shrink_to(&mut self, min_capacity: usize) {
        if is_zst::<T>() {
            return;
        }
        let target = if self.len > min_capacity { self.len } else { min_capacity };
        if target >= self.cap {
            return;
        }
        if target == 0 {
            heap::free(self.ptr as *mut u8, Vec::<T>::layout_for(self.cap));
            self.ptr = crate::ptr::dangling_mut::<T>();
            self.cap = 0;
        } else {
            let p = heap::realloc_or_abort(
                self.ptr as *mut u8,
                Vec::<T>::layout_for(self.cap),
                Vec::<T>::layout_for(target).size(),
            );
            self.ptr = p as *mut T;
            self.cap = target;
        }
    }

    pub fn into_boxed_slice(self) -> crate::boxed::Box<[T]> {
        let mut me = self;
        me.shrink_to_fit();
        let me = crate::mem::ManuallyDrop::new(me);
        unsafe { crate::boxed::Box::from_raw(im::slice_from_raw_mut(me.ptr, me.len) as *mut [T]) }
    }

    pub fn truncate(&mut self, len: usize) {
        while self.len > len {
            self.len -= 1;
            unsafe { im::drop_in_place(im::ptr_add_mut(self.ptr, self.len)) };
        }
    }

    pub fn as_slice(&self) -> &[T] {
        unsafe { im::slice_from_raw(self.ptr as *const T, self.len) }
    }

    pub fn as_mut_slice(&mut self) -> &mut [T] {
        unsafe { im::slice_from_raw_mut(self.ptr, self.len) }
    }

    pub fn as_ptr(&self) -> *const T {
        self.ptr as *const T
    }

    pub fn as_mut_ptr(&mut self) -> *mut T {
        self.ptr
    }

    pub unsafe fn set_len(&mut self, new_len: usize) {
        self.len = new_len;
    }

    #[track_caller]
    pub fn swap_remove(&mut self, index: usize) -> T {
        let len = self.len;
        if index >= len {
            crate::panicking::panic_fmt(format_args!("swap_remove index (is {}) should be < len (is {})", index, len));
        }
        unsafe {
            let value = im::ptr_read(im::ptr_add(self.ptr as *const T, index));
            let base = self.ptr;
            im::copy(im::ptr_add(base as *const T, len - 1), im::ptr_add_mut(base, index), 1);
            self.len = len - 1;
            value
        }
    }

    #[track_caller]
    pub fn insert(&mut self, index: usize, element: T) {
        let len = self.len;
        if index > len {
            crate::panicking::panic_fmt(format_args!("insertion index (is {}) should be <= len (is {})", index, len));
        }
        if len == self.capacity() {
            self.reserve(1);
        }
        unsafe {
            let p = im::ptr_add_mut(self.ptr, index);
            if index < len {
                im::copy(p as *const T, im::ptr_add_mut(p, 1), len - index);
            }
            im::ptr_write(p, element);
        }
        self.len = len + 1;
    }

    #[track_caller]
    pub fn remove(&mut self, index: usize) -> T {
        let len = self.len;
        if index >= len {
            crate::panicking::panic_fmt(format_args!("removal index (is {}) should be < len (is {})", index, len));
        }
        unsafe {
            let p = im::ptr_add_mut(self.ptr, index);
            let ret = im::ptr_read(p as *const T);
            im::copy(im::ptr_add(p as *const T, 1), p, len - index - 1);
            self.len = len - 1;
            ret
        }
    }

    pub fn retain<F: FnMut(&T) -> bool>(&mut self, f: F) {
        let mut f = f;
        self.retain_mut(|x| f(x));
    }

    pub fn retain_mut<F: FnMut(&mut T) -> bool>(&mut self, f: F) {
        let mut f = f;
        let len = self.len;
        // elements are dropped in order, survivors moved down (same observable order as core)
        self.len = 0;
        let mut deleted = 0;
        let mut i = 0;
        while i < len {
            unsafe {
                let cur = im::ptr_add_mut(self.ptr, i);
                if !f(&mut *cur) {
                    deleted += 1;
                    im::drop_in_place(cur);
                } else if deleted > 0 {
                    im::copy_nonoverlapping(cur as *const T, im::ptr_add_mut(self.ptr, i - deleted), 1);
                }
            }
            i += 1;
        }
        self.len = len - deleted;
    }

    pub fn dedup_by_key<K: PartialEq, F: FnMut(&mut T) -> K>(&mut self, key: F) {
        let mut key = key;
        self.dedup_by(|a, b| key(a) == key(b))
    }

    /// Removes consecutive elements for which `same_bucket(current, previous_kept)` is true.
    pub fn dedup_by<F: FnMut(&mut T, &mut T) -> bool>(&mut self, same_bucket: F) {
        let mut same_bucket = same_bucket;
        let len = self.len;
        if len <= 1 {
            return;
        }
        let mut write = 1;
        let mut read = 1;
        unsafe {
            while read < len {
                let r = im::ptr_add_mut(self.ptr, read);
                let prev = im::ptr_add_mut(self.ptr, write - 1);
                if same_bucket(&mut *r, &mut *prev) {
                    im::drop_in_place(r);
                } else {
                    if read != write {
                        im::copy_nonoverlapping(r as *const T, im::ptr_add_mut(self.ptr, write), 1);
                    }
                    write += 1;
                }
                read += 1;
            }
        }
        self.len = write;
    }

    pub fn push(&mut self, value: T) {
        if self.len == self.capacity() {
            self.reserve(1);
        }
        unsafe { im::ptr_write(im::ptr_add_mut(self.ptr, self.len), value) };
        self.len += 1;
    }

    pub fn push_within_capacity(&mut self, value: T) -> Result<(), T> {
        if self.len == self.capacity() {
            return Err(value);
        }
        self.push(value);
        Ok(())
    }

    pub fn pop(&mut self) -> Option<T> {
        if self.len == 0 {
            None
        } else {
            self.len -= 1;
            Some(unsafe { im::ptr_read(im::ptr_add(self.ptr as *const T, self.len)) })
        }
    }

    pub fn pop_if<F: FnOnce(&mut T) -> bool>(&mut self, predicate: F) -> Option<T> {
        if self.len == 0 {
            return None;
        }
        let last = unsafe { &mut *im::ptr_add_mut(self.ptr, self.len - 1) };
        if predicate(last) {
            self.pop()
        } else {
            None
        }
    }

    pub fn append(&mut self, other: &mut Vec<T>) {
        let count = other.len;
        self.reserve(count);
        unsafe {
            im::copy_nonoverlapping(other.ptr as *const T, im::ptr_add_mut(self.ptr, self.len), count);
            other.len = 0;
        }
        self.len += count;
    }

    pub fn drain<R: RangeBounds<usize>>(&mut self, range: R) -> Drain<'_, T> {
        let (start, end) = crate::slice::range(&range, self.len);
        drain::new(self, start, end)
    }

    pub fn clear(&mut self) {
        self.truncate(0)
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    #[track_caller]
    pub fn split_off(&mut self, at: usize) -> Vec<T> {
        if at > self.len {
            crate::panicking::panic_fmt(format_args!("`at` split index (is {}) should be <= len (is {})", at, self.len));
        }
        let other_len = self.len - at;
        let mut other = Vec::with_capacity(other_len);
        unsafe {
            im::copy_nonoverlapping(im::ptr_add(self.ptr as *const T, at), other.ptr, other_len);
            self.len = at;
            other.len = other_len;
        }
        other
    }

    pub fn resize_with<F: FnMut() -> T>(&mut self, new_len: usize, f: F) {
        let mut f = f;
        let len = self.len;
        if new_len > len {
            self.reserve(new_len - len);
            while self.len < new_len {
                let v = f();
                self.push(v);
            }
        } else {
            self.truncate(new_len);
        }
    }

    pub fn leak<'a>(self) -> &'a mut [T] {
        let me = crate::mem::ManuallyDrop::new(self);
        unsafe { im::slice_from_raw_mut(me.ptr, me.len) }
    }

    pub fn spare_capacity_mut(&mut self) -> &mut [crate::mem::MaybeUninit<T>] {
        unsafe {
            im::slice_from_raw_mut(
                im::ptr_add_mut(self.ptr, self.len) as *mut crate::mem::MaybeUninit<T>,
                self.capacity() - self.len,
            )
        }
    }

    /// Replaces `range` with the items of `replace_with`; returns the removed items.
    pub fn splice<R: RangeBounds<usize>, I: IntoIterator<Item = T>>(&mut self, range: R, replace_with: I) -> IntoIter<T> {
        let (start, end) = crate::slice::range(&range, self.len);
        let mut tail = self.split_off(end);
        let removed = self.split_off(start);
        self.extend(replace_with);
        self.append(&mut tail);
        removed.into_iter()
    }
}

impl<T: Clone> Vec<T> {
    pub fn resize(&mut self, new_len: usize, value: T) {
        let len = self.len;
        if new_len > len {
            self.reserve(new_len - len);
            let mut n = new_len - len;
            while n > 1 {
                self.push(value.clone());
                n -= 1;
            }
            self.push(value);
        } else {
            self.truncate(new_len);
        }
    }

    pub fn extend_from_slice(&mut self, other: &[T]) {
        self.reserve(other.len());
        let mut i = 0;
        while i < other.len() {
            unsafe { im::ptr_write(im::ptr_add_mut(self.ptr, self.len), other[i].clone()) };
            self.len += 1;
            i += 1;
        }
    }

    pub fn extend_from_within<R: RangeBounds<usize>>(&mut self, src: R) {
        let (start, end) = crate::slice::range(&src, self.len);
        self.reserve(end - start);
        let mut i = start;
        while i < end {
            let v = unsafe { (*im::ptr_add(self.ptr as *const T, i)).clone() };
            unsafe { im::ptr_write(im::ptr_add_mut(self.ptr, self.len), v) };
            self.len += 1;
            i += 1;
        }
    }
}

impl<T: PartialEq> Vec<T> {
    pub fn dedup(&mut self) {
        self.dedup_by(|a, b| a == b)
    }
}

/// `vec![elem; n]`
pub fn from_elem<T: Clone>(elem: T, n: usize) -> Vec<T> {
    let mut v = Vec::with_capacity(n);
    v.resize(n, elem);
    v
}

impl<T> Drop for Vec<T> {
    fn drop(&mut self) {
        unsafe {
            if crate::mem::needs_drop::<T>() {
                let mut i = 0;
                while i < self.len {
                    im::drop_in_place(im::ptr_add_mut(self.ptr, i));
                    i += 1;
                }
            }
        }
        if !is_zst::<T>() && self.cap != 0 {
            heap::free(self.ptr as *mut u8, Vec::<T>::layout_for(self.cap));
        }
    }
}

impl<T> Deref for Vec<T> {
    type Target = [T];
    fn deref(&self) -> &[T] {
        self.as_slice()
    }
}

impl<T> DerefMut for Vec<T> {
    fn deref_mut(&mut self) -> &mut [T] {
        self.as_mut_slice()
    }
}

impl<T, I: SliceIndex<[T]>> Index<I> for Vec<T> {
    type Output = I::Output;
    #[track_caller]
    fn index(&self, index: I) -> &I::Output {
        index.index(self.as_slice())
    }
}

impl<T, I: SliceIndex<[T]>> IndexMut<I> for Vec<T> {
    #[track_caller]
    fn index_mut(&mut self, index: I) -> &mut I::Output {
        index.index_mut(self.as_mut_slice())
    }
}

impl<T: Clone> Clone for Vec<T> {
    fn clone(&self) -> Vec<T> {
        let mut v = Vec::with_capacity(self.len);
        v.extend_from_slice(self.as_slice());
        v
    }
    fn clone_from(&mut self, source: &Vec<T>) {
        self.clear();
        self.extend_from_slice(source.as_slice());
    }
}

impl<T> Default for Vec<T> {
    fn default() -> Vec<T> {
        Vec::new()
    }
}

impl<T: fmt::Debug> fmt::Debug for Vec<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_slice(), f)
    }
}

impl<T: Hash> Hash for Vec<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        Hash::hash(self.as_slice(), state)
    }
}

impl<T, U> PartialEq<Vec<U>> for Vec<T>
where
    T: PartialEq<U>,
{
    fn eq(&self, other: &Vec<U>) -> bool {
        crate::slice::slice_eq(self.as_slice(), other.as_slice())
    }
}

impl<T, U> PartialEq<[U]> for Vec<T>
where
    T: PartialEq<U>,
{
    fn eq(&self, other: &[U]) -> bool {
        crate::slice::slice_eq(self.as_slice(), other)
    }
}

impl<'a, T, U> PartialEq<&'a [U]> for Vec<T>
where
    T: PartialEq<U>,
{
    fn eq(&self, other: &&'a [U]) -> bool {
        crate::slice::slice_eq(self.as_slice(), *other)
    }
}

impl<'a, T, U> PartialEq<&'a mut [U]> for Vec<T>
where
    T: PartialEq<U>,
{
    fn eq(&self, other: &&'a mut [U]) -> bool {
        crate::slice::slice_eq(self.as_slice(), &**other)
    }
}

#[cfg(any(rapid_check, rapid_const_generics))]
impl<T, U, const N: usize> PartialEq<[U; N]> for Vec<T>
where
    T: PartialEq<U>,
{
    fn eq(&self, other: &[U; N]) -> bool {
        crate::slice::slice_eq(self.as_slice(), &other[..])
    }
}

#[cfg(any(rapid_check, rapid_const_generics))]
impl<'a, T, U, const N: usize> PartialEq<&'a [U; N]> for Vec<T>
where
    T: PartialEq<U>,
{
    fn eq(&self, other: &&'a [U; N]) -> bool {
        crate::slice::slice_eq(self.as_slice(), &other[..])
    }
}

impl<T, U> PartialEq<Vec<U>> for [T]
where
    T: PartialEq<U>,
{
    fn eq(&self, other: &Vec<U>) -> bool {
        crate::slice::slice_eq(self, other.as_slice())
    }
}

impl<'a, T, U> PartialEq<Vec<U>> for &'a [T]
where
    T: PartialEq<U>,
{
    fn eq(&self, other: &Vec<U>) -> bool {
        crate::slice::slice_eq(*self, other.as_slice())
    }
}

#[cfg(not(rapid_check))]
impl<'a, T: Clone, U> PartialEq<Vec<U>> for Cow<'a, [T]>
where
    T: PartialEq<U>,
{
    fn eq(&self, other: &Vec<U>) -> bool {
        crate::slice::slice_eq(&**self, other.as_slice())
    }
}

impl<T: Eq> Eq for Vec<T> {}

impl<T: PartialOrd> PartialOrd for Vec<T> {
    fn partial_cmp(&self, other: &Vec<T>) -> Option<Ordering> {
        crate::slice::slice_partial_cmp(self.as_slice(), other.as_slice())
    }
}

impl<T: Ord> Ord for Vec<T> {
    fn cmp(&self, other: &Vec<T>) -> Ordering {
        crate::slice::slice_cmp(self.as_slice(), other.as_slice())
    }
}

impl<T> AsRef<Vec<T>> for Vec<T> {
    fn as_ref(&self) -> &Vec<T> {
        self
    }
}
impl<T> AsMut<Vec<T>> for Vec<T> {
    fn as_mut(&mut self) -> &mut Vec<T> {
        self
    }
}
impl<T> AsRef<[T]> for Vec<T> {
    fn as_ref(&self) -> &[T] {
        self
    }
}
impl<T> AsMut<[T]> for Vec<T> {
    fn as_mut(&mut self) -> &mut [T] {
        self
    }
}
impl<T> Borrow<[T]> for Vec<T> {
    fn borrow(&self) -> &[T] {
        &self[..]
    }
}
impl<T> BorrowMut<[T]> for Vec<T> {
    fn borrow_mut(&mut self) -> &mut [T] {
        &mut self[..]
    }
}

#[cfg(not(rapid_check))]
impl<T: Clone> ToOwned for [T] {
    type Owned = Vec<T>;
    fn to_owned(&self) -> Vec<T> {
        self.to_vec()
    }
}

impl<'a, T: Clone> From<&'a [T]> for Vec<T> {
    fn from(s: &'a [T]) -> Vec<T> {
        crate::slice::to_vec(s)
    }
}
impl<'a, T: Clone> From<&'a mut [T]> for Vec<T> {
    fn from(s: &'a mut [T]) -> Vec<T> {
        crate::slice::to_vec(s)
    }
}
#[cfg(any(rapid_check, rapid_const_generics))]
impl<T, const N: usize> From<[T; N]> for Vec<T> {
    fn from(s: [T; N]) -> Vec<T> {
        let mut v = Vec::with_capacity(N);
        let s = crate::mem::ManuallyDrop::new(s);
        unsafe {
            im::copy_nonoverlapping(&*s as *const [T; N] as *const T, v.ptr, N);
            v.len = N;
        }
        v
    }
}
#[cfg(any(rapid_check, rapid_const_generics))]
impl<'a, T: Clone, const N: usize> From<&'a [T; N]> for Vec<T> {
    fn from(s: &'a [T; N]) -> Vec<T> {
        crate::slice::to_vec(&s[..])
    }
}
#[cfg(not(rapid_check))]
impl<'a, T: Clone> From<Cow<'a, [T]>> for Vec<T> {
    fn from(s: Cow<'a, [T]>) -> Vec<T> {
        s.into_owned()
    }
}
#[cfg(not(rapid_check))]
impl<T> From<crate::boxed::Box<[T]>> for Vec<T> {
    fn from(s: crate::boxed::Box<[T]>) -> Vec<T> {
        s.into_vec()
    }
}
#[cfg(not(rapid_check))]
impl<T> From<Vec<T>> for crate::boxed::Box<[T]> {
    fn from(v: Vec<T>) -> crate::boxed::Box<[T]> {
        v.into_boxed_slice()
    }
}
impl<'a> From<&'a str> for Vec<u8> {
    fn from(s: &'a str) -> Vec<u8> {
        From::from(s.as_bytes())
    }
}
#[cfg(not(rapid_check))]
impl<'a, T: Clone> From<&'a Vec<T>> for Cow<'a, [T]> {
    fn from(v: &'a Vec<T>) -> Cow<'a, [T]> {
        Cow::Borrowed(v.as_slice())
    }
}
#[cfg(not(rapid_check))]
impl<'a, T: Clone> From<Vec<T>> for Cow<'a, [T]> {
    fn from(v: Vec<T>) -> Cow<'a, [T]> {
        Cow::Owned(v)
    }
}
#[cfg(not(rapid_check))]
impl<'a, T: Clone> From<&'a [T]> for Cow<'a, [T]> {
    fn from(s: &'a [T]) -> Cow<'a, [T]> {
        Cow::Borrowed(s)
    }
}

impl<T> FromIterator<T> for Vec<T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Vec<T> {
        let mut v = Vec::new();
        v.extend(iter);
        v
    }
}

impl<T> Extend<T> for Vec<T> {
    fn extend<I: IntoIterator<Item = T>>(&mut self, iter: I) {
        let mut iter = iter.into_iter();
        let (lower, _) = iter.size_hint();
        self.reserve(lower);
        while let Some(x) = iter.next() {
            if self.len == self.capacity() {
                let (lower, _) = iter.size_hint();
                self.reserve(lower.saturating_add(1));
            }
            unsafe { im::ptr_write(im::ptr_add_mut(self.ptr, self.len), x) };
            self.len += 1;
        }
    }
    #[cfg(not(rapid_check))]
    fn extend_one(&mut self, item: T) {
        self.push(item);
    }
    #[cfg(not(rapid_check))]
    fn extend_reserve(&mut self, additional: usize) {
        self.reserve(additional);
    }
}

impl<'a, T: Copy + 'a> Extend<&'a T> for Vec<T> {
    fn extend<I: IntoIterator<Item = &'a T>>(&mut self, iter: I) {
        let mut iter = iter.into_iter();
        let (lower, _) = iter.size_hint();
        self.reserve(lower);
        while let Some(x) = iter.next() {
            self.push(*x);
        }
    }
    #[cfg(not(rapid_check))]
    fn extend_one(&mut self, item: &'a T) {
        self.push(*item);
    }
}

impl<T> IntoIterator for Vec<T> {
    type Item = T;
    type IntoIter = IntoIter<T>;
    fn into_iter(self) -> IntoIter<T> {
        into_iter::new(self)
    }
}

impl<'a, T> IntoIterator for &'a Vec<T> {
    type Item = &'a T;
    type IntoIter = crate::slice::Iter<'a, T>;
    fn into_iter(self) -> crate::slice::Iter<'a, T> {
        crate::slice::Iter::new(self.as_slice())
    }
}

impl<'a, T> IntoIterator for &'a mut Vec<T> {
    type Item = &'a mut T;
    type IntoIter = crate::slice::IterMut<'a, T>;
    fn into_iter(self) -> crate::slice::IterMut<'a, T> {
        crate::slice::IterMut::new(self.as_mut_slice())
    }
}

#[cfg(any(rapid_check, rapid_const_generics))]
impl<T, const N: usize> TryFrom<Vec<T>> for [T; N] {
    type Error = Vec<T>;
    fn try_from(vec: Vec<T>) -> Result<[T; N], Vec<T>> {
        if vec.len() != N {
            return Err(vec);
        }
        let me = crate::mem::ManuallyDrop::new(vec);
        let arr = unsafe { im::ptr_read(me.ptr as *const [T; N]) };
        if !is_zst::<T>() && me.cap != 0 {
            heap::free(me.ptr as *mut u8, Vec::<T>::layout_for(me.cap));
        }
        Ok(arr)
    }
}
