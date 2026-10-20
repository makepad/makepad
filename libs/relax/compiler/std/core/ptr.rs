// ported from library/core/src/ptr/non_null.rs @8d1a764
//! Raw pointer helpers and NonNull. Methods on `*const T` / `*mut T` are in ptr_methods.rs.

use crate::intrinsics_mem as im;

#[rustc_promotable]
#[rustc_diagnostic_item = "ptr_null"]
pub const fn null<T>() -> *const T {
    0 as *const T
}
#[rustc_promotable]
#[rustc_diagnostic_item = "ptr_null_mut"]
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
#[rustc_diagnostic_item = "ptr_read"]
pub unsafe fn read<T>(src: *const T) -> T {
    im::ptr_read(src)
}
#[rustc_diagnostic_item = "ptr_read_unaligned"]
pub const unsafe fn read_unaligned<T>(src: *const T) -> T {
    let mut tmp = crate::mem::MaybeUninit::<T>::uninit();
    im::copy_nonoverlapping(src as *const u8, tmp.as_mut_ptr() as *mut u8, im::size_of::<T>());
    tmp.assume_init()
}
pub unsafe fn read_volatile<T>(src: *const T) -> T {
    im::volatile_load(src)
}
#[rustc_diagnostic_item = "ptr_write"]
pub unsafe fn write<T>(dst: *mut T, src: T) {
    im::ptr_write(dst, src)
}
#[rustc_diagnostic_item = "ptr_write_unaligned"]
pub const unsafe fn write_unaligned<T>(dst: *mut T, src: T) {
    let tmp = crate::mem::ManuallyDrop::new(src);
    im::copy_nonoverlapping(&*tmp as *const T as *const u8, dst as *mut u8, im::size_of::<T>());
}
#[rustc_diagnostic_item = "ptr_write_volatile"]
pub unsafe fn write_volatile<T>(dst: *mut T, src: T) {
    im::volatile_store(dst, src)
}
#[rustc_diagnostic_item = "ptr_write_bytes"]
pub const unsafe fn write_bytes<T>(dst: *mut T, val: u8, count: usize) {
    im::write_bytes(dst, val, count)
}
#[rustc_diagnostic_item = "ptr_copy"]
pub unsafe fn copy<T>(src: *const T, dst: *mut T, count: usize) {
    im::copy(src, dst, count)
}
#[rustc_diagnostic_item = "ptr_copy_nonoverlapping"]
pub const unsafe fn copy_nonoverlapping<T>(src: *const T, dst: *mut T, count: usize) {
    im::copy_nonoverlapping(src, dst, count)
}
#[cfg_attr(not(relax_check), lang = "drop_glue")]
pub const unsafe fn drop_in_place<T: crate::marker::PointeeSized>(to_drop: *mut T) {
    im::drop_in_place(to_drop)
}
#[rustc_diagnostic_item = "ptr_replace"]
pub unsafe fn replace<T>(dst: *mut T, src: T) -> T {
    let old = im::ptr_read(dst as *const T);
    im::ptr_write(dst, src);
    old
}
#[rustc_diagnostic_item = "ptr_swap"]
pub const unsafe fn swap<T>(x: *mut T, y: *mut T) {
    let t = im::ptr_read(x as *const T);
    im::copy(y as *const T, x, 1);
    im::ptr_write(y, t);
}
#[rustc_diagnostic_item = "ptr_swap_nonoverlapping"]
pub const unsafe fn swap_nonoverlapping<T>(x: *mut T, y: *mut T, count: usize) {
    let mut i = 0;
    while i < count {
        swap(im::ptr_add_mut(x, i), im::ptr_add_mut(y, i));
        i += 1;
    }
}
/// Address equality (fat pointers: address only, as `addr_eq`).
pub fn eq<T: crate::marker::PointeeSized>(a: *const T, b: *const T) -> bool {
    crate::ptr_methods::ptr_words(a) == crate::ptr_methods::ptr_words(b)
}
/// Whether two fn pointers have the same address (real core: may be true for different fns
/// merged by the compiler and false for one fn with several copies).
pub fn fn_addr_eq<T: crate::ops::FnPtr, U: crate::ops::FnPtr>(f: T, g: U) -> bool {
    f.addr() == g.addr()
}

