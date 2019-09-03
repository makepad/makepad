use crate::{F32Ext, Transform, Transformation, Vector};
use std::ops::Sub;

/// A point in 2-dimensional Euclidian space.
/// 
/// A point represents a position, whereas a vector represents a displacement. That is, the result
/// of subtracting two points is a vector. Moreover, the result of adding/subtracting a vector
/// to/from a point is another point. However, adding two points is not defined. Similarly, whereas
/// a point can be scaled, rotated, and translated, a vector can only be scaled and rotated.
#[derive(Clone, Copy, Debug, Default, PartialEq, PartialOrd)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    /// Creates a new point with the given coordinates.
    pub fn new(x: f32, y: f32) -> Point {
        Point {x, y}
    }

    /// Linearly interpolate between `self` and `other` with parameter `t`.
    pub fn lerp(self, other: Point, t: f32) -> Point {
        Point::new(self.x.lerp(other.x, t), self.y.lerp(other.y, t))
    }
}

impl Sub for Point {
    type Output = Vector;

    fn sub(self, other: Point) -> Vector {
        Vector::new(self.x - other.x, self.y - other.y)
    }
}

impl Transform for Point {
    fn transform<T>(self, t: &T) -> Point
    where
        T: Transformation,
    {
        t.transform_point(self)
    }

    fn transform_mut<T>(&mut self, t: &T)
    where
        T: Transformation,
    {
        *self = self.transform(t);
    }
}