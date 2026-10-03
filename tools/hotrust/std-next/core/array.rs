//! Arrays `[T; N]`: trait impls (const generic over N), by-value iteration, from_fn, map.
//! Indexing is in slice/index.rs; Hash in hash/mod.rs.

use crate::borrow::{Borrow, BorrowMut};
use crate::cmp::Ordering;
use crate::fmt;
use crate::intrinsics_mem as im;
use crate::mem::{ManuallyDrop, MaybeUninit};

/// `[f(0), f(1), ..., f(N-1)]`
#[cfg(any(hotrust_check, hotrust_const_generics))]
pub fn from_fn<T, const N: usize, F: FnMut(usize) -> T>(f: F) -> [T; N] {
    let mut f = f;
    let mut out: MaybeUninit<[T; N]> = MaybeUninit::uninit();
    let p = out.as_mut_ptr() as *mut T;
    let mut i = 0;
    while i < N {
        unsafe { im::ptr_write(im::ptr_add_mut(p, i), f(i)) };
        i += 1;
    }
    unsafe { out.assume_init() }
}

pub fn from_ref<T>(s: &T) -> &[T; 1] {
    unsafe { &*(s as *const T as *const [T; 1]) }
}

pub fn from_mut<T>(s: &mut T) -> &mut [T; 1] {
    unsafe { &mut *(s as *mut T as *mut [T; 1]) }
}

/// Error of `<[T; N]>::try_from(&[T])` when the length differs.
#[derive(Debug, Copy, Clone)]
pub struct TryFromSliceError(());

impl fmt::Display for TryFromSliceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt("could not convert slice to array", f)
    }
}

impl crate::error::Error for TryFromSliceError {}

#[cfg(not(hotrust_check))]
#[cfg(any(hotrust_check, hotrust_const_generics))]
impl<T, const N: usize> [T; N] {
    pub fn map<U, F: FnMut(T) -> U>(self, f: F) -> [U; N] {
        let mut f = f;
        let src = ManuallyDrop::new(self);
        let sp = &*src as *const [T; N] as *const T;
        from_fn(|i| f(unsafe { im::ptr_read(im::ptr_add(sp, i)) }))
    }
    pub const fn as_slice(&self) -> &[T] {
        self
    }
    pub fn as_mut_slice(&mut self) -> &mut [T] {
        self
    }
    pub fn each_ref(&self) -> [&T; N] {
        from_fn(|i| &self[i])
    }
}

#[cfg(any(hotrust_check, hotrust_const_generics))]
impl<T: Clone, const N: usize> Clone for [T; N] {
    fn clone(&self) -> [T; N] {
        from_fn(|i| self[i].clone())
    }
}

#[cfg(any(hotrust_check, hotrust_const_generics))]
impl<T: Copy, const N: usize> Copy for [T; N] {}

#[cfg(any(hotrust_check, hotrust_const_generics))]
impl<T: Default, const N: usize> Default for [T; N] {
    fn default() -> [T; N] {
        from_fn(|_| T::default())
    }
}

#[cfg(any(hotrust_check, hotrust_const_generics))]
impl<T: fmt::Debug, const N: usize> fmt::Debug for [T; N] {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self[..], f)
    }
}

#[cfg(any(hotrust_check, hotrust_const_generics))]
impl<T: PartialEq<U>, U, const N: usize> PartialEq<[U; N]> for [T; N] {
    fn eq(&self, other: &[U; N]) -> bool {
        crate::slice::slice_eq(&self[..], &other[..])
    }
}
#[cfg(any(hotrust_check, hotrust_const_generics))]
impl<T: PartialEq<U>, U, const N: usize> PartialEq<[U]> for [T; N] {
    fn eq(&self, other: &[U]) -> bool {
        crate::slice::slice_eq(&self[..], other)
    }
}
#[cfg(any(hotrust_check, hotrust_const_generics))]
impl<T: PartialEq<U>, U, const N: usize> PartialEq<[U; N]> for [T] {
    fn eq(&self, other: &[U; N]) -> bool {
        crate::slice::slice_eq(self, &other[..])
    }
}
#[cfg(any(hotrust_check, hotrust_const_generics))]
impl<'b, T: PartialEq<U>, U, const N: usize> PartialEq<&'b [U]> for [T; N] {
    fn eq(&self, other: &&'b [U]) -> bool {
        crate::slice::slice_eq(&self[..], *other)
    }
}
#[cfg(any(hotrust_check, hotrust_const_generics))]
impl<'b, T: PartialEq<U>, U, const N: usize> PartialEq<[U; N]> for &'b [T] {
    fn eq(&self, other: &[U; N]) -> bool {
        crate::slice::slice_eq(*self, &other[..])
    }
}
#[cfg(any(hotrust_check, hotrust_const_generics))]
impl<T: Eq, const N: usize> Eq for [T; N] {}

