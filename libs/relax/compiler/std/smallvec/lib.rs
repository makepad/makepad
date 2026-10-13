//! Relax's built-in `smallvec` crate: under Relax the crate path `smallvec` resolves here
//! (crate substitution, coord.md "SV1") instead of compiling libs/smallvec. Behaviour follows
//! smallvec 1.15.1 exactly (inline storage up to the array size, spill policy, capacities,
//! panic messages); the algorithms are ported from it (Copyright (c) 2018 The Servo Project
//! Developers, MIT/Apache-2.0; see libs/relax/compiler/std/CREDITS.md). Dialect form: no
//! macro_rules (`smallvec!` is a Relax built-in that expands per SV2), no features.
//! The rustc path keeps using the real crate; std/check_ext tests this file against it.

extern crate alloc;

use alloc::alloc::Layout;
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::borrow::{Borrow, BorrowMut};
use core::cmp;
use core::fmt;
use core::hash::{Hash, Hasher};
use core::iter::{FromIterator, FusedIterator, IntoIterator};
use core::mem;
use core::mem::{ManuallyDrop, MaybeUninit};
use core::ops::{self, RangeBounds};
use core::ptr;
use core::slice::{self, SliceIndex};

mod arrays;

/// Error type for APIs with fallible heap allocation.
#[derive(Debug)]
pub enum CollectionAllocErr {
    /// Overflow `usize::MAX` or other error during size computation
    CapacityOverflow,
    /// The allocator return an error
    AllocErr {
        /// The layout that was passed to the allocator
        layout: Layout,
    },
}

impl fmt::Display for CollectionAllocErr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Allocation error: {:?}", self)
    }
}

fn infallible<T>(result: Result<T, CollectionAllocErr>) -> T {
    match result {
        Ok(x) => x,
        Err(CollectionAllocErr::CapacityOverflow) => panic!("capacity overflow"),
        Err(CollectionAllocErr::AllocErr { layout }) => alloc::alloc::handle_alloc_error(layout),
    }
}

fn layout_array<T>(n: usize) -> Result<Layout, CollectionAllocErr> {
    let size = match mem::size_of::<T>().checked_mul(n) {
        Some(s) => s,
        None => return Err(CollectionAllocErr::CapacityOverflow),
    };
    let align = mem::align_of::<T>();
    match Layout::from_size_align(size, align) {
        Ok(l) => Ok(l),
        Err(_) => Err(CollectionAllocErr::CapacityOverflow),
    }
}

unsafe fn deallocate<T>(ptr: *mut T, capacity: usize) {
    let layout = layout_array::<T>(capacity).unwrap();
    alloc::alloc::dealloc(ptr as *mut u8, layout)
}

/// Types that can be used as the backing store for a SmallVec (implemented for `[T; N]` for
/// the same sizes smallvec 1.x implements without its const_generics feature: arrays.rs).
pub unsafe trait Array {
    /// The type of the array's elements.
    type Item;
    /// Returns the number of items the array can hold.
    fn size() -> usize;
}

/// Converts a slice of Copy items into a SmallVec.
pub trait ToSmallVec<A: Array> {
    /// Construct a new `SmallVec` from a slice.
    fn to_smallvec(&self) -> SmallVec<A>;
}

impl<A: Array> ToSmallVec<A> for [A::Item]
where
    A::Item: Copy,
{
    fn to_smallvec(&self) -> SmallVec<A> {
        SmallVec::from_slice(self)
    }
}

#[repr(C)]
union SmallVecData<A: Array> {
    inline: ManuallyDrop<MaybeUninit<A>>,
    heap: (*mut A::Item, usize),
}

impl<A: Array> SmallVecData<A> {
    fn from_inline(inline: MaybeUninit<A>) -> SmallVecData<A> {
        SmallVecData { inline: ManuallyDrop::new(inline) }
    }
    fn from_heap(ptr: *mut A::Item, len: usize) -> SmallVecData<A> {
        SmallVecData { heap: (ptr, len) }
    }
    unsafe fn inline(&self) -> *const A::Item {
        self.inline.as_ptr() as *const A::Item
    }
    unsafe fn inline_mut(&mut self) -> *mut A::Item {
        self.inline.as_mut_ptr() as *mut A::Item
    }
    unsafe fn into_inline(self) -> MaybeUninit<A> {
        ManuallyDrop::into_inner(self.inline)
    }
}

