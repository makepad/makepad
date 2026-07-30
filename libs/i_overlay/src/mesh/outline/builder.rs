use crate::mesh::math::Math;
use crate::mesh::outline::builder_join::JoinBuilder;
use crate::mesh::outline::builder_join::{BevelJoinBuilder, MiterJoinBuilder, RoundJoinBuilder};
use crate::mesh::outline::section::OffsetSection;
use crate::mesh::outline::uniq_iter::{UniqueSegment, UniqueSegmentsIter};
use crate::mesh::style::LineJoin;
use crate::segm::boolean::ShapeCountBoolean;
use crate::segm::segment::Segment;
use alloc::boxed::Box;
use alloc::vec::Vec;
use core::marker::PhantomData;
use i_float::adapter::FloatPointAdapter;
use i_float::float::compatible::FloatPointCompatible;
use i_float::float::number::FloatNumber;
use i_float::float::vector::FloatPointMath;
use i_float::int::number::int::IntNumber;
use i_float::int::number::wide_int::WideIntNumber;

trait OutlineBuild<P: FloatPointCompatible, I: IntNumber> {
    fn build(
        &self,
        path: &[P],
        adapter: &FloatPointAdapter<P, I>,
        segments: &mut Vec<Segment<ShapeCountBoolean, I>>,
    );

    fn capacity(&self, points_count: usize) -> usize;
    fn additional_offset(&self, radius: P::Scalar) -> P::Scalar;
}

pub(super) struct OutlineBuilder<P: FloatPointCompatible, I: IntNumber> {
    builder: Box<dyn OutlineBuild<P, I>>,
}

struct Builder<J: JoinBuilder<P, I>, P: FloatPointCompatible, I: IntNumber> {
    extend: bool,
    radius: P::Scalar,
    join_builder: J,
    _phantom: PhantomData<(P, I)>,
}

impl<P: FloatPointCompatible + 'static, I: IntNumber + 'static> OutlineBuilder<P, I> {
    pub(super) fn new(radius: P::Scalar, join: &LineJoin<P::Scalar>) -> OutlineBuilder<P, I> {
        let extend = radius > P::Scalar::from_float(0.0);
        let builder: Box<dyn OutlineBuild<P, I>> = {
            match join {
                LineJoin::Miter(ratio) => Box::new(Builder {
                    extend,
                    radius,
                    join_builder: MiterJoinBuilder::new(*ratio, radius),
                    _phantom: Default::default(),
                }),
                LineJoin::Round(ratio) => Box::new(Builder {
                    extend,
                    radius,
                    join_builder: RoundJoinBuilder::new(*ratio, radius),
                    _phantom: Default::default(),
                }),
                LineJoin::Bevel => Box::new(Builder {
                    extend,
                    radius,
                    join_builder: BevelJoinBuilder {},
                    _phantom: Default::default(),
                }),
            }
        };

        Self { builder }
    }

    #[inline]
    pub(super) fn build(
        &self,
        path: &[P],
        adapter: &FloatPointAdapter<P, I>,
        segments: &mut Vec<Segment<ShapeCountBoolean, I>>,
    ) {
        self.builder.build(path, adapter, segments);
    }

    #[inline]
    pub(super) fn capacity(&self, points_count: usize) -> usize {
        self.builder.capacity(points_count)
    }

    #[inline]
    pub(super) fn additional_offset(&self, radius: P::Scalar) -> P::Scalar {
        self.builder.additional_offset(radius)
    }
}

impl<J: JoinBuilder<P, I>, P: FloatPointCompatible, I: IntNumber> OutlineBuild<P, I> for Builder<J, P, I> {
    #[inline]
    fn build(
        &self,
        path: &[P],
        adapter: &FloatPointAdapter<P, I>,
        segments: &mut Vec<Segment<ShapeCountBoolean, I>>,
    ) {
        if path.len() < 2 {
            return;
        }

        self.build(path, adapter, segments);
    }

    #[inline]
    fn capacity(&self, points_count: usize) -> usize {
        self.join_builder.capacity() * points_count
    }

