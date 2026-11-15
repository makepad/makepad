//! Box<T, A>: an owned heap allocation in allocator A, Global unless one is named (rustc 1.100's
//! allocator_api; library/alloc/src/boxed.rs @1.100.0). Relax knows Box: `*b` moves out,
//! Box<T> -> Box<dyn Tr> and Box<[T; N]> -> Box<[T]> unsize, and calls through Box<dyn Fn*> are
//! built in. Its allocator is its last field: a Box is its pointer's leaves, then A's.
//! Constructors without an allocator are the global allocator's, as library/'s are; so are the
//! conversions from slices, arrays, str, String, Cow and iterators.

use crate::any::Any;
use crate::borrow::{Borrow, BorrowMut};
use crate::cmp::Ordering;
use crate::error::Error;
use crate::fmt;
use crate::hash::{Hash, Hasher};
use crate::heap::{self, Allocator, Global, Layout, StaticAllocator};
use crate::intrinsics_mem as im;
use crate::marker::{PhantomData, Unsize};
use crate::mem::MaybeUninit;
use crate::ops::{Deref, DerefMut, DispatchFromDyn};
use crate::pin::{Pin, PinSafePointer};
use crate::ptr::NonNull;

// library/alloc/src/boxed.rs @1.100.0: a pinned Box needs an allocator that keeps its blocks
// (StaticAllocator); only the global allocator's Box dispatches `self: Box<Self>` calls
unsafe impl<T: ?Sized, A: StaticAllocator> PinSafePointer for Box<T, A> {}
impl<T: ?Sized + Unsize<U>, U: ?Sized> DispatchFromDyn<Box<U>> for Box<T, Global> {}
impl<T: ?Sized + Unsize<U>, U: ?Sized, A: Allocator> crate::ops::CoerceUnsized<Box<U, A>> for Box<T, A> {}
use crate::string::String;
use crate::vec::Vec;

#[fundamental]
#[rustc_diagnostic_item = "Box"]
#[lang = "owned_box"]
#[rustc_insignificant_dtor]
pub struct Box<T: ?Sized, A: Allocator = Global> {
    ptr: NonNull<T>,
    _marker: PhantomData<T>,
    alloc: A,
}

unsafe impl<T: ?Sized + Send, A: Allocator + Send> Send for Box<T, A> {}
unsafe impl<T: ?Sized + Sync, A: Allocator + Sync> Sync for Box<T, A> {}

impl<T> Box<T> {
    #[rustc_diagnostic_item = "box_new"]
    pub fn new(x: T) -> Box<T> {
        let p = heap::alloc_or_abort(Layout::new::<T>()) as *mut T;
        unsafe { im::ptr_write(p, x) };
        Box { ptr: unsafe { NonNull::new_unchecked(p) }, _marker: PhantomData, alloc: Global }
    }
    pub fn pin(x: T) -> Pin<Box<T>> {
        unsafe { Pin::new_unchecked(Box::new(x)) }
    }
    pub fn new_uninit() -> Box<MaybeUninit<T>> {
        let p = heap::alloc_or_abort(Layout::new::<T>()) as *mut MaybeUninit<T>;
        Box { ptr: unsafe { NonNull::new_unchecked(p) }, _marker: PhantomData, alloc: Global }
    }
    pub fn new_zeroed() -> Box<MaybeUninit<T>> {
        let b = Box::<T>::new_uninit();
        unsafe { im::write_bytes(b.ptr.as_ptr() as *mut u8, 0, crate::mem::size_of::<T>()) };
        b
    }
}