/// A Vec-like container that stores up to `A::size()` items inline.
pub struct SmallVec<A: Array> {
    // capacity <= inline_capacity(): inline, and capacity holds the length;
    // otherwise heap, and capacity is the allocation's capacity.
    capacity: usize,
    data: SmallVecData<A>,
}

unsafe impl<A: Array> Send for SmallVec<A> where A::Item: Send {}

impl<A: Array> SmallVec<A> {
    /// Construct an empty vector
    pub fn new() -> SmallVec<A> {
        assert!(
            mem::size_of::<A>() == A::size() * mem::size_of::<A::Item>()
                && mem::align_of::<A>() >= mem::align_of::<A::Item>()
        );
        SmallVec { capacity: 0, data: SmallVecData::from_inline(MaybeUninit::uninit()) }
    }

    /// Construct an empty vector with room for at least `n` elements.
    pub fn with_capacity(n: usize) -> SmallVec<A> {
        let mut v = SmallVec::new();
        v.reserve_exact(n);
        v
    }

    /// Construct a new `SmallVec` from a `Vec<A::Item>`.
    pub fn from_vec(mut vec: Vec<A::Item>) -> SmallVec<A> {
        if vec.capacity() <= Self::inline_capacity() {
            unsafe {
                let mut data = SmallVecData::<A>::from_inline(MaybeUninit::uninit());
                let len = vec.len();
                vec.set_len(0);
                ptr::copy_nonoverlapping(vec.as_ptr(), data.inline_mut(), len);
                SmallVec { capacity: len, data }
            }
        } else {
            let (ptr, cap, len) = (vec.as_mut_ptr(), vec.capacity(), vec.len());
            mem::forget(vec);
            SmallVec { capacity: cap, data: SmallVecData::from_heap(ptr, len) }
        }
    }

    /// Constructs a new `SmallVec` from an `A` without copying elements.
    pub fn from_buf(buf: A) -> SmallVec<A> {
        SmallVec { capacity: A::size(), data: SmallVecData::from_inline(MaybeUninit::new(buf)) }
    }

    /// Constructs a new `SmallVec` from an `A`, with length `len <= A::size()`.
    pub fn from_buf_and_len(buf: A, len: usize) -> SmallVec<A> {
        assert!(len <= A::size());
        unsafe { SmallVec::from_buf_and_len_unchecked(MaybeUninit::new(buf), len) }
    }

    /// Like from_buf_and_len without the length check.
    pub unsafe fn from_buf_and_len_unchecked(buf: MaybeUninit<A>, len: usize) -> SmallVec<A> {
        SmallVec { capacity: len, data: SmallVecData::from_inline(buf) }
    }

    /// Sets the length of a vector.
    pub unsafe fn set_len(&mut self, new_len: usize) {
        *self.len_mut() = new_len;
    }

    fn inline_capacity() -> usize {
        if mem::size_of::<A::Item>() > 0 {
            A::size()
        } else {
            usize::MAX
        }
    }

    /// The maximum number of elements this vector can hold inline
    pub fn inline_size(&self) -> usize {
        Self::inline_capacity()
    }

    /// The number of elements stored in the vector
    pub fn len(&self) -> usize {
        if self.spilled() {
            unsafe { self.data.heap.1 }
        } else {
            self.capacity
        }
    }

    /// Returns `true` if the vector is empty
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The number of items the vector can hold without reallocating
    pub fn capacity(&self) -> usize {
        if self.spilled() {
            self.capacity
        } else {
            Self::inline_capacity()
        }
    }

    fn data_ptr(&self) -> *const A::Item {
        unsafe {
            if self.spilled() {
                self.data.heap.0 as *const A::Item
            } else {
                self.data.inline()
            }
        }
    }

    fn data_ptr_mut(&mut self) -> *mut A::Item {
        unsafe {
            if self.spilled() {
                self.data.heap.0
            } else {
                self.data.inline_mut()
            }
        }
    }

    fn len_mut(&mut self) -> &mut usize {
        unsafe {
            if self.spilled() {
                &mut self.data.heap.1
            } else {
                &mut self.capacity
            }
        }
    }

    /// Returns `true` if the data has spilled into a separate heap-allocated buffer.
    pub fn spilled(&self) -> bool {
        self.capacity > Self::inline_capacity()
    }

