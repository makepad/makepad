use crate::geom::x_segment::XSegment;
use core::cmp::Ordering;
use i_float::int::number::int::IntNumber;
use i_float::int::number::wide_int::WideIntNumber;
use i_float::int::point::IntPoint;
use i_float::triangle::Triangle;
use i_tree::{Expiration, ExpiredKey};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct VSegment<I: IntNumber> {
    pub(crate) a: IntPoint<I>,
    pub(crate) b: IntPoint<I>,
}

impl<I: IntNumber> VSegment<I> {
    #[inline(always)]
    fn is_under_segment_order(&self, other: &VSegment<I>) -> Ordering {
        match self.a.cmp(&other.a) {
            Ordering::Less => Triangle::clock_order(self.a, other.a, self.b),
            Ordering::Equal => Triangle::clock_order(self.a, other.b, self.b),
            Ordering::Greater => Triangle::clock_order(other.a, other.b, self.a),
        }
    }

    #[inline(always)]
    pub(crate) fn is_under_point_order(&self, p: IntPoint<I>) -> Ordering {
        debug_assert!(self.a.x <= p.x && p.x <= self.b.x);
        debug_assert!(p != self.a && p != self.b);

        Triangle::clock_order(self.a, p, self.b)
    }

    #[inline(always)]
    pub(crate) fn is_under_segment(&self, other: &VSegment<I>) -> bool {
        match self.a.cmp(&other.a) {
            Ordering::Less => Triangle::is_clockwise(self.a, other.a, self.b),
            Ordering::Equal => Triangle::is_clockwise(self.a, other.b, self.b),
            Ordering::Greater => Triangle::is_clockwise(other.a, other.b, self.a),
        }
    }

    #[inline(always)]
    pub(crate) fn cmp_by_angle(&self, other: &Self) -> Ordering {
        // sort angles counterclockwise
        // debug_assert!(self.a == other.a);
        let v0 = self.b - self.a;
        let v1 = other.b - other.a;
        let cross = v0.cross_product(v1);
        I::Wide::ZERO.cmp(&cross)
    }
}

pub(crate) trait BottomSegment<I: IntNumber> {
    fn update_if_under(&mut self, segment: VSegment<I>);
}

impl<I: IntNumber> BottomSegment<I> for Option<VSegment<I>> {
    #[inline(always)]
    fn update_if_under(&mut self, segment: VSegment<I>) {
        if let Some(best) = self {
            if segment.is_under_segment(&best) {
                *best = segment
            }
        } else {
            *self = Some(segment);
        }
    }
}

impl<I: IntNumber> From<XSegment<I>> for VSegment<I> {
    #[inline(always)]
    fn from(seg: XSegment<I>) -> Self {
        VSegment { a: seg.a, b: seg.b }
    }
}

impl<I: IntNumber> PartialOrd<Self> for VSegment<I> {
    #[inline(always)]
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<I: IntNumber> Ord for VSegment<I> {
    #[inline(always)]
    fn cmp(&self, other: &Self) -> Ordering {
        self.is_under_segment_order(other)
    }
}

impl<I: IntNumber + Expiration> ExpiredKey<I> for VSegment<I> {
    #[inline]
    fn expiration(&self) -> I {
        self.b.x
    }
}

#[cfg(test)]
mod tests {
    use crate::geom::v_segment::VSegment;
    use core::cmp::Ordering;
    use i_float::int::point::IntPoint;

    #[test]
    fn test_00() {
        let p = IntPoint::new(-10, 10);
        let s = VSegment {
            a: IntPoint::new(-10, -10),
            b: IntPoint::new(10, -10),
        };
        let order = s.is_under_point_order(p);
        assert_eq!(order, Ordering::Less);
    }
}