impl<T, A: Allocator> Box<T, A> {
    /// rustc 1.100.0 (allocator_api): `x` in a block of `alloc`; a zero-sized T takes none (its
    /// pointer is dangling, as library/'s)
    pub fn new_in(x: T, alloc: A) -> Self {
        let boxed = Self::new_uninit_in(alloc);
        Box::write(boxed, x)
    }
    /// (unstable allocator_ext) an uninitialized block of `alloc`; its failure is
    /// handle_alloc_error's, as library/'s
    pub fn new_uninit_in(alloc: A) -> Box<MaybeUninit<T>, A> {
        let ptr = if crate::mem::size_of::<T>() == 0 {
            NonNull::dangling()
        } else {
            let layout = Layout::new::<MaybeUninit<T>>();
            match alloc.allocate(layout) {
                Ok(p) => p.cast::<MaybeUninit<T>>(),
                Err(_) => heap::handle_alloc_error(layout),
            }
        };
        unsafe { Box::from_non_null_in(ptr, alloc) }
    }
    /// Moves the value out and frees the allocation, as `*boxed` does.
    pub fn into_inner(boxed: Self) -> T {
        *boxed
    }
    /// rustc 1.100.0 (box_take): the value, and its allocation left uninitialized
    pub fn take(boxed: Self) -> (T, Box<MaybeUninit<T>, A>) {
        unsafe {
            let (raw, alloc) = Box::into_non_null_with_allocator(boxed);
            let value = raw.read();
            let uninit = Box::from_non_null_in(raw.cast::<MaybeUninit<T>>(), alloc);
            (value, uninit)
        }
    }
    /// rustc 1.100.0 (smart_pointer_map): `f` of the value, in the same allocation when `U` fits it
    /// exactly; else that one is freed before `f` runs and the result gets a new one
    pub fn map<U>(this: Self, f: impl FnOnce(T) -> U) -> Box<U, A> {
        let (value, allocation) = Box::take(this);
        let (raw, alloc) = Box::into_non_null_with_allocator(allocation);
        if crate::mem::size_of::<T>() == crate::mem::size_of::<U>() && crate::mem::align_of::<T>() == crate::mem::align_of::<U>() {
            let allocation = unsafe { Box::from_non_null_in(raw.cast::<MaybeUninit<U>>(), alloc) };
            Box::write(allocation, f(value))
        } else {
            if crate::mem::size_of::<T>() != 0 {
                unsafe { alloc.deallocate(raw.cast(), Layout::new::<T>()) }
            }
            Box::new_in(f(value), alloc)
        }
    }
}

impl<T: ?Sized> Box<T> {
    #[must_use = "call `drop(Box::from_raw(ptr))` if you intend to drop the `Box`"]
    pub unsafe fn from_raw(raw: *mut T) -> Box<T> {
        Box { ptr: unsafe { NonNull::new_unchecked(raw) }, _marker: PhantomData, alloc: Global }
    }
    #[must_use = "call `drop(Box::from_non_null(ptr))` if you intend to drop the `Box`"]
    pub unsafe fn from_non_null(ptr: NonNull<T>) -> Box<T> {
        Box { ptr, _marker: PhantomData, alloc: Global }
    }
    pub fn into_raw(b: Box<T>) -> *mut T {
        let p = b.ptr.as_ptr();
        crate::mem::forget(b);
        p
    }
    pub fn into_non_null(b: Box<T>) -> NonNull<T> {
        let p = b.ptr;
        crate::mem::forget(b);
        p
    }
}

impl<T: ?Sized, A: Allocator> Box<T, A> {
    /// rustc 1.100.0 (allocator_api): the Box owning `raw`, a block of `alloc` holding a T
    pub unsafe fn from_raw_in(raw: *mut T, alloc: A) -> Self {
        Box { ptr: unsafe { NonNull::new_unchecked(raw) }, _marker: PhantomData, alloc }
    }
    pub unsafe fn from_non_null_in(raw: NonNull<T>, alloc: A) -> Self {
        Box { ptr: raw, _marker: PhantomData, alloc }
    }
    /// The pointer and the allocator, the Box given up (its block is the caller's to free).
    pub fn into_raw_with_allocator(b: Self) -> (*mut T, A) {
        let (p, alloc) = Box::into_non_null_with_allocator(b);
        (p.as_ptr(), alloc)
    }
    pub fn into_non_null_with_allocator(b: Self) -> (NonNull<T>, A) {
        let p = b.ptr;
        let alloc = unsafe { im::ptr_read(&b.alloc as *const A) };
        crate::mem::forget(b);
        (p, alloc)
    }
    pub fn allocator(b: &Self) -> &A {
        &b.alloc
    }
    pub fn leak<'a>(b: Self) -> &'a mut T
    where
        A: 'a,
    {
        let (p, alloc) = Box::into_non_null_with_allocator(b);
        crate::mem::forget(alloc);
        unsafe { &mut *p.as_ptr() }
    }
    #[rustc_never_returns_null_ptr]
    pub fn as_ptr(b: &Self) -> *const T {
        b.ptr.as_ptr() as *const T
    }
    #[rustc_never_returns_null_ptr]
    pub fn as_mut_ptr(b: &mut Self) -> *mut T {
        b.ptr.as_ptr()
    }
    pub fn into_pin(boxed: Self) -> Pin<Self>
    where
        A: StaticAllocator,
    {
        unsafe { Pin::new_unchecked(boxed) }
    }
}

