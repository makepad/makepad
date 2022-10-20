use crate::{Point, Vector};

/// A trait for transformations in 2-dimensional Euclidian space.
pub trait Transformation {
    /// Applies `self` to the given `point`.
    fn transform_point(&self, point: Point) -> Point;

    /// Applies `self` to the given `vector`.
    fn transform_vector(&self, vector: Vector) -> Vector;
}
