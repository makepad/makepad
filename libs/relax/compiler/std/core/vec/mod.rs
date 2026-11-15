//! Vec<T, A>: a contiguous growable array (alloc::vec) in allocator A, Global unless one is named
//! (rustc 1.100's allocator_api; library/alloc/src/vec/mod.rs @1.100.0). Its allocator is its last
//! field. Constructors without an allocator, Default, FromIterator and the conversions from
//! slices, arrays, str and Cow are the global allocator's, as library/'s are; the rest takes any
//! allocator.

mod drain;
mod extract_if;
mod into_iter;
mod splice;

pub use drain::Drain;
pub use extract_if::ExtractIf;
pub use into_iter::IntoIter;
pub use splice::Splice;

use crate::borrow::{Borrow, BorrowMut, Cow, ToOwned};
use crate::boxed::Box;
use crate::cmp::Ordering;
use crate::collections::TryReserveError;
use crate::fmt;
use crate::hash::{Hash, Hasher};
use crate::heap::{self, Allocator, AllocatorNightly, Global, Layout};
use crate::intrinsics_mem as im;
use crate::marker::PhantomData;
use crate::mem::ManuallyDrop;
use crate::ops::{Deref, DerefMut, Index, IndexMut, RangeBounds};
use crate::ptr::NonNull;
use crate::slice::SliceIndex;

/// RawVec's stored capacity: the high bit stays unused, including for ZSTs.
#[derive(Clone, Copy)]
#[repr(transparent)]
#[cfg_attr(target_pointer_width = "64", rustc_layout_scalar_valid_range_end(0x7fff_ffff_ffff_ffff))]
#[cfg_attr(target_pointer_width = "32", rustc_layout_scalar_valid_range_end(0x7fff_ffff))]
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
pub struct Vec<T, A: Allocator = Global> {
    ptr: NonNull<T>,
    cap: Cap,
    len: usize,
    _marker: PhantomData<T>,
    alloc: A,
}

unsafe impl<T: Send, A: Allocator + Send> Send for Vec<T, A> {}
unsafe impl<T: Sync, A: Allocator + Sync> Sync for Vec<T, A> {}

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

#[inline(never)]
#[cold]
const fn capacity_overflow() -> ! {
    crate::panicking::panic_str("capacity overflow")
}

// Real alloc's RawVecInner: the element-independent half of a Vec's buffer (its growth, the
// layout arithmetic and the allocator calls) is one body for every element type, the element's
// layout passed as a value, so the `Vec<T>` instances keep only a capacity compare and a call
// (a game's wasm had 694 `try_grow_to`, 88 `try_reserve`, 875 `do_reserve_and_handle` copies).
// They are generic over the allocator only, as RawVecInner<A> is.

/// `n` elements of `elem` (Layout::array).
fn raw_array(elem: Layout, n: usize) -> Result<Layout, TryReserveError> {
    match elem.size().checked_mul(n) {
        Some(size) => Layout::from_size_align(size, elem.align()).map_err(|_| TryReserveError::capacity_overflow()),
        None => Err(TryReserveError::capacity_overflow()),
    }
}

/// Reallocates the buffer to exactly `new_cap` elements of `elem` (not a ZST) in `alloc`
/// (RawVecInner::finish_grow): the allocator's error is an AllocError of the new layout.
#[inline(never)]
fn raw_grow_to<A: Allocator>(ptr: &mut NonNull<u8>, cap: &mut Cap, new_cap: usize, elem: Layout, alloc: &A) -> Result<(), TryReserveError> {
    let new_layout = raw_array(elem, new_cap)?;
    let memory = if cap.get() == 0 {
        alloc.allocate(new_layout)
    } else {
        // the current allocation's layout was valid when it was made
        let old = unsafe { Layout::from_size_align_unchecked(elem.size() * cap.get(), elem.align()) };
        unsafe { alloc.grow(*ptr, old, new_layout) }
    };
    match memory {
        Ok(p) => {
            *ptr = p.cast::<u8>();
            // raw_array checked that the allocation fits in isize::MAX bytes (a non-ZST: u8 stands in)
            *cap = unsafe { Cap::new::<u8>(new_cap) };
            Ok(())
        }
        Err(_) => Err(TryReserveError::alloc_error(new_layout)),
    }
}

/// RawVec's handle_error: a capacity overflow panics, an allocator's error is
/// handle_alloc_error.
#[cold]
#[inline(never)]
fn handle_error(e: TryReserveError) -> ! {
    match e.kind() {
        crate::collections::TryReserveErrorKind::AllocError { layout, .. } => heap::handle_alloc_error(layout),
        crate::collections::TryReserveErrorKind::CapacityOverflow => capacity_overflow(),
    }
}

