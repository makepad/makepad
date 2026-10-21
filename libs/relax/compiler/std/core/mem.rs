//! Memory primitives: size/align queries, moves, ManuallyDrop, MaybeUninit, discriminants.

use crate::intrinsics_mem as im;

#[rustc_promotable]
pub const fn size_of<T>() -> usize {
    unsafe { im::size_of::<T>() }
}
pub const fn size_of_val<T: ?Sized>(val: &T) -> usize {
    unsafe { im::size_of_val(val) }
}
#[rustc_promotable]
pub const fn align_of<T>() -> usize {
    unsafe { im::align_of::<T>() }
}
pub const fn align_of_val<T: ?Sized>(val: &T) -> usize {
    unsafe { im::align_of_val(val) }
}
pub const fn needs_drop<T>() -> bool {
    unsafe { im::needs_drop::<T>() }
}
#[rustc_diagnostic_item = "mem_forget"]
pub const fn forget<T>(t: T) {
    unsafe { im::forget(t) }
}
#[rustc_diagnostic_item = "mem_drop"]
pub fn drop<T>(_x: T) {}

pub fn replace<T>(dest: &mut T, src: T) -> T {
    unsafe {
        let p = dest as *mut T;
        let old = im::ptr_read(p as *const T);
        im::ptr_write(p, src);
        old
    }
}
pub const fn swap<T>(x: &mut T, y: &mut T) {
    unsafe {
        let pa = x as *mut T;
        let pb = y as *mut T;
        let t = im::ptr_read(pa as *const T);
        im::ptr_write(pa, im::ptr_read(pb as *const T));
        im::ptr_write(pb, t);
    }
}
pub fn take<T: Default>(dest: &mut T) -> T {
    replace(dest, T::default())
}
#[rustc_diagnostic_item = "mem_zeroed"]
pub const unsafe fn zeroed<T>() -> T {
    im::zeroed::<T>()
}
#[deprecated(since = "1.39.0", note = "use `mem::MaybeUninit` instead")]
#[rustc_diagnostic_item = "mem_uninitialized"]
pub unsafe fn uninitialized<T>() -> T {
    MaybeUninit::<T>::uninit().assume_init()
}
#[rustc_diagnostic_item = "transmute"]
#[rustc_intrinsic]
pub const unsafe fn transmute<T, U>(e: T) -> U {
    im::transmute::<T, U>(e)
}
pub const unsafe fn transmute_copy<T, U>(src: &T) -> U {
    im::ptr_read(src as *const T as *const U)
}

/// Opaque enum discriminant (`mem::discriminant`).
pub struct Discriminant<T> {
    value: i64,
    _m: crate::marker::PhantomData<T>,
}

pub const fn discriminant<T>(v: &T) -> Discriminant<T> {
    Discriminant { value: unsafe { im::discriminant_value(v) }, _m: crate::marker::PhantomData }
}

impl<T> Clone for Discriminant<T> {
    fn clone(&self) -> Discriminant<T> {
        Discriminant { value: self.value, _m: crate::marker::PhantomData }
    }
}
impl<T> Copy for Discriminant<T> {}
impl<T> PartialEq for Discriminant<T> {
    fn eq(&self, o: &Discriminant<T>) -> bool {
        self.value == o.value
    }
}
impl<T> Eq for Discriminant<T> {}
impl<T> crate::hash::Hash for Discriminant<T> {
    fn hash<H: crate::hash::Hasher>(&self, state: &mut H) {
        self.value.hash(state)
    }
}
impl<T> crate::fmt::Debug for Discriminant<T> {
    fn fmt(&self, f: &mut crate::fmt::Formatter<'_>) -> crate::fmt::Result {
        f.debug_tuple("Discriminant").field(&self.value).finish()
    }
}

// ---------------------------------------------------------------- ManuallyDrop

/// Inhibits the compiler from calling `T`'s destructor (the lang item has no drop glue); like real core a
/// struct, so `ManuallyDrop<dyn Trait>` is a dynamically sized type.
#[repr(transparent)]
#[cfg_attr(not(relax_check), lang = "manually_drop")]
#[rustc_diagnostic_item = "ManuallyDrop"]
pub struct ManuallyDrop<T: ?Sized> {
    value: T,
}

