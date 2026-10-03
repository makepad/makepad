//! core::sync::atomic: atomic integers, bool and pointers over `intrinsics_atomic`.
//!
//! The integer types are one generic `Atomic<T>` with `AtomicU32 = Atomic<u32>` etc. aliases
//! (the shape newer real core has), so every operation has one body; the intrinsics are
//! generic over the integer width. Orderings are passed as `Ordering as u8`
//! (0 Relaxed, 1 Release, 2 Acquire, 3 AcqRel, 4 SeqCst).

use crate::cell::UnsafeCell;
use crate::fmt;
use crate::intrinsics_atomic as ia;

#[derive(Copy, Clone, Debug, Eq, PartialEq, Hash)]
#[non_exhaustive]
pub enum Ordering {
    Relaxed,
    Release,
    Acquire,
    AcqRel,
    SeqCst,
}

fn load_order(order: Ordering) -> u8 {
    match order {
        Ordering::Release => panic!("there is no such thing as a release load"),
        Ordering::AcqRel => panic!("there is no such thing as an acquire-release load"),
        _ => order as u8,
    }
}

fn store_order(order: Ordering) -> u8 {
    match order {
        Ordering::Acquire => panic!("there is no such thing as an acquire store"),
        Ordering::AcqRel => panic!("there is no such thing as an acquire-release store"),
        _ => order as u8,
    }
}

fn failure_order(order: Ordering) -> u8 {
    match order {
        Ordering::Release => panic!("there is no such thing as a release failure ordering"),
        Ordering::AcqRel => panic!("there is no such thing as an acquire-release failure ordering"),
        _ => order as u8,
    }
}

/// The integer types an `Atomic<T>` can hold. Implemented for the primitive integers only.
pub trait AtomicPrimitive: Copy {}
impl AtomicPrimitive for u8 {}
impl AtomicPrimitive for u16 {}
impl AtomicPrimitive for u32 {}
impl AtomicPrimitive for u64 {}
impl AtomicPrimitive for usize {}
impl AtomicPrimitive for i8 {}
impl AtomicPrimitive for i16 {}
impl AtomicPrimitive for i32 {}
impl AtomicPrimitive for i64 {}
impl AtomicPrimitive for isize {}

/// An integer shared between threads.
#[repr(transparent)]
pub struct Atomic<T: AtomicPrimitive> {
    v: UnsafeCell<T>,
}

pub type AtomicU8 = Atomic<u8>;
pub type AtomicU16 = Atomic<u16>;
pub type AtomicU32 = Atomic<u32>;
pub type AtomicU64 = Atomic<u64>;
pub type AtomicUsize = Atomic<usize>;
pub type AtomicI8 = Atomic<i8>;
pub type AtomicI16 = Atomic<i16>;
pub type AtomicI32 = Atomic<i32>;
pub type AtomicI64 = Atomic<i64>;
pub type AtomicIsize = Atomic<isize>;

unsafe impl<T: AtomicPrimitive> Sync for Atomic<T> {}
unsafe impl<T: AtomicPrimitive> Send for Atomic<T> {}

impl<T: AtomicPrimitive> Atomic<T> {
    pub const fn new(v: T) -> Atomic<T> {
        Atomic { v: UnsafeCell::new(v) }
    }
    pub fn get_mut(&mut self) -> &mut T {
        self.v.get_mut()
    }
    pub fn into_inner(self) -> T {
        self.v.into_inner()
    }
    pub const fn as_ptr(&self) -> *mut T {
        self.v.get()
    }
    pub fn load(&self, order: Ordering) -> T {
        unsafe { ia::atomic_load(self.v.get() as *const T, load_order(order)) }
    }
    pub fn store(&self, val: T, order: Ordering) {
        unsafe { ia::atomic_store(self.v.get(), val, store_order(order)) }
    }
    pub fn swap(&self, val: T, order: Ordering) -> T {
        unsafe { ia::atomic_swap(self.v.get(), val, order as u8) }
    }
    pub fn compare_exchange(&self, current: T, new: T, success: Ordering, failure: Ordering) -> Result<T, T> {
        let f = failure_order(failure);
        let (prev, ok) = unsafe { ia::atomic_cxchg(self.v.get(), current, new, success as u8, f) };
        if ok {
            Ok(prev)
        } else {
            Err(prev)
        }
    }
    pub fn compare_exchange_weak(&self, current: T, new: T, success: Ordering, failure: Ordering) -> Result<T, T> {
        let f = failure_order(failure);
        let (prev, ok) = unsafe { ia::atomic_cxchg_weak(self.v.get(), current, new, success as u8, f) };
        if ok {
            Ok(prev)
        } else {
            Err(prev)
        }
    }
    pub fn fetch_add(&self, val: T, order: Ordering) -> T {
        unsafe { ia::atomic_fetch_add(self.v.get(), val, order as u8) }
    }
    pub fn fetch_sub(&self, val: T, order: Ordering) -> T {
        unsafe { ia::atomic_fetch_sub(self.v.get(), val, order as u8) }
    }
    pub fn fetch_and(&self, val: T, order: Ordering) -> T {
        unsafe { ia::atomic_fetch_and(self.v.get(), val, order as u8) }
    }
    pub fn fetch_nand(&self, val: T, order: Ordering) -> T {
        unsafe { ia::atomic_fetch_nand(self.v.get(), val, order as u8) }
    }
    pub fn fetch_or(&self, val: T, order: Ordering) -> T {
        unsafe { ia::atomic_fetch_or(self.v.get(), val, order as u8) }
    }
    pub fn fetch_xor(&self, val: T, order: Ordering) -> T {
        unsafe { ia::atomic_fetch_xor(self.v.get(), val, order as u8) }
    }
    pub fn fetch_max(&self, val: T, order: Ordering) -> T {
        unsafe { ia::atomic_fetch_max(self.v.get(), val, order as u8) }
    }
    pub fn fetch_min(&self, val: T, order: Ordering) -> T {
        unsafe { ia::atomic_fetch_min(self.v.get(), val, order as u8) }
    }
    pub fn fetch_update<F: FnMut(T) -> Option<T>>(&self, set_order: Ordering, fetch_order: Ordering, mut f: F) -> Result<T, T> {
        let mut prev = self.load(fetch_order);
        loop {
            match f(prev) {
                Some(next) => match self.compare_exchange_weak(prev, next, set_order, fetch_order) {
                    Ok(x) => return Ok(x),
                    Err(next_prev) => prev = next_prev,
                },
                None => return Err(prev),
            }
        }
    }
}

