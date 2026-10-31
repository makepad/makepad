//! Vec::extract_if: port of library/alloc/src/vec/extract_if.rs (stable 1.87).

use super::Vec;
use crate::fmt;
use crate::intrinsics_mem as im;

/// Yields the elements of a range of the vec the predicate selects, removing them; the kept
/// ones move down over the holes. While it lives the vec's len is 0 (leak-safe, as alloc); on
/// drop the unvisited tail moves down and the len is set.
#[must_use = "iterators are lazy and do nothing unless consumed; \
    use `retain_mut` or `extract_if().for_each(drop)` to remove and discard elements"]
pub struct ExtractIf<'a, T, F> {
    vec: &'a mut Vec<T>,
    /// the next element to check
    idx: usize,
    /// one past the last element of the range
    end: usize,
    /// elements yielded so far
    del: usize,
    old_len: usize,
    pred: F,
}

pub(super) fn new<'a, T, F>(vec: &'a mut Vec<T>, pred: F, start: usize, end: usize) -> ExtractIf<'a, T, F> {
    let old_len = vec.len;
    vec.len = 0;
    ExtractIf { vec, idx: start, end, del: 0, old_len, pred }
}

impl<T, F: FnMut(&mut T) -> bool> Iterator for ExtractIf<'_, T, F> {
    type Item = T;

    fn next(&mut self) -> Option<T> {
        while self.idx < self.end {
            let i = self.idx;
            let cur = unsafe { &mut *im::ptr_add_mut(self.vec.ptr.as_ptr(), i) };
            let drained = (self.pred)(cur);
            // after the predicate (it may panic): a panic leaves this element in the vec
            self.idx += 1;
            if drained {
                self.del += 1;
                return Some(unsafe { im::ptr_read(cur as *const T) });
            } else if self.del > 0 {
                unsafe { im::copy_nonoverlapping(cur as *const T, im::ptr_add_mut(self.vec.ptr.as_ptr(), i - self.del), 1) };
            }
        }
        None
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, Some(self.end - self.idx))
    }
}

impl<T, F> Drop for ExtractIf<'_, T, F> {
    fn drop(&mut self) {
        if self.del > 0 {
            unsafe {
                im::copy(im::ptr_add(self.vec.ptr.as_ptr() as *const T, self.idx), im::ptr_add_mut(self.vec.ptr.as_ptr(), self.idx - self.del), self.old_len - self.idx);
            }
        }
        self.vec.len = self.old_len - self.del;
    }
}

impl<T: fmt::Debug, F> fmt::Debug for ExtractIf<'_, T, F> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let start = self.vec.ptr.as_ptr() as *const T;
        let retained = unsafe { im::slice_from_raw(start, self.idx - self.del) };
        let remainder = unsafe { im::slice_from_raw(im::ptr_add(start, self.idx), self.end - self.idx) };
        let skipped_tail = unsafe { im::slice_from_raw(im::ptr_add(start, self.end), self.old_len - self.end) };
        f.debug_struct("ExtractIf").field("retained", &retained).field("remainder", &remainder).field("skipped_tail", &skipped_tail).finish_non_exhaustive()
    }
}