impl<T> Box<[T]> {
    pub fn new_uninit_slice(len: usize) -> Box<[MaybeUninit<T>]> {
        let mut v: Vec<MaybeUninit<T>> = Vec::with_capacity(len);
        unsafe { v.set_len(len) };
        v.into_boxed_slice()
    }
    pub fn new_zeroed_slice(len: usize) -> Box<[MaybeUninit<T>]> {
        let mut b = Box::<[T]>::new_uninit_slice(len);
        unsafe { im::write_bytes(b.as_mut_ptr(), 0, len) };
        b
    }
}

impl<T, A: Allocator> Box<[T], A> {
    pub fn new_uninit_slice_in(len: usize, alloc: A) -> Box<[MaybeUninit<T>], A> {
        let mut v: Vec<MaybeUninit<T>, A> = Vec::with_capacity_in(len, alloc);
        unsafe { v.set_len(len) };
        v.into_boxed_slice()
    }
    pub fn new_zeroed_slice_in(len: usize, alloc: A) -> Box<[MaybeUninit<T>], A> {
        let mut b = Box::<[T], A>::new_uninit_slice_in(len, alloc);
        unsafe { im::write_bytes(b.as_mut_ptr(), 0, len) };
        b
    }
}

impl<T, A: Allocator> Box<MaybeUninit<T>, A> {
    pub unsafe fn assume_init(self) -> Box<T, A> {
        let (raw, alloc) = Box::into_raw_with_allocator(self);
        unsafe { Box::from_raw_in(raw as *mut T, alloc) }
    }
    pub fn write(mut boxed: Self, value: T) -> Box<T, A> {
        unsafe {
            (*boxed).write(value);
            boxed.assume_init()
        }
    }
}

impl<T, A: Allocator> Box<[MaybeUninit<T>], A> {
    pub unsafe fn assume_init(self) -> Box<[T], A> {
        let (raw, alloc) = Box::into_raw_with_allocator(self);
        unsafe { Box::from_raw_in(raw as *mut [T], alloc) }
    }
}

/// `Box<[T]>` of exactly N elements as `Box<[T; N]>`, else the box back (alloc/src/boxed/convert.rs).
impl<T, const N: usize, A: Allocator> crate::convert::TryFrom<Box<[T], A>> for Box<[T; N], A> {
    type Error = Box<[T], A>;
    fn try_from(b: Box<[T], A>) -> Result<Box<[T; N], A>, Box<[T], A>> {
        if b.len() == N {
            let (raw, alloc) = Box::into_raw_with_allocator(b);
            Ok(unsafe { Box::from_raw_in(raw as *mut T as *mut [T; N], alloc) })
        } else {
            Err(b)
        }
    }
}

/// `Vec<T>` of exactly N elements as `Box<[T; N]>` (its boxed slice: in place when its capacity is
/// N, moved into one of N otherwise), else the Vec back as it was (alloc/src/boxed.rs).
impl<T, const N: usize> crate::convert::TryFrom<Vec<T>> for Box<[T; N]> {
    type Error = Vec<T>;
    fn try_from(vec: Vec<T>) -> Result<Box<[T; N]>, Vec<T>> {
        if vec.len() == N {
            let b = vec.into_boxed_slice();
            Ok(unsafe { Box::from_raw(Box::into_raw(b) as *mut T as *mut [T; N]) })
        } else {
            Err(vec)
        }
    }
}