/// Room for `additional` more after `len`, growing amortized (doubling, at least the minimum
/// non-zero capacity), as RawVecInner::grow_amortized.
#[inline(never)]
fn raw_reserve<A: Allocator>(ptr: &mut NonNull<u8>, cap: &mut Cap, len: usize, additional: usize, elem: Layout, alloc: &A) -> Result<(), TryReserveError> {
    if elem.size() == 0 {
        // a ZST's capacity is usize::MAX already
        return Err(TryReserveError::capacity_overflow());
    }
    let required = match len.checked_add(additional) {
        Some(r) => r,
        None => return Err(TryReserveError::capacity_overflow()),
    };
    let mut new_cap = cap.get() * 2;
    if new_cap < required {
        new_cap = required;
    }
    let min = min_non_zero_cap(elem.size());
    if new_cap < min {
        new_cap = min;
    }
    raw_grow_to(ptr, cap, new_cap, elem, alloc)
}

/// raw_reserve that aborts on an error (RawVecInner's do_reserve_and_handle).
#[cold]
#[inline(never)]
fn raw_reserve_or_abort<A: Allocator>(ptr: &mut NonNull<u8>, cap: &mut Cap, len: usize, additional: usize, elem: Layout, alloc: &A) {
    if let Err(e) = raw_reserve(ptr, cap, len, additional, elem, alloc) {
        handle_error(e);
    }
}

impl<T> Vec<T> {
    pub const fn new() -> Vec<T> {
        Vec { ptr: NonNull::dangling(), cap: Cap::ZERO, len: 0, _marker: PhantomData, alloc: Global }
    }

    pub const fn with_capacity(capacity: usize) -> Vec<T> {
        let mut v = Vec::new();
        if capacity > 0 && !is_zst::<T>() {
            v.grow_to(capacity);
        }
        v
    }

    /// rustc 1.100.0 (vec_from_fn): `[f(0), f(1), .., f(length - 1)]`
    pub fn from_fn<F>(length: usize, f: F) -> Self
    where
        F: FnMut(usize) -> T,
    {
        (0..length).map(f).collect()
    }

    /// # Safety: `ptr` must be non-null and aligned, with `length` initialized elements.
    /// For non-ZSTs it must own an allocation for `capacity` elements, fitting in
    /// isize::MAX bytes, and `length` must not exceed `capacity`.
    pub const unsafe fn from_raw_parts(ptr: *mut T, length: usize, capacity: usize) -> Vec<T> {
        Vec { ptr: NonNull::new_unchecked(ptr), cap: Cap::new::<T>(capacity), len: length, _marker: PhantomData, alloc: Global }
    }

    pub const fn into_raw_parts(self) -> (*mut T, usize, usize) {
        let me = ManuallyDrop::new(self);
        (me.ptr.as_ptr(), me.len, me.capacity())
    }

    /// # Safety: as from_raw_parts (library/'s from_parts, box_vec_non_null 1.99).
    pub const unsafe fn from_parts(ptr: NonNull<T>, length: usize, capacity: usize) -> Vec<T> {
        Self::from_parts_in(ptr, length, capacity, Global)
    }

    pub const fn into_parts(self) -> (NonNull<T>, usize, usize) {
        let (ptr, len, capacity) = self.into_raw_parts();
        // a Vec's pointer is never null
        (unsafe { NonNull::new_unchecked(ptr) }, len, capacity)
    }

    fn layout_for(cap: usize) -> Layout {
        match Layout::array::<T>(cap) {
            Ok(l) => l,
            Err(_) => capacity_overflow(),
        }
    }

    /// The layout of an existing buffer of `cap` elements (real alloc's RawVecInner::current_memory:
    /// the buffer was allocated with it, so its size cannot overflow -- no check, no panic path)
    pub(crate) fn current_layout(cap: usize) -> Layout {
        unsafe { Layout::from_size_align_unchecked(cap.wrapping_mul(crate::mem::size_of::<T>()), crate::mem::align_of::<T>()) }
    }
}

impl<T, A: Allocator> Vec<T, A> {
    pub const fn new_in(alloc: A) -> Vec<T, A> {
        Vec { ptr: NonNull::dangling(), cap: Cap::ZERO, len: 0, _marker: PhantomData, alloc }
    }

