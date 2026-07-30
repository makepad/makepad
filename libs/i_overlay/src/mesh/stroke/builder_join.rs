use crate::mesh::miter::{Miter, SharpMiter};
use crate::mesh::rotator::Rotator;
use crate::mesh::stroke::section::Section;
use crate::segm::boolean::ShapeCountBoolean;
use crate::segm::segment::Segment;
use alloc::vec::Vec;
use core::f64::consts::PI;
use i_float::adapter::FloatPointAdapter;
use i_float::float::compatible::FloatPointCompatible;
use i_float::float::number::FloatNumber;
use i_float::float::vector::FloatPointMath;
use i_float::int::number::int::IntNumber;

pub(super) trait JoinBuilder<P: FloatPointCompatible, I: IntNumber> {
    fn add_join(
        &self,
        s0: &Section<P>,
        s1: &Section<P>,
        adapter: &FloatPointAdapter<P, I>,
        segments: &mut Vec<Segment<ShapeCountBoolean, I>>,
    );
    fn capacity(&self) -> usize;
    fn additional_offset(&self, radius: P::Scalar) -> P::Scalar;
}

pub(super) struct BevelJoinBuilder;

impl BevelJoinBuilder {
    #[inline]
    fn join_top<P: FloatPointCompatible, I: IntNumber>(
        s0: &Section<P>,
        s1: &Section<P>,
        adapter: &FloatPointAdapter<P, I>,
        segments: &mut Vec<Segment<ShapeCountBoolean, I>>,
    ) {
        Self::add_segment(&s0.b_top, &s1.a_top, adapter, segments);
    }

    #[inline]
    fn join_bot<P: FloatPointCompatible, I: IntNumber>(
        s0: &Section<P>,
        s1: &Section<P>,
        adapter: &FloatPointAdapter<P, I>,
        segments: &mut Vec<Segment<ShapeCountBoolean, I>>,
    ) {
        Self::add_segment(&s1.a_bot, &s0.b_bot, adapter, segments);
    }

    #[inline]
    fn add_segment<P: FloatPointCompatible, I: IntNumber>(
        a: &P,
        b: &P,
        adapter: &FloatPointAdapter<P, I>,
        segments: &mut Vec<Segment<ShapeCountBoolean, I>>,
    ) {
        let ia = adapter.float_to_int(a);
        let ib = adapter.float_to_int(b);
        if ia != ib {
            segments.push(Segment::subject(ib, ia));
        }
    }
}

impl<P: FloatPointCompatible, I: IntNumber> JoinBuilder<P, I> for BevelJoinBuilder {
    #[inline]
    fn add_join(
        &self,
        s0: &Section<P>,
        s1: &Section<P>,
        adapter: &FloatPointAdapter<P, I>,
        segments: &mut Vec<Segment<ShapeCountBoolean, I>>,
    ) {
        Self::join_top(s0, s1, adapter, segments);
        Self::join_bot(s0, s1, adapter, segments);
    }

    #[inline]
    fn capacity(&self) -> usize {
        2
    }

    #[inline]
    fn additional_offset(&self, radius: P::Scalar) -> P::Scalar {
        // add extra 10% to avoid problems with floating point precision.
        P::Scalar::from_float(1.1) * radius
    }
}

pub(super) struct MiterJoinBuilder<T> {
    limit_dot_product: T,
    max_offset: T,
    max_length: T,
}

impl<T: FloatNumber> MiterJoinBuilder<T> {
    pub(super) fn new(angle: T, radius: T) -> Self {
        // angle - min possible angle
        let fixed_angle = angle.max(T::from_float(0.01));
        let limit_dot_product = -fixed_angle.cos();

        let half_angle = T::from_float(0.5) * fixed_angle;
        let tan = half_angle.tan();

        let r = radius;
        let max_length = r / tan;
        let sqr_len = max_length * max_length;
        let sqr_rad = r * r;
        // add extra 10% to avoid problems with floating point precision.
        let extra_scale = T::from_float(1.1);

        let max_offset = extra_scale * (sqr_rad + sqr_len).sqrt();

        Self {
            limit_dot_product,
            max_offset,
            max_length,
        }
    }
}

