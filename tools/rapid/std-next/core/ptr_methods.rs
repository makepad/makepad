//! Inherent methods of `*const T` and `*mut T` (Rapid only: impls on primitive types).
//! Address arithmetic is `wrapping`-exact like core's; methods that need `T: Sized` live in
//! the `impl<T>` blocks, address-only ones in `impl<T: ?Sized>`.

use crate::cmp::Ordering;
use crate::intrinsics_mem as im;

#[cfg(not(rapid_check))]
impl<T: ?Sized> *const T {
    pub fn is_null(self) -> bool {
        (self as *const u8 as usize) == 0
    }
    pub fn cast<U>(self) -> *const U {
        self as *const U
    }
    pub fn cast_mut(self) -> *mut T {
        self as *mut T
    }
    pub fn addr(self) -> usize {
        self as *const u8 as usize
    }
    pub unsafe fn as_ref<'a>(self) -> Option<&'a T> {
        if self.is_null() {
            None
        } else {
            Some(&*self)
        }
    }
    pub fn byte_add(self, count: usize) -> *const T {
        self.wrapping_byte_add(count)
    }
    pub fn byte_sub(self, count: usize) -> *const T {
        self.wrapping_byte_sub(count)
    }
    pub fn byte_offset(self, count: isize) -> *const T {
        self.wrapping_byte_add(count as usize)
    }
    pub fn wrapping_byte_add(self, count: usize) -> *const T {
        let mut p = self;
        let a = &mut p as *mut *const T as *mut usize;
        unsafe { *a = (*a).wrapping_add(count) };
        p
    }
    pub fn wrapping_byte_sub(self, count: usize) -> *const T {
        let mut p = self;
        let a = &mut p as *mut *const T as *mut usize;
        unsafe { *a = (*a).wrapping_sub(count) };
        p
    }
}