    pub fn with_capacity_in(capacity: usize, alloc: A) -> Vec<T, A> {
        let mut v = Vec::new_in(alloc);
        if capacity > 0 && !is_zst::<T>() {
            v.grow_to(capacity);
        }
        v
    }

    /// # Safety: as from_raw_parts, the buffer allocated by `alloc`.
    pub const unsafe fn from_raw_parts_in(ptr: *mut T, length: usize, capacity: usize, alloc: A) -> Vec<T, A> {
        Vec { ptr: NonNull::new_unchecked(ptr), cap: Cap::new::<T>(capacity), len: length, _marker: PhantomData, alloc }
    }

    /// The pointer, length, capacity and allocator, the Vec given up (its buffer is the caller's).
    pub fn into_raw_parts_with_allocator(self) -> (*mut T, usize, usize, A) {
        let me = ManuallyDrop::new(self);
        let alloc = unsafe { im::ptr_read(&me.alloc as *const A) };
        (me.ptr.as_ptr(), me.len, me.capacity(), alloc)
    }

    /// # Safety: as from_raw_parts_in.
    pub const unsafe fn from_parts_in(ptr: NonNull<T>, length: usize, capacity: usize, alloc: A) -> Vec<T, A> {
        Vec { ptr, cap: Cap::new::<T>(capacity), len: length, _marker: PhantomData, alloc }
    }

    pub fn into_parts_with_allocator(self) -> (NonNull<T>, usize, usize, A) {
        let (ptr, len, capacity, alloc) = self.into_raw_parts_with_allocator();
        // a Vec's pointer is never null
        (unsafe { NonNull::new_unchecked(ptr) }, len, capacity, alloc)
    }

    pub const fn allocator(&self) -> &A {
        &self.alloc
    }

    pub const fn capacity(&self) -> usize {
        if is_zst::<T>() {
            usize::MAX
        } else {
            self.cap.get()
        }
    }

    /// Reallocates to exactly `new_cap` (>= len). Not for ZSTs.
    fn grow_to(&mut self, new_cap: usize) {
        if let Err(e) = self.try_grow_to(new_cap) {
            handle_error(e);
        }
    }

    fn try_grow_to(&mut self, new_cap: usize) -> Result<(), TryReserveError> {
        let (ptr, cap, alloc) = self.raw_parts_mut();
        raw_grow_to(ptr, cap, new_cap, Layout::new::<T>(), alloc)
    }

    /// The buffer's pointer (as bytes), its capacity and the allocator, for the
    /// element-independent growth.
    fn raw_parts_mut(&mut self) -> (&mut NonNull<u8>, &mut Cap, &A) {
        // NonNull<T> and NonNull<u8> are one pointer
        (unsafe { &mut *(&mut self.ptr as *mut NonNull<T>).cast::<NonNull<u8>>() }, &mut self.cap, &self.alloc)
    }

    /// Real alloc's RawVecInner::reserve: only the capacity check inline, the growth and its
    /// error handling behind a cold call (do_reserve_and_handle), so a caller that grows rarely
    /// (`DrawVars::fit_dyn_uniforms` on every `set_uniform`) inlines a compare and a call.
    #[inline]
    pub fn reserve(&mut self, additional: usize) {
        if additional > self.capacity().wrapping_sub(self.len) {
            self.do_reserve_and_handle(additional);
        }
    }

    #[cold]
    fn do_reserve_and_handle(&mut self, additional: usize) {
        let len = self.len;
        let (ptr, cap, alloc) = self.raw_parts_mut();
        raw_reserve_or_abort(ptr, cap, len, additional, Layout::new::<T>(), alloc);
    }

    /// Room for `additional` more after `len` elements, `len` past this Vec's length (Splice's
    /// tail move: RawVec::reserve with Drain's length).
    fn reserve_after(&mut self, len: usize, additional: usize) {
        if additional > self.capacity().wrapping_sub(len) {
            let (ptr, cap, alloc) = self.raw_parts_mut();
            raw_reserve_or_abort(ptr, cap, len, additional, Layout::new::<T>(), alloc);
        }
    }

    pub fn reserve_exact(&mut self, additional: usize) {
        if let Err(e) = self.try_reserve_exact(additional) {
            handle_error(e);
        }
    }

    pub fn try_reserve(&mut self, additional: usize) -> Result<(), TryReserveError> {
        if self.capacity() - self.len >= additional {
            return Ok(());
        }
        let len = self.len;
        let (ptr, cap, alloc) = self.raw_parts_mut();
        raw_reserve(ptr, cap, len, additional, Layout::new::<T>(), alloc)
    }