    /// Creates a draining iterator that removes the specified range and yields the removed items.
    #[track_caller]
    pub fn drain<R: RangeBounds<usize>>(&mut self, range: R) -> Drain<'_, A> {
        use core::ops::Bound::*;
        let len = self.len();
        let start = match range.start_bound() {
            Included(&n) => n,
            Excluded(&n) => n.checked_add(1).expect("Range start out of bounds"),
            Unbounded => 0,
        };
        let end = match range.end_bound() {
            Included(&n) => n.checked_add(1).expect("Range end out of bounds"),
            Excluded(&n) => n,
            Unbounded => len,
        };
        assert!(start <= end);
        assert!(end <= len);
        unsafe {
            self.set_len(start);
            let range_slice = slice::from_raw_parts(self.as_ptr().add(start), end - start);
            Drain { tail_start: end, tail_len: len - end, iter: range_slice.iter(), vec: self as *mut SmallVec<A> }
        }
    }

    /// Append an item to the vector.
    pub fn push(&mut self, value: A::Item) {
        unsafe {
            if self.len() == self.capacity() {
                self.reserve_one_unchecked();
            }
            let len = self.len();
            ptr::write(self.data_ptr_mut().add(len), value);
            *self.len_mut() = len + 1;
        }
    }

    /// Remove an item from the end of the vector and return it, or None if empty.
    pub fn pop(&mut self) -> Option<A::Item> {
        unsafe {
            let len = self.len();
            if len == 0 {
                return None;
            }
            let last_index = len - 1;
            *self.len_mut() = last_index;
            Some(ptr::read(self.data_ptr().add(last_index)))
        }
    }

    /// Moves all the elements of `other` into `self`, leaving `other` empty.
    pub fn append<B: Array<Item = A::Item>>(&mut self, other: &mut SmallVec<B>) {
        self.extend(other.drain(..))
    }

    /// Re-allocate to set the capacity to `max(new_cap, inline_size())`.
    pub fn grow(&mut self, new_cap: usize) {
        infallible(self.try_grow(new_cap))
    }

    /// Re-allocate to set the capacity to `max(new_cap, inline_size())`.
    pub fn try_grow(&mut self, new_cap: usize) -> Result<(), CollectionAllocErr> {
        unsafe {
            let unspilled = !self.spilled();
            let ptr = self.data_ptr_mut();
            let len = self.len();
            let cap = self.capacity();
            assert!(new_cap >= len);
            if new_cap <= Self::inline_capacity() {
                if unspilled {
                    return Ok(());
                }
                self.data = SmallVecData::from_inline(MaybeUninit::uninit());
                ptr::copy_nonoverlapping(ptr, self.data.inline_mut(), len);
                self.capacity = len;
                deallocate(ptr, cap);
            } else if new_cap != cap {
                let layout = layout_array::<A::Item>(new_cap)?;
                let new_alloc: *mut A::Item;
                if unspilled {
                    new_alloc = alloc::alloc::alloc(layout) as *mut A::Item;
                    if new_alloc.is_null() {
                        return Err(CollectionAllocErr::AllocErr { layout });
                    }
                    ptr::copy_nonoverlapping(ptr, new_alloc, len);
                } else {
                    let old_layout = layout_array::<A::Item>(cap)?;
                    new_alloc = alloc::alloc::realloc(ptr as *mut u8, old_layout, layout.size()) as *mut A::Item;
                    if new_alloc.is_null() {
                        return Err(CollectionAllocErr::AllocErr { layout });
                    }
                }
                self.data = SmallVecData::from_heap(new_alloc, len);
                self.capacity = new_cap;
            }
            Ok(())
        }
    }

    /// Reserve capacity for `additional` more elements to be inserted.
    pub fn reserve(&mut self, additional: usize) {
        infallible(self.try_reserve(additional))
    }

    #[cold]
    fn reserve_one_unchecked(&mut self) {
        let new_cap = match self.len().checked_add(1).and_then(usize::checked_next_power_of_two) {
            Some(c) => c,
            None => panic!("capacity overflow"),
        };
        infallible(self.try_grow(new_cap))
    }

    /// Reserve capacity for `additional` more elements to be inserted.
    pub fn try_reserve(&mut self, additional: usize) -> Result<(), CollectionAllocErr> {
        let len = self.len();
        let cap = self.capacity();
        if cap - len >= additional {
            return Ok(());
        }
        let new_cap = match len.checked_add(additional).and_then(usize::checked_next_power_of_two) {
            Some(c) => c,
            None => return Err(CollectionAllocErr::CapacityOverflow),
        };
        self.try_grow(new_cap)
    }

    /// Reserve the minimum capacity for `additional` more elements to be inserted.
    pub fn reserve_exact(&mut self, additional: usize) {
        infallible(self.try_reserve_exact(additional))
    }

    /// Reserve the minimum capacity for `additional` more elements to be inserted.
    pub fn try_reserve_exact(&mut self, additional: usize) -> Result<(), CollectionAllocErr> {
        let len = self.len();
        let cap = self.capacity();
        if cap - len >= additional {
            return Ok(());
        }
        let new_cap = match len.checked_add(additional) {
            Some(c) => c,
            None => return Err(CollectionAllocErr::CapacityOverflow),
        };
        self.try_grow(new_cap)
    }

    /// Shrink the capacity of the vector as much as possible.
    pub fn shrink_to_fit(&mut self) {
        if !self.spilled() {
            return;
        }
        let len = self.len();
        if self.inline_size() >= len {
            unsafe {
                let (ptr, len) = self.data.heap;
                self.data = SmallVecData::from_inline(MaybeUninit::uninit());
                ptr::copy_nonoverlapping(ptr, self.data.inline_mut(), len);
                deallocate(ptr, self.capacity);
                self.capacity = len;
            }
        } else if self.capacity() > len {
            self.grow(len);
        }
    }

    /// Shorten the vector, keeping the first `len` elements and dropping the rest.
    pub fn truncate(&mut self, len: usize) {
        unsafe {
            let ptr = self.data_ptr_mut();
            while len < self.len() {
                let last_index = self.len() - 1;
                *self.len_mut() = last_index;
                ptr::drop_in_place(ptr.add(last_index));
            }
        }
    }

    /// Extracts a slice containing the entire vector.
    pub fn as_slice(&self) -> &[A::Item] {
        unsafe { slice::from_raw_parts(self.data_ptr(), self.len()) }
    }

    /// Extracts a mutable slice of the entire vector.
    pub fn as_mut_slice(&mut self) -> &mut [A::Item] {
        let len = self.len();
        unsafe { slice::from_raw_parts_mut(self.data_ptr_mut(), len) }
    }

    /// Remove the element at position `index`, replacing it with the last element.
    #[track_caller]
    pub fn swap_remove(&mut self, index: usize) -> A::Item {
        let len = self.len();
        self.as_mut_slice().swap(len - 1, index);
        match self.pop() {
            Some(v) => v,
            None => unsafe { core::hint::unreachable_unchecked() },
        }
    }

    /// Remove all elements from the vector.
    pub fn clear(&mut self) {
        self.truncate(0);
    }

    /// Remove and return the element at position `index`, shifting the rest left.
    #[track_caller]
    pub fn remove(&mut self, index: usize) -> A::Item {
        unsafe {
            let len = self.len();
            assert!(index < len);
            *self.len_mut() = len - 1;
            let ptr = self.data_ptr_mut().add(index);
            let item = ptr::read(ptr);
            ptr::copy(ptr.add(1), ptr, len - index - 1);
            item
        }
    }

    /// Insert an element at position `index`, shifting all elements after it to the right.
    #[track_caller]
    pub fn insert(&mut self, index: usize, element: A::Item) {
        unsafe {
            if self.len() == self.capacity() {
                self.reserve_one_unchecked();
            }
            let len = self.len();
            if index > len {
                panic!("index exceeds length");
            }
            let ptr = self.data_ptr_mut().add(index);
            if index < len {
                ptr::copy(ptr, ptr.add(1), len - index);
            }
            *self.len_mut() = len + 1;
            ptr::write(ptr, element);
        }
    }

    /// Insert multiple elements at position `index`, shifting following elements back.
    #[track_caller]
    pub fn insert_many<I: IntoIterator<Item = A::Item>>(&mut self, index: usize, iterable: I) {
        let mut iter = iterable.into_iter();
        if index == self.len() {
            return self.extend(iter);
        }
        let (lower_size_bound, _) = iter.size_hint();
        assert!(lower_size_bound <= isize::MAX as usize);
        assert!(index + lower_size_bound >= index);
        let mut num_added = 0;
        let old_len = self.len();
        assert!(index <= old_len);
        unsafe {
            self.reserve(lower_size_bound);
            let start = self.as_mut_ptr();
            let ptr = start.add(index);
            ptr::copy(ptr, ptr.add(lower_size_bound), old_len - index);
            self.set_len(0);
            let start = self.as_mut_ptr();
            let ptr = start.add(index);
            while num_added < lower_size_bound {
                let element = match iter.next() {
                    Some(x) => x,
                    None => break,
                };
                ptr::write(ptr.add(num_added), element);
                num_added += 1;
            }
            if num_added < lower_size_bound {
                ptr::copy(ptr.add(lower_size_bound), ptr.add(num_added), old_len - index);
            }
            self.set_len(old_len + num_added);
        }
        for element in iter {
            self.insert(index + num_added, element);
            num_added += 1;
        }
    }

    /// Convert to a `Vec`, reusing the heap buffer if spilled.
    pub fn into_vec(self) -> Vec<A::Item> {
        if self.spilled() {
            unsafe {
                let (ptr, len) = self.data.heap;
                let v = Vec::from_raw_parts(ptr, len, self.capacity);
                mem::forget(self);
                v
            }
        } else {
            self.into_iter().collect()
        }
    }

    /// Converts into a `Box<[A::Item]>`.
    pub fn into_boxed_slice(self) -> Box<[A::Item]> {
        self.into_vec().into_boxed_slice()
    }

    /// Convert to the inline array if it is full and not spilled.
    pub fn into_inner(self) -> Result<A, SmallVec<A>> {
        if self.spilled() || self.len() != A::size() {
            Err(self)
        } else {
            unsafe {
                let data = ptr::read(&self.data);
                mem::forget(self);
                Ok(data.into_inline().assume_init())
            }
        }
    }

    /// Retains only the elements for which `f` returns true (order preserved).
    pub fn retain<F: FnMut(&mut A::Item) -> bool>(&mut self, f: F) {
        let mut f = f;
        let mut del = 0;
        let len = self.len();
        let mut i = 0;
        while i < len {
            if !f(&mut self.as_mut_slice()[i]) {
                del += 1;
            } else if del > 0 {
                self.as_mut_slice().swap(i - del, i);
            }
            i += 1;
        }
        self.truncate(len - del);
    }

    /// Same as `retain` (kept for API compatibility with Vec).
    pub fn retain_mut<F: FnMut(&mut A::Item) -> bool>(&mut self, f: F) {
        self.retain(f)
    }

    /// Removes consecutive duplicate elements.
    pub fn dedup(&mut self)
    where
        A::Item: PartialEq<A::Item>,
    {
        self.dedup_by(|a, b| a == b);
    }

    /// Removes consecutive elements for which `same_bucket` returns true.
    pub fn dedup_by<F: FnMut(&mut A::Item, &mut A::Item) -> bool>(&mut self, same_bucket: F) {
        let mut same_bucket = same_bucket;
        let len = self.len();
        if len <= 1 {
            return;
        }
        let ptr = self.as_mut_ptr();
        let mut w: usize = 1;
        unsafe {
            let mut r = 1;
            while r < len {
                let p_r = ptr.add(r);
                let p_wm1 = ptr.add(w - 1);
                if !same_bucket(&mut *p_r, &mut *p_wm1) {
                    if r != w {
                        let p_w = p_wm1.add(1);
                        mem::swap(&mut *p_r, &mut *p_w);
                    }
                    w += 1;
                }
                r += 1;
            }
        }
        self.truncate(w);
    }

    /// Removes consecutive elements that map to the same key.
    pub fn dedup_by_key<K: PartialEq<K>, F: FnMut(&mut A::Item) -> K>(&mut self, key: F) {
        let mut key = key;
        self.dedup_by(|a, b| key(a) == key(b));
    }

    /// Resizes to `new_len`, filling new slots with values from `f`.
    pub fn resize_with<F: FnMut() -> A::Item>(&mut self, new_len: usize, f: F) {
        let old_len = self.len();
        if old_len < new_len {
            let mut f = f;
            let additional = new_len - old_len;
            self.reserve(additional);
            let mut i = 0;
            while i < additional {
                self.push(f());
                i += 1;
            }
        } else if old_len > new_len {
            self.truncate(new_len);
        }
    }

    /// Creates a `SmallVec` directly from a heap buffer (capacity must exceed the inline size).
    #[track_caller]
    pub unsafe fn from_raw_parts(ptr: *mut A::Item, length: usize, capacity: usize) -> SmallVec<A> {
        assert!(capacity > Self::inline_capacity());
        SmallVec { capacity, data: SmallVecData::from_heap(ptr, length) }
    }

    /// Returns a raw pointer to the vector's buffer.
    pub fn as_ptr(&self) -> *const A::Item {
        self.data_ptr()
    }

    /// Returns a raw mutable pointer to the vector's buffer.
    pub fn as_mut_ptr(&mut self) -> *mut A::Item {
        self.data_ptr_mut()
    }
}

