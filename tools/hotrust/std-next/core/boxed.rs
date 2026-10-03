//! Box<T>: an owned heap allocation (HotRust knows Box: `*b` moves out, Box<T> -> Box<dyn Tr>
//! and Box<[T; N]> -> Box<[T]> unsize, and calls through Box<dyn Fn*> are built in).

use crate::any::Any;
use crate::borrow::{Borrow, BorrowMut};
use crate::cmp::Ordering;
use crate::error::Error;
use crate::fmt;
use crate::hash::{Hash, Hasher};
use crate::heap::{self, Layout};
use crate::intrinsics_mem as im;
use crate::ops::{Deref, DerefMut};
use crate::string::String;
use crate::vec::Vec;

pub struct Box<T: ?Sized> {
    ptr: *mut T,
}

unsafe impl<T: ?Sized + Send> Send for Box<T> {}
unsafe impl<T: ?Sized + Sync> Sync for Box<T> {}

impl<T> Box<T> {
    pub fn new(x: T) -> Box<T> {
        let p = heap::alloc_or_abort(Layout::new::<T>()) as *mut T;
        unsafe { im::ptr_write(p, x) };
        Box { ptr: p }
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
        Box { ptr: p }
    }
}

impl<T: ?Sized> Box<T> {
    pub unsafe fn from_raw(raw: *mut T) -> Box<T> {
        Box { ptr: raw }
    }
    pub fn into_raw(b: Box<T>) -> *mut T {
        let p = b.ptr;
        crate::mem::forget(b);
        p
    }
    pub fn leak<'a>(b: Box<T>) -> &'a mut T {
        unsafe { &mut *Box::into_raw(b) }
    }
    pub fn as_ptr(b: &Box<T>) -> *const T {
        b.ptr as *const T
    }
    pub fn as_mut_ptr(b: &mut Box<T>) -> *mut T {
        b.ptr
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
}

impl<T: ?Sized> Drop for Box<T> {
    fn drop(&mut self) {
        unsafe {
            let layout = Layout::for_value(&*self.ptr);
            im::drop_in_place(self.ptr);
            heap::free(self.ptr as *mut u8, layout);
        }
    }
}

impl<T: ?Sized> Deref for Box<T> {
    type Target = T;
    fn deref(&self) -> &T {
        unsafe { &*self.ptr }
    }
}

impl<T: ?Sized> DerefMut for Box<T> {
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.ptr }
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
        fmt::Pointer::fmt(&(self.ptr as *const T), f)
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
#[cfg(any(hotrust_check, hotrust_const_generics))]
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

impl<T: Error + ?Sized> Error for Box<T> {
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