// The contents, then the block (library/'s Drop deallocates after the compiler dropped the
// contents), then the allocator itself (the field's own drop, after this).
unsafe impl<#[may_dangle] T: ?Sized, A: Allocator> Drop for Box<T, A> {
    fn drop(&mut self) {
        unsafe {
            let layout = Layout::for_value(&*self.ptr.as_ptr());
            // rustc_mir_transform/src/elaborate_drop.rs: open_drop_for_adt calls
            // Box's deallocator even when dropping its contents unwinds.
            let _free = heap::DeallocGuard { ptr: self.ptr.cast::<u8>(), layout, alloc: &self.alloc };
            im::drop_in_place(self.ptr.as_ptr());
        }
    }
}

/// Frees a Box's allocation whose contents were moved out (partly, the rest already dropped):
/// rustc's box_free after a move out of `*b`; the allocator is dropped after. Called by the
/// compiler only, with the Box's leaves: its pointer's, then its allocator's.
#[doc(hidden)]
pub unsafe fn box_free_after_move<T: ?Sized, A: Allocator>(p: *mut T, alloc: A) {
    // The pointee has been moved out: inspect its metadata, never form a reference to it.
    // A zero-sized layout is no block, as alloc::boxed::Box::drop skips it.
    let layout = Layout::for_value_raw(p);
    if layout.size() != 0 {
        unsafe { alloc.deallocate(NonNull::new_unchecked(p as *mut u8), layout) }
    }
}

impl<T: ?Sized, A: Allocator> crate::ops::LegacyReceiver for Box<T, A> {}
impl<T: ?Sized, A: Allocator> Deref for Box<T, A> {
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { &*self.ptr.as_ptr() }
    }
}

impl<T: ?Sized, A: Allocator> DerefMut for Box<T, A> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.ptr.as_ptr() }
    }
}

impl<T: Default> Default for Box<T> {
    fn default() -> Box<T> {
        Box::new(T::default())
    }
}

impl<T> Default for Box<[T]> {
    fn default() -> Box<[T]> {
        Vec::new().into_boxed_slice()
    }
}

impl Default for Box<str> {
    fn default() -> Box<str> {
        String::new().into_boxed_str()
    }
}

// library/'s: the block first, then the clone written into it (freed if the clone unwinds)
impl<T: Clone, A: Allocator + Clone> Clone for Box<T, A> {
    fn clone(&self) -> Self {
        let boxed = Box::new_uninit_in(self.alloc.clone());
        Box::write(boxed, (**self).clone())
    }
    fn clone_from(&mut self, source: &Self) {
        (**self).clone_from(&(**source));
    }
}

impl<T: Clone, A: Allocator + Clone> Clone for Box<[T], A> {
    fn clone(&self) -> Self {
        crate::slice::to_vec_in(&**self, self.alloc.clone()).into_boxed_slice()
    }
    fn clone_from(&mut self, source: &Self) {
        if self.len() == source.len() {
            self.clone_from_slice(source);
        } else {
            *self = source.clone();
        }
    }
}

impl<A: Allocator + Clone> Clone for Box<str, A> {
    fn clone(&self) -> Self {
        let bytes = crate::slice::to_vec_in(self.as_bytes(), self.alloc.clone()).into_boxed_slice();
        let (raw, alloc) = Box::into_raw_with_allocator(bytes);
        // the bytes of a str
        unsafe { Box::from_raw_in(raw as *mut str, alloc) }
    }
}

impl<A: Allocator> From<Box<str, A>> for Box<[u8], A> {
    fn from(s: Box<str, A>) -> Self {
        let (raw, alloc) = Box::into_raw_with_allocator(s);
        unsafe { Box::from_raw_in(raw as *mut [u8], alloc) }
    }
}

