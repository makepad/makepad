//! Rc / Weak: single-threaded reference counting.
//!
//! Layout (Relax's own, R14): one heap block = a header (strong, weak, block size, block
//! align) followed by the value at the next multiple of the value's alignment. An `Rc<T>` is
//! a single (possibly fat) pointer to the value; the header is found from the value's
//! alignment (header_of). So `Rc<str>`, `Rc<[T]>` and `Rc<dyn Trait>` need no custom DST and
//! unsizing an `Rc<T>` to `Rc<dyn Tr>` is the plain pointer unsizing of its one field.

use crate::vec::Vec;
use crate::borrow::Borrow;
use crate::cmp::Ordering;
use crate::fmt;
use crate::hash::{Hash, Hasher};
use crate::ops::{Deref, DispatchFromDyn};
use crate::marker::Unsize;
use crate::pin::PinSafePointer;

// library/alloc/src/rcs/rc.rs:357,2604 @8d1a764
impl<T: ?Sized + Unsize<U>, U: ?Sized> DispatchFromDyn<Rc<U>> for Rc<T> {}
unsafe impl<T: ?Sized> PinSafePointer for Rc<T> {}
use crate::ptr;

// ---------------------------------------------------------------- raw blocks (shared with Arc)

#[repr(C, align(16))]
struct Chunk16 {
    _b: [u8; 16],
}

#[repr(C, align(64))]
struct Chunk64 {
    _b: [u8; 64],
}

/// Allocates `size` bytes aligned to `align` (<= 64), never zero-sized.
pub(crate) fn block_alloc(size: usize, align: usize) -> *mut u8 {
    if align <= 16 {
        let n = (size + 15) / 16;
        let mut v: Vec<Chunk16> = Vec::with_capacity(n);
        let p = v.as_mut_ptr() as *mut u8;
        crate::mem::forget(v);
        p
    } else if align <= 64 {
        let n = (size + 63) / 64;
        let mut v: Vec<Chunk64> = Vec::with_capacity(n);
        let p = v.as_mut_ptr() as *mut u8;
        crate::mem::forget(v);
        p
    } else {
        crate::panicking::panic_str("Rc/Arc: alignment above 64 is not supported")
    }
}

pub(crate) unsafe fn block_free(p: *mut u8, size: usize, align: usize) {
    if align <= 16 {
        let n = (size + 15) / 16;
        drop(Vec::from_raw_parts(p as *mut Chunk16, 0, n));
    } else {
        let n = (size + 63) / 64;
        drop(Vec::from_raw_parts(p as *mut Chunk64, 0, n));
    }
}

/// Block header; `weak` counts the Weaks plus one for all strong pointers together.
#[repr(C)]
pub(crate) struct Header {
    pub strong: usize,
    pub weak: usize,
    pub size: usize,
    pub align: usize,
}

pub(crate) const HEADER: usize = 4 * crate::mem::size_of::<usize>();

/// Offset of a value with alignment `align` after the header.
#[inline]
pub(crate) fn value_offset(align: usize) -> usize {
    (HEADER + align - 1) & !(align - 1)
}

/// A block for `value_size` bytes at alignment `value_align`: (header, value address).
pub(crate) fn alloc_block(value_size: usize, value_align: usize, strong: usize, weak: usize) -> (*mut Header, *mut u8) {
    let off = value_offset(value_align);
    let align = if value_align > 8 { value_align } else { 8 };
    let size = off + value_size;
    let base = block_alloc(size, align);
    let h = base as *mut Header;
    unsafe {
        ptr::write(h, Header { strong, weak, size, align });
        (h, base.add(off))
    }
}

/// The header of the block holding the value at `p` (sized or not: the value's offset depends
/// only on its alignment, which `align_of_val` reads from the pointer's metadata).
pub(crate) fn header_of<T: ?Sized>(p: *const T) -> *mut Header {
    let align = unsafe { crate::intrinsics_mem::align_of_val(&*p) };
    ((p as *const u8 as usize) - value_offset(align)) as *mut Header
}

