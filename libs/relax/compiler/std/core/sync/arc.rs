//! Arc / Weak: thread-safe reference counting. Same block layout as Rc (rc.rs: header with
//! strong/weak/size/align, then the value; the Arc holds header + value pointers), with the
//! counts updated through atomic intrinsics using real std's orderings.

use crate::vec::Vec;
use crate::borrow::Borrow;
use crate::cmp::Ordering;
use crate::fmt;
use crate::hash::{Hash, Hasher};
use crate::intrinsics_atomic as ia;
use crate::ops::{Deref, DispatchFromDyn};
use crate::marker::Unsize;
use crate::pin::PinSafePointer;

// library/alloc/src/rcs/arc.rs:292,2577 @8d1a764
impl<T: ?Sized + Unsize<U>, U: ?Sized> DispatchFromDyn<Arc<U>> for Arc<T> {}
impl<T: ?Sized + Unsize<U>, U: ?Sized> crate::ops::CoerceUnsized<Arc<U>> for Arc<T> {}
impl<T: ?Sized + Unsize<U>, U: ?Sized> crate::ops::CoerceUnsized<Weak<U>> for Weak<T> {}
unsafe impl<T: ?Sized> PinSafePointer for Arc<T> {}
use crate::ptr;
use crate::rc::{alloc_block, free_block, header_of, is_dangling, value_offset, Header, DANGLING};

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

#[rustc_insignificant_dtor]
pub struct Arc<T: ?Sized> {
    ptr: crate::ptr::NonNull<T>,
    _marker: crate::marker::PhantomData<T>,
}

unsafe impl<T: ?Sized + Sync + Send> Send for Arc<T> {}
unsafe impl<T: ?Sized + Sync + Send> Sync for Arc<T> {}

impl<T> Arc<T> {
    pub fn new(data: T) -> Arc<T> {
        let (_, p) = alloc_block(crate::mem::size_of::<T>(), crate::mem::align_of::<T>(), 1, 1);
        let p = p as *mut T;
        unsafe { ptr::write(p, data) };
        Arc { ptr: unsafe { ptr::NonNull::new_unchecked(p) }, _marker: crate::marker::PhantomData }
    }

    pub fn new_cyclic<F: FnOnce(&Weak<T>) -> T>(data_fn: F) -> Arc<T> {
        let (h, p) = alloc_block(crate::mem::size_of::<T>(), crate::mem::align_of::<T>(), 0, 1);
        let p = p as *mut T;
        let weak = Weak { ptr: unsafe { ptr::NonNull::new_unchecked(p) } };
        let data = data_fn(&weak);
        unsafe {
            ptr::write(p, data);
            ia::atomic_store(strong_ptr(h), 1usize, RELEASE);
        }
        crate::mem::forget(weak);
        Arc { ptr: unsafe { ptr::NonNull::new_unchecked(p) }, _marker: crate::marker::PhantomData }
    }

    pub fn try_unwrap(this: Arc<T>) -> Result<T, Arc<T>> {
        let (_, ok) = unsafe { ia::atomic_cxchg(strong_ptr(header_of(this.ptr.as_ptr())), 1usize, 0usize, RELAXED, RELAXED) };
        if !ok {
            return Err(this);
        }
        unsafe {
            ia::atomic_fence(ACQUIRE);
            let elem = ptr::read(this.ptr.as_ptr());
            let weak = Weak { ptr: this.ptr };
            crate::mem::forget(this);
            drop(weak);
            Ok(elem)
        }
    }

    pub fn into_inner(this: Arc<T>) -> Option<T> {
        let this = crate::mem::ManuallyDrop::new(this);
        let old = unsafe { ia::atomic_fetch_sub(strong_ptr(header_of(this.ptr.as_ptr())), 1usize, RELEASE) };
        if old != 1 {
            return None;
        }
        unsafe {
            ia::atomic_fence(ACQUIRE);
            let elem = ptr::read(this.ptr.as_ptr());
            drop(Weak { ptr: this.ptr });
            Some(elem)
        }
    }
}

impl<T: ?Sized> Arc<T> {
    pub fn into_raw(this: Arc<T>) -> *const T {
        let p = this.ptr.as_ptr() as *const T;
        crate::mem::forget(this);
        p
    }