pub fn addr_eq<T: crate::marker::PointeeSized, U: crate::marker::PointeeSized>(p: *const T, q: *const U) -> bool {
    p as *const u8 == q as *const u8
}
pub const fn slice_from_raw_parts<T>(data: *const T, len: usize) -> *const [T] {
    unsafe { im::slice_from_raw(data, len) as *const [T] }
}
pub const fn slice_from_raw_parts_mut<T>(data: *mut T, len: usize) -> *mut [T] {
    unsafe { im::slice_from_raw_mut(data, len) as *mut [T] }
}
pub fn hash<T: crate::marker::PointeeSized, S: crate::hash::Hasher>(hashee: *const T, into: &mut S) {
    use crate::hash::Hash;
    (hashee as *const u8 as usize).hash(into);
}
#[rustc_diagnostic_item = "ptr_from_ref"]
#[rustc_never_returns_null_ptr]
pub const fn from_ref<T: crate::marker::PointeeSized>(r: &T) -> *const T {
    r
}
#[rustc_never_returns_null_ptr]
pub const fn from_mut<T: crate::marker::PointeeSized>(r: &mut T) -> *mut T {
    r
}

// ---------------------------------------------------------------- NonNull

/// A non-null `*mut T` (gives `Option<NonNull<T>>` the null niche).
#[repr(transparent)]
#[rustc_layout_scalar_valid_range_start(1)]
#[rustc_nonnull_optimization_guaranteed]
#[rustc_diagnostic_item = "NonNull"]
#[cfg_attr(not(relax_check), lang = "non_null")]
pub struct NonNull<T: crate::marker::PointeeSized> {
    pointer: *const T,
}

impl<T: crate::marker::PointeeSized + crate::marker::Unsize<U>, U: crate::marker::PointeeSized> crate::ops::CoerceUnsized<NonNull<U>> for NonNull<T> {}
impl<T> NonNull<T> {
    pub const fn dangling() -> NonNull<T> {
        NonNull { pointer: dangling::<T>() }
    }
    pub const unsafe fn add(self, count: usize) -> NonNull<T> {
        NonNull { pointer: im::ptr_add(self.pointer, count) }
    }
    pub const unsafe fn read(self) -> T {
        im::ptr_read(self.pointer)
    }
    pub const unsafe fn write(self, val: T) {
        im::ptr_write(self.pointer as *mut T, val)
    }
}

impl<T: crate::marker::PointeeSized> NonNull<T> {
    pub const fn cast<U>(self) -> NonNull<U> {
        NonNull { pointer: self.pointer as *const U }
    }
    pub fn addr(self) -> crate::num::NonZeroUsize {
        unsafe { crate::num::NonZeroUsize::new_unchecked(self.pointer as *const u8 as usize) }
    }
    pub const unsafe fn new_unchecked(ptr: *mut T) -> NonNull<T> {
        NonNull { pointer: ptr as *const T }
    }
    pub const fn new(ptr: *mut T) -> Option<NonNull<T>> {
        if (ptr as *const u8).is_null() {
            None
        } else {
            Some(NonNull { pointer: ptr as *const T })
        }
    }
    pub const fn from_ref(r: &T) -> NonNull<T> {
        NonNull { pointer: r as *const T }
    }
    pub const fn from_mut(r: &mut T) -> NonNull<T> {
        NonNull { pointer: r as *mut T as *const T }
    }
    pub const unsafe fn as_ref<'a>(&self) -> &'a T {
        &*self.pointer
    }
    pub const unsafe fn as_mut<'a>(&mut self) -> &'a mut T {
        &mut *(self.pointer as *mut T)
    }
    #[rustc_never_returns_null_ptr]
    pub const fn as_ptr(self) -> *mut T {
        self.pointer as *mut T
    }
}

