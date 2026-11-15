//! Vec::splice: port of library/alloc/src/vec/splice.rs @1.100.0.

use super::{Drain, Vec};
use crate::fmt;
use crate::heap::{Allocator, Global};
use crate::intrinsics_mem as im;

/// Yields the removed elements of a range (a Drain of it); its drop drops the ones not taken
/// and puts the replacement's items in their place, moving the tail as far as needed.
pub struct Splice<'a, I: Iterator + 'a, #[unstable(feature = "allocator_ext", issue = "163177", implied_by = "allocator_api")] A: Allocator + 'a = Global> {
    pub(super) drain: Drain<'a, I::Item, A>,
    pub(super) replace_with: I,
}

// library/'s `#[derive(Debug)]`
impl<'a, I: Iterator + fmt::Debug + 'a, A: Allocator + fmt::Debug + 'a> fmt::Debug for Splice<'a, I, A>
where
    I::Item: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Splice").field("drain", &self.drain).field("replace_with", &self.replace_with).finish()
    }
}

impl<I: Iterator, A: Allocator> Iterator for Splice<'_, I, A> {
    type Item = I::Item;
    fn next(&mut self) -> Option<I::Item> {
        self.drain.next()
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.drain.size_hint()
    }
}

impl<I: Iterator, A: Allocator> DoubleEndedIterator for Splice<'_, I, A> {
    fn next_back(&mut self) -> Option<I::Item> {
        self.drain.next_back()
    }
}

impl<I: Iterator, A: Allocator> ExactSizeIterator for Splice<'_, I, A> {}

impl<I: Iterator, A: Allocator> Drop for Splice<'_, I, A> {
    fn drop(&mut self) {
        // the removed elements not yet taken drop first, as library/'s `for_each(drop)`
        while self.drain.next().is_some() {}
        if self.drain.tail_len == 0 {
            self.drain.vec.extend(self.replace_with.by_ref());
            return;
        }
        // first fill the range left by the drain
        if !self.drain.fill(&mut self.replace_with) {
            return;
        }
        // more to insert than the range had: the lower bound of the size hint first
        let (lower_bound, _) = self.replace_with.size_hint();
        if lower_bound > 0 {
            self.drain.move_tail(lower_bound);
            if !self.drain.fill(&mut self.replace_with) {
                return;
            }
        }
        // the rest collected, so its exact count moves the tail once more
        let mut collected = self.replace_with.by_ref().collect::<Vec<I::Item>>().into_iter();
        if collected.len() > 0 {
            self.drain.move_tail(collected.len());
            self.drain.fill(&mut collected);
        }
        // the Drain's drop moves the tail back next to the filled range
    }
}

impl<T, A: Allocator> Drain<'_, T, A> {
    /// Writes items of `replace_with` into `vec[vec.len..tail_start]`; whether that range filled.
    fn fill<I: Iterator<Item = T>>(&mut self, replace_with: &mut I) -> bool {
        let range_start = self.vec.len;
        let range_end = self.tail_start;
        let mut idx = range_start;
        while idx < range_end {
            let Some(new_item) = replace_with.next() else {
                return false;
            };
            unsafe { im::ptr_write(im::ptr_add_mut(self.vec.ptr.as_ptr(), idx), new_item) };
            self.vec.len += 1;
            idx += 1;
        }
        true
    }

    /// Makes room for `additional` more before the tail, moving it up.
    fn move_tail(&mut self, additional: usize) {
        let len = self.tail_start + self.tail_len;
        self.vec.reserve_after(len, additional);
        let new_tail_start = self.tail_start + additional;
        unsafe {
            let src = im::ptr_add(self.vec.ptr.as_ptr() as *const T, self.tail_start);
            let dst = im::ptr_add_mut(self.vec.ptr.as_ptr(), new_tail_start);
            im::copy(src, dst, self.tail_len);
        }
        self.tail_start = new_tail_start;
    }
}