impl<A: Array> SmallVec<A>
where
    A::Item: Copy,
{
    /// Copy the elements from a slice into a new `SmallVec`.
    pub fn from_slice(slice: &[A::Item]) -> SmallVec<A> {
        let len = slice.len();
        if len <= Self::inline_capacity() {
            let mut data: MaybeUninit<A> = MaybeUninit::uninit();
            unsafe { ptr::copy_nonoverlapping(slice.as_ptr(), data.as_mut_ptr() as *mut A::Item, len) };
            SmallVec { capacity: len, data: SmallVecData::from_inline(data) }
        } else {
            let mut b = slice.to_vec();
            let cap = b.capacity();
            let ptr = b.as_mut_ptr();
            mem::forget(b);
            SmallVec { capacity: cap, data: SmallVecData::from_heap(ptr, len) }
        }
    }

    /// Copy elements from a slice into the vector at position `index`.
    #[track_caller]
    pub fn insert_from_slice(&mut self, index: usize, slice: &[A::Item]) {
        self.reserve(slice.len());
        let len = self.len();
        assert!(index <= len);
        unsafe {
            let slice_ptr = slice.as_ptr();
            let ptr = self.as_mut_ptr().add(index);
            ptr::copy(ptr, ptr.add(slice.len()), len - index);
            ptr::copy_nonoverlapping(slice_ptr, ptr, slice.len());
            self.set_len(len + slice.len());
        }
    }

    /// Copy elements from a slice and append them to the vector.
    pub fn extend_from_slice(&mut self, slice: &[A::Item]) {
        let len = self.len();
        self.insert_from_slice(len, slice);
    }
}