impl<T: AtomicPrimitive + Default> Default for Atomic<T> {
    fn default() -> Atomic<T> {
        Atomic::new(T::default())
    }
}

impl<T: AtomicPrimitive> From<T> for Atomic<T> {
    fn from(v: T) -> Atomic<T> {
        Atomic::new(v)
    }
}

impl<T: AtomicPrimitive + fmt::Debug> fmt::Debug for Atomic<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Debug::fmt(&self.load(Ordering::Relaxed), f)
    }
}

/// A boolean shared between threads (one byte, 0 or 1).
#[repr(transparent)]
pub struct AtomicBool {
    v: UnsafeCell<u8>,
}

unsafe impl Sync for AtomicBool {}
unsafe impl Send for AtomicBool {}

impl AtomicBool {
    pub const fn new(v: bool) -> AtomicBool {
        AtomicBool { v: UnsafeCell::new(v as u8) }
    }
    pub fn get_mut(&mut self) -> &mut bool {
        unsafe { &mut *(self.v.get() as *mut bool) }
    }
    pub fn into_inner(self) -> bool {
        self.v.into_inner() != 0
    }
    pub const fn as_ptr(&self) -> *mut bool {
        self.v.get() as *mut bool
    }
    pub fn load(&self, order: Ordering) -> bool {
        unsafe { ia::atomic_load(self.v.get() as *const u8, load_order(order)) != 0 }
    }
    pub fn store(&self, val: bool, order: Ordering) {
        unsafe { ia::atomic_store(self.v.get(), val as u8, store_order(order)) }
    }
    pub fn swap(&self, val: bool, order: Ordering) -> bool {
        unsafe { ia::atomic_swap(self.v.get(), val as u8, order as u8) != 0 }
    }
    pub fn compare_exchange(&self, current: bool, new: bool, success: Ordering, failure: Ordering) -> Result<bool, bool> {
        let f = failure_order(failure);
        let (prev, ok) = unsafe { ia::atomic_cxchg(self.v.get(), current as u8, new as u8, success as u8, f) };
        if ok {
            Ok(prev != 0)
        } else {
            Err(prev != 0)
        }
    }
    pub fn compare_exchange_weak(&self, current: bool, new: bool, success: Ordering, failure: Ordering) -> Result<bool, bool> {
        let f = failure_order(failure);
        let (prev, ok) = unsafe { ia::atomic_cxchg_weak(self.v.get(), current as u8, new as u8, success as u8, f) };
        if ok {
            Ok(prev != 0)
        } else {
            Err(prev != 0)
        }
    }
    pub fn fetch_and(&self, val: bool, order: Ordering) -> bool {
        unsafe { ia::atomic_fetch_and(self.v.get(), val as u8, order as u8) != 0 }
    }
    pub fn fetch_or(&self, val: bool, order: Ordering) -> bool {
        unsafe { ia::atomic_fetch_or(self.v.get(), val as u8, order as u8) != 0 }
    }
    pub fn fetch_xor(&self, val: bool, order: Ordering) -> bool {
        unsafe { ia::atomic_fetch_xor(self.v.get(), val as u8, order as u8) != 0 }
    }
    pub fn fetch_nand(&self, val: bool, order: Ordering) -> bool {
        // !(x & true) == !x, !(x & false) == true
        if val {
            self.fetch_xor(true, order)
        } else {
            self.swap(true, order)
        }
    }
    pub fn fetch_not(&self, order: Ordering) -> bool {
        self.fetch_xor(true, order)
    }
    pub fn fetch_update<F: FnMut(bool) -> Option<bool>>(&self, set_order: Ordering, fetch_order: Ordering, mut f: F) -> Result<bool, bool> {
        let mut prev = self.load(fetch_order);
        loop {
            match f(prev) {
                Some(next) => match self.compare_exchange_weak(prev, next, set_order, fetch_order) {
                    Ok(x) => return Ok(x),
                    Err(next_prev) => prev = next_prev,
                },
                None => return Err(prev),
            }
        }
    }
}

