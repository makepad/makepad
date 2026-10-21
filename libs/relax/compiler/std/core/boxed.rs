//! Box<T>: an owned heap allocation (Relax knows Box: `*b` moves out, Box<T> -> Box<dyn Tr>
//! and Box<[T; N]> -> Box<[T]> unsize, and calls through Box<dyn Fn*> are built in).

use crate::any::Any;
use crate::borrow::{Borrow, BorrowMut};
use crate::cmp::Ordering;
use crate::error::Error;
use crate::fmt;
use crate::hash::{Hash, Hasher};
use crate::heap::{self, Layout};
use crate::intrinsics_mem as im;
use crate::ops::{Deref, DerefMut, DispatchFromDyn};
use crate::marker::Unsize;
use crate::pin::PinSafePointer;

// library/alloc/src/boxed.rs:2384,2390 @8d1a764
unsafe impl<T: ?Sized> PinSafePointer for Box<T> {}
impl<T: ?Sized + Unsize<U>, U: ?Sized> DispatchFromDyn<Box<U>> for Box<T> {}
impl<T: ?Sized + Unsize<U>, U: ?Sized> crate::ops::CoerceUnsized<Box<U>> for Box<T> {}
use crate::string::String;
use crate::vec::Vec;

#[fundamental]
#[rustc_diagnostic_item = "Box"]
#[cfg_attr(not(relax_check), lang = "owned_box")]
#[rustc_insignificant_dtor]
pub struct Box<T: ?Sized> {
    ptr: crate::ptr::NonNull<T>,
    _marker: crate::marker::PhantomData<T>,
}

unsafe impl<T: ?Sized + Send> Send for Box<T> {}
unsafe impl<T: ?Sized + Sync> Sync for Box<T> {}

impl<T> Box<T> {
    #[rustc_diagnostic_item = "box_new"]
    pub fn new(x: T) -> Box<T> {
        let p = heap::alloc_or_abort(Layout::new::<T>()) as *mut T;
        unsafe { im::ptr_write(p, x) };
        Box { ptr: unsafe { crate::ptr::NonNull::new_unchecked(p) }, _marker: crate::marker::PhantomData }
    }
    pub fn pin(x: T) -> crate::pin::Pin<Box<T>> {
        unsafe { crate::pin::Pin::new_unchecked(Box::new(x)) }
    }
    /// Moves the value out and frees the allocation (`*b` for a Box<T> does the same).
    pub fn into_inner(b: Box<T>) -> T {
        let p = Box::into_raw(b);
        let v = unsafe { im::ptr_read(p as *const T) };
        heap::free(p as *mut u8, Layout::new::<T>());
        v
    }
    pub fn new_uninit() -> Box<crate::mem::MaybeUninit<T>> {
        let p = heap::alloc_or_abort(Layout::new::<T>()) as *mut crate::mem::MaybeUninit<T>;
        Box { ptr: unsafe { crate::ptr::NonNull::new_unchecked(p) }, _marker: crate::marker::PhantomData }
    }
    pub fn new_zeroed() -> Box<crate::mem::MaybeUninit<T>> {
        let b = Box::<T>::new_uninit();
        unsafe { im::write_bytes(b.ptr.as_ptr() as *mut u8, 0, crate::mem::size_of::<T>()) };
        b
    }
}

impl<T: ?Sized> Box<T> {
    #[must_use = "call `drop(Box::from_raw(ptr))` if you intend to drop the `Box`"]
    pub unsafe fn from_raw(raw: *mut T) -> Box<T> {
        Box { ptr: unsafe { crate::ptr::NonNull::new_unchecked(raw) }, _marker: crate::marker::PhantomData }
    }
    pub fn into_raw(b: Box<T>) -> *mut T {
        let p = b.ptr.as_ptr();
        crate::mem::forget(b);
        p
    }
    pub fn leak<'a>(b: Box<T>) -> &'a mut T {
        unsafe { &mut *Box::into_raw(b) }
    }
    #[rustc_never_returns_null_ptr]
    pub fn as_ptr(b: &Box<T>) -> *const T {
        b.ptr.as_ptr() as *const T
    }
    #[rustc_never_returns_null_ptr]
    pub fn as_mut_ptr(b: &mut Box<T>) -> *mut T {
        b.ptr.as_ptr()
    }
    pub fn into_pin(boxed: Box<T>) -> crate::pin::Pin<Box<T>> {
        unsafe { crate::pin::Pin::new_unchecked(boxed) }
    }
}

