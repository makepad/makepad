//! Rc / Weak: single-threaded reference counting.
//!
//! Layout (HotRust's own, R14): one heap block = a header (strong, weak, block size, block
//! align) followed by the value at the next multiple of the value's alignment. An `Rc<T>`
//! holds a pointer to the header and a (possibly fat) pointer to the value, so `Rc<str>`,
//! `Rc<[T]>` and `Rc<dyn Trait>` need no custom DST: unsizing an `Rc<T>` only rewrites the
//! value pointer (HotRust's CoerceUnsized for Rc/Arc/Weak converts the `ptr` field).

use crate::vec::Vec;
use crate::borrow::Borrow;
use crate::cmp::Ordering;
use crate::fmt;
use crate::hash::{Hash, Hasher};
use crate::marker::PhantomData;
use crate::ops::Deref;
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

pub(crate) unsafe fn free_block(h: *mut Header) {
    let size = (*h).size;
    let align = (*h).align;
    block_free(h as *mut u8, size, align);
}

// ---------------------------------------------------------------- Rc

pub struct Rc<T: ?Sized> {
    header: *mut Header,
    ptr: *mut T,
    _m: PhantomData<T>,
}

impl<T> Rc<T> {
    pub fn new(value: T) -> Rc<T> {
        let (h, p) = alloc_block(crate::mem::size_of::<T>(), crate::mem::align_of::<T>(), 1, 1);
        let p = p as *mut T;
        unsafe { ptr::write(p, value) };
        Rc { header: h, ptr: p, _m: PhantomData }
    }

    pub fn new_cyclic<F: FnOnce(&Weak<T>) -> T>(data_fn: F) -> Rc<T> {
        let (h, p) = alloc_block(crate::mem::size_of::<T>(), crate::mem::align_of::<T>(), 0, 1);
        let p = p as *mut T;
        let weak = Weak { header: h, ptr: p, _m: PhantomData };
        let data = data_fn(&weak);
        unsafe {
            ptr::write(p, data);
            (*h).strong = 1;
        }
        crate::mem::forget(weak);
        Rc { header: h, ptr: p, _m: PhantomData }
    }