#[cfg(any(hotrust_check, hotrust_const_generics))]
impl<T: PartialOrd, const N: usize> PartialOrd for [T; N] {
    fn partial_cmp(&self, other: &[T; N]) -> Option<Ordering> {
        crate::slice::slice_partial_cmp(&self[..], &other[..])
    }
}
#[cfg(any(hotrust_check, hotrust_const_generics))]
impl<T: Ord, const N: usize> Ord for [T; N] {
    fn cmp(&self, other: &[T; N]) -> Ordering {
        crate::slice::slice_cmp(&self[..], &other[..])
    }
}

#[cfg(any(hotrust_check, hotrust_const_generics))]
impl<T, const N: usize> AsRef<[T]> for [T; N] {
    fn as_ref(&self) -> &[T] {
        &self[..]
    }
}
#[cfg(any(hotrust_check, hotrust_const_generics))]
impl<T, const N: usize> AsMut<[T]> for [T; N] {
    fn as_mut(&mut self) -> &mut [T] {
        &mut self[..]
    }
}
#[cfg(any(hotrust_check, hotrust_const_generics))]
impl<T, const N: usize> Borrow<[T]> for [T; N] {
    fn borrow(&self) -> &[T] {
        self
    }
}
#[cfg(any(hotrust_check, hotrust_const_generics))]
impl<T, const N: usize> BorrowMut<[T]> for [T; N] {
    fn borrow_mut(&mut self) -> &mut [T] {
        self
    }
}

#[cfg(any(hotrust_check, hotrust_const_generics))]
impl<'a, T: Copy, const N: usize> TryFrom<&'a [T]> for [T; N] {
    type Error = TryFromSliceError;
    fn try_from(slice: &'a [T]) -> Result<[T; N], TryFromSliceError> {
        if slice.len() == N {
            Ok(unsafe { im::ptr_read(slice.as_ptr() as *const [T; N]) })
        } else {
            Err(TryFromSliceError(()))
        }
    }
}
#[cfg(any(hotrust_check, hotrust_const_generics))]
impl<'a, T: Copy, const N: usize> TryFrom<&'a mut [T]> for [T; N] {
    type Error = TryFromSliceError;
    fn try_from(slice: &'a mut [T]) -> Result<[T; N], TryFromSliceError> {
        if slice.len() == N {
            Ok(unsafe { im::ptr_read(slice.as_ptr() as *const [T; N]) })
        } else {
            Err(TryFromSliceError(()))
        }
    }
}
#[cfg(any(hotrust_check, hotrust_const_generics))]
impl<'a, T, const N: usize> TryFrom<&'a [T]> for &'a [T; N] {
    type Error = TryFromSliceError;
    fn try_from(slice: &'a [T]) -> Result<&'a [T; N], TryFromSliceError> {
        if slice.len() == N {
            Ok(unsafe { &*(slice.as_ptr() as *const [T; N]) })
        } else {
            Err(TryFromSliceError(()))
        }
    }
}
#[cfg(any(hotrust_check, hotrust_const_generics))]
impl<'a, T, const N: usize> TryFrom<&'a mut [T]> for &'a mut [T; N] {
    type Error = TryFromSliceError;
    fn try_from(slice: &'a mut [T]) -> Result<&'a mut [T; N], TryFromSliceError> {
        if slice.len() == N {
            Ok(unsafe { &mut *(slice.as_mut_ptr() as *mut [T; N]) })
        } else {
            Err(TryFromSliceError(()))
        }
    }
}

