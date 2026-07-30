use crate::base::data::Shape;

pub trait ContourReverse {
    fn reverse_contours(&mut self);
}

impl<P> ContourReverse for Shape<P> {
    #[inline]
    fn reverse_contours(&mut self) {
        for path in self {
            path.reverse()
        }
    }
}