    pub unsafe fn from_raw(p: *const T) -> Arc<T> {
        Arc { ptr: unsafe { ptr::NonNull::new_unchecked(p as *mut T) }, _marker: crate::marker::PhantomData }
    }

    pub unsafe fn increment_strong_count(p: *const T) {
        let a = crate::mem::ManuallyDrop::new(Arc::from_raw(p));
        let _c = crate::mem::ManuallyDrop::new(Arc::clone(&a));
    }

    pub unsafe fn decrement_strong_count(p: *const T) {
        drop(Arc::from_raw(p));
    }
}

impl<T: ?Sized + crate::clone_uninit::CloneToUninit> Arc<T> {
    /// Real alloc's make_mut (see Rc::make_mut); the unique check takes the strong count from 1
    /// to 0 so no Weak can upgrade meanwhile.
    pub fn make_mut(this: &mut Arc<T>) -> &mut T {
        let p = this.ptr.as_ptr();
        let (_, ok) = unsafe { ia::atomic_cxchg(strong_ptr(header_of(p)), 1usize, 0usize, ACQUIRE, RELAXED) };
        if !ok {
            // other strong pointers: clone the data
            unsafe {
                let size = crate::intrinsics_mem::size_of_val(&*p);
                let align = crate::intrinsics_mem::align_of_val(&*p);
                let (nh, np) = alloc_block(size, align, 1, 1);
                let guard = crate::rc::FreeBlockGuard(nh);
                (*p).clone_to_uninit(np);
                crate::mem::forget(guard);
                *this = Arc::from_parts(crate::clone_uninit::with_addr(p, np));
            }
        } else if unsafe { ia::atomic_load(weak_ptr(header_of(p)) as *const usize, RELAXED) } != 1 {
            // we were the only strong pointer but weaks remain: move the data out
            unsafe {
                let size = crate::intrinsics_mem::size_of_val(&*p);
                let align = crate::intrinsics_mem::align_of_val(&*p);
                let (_, np) = alloc_block(size, align, 1, 1);
                ptr::copy_nonoverlapping(p as *const u8, np, size);
                let fresh = Arc::from_parts(crate::clone_uninit::with_addr(p, np));
                let old_weak = Weak { ptr: this.ptr };
                let old = crate::mem::replace(this, fresh);
                crate::mem::forget(old);
                drop(old_weak);
            }
        } else {
            // unique: restore the strong count
            unsafe { ia::atomic_store(strong_ptr(header_of(p)), 1usize, RELEASE) };
        }
        unsafe { &mut *this.ptr.as_ptr() }
    }
}

impl<T: Clone> Arc<T> {
    pub fn unwrap_or_clone(this: Arc<T>) -> T {
        match Arc::try_unwrap(this) {
            Ok(t) => t,
            Err(a) => (*a).clone(),
        }
    }
}

impl<T: ?Sized> Arc<T> {
    #[rustc_never_returns_null_ptr]
    pub fn as_ptr(this: &Arc<T>) -> *const T {
        this.ptr.as_ptr() as *const T
    }

    pub fn downgrade(this: &Arc<T>) -> Weak<T> {
        let old = unsafe { ia::atomic_fetch_add(weak_ptr(header_of(this.ptr.as_ptr())), 1usize, ACQUIRE) };
        if old > MAX_REFCOUNT {
            unsafe { crate::intrinsics_rt::abort() };
        }
        Weak { ptr: this.ptr }
    }

    pub fn weak_count(this: &Arc<T>) -> usize {
        let cnt = unsafe { ia::atomic_load(weak_ptr(header_of(this.ptr.as_ptr())) as *const usize, ACQUIRE) };
        cnt - 1
    }

    pub fn strong_count(this: &Arc<T>) -> usize {
        unsafe { ia::atomic_load(strong_ptr(header_of(this.ptr.as_ptr())) as *const usize, ACQUIRE) }
    }

    /// Mutable access when this is the only Arc and no Weak exists.
    pub fn get_mut(this: &mut Arc<T>) -> Option<&mut T> {
        let unique = unsafe {
            ia::atomic_load(weak_ptr(header_of(this.ptr.as_ptr())) as *const usize, ACQUIRE) == 1
                && ia::atomic_load(strong_ptr(header_of(this.ptr.as_ptr())) as *const usize, ACQUIRE) == 1
        };
        if unique {
            Some(unsafe { &mut *this.ptr.as_ptr() })
        } else {
            None
        }
    }