impl Default for AtomicBool {
    fn default() -> AtomicBool {
        AtomicBool::new(false)
    }
}

impl From<bool> for AtomicBool {
    fn from(v: bool) -> AtomicBool {
        AtomicBool::new(v)
    }
}

impl fmt::Debug for AtomicBool {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Debug::fmt(&self.load(Ordering::Relaxed), f)
    }
}

/// A raw pointer shared between threads.
#[repr(transparent)]
pub struct AtomicPtr<T> {
    p: UnsafeCell<*mut T>,
}

unsafe impl<T> Sync for AtomicPtr<T> {}
unsafe impl<T> Send for AtomicPtr<T> {}

impl<T> AtomicPtr<T> {
    pub const fn new(p: *mut T) -> AtomicPtr<T> {
        AtomicPtr { p: UnsafeCell::new(p) }
    }
    pub fn get_mut(&mut self) -> &mut *mut T {
        self.p.get_mut()
    }
    pub fn into_inner(self) -> *mut T {
        self.p.into_inner()
    }
    pub const fn as_ptr(&self) -> *mut *mut T {
        self.p.get()
    }
    pub fn load(&self, order: Ordering) -> *mut T {
        unsafe { ia::atomic_load(self.p.get() as *const *mut T, load_order(order)) }
    }
    pub fn store(&self, ptr: *mut T, order: Ordering) {
        unsafe { ia::atomic_store(self.p.get(), ptr, store_order(order)) }
    }
    pub fn swap(&self, ptr: *mut T, order: Ordering) -> *mut T {
        unsafe { ia::atomic_swap(self.p.get(), ptr, order as u8) }
    }
    pub fn compare_exchange(&self, current: *mut T, new: *mut T, success: Ordering, failure: Ordering) -> Result<*mut T, *mut T> {
        let f = failure_order(failure);
        let (prev, ok) = unsafe { ia::atomic_cxchg(self.p.get(), current, new, success as u8, f) };
        if ok {
            Ok(prev)
        } else {
            Err(prev)
        }
    }
    pub fn compare_exchange_weak(&self, current: *mut T, new: *mut T, success: Ordering, failure: Ordering) -> Result<*mut T, *mut T> {
        let f = failure_order(failure);
        let (prev, ok) = unsafe { ia::atomic_cxchg_weak(self.p.get(), current, new, success as u8, f) };
        if ok {
            Ok(prev)
        } else {
            Err(prev)
        }
    }
    pub fn fetch_update<F: FnMut(*mut T) -> Option<*mut T>>(&self, set_order: Ordering, fetch_order: Ordering, mut f: F) -> Result<*mut T, *mut T> {
        let mut prev = self.load(fetch_order);
        loop {
            match f(prev) {
                Some(next) => match self.compare_exchange_weak(prev, next, set_order, fetch_order) {
                    Ok(x) => return Ok(x),
                    Err(next_prev) => prev = next_prev,
                },
                None => return Err(prev),
            }
        }
    }
}

impl<T> Default for AtomicPtr<T> {
    fn default() -> AtomicPtr<T> {
        AtomicPtr::new(crate::ptr::null_mut())
    }
}

impl<T> From<*mut T> for AtomicPtr<T> {
    fn from(p: *mut T) -> AtomicPtr<T> {
        AtomicPtr::new(p)
    }
}

impl<T> fmt::Debug for AtomicPtr<T> {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Debug::fmt(&self.load(Ordering::Relaxed), f)
    }
}

pub fn fence(order: Ordering) {
    match order {
        Ordering::Relaxed => panic!("there is no such thing as a relaxed fence"),
        _ => unsafe { ia::atomic_fence(order as u8) },
    }
}

pub fn compiler_fence(order: Ordering) {
    match order {
        Ordering::Relaxed => panic!("there is no such thing as a relaxed fence"),
        _ => unsafe { ia::atomic_compiler_fence(order as u8) },
    }
}

#[deprecated]
pub fn spin_loop_hint() {
    unsafe { ia::spin_loop_hint() }
}
