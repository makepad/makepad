//! Miscellaneous utilities.

use crate::math::{Real, Vector2, Vector3};

#[cfg(feature = "simd-is-enabled")]
use {
    crate::math::{SimdBool, SimdReal},
    simba::simd::SimdValue,
};

#[cfg(feature = "simd-is-enabled")]
#[allow(dead_code)]
/// Conditionally swaps each lanes of `a` with those of `b`.
///
/// For each `i in [0..SIMD_WIDTH[`, if `do_swap.extract(i)` is `true` then
/// `a.extract(i)` is swapped with `b.extract(i)`.
pub fn simd_swap(do_swap: SimdBool, a: &mut SimdReal, b: &mut SimdReal) {
    let _a = *a;
    *a = b.select(do_swap, *a);
    *b = _a.select(do_swap, *b);
}

/// Trait to copy the sign of each component of one scalar/vector/matrix to another.
pub trait WSign<Rhs>: Sized {
    // See SIMD implementations of copy_sign there: https://stackoverflow.com/a/57872652
    /// Copy the sign of each component of `self` to the corresponding component of `to`.
    fn copy_sign_to(self, to: Rhs) -> Rhs;
}

impl WSign<Real> for Real {
    fn copy_sign_to(self, to: Self) -> Self {
        let minus_zero: Real = -0.0;
        let signbit = minus_zero.to_bits();
        Real::from_bits((signbit & self.to_bits()) | ((!signbit) & to.to_bits()))
    }
}

impl WSign<Vector2> for Real {
    fn copy_sign_to(self, to: Vector2) -> Vector2 {
        Vector2::new(self.copy_sign_to(to.x), self.copy_sign_to(to.y))
    }
}

impl WSign<Vector3> for Real {
    fn copy_sign_to(self, to: Vector3) -> Vector3 {
        Vector3::new(
            self.copy_sign_to(to.x),
            self.copy_sign_to(to.y),
            self.copy_sign_to(to.z),
        )
    }
}

impl WSign<Vector2> for Vector2 {
    fn copy_sign_to(self, to: Vector2) -> Vector2 {
        Vector2::new(self.x.copy_sign_to(to.x), self.y.copy_sign_to(to.y))
    }
}

impl WSign<Vector3> for Vector3 {
    fn copy_sign_to(self, to: Vector3) -> Vector3 {
        Vector3::new(
            self.x.copy_sign_to(to.x),
            self.y.copy_sign_to(to.y),
            self.z.copy_sign_to(to.z),
        )
    }
}

#[cfg(feature = "simd-is-enabled")]
impl WSign<SimdReal> for SimdReal {
    fn copy_sign_to(self, to: SimdReal) -> SimdReal {
        use simba::simd::SimdRealField;
        to.simd_copysign(self)
    }
}

/// Trait to compute the orthonormal basis of a vector.
pub trait WBasis: Sized {
    /// The type of the array of orthonormal vectors.
    type Basis;
    /// Computes the vectors which, when combined with `self`, form an orthonormal basis.
    fn orthonormal_basis(self) -> Self::Basis;
}

impl WBasis for Vector2 {
    type Basis = [Vector2; 1];
    fn orthonormal_basis(self) -> [Vector2; 1] {
        [Vector2::new(-self.y, self.x)]
    }
}

impl WBasis for Vector3 {
    type Basis = [Vector3; 2];
    // Robust and branchless implementation from Pixar:
    // https://graphics.pixar.com/library/OrthonormalB/paper.pdf
    fn orthonormal_basis(self) -> [Vector3; 2] {
        let sign = self.z.copy_sign_to(1.0);
        let a = -1.0 / (sign + self.z);
        let b = self.x * self.y * a;

        [
            Vector3::new(1.0 + sign * self.x * self.x * a, sign * b, -sign * self.x),
            Vector3::new(b, sign + self.y * self.y * a, -self.y),
        ]
    }
}

pub(crate) trait WCross<Rhs>: Sized {
    type Result;
    fn gcross(&self, rhs: Rhs) -> Self::Result;
}

impl WCross<Vector3> for Vector3 {
    type Result = Self;

    fn gcross(&self, rhs: Vector3) -> Self::Result {
        self.cross(rhs)
    }
}

impl WCross<Vector2> for Vector2 {
    type Result = Real;

    fn gcross(&self, rhs: Vector2) -> Self::Result {
        self.x * rhs.y - self.y * rhs.x
    }
}

impl WCross<Vector2> for Real {
    type Result = Vector2;

    fn gcross(&self, rhs: Vector2) -> Self::Result {
        Vector2::new(-rhs.y * *self, rhs.x * *self)
    }
}
