use crate::base::data::Contour;
use crate::int::shape::IntContour;
use crate::util::reserve::Reserve;
use i_float::adapter::FloatPointAdapter;
use i_float::float::compatible::FloatPointCompatible;
use i_float::int::number::int::IntNumber;

pub trait IntContourInit<P: FloatPointCompatible, I: IntNumber> {
    fn set_with_float(&mut self, contour: &Contour<P>, adapter: &FloatPointAdapter<P, I>);
}

impl<P: FloatPointCompatible, I: IntNumber> IntContourInit<P, I> for IntContour<I> {
    fn set_with_float(&mut self, contour: &Contour<P>, adapter: &FloatPointAdapter<P, I>) {
        self.reserve_capacity(contour.len());
        self.clear();
        for p in contour.iter() {
            self.push(adapter.float_to_int(p))
        }
    }
}
