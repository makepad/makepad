//! Shareable mutable containers: UnsafeCell, Cell, RefCell (+Ref/RefMut), OnceCell, LazyCell.

use crate::cmp::Ordering;
use crate::fmt;
use crate::ops::{Deref, DerefMut};

/// The primitive for interior mutability (Relax knows it: no `noalias` on what it covers).
#[repr(transparent)]
#[cfg_attr(not(relax_check), rustc_layout_no_niche)]
#[cfg_attr(not(relax_check), lang = "unsafe_cell")]
pub struct UnsafeCell<T: ?Sized> {
    value: T,
}

impl<T> UnsafeCell<T> {
    pub const fn new(value: T) -> UnsafeCell<T> {
        UnsafeCell { value }
    }
    pub const fn into_inner(self) -> T {
        self.value
    }
}

impl<T: ?Sized> UnsafeCell<T> {
    #[rustc_as_ptr]
    #[rustc_never_returns_null_ptr]
    #[rustc_should_not_be_called_on_const_items]
    pub const fn get(&self) -> *mut T {
        &self.value as *const T as *mut T
    }
    pub const fn get_mut(&mut self) -> &mut T {
        &mut self.value
    }
    #[rustc_should_not_be_called_on_const_items]
    pub unsafe fn replace(&self, value: T) -> T where T: Sized {
        crate::mem::replace(&mut *self.get(), value)
    }
    #[rustc_should_not_be_called_on_const_items]
    pub unsafe fn as_ref_unchecked(&self) -> &T {
        &*self.get()
    }
    #[rustc_should_not_be_called_on_const_items]
    pub unsafe fn as_mut_unchecked(&self) -> &mut T {
        &mut *self.get()
    }
    #[rustc_diagnostic_item = "unsafe_cell_raw_get"]
    pub const fn raw_get(this: *const UnsafeCell<T>) -> *mut T {
        this as *const T as *mut T
    }
    /// `&mut T` as `&mut UnsafeCell<T>` (repr(transparent)).
    pub const fn from_mut(value: &mut T) -> &mut UnsafeCell<T> {
        unsafe { &mut *(value as *mut T as *mut UnsafeCell<T>) }
    }
}

/// `UnsafeCell` that is `Sync` when `T: Sync` (unstable `sync_unsafe_cell`).
#[repr(transparent)]
#[rustc_diagnostic_item = "SyncUnsafeCell"]
pub struct SyncUnsafeCell<T: ?Sized> {
    value: UnsafeCell<T>,
}

unsafe impl<T: ?Sized + Sync> Sync for SyncUnsafeCell<T> {}

impl<T> SyncUnsafeCell<T> {
    pub const fn new(value: T) -> SyncUnsafeCell<T> {
        SyncUnsafeCell { value: UnsafeCell::new(value) }
    }
    pub fn into_inner(self) -> T {
        self.value.into_inner()
    }
}

impl<T: ?Sized> SyncUnsafeCell<T> {
    #[rustc_as_ptr]
    #[rustc_never_returns_null_ptr]
    #[rustc_should_not_be_called_on_const_items]
    pub const fn get(&self) -> *mut T {
        self.value.get()
    }
    pub fn get_mut(&mut self) -> &mut T {
        self.value.get_mut()
    }
}

impl<T: Default> Default for UnsafeCell<T> {
    fn default() -> UnsafeCell<T> {
        UnsafeCell::new(T::default())
    }
}

impl<T> From<T> for UnsafeCell<T> {
    fn from(t: T) -> UnsafeCell<T> {
        UnsafeCell::new(t)
    }
}

// ---------------------------------------------------------------- Cell

#[repr(transparent)]
#[rustc_diagnostic_item = "Cell"]
pub struct Cell<T: ?Sized> {
    value: UnsafeCell<T>,
}

