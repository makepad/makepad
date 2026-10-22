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

/// RawVec's stored capacity: the high bit stays unused, including for ZSTs.
#[derive(Clone, Copy)]
#[repr(transparent)]
#[cfg_attr(all(not(relax_check), target_pointer_width = "64"), rustc_layout_scalar_valid_range_end(0x7fff_ffff_ffff_ffff))]
#[cfg_attr(all(not(relax_check), target_pointer_width = "32"), rustc_layout_scalar_valid_range_end(0x7fff_ffff))]
pub(crate) struct Cap(usize);

impl Cap {
    pub(crate) const ZERO: Cap = unsafe { Cap::new::<u8>(0) };

    /// ZSTs report usize::MAX capacity without storing it.
    /// # Safety: for non-ZSTs, `cap` must be at most isize::MAX.
    pub(crate) const unsafe fn new<T>(cap: usize) -> Cap {
        Cap(if crate::mem::size_of::<T>() == 0 { 0 } else { cap })
    }

    pub(crate) const fn get(self) -> usize {
        self.0
    }
}

#[rustc_diagnostic_item = "Vec"]
#[rustc_insignificant_dtor]
pub struct Vec<T> {
    ptr: crate::ptr::NonNull<T>,
    cap: Cap,
    len: usize,
    _marker: crate::marker::PhantomData<T>,
}

unsafe impl<T: Send> Send for Vec<T> {}
unsafe impl<T: Sync> Sync for Vec<T> {}

fn is_zst<T>() -> bool {
    crate::mem::size_of::<T>() == 0
}

const fn min_non_zero_cap(elem_size: usize) -> usize {
    if elem_size == 1 {
        8
    } else if elem_size <= 1024 {
        4
    } else {
        1
    }
}

#[cold]
const fn capacity_overflow() -> ! {
    crate::panicking::panic_str("capacity overflow")
}

impl<T> Vec<T> {
    pub const fn new() -> Vec<T> {
        Vec { ptr: crate::ptr::NonNull::dangling(), cap: Cap::ZERO, len: 0, _marker: crate::marker::PhantomData }
    }

    pub const fn with_capacity(capacity: usize) -> Vec<T> {
        let mut v = Vec::new();
        if capacity > 0 && !is_zst::<T>() {
            v.grow_to(capacity);
        }
        v
    }

    /// # Safety: `ptr` must be non-null and aligned, with `length` initialized elements.
    /// For non-ZSTs it must own an allocation for `capacity` elements, fitting in
    /// isize::MAX bytes, and `length` must not exceed `capacity`.
    pub const unsafe fn from_raw_parts(ptr: *mut T, length: usize, capacity: usize) -> Vec<T> {
        Vec { ptr: crate::ptr::NonNull::new_unchecked(ptr), cap: Cap::new::<T>(capacity), len: length, _marker: crate::marker::PhantomData }
    }

    pub const fn into_raw_parts(self) -> (*mut T, usize, usize) {
        let me = crate::mem::ManuallyDrop::new(self);
        (me.ptr.as_ptr(), me.len, me.capacity())
    }