#[cfg(not(rapid_check))]
impl<T> *const T {
    pub unsafe fn add(self, count: usize) -> *const T {
        im::ptr_add(self, count)
    }
    pub unsafe fn sub(self, count: usize) -> *const T {
        ((self as usize).wrapping_sub(count.wrapping_mul(im::size_of::<T>()))) as *const T
    }
    pub unsafe fn offset(self, count: isize) -> *const T {
        ((self as usize).wrapping_add((count as usize).wrapping_mul(im::size_of::<T>()))) as *const T
    }
    pub fn wrapping_add(self, count: usize) -> *const T {
        ((self as usize).wrapping_add(count.wrapping_mul(unsafe { im::size_of::<T>() }))) as *const T
    }
    pub fn wrapping_sub(self, count: usize) -> *const T {
        ((self as usize).wrapping_sub(count.wrapping_mul(unsafe { im::size_of::<T>() }))) as *const T
    }
    pub fn wrapping_offset(self, count: isize) -> *const T {
        ((self as usize).wrapping_add((count as usize).wrapping_mul(unsafe { im::size_of::<T>() }))) as *const T
    }
    /// Distance in elements; both pointers into the same allocation.
    pub unsafe fn offset_from(self, origin: *const T) -> isize {
        let size = im::size_of::<T>();
        if size == 0 {
            crate::panicking::panic_str("assertion failed: 0 < pointee_size && pointee_size <= isize::MAX as usize");
        }
        ((self as usize).wrapping_sub(origin as usize) as isize) / (size as isize)
    }
    pub unsafe fn offset_from_unsigned(self, origin: *const T) -> usize {
        self.offset_from(origin) as usize
    }
    pub unsafe fn read(self) -> T {
        im::ptr_read(self)
    }
    pub unsafe fn read_unaligned(self) -> T {
        crate::ptr::read_unaligned(self)
    }
    pub unsafe fn read_volatile(self) -> T {
        crate::ptr::read_volatile(self)
    }
    pub unsafe fn copy_to(self, dest: *mut T, count: usize) {
        im::copy(self, dest, count)
    }
    pub unsafe fn copy_to_nonoverlapping(self, dest: *mut T, count: usize) {
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

#[cfg(not(rapid_check))]
impl<T> *const [T] {
    pub fn len(self) -> usize {
        unsafe { im::slice_len(&*self) }
    }
    pub fn is_empty(self) -> bool {
        self.len() == 0
    }
    pub fn as_ptr(self) -> *const T {
        self as *const T
    }
}

#[cfg(not(rapid_check))]
impl<T: ?Sized> *mut T {
    pub fn is_null(self) -> bool {
        (self as *const u8 as usize) == 0
    }
    pub fn cast<U>(self) -> *mut U {
        self as *mut U
    }
    pub fn cast_const(self) -> *const T {
        self as *const T
    }
    pub fn addr(self) -> usize {
        self as *const u8 as usize
    }
    pub unsafe fn as_ref<'a>(self) -> Option<&'a T> {
        if self.is_null() {
            None
        } else {
            Some(&*self)
        }
    }
    pub unsafe fn as_mut<'a>(self) -> Option<&'a mut T> {
        if self.is_null() {
            None
        } else {
            Some(&mut *self)
        }
    }
    pub fn byte_add(self, count: usize) -> *mut T {
        self.wrapping_byte_add(count)
    }
    pub fn byte_sub(self, count: usize) -> *mut T {
        self.wrapping_byte_sub(count)
    }
    pub fn byte_offset(self, count: isize) -> *mut T {
        self.wrapping_byte_add(count as usize)
    }
    pub fn wrapping_byte_add(self, count: usize) -> *mut T {
        let mut p = self;
        let a = &mut p as *mut *mut T as *mut usize;
        unsafe { *a = (*a).wrapping_add(count) };
        p
    }
    pub fn wrapping_byte_sub(self, count: usize) -> *mut T {
        let mut p = self;
        let a = &mut p as *mut *mut T as *mut usize;
        unsafe { *a = (*a).wrapping_sub(count) };
        p
    }
    pub unsafe fn drop_in_place(self) {
        im::drop_in_place(self)
    }
}

#[cfg(not(rapid_check))]
impl<T> *mut T {
    pub unsafe fn add(self, count: usize) -> *mut T {
        im::ptr_add_mut(self, count)
    }
    pub unsafe fn sub(self, count: usize) -> *mut T {
        ((self as usize).wrapping_sub(count.wrapping_mul(im::size_of::<T>()))) as *mut T
    }
    pub unsafe fn offset(self, count: isize) -> *mut T {
        ((self as usize).wrapping_add((count as usize).wrapping_mul(im::size_of::<T>()))) as *mut T
    }
    pub fn wrapping_add(self, count: usize) -> *mut T {
        ((self as usize).wrapping_add(count.wrapping_mul(unsafe { im::size_of::<T>() }))) as *mut T
    }
    pub fn wrapping_sub(self, count: usize) -> *mut T {
        ((self as usize).wrapping_sub(count.wrapping_mul(unsafe { im::size_of::<T>() }))) as *mut T
    }
    pub fn wrapping_offset(self, count: isize) -> *mut T {
        ((self as usize).wrapping_add((count as usize).wrapping_mul(unsafe { im::size_of::<T>() }))) as *mut T
    }
    pub unsafe fn offset_from(self, origin: *const T) -> isize {
        (self as *const T).offset_from(origin)
    }
    pub unsafe fn offset_from_unsigned(self, origin: *const T) -> usize {
        (self as *const T).offset_from(origin) as usize
    }
    pub unsafe fn read(self) -> T {
        im::ptr_read(self as *const T)
    }
    pub unsafe fn read_unaligned(self) -> T {
        crate::ptr::read_unaligned(self as *const T)
    }
    pub unsafe fn read_volatile(self) -> T {
        crate::ptr::read_volatile(self as *const T)
    }
    pub unsafe fn write(self, val: T) {
        im::ptr_write(self, val)
    }
    pub unsafe fn write_unaligned(self, val: T) {
        crate::ptr::write_unaligned(self, val)
    }
    pub unsafe fn write_volatile(self, val: T) {
        crate::ptr::write_volatile(self, val)
    }
    pub unsafe fn write_bytes(self, val: u8, count: usize) {
        im::write_bytes(self, val, count)
    }
    pub unsafe fn replace(self, src: T) -> T {
        crate::ptr::replace(self, src)
    }
    pub unsafe fn swap(self, with: *mut T) {
        crate::ptr::swap(self, with)
    }
    pub unsafe fn copy_from(self, src: *const T, count: usize) {
        im::copy(src, self, count)
    }
    pub unsafe fn copy_from_nonoverlapping(self, src: *const T, count: usize) {
        im::copy_nonoverlapping(src, self, count)
    }
    pub unsafe fn copy_to(self, dest: *mut T, count: usize) {
        im::copy(self as *const T, dest, count)
    }
    pub unsafe fn copy_to_nonoverlapping(self, dest: *mut T, count: usize) {
        im::copy_nonoverlapping(self as *const T, dest, count)
    }
    pub fn is_aligned(self) -> bool {
        (self as usize) % unsafe { im::align_of::<T>() } == 0
    }
    pub fn align_offset(self, align: usize) -> usize {
        (self as *const T).align_offset(align)
    }
}

#[cfg(not(rapid_check))]
impl<T> *mut [T] {
    pub fn len(self) -> usize {
        unsafe { im::slice_len(&*self) }
    }
    pub fn is_empty(self) -> bool {
        self.len() == 0
    }
    pub fn as_mut_ptr(self) -> *mut T {
        self as *mut T
    }
}

// Raw pointers compare by address (fat pointers: address, then metadata as core; Rapid
// compares the address part).
impl<T: ?Sized> PartialEq for *const T {
    fn eq(&self, other: &*const T) -> bool {
        (*self as *const u8) == (*other as *const u8)
    }
}
impl<T: ?Sized> Eq for *const T {}
impl<T: ?Sized> PartialEq for *mut T {
    fn eq(&self, other: &*mut T) -> bool {
        (*self as *const u8) == (*other as *const u8)
    }
}
impl<T: ?Sized> Eq for *mut T {}
impl<T: ?Sized> PartialOrd for *const T {
    fn partial_cmp(&self, other: &*const T) -> Option<Ordering> {
        (*self as *const u8 as usize).partial_cmp(&(*other as *const u8 as usize))
    }
}
impl<T: ?Sized> Ord for *const T {
    fn cmp(&self, other: &*const T) -> Ordering {
        (*self as *const u8 as usize).cmp(&(*other as *const u8 as usize))
    }
}
impl<T: ?Sized> PartialOrd for *mut T {
    fn partial_cmp(&self, other: &*mut T) -> Option<Ordering> {
        (*self as *const u8 as usize).partial_cmp(&(*other as *const u8 as usize))
    }
}
impl<T: ?Sized> Ord for *mut T {
    fn cmp(&self, other: &*mut T) -> Ordering {
        (*self as *const u8 as usize).cmp(&(*other as *const u8 as usize))
    }
}
impl<T: ?Sized> Clone for *const T {
    fn clone(&self) -> *const T {
        *self
    }
}
impl<T: ?Sized> Copy for *const T {}
impl<T: ?Sized> Clone for *mut T {
    fn clone(&self) -> *mut T {
        *self
    }
}
impl<T: ?Sized> Copy for *mut T {}
