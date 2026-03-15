//! SimdAngularInertia trait for angular inertia operations.

#[cfg(feature = "simd-is-enabled")]
use crate::math::SimdReal;
#[cfg(feature = "dim3")]
use crate::math::{Matrix, Real, Vector};
use crate::utils::{SimdRealCopy, simd_inv};
#[cfg(feature = "dim3")]
use parry::utils::SdpMatrix3;

/// Trait for angular inertia operations.
pub trait AngularInertiaOps<N>: Copy + core::fmt::Debug + Default {
    /// The angular vector type.
    type AngVector;
    /// The angular matrix type.
    type AngMatrix;
    /// Returns the inverse of this angular inertia.
    fn inverse(&self) -> Self;
    /// Transforms a vector by this angular inertia.
    fn transform_vector(&self, pt: Self::AngVector) -> Self::AngVector;
    /// Converts this angular inertia into a matrix.
    fn into_matrix(self) -> Self::AngMatrix;
}

impl<N: SimdRealCopy + Default> AngularInertiaOps<N> for N {
    type AngVector = N;
    type AngMatrix = N;

    fn inverse(&self) -> Self {
        simd_inv(*self)
    }

    fn transform_vector(&self, pt: N) -> N {
        pt * *self
    }

    fn into_matrix(self) -> Self::AngMatrix {
        self
    }
}

#[cfg(feature = "dim3")]
impl AngularInertiaOps<Real> for SdpMatrix3<Real> {
    type AngVector = Vector;
    type AngMatrix = Matrix;

    #[inline]
    fn inverse(&self) -> Self {
        let minor_m12_m23 = self.m22 * self.m33 - self.m23 * self.m23;
        let minor_m11_m23 = self.m12 * self.m33 - self.m13 * self.m23;
        let minor_m11_m22 = self.m12 * self.m23 - self.m13 * self.m22;

        let determinant =
            self.m11 * minor_m12_m23 - self.m12 * minor_m11_m23 + self.m13 * minor_m11_m22;

        if determinant == 0.0 {
            Self::zero()
        } else {
            SdpMatrix3 {
                m11: minor_m12_m23 / determinant,
                m12: -minor_m11_m23 / determinant,
                m13: minor_m11_m22 / determinant,
                m22: (self.m11 * self.m33 - self.m13 * self.m13) / determinant,
                m23: (self.m13 * self.m12 - self.m23 * self.m11) / determinant,
                m33: (self.m11 * self.m22 - self.m12 * self.m12) / determinant,
            }
        }
    }

    fn transform_vector(&self, v: Vector) -> Vector {
        let x = self.m11 * v.x + self.m12 * v.y + self.m13 * v.z;
        let y = self.m12 * v.x + self.m22 * v.y + self.m23 * v.z;
        let z = self.m13 * v.x + self.m23 * v.y + self.m33 * v.z;
        Vector::new(x, y, z)
    }

    #[inline]
    #[rustfmt::skip]
    fn into_matrix(self) -> Matrix {
        Matrix::from_cols_array(&[
            self.m11, self.m12, self.m13,
            self.m12, self.m22, self.m23,
            self.m13, self.m23, self.m33,
        ])
    }
}

#[cfg(feature = "simd-is-enabled")]
impl AngularInertiaOps<SimdReal> for parry::utils::SdpMatrix3<SimdReal> {
    type AngVector = na::Vector3<SimdReal>;
    type AngMatrix = na::Matrix3<SimdReal>;

    #[inline]
    fn inverse(&self) -> Self {
        self.inverse_unchecked()
    }

    #[inline]
    fn transform_vector(&self, v: na::Vector3<SimdReal>) -> na::Vector3<SimdReal> {
        let x = self.m11 * v.x + self.m12 * v.y + self.m13 * v.z;
        let y = self.m12 * v.x + self.m22 * v.y + self.m23 * v.z;
        let z = self.m13 * v.x + self.m23 * v.y + self.m33 * v.z;
        na::Vector3::new(x, y, z)
    }

    #[inline]
    #[rustfmt::skip]
    fn into_matrix(self) -> na::Matrix3<SimdReal> {
        na::Matrix3::new(
            self.m11, self.m12, self.m13,
            self.m12, self.m22, self.m23,
            self.m13, self.m23, self.m33,
        )
    }
}