impl<T> Box<[T]> {
    pub fn new_uninit_slice(len: usize) -> Box<[crate::mem::MaybeUninit<T>]> {
        let mut v: Vec<crate::mem::MaybeUninit<T>> = Vec::with_capacity(len);
        unsafe { v.set_len(len) };
        v.into_boxed_slice()
    }
    pub fn new_zeroed_slice(len: usize) -> Box<[crate::mem::MaybeUninit<T>]> {
        let mut b = Box::<[T]>::new_uninit_slice(len);
        unsafe { im::write_bytes(b.as_mut_ptr(), 0, len) };
        b
    }
}

impl<T> Box<crate::mem::MaybeUninit<T>> {
    pub unsafe fn assume_init(self) -> Box<T> {
        Box::from_raw(Box::into_raw(self) as *mut T)
    }
    pub fn write(mut boxed: Box<crate::mem::MaybeUninit<T>>, value: T) -> Box<T> {
        unsafe {
            (*boxed).write(value);
            boxed.assume_init()
        }
    }
}

impl<T> Box<[crate::mem::MaybeUninit<T>]> {
    pub unsafe fn assume_init(self) -> Box<[T]> {
        Box::from_raw(Box::into_raw(self) as *mut [T])
    }
}

/// `Box<[T]>` of exactly N elements as `Box<[T; N]>`, else the box back (alloc/src/boxed.rs).
impl<T, const N: usize> crate::convert::TryFrom<Box<[T]>> for Box<[T; N]> {
    type Error = Box<[T]>;
    fn try_from(b: Box<[T]>) -> Result<Box<[T; N]>, Box<[T]>> {
        if b.len() == N {
            Ok(unsafe { Box::from_raw(Box::into_raw(b) as *mut T as *mut [T; N]) })
        } else {
            Err(b)
        }
    }
}

unsafe impl<#[may_dangle] T: ?Sized> Drop for Box<T> {
    fn drop(&mut self) {
        unsafe {
            let layout = Layout::for_value(&*self.ptr.as_ptr());
            im::drop_in_place(self.ptr.as_ptr());
            heap::free(self.ptr.as_ptr() as *mut u8, layout);
        }
    }
}

/// Frees a Box's allocation whose contents were moved out (partly, the rest already dropped):
/// rustc's box_free after a move out of `*b`. Called by the compiler only.
#[doc(hidden)]
pub unsafe fn box_free_after_move<T>(p: *mut T) {
    heap::free(p as *mut u8, Layout::new::<T>());
}

impl<T: ?Sized> crate::ops::LegacyReceiver for Box<T> {}
impl<T: ?Sized> Deref for Box<T> {
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { &*self.ptr.as_ptr() }
    }
}

impl<T: ?Sized> DerefMut for Box<T> {
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

impl<T: Clone> Clone for Box<T> {
    fn clone(&self) -> Box<T> {
        Box::new((**self).clone())
    }
}

impl<T: Clone> Clone for Box<[T]> {
    fn clone(&self) -> Box<[T]> {
        crate::slice::to_vec(&**self).into_boxed_slice()
    }
}

impl Clone for Box<str> {
    fn clone(&self) -> Box<str> {
        String::from(&**self).into_boxed_str()
    }
}

impl<T: ?Sized + PartialEq> PartialEq for Box<T> {
    fn eq(&self, other: &Box<T>) -> bool {
        PartialEq::eq(&**self, &**other)
    }
}
impl<T: ?Sized + Eq> Eq for Box<T> {}
impl<T: ?Sized + PartialOrd> PartialOrd for Box<T> {
    fn partial_cmp(&self, other: &Box<T>) -> Option<Ordering> {
        PartialOrd::partial_cmp(&**self, &**other)
    }
}
impl<T: ?Sized + Ord> Ord for Box<T> {
    fn cmp(&self, other: &Box<T>) -> Ordering {
        Ord::cmp(&**self, &**other)
    }
}
impl<T: ?Sized + Hash> Hash for Box<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        (**self).hash(state)
    }
}
impl<T: ?Sized + fmt::Display> fmt::Display for Box<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&**self, f)
    }
}
impl<T: ?Sized + fmt::Debug> fmt::Debug for Box<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&**self, f)
    }
}
impl<T: ?Sized> fmt::Pointer for Box<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Pointer::fmt(&(self.ptr.as_ptr() as *const T), f)
    }
}
impl<T: ?Sized> Borrow<T> for Box<T> {
    fn borrow(&self) -> &T {
        &**self
    }
}
impl<T: ?Sized> BorrowMut<T> for Box<T> {
    fn borrow_mut(&mut self) -> &mut T {
        &mut **self
    }
}
impl<T: ?Sized> AsRef<T> for Box<T> {
    fn as_ref(&self) -> &T {
        &**self
    }
}
impl<T: ?Sized> AsMut<T> for Box<T> {
    fn as_mut(&mut self) -> &mut T {
        &mut **self
    }
}