    pub const fn capacity(&self) -> usize {
        if is_zst::<T>() {
            usize::MAX
        } else {
            self.cap.get()
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
        if self.try_grow_to(new_cap).is_err() {
            capacity_overflow();
        }
    }

    fn try_grow_to(&mut self, new_cap: usize) -> Result<(), crate::collections::TryReserveError> {
        let new_layout = match Layout::array::<T>(new_cap) {
            Ok(l) => l,
            Err(_) => return Err(crate::collections::TryReserveError::capacity_overflow()),
        };
        let p = if self.cap.get() == 0 {
            heap::alloc_or_abort(new_layout)
        } else {
            heap::realloc_or_abort(self.ptr.as_ptr() as *mut u8, Vec::<T>::layout_for(self.cap.get()), new_layout.size())
        };
        self.ptr = unsafe { crate::ptr::NonNull::new_unchecked(p as *mut T) };
        // Layout::array checked that the non-ZST allocation fits in isize::MAX bytes.
        self.cap = unsafe { Cap::new::<T>(new_cap) };
        Ok(())
    }

    pub fn reserve(&mut self, additional: usize) {
        if self.try_reserve(additional).is_err() {
            capacity_overflow();
        }
    }

    pub fn reserve_exact(&mut self, additional: usize) {
        if self.try_reserve_exact(additional).is_err() {
            capacity_overflow();
        }
    }

    pub fn try_reserve(&mut self, additional: usize) -> Result<(), crate::collections::TryReserveError> {
        if self.capacity() - self.len >= additional {
            return Ok(());
        }
        let required = match self.len.checked_add(additional) {
            Some(r) => r,
            None => return Err(crate::collections::TryReserveError::capacity_overflow()),
        };
        let mut cap = self.cap.get() * 2;
        if cap < required {
            cap = required;
        }
        let min = min_non_zero_cap(crate::mem::size_of::<T>());
        if cap < min {
            cap = min;
        }
        self.try_grow_to(cap)
    }

    pub fn try_reserve_exact(&mut self, additional: usize) -> Result<(), crate::collections::TryReserveError> {
        if self.capacity() - self.len >= additional {
            return Ok(());
        }
        let required = match self.len.checked_add(additional) {
            Some(r) => r,
            None => return Err(crate::collections::TryReserveError::capacity_overflow()),
        };
        self.try_grow_to(required)
    }

    pub fn shrink_to_fit(&mut self) {
        self.shrink_to(0);
    }

    pub fn shrink_to(&mut self, min_capacity: usize) {
        if is_zst::<T>() {
            return;
        }
        let target = if self.len > min_capacity { self.len } else { min_capacity };
        if target >= self.cap.get() {
            return;
        }
        if target == 0 {
            heap::free(self.ptr.as_ptr() as *mut u8, Vec::<T>::layout_for(self.cap.get()));
            self.ptr = crate::ptr::NonNull::dangling();
            self.cap = Cap::ZERO;
        } else {
            let p = heap::realloc_or_abort(
                self.ptr.as_ptr() as *mut u8,
                Vec::<T>::layout_for(self.cap.get()),
                Vec::<T>::layout_for(target).size(),
            );
            self.ptr = unsafe { crate::ptr::NonNull::new_unchecked(p as *mut T) };
            self.cap = unsafe { Cap::new::<T>(target) };
        }
    }

    pub fn into_boxed_slice(self) -> crate::boxed::Box<[T]> {
        let mut me = self;
        me.shrink_to_fit();
        let me = crate::mem::ManuallyDrop::new(me);
        unsafe { crate::boxed::Box::from_raw(im::slice_from_raw_mut(me.ptr.as_ptr(), me.len) as *mut [T]) }
    }

    pub fn truncate(&mut self, len: usize) {
        if !crate::mem::needs_drop::<T>() {
            if len < self.len {
                self.len = len;
            }
            return;
        }
        while self.len > len {
            self.len -= 1;
            unsafe { im::drop_in_place(im::ptr_add_mut(self.ptr.as_ptr(), self.len)) };
        }
    }

    pub const fn as_slice(&self) -> &[T] {
        unsafe { im::slice_from_raw(self.ptr.as_ptr() as *const T, self.len) }
    }

    pub const fn as_mut_slice(&mut self) -> &mut [T] {
        unsafe { im::slice_from_raw_mut(self.ptr.as_ptr(), self.len) }
    }

    #[rustc_as_ptr]
    #[rustc_never_returns_null_ptr]
    pub const fn as_ptr(&self) -> *const T {
        self.ptr.as_ptr() as *const T
    }

    #[rustc_as_ptr]
    #[rustc_never_returns_null_ptr]
    pub const fn as_mut_ptr(&mut self) -> *mut T {
        self.ptr.as_ptr()
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
            let value = im::ptr_read(im::ptr_add(self.ptr.as_ptr() as *const T, index));
            let base = self.ptr.as_ptr();
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
            let p = im::ptr_add_mut(self.ptr.as_ptr(), index);
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
            let p = im::ptr_add_mut(self.ptr.as_ptr(), index);
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

    /// Port of real alloc's `Vec::retain_mut` (library/alloc/src/vec/mod.rs): a first loop that only
    /// reads until the first removed element, then the compacting loop; a panic guard that, if the
    /// predicate or a drop panics, shifts the unchecked tail over the holes and sets the length
    /// (the elements not yet visited stay in the Vec, as in real std).
    pub fn retain_mut<F: FnMut(&mut T) -> bool>(&mut self, f: F) {
        let mut f = f;
        let original_len = self.len;
        if original_len == 0 {
            return;
        }
        struct PanicGuard<'a, T> {
            v: &'a mut Vec<T>,
            read: usize,
            write: usize,
            original_len: usize,
        }
        impl<T> Drop for PanicGuard<'_, T> {
            #[cold]
            fn drop(&mut self) {
                let remaining = self.original_len - self.read;
                unsafe {
                    im::copy(im::ptr_add(self.v.ptr.as_ptr() as *const T, self.read), im::ptr_add_mut(self.v.ptr.as_ptr(), self.write), remaining);
                    self.v.set_len(self.write + remaining);
                }
            }
        }
        let mut read = 0;
        loop {
            // read < original_len
            let cur = unsafe { &mut *im::ptr_add_mut(self.ptr.as_ptr(), read) };
            if !f(cur) {
                break;
            }
            read += 1;
            if read == original_len {
                // every element kept
                return;
            }
        }
        // at least one element goes; `read` advances before the drop (a panicking drop is not
        // dropped twice)
        let mut g = PanicGuard { v: self, read: read + 1, write: read, original_len };
        unsafe { im::drop_in_place(im::ptr_add_mut(g.v.ptr.as_ptr(), read)) };
        while g.read < g.original_len {
            let cur = unsafe { im::ptr_add_mut(g.v.ptr.as_ptr(), g.read) };
            if !f(unsafe { &mut *cur }) {
                g.read += 1;
                unsafe { im::drop_in_place(cur) };
            } else {
                unsafe { im::copy_nonoverlapping(cur as *const T, im::ptr_add_mut(g.v.ptr.as_ptr(), g.write), 1) };
                g.write += 1;
                g.read += 1;
            }
        }
        unsafe { g.v.set_len(g.write) };
        crate::mem::forget(g);
    }

