//! Vec's owning iterator.

use super::Vec;
use crate::fmt;
use crate::heap;
use crate::intrinsics_mem as im;

#[rustc_insignificant_dtor]
pub struct IntoIter<T> {
    buf: crate::ptr::NonNull<T>,
    cap: usize,
    /// next front index
    start: usize,
    /// one past the next back index
    end: usize,
    _marker: crate::marker::PhantomData<T>,
}

unsafe impl<T: Send> Send for IntoIter<T> {}
unsafe impl<T: Sync> Sync for IntoIter<T> {}

pub(super) fn new<T>(v: Vec<T>) -> IntoIter<T> {
    let me = crate::mem::ManuallyDrop::new(v);
    IntoIter { buf: me.ptr, cap: me.cap.get(), start: 0, end: me.len, _marker: crate::marker::PhantomData }
}

impl<T> IntoIter<T> {
    pub fn as_slice(&self) -> &[T] {
        unsafe { im::slice_from_raw(im::ptr_add(self.buf.as_ptr() as *const T, self.start), self.end - self.start) }
    }
    pub fn as_mut_slice(&mut self) -> &mut [T] {
        unsafe { im::slice_from_raw_mut(im::ptr_add_mut(self.buf.as_ptr(), self.start), self.end - self.start) }
    }
}

impl<T> Iterator for IntoIter<T> {
    type Item = T;
    fn next(&mut self) -> Option<T> {
        if self.start == self.end {
            None
        } else {
            let v = unsafe { im::ptr_read(im::ptr_add(self.buf.as_ptr() as *const T, self.start)) };
            self.start += 1;
            Some(v)
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = self.end - self.start;
        (n, Some(n))
    }
    fn count(self) -> usize {
        self.end - self.start
    }
    // alloc vec/into_iter.rs: the skipped items leave the live range first, then drop as one
    // slice (a panicking drop leaves the others to the slice's drop); nth is the default's: this,
    // then a next
    fn advance_by(&mut self, n: usize) -> Result<(), usize> {
        let avail = self.end - self.start;
        let k = if n < avail { n } else { avail };
        let from = self.start;
        self.start += k;
        if crate::mem::needs_drop::<T>() {
            // SAFETY: from..from + k were alive and are out of the live range now: dropped once
            unsafe { crate::ptr::drop_in_place(crate::ptr::slice_from_raw_parts_mut(im::ptr_add_mut(self.buf.as_ptr(), from), k)) };
        }
        if k < n { Err(n - k) } else { Ok(()) }
    }
    #[doc(hidden)]
    #[inline]
    unsafe fn __iterator_get_unchecked(&mut self, i: usize) -> T {
        // SAFETY: i is below the items left where Zip took the iterator, so start + i < end; T
        // is Copy (the TrustedRandomAccessNoCoerce impl), so reading an item does not take it
        // from the buffer's drop
        unsafe { im::ptr_read(im::ptr_add(self.buf.as_ptr() as *const T, self.start + i)) }
    }
}

impl<T> DoubleEndedIterator for IntoIter<T> {
    fn next_back(&mut self) -> Option<T> {
        if self.start == self.end {
            None
        } else {
            self.end -= 1;
            Some(unsafe { im::ptr_read(im::ptr_add(self.buf.as_ptr() as *const T, self.end)) })
        }
    }
}

impl<T> ExactSizeIterator for IntoIter<T> {}
unsafe impl<T> crate::iter::TrustedLen for IntoIter<T> {}
impl<T> crate::iter::FusedIterator for IntoIter<T> {}
// real alloc: `T: NonDrop`, its marker for Copy (the indexed reads leave the items in the buffer, and
// its drop drops start..end)
unsafe impl<T: Copy> crate::iter::TrustedRandomAccessNoCoerce for IntoIter<T> {
    const MAY_HAVE_SIDE_EFFECT: bool = false;
}

impl<T: Clone> Clone for IntoIter<T> {
    fn clone(&self) -> IntoIter<T> {
        new(crate::slice::to_vec(self.as_slice()))
    }
}

impl<T: fmt::Debug> fmt::Debug for IntoIter<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("IntoIter").field(&self.as_slice()).finish()
    }
}

impl<T> Default for IntoIter<T> {
    fn default() -> IntoIter<T> {
        new(Vec::new())
    }
}

// alloc vec/into_iter.rs: the items left drop as one slice (a panicking drop leaves the others to
// the slice's drop), and a guard frees the buffer, on an unwind too
unsafe impl<#[may_dangle] T> Drop for IntoIter<T> {
    fn drop(&mut self) {
        struct DropGuard<'a, T>(&'a mut IntoIter<T>);
        impl<T> Drop for DropGuard<'_, T> {
            fn drop(&mut self) {
                if crate::mem::size_of::<T>() != 0 && self.0.cap != 0 {
                    heap::free(self.0.buf.as_ptr() as *mut u8, Vec::<T>::current_layout(self.0.cap));
                }
            }
        }
        let guard = DropGuard(self);
        // SAFETY: start..end are the items still owned; nothing reads them after this
        unsafe { crate::ptr::drop_in_place(crate::ptr::slice_from_raw_parts_mut(im::ptr_add_mut(guard.0.buf.as_ptr(), guard.0.start), guard.0.end - guard.0.start)) };
    }
}