unsafe impl<T: ?Sized + Send> Send for Cell<T> {}
impl<T: crate::ops::CoerceUnsized<U>, U> crate::ops::CoerceUnsized<Cell<U>> for Cell<T> {}
impl<T: crate::ops::CoerceUnsized<U>, U> crate::ops::CoerceUnsized<UnsafeCell<U>> for UnsafeCell<T> {}
impl<T: crate::ops::CoerceUnsized<U>, U> crate::ops::CoerceUnsized<RefCell<U>> for RefCell<T> {}
impl<T: crate::ops::CoerceUnsized<U>, U> crate::ops::CoerceUnsized<SyncUnsafeCell<U>> for SyncUnsafeCell<T> {}
impl<'b, T: ?Sized + crate::marker::Unsize<U>, U: ?Sized> crate::ops::CoerceUnsized<Ref<'b, U>> for Ref<'b, T> {}
impl<'b, T: ?Sized + crate::marker::Unsize<U>, U: ?Sized> crate::ops::CoerceUnsized<RefMut<'b, U>> for RefMut<'b, T> {}
// library/core/src/cell.rs:325,1439,2328: shared access to interior mutability is not thread-safe.
impl<T: ?Sized> !Sync for UnsafeCell<T> {}
impl<T: ?Sized> !Sync for Cell<T> {}
impl<T: ?Sized> !Sync for RefCell<T> {}
impl<T> !Sync for OnceCell<T> {}
// library/core/src/panic/unwind_safe.rs: UnsafeCell is the one type that is not RefUnwindSafe.
impl<T: ?Sized> !crate::panic::RefUnwindSafe for UnsafeCell<T> {}

impl<T> Cell<T> {
    pub const fn new(value: T) -> Cell<T> {
        Cell { value: UnsafeCell::new(value) }
    }
    #[rustc_should_not_be_called_on_const_items]
    pub const fn set(&self, val: T) {
        let old = self.replace(val);
        drop(old);
    }
    #[rustc_should_not_be_called_on_const_items]
    pub fn swap(&self, other: &Cell<T>) {
        if crate::ptr::eq(self, other) {
            return;
        }
        unsafe { crate::ptr::swap(self.value.get(), other.value.get()) }
    }
    #[rustc_should_not_be_called_on_const_items]
    pub const fn replace(&self, val: T) -> T {
        unsafe { crate::mem::replace(&mut *self.value.get(), val) }
    }
    pub const fn into_inner(self) -> T {
        self.value.into_inner()
    }
}

impl<T: Copy> Cell<T> {
    #[rustc_should_not_be_called_on_const_items]
    pub const fn get(&self) -> T {
        unsafe { *self.value.get() }
    }
    #[rustc_should_not_be_called_on_const_items]
    pub const fn update<F: FnOnce(T) -> T>(&self, f: F) {
        let old = self.get();
        self.set(f(old));
    }
}

impl<T: ?Sized> Cell<T> {
    #[rustc_as_ptr]
    #[rustc_never_returns_null_ptr]
    pub const fn as_ptr(&self) -> *mut T {
        self.value.get()
    }
    pub const fn get_mut(&mut self) -> &mut T {
        self.value.get_mut()
    }
    pub const fn from_mut(t: &mut T) -> &Cell<T> {
        unsafe { &*(t as *mut T as *const Cell<T>) }
    }
}

impl<T> Cell<[T]> {
    /// `&Cell<[T]>` as `&[Cell<T>]` (Cell is repr(transparent)).
    pub const fn as_slice_of_cells(&self) -> &[Cell<T>] {
        unsafe { &*(self as *const Cell<[T]> as *const [Cell<T>]) }
    }
}

impl<T, const N: usize> Cell<[T; N]> {
    /// `&Cell<[T; N]>` as `&[Cell<T>; N]`.
    pub const fn as_array_of_cells(&self) -> &[Cell<T>; N] {
        unsafe { &*(self as *const Cell<[T; N]> as *const [Cell<T>; N]) }
    }
}

impl<T: Default> Cell<T> {
    pub const fn take(&self) -> T {
        self.replace(T::default())
    }
}

impl<T: Copy> Clone for Cell<T> {
    fn clone(&self) -> Cell<T> {
        Cell::new(self.get())
    }
}
impl<T: Default> Default for Cell<T> {
    fn default() -> Cell<T> {
        Cell::new(T::default())
    }
}
impl<T: PartialEq + Copy> PartialEq for Cell<T> {
    fn eq(&self, other: &Cell<T>) -> bool {
        self.get() == other.get()
    }
}
impl<T: Eq + Copy> Eq for Cell<T> {}
impl<T: PartialOrd + Copy> PartialOrd for Cell<T> {
    fn partial_cmp(&self, other: &Cell<T>) -> Option<Ordering> {
        self.get().partial_cmp(&other.get())
    }
}
impl<T: Ord + Copy> Ord for Cell<T> {
    fn cmp(&self, other: &Cell<T>) -> Ordering {
        self.get().cmp(&other.get())
    }
}
impl<T> From<T> for Cell<T> {
    fn from(t: T) -> Cell<T> {
        Cell::new(t)
    }
}

// ---------------------------------------------------------------- RefCell

/// Borrow state: 0 unused, > 0 that many shared borrows, -1 mutably borrowed.
type BorrowFlag = isize;
const UNUSED: BorrowFlag = 0;

