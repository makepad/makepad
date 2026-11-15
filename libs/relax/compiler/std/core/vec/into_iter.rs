//! Vec's owning iterator.

use super::Vec;
use crate::fmt;
use crate::heap::{Allocator, Global};
use crate::intrinsics_mem as im;

#[rustc_insignificant_dtor]
pub struct IntoIter<T, #[unstable(feature = "allocator_ext", issue = "163177", implied_by = "allocator_api")] A: Allocator = Global> {
    buf: crate::ptr::NonNull<T>,
    cap: usize,
    /// next front index
    start: usize,
    /// one past the next back index
    end: usize,
    _marker: crate::marker::PhantomData<T>,
    alloc: A,
}

unsafe impl<T: Send, A: Allocator + Send> Send for IntoIter<T, A> {}
unsafe impl<T: Sync, A: Allocator + Sync> Sync for IntoIter<T, A> {}

// inlined into `into_iter` as before the allocator parameter (moving the allocator out took the
// source past O1's size rule): the iterator's fields are registers from the loop's first round
#[inline]
pub(super) fn new<T, A: Allocator>(v: Vec<T, A>) -> IntoIter<T, A> {
    let me = crate::mem::ManuallyDrop::new(v);
    let alloc = unsafe { im::ptr_read(&me.alloc as *const A) };
    IntoIter { buf: me.ptr, cap: me.cap.get(), start: 0, end: me.len, _marker: crate::marker::PhantomData, alloc }
}

impl<T, A: Allocator> IntoIter<T, A> {
    pub fn allocator(&self) -> &A {
        &self.alloc
    }
    pub fn as_slice(&self) -> &[T] {
        unsafe { im::slice_from_raw(im::ptr_add(self.buf.as_ptr() as *const T, self.start), self.end - self.start) }
    }
    pub fn as_mut_slice(&mut self) -> &mut [T] {
        unsafe { im::slice_from_raw_mut(im::ptr_add_mut(self.buf.as_ptr(), self.start), self.end - self.start) }
    }
}

impl<T, A: Allocator> Iterator for IntoIter<T, A> {
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

impl<T, A: Allocator> DoubleEndedIterator for IntoIter<T, A> {
    fn next_back(&mut self) -> Option<T> {
        if self.start == self.end {
            None
        } else {
            self.end -= 1;
            Some(unsafe { im::ptr_read(im::ptr_add(self.buf.as_ptr() as *const T, self.end)) })
        }
    }
}

impl<T, A: Allocator> ExactSizeIterator for IntoIter<T, A> {}
unsafe impl<T, A: Allocator> crate::iter::TrustedLen for IntoIter<T, A> {}
impl<T, A: Allocator> crate::iter::FusedIterator for IntoIter<T, A> {}
// real alloc: `T: NonDrop`, its marker for Copy (the indexed reads leave the items in the buffer, and
// its drop drops start..end)
unsafe impl<T: Copy, A: Allocator> crate::iter::TrustedRandomAccessNoCoerce for IntoIter<T, A> {
    const MAY_HAVE_SIDE_EFFECT: bool = false;
}

impl<T: Clone, A: Allocator + Clone> Clone for IntoIter<T, A> {
    fn clone(&self) -> IntoIter<T, A> {
        new(crate::slice::to_vec_in(self.as_slice(), self.alloc.clone()))
    }
}

impl<T: fmt::Debug, A: Allocator> fmt::Debug for IntoIter<T, A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("IntoIter").field(&self.as_slice()).finish()
    }
}

impl<T, A: Allocator + Default> Default for IntoIter<T, A> {
    fn default() -> IntoIter<T, A> {
        new(Vec::new_in(A::default()))
    }
}

// alloc vec/into_iter.rs: the items left drop as one slice (a panicking drop leaves the others to
// the slice's drop), and a guard frees the buffer, on an unwind too; the allocator drops last
unsafe impl<#[may_dangle] T, A: Allocator> Drop for IntoIter<T, A> {
    fn drop(&mut self) {
        struct DropGuard<'a, T, A: Allocator>(&'a mut IntoIter<T, A>);
        impl<T, A: Allocator> Drop for DropGuard<'_, T, A> {
            fn drop(&mut self) {
                if crate::mem::size_of::<T>() != 0 && self.0.cap != 0 {
                    unsafe { self.0.alloc.deallocate(self.0.buf.cast::<u8>(), Vec::<T>::current_layout(self.0.cap)) };
                }
            }
        }
        let guard = DropGuard(self);
        // SAFETY: start..end are the items still owned; nothing reads them after this
        unsafe { crate::ptr::drop_in_place(crate::ptr::slice_from_raw_parts_mut(im::ptr_add_mut(guard.0.buf.as_ptr(), guard.0.start), guard.0.end - guard.0.start)) };
    }
}
