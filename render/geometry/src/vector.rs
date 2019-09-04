use crate::{Transform, Transformation};
use std::ops::Div;

/// A vector in 2-dimensional Euclidian space.
///
/// A point represents a position, whereas a vector represents a displacement. That is, the result
/// of subtracting two points is a vector. Moreover, the result of adding/subtracting a vector
/// to/from a point is another point. However, adding two points is not defined. Similarly, whereas
/// a point can be scaled, rotated, and translated, a vector can only be scaled and rotated.
#[derive(Clone, Copy, Debug, Default, PartialEq, PartialOrd)]
pub struct Vector {
    pub x: f32,
    pub y: f32,
}

impl Vector {
    /// Creates a new vector with the given coordinates.
    pub fn new(x: f32, y: f32) -> Vector {
        Vector { x, y }
    }

    /// Returns the length of `self`.
    pub fn length(self) -> f32 {
        // `hypot` is more numerically stable than using `sqrt`. See:
        // https://en.wikipedia.org/wiki/Hypot
        self.x.hypot(self.y)
    }

    /// Returns the unit vector in the direction of `self`, or `None` if `self` is the zero vector.
    pub fn normalize(self) -> Option<Vector> {
        let length = self.length();
        if length == 0.0 {
            None
        } else {
            Some(self / length)
        }
    }

    /// Returns the dot product of `self` and `other`.
    pub fn dot(self, other: Vector) -> f32 {
        self.x * other.x + self.y * other.y
    }

    /// Returns the cross product of `self` and `other`.
    pub fn cross(self, other: Vector) -> f32 {
        self.x * other.y - self.y * other.x
    }
}

impl Div<f32> for Vector {
    type Output = Vector;

    fn div(self, k: f32) -> Vector {
        Vector::new(self.x / k, self.y / k)
    }
}

impl Transform for Vector {
    fn transform<T>(self, t: &T) -> Vector
    where
        T: Transformation,
    {
        t.transform_vector(self)
    }

    fn transform_mut<T>(&mut self, t: &T)
    where
        T: Transformation,
    {
        *self = self.transform(t);
    }
}