impl<T: crate::marker::PointeeSized> Clone for NonNull<T> {
    fn clone(&self) -> NonNull<T> {
        *self
    }
}
impl<T: crate::marker::PointeeSized> Copy for NonNull<T> {}
impl<T: crate::marker::PointeeSized> PartialEq for NonNull<T> {
    fn eq(&self, other: &NonNull<T>) -> bool {
        crate::ptr_methods::ptr_words(self.pointer) == crate::ptr_methods::ptr_words(other.pointer)
    }
}
impl<T: crate::marker::PointeeSized> Eq for NonNull<T> {}
impl<T: crate::marker::PointeeSized> PartialOrd for NonNull<T> {
    fn partial_cmp(&self, other: &NonNull<T>) -> Option<crate::cmp::Ordering> {
        (self.pointer as *const T).partial_cmp(&(other.pointer as *const T))
    }
}
impl<T: crate::marker::PointeeSized> Ord for NonNull<T> {
    fn cmp(&self, other: &NonNull<T>) -> crate::cmp::Ordering {
        (self.pointer as *const T).cmp(&(other.pointer as *const T))
    }
}
impl<T: crate::marker::PointeeSized> crate::hash::Hash for NonNull<T> {
    fn hash<H: crate::hash::Hasher>(&self, state: &mut H) {
        (self.pointer as *const u8 as usize).hash(state)
    }
}
impl<T: crate::marker::PointeeSized> crate::fmt::Debug for NonNull<T> {
    fn fmt(&self, f: &mut crate::fmt::Formatter<'_>) -> crate::fmt::Result {
        crate::fmt::Pointer::fmt(&(self.pointer as *const u8), f)
    }
}
impl<T: crate::marker::PointeeSized> crate::fmt::Pointer for NonNull<T> {
    fn fmt(&self, f: &mut crate::fmt::Formatter<'_>) -> crate::fmt::Result {
        crate::fmt::Pointer::fmt(&(self.pointer as *const u8), f)
    }
}
impl<'a, T: crate::marker::PointeeSized> From<&'a T> for NonNull<T> {
    fn from(r: &'a T) -> NonNull<T> {
        NonNull { pointer: r as *const T }
    }
}
impl<'a, T: crate::marker::PointeeSized> From<&'a mut T> for NonNull<T> {
    fn from(r: &'a mut T) -> NonNull<T> {
        NonNull { pointer: r as *mut T as *const T }
    }
}
// library/core/src/ptr/non_null.rs:81-86
impl<T: crate::marker::PointeeSized> !Send for NonNull<T> {}
impl<T: crate::marker::PointeeSized> !Sync for NonNull<T> {}

// ported from library/core/src/ptr/metadata.rs:56-68,158-169 @8d1a764
#[cfg_attr(not(relax_check), lang = "pointee_trait")]
pub trait Pointee: crate::marker::PointeeSized {
    #[cfg_attr(not(relax_check), lang = "metadata_type")]
    type Metadata: crate::fmt::Debug + crate::marker::Copy + crate::marker::Send + crate::marker::Sync + crate::cmp::Ord + crate::hash::Hash + crate::marker::Unpin + crate::marker::Freeze;
}
#[cfg_attr(not(relax_check), lang = "dyn_metadata")]
pub struct DynMetadata<Dyn: crate::marker::PointeeSized> {
    _vtable_ptr: NonNull<VTable>,
    _phantom: crate::marker::PhantomData<Dyn>,
}
unsafe extern "C" { type VTable; }
// library/core/src/ptr/metadata.rs:216-281: these impls do not constrain Dyn.
unsafe impl<Dyn: crate::marker::PointeeSized> crate::marker::Send for DynMetadata<Dyn> {}
unsafe impl<Dyn: crate::marker::PointeeSized> crate::marker::Sync for DynMetadata<Dyn> {}
impl<Dyn: crate::marker::PointeeSized> crate::marker::Unpin for DynMetadata<Dyn> {}
impl<Dyn: crate::marker::PointeeSized> crate::marker::Copy for DynMetadata<Dyn> {}
impl<Dyn: crate::marker::PointeeSized> crate::clone::Clone for DynMetadata<Dyn> {
    fn clone(&self) -> Self { *self }
}
unsafe impl<Dyn: crate::marker::PointeeSized> crate::clone::TrivialClone for DynMetadata<Dyn> {}
impl<Dyn: crate::marker::PointeeSized> crate::fmt::Debug for DynMetadata<Dyn> {
    fn fmt(&self, f: &mut crate::fmt::Formatter<'_>) -> crate::fmt::Result {
        f.debug_tuple("DynMetadata").field(&self._vtable_ptr.as_ptr()).finish()
    }
}
impl<Dyn: crate::marker::PointeeSized> crate::cmp::Eq for DynMetadata<Dyn> {}
impl<Dyn: crate::marker::PointeeSized> crate::cmp::PartialEq for DynMetadata<Dyn> {
    fn eq(&self, other: &Self) -> bool { self._vtable_ptr.as_ptr() == other._vtable_ptr.as_ptr() }
}
impl<Dyn: crate::marker::PointeeSized> crate::cmp::Ord for DynMetadata<Dyn> {
    fn cmp(&self, other: &Self) -> crate::cmp::Ordering { self._vtable_ptr.as_ptr().cmp(&other._vtable_ptr.as_ptr()) }
}
impl<Dyn: crate::marker::PointeeSized> crate::cmp::PartialOrd for DynMetadata<Dyn> {
    fn partial_cmp(&self, other: &Self) -> Option<crate::cmp::Ordering> { Some(self.cmp(other)) }
}
impl<Dyn: crate::marker::PointeeSized> crate::hash::Hash for DynMetadata<Dyn> {
    fn hash<H: crate::hash::Hasher>(&self, state: &mut H) { self._vtable_ptr.as_ptr().hash(state); }
}