#[cfg(any(hotrust_check, hotrust_const_generics))]
impl<'a, T, const N: usize> IntoIterator for &'a [T; N] {
    type Item = &'a T;
    type IntoIter = crate::slice::Iter<'a, T>;
    fn into_iter(self) -> crate::slice::Iter<'a, T> {
        crate::slice::Iter::new(&self[..])
    }
}

#[cfg(any(hotrust_check, hotrust_const_generics))]
impl<'a, T, const N: usize> IntoIterator for &'a mut [T; N] {
    type Item = &'a mut T;
    type IntoIter = crate::slice::IterMut<'a, T>;
    fn into_iter(self) -> crate::slice::IterMut<'a, T> {
        crate::slice::IterMut::new(&mut self[..])
    }
}

#[cfg(any(hotrust_check, hotrust_const_generics))]
impl<T, const N: usize> IntoIterator for [T; N] {
    type Item = T;
    type IntoIter = IntoIter<T, N>;
    fn into_iter(self) -> IntoIter<T, N> {
        IntoIter { data: ManuallyDrop::new(self), start: 0, end: N }
    }
}

/// By-value array iterator.
#[cfg(any(hotrust_check, hotrust_const_generics))]
pub struct IntoIter<T, const N: usize> {
    data: ManuallyDrop<[T; N]>,
    start: usize,
    end: usize,
}

#[cfg(any(hotrust_check, hotrust_const_generics))]
impl<T, const N: usize> IntoIter<T, N> {
    fn base(&self) -> *const T {
        &*self.data as *const [T; N] as *const T
    }
    pub fn as_slice(&self) -> &[T] {
        unsafe { im::slice_from_raw(im::ptr_add(self.base(), self.start), self.end - self.start) }
    }
}

#[cfg(any(hotrust_check, hotrust_const_generics))]
impl<T, const N: usize> Iterator for IntoIter<T, N> {
    type Item = T;
    fn next(&mut self) -> Option<T> {
        if self.start == self.end {
            None
        } else {
            let v = unsafe { im::ptr_read(im::ptr_add(self.base(), self.start)) };
            self.start += 1;
            Some(v)
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = self.end - self.start;
        (n, Some(n))
    }
}

#[cfg(any(hotrust_check, hotrust_const_generics))]
impl<T, const N: usize> DoubleEndedIterator for IntoIter<T, N> {
    fn next_back(&mut self) -> Option<T> {
        if self.start == self.end {
            None
        } else {
            self.end -= 1;
            Some(unsafe { im::ptr_read(im::ptr_add(self.base(), self.end)) })
        }
    }
}

#[cfg(any(hotrust_check, hotrust_const_generics))]
impl<T, const N: usize> ExactSizeIterator for IntoIter<T, N> {}
#[cfg(any(hotrust_check, hotrust_const_generics))]
impl<T, const N: usize> crate::iter::FusedIterator for IntoIter<T, N> {}

#[cfg(any(hotrust_check, hotrust_const_generics))]
impl<T: Clone, const N: usize> Clone for IntoIter<T, N> {
    fn clone(&self) -> IntoIter<T, N> {
        // clones the remaining elements into a fresh array position-for-position
        let base = self.base();
        let start = self.start;
        let end = self.end;
        let data: [T; N] = from_fn(|i| {
            let j = if i >= start && i < end { i } else { start };
            unsafe { (*im::ptr_add(base, j)).clone() }
        });
        // the slots outside start..end hold extra clones; drop them now
        let data = ManuallyDrop::new(data);
        let p = &*data as *const [T; N] as *mut T;
        let mut i = 0;
        while i < N {
            if i < start || i >= end {
                unsafe { im::drop_in_place(im::ptr_add_mut(p, i)) };
            }
            i += 1;
        }
        IntoIter { data, start, end }
    }
}

#[cfg(any(hotrust_check, hotrust_const_generics))]
impl<T: fmt::Debug, const N: usize> fmt::Debug for IntoIter<T, N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("IntoIter").field(&self.as_slice()).finish()
    }
}

#[cfg(any(hotrust_check, hotrust_const_generics))]
impl<T, const N: usize> Drop for IntoIter<T, N> {
    fn drop(&mut self) {
        let p = &mut *self.data as *mut [T; N] as *mut T;
        while self.start < self.end {
            unsafe { im::drop_in_place(im::ptr_add_mut(p, self.start)) };
            self.start += 1;
        }
    }
}