    pub fn try_reserve_exact(&mut self, additional: usize) -> Result<(), TryReserveError> {
        if self.capacity() - self.len >= additional {
            return Ok(());
        }
        let required = match self.len.checked_add(additional) {
            Some(r) => r,
            None => return Err(TryReserveError::capacity_overflow()),
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
        let old = Vec::<T>::current_layout(self.cap.get());
        if target == 0 {
            unsafe { self.alloc.deallocate(self.ptr.cast::<u8>(), old) };
            self.ptr = NonNull::dangling();
            self.cap = Cap::ZERO;
        } else {
            // RawVecInner::shrink_unchecked: a smaller size of the same element layout
            let new_layout = unsafe { Layout::from_size_align_unchecked(crate::mem::size_of::<T>() * target, old.align()) };
            match unsafe { self.alloc.shrink(self.ptr.cast::<u8>(), old, new_layout) } {
                Ok(p) => {
                    self.ptr = p.cast::<T>();
                    self.cap = unsafe { Cap::new::<T>(target) };
                }
                Err(_) => heap::handle_alloc_error(new_layout),
            }
        }
    }

    pub fn into_boxed_slice(self) -> Box<[T], A> {
        let mut me = self;
        me.shrink_to_fit();
        let me = ManuallyDrop::new(me);
        let alloc = unsafe { im::ptr_read(&me.alloc as *const A) };
        unsafe { Box::from_raw_in(im::slice_from_raw_mut(me.ptr.as_ptr(), me.len) as *mut [T], alloc) }
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

    /// library/'s insert_mut (push_mut, 1.95): insert, then the inserted element.
    #[track_caller]
    pub fn insert_mut(&mut self, index: usize, element: T) -> &mut T {
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
            self.len = len + 1;
            &mut *p
        }
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
        struct PanicGuard<'a, T, A: Allocator> {
            v: &'a mut Vec<T, A>,
            read: usize,
            write: usize,
            original_len: usize,
        }
        impl<T, A: Allocator> Drop for PanicGuard<'_, T, A> {
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

    pub fn push_mut(&mut self, value: T) -> &mut T {
        self.push(value);
        let idx = self.len - 1;
        unsafe { &mut *im::ptr_add_mut(self.ptr.as_ptr(), idx) }
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

    pub fn append(&mut self, other: &mut Self) {
        let count = other.len;
        self.reserve(count);
        unsafe {
            im::copy_nonoverlapping(other.ptr.as_ptr() as *const T, im::ptr_add_mut(self.ptr.as_ptr(), self.len), count);
            other.len = 0;
        }
        self.len += count;
    }

    pub fn drain<R: RangeBounds<usize>>(&mut self, range: R) -> Drain<'_, T, A>
    where
        A: AllocatorNightly,
    {
        let (start, end) = crate::slice::range(&range, self.len);
        drain::new(self, start, end)
    }

    /// library/alloc/src/vec/mod.rs `Vec::extract_if` (stable 1.87): removes and yields the
    /// elements of `range` the filter selects.
    pub fn extract_if<F, R>(&mut self, range: R, filter: F) -> ExtractIf<'_, T, F, A>
    where
        A: AllocatorNightly,
        F: FnMut(&mut T) -> bool,
        R: RangeBounds<usize>,
    {
        let (start, end) = crate::slice::range(&range, self.len);
        extract_if::new(self, filter, start, end)
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
    pub fn split_off(&mut self, at: usize) -> Self
    where
        A: Clone,
    {
        if at > self.len {
            crate::panicking::panic_fmt(format_args!("`at` split index (is {}) should be <= len (is {})", at, self.len));
        }
        let other_len = self.len - at;
        let mut other = Vec::with_capacity_in(other_len, self.alloc.clone());
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

    pub fn leak<'a>(self) -> &'a mut [T]
    where
        A: 'a,
    {
        let me = ManuallyDrop::new(self);
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

    /// Replaces `range` with the items of `replace_with`: a Drain of the range whose drop puts
    /// them in (library/alloc/src/vec/splice.rs).
    pub fn splice<R: RangeBounds<usize>, I: IntoIterator<Item = T>>(&mut self, range: R, replace_with: I) -> Splice<'_, I::IntoIter, A>
    where
        A: AllocatorNightly,
    {
        Splice { drain: self.drain(range), replace_with: replace_with.into_iter() }
    }
}

impl<T: Clone, A: Allocator> Vec<T, A> {
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
                if im::is_primitive_scalar::<T>() && crate::mem::size_of::<T>() == 1 {
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

impl<T: PartialEq, A: Allocator> Vec<T, A> {
    pub fn dedup(&mut self) {
        self.dedup_by(|a, b| a == b)
    }
}

/// `vec![elem; n]`
pub fn from_elem<T: Clone>(elem: T, n: usize) -> Vec<T> {
    // Like real alloc's scalar IsZero implementations. Copy alone does not make a byte
    // scan valid: aggregates can have uninitialized padding or inactive enum storage.
    if n > 0 && primitive_is_zero(&elem) {
        let layout = Vec::<T>::layout_for(n);
        if layout.size() > isize::MAX as usize {
            capacity_overflow();
        }
        let p = unsafe { heap::alloc_zeroed(layout) };
        if p.is_null() {
            heap::handle_alloc_error(layout);
        }
        return Vec { ptr: unsafe { NonNull::new_unchecked(p as *mut T) }, cap: unsafe { Cap::new::<T>(n) }, len: n, _marker: PhantomData, alloc: Global };
    }
    let mut v = Vec::with_capacity(n);
    v.extend_with(n, elem);
    // extend_with wrote all n elements (a panicking clone never gets here): the caller sees the
    // length n, not a load after the call, so its bounds checks against `n` fold (LLVM inlines
    // extend_with and knows the same)
    v.len = n;
    v
}

fn primitive_is_zero<T>(v: &T) -> bool {
    if !unsafe { im::is_primitive_scalar::<T>() } {
        return false;
    }
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
            _ => return false,
        }
    }
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

unsafe impl<#[may_dangle] T, A: Allocator> Drop for Vec<T, A> {
    fn drop(&mut self) {
        // library/alloc/src/vec/mod.rs: Vec::drop leaves deallocation to RawVec's
        // field drop, including on unwind. A zero-sized layout has no allocation.
        let _guard = heap::DeallocGuard { ptr: self.ptr.cast::<u8>(), layout: Vec::<T>::current_layout(self.cap.get()), alloc: &self.alloc };
        unsafe {
            if crate::mem::needs_drop::<T>() {
                // one drop_in_place of the slice: an element whose drop panics does not stop
                // the others (real alloc drops the slice the same way)
                let s = im::slice_from_raw_mut(self.ptr.as_ptr(), self.len) as *mut [T];
                im::drop_in_place(s);
            }
        }
    }
}

impl<T, A: Allocator> Deref for Vec<T, A> {
    type Target = [T];
    fn deref(&self) -> &[T] {
        self.as_slice()
    }
}

impl<T, A: Allocator> DerefMut for Vec<T, A> {
    fn deref_mut(&mut self) -> &mut [T] {
        self.as_mut_slice()
    }
}

impl<T, I: SliceIndex<[T]>, A: Allocator> Index<I> for Vec<T, A> {
    type Output = I::Output;
    #[track_caller]
    fn index(&self, index: I) -> &I::Output {
        index.index(self.as_slice())
    }
}

impl<T, I: SliceIndex<[T]>, A: Allocator> IndexMut<I> for Vec<T, A> {
    #[track_caller]
    fn index_mut(&mut self, index: I) -> &mut I::Output {
        index.index_mut(self.as_mut_slice())
    }
}

impl<T: Clone, A: Allocator + Clone> Clone for Vec<T, A> {
    fn clone(&self) -> Self {
        let mut v = Vec::with_capacity_in(self.len, self.alloc.clone());
        v.extend_from_slice(self.as_slice());
        v
    }
    fn clone_from(&mut self, source: &Self) {
        self.clear();
        self.extend_from_slice(source.as_slice());
    }
}

impl<T> Default for Vec<T> {
    fn default() -> Vec<T> {
        Vec::new()
    }
}

impl<T: fmt::Debug, A: Allocator> fmt::Debug for Vec<T, A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(self.as_slice(), f)
    }
}

impl<T: Hash, A: Allocator> Hash for Vec<T, A> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        Hash::hash(self.as_slice(), state)
    }
}

impl<T, U, A1: Allocator, A2: Allocator> PartialEq<Vec<U, A2>> for Vec<T, A1>
where
    T: PartialEq<U>,
{
    fn eq(&self, other: &Vec<U, A2>) -> bool {
        crate::slice::slice_eq(self.as_slice(), other.as_slice())
    }
}

impl<T, U, A: Allocator> PartialEq<[U]> for Vec<T, A>
where
    T: PartialEq<U>,
{
    fn eq(&self, other: &[U]) -> bool {
        crate::slice::slice_eq(self.as_slice(), other)
    }
}

impl<'a, T, U, A: Allocator> PartialEq<&'a [U]> for Vec<T, A>
where
    T: PartialEq<U>,
{
    fn eq(&self, other: &&'a [U]) -> bool {
        crate::slice::slice_eq(self.as_slice(), *other)
    }
}

impl<'a, T, U, A: Allocator> PartialEq<&'a mut [U]> for Vec<T, A>
where
    T: PartialEq<U>,
{
    fn eq(&self, other: &&'a mut [U]) -> bool {
        crate::slice::slice_eq(self.as_slice(), &**other)
    }
}

impl<T, U, A: Allocator, const N: usize> PartialEq<[U; N]> for Vec<T, A>
where
    T: PartialEq<U>,
{
    fn eq(&self, other: &[U; N]) -> bool {
        crate::slice::slice_eq(self.as_slice(), &other[..])
    }
}

impl<'a, T, U, A: Allocator, const N: usize> PartialEq<&'a [U; N]> for Vec<T, A>
where
    T: PartialEq<U>,
{
    fn eq(&self, other: &&'a [U; N]) -> bool {
        crate::slice::slice_eq(self.as_slice(), &other[..])
    }
}

impl<T, U, A: Allocator> PartialEq<Vec<U, A>> for [T]
where
    T: PartialEq<U>,
{
    fn eq(&self, other: &Vec<U, A>) -> bool {
        crate::slice::slice_eq(self, other.as_slice())
    }
}

impl<'a, T, U, A: Allocator> PartialEq<Vec<U, A>> for &'a [T]
where
    T: PartialEq<U>,
{
    fn eq(&self, other: &Vec<U, A>) -> bool {
        crate::slice::slice_eq(*self, other.as_slice())
    }
}

impl<'a, T, U, A: Allocator> PartialEq<Vec<U, A>> for &'a mut [T]
where
    T: PartialEq<U>,
{
    fn eq(&self, other: &Vec<U, A>) -> bool {
        crate::slice::slice_eq(&**self, other.as_slice())
    }
}

impl<'a, T: Clone, U, A: Allocator> PartialEq<Vec<U, A>> for Cow<'a, [T]>
where
    T: PartialEq<U>,
{
    fn eq(&self, other: &Vec<U, A>) -> bool {
        crate::slice::slice_eq(&**self, other.as_slice())
    }
}

// alloc vec/partial_eq.rs: a borrowed or owned slice compares with slice references too
impl<'a, 'b, T: Clone, U> PartialEq<&'b [U]> for Cow<'a, [T]>
where
    T: PartialEq<U>,
{
    fn eq(&self, other: &&'b [U]) -> bool {
        crate::slice::slice_eq(&**self, *other)
    }
}

impl<'a, 'b, T: Clone, U> PartialEq<&'b mut [U]> for Cow<'a, [T]>
where
    T: PartialEq<U>,
{
    fn eq(&self, other: &&'b mut [U]) -> bool {
        crate::slice::slice_eq(&**self, &**other)
    }
}

// rustc 1.100.0 (partialeq_cow_for_vec_and_slice): a vector or slice reference compares with a borrowed or
// owned slice
impl<'a, T, U: Clone, A: Allocator> PartialEq<Cow<'a, [U]>> for Vec<T, A>
where
    T: PartialEq<U>,
{
    fn eq(&self, other: &Cow<'a, [U]>) -> bool {
        crate::slice::slice_eq(self.as_slice(), &**other)
    }
}

impl<'a, 'b, T, U: Clone> PartialEq<Cow<'a, [U]>> for &'b [T]
where
    T: PartialEq<U>,
{
    fn eq(&self, other: &Cow<'a, [U]>) -> bool {
        crate::slice::slice_eq(*self, &**other)
    }
}

impl<'a, 'b, T, U: Clone> PartialEq<Cow<'a, [U]>> for &'b mut [T]
where
    T: PartialEq<U>,
{
    fn eq(&self, other: &Cow<'a, [U]>) -> bool {
        crate::slice::slice_eq(&**self, &**other)
    }
}

impl<T: Eq, A: Allocator> Eq for Vec<T, A> {}

impl<T: PartialOrd, A1: Allocator, A2: Allocator> PartialOrd<Vec<T, A2>> for Vec<T, A1> {
    fn partial_cmp(&self, other: &Vec<T, A2>) -> Option<Ordering> {
        crate::slice::slice_partial_cmp(self.as_slice(), other.as_slice())
    }
}

impl<T: Ord, A: Allocator> Ord for Vec<T, A> {
    fn cmp(&self, other: &Self) -> Ordering {
        crate::slice::slice_cmp(self.as_slice(), other.as_slice())
    }
}

impl<T, A: Allocator> AsRef<Vec<T, A>> for Vec<T, A> {
    fn as_ref(&self) -> &Vec<T, A> {
        self
    }
}
impl<T, A: Allocator> AsMut<Vec<T, A>> for Vec<T, A> {
    fn as_mut(&mut self) -> &mut Vec<T, A> {
        self
    }
}
impl<T, A: Allocator> AsRef<[T]> for Vec<T, A> {
    fn as_ref(&self) -> &[T] {
        self
    }
}
impl<T, A: Allocator> AsMut<[T]> for Vec<T, A> {
    fn as_mut(&mut self) -> &mut [T] {
        self
    }
}
impl<T, A: Allocator> Borrow<[T]> for Vec<T, A> {
    fn borrow(&self) -> &[T] {
        &self[..]
    }
}
impl<T, A: Allocator> BorrowMut<[T]> for Vec<T, A> {
    fn borrow_mut(&mut self) -> &mut [T] {
        &mut self[..]
    }
}

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
        let s = ManuallyDrop::new(s);
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
impl<'a, T: Clone> From<Cow<'a, [T]>> for Vec<T> {
    fn from(s: Cow<'a, [T]>) -> Vec<T> {
        s.into_owned()
    }
}
impl<T, A: Allocator> From<Box<[T], A>> for Vec<T, A> {
    fn from(s: Box<[T], A>) -> Vec<T, A> {
        s.into_vec()
    }
}
impl<T, A: Allocator> From<Vec<T, A>> for Box<[T], A> {
    fn from(v: Vec<T, A>) -> Box<[T], A> {
        v.into_boxed_slice()
    }
}
impl<'a> From<&'a str> for Vec<u8> {
    fn from(s: &'a str) -> Vec<u8> {
        From::from(s.as_bytes())
    }
}
impl<'a, T: Clone> From<&'a Vec<T>> for Cow<'a, [T]> {
    fn from(v: &'a Vec<T>) -> Cow<'a, [T]> {
        Cow::Borrowed(v.as_slice())
    }
}
// library/alloc/src/vec/cow.rs
impl<'a, T: Clone> FromIterator<T> for Cow<'a, [T]> {
    fn from_iter<I: IntoIterator<Item = T>>(it: I) -> Cow<'a, [T]> {
        Cow::Owned(FromIterator::from_iter(it))
    }
}

