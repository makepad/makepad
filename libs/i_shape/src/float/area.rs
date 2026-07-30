use crate::base::data::{Contour, Shape};
use i_float::float::compatible::FloatPointCompatible;
use i_float::float::number::FloatNumber;

pub trait Area<P: FloatPointCompatible> {
    fn area(&self) -> P::Scalar;
}

impl<P: FloatPointCompatible> Area<P> for [P] {
    #[inline]
    fn area(&self) -> P::Scalar {
        let mut area = P::Scalar::from_float(0.0);

        let mut a = if let Some(p) = self.last() {
            *p
        } else {
            return area;
        };

        for &b in self.iter() {
            let ab = a.x() * b.y() - b.x() * a.y();
            area = area + ab;
            a = b;
        }

        P::Scalar::from_float(0.5) * area
    }
}

impl<P: FloatPointCompatible> Area<P> for [Contour<P>] {
    #[inline]
    fn area(&self) -> P::Scalar {
        let mut area = P::Scalar::from_float(0.0);
        for contour in self.iter() {
            area = area + contour.area();
        }
        area
    }
}

impl<P: FloatPointCompatible> Area<P> for [Shape<P>] {
    #[inline]
    fn area(&self) -> P::Scalar {
        let mut area = P::Scalar::from_float(0.0);
        for shape in self.iter() {
            area = area + shape.area();
        }
        area
    }
}

#[cfg(test)]
mod tests {
    use crate::float::area::Area;
    use alloc::vec;

    #[test]
    fn test_0() {
        let square = vec![[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]];

        let area = square.area();
        assert_eq!(area, 4.0);
    }
}
