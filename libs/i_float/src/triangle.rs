use crate::int::number::int::IntNumber;
use crate::int::number::wide_int::WideIntNumber;
use crate::int::point::IntPoint;
use core::cmp::Ordering;

pub struct Triangle;

impl Triangle {
    #[inline(always)]
    pub fn area_two<T: IntNumber>(p0: IntPoint<T>, p1: IntPoint<T>, p2: IntPoint<T>) -> T::Wide {
        (p1 - p0).cross_product(p2 - p0)
    }

    #[inline(always)]
    pub fn is_clockwise<T: IntNumber>(p0: IntPoint<T>, p1: IntPoint<T>, p2: IntPoint<T>) -> bool {
        Self::area_two(p0, p1, p2) < T::Wide::ZERO
    }

    #[inline(always)]
    pub fn is_cw_or_line<T: IntNumber>(p0: IntPoint<T>, p1: IntPoint<T>, p2: IntPoint<T>) -> bool {
        Self::area_two(p0, p1, p2) <= T::Wide::ZERO
    }

    #[inline(always)]
    pub fn is_not_line<T: IntNumber>(p0: IntPoint<T>, p1: IntPoint<T>, p2: IntPoint<T>) -> bool {
        Self::area_two(p0, p1, p2) != T::Wide::ZERO
    }

    #[inline(always)]
    pub fn is_line<T: IntNumber>(p0: IntPoint<T>, p1: IntPoint<T>, p2: IntPoint<T>) -> bool {
        Self::area_two(p0, p1, p2) == T::Wide::ZERO
    }

    #[inline(always)]
    pub fn clock_direction<T: IntNumber>(p0: IntPoint<T>, p1: IntPoint<T>, p2: IntPoint<T>) -> T::Wide {
        Self::area_two(p0, p2, p1).signum()
    }

    #[inline]
    pub fn is_contain<T: IntNumber>(
        p: IntPoint<T>,
        p0: IntPoint<T>,
        p1: IntPoint<T>,
        p2: IntPoint<T>,
    ) -> bool {
        let q0 = (p - p1).cross_product(p0 - p1);
        let q1 = (p - p2).cross_product(p1 - p2);
        let q2 = (p - p0).cross_product(p2 - p0);

        let has_neg = q0 < T::Wide::ZERO || q1 < T::Wide::ZERO || q2 < T::Wide::ZERO;
        let has_pos = q0 > T::Wide::ZERO || q1 > T::Wide::ZERO || q2 > T::Wide::ZERO;

        !(has_neg && has_pos)
    }

    #[inline]
    pub fn is_not_contain<T: IntNumber>(
        p: IntPoint<T>,
        p0: IntPoint<T>,
        p1: IntPoint<T>,
        p2: IntPoint<T>,
    ) -> bool {
        let q0 = (p - p1).cross_product(p0 - p1);
        let q1 = (p - p2).cross_product(p1 - p2);
        let q2 = (p - p0).cross_product(p2 - p0);

        let has_neg = q0 <= T::Wide::ZERO || q1 <= T::Wide::ZERO || q2 <= T::Wide::ZERO;
        let has_pos = q0 >= T::Wide::ZERO || q1 >= T::Wide::ZERO || q2 >= T::Wide::ZERO;

        has_neg && has_pos
    }
    #[inline]
    pub fn is_contain_exclude_borders<T: IntNumber>(
        p: IntPoint<T>,
        p0: IntPoint<T>,
        p1: IntPoint<T>,
        p2: IntPoint<T>,
    ) -> bool {
        let q0 = (p - p1).cross_product(p0 - p1);
        let q1 = (p - p2).cross_product(p1 - p2);
        let q2 = (p - p0).cross_product(p2 - p0);

        let has_neg = q0 < T::Wide::ZERO || q1 < T::Wide::ZERO || q2 < T::Wide::ZERO;
        let has_pos = q0 > T::Wide::ZERO || q1 > T::Wide::ZERO || q2 > T::Wide::ZERO;

        !(has_neg && has_pos) && q0 != T::Wide::ZERO && q1 != T::Wide::ZERO && q2 != T::Wide::ZERO
    }

    #[inline(always)]
    pub fn clock_order<T: IntNumber>(p0: IntPoint<T>, p1: IntPoint<T>, p2: IntPoint<T>) -> Ordering {
        Self::area_two(p0, p1, p2).cmp(&T::Wide::ZERO)
    }
}