impl<'a, T: Clone> From<Vec<T>> for Cow<'a, [T]> {
    fn from(v: Vec<T>) -> Cow<'a, [T]> {
        Cow::Owned(v)
    }
}
impl<'a, T: Clone> From<&'a [T]> for Cow<'a, [T]> {
    fn from(s: &'a [T]) -> Cow<'a, [T]> {
        Cow::Borrowed(s)
    }
}
/// library/alloc/src/vec/cow.rs (stable 1.77, cow_from_array_ref): `(&[]).into()` (Robrix)
impl<'a, T: Clone, const N: usize> From<&'a [T; N]> for Cow<'a, [T]> {
    fn from(s: &'a [T; N]) -> Cow<'a, [T]> {
        Cow::Borrowed(s as &[_])
    }
}

impl<T> FromIterator<T> for Vec<T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Vec<T> {
        let mut v = Vec::new();
        v.extend(iter);
        v
    }
}

impl<T, A: Allocator> Vec<T, A> {
    /// Real alloc's extend_trusted, for a TrustedLen iterator of exactly `n` items (its upper size
    /// hint): one reserve, then writes through a raw pointer with the length in a local (no
    /// capacity check per item). A short iterator just ends early.
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
/// specialization; here by type identity, lifetimes erased).
fn is_slice_iter<I, T>() -> bool {
    {
        unsafe { im::type_id::<I>() == im::type_id::<crate::slice::Iter<'static, T>>() }
    }
}

impl<T, A: Allocator> Extend<T> for Vec<T, A> {
    fn extend<I: IntoIterator<Item = T>>(&mut self, iter: I) {
        let mut iter = iter.into_iter();
        // real alloc's SpecExtend: a TrustedLen iterator takes extend_trusted (its size hint is
        // exact by contract: one reserve, no capacity test per item), any other the desugared
        // loop -- one loop per instance, never both
        if unsafe { im::is_trusted_len::<I::IntoIter>() } {
            match iter.size_hint() {
                (_, Some(n)) => self.extend_counted(&mut iter, n),
                (_, None) => capacity_overflow(),
            }
            return;
        }
        let (lower, _) = iter.size_hint();
        self.reserve(lower);
        while let Some(x) = iter.next() {
            if self.len == self.capacity() {
                let (lower, _) = iter.size_hint();
                self.reserve(lower.saturating_add(1));
            }
            unsafe { im::ptr_write(im::ptr_add_mut(self.ptr.as_ptr(), self.len), x) };
            self.len += 1;
        }
    }
    fn extend_one(&mut self, item: T) {
        self.push(item);
    }
    fn extend_reserve(&mut self, additional: usize) {
        self.reserve(additional);
    }
}

impl<'a, T: Copy + 'a, A: Allocator> Extend<&'a T> for Vec<T, A> {
    fn extend<I: IntoIterator<Item = &'a T>>(&mut self, iter: I) {
        let mut iter = iter.into_iter();
        if is_slice_iter::<I::IntoIter, T>() {
            // one memcpy, as real alloc's SpecExtend<&T, slice::Iter<T>>
            let it: crate::slice::Iter<'a, T> = unsafe { crate::mem::transmute_copy(&iter) };
            crate::mem::forget(iter);
            self.extend_from_slice(it.as_slice());
            return;
        }
        if unsafe { im::is_trusted_len::<I::IntoIter>() } {
            match iter.size_hint() {
                (_, Some(n)) => {
                    let mut copied = CopiedRefs { it: &mut iter };
                    self.extend_counted(&mut copied, n);
                }
                (_, None) => capacity_overflow(),
            }
            return;
        }
        let (lower, _) = iter.size_hint();
        self.reserve(lower);
        while let Some(x) = iter.next() {
            self.push(*x);
        }
    }
    fn extend_one(&mut self, item: &'a T) {
        self.push(*item);
    }
}

