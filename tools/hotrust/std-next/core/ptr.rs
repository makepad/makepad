//! Raw pointer helpers and NonNull. Methods on `*const T` / `*mut T` are in ptr_methods.rs.

use crate::intrinsics_mem as im;

pub const fn null<T>() -> *const T {
    0 as *const T
}
pub const fn null_mut<T>() -> *mut T {
    0 as *mut T
}
pub const fn dangling<T>() -> *const T {
    // like core: the type's alignment as an address
    unsafe { im::align_of::<T>() as *const T }
}
pub const fn dangling_mut<T>() -> *mut T {
    unsafe { im::align_of::<T>() as *mut T }
}
pub const fn without_provenance<T>(addr: usize) -> *const T {
    addr as *const T
}
pub const fn without_provenance_mut<T>(addr: usize) -> *mut T {
    addr as *mut T
}
pub unsafe fn read<T>(src: *const T) -> T {
    im::ptr_read(src)
}
pub unsafe fn read_unaligned<T>(src: *const T) -> T {
    let mut tmp = crate::mem::MaybeUninit::<T>::uninit();
    im::copy_nonoverlapping(src as *const u8, tmp.as_mut_ptr() as *mut u8, im::size_of::<T>());
    tmp.assume_init()
}
pub unsafe fn read_volatile<T>(src: *const T) -> T {
    im::ptr_read(im::black_box(src))
}
pub unsafe fn write<T>(dst: *mut T, src: T) {
    im::ptr_write(dst, src)
}
pub unsafe fn write_unaligned<T>(dst: *mut T, src: T) {
    let tmp = crate::mem::ManuallyDrop::new(src);
    im::copy_nonoverlapping(&*tmp as *const T as *const u8, dst as *mut u8, im::size_of::<T>());
}
pub unsafe fn write_volatile<T>(dst: *mut T, src: T) {
    im::ptr_write(im::black_box(dst), src)
}
pub unsafe fn write_bytes<T>(dst: *mut T, val: u8, count: usize) {
    im::write_bytes(dst, val, count)
}
pub unsafe fn copy<T>(src: *const T, dst: *mut T, count: usize) {
    im::copy(src, dst, count)
}
pub unsafe fn copy_nonoverlapping<T>(src: *const T, dst: *mut T, count: usize) {
    im::copy_nonoverlapping(src, dst, count)
}
pub unsafe fn drop_in_place<T: ?Sized>(to_drop: *mut T) {
    im::drop_in_place(to_drop)
}
pub unsafe fn replace<T>(dst: *mut T, src: T) -> T {
    let old = im::ptr_read(dst as *const T);
    im::ptr_write(dst, src);
    old
}
pub unsafe fn swap<T>(x: *mut T, y: *mut T) {
    let t = im::ptr_read(x as *const T);
    im::copy(y as *const T, x, 1);
    im::ptr_write(y, t);
}
pub unsafe fn swap_nonoverlapping<T>(x: *mut T, y: *mut T, count: usize) {
    let mut i = 0;
    while i < count {
        swap(im::ptr_add_mut(x, i), im::ptr_add_mut(y, i));
        i += 1;
    }
}
/// Address equality (fat pointers: address only, as `addr_eq`).
pub fn eq<T: ?Sized>(a: *const T, b: *const T) -> bool {
    a as *const u8 == b as *const u8
}
pub fn addr_eq<T: ?Sized, U: ?Sized>(p: *const T, q: *const U) -> bool {
    p as *const u8 == q as *const u8
}
pub fn slice_from_raw_parts<T>(data: *const T, len: usize) -> *const [T] {
    unsafe { im::slice_from_raw(data, len) as *const [T] }
}
pub fn slice_from_raw_parts_mut<T>(data: *mut T, len: usize) -> *mut [T] {
    unsafe { im::slice_from_raw_mut(data, len) as *mut [T] }
}
pub fn hash<T: ?Sized, S: crate::hash::Hasher>(hashee: *const T, into: &mut S) {
    use crate::hash::Hash;
    (hashee as *const u8 as usize).hash(into);
}
pub fn from_ref<T: ?Sized>(r: &T) -> *const T {
    r
}
pub fn from_mut<T: ?Sized>(r: &mut T) -> *mut T {
    r
}