impl<T: ?Sized + PartialEq, A: Allocator> PartialEq for Box<T, A> {
    fn eq(&self, other: &Self) -> bool {
        PartialEq::eq(&**self, &**other)
    }
}
impl<T: ?Sized + Eq, A: Allocator> Eq for Box<T, A> {}
impl<T: ?Sized + PartialOrd, A: Allocator> PartialOrd for Box<T, A> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        PartialOrd::partial_cmp(&**self, &**other)
    }
}
impl<T: ?Sized + Ord, A: Allocator> Ord for Box<T, A> {
    fn cmp(&self, other: &Self) -> Ordering {
        Ord::cmp(&**self, &**other)
    }
}
impl<T: ?Sized + Hash, A: Allocator> Hash for Box<T, A> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        (**self).hash(state)
    }
}
impl<T: ?Sized + fmt::Display, A: Allocator> fmt::Display for Box<T, A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&**self, f)
    }
}
impl<T: ?Sized + fmt::Debug, A: Allocator> fmt::Debug for Box<T, A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}
impl<T: ?Sized, A: Allocator> fmt::Pointer for Box<T, A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Pointer::fmt(&(self.ptr.as_ptr() as *const T), f)
    }
}
impl<T: ?Sized, A: Allocator> Borrow<T> for Box<T, A> {
    fn borrow(&self) -> &T {
        &**self
    }
}
impl<T: ?Sized, A: Allocator> BorrowMut<T> for Box<T, A> {
    fn borrow_mut(&mut self) -> &mut T {
        &mut **self
    }
}
impl<T: ?Sized, A: Allocator> AsRef<T> for Box<T, A> {
    fn as_ref(&self) -> &T {
        &**self
    }
}
impl<T: ?Sized, A: Allocator> AsMut<T> for Box<T, A> {
    fn as_mut(&mut self) -> &mut T {
        &mut **self
    }
}

impl<T> From<T> for Box<T> {
    fn from(t: T) -> Box<T> {
        Box::new(t)
    }
}

// alloc/src/boxed/convert.rs (futures' join_all: `collect::<Box<[_]>>().into()`)
impl<T: ?Sized, A: Allocator> From<Box<T, A>> for Pin<Box<T, A>>
where
    A: StaticAllocator,
{
    fn from(boxed: Box<T, A>) -> Self {
        Box::into_pin(boxed)
    }
}
impl<'a, T: Clone> From<&'a [T]> for Box<[T]> {
    fn from(s: &'a [T]) -> Box<[T]> {
        crate::slice::to_vec(s).into_boxed_slice()
    }
}
impl<'a> From<&'a str> for Box<str> {
    fn from(s: &'a str) -> Box<str> {
        String::from(s).into_boxed_str()
    }
}
impl From<String> for Box<str> {
    fn from(s: String) -> Box<str> {
        s.into_boxed_str()
    }
}
// alloc/src/boxed/convert.rs (Rust 1.45)
impl<'a> From<crate::borrow::Cow<'a, str>> for Box<str> {
    fn from(cow: crate::borrow::Cow<'a, str>) -> Box<str> {
        match cow {
            crate::borrow::Cow::Borrowed(s) => Box::from(s),
            crate::borrow::Cow::Owned(s) => Box::from(s),
        }
    }
}
impl<'a, T: Clone> From<crate::borrow::Cow<'a, [T]>> for Box<[T]> {
    fn from(cow: crate::borrow::Cow<'a, [T]>) -> Box<[T]> {
        match cow {
            crate::borrow::Cow::Borrowed(s) => Box::from(s),
            crate::borrow::Cow::Owned(s) => s.into_boxed_slice(),
        }
    }
}
impl<T, const N: usize> From<[T; N]> for Box<[T]> {
    fn from(a: [T; N]) -> Box<[T]> {
        Vec::from(a).into_boxed_slice()
    }
}
impl<T> FromIterator<T> for Box<[T]> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Box<[T]> {
        let v: Vec<T> = Vec::from_iter(iter);
        v.into_boxed_slice()
    }
}

impl<I: Iterator + ?Sized, A: Allocator> Iterator for Box<I, A> {
    type Item = I::Item;
    fn next(&mut self) -> Option<I::Item> {
        (**self).next()
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        (**self).size_hint()
    }
}
impl<I: DoubleEndedIterator + ?Sized, A: Allocator> DoubleEndedIterator for Box<I, A> {
    fn next_back(&mut self) -> Option<I::Item> {
        (**self).next_back()
    }
}
impl<I: ExactSizeIterator + ?Sized, A: Allocator> ExactSizeIterator for Box<I, A> {}

