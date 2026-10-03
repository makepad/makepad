//! Arc / Weak: thread-safe reference counting. Same block layout as Rc (rc.rs: header with
//! strong/weak/size/align, then the value; the Arc holds header + value pointers), with the
//! counts updated through atomic intrinsics using real std's orderings.

use crate::vec::Vec;
use crate::borrow::Borrow;
use crate::cmp::Ordering;
use crate::fmt;
use crate::hash::{Hash, Hasher};
use crate::intrinsics_atomic as ia;
use crate::marker::PhantomData;
use crate::ops::Deref;
use crate::ptr;
use crate::rc::{alloc_block, free_block, value_offset, Header};

const RELAXED: u8 = 0;
const RELEASE: u8 = 1;
const ACQUIRE: u8 = 2;

/// Above this many strong references a clone aborts (as real std, guards overflow).
const MAX_REFCOUNT: usize = isize::MAX as usize;

#[inline]
fn strong_ptr(h: *mut Header) -> *mut usize {
    unsafe { &mut (*h).strong as *mut usize }
}

#[inline]
fn weak_ptr(h: *mut Header) -> *mut usize {
    unsafe { &mut (*h).weak as *mut usize }
}

pub struct Arc<T: ?Sized> {
    header: *mut Header,
    ptr: *mut T,
    _m: PhantomData<T>,
}

unsafe impl<T: ?Sized + Sync + Send> Send for Arc<T> {}
unsafe impl<T: ?Sized + Sync + Send> Sync for Arc<T> {}

impl<T> Arc<T> {
    pub fn new(data: T) -> Arc<T> {
        let (h, p) = alloc_block(crate::mem::size_of::<T>(), crate::mem::align_of::<T>(), 1, 1);
        let p = p as *mut T;
        unsafe { ptr::write(p, data) };
        Arc { header: h, ptr: p, _m: PhantomData }
    }

    pub fn new_cyclic<F: FnOnce(&Weak<T>) -> T>(data_fn: F) -> Arc<T> {
        let (h, p) = alloc_block(crate::mem::size_of::<T>(), crate::mem::align_of::<T>(), 0, 1);
        let p = p as *mut T;
        let weak = Weak { header: h, ptr: p, _m: PhantomData };
        let data = data_fn(&weak);
        unsafe {
            ptr::write(p, data);
            ia::atomic_store(strong_ptr(h), 1usize, RELEASE);
        }
        crate::mem::forget(weak);
        Arc { header: h, ptr: p, _m: PhantomData }
    }

    pub fn try_unwrap(this: Arc<T>) -> Result<T, Arc<T>> {
        let (_, ok) = unsafe { ia::atomic_cxchg(strong_ptr(this.header), 1usize, 0usize, RELAXED, RELAXED) };
        if !ok {
            return Err(this);
        }
        unsafe {
            ia::atomic_fence(ACQUIRE);
            let elem = ptr::read(this.ptr);
            let weak = Weak { header: this.header, ptr: this.ptr, _m: PhantomData };
            crate::mem::forget(this);
            drop(weak);
            Ok(elem)
        }
    }

    pub fn into_inner(this: Arc<T>) -> Option<T> {
        let this = crate::mem::ManuallyDrop::new(this);
        let old = unsafe { ia::atomic_fetch_sub(strong_ptr(this.header), 1usize, RELEASE) };
        if old != 1 {
            return None;
        }
        unsafe {
            ia::atomic_fence(ACQUIRE);
            let elem = ptr::read(this.ptr);
            drop(Weak { header: this.header, ptr: this.ptr, _m: PhantomData });
            Some(elem)
        }
    }

    pub fn into_raw(this: Arc<T>) -> *const T {
        let p = this.ptr as *const T;
        crate::mem::forget(this);
        p
    }

    pub unsafe fn from_raw(p: *const T) -> Arc<T> {
        let off = value_offset(crate::mem::align_of::<T>());
        let h = (p as *const u8).sub(off) as *mut Header;
        Arc { header: h, ptr: p as *mut T, _m: PhantomData }
    }

    pub unsafe fn increment_strong_count(p: *const T) {
        let a = crate::mem::ManuallyDrop::new(Arc::from_raw(p));
        let _c = crate::mem::ManuallyDrop::new(Arc::clone(&a));
    }

    pub unsafe fn decrement_strong_count(p: *const T) {
        drop(Arc::from_raw(p));
    }
}