/// Address of `Weak::new()`'s pointer (no block).
pub(crate) const DANGLING: usize = usize::MAX;

pub(crate) fn is_dangling<T: ?Sized>(p: *const T) -> bool {
    (p as *const u8 as usize) == DANGLING
}

pub(crate) unsafe fn free_block(h: *mut Header) {
    let size = (*h).size;
    let align = (*h).align;
    block_free(h as *mut u8, size, align);
}

// ---------------------------------------------------------------- Rc

pub struct Rc<T: ?Sized> {
    ptr: crate::ptr::NonNull<T>,
}

// library/alloc/src/rc.rs:330-338: the count is shared and non-atomic.
impl<T: ?Sized> !Send for Rc<T> {}
impl<T: ?Sized> !Sync for Rc<T> {}

impl<T> Rc<T> {
    pub fn new(value: T) -> Rc<T> {
        let (_, p) = alloc_block(crate::mem::size_of::<T>(), crate::mem::align_of::<T>(), 1, 1);
        let p = p as *mut T;
        unsafe { ptr::write(p, value) };
        Rc { ptr: unsafe { ptr::NonNull::new_unchecked(p) } }
    }

    pub fn new_cyclic<F: FnOnce(&Weak<T>) -> T>(data_fn: F) -> Rc<T> {
        let (h, p) = alloc_block(crate::mem::size_of::<T>(), crate::mem::align_of::<T>(), 0, 1);
        let p = p as *mut T;
        let weak = Weak { ptr: unsafe { ptr::NonNull::new_unchecked(p) } };
        let data = data_fn(&weak);
        unsafe {
            ptr::write(p, data);
            (*h).strong = 1;
        }
        crate::mem::forget(weak);
        Rc { ptr: unsafe { ptr::NonNull::new_unchecked(p) } }
    }

    pub fn try_unwrap(this: Rc<T>) -> Result<T, Rc<T>> {
        if Rc::strong_count(&this) == 1 {
            unsafe {
                let val = ptr::read(this.ptr.as_ptr());
                (*header_of(this.ptr.as_ptr())).strong = 0;
                // the strong pointers' implicit weak reference
                let weak = Weak { ptr: this.ptr };
                crate::mem::forget(this);
                drop(weak);
                Ok(val)
            }
        } else {
            Err(this)
        }
    }

    pub fn into_inner(this: Rc<T>) -> Option<T> {
        match Rc::try_unwrap(this) {
            Ok(v) => Some(v),
            Err(_) => None,
        }
    }
}

impl<T: ?Sized> Rc<T> {
    pub fn into_raw(this: Rc<T>) -> *const T {
        let p = this.ptr.as_ptr() as *const T;
        crate::mem::forget(this);
        p
    }

    pub unsafe fn from_raw(p: *const T) -> Rc<T> {
        Rc { ptr: unsafe { ptr::NonNull::new_unchecked(p as *mut T) } }
    }

    pub unsafe fn increment_strong_count(p: *const T) {
        let r = crate::mem::ManuallyDrop::new(Rc::from_raw(p));
        let _c = crate::mem::ManuallyDrop::new(Rc::clone(&r));
    }

    pub unsafe fn decrement_strong_count(p: *const T) {
        drop(Rc::from_raw(p));
    }
}