pub struct RefCell<T: ?Sized> {
    borrow: Cell<BorrowFlag>,
    value: UnsafeCell<T>,
}

unsafe impl<T: ?Sized + Send> Send for RefCell<T> {}

pub struct BorrowError {
    _private: (),
}

impl fmt::Debug for BorrowError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("BorrowError")
    }
}

impl fmt::Display for BorrowError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt("RefCell already mutably borrowed", f)
    }
}

pub struct BorrowMutError {
    _private: (),
}

impl fmt::Debug for BorrowMutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("BorrowMutError")
    }
}

impl fmt::Display for BorrowMutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt("RefCell already borrowed", f)
    }
}

#[cfg(not(relax_check))]
impl crate::error::Error for BorrowError {}
#[cfg(not(relax_check))]
impl crate::error::Error for BorrowMutError {}

#[cold]
#[track_caller]
fn panic_already_borrowed() -> ! {
    crate::panicking::panic_str("RefCell already borrowed")
}

#[cold]
#[track_caller]
const fn panic_already_mutably_borrowed() -> ! {
    crate::panicking::panic_str("RefCell already mutably borrowed")
}

impl<T> RefCell<T> {
    pub const fn new(value: T) -> RefCell<T> {
        RefCell { borrow: Cell::new(UNUSED), value: UnsafeCell::new(value) }
    }
    pub const fn into_inner(self) -> T {
        self.value.into_inner()
    }
    #[track_caller]
    #[rustc_should_not_be_called_on_const_items]
    pub const fn replace(&self, t: T) -> T {
        crate::mem::replace(&mut *self.borrow_mut(), t)
    }
    #[track_caller]
    #[rustc_should_not_be_called_on_const_items]
    pub fn replace_with<F: FnOnce(&mut T) -> T>(&self, f: F) -> T {
        let mut_borrow = &mut *self.borrow_mut();
        let replacement = f(mut_borrow);
        crate::mem::replace(mut_borrow, replacement)
    }
    #[track_caller]
    #[rustc_should_not_be_called_on_const_items]
    pub const fn swap(&self, other: &RefCell<T>) {
        crate::mem::swap(&mut *self.borrow_mut(), &mut *other.borrow_mut())
    }
}

impl<T: ?Sized> RefCell<T> {
    #[track_caller]
    #[rustc_should_not_be_called_on_const_items]
    pub const fn borrow(&self) -> Ref<'_, T> {
        match self.try_borrow() {
            Ok(b) => b,
            Err(_) => panic_already_mutably_borrowed(),
        }
    }
    #[rustc_should_not_be_called_on_const_items]
    pub const fn try_borrow(&self) -> Result<Ref<'_, T>, BorrowError> {
        let b = self.borrow.get();
        if b < 0 || b == isize::MAX {
            Err(BorrowError { _private: () })
        } else {
            self.borrow.set(b + 1);
            Ok(Ref { value: unsafe { &*self.value.get() }, borrow: &self.borrow })
        }
    }
    #[track_caller]
    #[rustc_should_not_be_called_on_const_items]
    pub const fn borrow_mut(&self) -> RefMut<'_, T> {
        match self.try_borrow_mut() {
            Ok(b) => b,
            Err(_) => panic_already_borrowed(),
        }
    }
    #[rustc_should_not_be_called_on_const_items]
    pub const fn try_borrow_mut(&self) -> Result<RefMut<'_, T>, BorrowMutError> {
        if self.borrow.get() != UNUSED {
            Err(BorrowMutError { _private: () })
        } else {
            self.borrow.set(-1);
            Ok(RefMut { value: unsafe { &mut *self.value.get() }, borrow: &self.borrow })
        }
    }
    #[rustc_never_returns_null_ptr]
    pub const fn as_ptr(&self) -> *mut T {
        self.value.get()
    }
    pub const fn get_mut(&mut self) -> &mut T {
        self.value.get_mut()
    }
    pub const unsafe fn try_borrow_unguarded(&self) -> Result<&T, BorrowError> {
        if self.borrow.get() < 0 {
            Err(BorrowError { _private: () })
        } else {
            Ok(&*self.value.get())
        }
    }
}

impl<T: Default> RefCell<T> {
    pub fn take(&self) -> T {
        self.replace(T::default())
    }
}