// ---------------------------------------------------------------- NonNull

/// A non-null `*mut T` (gives `Option<NonNull<T>>` the null niche).
#[repr(transparent)]
pub struct NonNull<T: ?Sized> {
    pointer: *const T,
}

impl<T> NonNull<T> {
    pub const fn dangling() -> NonNull<T> {
        NonNull { pointer: dangling::<T>() }
    }
    pub fn as_ptr(self) -> *mut T {
        self.pointer as *mut T
    }
    pub unsafe fn add(self, count: usize) -> NonNull<T> {
        NonNull { pointer: im::ptr_add(self.pointer, count) }
    }
    pub fn cast<U>(self) -> NonNull<U> {
        NonNull { pointer: self.pointer as *const U }
    }
    pub unsafe fn read(self) -> T {
        im::ptr_read(self.pointer)
    }
    pub unsafe fn write(self, val: T) {
        im::ptr_write(self.pointer as *mut T, val)
    }
}

impl<T: ?Sized> NonNull<T> {
    pub const unsafe fn new_unchecked(ptr: *mut T) -> NonNull<T> {
        NonNull { pointer: ptr as *const T }
    }
    pub fn new(ptr: *mut T) -> Option<NonNull<T>> {
        if (ptr as *const u8).is_null() {
            None
        } else {
            Some(NonNull { pointer: ptr as *const T })
        }
    }
    pub fn from_ref(r: &T) -> NonNull<T> {
        NonNull { pointer: r as *const T }
    }
    pub fn from_mut(r: &mut T) -> NonNull<T> {
        NonNull { pointer: r as *mut T as *const T }
    }
    pub unsafe fn as_ref<'a>(&self) -> &'a T {
        &*self.pointer
    }
    pub unsafe fn as_mut<'a>(&mut self) -> &'a mut T {
        &mut *(self.pointer as *mut T)
    }
    pub fn as_ptr_unsized(self) -> *mut T {
        self.pointer as *mut T
    }
}

impl<T: ?Sized> Clone for NonNull<T> {
    fn clone(&self) -> NonNull<T> {
        *self
    }
}
impl<T: ?Sized> Copy for NonNull<T> {}
impl<T: ?Sized> PartialEq for NonNull<T> {
    fn eq(&self, other: &NonNull<T>) -> bool {
        self.pointer as *const u8 == other.pointer as *const u8
    }
}
impl<T: ?Sized> Eq for NonNull<T> {}
impl<T: ?Sized> crate::hash::Hash for NonNull<T> {
    fn hash<H: crate::hash::Hasher>(&self, state: &mut H) {
        (self.pointer as *const u8 as usize).hash(state)
    }
}
impl<T: ?Sized> crate::fmt::Debug for NonNull<T> {
    fn fmt(&self, f: &mut crate::fmt::Formatter<'_>) -> crate::fmt::Result {
        crate::fmt::Pointer::fmt(&(self.pointer as *const u8), f)
    }
}
impl<T: ?Sized> crate::fmt::Pointer for NonNull<T> {
    fn fmt(&self, f: &mut crate::fmt::Formatter<'_>) -> crate::fmt::Result {
        crate::fmt::Pointer::fmt(&(self.pointer as *const u8), f)
    }
}
impl<'a, T: ?Sized> From<&'a T> for NonNull<T> {
    fn from(r: &'a T) -> NonNull<T> {
        NonNull { pointer: r as *const T }
    }
}
impl<'a, T: ?Sized> From<&'a mut T> for NonNull<T> {
    fn from(r: &'a mut T) -> NonNull<T> {
        NonNull { pointer: r as *mut T as *const T }
    }
}
unsafe impl<T: ?Sized> Send for NonNull<T> where T: Send {}