    pub fn ptr_eq(this: &Arc<T>, other: &Arc<T>) -> bool {
        this.ptr.as_ptr() as *const u8 == other.ptr.as_ptr() as *const u8
    }

    unsafe fn from_parts(ptr: *mut T) -> Arc<T> {
        Arc { ptr: ptr::NonNull::new_unchecked(ptr), _marker: crate::marker::PhantomData }
    }

    fn drop_slow(&mut self) {
        unsafe {
            ptr::drop_in_place(self.ptr.as_ptr());
            drop(Weak { ptr: self.ptr });
        }
    }
}

impl Arc<dyn crate::any::Any + Send + Sync> {
    /// The concrete `Arc<T>` when the value is a `T` (the same allocation), else the original.
    pub fn downcast<T: crate::any::Any + Send + Sync>(self) -> Result<Arc<T>, Arc<dyn crate::any::Any + Send + Sync>> {
        if crate::any::Any::type_id(&*self) == crate::any::TypeId::of::<T>() {
            let p = self.ptr.as_ptr() as *mut T;
            crate::mem::forget(self);
            Ok(Arc { ptr: unsafe { ptr::NonNull::new_unchecked(p) }, _marker: crate::marker::PhantomData })
        } else {
            Err(self)
        }
    }
}

impl<T: ?Sized> Clone for Arc<T> {
    fn clone(&self) -> Arc<T> {
        let old = unsafe { ia::atomic_fetch_add(strong_ptr(header_of(self.ptr.as_ptr())), 1usize, RELAXED) };
        if old > MAX_REFCOUNT {
            unsafe { crate::intrinsics_rt::abort() };
        }
        Arc { ptr: self.ptr, _marker: crate::marker::PhantomData }
    }
}

unsafe impl<#[may_dangle] T: ?Sized> Drop for Arc<T> {
    fn drop(&mut self) {
        let old = unsafe { ia::atomic_fetch_sub(strong_ptr(header_of(self.ptr.as_ptr())), 1usize, RELEASE) };
        if old != 1 {
            return;
        }
        unsafe { ia::atomic_fence(ACQUIRE) };
        self.drop_slow();
    }
}

impl<T: ?Sized> crate::ops::LegacyReceiver for Arc<T> {}
impl<T: ?Sized> Deref for Arc<T> {
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { &*self.ptr.as_ptr() }
    }
}

impl<T: ?Sized> AsRef<T> for Arc<T> {
    fn as_ref(&self) -> &T {
        unsafe { &*self.ptr.as_ptr() }
    }
}

impl<T: ?Sized> Borrow<T> for Arc<T> {
    fn borrow(&self) -> &T {
        unsafe { &*self.ptr.as_ptr() }
    }
}

impl<T: Default> Default for Arc<T> {
    fn default() -> Arc<T> {
        Arc::new(T::default())
    }
}

/// An empty slice (real alloc: one shared empty allocation; here a fresh empty block).
impl<T> Default for Arc<[T]> {
    fn default() -> Arc<[T]> {
        Arc::from(Vec::new())
    }
}

impl Default for Arc<str> {
    fn default() -> Arc<str> {
        Arc::from("")
    }
}

impl<T> From<T> for Arc<T> {
    fn from(t: T) -> Arc<T> {
        Arc::new(t)
    }
}

