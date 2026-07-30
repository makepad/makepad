use crate::geom::x_segment::XSegment;
use crate::segm::boolean::ShapeCountBoolean;
use crate::segm::segment::Segment;
use i_float::int::number::int::IntNumber;
use i_float::int::point::IntPoint;

impl<I: IntNumber> Segment<ShapeCountBoolean, I> {
    #[inline]
    pub(crate) fn subject(p0: IntPoint<I>, p1: IntPoint<I>) -> Self {
        if p0 < p1 {
            Self {
                x_segment: XSegment { a: p0, b: p1 },
                count: ShapeCountBoolean { subj: 1, clip: 0 },
                data: (),
            }
        } else {
            Self {
                x_segment: XSegment { a: p1, b: p0 },
                count: ShapeCountBoolean { subj: -1, clip: 0 },
                data: (),
            }
        }
    }
}