    pub fn dedup_by_key<K: PartialEq, F: FnMut(&mut T) -> K>(&mut self, key: F) {
        let mut key = key;
        self.dedup_by(|a, b| key(a) == key(b))
    }

    /// Removes consecutive elements for which `same_bucket(current, previous_kept)` is true.
    /// Real alloc's algorithm: no writes until the first duplicate, and a gap guard that keeps
    /// the Vec valid (no double drop) if `same_bucket` or `T::drop` panics.
    pub fn dedup_by<F: FnMut(&mut T, &mut T) -> bool>(&mut self, same_bucket: F) {
        let mut same_bucket = same_bucket;
        let len = self.len;
        if len <= 1 {
            return;
        }
        let start = self.ptr.as_ptr();
        let mut first_dup = 1;
        while first_dup != len {
            let found = unsafe { same_bucket(&mut *im::ptr_add_mut(start, first_dup), &mut *im::ptr_add_mut(start, first_dup - 1)) };
            if found {
                break;
            }
            first_dup += 1;
        }
        if first_dup == len {
            return;
        }
        let mut gap = DedupGap { read: first_dup + 1, write: first_dup, vec: self };
        unsafe {
            im::drop_in_place(im::ptr_add_mut(start, first_dup));
            while gap.read < len {
                let read_ptr = im::ptr_add_mut(start, gap.read);
                let prev_ptr = im::ptr_add_mut(start, gap.write - 1);
                if same_bucket(&mut *read_ptr, &mut *prev_ptr) {
                    gap.read += 1;
                    im::drop_in_place(read_ptr);
                } else {
                    im::copy_nonoverlapping(read_ptr as *const T, im::ptr_add_mut(start, gap.write), 1);
                    gap.write += 1;
                    gap.read += 1;
                }
            }
            gap.vec.len = gap.write;
            crate::mem::forget(gap);
        }
    }