impl<A: Array> SmallVec<A>
where
    A::Item: Clone,
{
    /// Resizes the vector so that its length is equal to `len`.
    pub fn resize(&mut self, len: usize, value: A::Item) {
        let old_len = self.len();
        if len > old_len {
            self.extend(core::iter::repeat(value).take(len - old_len));
        } else {
            self.truncate(len);
        }
    }

    /// Creates a `SmallVec` with `n` copies of `elem`.
    pub fn from_elem(elem: A::Item, n: usize) -> SmallVec<A> {
        if n > Self::inline_capacity() {
            {
                let mut v = Vec::with_capacity(n);
                v.resize(n, elem);
                SmallVec::from_vec(v)
            }
        } else {
            let mut v = SmallVec::<A>::new();
            unsafe {
                let ptr = v.data_ptr_mut();
                let mut i = 0;
                while i < n {
                    ptr::write(ptr.add(i), elem.clone());
                    v.capacity += 1;
                    i += 1;
                }
            }
            v
        }
    }
}

impl<A: Array> ops::Deref for SmallVec<A> {
    type Target = [A::Item];
    fn deref(&self) -> &[A::Item] {
        self.as_slice()
    }
}

impl<A: Array> ops::DerefMut for SmallVec<A> {
    fn deref_mut(&mut self) -> &mut [A::Item] {
        self.as_mut_slice()
    }
}

