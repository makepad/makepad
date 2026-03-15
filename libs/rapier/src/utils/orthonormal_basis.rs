//! SimdBasis trait for computing orthonormal bases.

#[cfg(feature = "dim3")]
use crate::math::Real;
use crate::math::Vector;
use crate::utils::{CopySign, SimdRealCopy};
use na::{Vector2, Vector3};

/// Trait to compute the orthonormal basis of a vector.
pub trait OrthonormalBasis: Sized {
    /// The type of the array of orthonormal vectors.
    type Basis;
    /// Computes the vectors which, when combined with `self`, form an orthonormal basis.
    fn orthonormal_basis(self) -> Self::Basis;
    /// Computes a vector orthogonal to `self` with a unit length (if `self` has a unit length).
    fn orthonormal_vector(self) -> Self;
}

impl<N: SimdRealCopy> OrthonormalBasis for Vector2<N> {
    type Basis = [Vector2<N>; 1];
    fn orthonormal_basis(self) -> [Vector2<N>; 1] {
        [Vector2::new(-self.y, self.x)]
    }
    fn orthonormal_vector(self) -> Vector2<N> {
        Vector2::new(-self.y, self.x)
    }
}

impl<N: SimdRealCopy + CopySign<N>> OrthonormalBasis for Vector3<N> {
    type Basis = [Vector3<N>; 2];
    // Robust and branchless implementation from Pixar:
    // https://graphics.pixar.com/library/OrthonormalB/paper.pdf
    fn orthonormal_basis(self) -> [Vector3<N>; 2] {
        let sign = self.z.copy_sign_to(N::one());
        let a = -N::one() / (sign + self.z);
        let b = self.x * self.y * a;

        [
            Vector3::new(
                N::one() + sign * self.x * self.x * a,
                sign * b,
                -sign * self.x,
            ),
            Vector3::new(b, sign + self.y * self.y * a, -self.y),
        ]
    }

    fn orthonormal_vector(self) -> Vector3<N> {
        let sign = self.z.copy_sign_to(N::one());
        let a = -N::one() / (sign + self.z);
        let b = self.x * self.y * a;
        Vector3::new(b, sign + self.y * self.y * a, -self.y)
    }
}

// Glam implementations for concrete Vector type
#[cfg(feature = "dim2")]
impl OrthonormalBasis for Vector {
    type Basis = [Vector; 1];
    fn orthonormal_basis(self) -> [Vector; 1] {
        [Vector::new(-self.y, self.x)]
    }
    fn orthonormal_vector(self) -> Vector {
        Vector::new(-self.y, self.x)
    }
}

#[cfg(feature = "dim3")]
impl OrthonormalBasis for Vector {
    type Basis = [Vector; 2];
    // Robust and branchless implementation from Pixar:
    // https://graphics.pixar.com/library/OrthonormalB/paper.pdf
    fn orthonormal_basis(self) -> [Vector; 2] {
        let sign = Real::copysign(1.0, self.z);
        let a = -1.0 / (sign + self.z);
        let b = self.x * self.y * a;

        [
            Vector::new(1.0 + sign * self.x * self.x * a, sign * b, -sign * self.x),
            Vector::new(b, sign + self.y * self.y * a, -self.y),
        ]
    }

    fn orthonormal_vector(self) -> Vector {
        let sign = Real::copysign(1.0, self.z);
        let a = -1.0 / (sign + self.z);
        let b = self.x * self.y * a;
        Vector::new(b, sign + self.y * self.y * a, -self.y)
    }
}
