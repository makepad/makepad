use crate::base::data::{Contour, Shape, Shapes};
use crate::flat::buffer::FlatContoursBuffer;
use crate::flat::float::FloatFlatContoursBuffer;
use crate::float::adapter::{
    BufferToFloat, BufferToInt, PathToFloat, PathToInt, ShapeToFloat, ShapeToInt, ShapesToFloat, ShapesToInt,
};
use crate::int::despike::DeSpike;
use i_float::adapter::FloatPointAdapter;
use i_float::float::compatible::FloatPointCompatible;
use i_float::int::number::int::IntNumber;

/// A trait that provides methods for despike complex geometrical structures.
pub trait DeSpikeContour<P: FloatPointCompatible, I: IntNumber> {
    /// Simplifies the structure in-place if it is not already simple.
    ///
    /// # Returns
    ///
    /// - `true` if the structure was simplified successfully.
    /// - `false` if the structure was already simple and no modification was made.
    fn despike_contour(&mut self, adapter: &FloatPointAdapter<P, I>) -> bool;
}

impl<P: FloatPointCompatible, I: IntNumber> DeSpikeContour<P, I> for Contour<P> {
    fn despike_contour(&mut self, adapter: &FloatPointAdapter<P, I>) -> bool {
        let mut int_contour = self.to_int(adapter);
        if !int_contour.remove_spikes() {
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

impl<P: FloatPointCompatible, I: IntNumber> DeSpikeContour<P, I> for Shape<P> {
    fn despike_contour(&mut self, adapter: &FloatPointAdapter<P, I>) -> bool {
        let mut int_shape = self.to_int(adapter);
        if !int_shape.remove_spikes() {
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

impl<P: FloatPointCompatible, I: IntNumber> DeSpikeContour<P, I> for Shapes<P> {
    fn despike_contour(&mut self, adapter: &FloatPointAdapter<P, I>) -> bool {
        let mut int_shapes = self.to_int(adapter);
        if !int_shapes.remove_spikes() {
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

impl<P: FloatPointCompatible, I: IntNumber> DeSpikeContour<P, I> for FloatFlatContoursBuffer<P> {
    fn despike_contour(&mut self, adapter: &FloatPointAdapter<P, I>) -> bool {
        let int_buffer = self.to_int(adapter);
        let mut out = FlatContoursBuffer::with_capacity(int_buffer.points.len());
        let mut changed = false;

        for mut contour in int_buffer.to_contours().into_iter() {
            changed |= contour.remove_spikes();
            if !contour.is_empty() {
                out.add_contour(&contour);
            }
        }

        if !changed {
            return false;
        }

        *self = out.to_float(adapter);
        true
    }
}
