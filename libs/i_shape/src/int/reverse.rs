use crate::int::shape::{IntShape, IntShapes};
use i_float::int::number::int::IntNumber;

pub trait IntContourReverse {
    fn reverse_contours(&mut self);
}

impl<I: IntNumber> IntContourReverse for IntShape<I> {
    #[inline]
    fn reverse_contours(&mut self) {
        for path in self {
            path.reverse()
        }
    }
}

impl<I: IntNumber> IntContourReverse for IntShapes<I> {
    #[inline]
    fn reverse_contours(&mut self) {
        for shape in self {
            for path in shape {
                path.reverse()
            }
        }
    }
}
