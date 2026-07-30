use crate::base::data::{Contour, Shape, Shapes};
use crate::flat::float::FloatFlatContoursBuffer;
use crate::float::adapter::{
    BufferToInt, PathToFloat, PathToInt, ShapeToFloat, ShapeToInt, ShapesToFloat, ShapesToInt,
};
use crate::int::simple::Simplify as IntSimplify;
use i_float::adapter::FloatPointAdapter;
use i_float::float::compatible::FloatPointCompatible;
use i_float::int::number::int::IntNumber;

/// A trait that provides methods for simplifying complex geometrical structures.
pub trait SimplifyContour<P: FloatPointCompatible, I: IntNumber> {
    /// Simplifies the structure in-place if it is not already simple.
    ///
    /// # Returns
    ///
    /// - `true` if the structure was simplified successfully.
    /// - `false` if the structure was already simple and no modification was made.
    fn simplify_contour(&mut self, adapter: &FloatPointAdapter<P, I>) -> bool;
}

impl<P: FloatPointCompatible, I: IntNumber> SimplifyContour<P, I> for Contour<P> {
    fn simplify_contour(&mut self, adapter: &FloatPointAdapter<P, I>) -> bool {
        let mut int_contour = self.to_int(adapter);
        if !int_contour.simplify_contour() {
            return false;
        }

        if int_contour.is_empty() {
            self.clear();
        } else {
            *self = int_contour.to_float(adapter);
        }
        true
    }
}

impl<P: FloatPointCompatible, I: IntNumber> SimplifyContour<P, I> for Shape<P> {
    fn simplify_contour(&mut self, adapter: &FloatPointAdapter<P, I>) -> bool {
        let mut int_shape = self.to_int(adapter);
        if !int_shape.simplify_contour() {
            return false;
        }

        if int_shape.is_empty() {
            self.clear();
        } else {
            *self = int_shape.to_float(adapter);
        }
        true
    }
}

impl<P: FloatPointCompatible, I: IntNumber> SimplifyContour<P, I> for Shapes<P> {
    fn simplify_contour(&mut self, adapter: &FloatPointAdapter<P, I>) -> bool {
        let mut int_shapes = self.to_int(adapter);
        if !int_shapes.simplify_contour() {
            return false;
        }

        if int_shapes.is_empty() {
            self.clear();
        } else {
            *self = int_shapes.to_float(adapter);
        }
        true
    }
}

impl<P: FloatPointCompatible, I: IntNumber> SimplifyContour<P, I> for FloatFlatContoursBuffer<P> {
    fn simplify_contour(&mut self, adapter: &FloatPointAdapter<P, I>) -> bool {
        let int_buffer = self.to_int(adapter);
        self.clear_and_reserve(int_buffer.ranges.len(), int_buffer.ranges.len());
        let mut changed = false;

        for mut contour in int_buffer.to_contours().into_iter() {
            changed |= contour.simplify_contour();
            if !contour.is_empty() {
                self.add_contour_iter(contour.iter().map(|p| adapter.int_to_float(p)));
            }
        }

        if !changed {
            return false;
        }
        true
    }
}