impl<T: ?Sized + crate::clone_uninit::CloneToUninit> Rc<T> {
    /// Real alloc's make_mut: clone into a fresh block when other strong pointers exist, move the
    /// value to a fresh block when only weak ones do, else hand out the value in place.
    pub fn make_mut(this: &mut Rc<T>) -> &mut T {
        unsafe {
            let p = this.ptr.as_ptr();
            let h = header_of(p);
            if (*h).strong != 1 {
                // other strong pointers: clone the data
                let size = crate::intrinsics_mem::size_of_val(&*p);
                let align = crate::intrinsics_mem::align_of_val(&*p);
                let (nh, np) = alloc_block(size, align, 1, 1);
                let guard = FreeBlockGuard(nh);
                (*p).clone_to_uninit(np);
                crate::mem::forget(guard);
                *this = Rc::from_parts(crate::clone_uninit::with_addr(p, np));
            } else if (*h).weak != 1 {
                // only weak pointers: move the data to a new allocation, disassociating them
                let size = crate::intrinsics_mem::size_of_val(&*p);
                let align = crate::intrinsics_mem::align_of_val(&*p);
                let (_, np) = alloc_block(size, align, 1, 1);
                ptr::copy_nonoverlapping(p as *const u8, np, size);
                (*h).strong = 0;
                (*h).weak -= 1;
                let fresh = Rc::from_parts(crate::clone_uninit::with_addr(p, np));
                let old = crate::mem::replace(this, fresh);
                crate::mem::forget(old);
            }
            &mut *this.ptr.as_ptr()
        }
    }
}

/// Frees a fresh block if cloning into it panics.
pub(crate) struct FreeBlockGuard(pub(crate) *mut Header);

impl Drop for FreeBlockGuard {
    fn drop(&mut self) {
        unsafe { free_block(self.0) }
    }
}

impl<T: Clone> Rc<T> {
    pub fn unwrap_or_clone(this: Rc<T>) -> T {
        match Rc::try_unwrap(this) {
            Ok(t) => t,
            Err(rc) => (*rc).clone(),
        }
    }
}

impl<T: ?Sized> Rc<T> {
    pub fn as_ptr(this: &Rc<T>) -> *const T {
        this.ptr.as_ptr() as *const T
    }

    pub fn downgrade(this: &Rc<T>) -> Weak<T> {
        unsafe { (*header_of(this.ptr.as_ptr())).weak += 1 };
        Weak { ptr: this.ptr }
    }

    pub fn weak_count(this: &Rc<T>) -> usize {
        unsafe { (*header_of(this.ptr.as_ptr())).weak - 1 }
    }

    pub fn strong_count(this: &Rc<T>) -> usize {
        unsafe { (*header_of(this.ptr.as_ptr())).strong }
    }

    pub fn get_mut(this: &mut Rc<T>) -> Option<&mut T> {
        unsafe {
            if (*header_of(this.ptr.as_ptr())).strong == 1 && (*header_of(this.ptr.as_ptr())).weak == 1 {
                Some(&mut *this.ptr.as_ptr())
            } else {
                None
            }
        }
    }

    pub fn ptr_eq(this: &Rc<T>, other: &Rc<T>) -> bool {
        this.ptr.as_ptr() as *const u8 == other.ptr.as_ptr() as *const u8
    }

    /// An Rc for a value already placed at `ptr` inside block `header` (used by the
    /// unsized constructors).
    unsafe fn from_parts(ptr: *mut T) -> Rc<T> {
        Rc { ptr: ptr::NonNull::new_unchecked(ptr) }
    }
}

impl Rc<dyn crate::any::Any> {
    /// The concrete `Rc<T>` when the value is a `T` (the same allocation), else the original.
    pub fn downcast<T: crate::any::Any>(self) -> Result<Rc<T>, Rc<dyn crate::any::Any>> {
        if crate::any::Any::type_id(&*self) == crate::any::TypeId::of::<T>() {
            let p = self.ptr.as_ptr() as *mut T;
            crate::mem::forget(self);
            Ok(Rc { ptr: unsafe { ptr::NonNull::new_unchecked(p) } })
        } else {
            Err(self)
        }
    }
}

impl<T: ?Sized> Clone for Rc<T> {
    fn clone(&self) -> Rc<T> {
        unsafe { (*header_of(self.ptr.as_ptr())).strong += 1 };
        Rc { ptr: self.ptr }
    }
}