impl<T: Clone> Arc<T> {
    pub fn make_mut(this: &mut Arc<T>) -> &mut T {
        let (_, ok) = unsafe { ia::atomic_cxchg(strong_ptr(this.header), 1usize, 0usize, ACQUIRE, RELAXED) };
        if !ok {
            // other strong pointers: clone the data
            *this = Arc::new((**this).clone());
        } else if unsafe { ia::atomic_load(weak_ptr(this.header) as *const usize, RELAXED) } != 1 {
            // we were the only strong pointer but weaks remain: move the data out
            unsafe {
                let fresh = Arc::new(ptr::read(this.ptr));
                let old_weak = Weak { header: this.header, ptr: this.ptr, _m: PhantomData };
                let old = crate::mem::replace(this, fresh);
                crate::mem::forget(old);
                drop(old_weak);
            }
        } else {
            // unique: restore the strong count
            unsafe { ia::atomic_store(strong_ptr(this.header), 1usize, RELEASE) };
        }
        unsafe { &mut *this.ptr }
    }

    pub fn unwrap_or_clone(this: Arc<T>) -> T {
        match Arc::try_unwrap(this) {
            Ok(t) => t,
            Err(a) => (*a).clone(),
        }
    }
}

impl<T: ?Sized> Arc<T> {
    pub fn as_ptr(this: &Arc<T>) -> *const T {
        this.ptr as *const T
    }

    pub fn downgrade(this: &Arc<T>) -> Weak<T> {
        let old = unsafe { ia::atomic_fetch_add(weak_ptr(this.header), 1usize, ACQUIRE) };
        if old > MAX_REFCOUNT {
            unsafe { crate::intrinsics_rt::abort() };
        }
        Weak { header: this.header, ptr: this.ptr, _m: PhantomData }
    }

    pub fn weak_count(this: &Arc<T>) -> usize {
        let cnt = unsafe { ia::atomic_load(weak_ptr(this.header) as *const usize, ACQUIRE) };
        cnt - 1
    }

    pub fn strong_count(this: &Arc<T>) -> usize {
        unsafe { ia::atomic_load(strong_ptr(this.header) as *const usize, ACQUIRE) }
    }

    /// Mutable access when this is the only Arc and no Weak exists.
    pub fn get_mut(this: &mut Arc<T>) -> Option<&mut T> {
        let unique = unsafe {
            ia::atomic_load(weak_ptr(this.header) as *const usize, ACQUIRE) == 1
                && ia::atomic_load(strong_ptr(this.header) as *const usize, ACQUIRE) == 1
        };
        if unique {
            Some(unsafe { &mut *this.ptr })
        } else {
            None
        }
    }

    pub fn ptr_eq(this: &Arc<T>, other: &Arc<T>) -> bool {
        this.ptr as *const u8 == other.ptr as *const u8
    }

    unsafe fn from_parts(header: *mut Header, ptr: *mut T) -> Arc<T> {
        Arc { header, ptr, _m: PhantomData }
    }

    fn drop_slow(&mut self) {
        unsafe {
            ptr::drop_in_place(self.ptr);
            drop(Weak { header: self.header, ptr: self.ptr, _m: PhantomData });
        }
    }
}

impl<T: ?Sized> Clone for Arc<T> {
    fn clone(&self) -> Arc<T> {
        let old = unsafe { ia::atomic_fetch_add(strong_ptr(self.header), 1usize, RELAXED) };
        if old > MAX_REFCOUNT {
            unsafe { crate::intrinsics_rt::abort() };
        }
        Arc { header: self.header, ptr: self.ptr, _m: PhantomData }
    }
}

impl<T: ?Sized> Drop for Arc<T> {
    fn drop(&mut self) {
        let old = unsafe { ia::atomic_fetch_sub(strong_ptr(self.header), 1usize, RELEASE) };
        if old != 1 {
            return;
        }
        unsafe { ia::atomic_fence(ACQUIRE) };
        self.drop_slow();
    }
}

impl<T: ?Sized> Deref for Arc<T> {
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { &*self.ptr }
    }
}

impl<T: ?Sized> AsRef<T> for Arc<T> {
    fn as_ref(&self) -> &T {
        unsafe { &*self.ptr }
    }
}

impl<T: ?Sized> Borrow<T> for Arc<T> {
    fn borrow(&self) -> &T {
        unsafe { &*self.ptr }
    }
}

impl<T: Default> Default for Arc<T> {
    fn default() -> Arc<T> {
        Arc::new(T::default())
    }
}

impl<T> From<T> for Arc<T> {
    fn from(t: T) -> Arc<T> {
        Arc::new(t)
    }
}

impl<'a> From<&'a str> for Arc<str> {
    fn from(v: &'a str) -> Arc<str> {
        let (h, p) = alloc_block(v.len(), 1, 1, 1);
        unsafe {
            ptr::copy_nonoverlapping(v.as_ptr(), p, v.len());
            Arc::from_parts(h, ptr::slice_from_raw_parts_mut(p, v.len()) as *mut str)
        }
    }
}

impl From<crate::string::String> for Arc<str> {
    fn from(v: crate::string::String) -> Arc<str> {
        Arc::from(v.as_str())
    }
}

