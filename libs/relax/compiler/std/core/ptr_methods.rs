//! Inherent methods of `*const T` and `*mut T` (Relax only: impls on primitive types).
//! Address arithmetic is `wrapping`-exact like core's; methods that need `T: Sized` live in
//! the `impl<T>` blocks, address-only ones in `impl<T: PointeeSized>` (library/core/src/ptr/
//! const_ptr.rs:7, mut_ptr.rs:8).

use crate::cmp::Ordering;
use crate::intrinsics_mem as im;

impl<T: crate::marker::PointeeSized> *const T {
    #[rustc_diagnostic_item = "ptr_const_is_null"]
    pub const fn is_null(self) -> bool {
        // (core's is_null: the address at run time, `ptr_guaranteed_cmp` with null in const evaluation)
        unsafe { im::ptr_is_null(self as *const u8) }
    }
    #[rustc_diagnostic_item = "const_ptr_cast"]
    pub const fn cast<U>(self) -> *const U {
        self as *const U
    }
    #[rustc_diagnostic_item = "ptr_cast_mut"]
    pub const fn cast_mut(self) -> *mut T {
        self as *mut T
    }
    pub const fn addr(self) -> usize {
        self as *const u8 as usize
    }
    pub fn with_addr(self, addr: usize) -> Self {
        let delta = (addr as isize).wrapping_sub(self.addr() as isize);
        self.wrapping_byte_add(delta as usize)
    }
    pub fn map_addr(self, f: impl FnOnce(usize) -> usize) -> Self {
        self.with_addr(f(self.addr()))
    }
    pub const unsafe fn as_ref<'a>(self) -> Option<&'a T> {
        if self.is_null() {
            None
        } else {
            Some(&*self)
        }
    }
    /// `&*self`: as_ref without the null check (stable 1.95); the pointer must be valid for 'a.
    #[must_use]
    pub const unsafe fn as_ref_unchecked<'a>(self) -> &'a T {
        unsafe { &*self }
    }
    pub const fn byte_add(self, count: usize) -> *const T {
        self.wrapping_byte_add(count)
    }
    pub const fn byte_sub(self, count: usize) -> *const T {
        self.wrapping_byte_sub(count)
    }
    pub const fn byte_offset(self, count: isize) -> *const T {
        self.wrapping_byte_add(count as usize)
    }
    /// Distance in bytes to `origin`, which may point to another type; both into the same
    /// allocation (core const_ptr.rs: the `u8` casts' offset_from).
    pub const unsafe fn byte_offset_from<U: crate::marker::PointeeSized>(self, origin: *const U) -> isize {
        unsafe { self.cast::<u8>().offset_from(origin.cast::<u8>()) }
    }
    pub const unsafe fn byte_offset_from_unsigned<U: crate::marker::PointeeSized>(self, origin: *const U) -> usize {
        unsafe { self.cast::<u8>().offset_from_unsigned(origin.cast::<u8>()) }
    }
    // (core: `self.cast::<u8>().wrapping_add(count).with_metadata_of(self)`: the address word moves
    // with its provenance, the metadata stays)
    pub const fn wrapping_byte_add(self, count: usize) -> *const T {
        let mut p = self;
        let a = &mut p as *mut *const T as *mut *const u8;
        unsafe { *a = im::arith_offset(*a, count as isize) };
        p
    }
    pub const fn wrapping_byte_sub(self, count: usize) -> *const T {
        let mut p = self;
        let a = &mut p as *mut *const T as *mut *const u8;
        unsafe { *a = im::arith_offset(*a, (count as isize).wrapping_neg()) };
        p
    }
}

