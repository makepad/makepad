use crate::mesh::math::Math;
use crate::segm::boolean::ShapeCountBoolean;
use crate::segm::segment::Segment;
use alloc::vec::Vec;
use i_float::adapter::FloatPointAdapter;
use i_float::float::compatible::FloatPointCompatible;
use i_float::float::vector::FloatPointMath;
use i_float::int::number::int::IntNumber;

#[derive(Clone)]
pub(super) struct Section<P: FloatPointCompatible> {
    pub(super) a: P,
    pub(super) b: P,
    pub(super) a_top: P,
    pub(super) b_top: P,
    pub(super) a_bot: P,
    pub(super) b_bot: P,
    pub(super) dir: P,
}

impl<P: FloatPointCompatible> Section<P> {
    pub(crate) fn new(radius: P::Scalar, a: &P, b: &P) -> Self {
        let dir = Math::normal(b, a);
        let t = Math::ortho_and_scale(&dir, radius);

        let a_top = FloatPointMath::add(a, &t);
        let a_bot = FloatPointMath::sub(a, &t);

        let b_top = FloatPointMath::add(b, &t);
        let b_bot = FloatPointMath::sub(b, &t);

        Section {
            a: *a,
            b: *b,
            a_top,
            b_top,
            a_bot,
            b_bot,
            dir,
        }
    }
}

pub(crate) trait SectionToSegment<P: FloatPointCompatible, I: IntNumber> {
    fn add_section(&mut self, section: &Section<P>, adapter: &FloatPointAdapter<P, I>);
}

impl<P: FloatPointCompatible, I: IntNumber> SectionToSegment<P, I> for Vec<Segment<ShapeCountBoolean, I>> {
    fn add_section(&mut self, section: &Section<P>, adapter: &FloatPointAdapter<P, I>) {
        let a_top = adapter.float_to_int(&section.a_top);
        let b_top = adapter.float_to_int(&section.b_top);
        let a_bot = adapter.float_to_int(&section.a_bot);
        let b_bot = adapter.float_to_int(&section.b_bot);

        if a_top != b_top {
            self.push(Segment::subject(b_top, a_top));
        }
        if a_bot != b_bot {
            self.push(Segment::subject(a_bot, b_bot));
        }
    }
}
