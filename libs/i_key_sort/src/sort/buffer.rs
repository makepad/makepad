use core::ops::Range;
use core::ptr;

pub(crate) trait CopyFromNotOverlap<T> {
    fn copy_from_not_overlap(&mut self, buffer: &[T]);
    fn copy_to_range_from_not_overlap(&mut self, buffer: &[T], range: Range<usize>);
}

pub(crate) trait CopyNotOverlapValue<T> {
    fn copy_value_from(&mut self, src: &[T], index: usize);
}

impl<T: Copy> CopyNotOverlapValue<T> for [T] {
    #[inline(always)]
    fn copy_value_from(&mut self, src: &[T], index: usize) {
        unsafe {
            ptr::copy_nonoverlapping(src.as_ptr().add(index), self.as_mut_ptr().add(index), 1);
        }
    }
}

impl<T> CopyFromNotOverlap<T> for [T] {
    #[inline(always)]
    fn copy_from_not_overlap(&mut self, buffer: &[T]) {
        unsafe {
            ptr::copy_nonoverlapping(buffer.as_ptr(), self.as_mut_ptr(), self.len());
        }
    }

    #[inline(always)]
    fn copy_to_range_from_not_overlap(&mut self, buffer: &[T], range: Range<usize>) {
        debug_assert_eq!(range.len(), buffer.len());
        let dst = unsafe { self.get_unchecked_mut(range) };
        dst.copy_from_not_overlap(buffer);
    }
}

pub(crate) trait DoubleRangeSlices<T> {
    fn mut_slices<'a>(
        &self,
        slice1: &'a mut [T],
        slice2: &'a mut [T],
    ) -> (&'a mut [T], &'a mut [T]);
}

impl<T> DoubleRangeSlices<T> for Range<usize> {
    #[inline(always)]
    fn mut_slices<'a>(
        &self,
        slice1: &'a mut [T],
        slice2: &'a mut [T],
    ) -> (&'a mut [T], &'a mut [T]) {
        unsafe {
            let sub1 = slice1.get_unchecked_mut(self.clone());
            let sub2 = slice2.get_unchecked_mut(self.clone());
            (sub1, sub2)
        }
    }
}
