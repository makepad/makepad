use crate::{Point, Transform, Transformation};
use std::ops::{Add, AddAssign, Div, DivAssign, Mul, MulAssign, Neg, Sub, SubAssign};

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

    /// Returns the zero vector.
    pub fn zero() -> Vector {
        Vector::new(0.0, 0.0)
    }

    /// Converts `self` to a point.
    ///
    /// This is equivalent to adding `self` to the origin.
    pub fn to_point(self) -> Point {
        Point::new(self.x, self.y)
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

    /// Non-uniformly scales `self` with the scale vector `v`.
    pub fn scale(self, v: Vector) -> Vector {
        Vector::new(self.x * v.x, self.y * v.y)
    }
}

impl AddAssign for Vector {
    fn add_assign(&mut self, other: Vector) {
        *self = *self + other
    }
}

impl SubAssign for Vector {
    fn sub_assign(&mut self, other: Vector) {
        *self = *self - other
    }
}

impl MulAssign<f32> for Vector {
    fn mul_assign(&mut self, k: f32) {
        *self = *self * k
    }
}

impl DivAssign<f32> for Vector {
    fn div_assign(&mut self, k: f32) {
        *self = *self / k
    }
}

impl Add for Vector {
    type Output = Vector;

    fn add(self, other: Vector) -> Vector {
        Vector::new(self.x + other.x, self.y + other.y)
    }
}

impl Sub for Vector {
    type Output = Vector;

    fn sub(self, other: Vector) -> Vector {
        Vector::new(self.x - other.x, self.y - other.y)
    }
}

impl Mul<f32> for Vector {
    type Output = Vector;

    fn mul(self, k: f32) -> Vector {
        Vector::new(self.x * k, self.y * k)
    }
}

impl Div<f32> for Vector {
    type Output = Vector;

    fn div(self, k: f32) -> Vector {
        Vector::new(self.x / k, self.y / k)
    }
}

impl Neg for Vector {
    type Output = Vector;

    fn neg(self) -> Vector {
        Vector::new(-self.x, -self.y)
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