impl<T: Clone> Clone for RefCell<T> {
    #[track_caller]
    fn clone(&self) -> RefCell<T> {
        RefCell::new(self.borrow().clone())
    }
}
impl<T: Default> Default for RefCell<T> {
    fn default() -> RefCell<T> {
        RefCell::new(T::default())
    }
}
impl<T: ?Sized + PartialEq> PartialEq for RefCell<T> {
    fn eq(&self, other: &RefCell<T>) -> bool {
        *self.borrow() == *other.borrow()
    }
}
impl<T: ?Sized + Eq> Eq for RefCell<T> {}
impl<T: ?Sized + PartialOrd> PartialOrd for RefCell<T> {
    fn partial_cmp(&self, other: &RefCell<T>) -> Option<Ordering> {
        self.borrow().partial_cmp(&*other.borrow())
    }
}
impl<T: ?Sized + Ord> Ord for RefCell<T> {
    fn cmp(&self, other: &RefCell<T>) -> Ordering {
        self.borrow().cmp(&*other.borrow())
    }
}
impl<T> From<T> for RefCell<T> {
    fn from(t: T) -> RefCell<T> {
        RefCell::new(t)
    }
}

pub struct Ref<'b, T: ?Sized + 'b> {
    value: &'b T,
    borrow: &'b Cell<BorrowFlag>,
}

impl<'b, T: ?Sized> Drop for Ref<'b, T> {
    fn drop(&mut self) {
        self.borrow.set(self.borrow.get() - 1);
    }
}

impl<'b, T: ?Sized> Deref for Ref<'b, T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.value
    }
}

impl<'b, T: ?Sized> Ref<'b, T> {
    pub const fn clone(orig: &Ref<'b, T>) -> Ref<'b, T> {
        orig.borrow.set(orig.borrow.get() + 1);
        Ref { value: orig.value, borrow: orig.borrow }
    }
    pub fn map<U: ?Sized, F: FnOnce(&T) -> &U>(orig: Ref<'b, T>, f: F) -> Ref<'b, U> {
        let value: *const T = orig.value;
        let borrow = orig.borrow;
        crate::mem::forget(orig);
        Ref { value: f(unsafe { &*value }), borrow }
    }
    pub fn filter_map<U: ?Sized, F: FnOnce(&T) -> Option<&U>>(orig: Ref<'b, T>, f: F) -> Result<Ref<'b, U>, Ref<'b, T>> {
        let value: *const T = orig.value;
        match f(unsafe { &*value }) {
            Some(u) => {
                let borrow = orig.borrow;
                crate::mem::forget(orig);
                Ok(Ref { value: u, borrow })
            }
            None => Err(orig),
        }
    }
    pub fn map_split<U: ?Sized, V: ?Sized, F: FnOnce(&T) -> (&U, &V)>(orig: Ref<'b, T>, f: F) -> (Ref<'b, U>, Ref<'b, V>) {
        let value: *const T = orig.value;
        let borrow = orig.borrow;
        crate::mem::forget(orig);
        let (a, b) = f(unsafe { &*value });
        borrow.set(borrow.get() + 1);
        (Ref { value: a, borrow }, Ref { value: b, borrow })
    }
}

impl<'b, T: ?Sized + fmt::Display> fmt::Display for Ref<'b, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self.value, f)
    }
}

pub struct RefMut<'b, T: ?Sized + 'b> {
    value: &'b mut T,
    borrow: &'b Cell<BorrowFlag>,
}

impl<'b, T: ?Sized> Drop for RefMut<'b, T> {
    fn drop(&mut self) {
        self.borrow.set(self.borrow.get() + 1);
    }
}

impl<'b, T: ?Sized> Deref for RefMut<'b, T> {
    type Target = T;
    fn deref(&self) -> &T {
        self.value
    }
}

impl<'b, T: ?Sized> DerefMut for RefMut<'b, T> {
    fn deref_mut(&mut self) -> &mut T {
        self.value
    }
}

impl<'b, T: ?Sized> RefMut<'b, T> {
    pub fn map<U: ?Sized, F: FnOnce(&mut T) -> &mut U>(orig: RefMut<'b, T>, f: F) -> RefMut<'b, U> {
        let value: *mut T = &mut *orig.value;
        let borrow = orig.borrow;
        crate::mem::forget(orig);
        RefMut { value: f(unsafe { &mut *value }), borrow }
    }
    pub fn filter_map<U: ?Sized, F: FnOnce(&mut T) -> Option<&mut U>>(orig: RefMut<'b, T>, f: F) -> Result<RefMut<'b, U>, RefMut<'b, T>> {
        let value: *mut T = &mut *orig.value;
        match f(unsafe { &mut *value }) {
            Some(u) => {
                let borrow = orig.borrow;
                crate::mem::forget(orig);
                Ok(RefMut { value: u, borrow })
            }
            None => Err(orig),
        }
    }
}

impl<'b, T: ?Sized + fmt::Display> fmt::Display for RefMut<'b, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&*self.value, f)
    }
}