impl<T: ?Sized> Drop for Rc<T> {
    fn drop(&mut self) {
        unsafe {
            (*header_of(self.ptr.as_ptr())).strong -= 1;
            if (*header_of(self.ptr.as_ptr())).strong == 0 {
                ptr::drop_in_place(self.ptr.as_ptr());
                (*header_of(self.ptr.as_ptr())).weak -= 1;
                if (*header_of(self.ptr.as_ptr())).weak == 0 {
                    free_block(header_of(self.ptr.as_ptr()));
                }
            }
        }
    }
}

impl<T: ?Sized> Deref for Rc<T> {
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { &*self.ptr.as_ptr() }
    }
}

impl<T: ?Sized> AsRef<T> for Rc<T> {
    fn as_ref(&self) -> &T {
        unsafe { &*self.ptr.as_ptr() }
    }
}

impl<T: ?Sized> Borrow<T> for Rc<T> {
    fn borrow(&self) -> &T {
        unsafe { &*self.ptr.as_ptr() }
    }
}

impl<T: Default> Default for Rc<T> {
    fn default() -> Rc<T> {
        Rc::new(T::default())
    }
}

/// An empty slice (real alloc: one shared empty allocation; here a fresh empty block).
impl<T> Default for Rc<[T]> {
    fn default() -> Rc<[T]> {
        Rc::from(Vec::new())
    }
}

impl Default for Rc<str> {
    fn default() -> Rc<str> {
        Rc::from("")
    }
}

impl<T> From<T> for Rc<T> {
    fn from(t: T) -> Rc<T> {
        Rc::new(t)
    }
}

impl<'a> From<&'a str> for Rc<str> {
    fn from(v: &'a str) -> Rc<str> {
        let (_, p) = alloc_block(v.len(), 1, 1, 1);
        unsafe {
            ptr::copy_nonoverlapping(v.as_ptr(), p, v.len());
            let s = ptr::slice_from_raw_parts_mut(p, v.len()) as *mut str;
            Rc::from_parts(s)
        }
    }
}

impl From<crate::string::String> for Rc<str> {
    fn from(v: crate::string::String) -> Rc<str> {
        Rc::from(v.as_str())
    }
}

impl<'a, T: Clone> From<&'a [T]> for Rc<[T]> {
    fn from(v: &'a [T]) -> Rc<[T]> {
        let (_, p) = alloc_block(v.len() * crate::mem::size_of::<T>(), crate::mem::align_of::<T>(), 1, 1);
        let p = p as *mut T;
        let mut i = 0;
        while i < v.len() {
            unsafe { ptr::write(p.add(i), v[i].clone()) };
            i += 1;
        }
        unsafe { Rc::from_parts(ptr::slice_from_raw_parts_mut(p, v.len())) }
    }
}

/// Moves a boxed (possibly unsized) value into a new Rc block and frees the box's allocation.
impl<T: ?Sized> From<crate::boxed::Box<T>> for Rc<T> {
    fn from(b: crate::boxed::Box<T>) -> Rc<T> {
        unsafe {
            let raw = crate::boxed::Box::into_raw(b);
            let size = crate::intrinsics_mem::size_of_val(&*raw);
            let align = crate::intrinsics_mem::align_of_val(&*raw);
            let (_, np) = alloc_block(size, align, 1, 1);
            ptr::copy_nonoverlapping(raw as *const u8, np, size);
            free_box_allocation(raw as *mut u8, size, align);
            Rc::from_parts(crate::clone_uninit::with_addr(raw, np))
        }
    }
}

/// Frees a Box's allocation whose value was moved out (bytes copied, not dropped).
pub(crate) unsafe fn free_box_allocation(p: *mut u8, size: usize, align: usize) {
    if size != 0 {
        #[cfg(not(relax_check))]
        crate::heap::dealloc(p, crate::heap::Layout::from_size_align_unchecked(size, align));
        #[cfg(relax_check)]
        std::alloc::dealloc(p, std::alloc::Layout::from_size_align_unchecked(size, align));
    }
}