impl<A: Array> AsRef<[A::Item]> for SmallVec<A> {
    fn as_ref(&self) -> &[A::Item] {
        self
    }
}

impl<A: Array> AsMut<[A::Item]> for SmallVec<A> {
    fn as_mut(&mut self) -> &mut [A::Item] {
        self
    }
}

impl<A: Array> Borrow<[A::Item]> for SmallVec<A> {
    fn borrow(&self) -> &[A::Item] {
        self
    }
}

impl<A: Array> BorrowMut<[A::Item]> for SmallVec<A> {
    fn borrow_mut(&mut self) -> &mut [A::Item] {
        self
    }
}

impl<'a, A: Array> From<&'a [A::Item]> for SmallVec<A>
where
    A::Item: Clone,
{
    fn from(slice: &'a [A::Item]) -> SmallVec<A> {
        slice.iter().cloned().collect()
    }
}

impl<A: Array> From<Vec<A::Item>> for SmallVec<A> {
    fn from(vec: Vec<A::Item>) -> SmallVec<A> {
        SmallVec::from_vec(vec)
    }
}

impl<A: Array> From<A> for SmallVec<A> {
    fn from(array: A) -> SmallVec<A> {
        SmallVec::from_buf(array)
    }
}

impl<A: Array, I: SliceIndex<[A::Item]>> ops::Index<I> for SmallVec<A> {
    type Output = I::Output;
    #[track_caller]
    fn index(&self, index: I) -> &I::Output {
        &(**self)[index]
    }
}