    pub fn try_unwrap(this: Rc<T>) -> Result<T, Rc<T>> {
        if Rc::strong_count(&this) == 1 {
            unsafe {
                let val = ptr::read(this.ptr);
                (*this.header).strong = 0;
                // the strong pointers' implicit weak reference
                let weak = Weak { header: this.header, ptr: this.ptr, _m: PhantomData };
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

    pub fn into_raw(this: Rc<T>) -> *const T {
        let p = this.ptr as *const T;
        crate::mem::forget(this);
        p
    }

    pub unsafe fn from_raw(p: *const T) -> Rc<T> {
        let off = value_offset(crate::mem::align_of::<T>());
        let h = (p as *const u8).sub(off) as *mut Header;
        Rc { header: h, ptr: p as *mut T, _m: PhantomData }
    }

    pub unsafe fn increment_strong_count(p: *const T) {
        let r = crate::mem::ManuallyDrop::new(Rc::from_raw(p));
        let _c = crate::mem::ManuallyDrop::new(Rc::clone(&r));
    }

    pub unsafe fn decrement_strong_count(p: *const T) {
        drop(Rc::from_raw(p));
    }
}

impl<T: Clone> Rc<T> {
    pub fn make_mut(this: &mut Rc<T>) -> &mut T {
        unsafe {
            if (*this.header).strong != 1 {
                // other strong pointers: clone the data
                *this = Rc::new((**this).clone());
            } else if (*this.header).weak != 1 {
                // only weak pointers: move the data to a new allocation, disassociating them
                let fresh = Rc::new(ptr::read(this.ptr));
                (*this.header).strong = 0;
                (*this.header).weak -= 1;
                let old = crate::mem::replace(this, fresh);
                crate::mem::forget(old);
            }
            &mut *this.ptr
        }
    }

    pub fn unwrap_or_clone(this: Rc<T>) -> T {
        match Rc::try_unwrap(this) {
            Ok(t) => t,
            Err(rc) => (*rc).clone(),
        }
    }
}

impl<T: ?Sized> Rc<T> {
    pub fn as_ptr(this: &Rc<T>) -> *const T {
        this.ptr as *const T
    }

    pub fn downgrade(this: &Rc<T>) -> Weak<T> {
        unsafe { (*this.header).weak += 1 };
        Weak { header: this.header, ptr: this.ptr, _m: PhantomData }
    }

    pub fn weak_count(this: &Rc<T>) -> usize {
        unsafe { (*this.header).weak - 1 }
    }

    pub fn strong_count(this: &Rc<T>) -> usize {
        unsafe { (*this.header).strong }
    }

    pub fn get_mut(this: &mut Rc<T>) -> Option<&mut T> {
        unsafe {
            if (*this.header).strong == 1 && (*this.header).weak == 1 {
                Some(&mut *this.ptr)
            } else {
                None
            }
        }
    }

    pub fn ptr_eq(this: &Rc<T>, other: &Rc<T>) -> bool {
        this.ptr as *const u8 == other.ptr as *const u8
    }

    /// An Rc for a value already placed at `ptr` inside block `header` (used by the
    /// unsized constructors).
    unsafe fn from_parts(header: *mut Header, ptr: *mut T) -> Rc<T> {
        Rc { header, ptr, _m: PhantomData }
    }
}

impl<T: ?Sized> Clone for Rc<T> {
    fn clone(&self) -> Rc<T> {
        unsafe { (*self.header).strong += 1 };
        Rc { header: self.header, ptr: self.ptr, _m: PhantomData }
    }
}

impl<T: ?Sized> Drop for Rc<T> {
    fn drop(&mut self) {
        unsafe {
            (*self.header).strong -= 1;
            if (*self.header).strong == 0 {
                ptr::drop_in_place(self.ptr);
                (*self.header).weak -= 1;
                if (*self.header).weak == 0 {
                    free_block(self.header);
                }
            }
        }
    }
}

impl<T: ?Sized> Deref for Rc<T> {
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { &*self.ptr }
    }
}

impl<T: ?Sized> AsRef<T> for Rc<T> {
    fn as_ref(&self) -> &T {
        unsafe { &*self.ptr }
    }
}

impl<T: ?Sized> Borrow<T> for Rc<T> {
    fn borrow(&self) -> &T {
        unsafe { &*self.ptr }
    }
}

impl<T: Default> Default for Rc<T> {
    fn default() -> Rc<T> {
        Rc::new(T::default())
    }
}

impl<T> From<T> for Rc<T> {
    fn from(t: T) -> Rc<T> {
        Rc::new(t)
    }
}

impl<'a> From<&'a str> for Rc<str> {
    fn from(v: &'a str) -> Rc<str> {
        let (h, p) = alloc_block(v.len(), 1, 1, 1);
        unsafe {
            ptr::copy_nonoverlapping(v.as_ptr(), p, v.len());
            let s = ptr::slice_from_raw_parts_mut(p, v.len()) as *mut str;
            Rc::from_parts(h, s)
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
        let (h, p) = alloc_block(v.len() * crate::mem::size_of::<T>(), crate::mem::align_of::<T>(), 1, 1);
        let p = p as *mut T;
        let mut i = 0;
        while i < v.len() {
            unsafe { ptr::write(p.add(i), v[i].clone()) };
            i += 1;
        }
        unsafe { Rc::from_parts(h, ptr::slice_from_raw_parts_mut(p, v.len())) }
    }
}

impl<T> From<Vec<T>> for Rc<[T]> {
    fn from(v: Vec<T>) -> Rc<[T]> {
        let mut v = v;
        let len = v.len();
        let (h, p) = alloc_block(len * crate::mem::size_of::<T>(), crate::mem::align_of::<T>(), 1, 1);
        let p = p as *mut T;
        unsafe {
            ptr::copy_nonoverlapping(v.as_ptr(), p, len);
            v.set_len(0);
            Rc::from_parts(h, ptr::slice_from_raw_parts_mut(p, len))
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
        fmt::Pointer::fmt(&(self.ptr as *const T), f)
    }
}

// ---------------------------------------------------------------- Weak

pub struct Weak<T: ?Sized> {
    /// null for `Weak::new()`
    header: *mut Header,
    ptr: *mut T,
    _m: PhantomData<T>,
}

impl<T> Weak<T> {
    pub const fn new() -> Weak<T> {
        Weak { header: ptr::null_mut(), ptr: ptr::null_mut(), _m: PhantomData }
    }
}

impl<T: ?Sized> Weak<T> {
    pub fn upgrade(&self) -> Option<Rc<T>> {
        if self.header.is_null() {
            return None;
        }
        unsafe {
            if (*self.header).strong == 0 {
                None
            } else {
                (*self.header).strong += 1;
                Some(Rc { header: self.header, ptr: self.ptr, _m: PhantomData })
            }
        }
    }

    pub fn strong_count(&self) -> usize {
        if self.header.is_null() {
            0
        } else {
            unsafe { (*self.header).strong }
        }
    }

    pub fn weak_count(&self) -> usize {
        if self.header.is_null() {
            return 0;
        }
        unsafe {
            if (*self.header).strong > 0 {
                (*self.header).weak - 1
            } else {
                0
            }
        }
    }

    pub fn ptr_eq(&self, other: &Weak<T>) -> bool {
        self.ptr as *const u8 == other.ptr as *const u8
    }

    pub fn as_ptr(&self) -> *const T {
        self.ptr as *const T
    }
}

impl<T: ?Sized> Clone for Weak<T> {
    fn clone(&self) -> Weak<T> {
        if !self.header.is_null() {
            unsafe { (*self.header).weak += 1 };
        }
        Weak { header: self.header, ptr: self.ptr, _m: PhantomData }
    }
}

impl<T: ?Sized> Drop for Weak<T> {
    fn drop(&mut self) {
        if self.header.is_null() {
            return;
        }
        unsafe {
            (*self.header).weak -= 1;
            if (*self.header).weak == 0 {
                free_block(self.header);
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