/// The array's elements moved into a new slice block (real alloc).
impl<T, const N: usize> From<[T; N]> for Rc<[T]> {
    fn from(v: [T; N]) -> Rc<[T]> {
        let (_, p) = alloc_block(N * crate::mem::size_of::<T>(), crate::mem::align_of::<T>(), 1, 1);
        let p = p as *mut T;
        unsafe {
            ptr::copy_nonoverlapping(&v as *const [T; N] as *const T, p, N);
            crate::mem::forget(v);
            Rc::from_parts(ptr::slice_from_raw_parts_mut(p, N))
        }
    }
}

impl<T> From<Vec<T>> for Rc<[T]> {
    fn from(v: Vec<T>) -> Rc<[T]> {
        let mut v = v;
        let len = v.len();
        let (_, p) = alloc_block(len * crate::mem::size_of::<T>(), crate::mem::align_of::<T>(), 1, 1);
        let p = p as *mut T;
        unsafe {
            ptr::copy_nonoverlapping(v.as_ptr(), p, len);
            v.set_len(0);
            Rc::from_parts(ptr::slice_from_raw_parts_mut(p, len))
        }
    }
}

impl<T> crate::iter::FromIterator<T> for Rc<[T]> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Rc<[T]> {
        let v: Vec<T> = iter.into_iter().collect();
        Rc::from(v)
    }
}

impl<T: ?Sized + PartialEq> PartialEq for Rc<T> {
    fn eq(&self, other: &Rc<T>) -> bool {
        **self == **other
    }
}

impl<T: ?Sized + Eq> Eq for Rc<T> {}

impl<T: ?Sized + PartialOrd> PartialOrd for Rc<T> {
    fn partial_cmp(&self, other: &Rc<T>) -> Option<Ordering> {
        (**self).partial_cmp(&**other)
    }
}

impl<T: ?Sized + Ord> Ord for Rc<T> {
    fn cmp(&self, other: &Rc<T>) -> Ordering {
        (**self).cmp(&**other)
    }
}

impl<T: ?Sized + Hash> Hash for Rc<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        (**self).hash(state)
    }
}

impl<T: ?Sized + fmt::Display> fmt::Display for Rc<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&**self, f)
    }
}

impl<T: ?Sized + fmt::Debug> fmt::Debug for Rc<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl<T: ?Sized> fmt::Pointer for Rc<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Pointer::fmt(&(self.ptr.as_ptr() as *const T), f)
    }
}

// ---------------------------------------------------------------- Weak

pub struct Weak<T: ?Sized> {
    /// The non-null `DANGLING` sentinel for `Weak::new()`.
    ptr: crate::ptr::NonNull<T>,
}

// library/alloc/src/rc.rs:3207-3209: weak handles share the same count.
impl<T: ?Sized> !Send for Weak<T> {}
impl<T: ?Sized> !Sync for Weak<T> {}

impl<T> Weak<T> {
    pub const fn new() -> Weak<T> {
        Weak { ptr: unsafe { ptr::NonNull::new_unchecked(DANGLING as *mut T) } }
    }
}

impl<T: ?Sized> Weak<T> {
    pub fn upgrade(&self) -> Option<Rc<T>> {
        if is_dangling(self.ptr.as_ptr()) {
            return None;
        }
        unsafe {
            if (*header_of(self.ptr.as_ptr())).strong == 0 {
                None
            } else {
                (*header_of(self.ptr.as_ptr())).strong += 1;
                Some(Rc { ptr: self.ptr })
            }
        }
    }

    pub fn strong_count(&self) -> usize {
        if is_dangling(self.ptr.as_ptr()) {
            0
        } else {
            unsafe { (*header_of(self.ptr.as_ptr())).strong }
        }
    }

    pub fn weak_count(&self) -> usize {
        if is_dangling(self.ptr.as_ptr()) {
            return 0;
        }
        unsafe {
            if (*header_of(self.ptr.as_ptr())).strong > 0 {
                (*header_of(self.ptr.as_ptr())).weak - 1
            } else {
                0
            }
        }
    }

    pub fn ptr_eq(&self, other: &Weak<T>) -> bool {
        self.ptr.as_ptr() as *const u8 == other.ptr.as_ptr() as *const u8
    }

