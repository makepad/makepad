//! Vec::drain.

use super::Vec;
use crate::fmt;
use crate::intrinsics_mem as im;

/// Yields the elements of `vec[start..end]`; on drop the remaining ones are dropped and the
/// tail is moved down. While it lives the vec's len is `start` (leak-safe, as core).
pub struct Drain<'a, T> {
    vec: &'a mut Vec<T>,
    /// next front index
    idx: usize,
    /// one past the next back index
    end: usize,
    tail_start: usize,
    tail_len: usize,
}

pub(super) fn new<T>(vec: &mut Vec<T>, start: usize, end: usize) -> Drain<'_, T> {
    let len = vec.len;
    vec.len = start;
    Drain { vec, idx: start, end, tail_start: end, tail_len: len - end }
}

impl<'a, T> Drain<'a, T> {
    pub fn as_slice(&self) -> &[T] {
        unsafe { im::slice_from_raw(im::ptr_add(self.vec.ptr as *const T, self.idx), self.end - self.idx) }
    }
    pub fn keep_rest(self) {
        let mut me = crate::mem::ManuallyDrop::new(self);
        unsafe {
            let start = me.vec.len;
            let unyielded = me.end - me.idx;
            if me.idx != start {
                im::copy(im::ptr_add(me.vec.ptr as *const T, me.idx), im::ptr_add_mut(me.vec.ptr, start), unyielded);
            }
            let new_tail = start + unyielded;
            if me.tail_start != new_tail {
                im::copy(im::ptr_add(me.vec.ptr as *const T, me.tail_start), im::ptr_add_mut(me.vec.ptr, new_tail), me.tail_len);
            }
            me.vec.len = new_tail + me.tail_len;
        }
    }
}

impl<'a, T> Iterator for Drain<'a, T> {
    type Item = T;
    fn next(&mut self) -> Option<T> {
        if self.idx == self.end {
            None
        } else {
            let v = unsafe { im::ptr_read(im::ptr_add(self.vec.ptr as *const T, self.idx)) };
            self.idx += 1;
            Some(v)
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = self.end - self.idx;
        (n, Some(n))
    }
}

impl<'a, T> DoubleEndedIterator for Drain<'a, T> {
    fn next_back(&mut self) -> Option<T> {
        if self.idx == self.end {
            None
        } else {
            self.end -= 1;
            Some(unsafe { im::ptr_read(im::ptr_add(self.vec.ptr as *const T, self.end)) })
        }
    }
}

impl<'a, T> ExactSizeIterator for Drain<'a, T> {}
impl<'a, T> crate::iter::FusedIterator for Drain<'a, T> {}

impl<'a, T: fmt::Debug> fmt::Debug for Drain<'a, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Drain").field(&self.as_slice()).finish()
    }
}

impl<'a, T> Drop for Drain<'a, T> {
    fn drop(&mut self) {
        while self.idx < self.end {
            unsafe { im::drop_in_place(im::ptr_add_mut(self.vec.ptr, self.idx)) };
            self.idx += 1;
        }
        unsafe {
            let start = self.vec.len;
            if self.tail_len > 0 && self.tail_start != start {
                im::copy(im::ptr_add(self.vec.ptr as *const T, self.tail_start), im::ptr_add_mut(self.vec.ptr, start), self.tail_len);
            }
            self.vec.len = start + self.tail_len;
        }
    }
}