impl<'a, T: Clone> From<&'a [T]> for Arc<[T]> {
    fn from(v: &'a [T]) -> Arc<[T]> {
        let (h, p) = alloc_block(v.len() * crate::mem::size_of::<T>(), crate::mem::align_of::<T>(), 1, 1);
        let p = p as *mut T;
        let mut i = 0;
        while i < v.len() {
            unsafe { ptr::write(p.add(i), v[i].clone()) };
            i += 1;
        }
        unsafe { Arc::from_parts(h, ptr::slice_from_raw_parts_mut(p, v.len())) }
    }
}

impl<T> From<Vec<T>> for Arc<[T]> {
    fn from(v: Vec<T>) -> Arc<[T]> {
        let mut v = v;
        let len = v.len();
        let (h, p) = alloc_block(len * crate::mem::size_of::<T>(), crate::mem::align_of::<T>(), 1, 1);
        let p = p as *mut T;
        unsafe {
            ptr::copy_nonoverlapping(v.as_ptr(), p, len);
            v.set_len(0);
            Arc::from_parts(h, ptr::slice_from_raw_parts_mut(p, len))
        }
    }
}

impl<T> crate::iter::FromIterator<T> for Arc<[T]> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Arc<[T]> {
        let v: Vec<T> = iter.into_iter().collect();
        Arc::from(v)
    }
}

impl<T: ?Sized + PartialEq> PartialEq for Arc<T> {
    fn eq(&self, other: &Arc<T>) -> bool {
        **self == **other
    }
}

impl<T: ?Sized + Eq> Eq for Arc<T> {}

impl<T: ?Sized + PartialOrd> PartialOrd for Arc<T> {
    fn partial_cmp(&self, other: &Arc<T>) -> Option<Ordering> {
        (**self).partial_cmp(&**other)
    }
}

impl<T: ?Sized + Ord> Ord for Arc<T> {
    fn cmp(&self, other: &Arc<T>) -> Ordering {
        (**self).cmp(&**other)
    }
}

impl<T: ?Sized + Hash> Hash for Arc<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        (**self).hash(state)
    }
}

impl<T: ?Sized + fmt::Display> fmt::Display for Arc<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&**self, f)
    }
}

impl<T: ?Sized + fmt::Debug> fmt::Debug for Arc<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}

impl<T: ?Sized> fmt::Pointer for Arc<T> {
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

unsafe impl<T: ?Sized + Sync + Send> Send for Weak<T> {}
unsafe impl<T: ?Sized + Sync + Send> Sync for Weak<T> {}

impl<T> Weak<T> {
    pub const fn new() -> Weak<T> {
        Weak { header: ptr::null_mut(), ptr: ptr::null_mut(), _m: PhantomData }
    }
}

impl<T: ?Sized> Weak<T> {
    pub fn upgrade(&self) -> Option<Arc<T>> {
        if self.header.is_null() {
            return None;
        }
        let sp = strong_ptr(self.header);
        let mut n = unsafe { ia::atomic_load(sp as *const usize, RELAXED) };
        loop {
            if n == 0 {
                return None;
            }
            if n > MAX_REFCOUNT {
                unsafe { crate::intrinsics_rt::abort() };
            }
            let (prev, ok) = unsafe { ia::atomic_cxchg_weak(sp, n, n + 1, ACQUIRE, RELAXED) };
            if ok {
                return Some(Arc { header: self.header, ptr: self.ptr, _m: PhantomData });
            }
            n = prev;
        }
    }

    pub fn strong_count(&self) -> usize {
        if self.header.is_null() {
            0
        } else {
            unsafe { ia::atomic_load(strong_ptr(self.header) as *const usize, ACQUIRE) }
        }
    }

    pub fn weak_count(&self) -> usize {
        if self.header.is_null() {
            return 0;
        }
        let weak = unsafe { ia::atomic_load(weak_ptr(self.header) as *const usize, ACQUIRE) };
        let strong = unsafe { ia::atomic_load(strong_ptr(self.header) as *const usize, ACQUIRE) };
        if strong == 0 {
            0
        } else {
            weak - 1
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
            let old = unsafe { ia::atomic_fetch_add(weak_ptr(self.header), 1usize, RELAXED) };
            if old > MAX_REFCOUNT {
                unsafe { crate::intrinsics_rt::abort() };
            }
        }
        Weak { header: self.header, ptr: self.ptr, _m: PhantomData }
    }
}

impl<T: ?Sized> Drop for Weak<T> {
    fn drop(&mut self) {
        if self.header.is_null() {
            return;
        }
        let old = unsafe { ia::atomic_fetch_sub(weak_ptr(self.header), 1usize, RELEASE) };
        if old == 1 {
            unsafe {
                ia::atomic_fence(ACQUIRE);
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
