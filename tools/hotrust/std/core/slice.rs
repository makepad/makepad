//! Slice iterators.

use crate::mem::intrinsics_mem as im;

pub struct Iter<'a, T> {
    ptr: *const T,
    end: usize,
    i: usize,
    _s: &'a [T],
}

impl<'a, T> crate::iter::Iterator for Iter<'a, T> {
    type Item = &'a T;
    fn next(&mut self) -> Option<&'a T> {
        if self.i < self.end {
            let p = unsafe { im::ptr_add(self.ptr, self.i) };
            self.i += 1;
            Some(unsafe { &*p })
        } else {
            None
        }
    }
}

pub struct IterMut<'a, T> {
    ptr: *mut T,
    end: usize,
    i: usize,
    _s: &'a mut [T],
}

impl<'a, T> crate::iter::Iterator for IterMut<'a, T> {
    type Item = &'a mut T;
    fn next(&mut self) -> Option<&'a mut T> {
        if self.i < self.end {
            let p = unsafe { im::ptr_add_mut(self.ptr, self.i) };
            self.i += 1;
            Some(unsafe { &mut *p })
        } else {
            None
        }
    }
}

pub fn iter<'a, T>(s: &'a [T]) -> Iter<'a, T> {
    Iter { ptr: unsafe { im::slice_ptr(s) }, end: s.len(), i: 0, _s: s }
}