impl<A: Array, I: SliceIndex<[A::Item]>> ops::IndexMut<I> for SmallVec<A> {
    #[track_caller]
    fn index_mut(&mut self, index: I) -> &mut I::Output {
        &mut (&mut **self)[index]
    }
}

impl<A: Array> FromIterator<A::Item> for SmallVec<A> {
    fn from_iter<I: IntoIterator<Item = A::Item>>(iterable: I) -> SmallVec<A> {
        let mut v = SmallVec::new();
        v.extend(iterable);
        v
    }
}

impl<A: Array> Extend<A::Item> for SmallVec<A> {
    fn extend<I: IntoIterator<Item = A::Item>>(&mut self, iterable: I) {
        let mut iter = iterable.into_iter();
        let (lower_size_bound, _) = iter.size_hint();
        self.reserve(lower_size_bound);
        unsafe {
            let cap = self.capacity();
            let ptr = self.data_ptr_mut();
            let mut len = self.len();
            while len < cap {
                match iter.next() {
                    Some(out) => {
                        ptr::write(ptr.add(len), out);
                        len += 1;
                        *self.len_mut() = len;
                    }
                    None => return,
                }
            }
        }
        for elem in iter {
            self.push(elem);
        }
    }
}

impl<A: Array> fmt::Debug for SmallVec<A>
where
    A::Item: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

impl<A: Array> Default for SmallVec<A> {
    fn default() -> SmallVec<A> {
        SmallVec::new()
    }
}

impl<A: Array> Drop for SmallVec<A> {
    fn drop(&mut self) {
        unsafe {
            if self.spilled() {
                let (ptr, len) = self.data.heap;
                drop(Vec::from_raw_parts(ptr, len, self.capacity));
            } else {
                ptr::drop_in_place(self.as_mut_slice() as *mut [A::Item]);
            }
        }
    }
}

impl<A: Array> Clone for SmallVec<A>
where
    A::Item: Clone,
{
    fn clone(&self) -> SmallVec<A> {
        SmallVec::from(self.as_slice())
    }
    fn clone_from(&mut self, source: &SmallVec<A>) {
        self.truncate(source.len());
        let (init, tail) = source.split_at(self.len());
        self.as_mut_slice().clone_from_slice(init);
        self.extend(tail.iter().cloned());
    }
}

impl<A: Array, B: Array> PartialEq<SmallVec<B>> for SmallVec<A>
where
    A::Item: PartialEq<B::Item>,
{
    fn eq(&self, other: &SmallVec<B>) -> bool {
        self[..] == other[..]
    }
}

impl<A: Array> Eq for SmallVec<A> where A::Item: Eq {}

impl<A: Array> PartialOrd for SmallVec<A>
where
    A::Item: PartialOrd,
{
    fn partial_cmp(&self, other: &SmallVec<A>) -> Option<cmp::Ordering> {
        PartialOrd::partial_cmp(&**self, &**other)
    }
}

impl<A: Array> Ord for SmallVec<A>
where
    A::Item: Ord,
{
    fn cmp(&self, other: &SmallVec<A>) -> cmp::Ordering {
        Ord::cmp(&**self, &**other)
    }
}

impl<A: Array> Hash for SmallVec<A>
where
    A::Item: Hash,
{
    fn hash<H: Hasher>(&self, state: &mut H) {
        (**self).hash(state)
    }
}

// ---------------------------------------------------------------- IntoIter

/// An iterator that consumes a `SmallVec` and yields its items by value.
pub struct IntoIter<A: Array> {
    data: SmallVec<A>,
    current: usize,
    end: usize,
}

impl<A: Array> fmt::Debug for IntoIter<A>
where
    A::Item: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("IntoIter").field(&self.as_slice()).finish()
    }
}

impl<A: Array + Clone> Clone for IntoIter<A>
where
    A::Item: Clone,
{
    fn clone(&self) -> IntoIter<A> {
        SmallVec::from(self.as_slice()).into_iter()
    }
}

impl<A: Array> Drop for IntoIter<A> {
    fn drop(&mut self) {
        while let Some(x) = self.next() {
            drop(x);
        }
    }
}

