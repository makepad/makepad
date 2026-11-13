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
pub const fn needs_drop<T: ?Sized>() -> bool {
    unsafe { im::needs_drop::<T>() }
}
#[rustc_diagnostic_item = "mem_forget"]
pub const fn forget<T>(t: T) {
    unsafe { im::forget(t) }
}
#[rustc_diagnostic_item = "mem_drop"]
pub fn drop<T>(_x: T) {}

pub const fn replace<T>(dest: &mut T, src: T) -> T {
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
/// rustc 1.100.0 (mem_conjure_zst): the value of a zero-sized type, which has nothing to initialize. The caller
/// guarantees that `T` is inhabited (core/src/mem/mod.rs; its run-time message).
pub const unsafe fn conjure_zst<T>() -> T {
    if size_of::<T>() != 0 {
        panic!("mem::conjure_zst invoked on type {}, which is not zero-sized", crate::any::type_name::<T>());
    }
    MaybeUninit::uninit().assume_init()
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

/// Opaque enum discriminant (`mem::discriminant`): library/core/src/mem/mod.rs, held as the
/// discriminant's own type (the enum's repr int, isize by default; u8 for a type that is no
/// enum), so its hash, comparison and size are rustc's.
pub struct Discriminant<T>(<T as crate::marker::DiscriminantKind>::Discriminant);

pub const fn discriminant<T>(v: &T) -> Discriminant<T> {
    // the intrinsic's i64, sign-extended to the widest discriminant type: its low bytes are the
    // value as the discriminant type (the JIT's targets are little-endian)
    let wide = unsafe { im::discriminant_value(v) } as i128;
    Discriminant(unsafe { transmute_copy::<i128, <T as crate::marker::DiscriminantKind>::Discriminant>(&wide) })
}

impl<T> Clone for Discriminant<T> {
    fn clone(&self) -> Discriminant<T> {
        *self
    }
}
impl<T> Copy for Discriminant<T> {}
impl<T> PartialEq for Discriminant<T> {
    fn eq(&self, o: &Discriminant<T>) -> bool {
        self.0 == o.0
    }
}
impl<T> Eq for Discriminant<T> {}
impl<T> crate::hash::Hash for Discriminant<T> {
    fn hash<H: crate::hash::Hasher>(&self, state: &mut H) {
        self.0.hash(state)
    }
}
impl<T> crate::fmt::Debug for Discriminant<T> {
    fn fmt(&self, f: &mut crate::fmt::Formatter<'_>) -> crate::fmt::Result {
        f.debug_tuple("Discriminant").field(&self.0).finish()
    }
}

// ---------------------------------------------------------------- ManuallyDrop

/// Inhibits the compiler from calling `T`'s destructor (the lang item has no drop glue); like real core a
/// struct, so `ManuallyDrop<dyn Trait>` is a dynamically sized type.
#[repr(transparent)]
#[lang = "manually_drop"]
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

impl<T: ?Sized> crate::ops::Deref for ManuallyDrop<T> {
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { &self.value }
    }
}
impl<T: ?Sized> crate::ops::DerefMut for ManuallyDrop<T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut self.value }
    }
}

// ---------------------------------------------------------------- MaybeUninit

#[repr(C)]
#[lang = "maybe_uninit"]
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

impl<T> crate::fmt::Debug for MaybeUninit<T> {
    fn fmt(&self, f: &mut crate::fmt::Formatter<'_>) -> crate::fmt::Result {
        let full_name = crate::any::type_name::<Self>();
        let prefix_len = full_name.find("MaybeUninit").unwrap();
        f.pad(&full_name[prefix_len..])
    }
}

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

// ---------------------------------------------------------------- DropGuard (rustc 1.100.0: drop_guard)

/// Calls `f` on the value when the guard is dropped, unless `dismiss` takes the value back
/// (core/src/mem/drop_guard.rs; `new` and `dismiss` are const only under the unstable const_drop_guard).
pub struct DropGuard<T, F>
where
    F: FnOnce(T),
{
    inner: ManuallyDrop<T>,
    f: ManuallyDrop<F>,
}

impl<T, F> DropGuard<T, F>
where
    F: FnOnce(T),
{
    #[must_use]
    pub fn new(inner: T, f: F) -> Self {
        Self { inner: ManuallyDrop::new(inner), f: ManuallyDrop::new(f) }
    }

    pub fn dismiss(guard: Self) -> T {
        // the guard's own drop must not run; the value is read before the closure is dropped, so a panicking
        // closure drop still unwinds through the value
        let mut guard = ManuallyDrop::new(guard);
        let value = unsafe { ManuallyDrop::take(&mut guard.inner) };
        unsafe { ManuallyDrop::drop(&mut guard.f) };
        value
    }
}

impl<T, F> crate::ops::Deref for DropGuard<T, F>
where
    F: FnOnce(T),
{
    type Target = T;

    fn deref(&self) -> &T {
        &self.inner
    }
}

impl<T, F> crate::ops::DerefMut for DropGuard<T, F>
where
    F: FnOnce(T),
{
    fn deref_mut(&mut self) -> &mut T {
        &mut self.inner
    }
}

impl<T, F> Drop for DropGuard<T, F>
where
    F: FnOnce(T),
{
    fn drop(&mut self) {
        let inner = unsafe { ManuallyDrop::take(&mut self.inner) };
        let f = unsafe { ManuallyDrop::take(&mut self.f) };
        f(inner);
    }
}

impl<T, F> crate::fmt::Debug for DropGuard<T, F>
where
    T: crate::fmt::Debug,
    F: FnOnce(T),
{
    fn fmt(&self, f: &mut crate::fmt::Formatter<'_>) -> crate::fmt::Result {
        crate::fmt::Debug::fmt(&**self, f)
    }
}
