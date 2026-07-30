use crate::int::path::ContourExtension;
use crate::int::shape::{IntContour, IntShape};
use i_float::int::number::int::IntNumber;
use i_float::int::number::wide_int::WideIntNumber;
use i_float::int::point::IntPoint;

pub trait Area<I: IntNumber> {
    fn area_two(&self) -> I::Wide;
    fn area(&self) -> I::Wide;
}

impl<I: IntNumber> Area<I> for [IntPoint<I>] {
    #[inline]
    fn area_two(&self) -> I::Wide {
        self.unsafe_area()
    }

    #[inline]
    fn area(&self) -> I::Wide {
        self.area_two() / I::Wide::TWO
    }
}

impl<I: IntNumber> Area<I> for [IntContour<I>] {
    #[inline]
    fn area_two(&self) -> I::Wide {
        let mut s = I::Wide::ZERO;
        for path in self.iter() {
            s = s.wrapping_add(path.area_two())
        }
        s
    }

    #[inline]
    fn area(&self) -> I::Wide {
        self.area_two() / I::Wide::TWO
    }
}

impl<I: IntNumber> Area<I> for [IntShape<I>] {
    #[inline]
    fn area_two(&self) -> I::Wide {
        let mut s = I::Wide::ZERO;
        for shape in self.iter() {
            s = s.wrapping_add(shape.area_two())
        }
        s
    }

    #[inline]
    fn area(&self) -> I::Wide {
        self.area_two() / I::Wide::TWO
    }
}

#[cfg(test)]
mod tests {
    use crate::int::area::Area;
    use crate::int_path;

    #[test]
    fn test_0() {
        let square = int_path![[-1, -1], [1, -1], [1, 1], [-1, 1]];

        let area = square.area_two();
        assert_eq!(area, 8i64);
    }
}