    pub fn as_ptr(&self) -> *const T {
        self.ptr.as_ptr() as *const T
    }
}

impl<T: ?Sized> Clone for Weak<T> {
    fn clone(&self) -> Weak<T> {
        if !is_dangling(self.ptr.as_ptr()) {
            unsafe { (*header_of(self.ptr.as_ptr())).weak += 1 };
        }
        Weak { ptr: self.ptr }
    }
}

impl<T: ?Sized> Drop for Weak<T> {
    fn drop(&mut self) {
        if is_dangling(self.ptr.as_ptr()) {
            return;
        }
        unsafe {
            (*header_of(self.ptr.as_ptr())).weak -= 1;
            if (*header_of(self.ptr.as_ptr())).weak == 0 {
                free_block(header_of(self.ptr.as_ptr()));
            }
        }
    }
}

impl<T> Default for Weak<T> {
    fn default() -> Weak<T> {
        Weak::new()
    }
}

impl<T: ?Sized> fmt::Debug for Weak<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("(Weak)")
    }
}

// ---------------------------------------------------------------- uninit / zeroed (alloc's API)

impl<T> Rc<T> {
    pub fn new_uninit() -> Rc<crate::mem::MaybeUninit<T>> {
        let (_, p) = alloc_block(crate::mem::size_of::<T>(), crate::mem::align_of::<T>(), 1, 1);
        Rc { ptr: unsafe { ptr::NonNull::new_unchecked(p as *mut crate::mem::MaybeUninit<T>) } }
    }
    pub fn new_zeroed() -> Rc<crate::mem::MaybeUninit<T>> {
        let (_, p) = alloc_block(crate::mem::size_of::<T>(), crate::mem::align_of::<T>(), 1, 1);
        unsafe { ptr::write_bytes(p, 0, crate::mem::size_of::<T>()) };
        Rc { ptr: unsafe { ptr::NonNull::new_unchecked(p as *mut crate::mem::MaybeUninit<T>) } }
    }
}

impl<T> Rc<[T]> {
    pub fn new_uninit_slice(len: usize) -> Rc<[crate::mem::MaybeUninit<T>]> {
        let size = match len.checked_mul(crate::mem::size_of::<T>()) {
            Some(s) => s,
            None => crate::panicking::panic_str("capacity overflow"),
        };
        let (_, p) = alloc_block(size, crate::mem::align_of::<T>(), 1, 1);
        let s = ptr::slice_from_raw_parts_mut(p as *mut crate::mem::MaybeUninit<T>, len);
        Rc { ptr: unsafe { ptr::NonNull::new_unchecked(s) } }
    }
    pub fn new_zeroed_slice(len: usize) -> Rc<[crate::mem::MaybeUninit<T>]> {
        let r = Rc::<[T]>::new_uninit_slice(len);
        unsafe { ptr::write_bytes(r.ptr.as_ptr() as *mut u8, 0, len * crate::mem::size_of::<T>()) };
        r
    }
}

impl<T> Rc<crate::mem::MaybeUninit<T>> {
    pub unsafe fn assume_init(self) -> Rc<T> {
        let p = self.ptr.as_ptr() as *mut T;
        crate::mem::forget(self);
        Rc { ptr: ptr::NonNull::new_unchecked(p) }
    }
}

impl<T> Rc<[crate::mem::MaybeUninit<T>]> {
    pub unsafe fn assume_init(self) -> Rc<[T]> {
        let p = self.ptr.as_ptr() as *mut [T];
        crate::mem::forget(self);
        Rc { ptr: ptr::NonNull::new_unchecked(p) }
    }
}

impl<T: ?Sized> Rc<T> {
    /// Mutable access without the uniqueness check (alloc's unstable get_mut_unchecked).
    pub unsafe fn get_mut_unchecked(this: &mut Rc<T>) -> &mut T {
        &mut *this.ptr.as_ptr()
    }
}
