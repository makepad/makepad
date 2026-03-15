//! SimdCross trait for generalized cross product.

use crate::math::{Real, Vector};
use na::{SimdRealField, Vector2, Vector3};

/// Trait for computing generalized cross products.
pub trait CrossProduct<Rhs>: Sized {
    /// The result type of the cross product.
    type Result;
    /// Computes the generalized cross product of `self` with `rhs`.
    fn gcross(&self, rhs: Rhs) -> Self::Result;
}

impl<T: SimdRealField + Copy> CrossProduct<Vector3<T>> for Vector3<T> {
    type Result = Self;

    fn gcross(&self, rhs: Vector3<T>) -> Self::Result {
        self.cross(&rhs)
    }
}

impl<T: SimdRealField + Copy> CrossProduct<Vector2<T>> for Vector2<T> {
    type Result = T;

    fn gcross(&self, rhs: Vector2<T>) -> Self::Result {
        self.x * rhs.y - self.y * rhs.x
    }
}

impl<T: SimdRealField + Copy> CrossProduct<Vector2<T>> for T {
    type Result = Vector2<T>;

    fn gcross(&self, rhs: Vector2<T>) -> Self::Result {
        Vector2::new(-rhs.y * *self, rhs.x * *self)
    }
}

impl<T: SimdRealField + Copy> CrossProduct<Vector3<T>> for T {
    type Result = Vector3<T>;

    fn gcross(&self, _rhs: Vector3<T>) -> Self::Result {
        unreachable!()
    }
}

#[cfg(feature = "dim3")]
impl CrossProduct<Vector> for Real {
    type Result = Vector;

    fn gcross(&self, _rhs: Vector) -> Self::Result {
        todo!("This isn't really used by rapier")
    }
}

// Glam implementations for concrete Vector type
#[cfg(feature = "dim3")]
impl CrossProduct<Vector> for Vector {
    type Result = Vector;

    fn gcross(&self, rhs: Vector) -> Self::Result {
        self.cross(rhs)
    }
}

#[cfg(feature = "dim2")]
impl CrossProduct<Vector> for Vector {
    type Result = Real;

    fn gcross(&self, rhs: Vector) -> Self::Result {
        self.x * rhs.y - self.y * rhs.x
    }
}

#[cfg(feature = "dim2")]
impl CrossProduct<Vector> for Real {
    type Result = Vector;

    fn gcross(&self, rhs: Vector) -> Self::Result {
        Vector::new(-rhs.y * *self, rhs.x * *self)
    }
}