impl<T, A: Allocator> IntoIterator for Box<[T], A> {
    type Item = T;
    type IntoIter = crate::vec::IntoIter<T, A>;
    fn into_iter(self) -> crate::vec::IntoIter<T, A> {
        self.into_vec().into_iter()
    }
}
// alloc/src/boxed/iter.rs: `for x in &boxed_slice` / `&mut boxed_slice` (Rust 1.80)
impl<'a, T, A: Allocator> IntoIterator for &'a Box<[T], A> {
    type Item = &'a T;
    type IntoIter = crate::slice::Iter<'a, T>;
    fn into_iter(self) -> crate::slice::Iter<'a, T> {
        crate::slice::Iter::new(&**self)
    }
}
impl<'a, T, A: Allocator> IntoIterator for &'a mut Box<[T], A> {
    type Item = &'a mut T;
    type IntoIter = crate::slice::IterMut<'a, T>;
    fn into_iter(self) -> crate::slice::IterMut<'a, T> {
        crate::slice::IterMut::new(&mut **self)
    }
}

// ---------------------------------------------------------------- Box<dyn Error>

impl<'a, E: Error + 'a> From<E> for Box<dyn Error + 'a> {
    fn from(err: E) -> Box<dyn Error + 'a> {
        Box::new(err)
    }
}
impl<'a, E: Error + Send + Sync + 'a> From<E> for Box<dyn Error + Send + Sync + 'a> {
    fn from(err: E) -> Box<dyn Error + Send + Sync + 'a> {
        Box::new(err)
    }
}

/// The error type `Box<dyn Error>::from(String)` wraps (Debug prints like the string).
struct StringError(String);

impl Error for StringError {}
impl fmt::Display for StringError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}
impl fmt::Debug for StringError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.0, f)
    }
}

impl<'a> From<String> for Box<dyn Error + Send + Sync + 'a> {
    fn from(err: String) -> Box<dyn Error + Send + Sync + 'a> {
        Box::new(StringError(err))
    }
}
impl<'a> From<String> for Box<dyn Error + 'a> {
    fn from(err: String) -> Box<dyn Error + 'a> {
        Box::new(StringError(err))
    }
}
impl<'a, 'b> From<&'b str> for Box<dyn Error + Send + Sync + 'a> {
    fn from(err: &'b str) -> Box<dyn Error + Send + Sync + 'a> {
        Box::new(StringError(String::from(err)))
    }
}
impl<'a, 'b> From<&'b str> for Box<dyn Error + 'a> {
    fn from(err: &'b str) -> Box<dyn Error + 'a> {
        Box::new(StringError(String::from(err)))
    }
}

// library/alloc/src/boxed.rs:2420: `E: Error` is Sized, so Box<dyn Error> is not an Error and
// `From<E: Error> for Box<dyn Error>` does not overlap the reflexive `From<T> for T`
impl<E: Error, A: Allocator> Error for Box<E, A> {
    fn provide<'a>(&'a self, request: &mut crate::error::Request<'a>) {
        Error::provide(&**self, request);
    }
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Error::source(&**self)
    }
}

impl Box<dyn Error> {
    pub fn downcast<T: Error + 'static>(self) -> Result<Box<T>, Box<dyn Error>> {
        if self.is::<T>() {
            let raw: *mut dyn Error = Box::into_raw(self);
            Ok(unsafe { Box::from_raw(raw as *mut T) })
        } else {
            Err(self)
        }
    }
}

impl Box<dyn Error + Send> {
    pub fn downcast<T: Error + Send + 'static>(self) -> Result<Box<T>, Box<dyn Error + Send>> {
        if self.type_id_internal() == crate::any::TypeId::of::<T>() {
            let raw: *mut (dyn Error + Send) = Box::into_raw(self);
            Ok(unsafe { Box::from_raw(raw as *mut T) })
        } else {
            Err(self)
        }
    }
}