impl<T> *const T {
    pub const unsafe fn add(self, count: usize) -> *const T {
        im::ptr_add(self, count)
    }
    // (core: in-bounds `offset` / `add` / `sub` and wrapping `arith_offset`, all keeping provenance)
    pub const unsafe fn sub(self, count: usize) -> *const T {
        im::ptr_sub(self, count)
    }
    pub const unsafe fn offset(self, count: isize) -> *const T {
        im::ptr_add(self, count as usize)
    }
    pub const fn wrapping_add(self, count: usize) -> *const T {
        unsafe { im::arith_offset(self, count as isize) }
    }
    pub const fn wrapping_sub(self, count: usize) -> *const T {
        unsafe { im::arith_offset(self, (count as isize).wrapping_neg()) }
    }
    pub const fn wrapping_offset(self, count: isize) -> *const T {
        unsafe { im::arith_offset(self, count) }
    }
    /// Distance in elements; both pointers into the same allocation (core: ptr_offset_from).
    pub const unsafe fn offset_from(self, origin: *const T) -> isize {
        let size = im::size_of::<T>();
        if size == 0 {
            crate::panicking::panic_str("assertion failed: 0 < pointee_size && pointee_size <= isize::MAX as usize");
        }
        im::ptr_offset_from(self, origin)
    }
    pub const unsafe fn offset_from_unsigned(self, origin: *const T) -> usize {
        self.offset_from(origin) as usize
    }
    pub const unsafe fn read(self) -> T {
        im::ptr_read(self)
    }
    pub const unsafe fn read_unaligned(self) -> T {
        crate::ptr::read_unaligned(self)
    }
    pub unsafe fn read_volatile(self) -> T {
        crate::ptr::read_volatile(self)
    }
    pub const unsafe fn copy_to(self, dest: *mut T, count: usize) {
        im::copy(self, dest, count)
    }
    pub const unsafe fn copy_to_nonoverlapping(self, dest: *mut T, count: usize) {
        im::copy_nonoverlapping(self, dest, count)
    }
    pub fn is_aligned(self) -> bool {
        (self as usize) % unsafe { im::align_of::<T>() } == 0
    }
    pub fn align_offset(self, align: usize) -> usize {
        if !align.is_power_of_two() {
            crate::panicking::panic_str("align_offset: align is not a power-of-two");
        }
        let addr = self as usize;
        let size = unsafe { im::size_of::<T>() };
        let rem = addr & (align - 1);
        if rem == 0 {
            return 0;
        }
        let bytes = align - rem;
        if size == 0 {
            return usize::MAX;
        }
        if bytes % size == 0 {
            bytes / size
        } else {
            usize::MAX
        }
    }
}

impl<T> *const [T] {
    pub const fn len(self) -> usize {
        unsafe { im::slice_len(&*self) }
    }
    pub const fn is_empty(self) -> bool {
        self.len() == 0
    }
    pub const fn as_ptr(self) -> *const T {
        self as *const T
    }
    /// A raw pointer to the underlying array, `None` when the length is not exactly `N` (stable
    /// since 1.93).
    pub const fn as_array<const N: usize>(self) -> Option<*const [T; N]> {
        if self.len() == N {
            Some(self.as_ptr() as *const [T; N])
        } else {
            None
        }
    }
}

