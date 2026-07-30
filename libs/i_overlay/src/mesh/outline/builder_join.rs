use crate::mesh::miter::Miter;
use crate::mesh::outline::section::OffsetSection;
use crate::mesh::rotator::Rotator;
use crate::segm::boolean::ShapeCountBoolean;
use crate::segm::segment::Segment;
use alloc::vec::Vec;
use core::f64::consts::PI;
use i_float::adapter::FloatPointAdapter;
use i_float::float::compatible::FloatPointCompatible;
use i_float::float::number::FloatNumber;
use i_float::float::vector::FloatPointMath;
use i_float::int::number::int::IntNumber;
use i_float::int::number::wide_int::WideIntNumber;

pub(super) trait JoinBuilder<P: FloatPointCompatible, I: IntNumber> {
    fn add_join(
        &self,
        s0: &OffsetSection<P, I>,
        s1: &OffsetSection<P, I>,
        adapter: &FloatPointAdapter<P, I>,
        segments: &mut Vec<Segment<ShapeCountBoolean, I>>,
    );
    fn capacity(&self) -> usize;
    fn additional_offset(&self, radius: P::Scalar) -> P::Scalar;
}

pub(super) struct BevelJoinBuilder;

impl BevelJoinBuilder {
    #[inline]
    fn join<P: FloatPointCompatible, I: IntNumber>(
        s0: &OffsetSection<P, I>,
        s1: &OffsetSection<P, I>,
        _adapter: &FloatPointAdapter<P, I>,
        segments: &mut Vec<Segment<ShapeCountBoolean, I>>,
    ) {
        debug_assert!(s0.b_top != s1.a_top, "must be validated before");
        segments.push(Segment::subject(s0.b_top, s1.a_top));
    }
}

impl<P: FloatPointCompatible, I: IntNumber> JoinBuilder<P, I> for BevelJoinBuilder {
    #[inline]
    fn add_join(
        &self,
        s0: &OffsetSection<P, I>,
        s1: &OffsetSection<P, I>,
        adapter: &FloatPointAdapter<P, I>,
        segments: &mut Vec<Segment<ShapeCountBoolean, I>>,
    ) {
        Self::join(s0, s1, adapter, segments);
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
        let fixed_angle = angle.to_f64().max(0.01);
        let limit_dot_product = -T::from_float(fixed_angle.cos());

        let half_angle = 0.5 * fixed_angle;
        let tan = half_angle.tan();

        let r = radius.to_f64().abs();
        let l = r / tan;

        // add extra 10% to avoid problems with floating point precision.
        let max_offset = T::from_float(1.1 * (r * r + l * l).sqrt());
        let max_length = T::from_float(l);

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
        s0: &OffsetSection<P, I>,
        s1: &OffsetSection<P, I>,
        adapter: &FloatPointAdapter<P, I>,
        segments: &mut Vec<Segment<ShapeCountBoolean, I>>,
    ) {
        let ia = s0.b_top;
        let ib = s1.a_top;

        let sq_len = ia.sqr_distance(ib);
        if sq_len < I::Wide::from_usize(4) {
            BevelJoinBuilder::join(s0, s1, adapter, segments);
            return;
        }

        let dot_product = FloatPointMath::dot_product(&s0.dir, &s1.dir);
        let is_limited = self.limit_dot_product > dot_product;

        let pa = adapter.int_to_float(&ia);
        let pb = adapter.int_to_float(&ib);

        if is_limited {
            let (va, vb) = (s0.dir, s1.dir);

            let ax = pa.x() + self.max_length * va.x();
            let ay = pa.y() + self.max_length * va.y();
            let bx = pb.x() - self.max_length * vb.x();
            let by = pb.y() - self.max_length * vb.y();

            let ac = P::from_xy(ax, ay);
            let bc = P::from_xy(bx, by);

            let iac = adapter.float_to_int(&ac);
            let ibc = adapter.float_to_int(&bc);

            if ia != iac {
                segments.push(Segment::subject(ia, iac));
            }
            if iac != ibc {
                segments.push(Segment::subject(iac, ibc));
            }
            if ibc != ib {
                segments.push(Segment::subject(ibc, ib));
            }
        } else {
            let c = Miter::peak(pa, pb, s0.dir, s1.dir);
            debug_assert!(ia != ib);

            let ic = adapter.float_to_int(&c);
            if ia == ic || ib == ic {
                segments.push(Segment::subject(ia, ib))
            } else {
                segments.push(Segment::subject(ia, ic));
                segments.push(Segment::subject(ic, ib));
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
    rot_dir: T,
}

impl<T: FloatNumber> RoundJoinBuilder<T> {
    pub(super) fn new(ratio: T, radius: T) -> Self {
        // ratio = A / R
        let fixed_ratio = ratio.min(T::from_float(0.25 * PI));
        let limit_dot_product = fixed_ratio.cos();
        let average_count = (T::from_float(0.6 * PI) / fixed_ratio).to_usize() + 2;
        let rot_dir = if radius >= T::from_float(0.0) {
            T::from_float(-1.0)
        } else {
            T::from_float(1.0)
        };

        Self {
            inv_ratio: T::from_float(1.0) / fixed_ratio,
            average_count,
            radius,
            limit_dot_product,
            rot_dir,
        }
    }
}
impl<P: FloatPointCompatible, I: IntNumber> JoinBuilder<P, I> for RoundJoinBuilder<P::Scalar> {
    fn add_join(
        &self,
        s0: &OffsetSection<P, I>,
        s1: &OffsetSection<P, I>,
        adapter: &FloatPointAdapter<P, I>,
        segments: &mut Vec<Segment<ShapeCountBoolean, I>>,
    ) {
        let dot_product = FloatPointMath::dot_product(&s0.dir, &s1.dir);
        if self.limit_dot_product < dot_product {
            BevelJoinBuilder::join(s0, s1, adapter, segments);
            return;
        }

        let angle = dot_product.acos();
        let n = (angle * self.inv_ratio).to_usize();
        let delta_angle = angle / P::Scalar::from_usize(n);

        let start = s0.b_top;
        let end = s1.a_top;

        let dir = P::from_xy(-s0.dir.y(), s0.dir.x());

        let rotator = Rotator::<P::Scalar>::with_angle(self.rot_dir * delta_angle);

        let center = adapter.int_to_float(&s0.b);
        let mut v = dir;
        let mut a = start;
        for _ in 1..n {
            v = rotator.rotate(&v);
            let p = FloatPointMath::add(&center, &FloatPointMath::scale(&v, self.radius));

            let b = adapter.float_to_int(&p);
            if a != b {
                segments.push(Segment::subject(a, b));
                a = b;
            }
        }

        if a != end {
            segments.push(Segment::subject(a, end));
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
