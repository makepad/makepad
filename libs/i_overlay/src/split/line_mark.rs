use alloc::vec::Vec;
use i_float::int::number::int::IntNumber;
use i_float::int::point::IntPoint;
use i_key_sort::sort::one_key_cmp::OneKeyAndCmpSort;

#[derive(Clone, Copy, PartialEq)]
pub(super) struct LineMark<I: IntNumber> {
    pub(super) index: usize,
    pub(super) point: IntPoint<I>,
}

pub(super) trait SortMarkByIndexAndPoint<I: IntNumber> {
    fn sort_by_index_and_point(&mut self, parallel: bool, reusable_buffer: &mut Vec<LineMark<I>>);
}

impl<I: IntNumber> SortMarkByIndexAndPoint<I> for [LineMark<I>] {
    #[inline]
    fn sort_by_index_and_point(&mut self, parallel: bool, reusable_buffer: &mut Vec<LineMark<I>>) {
        self.sort_by_one_key_then_by_and_buffer(
            parallel,
            reusable_buffer,
            |m| m.index,
            |m0, m1| m0.point.cmp(&m1.point),
        );
    }
}