impl<T: crate::marker::PointeeSized> *mut T {
    #[rustc_diagnostic_item = "ptr_is_null"]
    pub const fn is_null(self) -> bool {
        // (core's is_null: the address at run time, `ptr_guaranteed_cmp` with null in const evaluation)
        unsafe { im::ptr_is_null(self as *const u8) }
    }
    #[rustc_diagnostic_item = "ptr_cast"]
    pub const fn cast<U>(self) -> *mut U {
        self as *mut U
    }
    #[rustc_diagnostic_item = "ptr_cast_const"]
    pub const fn cast_const(self) -> *const T {
        self as *const T
    }
    pub const fn addr(self) -> usize {
        self as *const u8 as usize
    }
    pub fn with_addr(self, addr: usize) -> Self {
        let delta = (addr as isize).wrapping_sub(self.addr() as isize);
        self.wrapping_byte_add(delta as usize)
    }
    pub fn map_addr(self, f: impl FnOnce(usize) -> usize) -> Self {
        self.with_addr(f(self.addr()))
    }
    pub const unsafe fn as_ref<'a>(self) -> Option<&'a T> {
        if self.is_null() {
            None
        } else {
            Some(&*self)
        }
    }
    pub const unsafe fn as_mut<'a>(self) -> Option<&'a mut T> {
        if self.is_null() {
            None
        } else {
            Some(&mut *self)
        }
    }
    /// `&*self`: as_ref without the null check (stable 1.95); the pointer must be valid for 'a.
    #[must_use]
    pub const unsafe fn as_ref_unchecked<'a>(self) -> &'a T {
        unsafe { &*self }
    }
    /// `&mut *self`: as_mut without the null check (stable 1.95); the pointer must be valid and
    /// unaliased for 'a.
    #[must_use]
    pub const unsafe fn as_mut_unchecked<'a>(self) -> &'a mut T {
        unsafe { &mut *self }
    }
    pub const fn byte_add(self, count: usize) -> *mut T {
        self.wrapping_byte_add(count)
    }
    pub const fn byte_sub(self, count: usize) -> *mut T {
        self.wrapping_byte_sub(count)
    }
    pub const fn byte_offset(self, count: isize) -> *mut T {
        self.wrapping_byte_add(count as usize)
    }
    // (core mut_ptr.rs: the unsigned form takes a `*mut U`, the signed one a `*const U`)
    pub const unsafe fn byte_offset_from<U: crate::marker::PointeeSized>(self, origin: *const U) -> isize {
        unsafe { (self as *const T).byte_offset_from(origin) }
    }
    pub const unsafe fn byte_offset_from_unsigned<U: crate::marker::PointeeSized>(self, origin: *mut U) -> usize {
        unsafe { (self as *const T).byte_offset_from_unsigned(origin as *const U) }
    }
    // (core: `self.cast::<u8>().wrapping_add(count).with_metadata_of(self)`: the address word moves
    // with its provenance, the metadata stays)
    pub const fn wrapping_byte_add(self, count: usize) -> *mut T {
        let mut p = self;
        let a = &mut p as *mut *mut T as *mut *const u8;
        unsafe { *a = im::arith_offset(*a, count as isize) };
        p
    }
    pub const fn wrapping_byte_sub(self, count: usize) -> *mut T {
        let mut p = self;
        let a = &mut p as *mut *mut T as *mut *const u8;
        unsafe { *a = im::arith_offset(*a, (count as isize).wrapping_neg()) };
        p
    }
    pub const unsafe fn drop_in_place(self) {
        im::drop_in_place(self)
    }
}

impl<T> *mut T {
    pub const unsafe fn add(self, count: usize) -> *mut T {
        im::ptr_add_mut(self, count)
    }
    pub const unsafe fn sub(self, count: usize) -> *mut T {
        im::ptr_sub(self as *const T, count) as *mut T
    }
    pub const unsafe fn offset(self, count: isize) -> *mut T {
        im::ptr_add_mut(self, count as usize)
    }
    pub const fn wrapping_add(self, count: usize) -> *mut T {
        unsafe { im::arith_offset(self as *const T, count as isize) as *mut T }
    }
    pub const fn wrapping_sub(self, count: usize) -> *mut T {
        unsafe { im::arith_offset(self as *const T, (count as isize).wrapping_neg()) as *mut T }
    }
    pub const fn wrapping_offset(self, count: isize) -> *mut T {
        unsafe { im::arith_offset(self as *const T, count) as *mut T }
    }
    pub const unsafe fn offset_from(self, origin: *const T) -> isize {
        (self as *const T).offset_from(origin)
    }
    pub const unsafe fn offset_from_unsigned(self, origin: *const T) -> usize {
        (self as *const T).offset_from(origin) as usize
    }
    pub const unsafe fn read(self) -> T {
        im::ptr_read(self as *const T)
    }
    pub const unsafe fn read_unaligned(self) -> T {
        crate::ptr::read_unaligned(self as *const T)
    }
    pub unsafe fn read_volatile(self) -> T {
        crate::ptr::read_volatile(self as *const T)
    }
    pub const unsafe fn write(self, val: T) {
        im::ptr_write(self, val)
    }
    pub const unsafe fn write_unaligned(self, val: T) {
        crate::ptr::write_unaligned(self, val)
    }
    pub unsafe fn write_volatile(self, val: T) {
        crate::ptr::write_volatile(self, val)
    }
    pub const unsafe fn write_bytes(self, val: u8, count: usize) {
        im::write_bytes(self, val, count)
    }
    pub const unsafe fn replace(self, src: T) -> T {
        crate::ptr::replace(self, src)
    }
    pub const unsafe fn swap(self, with: *mut T) {
        crate::ptr::swap(self, with)
    }
    pub const unsafe fn copy_from(self, src: *const T, count: usize) {
        im::copy(src, self, count)
    }
    pub const unsafe fn copy_from_nonoverlapping(self, src: *const T, count: usize) {
        im::copy_nonoverlapping(src, self, count)
    }
    pub const unsafe fn copy_to(self, dest: *mut T, count: usize) {
        im::copy(self as *const T, dest, count)
    }
    pub const unsafe fn copy_to_nonoverlapping(self, dest: *mut T, count: usize) {
        im::copy_nonoverlapping(self as *const T, dest, count)
    }
    pub fn is_aligned(self) -> bool {
        (self as usize) % unsafe { im::align_of::<T>() } == 0
    }
    pub fn align_offset(self, align: usize) -> usize {
        (self as *const T).align_offset(align)
    }
}