impl<T> ManuallyDrop<T> {
    pub const fn new(value: T) -> ManuallyDrop<T> {
        ManuallyDrop { value }
    }
    pub const fn into_inner(slot: ManuallyDrop<T>) -> T {
        unsafe { im::ptr_read(&slot.value as *const T) }
    }
    pub const unsafe fn take(slot: &mut ManuallyDrop<T>) -> T {
        im::ptr_read(&slot.value as *const T)
    }
    pub const unsafe fn drop(slot: &mut ManuallyDrop<T>) {
        im::drop_in_place(&mut slot.value as *mut T)
    }
}

impl<T: Clone> Clone for ManuallyDrop<T> {
    fn clone(&self) -> ManuallyDrop<T> {
        ManuallyDrop::new(unsafe { (*(&self.value as *const T)).clone() })
    }
}
impl<T: Copy> Copy for ManuallyDrop<T> {}
impl<T: Default> Default for ManuallyDrop<T> {
    fn default() -> ManuallyDrop<T> {
        ManuallyDrop::new(T::default())
    }
}
impl<T: ?Sized> crate::marker::StructuralPartialEq for ManuallyDrop<T> {}
impl<T: PartialEq> PartialEq for ManuallyDrop<T> {
    fn eq(&self, other: &ManuallyDrop<T>) -> bool {
        unsafe { self.value == other.value }
    }
}
impl<T: Eq> Eq for ManuallyDrop<T> {}
impl<T: crate::hash::Hash> crate::hash::Hash for ManuallyDrop<T> {
    fn hash<H: crate::hash::Hasher>(&self, state: &mut H) {
        unsafe { self.value.hash(state) }
    }
}
impl<T: crate::fmt::Debug> crate::fmt::Debug for ManuallyDrop<T> {
    fn fmt(&self, f: &mut crate::fmt::Formatter<'_>) -> crate::fmt::Result {
        f.debug_struct("ManuallyDrop").field("value", unsafe { &self.value }).finish()
    }
}

impl<T> crate::ops::Deref for ManuallyDrop<T> {
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { &self.value }
    }
}
impl<T> crate::ops::DerefMut for ManuallyDrop<T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut self.value }
    }
}

// ---------------------------------------------------------------- MaybeUninit

#[repr(C)]
#[cfg_attr(not(relax_check), lang = "maybe_uninit")]
pub union MaybeUninit<T> {
    uninit: (),
    value: ManuallyDrop<T>,
}

impl<T: Copy> Clone for MaybeUninit<T> {
    fn clone(&self) -> MaybeUninit<T> {
        *self
    }
}
impl<T: Copy> Copy for MaybeUninit<T> {}

impl<T> MaybeUninit<T> {
    pub const fn new(val: T) -> MaybeUninit<T> {
        MaybeUninit { value: ManuallyDrop::new(val) }
    }
    #[rustc_diagnostic_item = "maybe_uninit_uninit"]
    pub const fn uninit() -> MaybeUninit<T> {
        MaybeUninit { uninit: () }
    }
    #[rustc_diagnostic_item = "maybe_uninit_zeroed"]
    pub const fn zeroed() -> MaybeUninit<T> {
        let mut u = MaybeUninit::<T>::uninit();
        unsafe { im::write_bytes(u.as_mut_ptr(), 0, 1) };
        u
    }
    pub const fn write(&mut self, val: T) -> &mut T {
        unsafe {
            im::ptr_write(self.as_mut_ptr(), val);
            &mut *self.as_mut_ptr()
        }
    }
    #[rustc_as_ptr]
    pub const fn as_ptr(&self) -> *const T {
        self as *const MaybeUninit<T> as *const T
    }
    #[rustc_as_ptr]
    pub const fn as_mut_ptr(&mut self) -> *mut T {
        self as *mut MaybeUninit<T> as *mut T
    }
    #[rustc_diagnostic_item = "assume_init"]
    pub const unsafe fn assume_init(self) -> T {
        ManuallyDrop::into_inner(self.value)
    }
    pub const unsafe fn assume_init_read(&self) -> T {
        im::ptr_read(self.as_ptr())
    }
    pub const unsafe fn assume_init_ref(&self) -> &T {
        &*self.as_ptr()
    }
    pub const unsafe fn assume_init_mut(&mut self) -> &mut T {
        &mut *self.as_mut_ptr()
    }
    pub const unsafe fn assume_init_drop(&mut self) {
        im::drop_in_place(self.as_mut_ptr())
    }
}
