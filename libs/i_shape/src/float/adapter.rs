use crate::base::data::{Contour, Path, Shape, Shapes};
use crate::flat::buffer::FlatContoursBuffer;
use crate::flat::float::FloatFlatContoursBuffer;
use crate::int::path::IntPath;
use crate::int::shape::{IntContour, IntShape, IntShapes};
use i_float::adapter::FloatPointAdapter;
use i_float::float::compatible::FloatPointCompatible;
use i_float::int::number::int::IntNumber;
use i_float::int::point::IntPoint;

pub trait PathToFloat<P: FloatPointCompatible, I: IntNumber> {
    fn to_float(&self, adapter: &FloatPointAdapter<P, I>) -> Path<P>;
}

pub trait ShapeToFloat<P: FloatPointCompatible, I: IntNumber> {
    fn to_float(&self, adapter: &FloatPointAdapter<P, I>) -> Shape<P>;
}

pub trait ShapesToFloat<P: FloatPointCompatible, I: IntNumber> {
    fn to_float(&self, adapter: &FloatPointAdapter<P, I>) -> Shapes<P>;
}

pub trait BufferToFloat<P: FloatPointCompatible, I: IntNumber> {
    fn to_float(&self, adapter: &FloatPointAdapter<P, I>) -> FloatFlatContoursBuffer<P>;
}

pub trait PathToInt<P: FloatPointCompatible, I: IntNumber> {
    fn to_int(&self, adapter: &FloatPointAdapter<P, I>) -> IntPath<I>;
}

pub trait ShapeToInt<P: FloatPointCompatible, I: IntNumber> {
    fn to_int(&self, adapter: &FloatPointAdapter<P, I>) -> IntShape<I>;
}

pub trait ShapesToInt<P: FloatPointCompatible, I: IntNumber> {
    fn to_int(&self, adapter: &FloatPointAdapter<P, I>) -> IntShapes<I>;
}

pub trait BufferToInt<P: FloatPointCompatible, I: IntNumber> {
    fn to_int(&self, adapter: &FloatPointAdapter<P, I>) -> FlatContoursBuffer<I>;
}

impl<P: FloatPointCompatible, I: IntNumber> PathToFloat<P, I> for [IntPoint<I>] {
    #[inline(always)]
    fn to_float(&self, adapter: &FloatPointAdapter<P, I>) -> Path<P> {
        self.iter().map(|p| adapter.int_to_float(p)).collect()
    }
}

impl<P: FloatPointCompatible, I: IntNumber> ShapeToFloat<P, I> for [IntContour<I>] {
    #[inline(always)]
    fn to_float(&self, adapter: &FloatPointAdapter<P, I>) -> Shape<P> {
        self.iter().map(|path| path.to_float(adapter)).collect()
    }
}

impl<P: FloatPointCompatible, I: IntNumber> ShapesToFloat<P, I> for [IntShape<I>] {
    #[inline(always)]
    fn to_float(&self, adapter: &FloatPointAdapter<P, I>) -> Shapes<P> {
        self.iter().map(|shape| shape.to_float(adapter)).collect()
    }
}

impl<P: FloatPointCompatible, I: IntNumber> BufferToFloat<P, I> for FlatContoursBuffer<I> {
    #[inline(always)]
    fn to_float(&self, adapter: &FloatPointAdapter<P, I>) -> FloatFlatContoursBuffer<P> {
        FloatFlatContoursBuffer {
            points: self.points.to_float(adapter),
            ranges: self.ranges.clone(),
        }
    }
}

impl<P: FloatPointCompatible, I: IntNumber> PathToInt<P, I> for [P] {
    #[inline(always)]
    fn to_int(&self, adapter: &FloatPointAdapter<P, I>) -> IntPath<I> {
        self.iter().map(|p| adapter.float_to_int(p)).collect()
    }
}

impl<P: FloatPointCompatible, I: IntNumber> ShapeToInt<P, I> for [Contour<P>] {
    #[inline(always)]
    fn to_int(&self, adapter: &FloatPointAdapter<P, I>) -> IntShape<I> {
        self.iter().map(|path| path.to_int(adapter)).collect()
    }
}

impl<P: FloatPointCompatible, I: IntNumber> ShapesToInt<P, I> for [Shape<P>] {
    #[inline(always)]
    fn to_int(&self, adapter: &FloatPointAdapter<P, I>) -> IntShapes<I> {
        self.iter().map(|shape| shape.to_int(adapter)).collect()
    }
}

impl<P: FloatPointCompatible, I: IntNumber> BufferToInt<P, I> for FloatFlatContoursBuffer<P> {
    #[inline(always)]
    fn to_int(&self, adapter: &FloatPointAdapter<P, I>) -> FlatContoursBuffer<I> {
        FlatContoursBuffer {
            points: self.points.to_int(adapter),
            ranges: self.ranges.clone(),
        }
    }
}
