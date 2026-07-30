use crate::geom::x_segment::XSegment;
use i_float::int::number::int::IntNumber;
use i_float::int::rect::IntRect;

#[derive(Clone)]
pub(super) struct Fragment<I: IntNumber> {
    pub(super) index: usize,
    pub(super) rect: IntRect<I>,
    pub(super) x_segment: XSegment<I>,
}

impl<I: IntNumber> Fragment<I> {
    #[inline]
    pub(super) fn with_index_and_segment(index: usize, x_segment: XSegment<I>) -> Self {
        let (min_y, max_y) = if x_segment.a.y < x_segment.b.y {
            (x_segment.a.y, x_segment.b.y)
        } else {
            (x_segment.b.y, x_segment.a.y)
        };

        let rect = IntRect {
            min_x: x_segment.a.x,
            max_x: x_segment.b.x,
            min_y,
            max_y,
        };

        Self {
            index,
            rect,
            x_segment,
        }
    }
}