    pub fn push(&mut self, value: T) {
        // as upstream `push_mut`: the length read once does not change across `grow_one`
        let len = self.len;
        if len == self.capacity() {
            self.grow_one();
        }
        unsafe { im::ptr_write(im::ptr_add_mut(self.ptr.as_ptr(), len), value) };
        self.len = len + 1;
    }

    /// `reserve(1)` of a full vector, out of line as upstream `RawVec::grow_one`
    #[inline(never)]
    #[relax(keep_reachable)]
    fn grow_one(&mut self) {
        self.reserve(1);
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
            Some(unsafe { im::ptr_read(im::ptr_add(self.ptr.as_ptr() as *const T, self.len)) })
        }
    }

    pub fn pop_if<F: FnOnce(&mut T) -> bool>(&mut self, predicate: F) -> Option<T> {
        if self.len == 0 {
            return None;
        }
        let last = unsafe { &mut *im::ptr_add_mut(self.ptr.as_ptr(), self.len - 1) };
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
            im::copy_nonoverlapping(other.ptr.as_ptr() as *const T, im::ptr_add_mut(self.ptr.as_ptr(), self.len), count);
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

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
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
            im::copy_nonoverlapping(im::ptr_add(self.ptr.as_ptr() as *const T, at), other.ptr.as_ptr(), other_len);
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
        unsafe { im::slice_from_raw_mut(me.ptr.as_ptr(), me.len) }
    }

    pub fn spare_capacity_mut(&mut self) -> &mut [crate::mem::MaybeUninit<T>] {
        unsafe {
            im::slice_from_raw_mut(
                im::ptr_add_mut(self.ptr.as_ptr(), self.len) as *mut crate::mem::MaybeUninit<T>,
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
            self.extend_with(new_len - len, value);
        } else {
            self.truncate(new_len);
        }
    }

    /// Appends `n` copies of `value` (real alloc's extend_with): written through a raw pointer
    /// with the length kept in a local guard, the last one moved in. For T: Copy the block is
    /// filled by memset (1-byte T) or by doubling memcpys instead of one write per element.
    fn extend_with(&mut self, n: usize, value: T) {
        self.reserve(n);
        if n == 0 {
            return;
        }
        unsafe {
            let start = im::ptr_add_mut(self.ptr.as_ptr(), self.len);
            if im::is_copy::<T>() {
                if crate::mem::size_of::<T>() == 1 {
                    let byte: u8 = im::ptr_read(&value as *const T as *const u8);
                    im::write_bytes(start as *mut u8, byte, n);
                } else {
                    im::ptr_write(start, value);
                    let mut filled = 1;
                    while filled < n {
                        let chunk = if filled < n - filled { filled } else { n - filled };
                        im::copy_nonoverlapping(start as *const T, im::ptr_add_mut(start, filled), chunk);
                        filled += chunk;
                    }
                }
                self.len += n;
                return;
            }
            let mut guard = SetLenOnDrop { len: &mut self.len, local_len: 0 };
            let mut i = 0;
            while i + 1 < n {
                im::ptr_write(im::ptr_add_mut(start, i), value.clone());
                i += 1;
                guard.local_len = i;
            }
            im::ptr_write(im::ptr_add_mut(start, i), value);
            guard.local_len = n;
        }
    }

    pub fn extend_from_slice(&mut self, other: &[T]) {
        self.reserve(other.len());
        // T: Copy -- one memcpy (real std specializes Clone-for-Copy the same way)
        if unsafe { im::is_copy::<T>() } {
            unsafe { im::copy_nonoverlapping(im::slice_ptr(other), im::ptr_add_mut(self.ptr.as_ptr(), self.len), other.len()) };
            self.len += other.len();
            return;
        }
        let mut i = 0;
        while i < other.len() {
            unsafe { im::ptr_write(im::ptr_add_mut(self.ptr.as_ptr(), self.len), other[i].clone()) };
            self.len += 1;
            i += 1;
        }
    }

    pub fn extend_from_within<R: RangeBounds<usize>>(&mut self, src: R) {
        let (start, end) = crate::slice::range(&src, self.len);
        self.reserve(end - start);
        let mut i = start;
        while i < end {
            let v = unsafe { (*im::ptr_add(self.ptr.as_ptr() as *const T, i)).clone() };
            unsafe { im::ptr_write(im::ptr_add_mut(self.ptr.as_ptr(), self.len), v) };
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
    // real alloc's SpecFromElem + IsZero: a Copy value whose bytes are all zero becomes one
    // zeroed allocation (calloc: untouched pages stay unmapped until written)
    if n > 0 && !is_zst::<T>() && unsafe { im::is_copy::<T>() } && all_zero_bytes(&elem) {
        let layout = Vec::<T>::layout_for(n);
        if layout.size() > isize::MAX as usize {
            capacity_overflow();
        }
        let p = unsafe { heap::alloc_zeroed(layout) };
        if p.is_null() {
            heap::handle_alloc_error(layout);
        }
        return Vec { ptr: unsafe { crate::ptr::NonNull::new_unchecked(p as *mut T) }, cap: unsafe { Cap::new::<T>(n) }, len: n, _marker: crate::marker::PhantomData };
    }
    let mut v = Vec::with_capacity(n);
    v.extend_with(n, elem);
    // extend_with wrote all n elements (a panicking clone never gets here): the caller sees the
    // length n, not a load after the call, so its bounds checks against `n` fold (LLVM inlines
    // extend_with and knows the same)
    v.len = n;
    v
}

fn all_zero_bytes<T>(v: &T) -> bool {
    let n = crate::mem::size_of::<T>();
    let p = v as *const T as *const u8;
    // one compare of the value's bits for scalar sizes, as real alloc's IsZero impls do
    // (`self.to_bits() == 0`, `*self == 0`): a constant element folds the whole test
    unsafe {
        match n {
            1 => return im::ptr_read(p) == 0,
            2 => return crate::ptr::read_unaligned(p as *const u16) == 0,
            4 => return crate::ptr::read_unaligned(p as *const u32) == 0,
            8 => return crate::ptr::read_unaligned(p as *const u64) == 0,
            16 => return crate::ptr::read_unaligned(p as *const u128) == 0,
            _ => {}
        }
    }
    let mut i = 0;
    while i < n {
        if unsafe { im::ptr_read(im::ptr_add(p, i)) } != 0 {
            return false;
        }
        i += 1;
    }
    true
}

/// extend_with's length guard (real alloc's SetLenOnDrop): the elements written so far count
/// even if a clone panics.
struct SetLenOnDrop<'a> {
    len: &'a mut usize,
    local_len: usize,
}

impl<'a> Drop for SetLenOnDrop<'a> {
    fn drop(&mut self) {
        *self.len += self.local_len;
    }
}

unsafe impl<#[may_dangle] T> Drop for Vec<T> {
    fn drop(&mut self) {
        unsafe {
            if crate::mem::needs_drop::<T>() {
                // one drop_in_place of the slice: an element whose drop panics does not stop
                // the others (real alloc drops the slice the same way)
                let s = im::slice_from_raw_mut(self.ptr.as_ptr(), self.len) as *mut [T];
                im::drop_in_place(s);
            }
        }
        if !is_zst::<T>() && self.cap.get() != 0 {
            heap::free(self.ptr.as_ptr() as *mut u8, Vec::<T>::layout_for(self.cap.get()));
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

impl<T, U, const N: usize> PartialEq<[U; N]> for Vec<T>
where
    T: PartialEq<U>,
{
    fn eq(&self, other: &[U; N]) -> bool {
        crate::slice::slice_eq(self.as_slice(), &other[..])
    }
}

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

#[cfg(not(relax_check))]
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

#[cfg(not(relax_check))]
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
impl<T, const N: usize> From<[T; N]> for Vec<T> {
    fn from(s: [T; N]) -> Vec<T> {
        let mut v = Vec::with_capacity(N);
        let s = crate::mem::ManuallyDrop::new(s);
        unsafe {
            im::copy_nonoverlapping(&*s as *const [T; N] as *const T, v.ptr.as_ptr(), N);
            v.len = N;
        }
        v
    }
}
impl<'a, T: Clone, const N: usize> From<&'a [T; N]> for Vec<T> {
    fn from(s: &'a [T; N]) -> Vec<T> {
        crate::slice::to_vec(&s[..])
    }
}

impl<'a, T: Clone, const N: usize> From<&'a mut [T; N]> for Vec<T> {
    fn from(s: &'a mut [T; N]) -> Vec<T> {
        let mut v = Vec::with_capacity(N);
        v.extend_from_slice(&s[..]);
        v
    }
}
#[cfg(not(relax_check))]
impl<'a, T: Clone> From<Cow<'a, [T]>> for Vec<T> {
    fn from(s: Cow<'a, [T]>) -> Vec<T> {
        s.into_owned()
    }
}
#[cfg(not(relax_check))]
impl<T> From<crate::boxed::Box<[T]>> for Vec<T> {
    fn from(s: crate::boxed::Box<[T]>) -> Vec<T> {
        s.into_vec()
    }
}
#[cfg(not(relax_check))]
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
#[cfg(not(relax_check))]
impl<'a, T: Clone> From<&'a Vec<T>> for Cow<'a, [T]> {
    fn from(v: &'a Vec<T>) -> Cow<'a, [T]> {
        Cow::Borrowed(v.as_slice())
    }
}
#[cfg(not(relax_check))]
impl<'a, T: Clone> From<Vec<T>> for Cow<'a, [T]> {
    fn from(v: Vec<T>) -> Cow<'a, [T]> {
        Cow::Owned(v)
    }
}
#[cfg(not(relax_check))]
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

impl<T> Vec<T> {
    /// Real alloc's extend_trusted, for an iterator that reported exactly `n` items: one reserve,
    /// then writes through a raw pointer with the length in a local (no capacity check per item).
    /// Unlike TrustedLen nothing is trusted: a short iterator just ends early, and items beyond
    /// `n` go through the caller's checked loop.
    fn extend_counted<I: Iterator<Item = T>>(&mut self, iter: &mut I, n: usize) {
        self.reserve(n);
        let dst = unsafe { im::ptr_add_mut(self.ptr.as_ptr(), self.len) };
        let mut guard = SetLenOnDrop { len: &mut self.len, local_len: 0 };
        while guard.local_len < n {
            match iter.next() {
                Some(x) => {
                    unsafe { im::ptr_write(im::ptr_add_mut(dst, guard.local_len), x) };
                    guard.local_len += 1;
                }
                None => break,
            }
        }
    }
}

/// Whether `I` is `slice::Iter<'_, T>` (real alloc reaches extend_from_slice for it through
/// specialization; here by type identity, lifetimes erased). The rustc shim has no lifetime-free
/// type id and always says no (the generic path is checked there).
fn is_slice_iter<I, T>() -> bool {
    #[cfg(not(relax_check))]
    {
        unsafe { im::type_id::<I>() == im::type_id::<crate::slice::Iter<'static, T>>() }
    }
    #[cfg(relax_check)]
    {
        false
    }
}

impl<T> Extend<T> for Vec<T> {
    fn extend<I: IntoIterator<Item = T>>(&mut self, iter: I) {
        let mut iter = iter.into_iter();
        let (lower, upper) = iter.size_hint();
        if upper == Some(lower) {
            self.extend_counted(&mut iter, lower);
        } else {
            self.reserve(lower);
        }
        while let Some(x) = iter.next() {
            if self.len == self.capacity() {
                let (lower, _) = iter.size_hint();
                self.reserve(lower.saturating_add(1));
            }
            unsafe { im::ptr_write(im::ptr_add_mut(self.ptr.as_ptr(), self.len), x) };
            self.len += 1;
        }
    }
    #[cfg(not(relax_check))]
    fn extend_one(&mut self, item: T) {
        self.push(item);
    }
    #[cfg(not(relax_check))]
    fn extend_reserve(&mut self, additional: usize) {
        self.reserve(additional);
    }
}

impl<'a, T: Copy + 'a> Extend<&'a T> for Vec<T> {
    fn extend<I: IntoIterator<Item = &'a T>>(&mut self, iter: I) {
        let mut iter = iter.into_iter();
        if is_slice_iter::<I::IntoIter, T>() {
            // one memcpy, as real alloc's SpecExtend<&T, slice::Iter<T>>
            let it: crate::slice::Iter<'a, T> = unsafe { crate::mem::transmute_copy(&iter) };
            crate::mem::forget(iter);
            self.extend_from_slice(it.as_slice());
            return;
        }
        let (lower, upper) = iter.size_hint();
        if upper == Some(lower) {
            let mut copied = CopiedRefs { it: &mut iter };
            self.extend_counted(&mut copied, lower);
        } else {
            self.reserve(lower);
        }
        while let Some(x) = iter.next() {
            self.push(*x);
        }
    }
    #[cfg(not(relax_check))]
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

impl<T, const N: usize> TryFrom<Vec<T>> for [T; N] {
    type Error = Vec<T>;
    fn try_from(vec: Vec<T>) -> Result<[T; N], Vec<T>> {
        if vec.len() != N {
            return Err(vec);
        }
        let me = crate::mem::ManuallyDrop::new(vec);
        let arr = unsafe { im::ptr_read(me.ptr.as_ptr() as *const [T; N]) };
        if !is_zst::<T>() && me.cap.get() != 0 {
            heap::free(me.ptr.as_ptr() as *mut u8, Vec::<T>::layout_for(me.cap.get()));
        }
        Ok(arr)
    }
}

/// dedup_by's panic guard (real alloc's FillGapOnDrop): moves the unchecked tail `[read..len)` down
/// over the dropped gap `[write..read)` and shortens the Vec.
struct DedupGap<'a, T> {
    read: usize,
    write: usize,
    vec: &'a mut Vec<T>,
}

impl<'a, T> Drop for DedupGap<'a, T> {
    fn drop(&mut self) {
        unsafe {
            let ptr = self.vec.ptr.as_ptr();
            let len = self.vec.len;
            let left = len - self.read;
            im::copy(im::ptr_add_mut(ptr, self.read) as *const T, im::ptr_add_mut(ptr, self.write), left);
            self.vec.len = len - (self.read - self.write);
        }
    }
}

/// `*x` for each `&T` of the borrowed iterator (Extend<&T> feeding extend_counted).
struct CopiedRefs<'b, I> {
    it: &'b mut I,
}

impl<'a, 'b, T: Copy + 'a, I: Iterator<Item = &'a T>> Iterator for CopiedRefs<'b, I> {
    type Item = T;
    fn next(&mut self) -> Option<T> {
        match self.it.next() {
            Some(x) => Some(*x),
            None => None,
        }
    }
}

impl<T, const N: usize> Vec<[T; N]> {
    /// The arrays' elements as one Vec, in place (real alloc's into_flattened).
    pub fn into_flattened(self) -> Vec<T> {
        let (ptr, len, cap) = self.into_raw_parts();
        let (new_len, new_cap) = if crate::mem::size_of::<T>() == 0 {
            match len.checked_mul(N) {
                Some(l) => (l, usize::MAX),
                None => crate::panicking::panic_str("vec len overflow"),
            }
        } else {
            (len * N, cap * N)
        };
        unsafe { Vec::from_raw_parts(ptr as *mut T, new_len, new_cap) }
    }
}