impl<'a> From<&'a str> for Arc<str> {
    fn from(v: &'a str) -> Arc<str> {
        let (_, p) = alloc_block(v.len(), 1, 1, 1);
        unsafe {
            ptr::copy_nonoverlapping(v.as_ptr(), p, v.len());
            Arc::from_parts(ptr::slice_from_raw_parts_mut(p, v.len()) as *mut str)
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
        let (_, p) = alloc_block(v.len() * crate::mem::size_of::<T>(), crate::mem::align_of::<T>(), 1, 1);
        let p = p as *mut T;
        let mut i = 0;
        while i < v.len() {
            unsafe { ptr::write(p.add(i), v[i].clone()) };
            i += 1;
        }
        unsafe { Arc::from_parts(ptr::slice_from_raw_parts_mut(p, v.len())) }
    }
}

/// Moves a boxed (possibly unsized) value into a new Arc block and frees the box's allocation.
impl<T: ?Sized> From<crate::boxed::Box<T>> for Arc<T> {
    fn from(b: crate::boxed::Box<T>) -> Arc<T> {
        unsafe {
            let raw = crate::boxed::Box::into_raw(b);
            let size = crate::intrinsics_mem::size_of_val(&*raw);
            let align = crate::intrinsics_mem::align_of_val(&*raw);
            let (_, np) = alloc_block(size, align, 1, 1);
            ptr::copy_nonoverlapping(raw as *const u8, np, size);
            crate::rc::free_box_allocation(raw as *mut u8, size, align);
            Arc::from_parts(crate::clone_uninit::with_addr(raw, np))
        }
    }
}

/// The array's elements moved into a new slice block (real alloc).
impl<T, const N: usize> From<[T; N]> for Arc<[T]> {
    fn from(v: [T; N]) -> Arc<[T]> {
        let (_, p) = alloc_block(N * crate::mem::size_of::<T>(), crate::mem::align_of::<T>(), 1, 1);
        let p = p as *mut T;
        unsafe {
            ptr::copy_nonoverlapping(&v as *const [T; N] as *const T, p, N);
            crate::mem::forget(v);
            Arc::from_parts(ptr::slice_from_raw_parts_mut(p, N))
        }
    }
}

/// `Arc<[T]>` of exactly N elements as `Arc<[T; N]>`, else the pointer back (alloc/src/arc.rs).
impl<T, const N: usize> crate::convert::TryFrom<Arc<[T]>> for Arc<[T; N]> {
    type Error = Arc<[T]>;
    fn try_from(boxed_slice: Arc<[T]>) -> Result<Arc<[T; N]>, Arc<[T]>> {
        if boxed_slice.len() == N {
            let ptr = Arc::into_raw(boxed_slice) as *mut [T; N];
            Ok(unsafe { Arc::from_raw(ptr) })
        } else {
            Err(boxed_slice)
        }
    }
}

impl<T> From<Vec<T>> for Arc<[T]> {
    fn from(v: Vec<T>) -> Arc<[T]> {
        let mut v = v;
        let len = v.len();
        let (_, p) = alloc_block(len * crate::mem::size_of::<T>(), crate::mem::align_of::<T>(), 1, 1);
        let p = p as *mut T;
        unsafe {
            ptr::copy_nonoverlapping(v.as_ptr(), p, len);
            v.set_len(0);
            Arc::from_parts(ptr::slice_from_raw_parts_mut(p, len))
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
        fmt::Pointer::fmt(&(self.ptr.as_ptr() as *const T), f)
    }
}

// ---------------------------------------------------------------- Weak

pub struct Weak<T: ?Sized> {
    /// The non-null `DANGLING` sentinel for `Weak::new()`.
    ptr: crate::ptr::NonNull<T>,
}

unsafe impl<T: ?Sized + Sync + Send> Send for Weak<T> {}
unsafe impl<T: ?Sized + Sync + Send> Sync for Weak<T> {}

impl<T> Weak<T> {
    pub const fn new() -> Weak<T> {
        Weak { ptr: unsafe { ptr::NonNull::new_unchecked(DANGLING as *mut T) } }
    }
}

impl<T: ?Sized> Weak<T> {
    pub fn upgrade(&self) -> Option<Arc<T>> {
        if is_dangling(self.ptr.as_ptr()) {
            return None;
        }
        let sp = strong_ptr(header_of(self.ptr.as_ptr()));
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
                return Some(Arc { ptr: self.ptr, _marker: crate::marker::PhantomData });
            }
            n = prev;
        }
    }

    pub fn strong_count(&self) -> usize {
        if is_dangling(self.ptr.as_ptr()) {
            0
        } else {
            unsafe { ia::atomic_load(strong_ptr(header_of(self.ptr.as_ptr())) as *const usize, ACQUIRE) }
        }
    }

    pub fn weak_count(&self) -> usize {
        if is_dangling(self.ptr.as_ptr()) {
            return 0;
        }
        let weak = unsafe { ia::atomic_load(weak_ptr(header_of(self.ptr.as_ptr())) as *const usize, ACQUIRE) };
        let strong = unsafe { ia::atomic_load(strong_ptr(header_of(self.ptr.as_ptr())) as *const usize, ACQUIRE) };
        if strong == 0 {
            0
        } else {
            weak - 1
        }
    }

