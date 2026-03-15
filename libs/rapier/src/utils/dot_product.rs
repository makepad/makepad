//! SimdDot and SimdLength traits for generalized dot product and length.

#[cfg(feature = "simd-is-enabled")]
use crate::math::SimdReal;
use crate::math::{Real, Vector};
use crate::utils::SimdRealCopy;
use na::{SimdRealField, Vector1, Vector2, Vector3};

/// Trait for computing generalized dot products.
pub trait DotProduct<Rhs>: Sized + Copy {
    /// The result type of the dot product.
    type Result: SimdRealField;
    /// Computes the generalized dot product of `self` with `rhs`.
    fn gdot(&self, rhs: Rhs) -> Self::Result;
}

/// Trait for computing generalized lengths.
pub trait SimdLength: DotProduct<Self> {
    /// Computes the SIMD length of this value.
    fn simd_length(&self) -> Self::Result {
        use crate::na::SimdComplexField;
        self.gdot(*self).simd_sqrt()
    }
}

impl<T: DotProduct<T>> SimdLength for T {}

impl<N: SimdRealCopy> DotProduct<Vector3<N>> for Vector3<N> {
    type Result = N;

    fn gdot(&self, rhs: Vector3<N>) -> Self::Result {
        self.x * rhs.x + self.y * rhs.y + self.z * rhs.z
    }
}

impl<N: SimdRealCopy> DotProduct<Vector2<N>> for Vector2<N> {
    type Result = N;

    fn gdot(&self, rhs: Vector2<N>) -> Self::Result {
        self.x * rhs.x + self.y * rhs.y
    }
}

impl<N: SimdRealCopy> DotProduct<Vector1<N>> for N {
    type Result = N;

    fn gdot(&self, rhs: Vector1<N>) -> Self::Result {
        *self * rhs.x
    }
}

impl DotProduct<Real> for Real {
    type Result = Real;

    fn gdot(&self, rhs: Real) -> Self::Result {
        *self * rhs
    }
}

#[cfg(feature = "simd-is-enabled")]
impl DotProduct<SimdReal> for SimdReal {
    type Result = SimdReal;

    fn gdot(&self, rhs: SimdReal) -> Self::Result {
        *self * rhs
    }
}

impl<N: SimdRealCopy> DotProduct<N> for Vector1<N> {
    type Result = N;

    fn gdot(&self, rhs: N) -> Self::Result {
        self.x * rhs
    }
}

// Glam implementations for concrete Vector type
impl DotProduct<Vector> for Vector {
    type Result = Real;

    fn gdot(&self, rhs: Vector) -> Self::Result {
        self.dot(rhs)
    }
}
