//! RotationOps trait for quaternion operations.

#[cfg(feature = "dim3")]
use crate::math::Mat3;
#[cfg(feature = "simd-is-enabled")]
use crate::math::SimdReal;
use crate::math::{Matrix, Real, Rotation, Vector};
#[cfg(feature = "dim3")]
use crate::utils::CrossProductMatrix;
use crate::utils::ScalarType;
use core::fmt::Debug;
use core::ops::Mul;
#[cfg(all(feature = "dim3", feature = "simd-is-enabled"))]
use na::{Matrix3, UnitQuaternion};

/// Trait implemented by quaternions.
#[cfg(feature = "dim3")]
pub trait RotationOps<N: ScalarType>:
    Copy + Debug + Mul<N::Rotation, Output = N::Rotation> + Mul<N::Vector, Output = N::Vector>
{
    /// Converts this rotation to a rotation matrix.
    fn to_mat(self) -> N::Matrix;
    /// Returns the inverse of this rotation.
    fn inverse(self) -> Self;
    /// Compute the differential of `inv(q1) * q2`.
    fn diff_conj1_2(&self, rhs: &Self) -> N::Matrix;
    /// Compute the transposed differential of `inv(q1) * q2`.
    fn diff_conj1_2_tr(&self, rhs: &Self) -> N::Matrix;
    /// Compute the dot product of two quaternions.
    fn dot(&self, rhs: &Self) -> N;
    /// The imaginary part of the quaternion.
    fn imag(&self) -> N::Vector;
    /// Multiply this quaternion by a scalar without renormalizing.
    fn mul_assign_unchecked(&mut self, rhs: N);
}

/// Trait implemented by 2D rotation types (UnitComplex).
#[cfg(feature = "dim2")]
pub trait RotationOps<N: ScalarType>:
    Copy + Debug + Mul<N::Rotation, Output = N::Rotation> + Mul<N::Vector, Output = N::Vector>
{
    /// Converts this rotation to a rotation matrix.
    fn to_mat(self) -> N::Matrix;
    /// Returns the inverse of this rotation.
    fn inverse(self) -> Self;
    /// The imaginary part of the complex rotation.
    fn imag(&self) -> N;
    /// The angle of the rotation.
    fn angle(&self) -> N;
}

#[cfg(all(feature = "dim3", feature = "simd-is-enabled"))]
impl RotationOps<SimdReal> for UnitQuaternion<SimdReal> {
    #[inline]
    fn to_mat(self) -> na::Matrix3<SimdReal> {
        self.to_rotation_matrix().into_inner()
    }

    #[inline]
    fn inverse(self) -> Self {
        self.conjugate()
    }

    #[inline]
    fn diff_conj1_2(&self, rhs: &Self) -> Matrix3<SimdReal> {
        use crate::na::SimdValue;
        let half = SimdReal::splat(0.5);
        let v1 = self.imag();
        let v2 = rhs.imag();
        let w1 = self.w;
        let w2 = rhs.w;

        // TODO: this can probably be optimized a lot by unrolling the ops.
        (v1 * v2.transpose() + Matrix3::from_diagonal_element(w1 * w2)
            - (v1 * w2 + v2 * w1).cross_matrix()
            + v1.cross_matrix() * v2.cross_matrix())
            * half
    }

    #[inline]
    fn diff_conj1_2_tr(&self, rhs: &Self) -> Matrix3<SimdReal> {
        self.diff_conj1_2(rhs).transpose()
    }

    #[inline]
    fn dot(&self, rhs: &Self) -> SimdReal {
        self.coords.dot(&rhs.coords)
    }

    #[inline]
    fn imag(&self) -> na::Vector3<SimdReal> {
        (**self).imag()
    }

    #[inline]
    fn mul_assign_unchecked(&mut self, rhs: SimdReal) {
        *self.as_mut_unchecked() *= rhs;
    }
}

#[cfg(feature = "dim3")]
impl RotationOps<Real> for Rotation {
    #[inline]
    fn to_mat(self) -> Mat3 {
        Matrix::from_quat(self)
    }

    #[inline]
    fn inverse(self) -> Self {
        self.inverse()
    }

    fn diff_conj1_2(&self, rhs: &Self) -> Mat3 {
        use parry::math::VectorExt;

        let half = 0.5;
        let v1 = self.xyz();
        let v2 = rhs.xyz();
        let w1 = self.w;
        let w2 = rhs.w;

        // TODO: this can probably be optimized a lot by unrolling the ops.
        (v1.kronecker(v2) + Matrix::from_diagonal(Vector::splat(w1 * w2))
            - (v1 * w2 + v2 * w1).gcross_matrix()
            + v1.gcross_matrix() * v2.gcross_matrix())
            * half
    }

    #[inline]
    fn diff_conj1_2_tr(&self, rhs: &Self) -> Mat3 {
        self.diff_conj1_2(rhs).transpose()
    }

    #[inline]
    fn dot(&self, rhs: &Self) -> Real {
        (*self).dot(*rhs)
    }

    #[inline]
    fn imag(&self) -> Vector {
        self.xyz()
    }

    #[inline]
    fn mul_assign_unchecked(&mut self, rhs: Real) {
        *self *= rhs;
    }
}

#[cfg(feature = "dim2")]
impl RotationOps<Real> for Rotation {
    #[inline]
    fn to_mat(self) -> Matrix {
        Matrix::from_cols(
            Vector::new(self.re, self.im),
            Vector::new(-self.im, self.re),
        )
    }

    #[inline]
    fn inverse(self) -> Self {
        self.inverse()
    }

    #[inline]
    fn imag(&self) -> Real {
        self.im
    }

    #[inline]
    fn angle(&self) -> Real {
        (*self).angle()
    }
}

#[cfg(all(feature = "dim2", feature = "simd-is-enabled"))]
impl RotationOps<SimdReal> for na::UnitComplex<SimdReal> {
    #[inline]
    fn to_mat(self) -> na::Matrix2<SimdReal> {
        self.to_rotation_matrix().into_inner()
    }

    #[inline]
    fn inverse(self) -> Self {
        self.conjugate()
    }

    #[inline]
    fn imag(&self) -> SimdReal {
        self.im
    }

    #[inline]
    fn angle(&self) -> SimdReal {
        (*self).angle()
    }
}