// ---------------------------------------------------------------- OnceCell / LazyCell

pub struct OnceCell<T> {
    inner: UnsafeCell<Option<T>>,
}

impl<T> OnceCell<T> {
    pub const fn new() -> OnceCell<T> {
        OnceCell { inner: UnsafeCell::new(None) }
    }
    pub fn get(&self) -> Option<&T> {
        unsafe { (*self.inner.get()).as_ref() }
    }
    pub fn get_mut(&mut self) -> Option<&mut T> {
        self.inner.get_mut().as_mut()
    }
    #[rustc_should_not_be_called_on_const_items]
    pub fn set(&self, value: T) -> Result<(), T> {
        if self.get().is_some() {
            return Err(value);
        }
        unsafe { *self.inner.get() = Some(value) };
        Ok(())
    }
    #[track_caller]
    #[rustc_should_not_be_called_on_const_items]
    pub fn get_or_init<F: FnOnce() -> T>(&self, f: F) -> &T {
        if let Some(v) = self.get() {
            return v;
        }
        let val = f();
        if self.get().is_some() {
            crate::panicking::panic_str("reentrant init");
        }
        unsafe { *self.inner.get() = Some(val) };
        match self.get() {
            Some(v) => v,
            None => unsafe { crate::hint::unreachable_unchecked() },
        }
    }
    #[rustc_should_not_be_called_on_const_items]
    pub fn try_insert(&self, value: T) -> Result<&T, (&T, T)> {
        if let Some(old) = self.get() {
            return Err((old, value));
        }
        unsafe { *self.inner.get() = Some(value) };
        match self.get() {
            Some(v) => Ok(v),
            None => unsafe { crate::hint::unreachable_unchecked() },
        }
    }
    #[rustc_should_not_be_called_on_const_items]
    pub fn get_or_try_init<F, E>(&self, f: F) -> Result<&T, E> where F: FnOnce() -> Result<T, E> {
        if let Some(v) = self.get() {
            return Ok(v);
        }
        let val = f()?;
        if self.get().is_some() {
            crate::panicking::panic_str("reentrant init");
        }
        unsafe { *self.inner.get() = Some(val) };
        match self.get() {
            Some(v) => Ok(v),
            None => unsafe { crate::hint::unreachable_unchecked() },
        }
    }
    pub const fn into_inner(self) -> Option<T> {
        self.inner.into_inner()
    }
    pub fn take(&mut self) -> Option<T> {
        crate::mem::replace(self, OnceCell::new()).into_inner()
    }
}

impl<T> Default for OnceCell<T> {
    fn default() -> OnceCell<T> {
        OnceCell::new()
    }
}

impl<T: Clone> Clone for OnceCell<T> {
    fn clone(&self) -> OnceCell<T> {
        let res = OnceCell::new();
        if let Some(value) = self.get() {
            let _ = res.set(value.clone());
        }
        res
    }
}

impl<T: fmt::Debug> fmt::Debug for OnceCell<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut d = f.debug_tuple("OnceCell");
        match self.get() {
            Some(v) => d.field(v),
            None => d.field(&Uninit),
        };
        d.finish()
    }
}

struct Uninit;

impl fmt::Debug for Uninit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("<uninit>")
    }
}

impl<T: PartialEq> PartialEq for OnceCell<T> {
    fn eq(&self, other: &OnceCell<T>) -> bool {
        self.get() == other.get()
    }
}

impl<T> From<T> for OnceCell<T> {
    fn from(value: T) -> OnceCell<T> {
        OnceCell { inner: UnsafeCell::new(Some(value)) }
    }
}

pub struct LazyCell<T, F = fn() -> T> {
    cell: OnceCell<T>,
    init: Cell<Option<F>>,
}

impl<T, F: FnOnce() -> T> LazyCell<T, F> {
    pub const fn new(f: F) -> LazyCell<T, F> {
        LazyCell { cell: OnceCell::new(), init: Cell::new(Some(f)) }
    }
    #[rustc_should_not_be_called_on_const_items]
    pub fn force(this: &LazyCell<T, F>) -> &T {
        this.cell.get_or_init(|| match this.init.take() {
            Some(f) => f(),
            None => crate::panicking::panic_str("LazyCell instance has previously been poisoned"),
        })
    }
}

impl<T, F: FnOnce() -> T> Deref for LazyCell<T, F> {
    type Target = T;
    fn deref(&self) -> &T {
        LazyCell::force(self)
    }
}
