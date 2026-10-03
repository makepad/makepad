//! Vec's owning iterator.

use super::Vec;
use crate::fmt;
use crate::heap;
use crate::intrinsics_mem as im;

pub struct IntoIter<T> {
    buf: *mut T,
    cap: usize,
    /// next front index
    start: usize,
    /// one past the next back index
    end: usize,
}

unsafe impl<T: Send> Send for IntoIter<T> {}
unsafe impl<T: Sync> Sync for IntoIter<T> {}

pub(super) fn new<T>(v: Vec<T>) -> IntoIter<T> {
    let me = crate::mem::ManuallyDrop::new(v);
    IntoIter { buf: me.ptr, cap: me.cap, start: 0, end: me.len }
}

impl<T> IntoIter<T> {
    pub fn as_slice(&self) -> &[T] {
        unsafe { im::slice_from_raw(im::ptr_add(self.buf as *const T, self.start), self.end - self.start) }
    }
    pub fn as_mut_slice(&mut self) -> &mut [T] {
        unsafe { im::slice_from_raw_mut(im::ptr_add_mut(self.buf, self.start), self.end - self.start) }
    }
}

impl<T> Iterator for IntoIter<T> {
    type Item = T;
    fn next(&mut self) -> Option<T> {
        if self.start == self.end {
            None
        } else {
            let v = unsafe { im::ptr_read(im::ptr_add(self.buf as *const T, self.start)) };
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
    fn nth(&mut self, n: usize) -> Option<T> {
        let avail = self.end - self.start;
        let skip = if n < avail { n } else { avail };
        let mut i = 0;
        while i < skip {
            unsafe { im::drop_in_place(im::ptr_add_mut(self.buf, self.start)) };
            self.start += 1;
            i += 1;
        }
        self.next()
    }
}

impl<T> DoubleEndedIterator for IntoIter<T> {
    fn next_back(&mut self) -> Option<T> {
        if self.start == self.end {
            None
        } else {
            self.end -= 1;
            Some(unsafe { im::ptr_read(im::ptr_add(self.buf as *const T, self.end)) })
        }
    }
}

impl<T> ExactSizeIterator for IntoIter<T> {}
impl<T> crate::iter::FusedIterator for IntoIter<T> {}

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

impl<T> Drop for IntoIter<T> {
    fn drop(&mut self) {
        while self.start < self.end {
            unsafe { im::drop_in_place(im::ptr_add_mut(self.buf, self.start)) };
            self.start += 1;
        }
        if crate::mem::size_of::<T>() != 0 && self.cap != 0 {
            heap::free(self.buf as *mut u8, Vec::<T>::layout_for(self.cap));
        }
    }
}