impl<A: Array> Iterator for IntoIter<A> {
    type Item = A::Item;
    fn next(&mut self) -> Option<A::Item> {
        if self.current == self.end {
            None
        } else {
            unsafe {
                let current = self.current;
                self.current += 1;
                Some(ptr::read(self.data.as_ptr().add(current)))
            }
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let size = self.end - self.current;
        (size, Some(size))
    }
}

impl<A: Array> DoubleEndedIterator for IntoIter<A> {
    fn next_back(&mut self) -> Option<A::Item> {
        if self.current == self.end {
            None
        } else {
            unsafe {
                self.end -= 1;
                Some(ptr::read(self.data.as_ptr().add(self.end)))
            }
        }
    }
}

impl<A: Array> ExactSizeIterator for IntoIter<A> {}
impl<A: Array> FusedIterator for IntoIter<A> {}

impl<A: Array> IntoIter<A> {
    /// Returns the remaining items of this iterator as a slice.
    pub fn as_slice(&self) -> &[A::Item] {
        let len = self.end - self.current;
        unsafe { slice::from_raw_parts(self.data.as_ptr().add(self.current), len) }
    }
    /// Returns the remaining items of this iterator as a mutable slice.
    pub fn as_mut_slice(&mut self) -> &mut [A::Item] {
        let len = self.end - self.current;
        unsafe { slice::from_raw_parts_mut(self.data.as_mut_ptr().add(self.current), len) }
    }
}

impl<A: Array> IntoIterator for SmallVec<A> {
    type IntoIter = IntoIter<A>;
    type Item = A::Item;
    fn into_iter(self) -> IntoIter<A> {
        let mut data = self;
        unsafe {
            let len = data.len();
            data.set_len(0);
            IntoIter { data, current: 0, end: len }
        }
    }
}

impl<'a, A: Array> IntoIterator for &'a SmallVec<A> {
    type IntoIter = slice::Iter<'a, A::Item>;
    type Item = &'a A::Item;
    fn into_iter(self) -> slice::Iter<'a, A::Item> {
        self.as_slice().iter()
    }
}

impl<'a, A: Array> IntoIterator for &'a mut SmallVec<A> {
    type IntoIter = slice::IterMut<'a, A::Item>;
    type Item = &'a mut A::Item;
    fn into_iter(self) -> slice::IterMut<'a, A::Item> {
        self.as_mut_slice().iter_mut()
    }
}

// ---------------------------------------------------------------- Drain

/// An iterator that removes the items from a `SmallVec` and yields them by value.
pub struct Drain<'a, T: 'a + Array> {
    tail_start: usize,
    tail_len: usize,
    iter: slice::Iter<'a, T::Item>,
    vec: *mut SmallVec<T>,
}

impl<'a, T: 'a + Array> fmt::Debug for Drain<'a, T>
where
    T::Item: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Drain").field(&self.iter.as_slice()).finish()
    }
}

unsafe impl<'a, T: Sync + Array> Sync for Drain<'a, T> {}
unsafe impl<'a, T: Send + Array> Send for Drain<'a, T> {}

impl<'a, T: 'a + Array> Iterator for Drain<'a, T> {
    type Item = T::Item;
    fn next(&mut self) -> Option<T::Item> {
        match self.iter.next() {
            Some(reference) => Some(unsafe { ptr::read(reference) }),
            None => None,
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.iter.size_hint()
    }
}

impl<'a, T: 'a + Array> DoubleEndedIterator for Drain<'a, T> {
    fn next_back(&mut self) -> Option<T::Item> {
        match self.iter.next_back() {
            Some(reference) => Some(unsafe { ptr::read(reference) }),
            None => None,
        }
    }
}

impl<'a, T: Array> ExactSizeIterator for Drain<'a, T> {}
impl<'a, T: Array> FusedIterator for Drain<'a, T> {}

impl<'a, T: 'a + Array> Drop for Drain<'a, T> {
    fn drop(&mut self) {
        while let Some(x) = self.next() {
            drop(x);
        }
        if self.tail_len > 0 {
            unsafe {
                let source_vec = &mut *self.vec;
                let start = source_vec.len();
                let tail = self.tail_start;
                if tail != start {
                    let ptr = source_vec.as_mut_ptr();
                    ptr::copy(ptr.add(tail), ptr.add(start), self.tail_len);
                }
                source_vec.set_len(start + self.tail_len);
            }
        }
    }
}