impl<T> From<T> for Box<T> {
    fn from(t: T) -> Box<T> {
        Box::new(t)
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

impl<I: Iterator + ?Sized> Iterator for Box<I> {
    type Item = I::Item;
    fn next(&mut self) -> Option<I::Item> {
        (**self).next()
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        (**self).size_hint()
    }
}
impl<I: DoubleEndedIterator + ?Sized> DoubleEndedIterator for Box<I> {
    fn next_back(&mut self) -> Option<I::Item> {
        (**self).next_back()
    }
}
impl<I: ExactSizeIterator + ?Sized> ExactSizeIterator for Box<I> {}

impl<T> IntoIterator for Box<[T]> {
    type Item = T;
    type IntoIter = crate::vec::IntoIter<T>;
    fn into_iter(self) -> crate::vec::IntoIter<T> {
        self.into_vec().into_iter()
    }
}
// alloc/src/boxed/iter.rs: `for x in &boxed_slice` / `&mut boxed_slice` (Rust 1.80)
impl<'a, T> IntoIterator for &'a Box<[T]> {
    type Item = &'a T;
    type IntoIter = crate::slice::Iter<'a, T>;
    fn into_iter(self) -> crate::slice::Iter<'a, T> {
        crate::slice::Iter::new(&**self)
    }
}
impl<'a, T> IntoIterator for &'a mut Box<[T]> {
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
impl<E: Error> Error for Box<E> {
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

// ---------------------------------------------------------------- Box<dyn Any>

impl Box<dyn Any> {
    pub fn downcast<T: Any>(self) -> Result<Box<T>, Box<dyn Any>> {
        if self.is::<T>() {
            let raw: *mut dyn Any = Box::into_raw(self);
            Ok(unsafe { Box::from_raw(raw as *mut T) })
        } else {
            Err(self)
        }
    }
}

impl Box<dyn Any + Send> {
    pub fn downcast<T: Any>(self) -> Result<Box<T>, Box<dyn Any + Send>> {
        if self.is::<T>() {
            let raw: *mut (dyn Any + Send) = Box::into_raw(self);
            Ok(unsafe { Box::from_raw(raw as *mut T) })
        } else {
            Err(self)
        }
    }
}

// library/alloc/src/boxed.rs:2271-2294 (Relax's Box has no allocator parameter).
impl<A: crate::marker::Tuple, F: ?Sized + crate::ops::FnOnce<A>> crate::ops::FnOnce<A> for Box<F> {
    type Output = <F as crate::ops::FnOnce<A>>::Output;
    extern "rust-call" fn call_once(self, args: A) -> Self::Output { <F as crate::ops::FnOnce<A>>::call_once(*self, args) }
}
impl<A: crate::marker::Tuple, F: ?Sized + crate::ops::FnMut<A>> crate::ops::FnMut<A> for Box<F> {
    extern "rust-call" fn call_mut(&mut self, args: A) -> Self::Output { <F as crate::ops::FnMut<A>>::call_mut(self, args) }
}
impl<A: crate::marker::Tuple, F: ?Sized + crate::ops::Fn<A>> crate::ops::Fn<A> for Box<F> {
    extern "rust-call" fn call(&self, args: A) -> Self::Output { <F as crate::ops::Fn<A>>::call(self, args) }
}

// library/alloc/src/boxed.rs:2385.
impl<T: ?Sized> crate::marker::Unpin for Box<T> {}