impl<T> *mut [T] {
    pub const fn len(self) -> usize {
        unsafe { im::slice_len(&*self) }
    }
    pub const fn is_empty(self) -> bool {
        self.len() == 0
    }
    pub const fn as_mut_ptr(self) -> *mut T {
        self as *mut T
    }
    /// A raw, mutable pointer to the underlying array, `None` when the length is not exactly `N`
    /// (stable since 1.93).
    pub const fn as_mut_array<const N: usize>(self) -> Option<*mut [T; N]> {
        if self.len() == N {
            Some(self.as_mut_ptr() as *mut [T; N])
        } else {
            None
        }
    }
}

/// (address, metadata) of a possibly fat raw pointer (metadata 0 for thin pointers), so raw
/// pointer comparisons see the length / vtable like core's.
pub(crate) fn ptr_words<T: crate::marker::PointeeSized>(p: *const T) -> (usize, usize) {
    let words = unsafe { im::size_of::<*const T>() } / unsafe { im::size_of::<usize>() };
    let base = &p as *const *const T as *const usize;
    unsafe {
        if words == 2 {
            (im::ptr_read(base), im::ptr_read(im::ptr_add(base, 1)))
        } else {
            (im::ptr_read(base), 0)
        }
    }
}

// Raw pointers compare by address (fat pointers: address, then metadata as core; Relax
// compares the address part).
impl<T: crate::marker::PointeeSized> PartialEq for *const T {
    fn eq(&self, other: &*const T) -> bool {
        ptr_words(*self) == ptr_words(*other)
    }
}
impl<T: crate::marker::PointeeSized> Eq for *const T {}
impl<T: crate::marker::PointeeSized> PartialEq for *mut T {
    fn eq(&self, other: &*mut T) -> bool {
        ptr_words(*self as *const T) == ptr_words(*other as *const T)
    }
}
impl<T: crate::marker::PointeeSized> Eq for *mut T {}
impl<T: crate::marker::PointeeSized> PartialOrd for *const T {
    fn partial_cmp(&self, other: &*const T) -> Option<Ordering> {
        ptr_words(*self as *const T).partial_cmp(&ptr_words(*other as *const T))
    }
}
impl<T: crate::marker::PointeeSized> Ord for *const T {
    fn cmp(&self, other: &*const T) -> Ordering {
        ptr_words(*self as *const T).cmp(&ptr_words(*other as *const T))
    }
}
impl<T: crate::marker::PointeeSized> PartialOrd for *mut T {
    fn partial_cmp(&self, other: &*mut T) -> Option<Ordering> {
        ptr_words(*self as *const T).partial_cmp(&ptr_words(*other as *const T))
    }
}
impl<T: crate::marker::PointeeSized> Ord for *mut T {
    fn cmp(&self, other: &*mut T) -> Ordering {
        ptr_words(*self as *const T).cmp(&ptr_words(*other as *const T))
    }
}
impl<T: crate::marker::PointeeSized> Clone for *const T {
    fn clone(&self) -> *const T {
        *self
    }
}
impl<T: crate::marker::PointeeSized> Copy for *const T {}
impl<T: crate::marker::PointeeSized> Clone for *mut T {
    fn clone(&self) -> *mut T {
        *self
    }
}
impl<T: crate::marker::PointeeSized> Copy for *mut T {}