    #[inline]
    fn additional_offset(&self, radius: P::Scalar) -> P::Scalar {
        self.join_builder.additional_offset(radius)
    }
}

impl<J: JoinBuilder<P, I>, P: FloatPointCompatible, I: IntNumber> Builder<J, P, I> {
    fn build(
        &self,
        path: &[P],
        adapter: &FloatPointAdapter<P, I>,
        segments: &mut Vec<Segment<ShapeCountBoolean, I>>,
    ) {
        let iter = path.iter().map(|p| adapter.float_to_int(p));
        let mut uniq_segments = if let Some(iter) = UniqueSegmentsIter::new(iter) {
            iter
        } else {
            return;
        };

        let us0 = if let Some(us) = uniq_segments.next() {
            us
        } else {
            return;
        };

        let s0 = OffsetSection::new(self.radius, &us0, adapter);
        let mut sk = s0.clone();

        segments.push_some(sk.top_segment());

        for usi in uniq_segments {
            let si = OffsetSection::new(self.radius, &usi, adapter);
            segments.push_some(si.top_segment());
            self.feed_join(&sk, &si, adapter, segments);
            sk = si;
        }
        self.feed_join(&sk, &s0, adapter, segments);
    }

    #[inline]
    fn feed_join(
        &self,
        s0: &OffsetSection<P, I>,
        s1: &OffsetSection<P, I>,
        adapter: &FloatPointAdapter<P, I>,
        segments: &mut Vec<Segment<ShapeCountBoolean, I>>,
    ) {
        let vi = s1.b - s1.a;
        let vp = s0.b - s0.a;

        let cross = vi.cross_product(vp);

        let outer_corner = if cross != I::Wide::ZERO {
            (cross > I::Wide::ZERO) == self.extend
        } else {
            vi.dot_product(vp) < I::Wide::ZERO
        };

        if outer_corner {
            if s0.b_top != s1.a_top {
                self.join_builder.add_join(s0, s1, adapter, segments);
            }
        } else {
            // no join
            segments.push_some(s0.b_segment());
            segments.push_some(s1.a_segment());
        }
    }
}

impl<P: FloatPointCompatible, I: IntNumber> OffsetSection<P, I> {
    #[inline]
    fn new(radius: P::Scalar, s: &UniqueSegment<I>, adapter: &FloatPointAdapter<P, I>) -> Self {
        let a = adapter.int_to_float(&s.a);
        let b = adapter.int_to_float(&s.b);
        let ab = FloatPointMath::sub(&b, &a);
        let dir = FloatPointMath::normalize(&ab);
        let t = Math::ortho_and_scale(&dir, radius);

        let at = FloatPointMath::add(&a, &t);
        let bt = FloatPointMath::add(&b, &t);
        let a_top = adapter.float_to_int(&at);
        let b_top = adapter.float_to_int(&bt);

        Self {
            a: s.a,
            b: s.b,
            a_top,
            b_top,
            dir,
        }
    }
}

trait VecPushSome<T> {
    fn push_some(&mut self, value: Option<T>);
}

impl<T> VecPushSome<T> for Vec<T> {
    #[inline]
    fn push_some(&mut self, value: Option<T>) {
        if let Some(v) = value {
            self.push(v);
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::mesh::outline::builder::OutlineBuilder;
    use crate::mesh::style::LineJoin;
    use crate::segm::boolean::ShapeCountBoolean;
    use crate::segm::segment::Segment;
    use alloc::vec::Vec;
    use i_float::adapter::FloatPointAdapter;
    use i_float::float::rect::FloatRect;

    #[test]
    fn test_i64_builder() {
        let path = [[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 10.0]];
        let mut rect = FloatRect::with_iter(path.iter()).unwrap();
        rect.add_offset(2.0);
        let adapter = FloatPointAdapter::<[f64; 2], i64>::new(rect);
        let builder = OutlineBuilder::<[f64; 2], i64>::new(1.0, &LineJoin::Bevel);

        let mut segments = Vec::<Segment<ShapeCountBoolean, i64>>::new();
        builder.build(&path, &adapter, &mut segments);

        assert!(!segments.is_empty());
    }
}
