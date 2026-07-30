use crate::segm::boolean::ShapeCountBoolean;
use crate::segm::segment::Segment;
use i_float::float::compatible::FloatPointCompatible;
use i_float::int::number::int::IntNumber;
use i_float::int::point::IntPoint;

#[derive(Clone)]
pub(super) struct OffsetSection<P: FloatPointCompatible, I: IntNumber> {
    pub(super) a: IntPoint<I>,
    pub(super) b: IntPoint<I>,
    pub(super) a_top: IntPoint<I>,
    pub(super) b_top: IntPoint<I>,
    pub(super) dir: P,
}

impl<P: FloatPointCompatible, I: IntNumber> OffsetSection<P, I> {
    #[inline]
    pub(super) fn top_segment(&self) -> Option<Segment<ShapeCountBoolean, I>> {
        if self.a_top != self.b_top {
            Some(Segment::subject(self.a_top, self.b_top))
        } else {
            None
        }
    }

    #[inline]
    pub(super) fn a_segment(&self) -> Option<Segment<ShapeCountBoolean, I>> {
        if self.a_top != self.a {
            Some(Segment::subject(self.a, self.a_top))
        } else {
            None
        }
    }

    #[inline]
    pub(super) fn b_segment(&self) -> Option<Segment<ShapeCountBoolean, I>> {
        if self.b_top != self.b {
            Some(Segment::subject(self.b_top, self.b))
        } else {
            None
        }
    }
}