    pub fn ptr_eq(&self, other: &Weak<T>) -> bool {
        self.ptr.as_ptr() as *const u8 == other.ptr.as_ptr() as *const u8
    }

    #[rustc_never_returns_null_ptr]
    pub fn as_ptr(&self) -> *const T {
        self.ptr.as_ptr() as *const T
    }
}

impl<T: ?Sized> Clone for Weak<T> {
    fn clone(&self) -> Weak<T> {
        if !is_dangling(self.ptr.as_ptr()) {
            let old = unsafe { ia::atomic_fetch_add(weak_ptr(header_of(self.ptr.as_ptr())), 1usize, RELAXED) };
            if old > MAX_REFCOUNT {
                unsafe { crate::intrinsics_rt::abort() };
            }
        }
        Weak { ptr: self.ptr }
    }
}

unsafe impl<#[may_dangle] T: ?Sized> Drop for Weak<T> {
    fn drop(&mut self) {
        if is_dangling(self.ptr.as_ptr()) {
            return;
        }
        let old = unsafe { ia::atomic_fetch_sub(weak_ptr(header_of(self.ptr.as_ptr())), 1usize, RELEASE) };
        if old == 1 {
            unsafe {
                ia::atomic_fence(ACQUIRE);
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

impl<T> Arc<T> {
    pub fn new_uninit() -> Arc<crate::mem::MaybeUninit<T>> {
        let (_, p) = alloc_block(crate::mem::size_of::<T>(), crate::mem::align_of::<T>(), 1, 1);
        Arc { ptr: unsafe { ptr::NonNull::new_unchecked(p as *mut crate::mem::MaybeUninit<T>) }, _marker: crate::marker::PhantomData }
    }
    pub fn new_zeroed() -> Arc<crate::mem::MaybeUninit<T>> {
        let (_, p) = alloc_block(crate::mem::size_of::<T>(), crate::mem::align_of::<T>(), 1, 1);
        unsafe { ptr::write_bytes(p, 0, crate::mem::size_of::<T>()) };
        Arc { ptr: unsafe { ptr::NonNull::new_unchecked(p as *mut crate::mem::MaybeUninit<T>) }, _marker: crate::marker::PhantomData }
    }
}

impl<T> Arc<[T]> {
    pub fn new_uninit_slice(len: usize) -> Arc<[crate::mem::MaybeUninit<T>]> {
        let size = match len.checked_mul(crate::mem::size_of::<T>()) {
            Some(s) => s,
            None => crate::panicking::panic_str("capacity overflow"),
        };
        let (_, p) = alloc_block(size, crate::mem::align_of::<T>(), 1, 1);
        let s = ptr::slice_from_raw_parts_mut(p as *mut crate::mem::MaybeUninit<T>, len);
        Arc { ptr: unsafe { ptr::NonNull::new_unchecked(s) }, _marker: crate::marker::PhantomData }
    }
    pub fn new_zeroed_slice(len: usize) -> Arc<[crate::mem::MaybeUninit<T>]> {
        let r = Arc::<[T]>::new_uninit_slice(len);
        unsafe { ptr::write_bytes(r.ptr.as_ptr() as *mut u8, 0, len * crate::mem::size_of::<T>()) };
        r
    }
}

impl<T> Arc<crate::mem::MaybeUninit<T>> {
    pub unsafe fn assume_init(self) -> Arc<T> {
        let p = self.ptr.as_ptr() as *mut T;
        crate::mem::forget(self);
        Arc { ptr: ptr::NonNull::new_unchecked(p), _marker: crate::marker::PhantomData }
    }
}

impl<T> Arc<[crate::mem::MaybeUninit<T>]> {
    pub unsafe fn assume_init(self) -> Arc<[T]> {
        let p = self.ptr.as_ptr() as *mut [T];
        crate::mem::forget(self);
        Arc { ptr: ptr::NonNull::new_unchecked(p), _marker: crate::marker::PhantomData }
    }
}

impl<T: ?Sized> Arc<T> {
    /// Mutable access without the uniqueness check (alloc's unstable get_mut_unchecked).
    pub unsafe fn get_mut_unchecked(this: &mut Arc<T>) -> &mut T {
        &mut *this.ptr.as_ptr()
    }
}