impl<P: FloatPointCompatible, I: IntNumber> JoinBuilder<P, I> for MiterJoinBuilder<P::Scalar> {
    fn add_join(
        &self,
        s0: &Section<P>,
        s1: &Section<P>,
        adapter: &FloatPointAdapter<P, I>,
        segments: &mut Vec<Segment<ShapeCountBoolean, I>>,
    ) {
        let cross_product = FloatPointMath::cross_product(&s0.dir, &s1.dir);
        if cross_product.abs() < P::Scalar::from_float(0.0001) {
            BevelJoinBuilder::join_top(s0, s1, adapter, segments);
            BevelJoinBuilder::join_bot(s0, s1, adapter, segments);
            return;
        }

        let turn = cross_product > P::Scalar::from_float(0.0);

        let dot_product = FloatPointMath::dot_product(&s0.dir, &s1.dir);

        let is_limited = self.limit_dot_product > dot_product;

        if is_limited {
            let (pa, pb, ac, bc) = if turn {
                BevelJoinBuilder::join_top(s0, s1, adapter, segments);
                let (pa, pb, va, vb) = (s1.a_bot, s0.b_bot, s1.dir, s0.dir);

                let ax = pa.x() - self.max_length * va.x();
                let ay = pa.y() - self.max_length * va.y();
                let bx = pb.x() + self.max_length * vb.x();
                let by = pb.y() + self.max_length * vb.y();

                let ac = P::from_xy(ax, ay);
                let bc = P::from_xy(bx, by);

                (pa, pb, ac, bc)
            } else {
                BevelJoinBuilder::join_bot(s0, s1, adapter, segments);
                let (pa, pb, va, vb) = (s0.b_top, s1.a_top, s0.dir, s1.dir);

                let ax = pa.x() + self.max_length * va.x();
                let ay = pa.y() + self.max_length * va.y();
                let bx = pb.x() - self.max_length * vb.x();
                let by = pb.y() - self.max_length * vb.y();

                let ac = P::from_xy(ax, ay);
                let bc = P::from_xy(bx, by);

                (pa, pb, ac, bc)
            };

            let ia = adapter.float_to_int(&pa);
            let ib = adapter.float_to_int(&pb);

            if ia == ib {
                return;
            }

            let iac = adapter.float_to_int(&ac);
            let ibc = adapter.float_to_int(&bc);

            if ia != iac {
                segments.push(Segment::subject(iac, ia));
            }
            if iac != ibc {
                segments.push(Segment::subject(ibc, iac));
            }
            if ibc != ib {
                segments.push(Segment::subject(ib, ibc));
            }
        } else {
            let (pa, pb, va, vb) = if turn {
                BevelJoinBuilder::join_top(s0, s1, adapter, segments);
                (s1.a_bot, s0.b_bot, s1.dir, s0.dir)
            } else {
                BevelJoinBuilder::join_bot(s0, s1, adapter, segments);
                (s0.b_top, s1.a_top, s0.dir, s1.dir)
            };
            match Miter::sharp(pa, pb, va, vb, adapter) {
                SharpMiter::AB(a, b) => segments.push(Segment::subject(b, a)),
                SharpMiter::AcB(a, c, b) => {
                    segments.push(Segment::subject(c, a));
                    segments.push(Segment::subject(b, c));
                }
                SharpMiter::Degenerate => {}
            }
        }
    }

    #[inline]
    fn capacity(&self) -> usize {
        4
    }

    #[inline]
    fn additional_offset(&self, _radius: P::Scalar) -> P::Scalar {
        self.max_offset
    }
}

pub(super) struct RoundJoinBuilder<T> {
    inv_ratio: T,
    average_count: usize,
    radius: T,
    limit_dot_product: T,
}

impl<T: FloatNumber> RoundJoinBuilder<T> {
    pub(super) fn new(ratio: T, radius: T) -> Self {
        // ratio = A / R
        let fixed_ratio = ratio.min(T::from_float(0.25 * PI));
        let limit_dot_product = fixed_ratio.cos();
        let average_count = (T::from_float(0.6 * PI) / fixed_ratio).to_usize() + 2;
        Self {
            inv_ratio: T::from_float(1.0) / fixed_ratio,
            average_count,
            radius,
            limit_dot_product,
        }
    }
}
impl<P: FloatPointCompatible, I: IntNumber> JoinBuilder<P, I> for RoundJoinBuilder<P::Scalar> {
    fn add_join(
        &self,
        s0: &Section<P>,
        s1: &Section<P>,
        adapter: &FloatPointAdapter<P, I>,
        segments: &mut Vec<Segment<ShapeCountBoolean, I>>,
    ) {
        let dot_product = FloatPointMath::dot_product(&s0.dir, &s1.dir);
        if self.limit_dot_product < dot_product {
            BevelJoinBuilder::join_top(s0, s1, adapter, segments);
            BevelJoinBuilder::join_bot(s0, s1, adapter, segments);
            return;
        }

        let angle = dot_product.acos();
        let n = (angle * self.inv_ratio).to_usize();
        let delta_angle = angle / P::Scalar::from_usize(n);

        let cross_product = FloatPointMath::cross_product(&s0.dir, &s1.dir);
        let (start, end, dir) = if cross_product > P::Scalar::from_float(0.0) {
            BevelJoinBuilder::join_top(s0, s1, adapter, segments);
            let ortho = P::from_xy(s1.dir.y(), -s1.dir.x());
            (s1.a_bot, s0.b_bot, ortho)
        } else {
            BevelJoinBuilder::join_bot(s0, s1, adapter, segments);
            let ortho = P::from_xy(-s0.dir.y(), s0.dir.x());
            (s0.b_top, s1.a_top, ortho)
        };
        let rotator = Rotator::<P::Scalar>::with_angle(-delta_angle);

        let center = s0.b;
        let mut v = dir;
        let mut a = adapter.float_to_int(&start);
        for _ in 1..n {
            v = rotator.rotate(&v);
            let p = FloatPointMath::add(&center, &FloatPointMath::scale(&v, self.radius));

            let b = adapter.float_to_int(&p);
            if a != b {
                segments.push(Segment::subject(b, a));
                a = b;
            }
        }

        let b = adapter.float_to_int(&end);
        if a != b {
            segments.push(Segment::subject(b, a));
        }
    }

    #[inline]
    fn capacity(&self) -> usize {
        self.average_count
    }

    #[inline]
    fn additional_offset(&self, radius: P::Scalar) -> P::Scalar {
        // add extra 10% to avoid problems with floating point precision.
        P::Scalar::from_float(1.1) * radius
    }
}