impl<T, A: Allocator> IntoIterator for Vec<T, A> {
    type Item = T;
    type IntoIter = IntoIter<T, A>;
    fn into_iter(self) -> IntoIter<T, A> {
        into_iter::new(self)
    }
}

impl<'a, T, A: Allocator> IntoIterator for &'a Vec<T, A> {
    type Item = &'a T;
    type IntoIter = crate::slice::Iter<'a, T>;
    fn into_iter(self) -> crate::slice::Iter<'a, T> {
        crate::slice::Iter::new(self.as_slice())
    }
}

impl<'a, T, A: Allocator> IntoIterator for &'a mut Vec<T, A> {
    type Item = &'a mut T;
    type IntoIter = crate::slice::IterMut<'a, T>;
    fn into_iter(self) -> crate::slice::IterMut<'a, T> {
        crate::slice::IterMut::new(self.as_mut_slice())
    }
}

impl<T, A: Allocator, const N: usize> TryFrom<Vec<T, A>> for [T; N] {
    type Error = Vec<T, A>;
    fn try_from(vec: Vec<T, A>) -> Result<[T; N], Vec<T, A>> {
        if vec.len() != N {
            return Err(vec);
        }
        // the elements move out; the Vec's drop frees the buffer only (and drops its allocator)
        let mut vec = vec;
        unsafe { vec.set_len(0) };
        Ok(unsafe { im::ptr_read(vec.ptr.as_ptr() as *const [T; N]) })
    }
}

/// dedup_by's panic guard (real alloc's FillGapOnDrop): moves the unchecked tail `[read..len)` down
/// over the dropped gap `[write..read)` and shortens the Vec.
struct DedupGap<'a, T, A: Allocator> {
    read: usize,
    write: usize,
    vec: &'a mut Vec<T, A>,
}

impl<'a, T, A: Allocator> Drop for DedupGap<'a, T, A> {
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

impl<T, A: Allocator, const N: usize> Vec<[T; N], A> {
    /// The arrays' elements as one Vec, in place (real alloc's into_flattened).
    pub fn into_flattened(self) -> Vec<T, A> {
        let (ptr, len, cap, alloc) = self.into_raw_parts_with_allocator();
        let (new_len, new_cap) = if crate::mem::size_of::<T>() == 0 {
            match len.checked_mul(N) {
                Some(l) => (l, usize::MAX),
                None => crate::panicking::panic_str("vec len overflow"),
            }
        } else {
            (len * N, cap * N)
        };
        unsafe { Vec::from_raw_parts_in(ptr as *mut T, new_len, new_cap, alloc) }
    }
}