impl Box<dyn Error + Send + Sync> {
    pub fn downcast<T: Error + Send + Sync + 'static>(self) -> Result<Box<T>, Box<dyn Error + Send + Sync>> {
        if self.is::<T>() {
            let raw: *mut (dyn Error + Send + Sync) = Box::into_raw(self);
            Ok(unsafe { Box::from_raw(raw as *mut T) })
        } else {
            Err(self)
        }
    }
}

// ---------------------------------------------------------------- Box<dyn Any>

impl<A: Allocator> Box<dyn Any, A> {
    pub fn downcast<T: Any>(self) -> Result<Box<T, A>, Self> {
        if self.is::<T>() {
            let (raw, alloc): (*mut dyn Any, A) = Box::into_raw_with_allocator(self);
            Ok(unsafe { Box::from_raw_in(raw as *mut T, alloc) })
        } else {
            Err(self)
        }
    }
}

impl<A: Allocator> Box<dyn Any + Send, A> {
    pub fn downcast<T: Any>(self) -> Result<Box<T, A>, Self> {
        if self.is::<T>() {
            let (raw, alloc): (*mut (dyn Any + Send), A) = Box::into_raw_with_allocator(self);
            Ok(unsafe { Box::from_raw_in(raw as *mut T, alloc) })
        } else {
            Err(self)
        }
    }
}

impl<A: Allocator> Box<dyn Any + Send + Sync, A> {
    pub fn downcast<T: Any>(self) -> Result<Box<T, A>, Self> {
        if self.is::<T>() {
            let (raw, alloc): (*mut (dyn Any + Send + Sync), A) = Box::into_raw_with_allocator(self);
            Ok(unsafe { Box::from_raw_in(raw as *mut T, alloc) })
        } else {
            Err(self)
        }
    }
}

// library/alloc/src/boxed.rs @1.100.0
impl<Args: crate::marker::Tuple, F: ?Sized + crate::ops::FnOnce<Args>, A: Allocator> crate::ops::FnOnce<Args> for Box<F, A> {
    type Output = <F as crate::ops::FnOnce<Args>>::Output;
    extern "rust-call" fn call_once(self, args: Args) -> Self::Output { <F as crate::ops::FnOnce<Args>>::call_once(*self, args) }
}
impl<Args: crate::marker::Tuple, F: ?Sized + crate::ops::FnMut<Args>, A: Allocator> crate::ops::FnMut<Args> for Box<F, A> {
    extern "rust-call" fn call_mut(&mut self, args: Args) -> Self::Output { <F as crate::ops::FnMut<Args>>::call_mut(self, args) }
}
impl<Args: crate::marker::Tuple, F: ?Sized + crate::ops::Fn<Args>, A: Allocator> crate::ops::Fn<Args> for Box<F, A> {
    extern "rust-call" fn call(&self, args: Args) -> Self::Output { <F as crate::ops::Fn<Args>>::call(self, args) }
}

impl<T: ?Sized, A: Allocator> crate::marker::Unpin for Box<T, A> {}

/// rustc 1.100.0 (allocator_api): a boxed allocator is one.
unsafe impl<T: ?Sized + Allocator, A: Allocator> Allocator for Box<T, A> {
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, heap::AllocError> {
        (**self).allocate(layout)
    }
    fn allocate_zeroed(&self, layout: Layout) -> Result<NonNull<[u8]>, heap::AllocError> {
        (**self).allocate_zeroed(layout)
    }
    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) {
        unsafe { (**self).deallocate(ptr, layout) }
    }
    unsafe fn grow(&self, ptr: NonNull<u8>, old_layout: Layout, new_layout: Layout) -> Result<NonNull<[u8]>, heap::AllocError> {
        unsafe { (**self).grow(ptr, old_layout, new_layout) }
    }
    unsafe fn grow_zeroed(&self, ptr: NonNull<u8>, old_layout: Layout, new_layout: Layout) -> Result<NonNull<[u8]>, heap::AllocError> {
        unsafe { (**self).grow_zeroed(ptr, old_layout, new_layout) }
    }
    unsafe fn shrink(&self, ptr: NonNull<u8>, old_layout: Layout, new_layout: Layout) -> Result<NonNull<[u8]>, heap::AllocError> {
        unsafe { (**self).shrink(ptr, old_layout, new_layout) }
    }
}
