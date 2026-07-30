use crate::geom::v_segment::VSegment;
use crate::vector::edge::{DataVectorEdge, DataVectorPath};
use alloc::vec::Vec;
use i_float::int::number::int::IntNumber;
use i_float::int::point::IntPoint;
use i_shape::int::path::IntPath;

#[derive(Debug, Clone, Copy)]
pub(crate) struct ContourIndex {
    data: usize,
}

impl ContourIndex {
    pub(crate) const EMPTY: ContourIndex = ContourIndex { data: usize::MAX };

    #[inline]
    pub(crate) fn is_hole(&self) -> bool {
        self.data & 1 == 1
    }

    #[inline]
    pub(crate) fn index(&self) -> usize {
        self.data >> 1
    }

    #[inline]
    pub(crate) fn new_hole(index: usize) -> Self {
        Self {
            data: (index << 1) | 1,
        }
    }

    #[inline]
    pub(crate) fn new_shape(index: usize) -> Self {
        Self { data: index << 1 }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct IdSegment<I: IntNumber> {
    pub(crate) contour_index: ContourIndex,
    pub(crate) v_segment: VSegment<I>,
}

impl<I: IntNumber> IdSegment<I> {
    #[inline]
    fn new(data: ContourIndex, a: IntPoint<I>, b: IntPoint<I>) -> Self {
        Self {
            contour_index: data,
            v_segment: VSegment { a, b },
        }
    }

    #[inline]
    pub(crate) fn with_segment(data: ContourIndex, v_segment: VSegment<I>) -> Self {
        Self {
            contour_index: data,
            v_segment,
        }
    }
}

pub(crate) trait IdSegments<I: IntNumber> {
    fn append_id_segments(
        &self,
        buffer: &mut Vec<IdSegment<I>>,
        id_data: ContourIndex,
        x_min: I,
        x_max: I,
        clockwise: bool,
    );
}

impl<I: IntNumber> IdSegments<I> for IntPath<I> {
    #[inline]
    fn append_id_segments(
        &self,
        buffer: &mut Vec<IdSegment<I>>,
        id_data: ContourIndex,
        x_min: I,
        x_max: I,
        clockwise: bool,
    ) {
        fn inner<I: IntNumber, It: Iterator<Item = IntPoint<I>>>(
            mut iter: It,
            buffer: &mut Vec<IdSegment<I>>,
            id_data: ContourIndex,
            x_min: I,
            x_max: I,
        ) {
            let first = iter.next().unwrap();
            let mut b = first;
            for a in iter {
                if a.x < b.x && x_min < b.x && a.x <= x_max {
                    buffer.push(IdSegment::<I>::new(id_data, a, b));
                }
                b = a;
            }
            let a = first;
            if a.x < b.x && x_min < b.x && a.x <= x_max {
                buffer.push(IdSegment::<I>::new(id_data, a, b));
            }
        }

        if clockwise {
            inner(self.iter().copied(), buffer, id_data, x_min, x_max);
        } else {
            inner(self.iter().rev().copied(), buffer, id_data, x_min, x_max);
        }
    }
}

impl<I: IntNumber, D> IdSegments<I> for DataVectorPath<I, D> {
    #[inline]
    fn append_id_segments(
        &self,
        buffer: &mut Vec<IdSegment<I>>,
        id_data: ContourIndex,
        x_min: I,
        x_max: I,
        clockwise: bool,
    ) {
        fn inner<'a, D: 'a, I: IntNumber + 'a, It: Iterator<Item = &'a DataVectorEdge<I, D>>>(
            iter: It,
            buffer: &mut Vec<IdSegment<I>>,
            id_data: ContourIndex,
            x_min: I,
            x_max: I,
        ) {
            for vec in iter {
                if vec.a.x < vec.b.x && x_min < vec.b.x && vec.a.x <= x_max {
                    buffer.push(IdSegment::<I>::new(id_data, vec.a, vec.b));
                }
            }
        }

        if clockwise {
            inner(self.iter(), buffer, id_data, x_min, x_max);
        } else {
            inner(self.iter().rev(), buffer, id_data, x_min, x_max);
        }
    }
}
