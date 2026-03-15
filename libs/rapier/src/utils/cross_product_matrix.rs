//! SimdCrossMatrix trait for computing cross product matrices.

#[cfg(feature = "dim3")]
use crate::math::Matrix;
#[cfg(feature = "simd-is-enabled")]
use crate::math::SimdReal;
use crate::math::{Real, Vector};
#[cfg(feature = "simd-is-enabled")]
use crate::num::Zero;
use crate::utils::SimdRealCopy;
use na::{Matrix2, Matrix3, Vector2, Vector3};

/// Trait for computing cross product matrices.
pub trait CrossProductMatrix: Sized {
    /// The cross product matrix type.
    type CrossMat;
    /// The transposed cross product matrix type.
    type CrossMatTr;
    /// Returns the cross product matrix of this vector.
    fn gcross_matrix(self) -> Self::CrossMat;
    /// Returns the transposed cross product matrix of this vector.
    fn gcross_matrix_tr(self) -> Self::CrossMatTr;
}

impl<N: SimdRealCopy> CrossProductMatrix for Vector3<N> {
    type CrossMat = Matrix3<N>;
    type CrossMatTr = Matrix3<N>;

    #[inline]
    #[rustfmt::skip]
    fn gcross_matrix(self) -> Self::CrossMat {
        Matrix3::new(
            N::zero(), -self.z, self.y,
            self.z, N::zero(), -self.x,
            -self.y, self.x, N::zero(),
        )
    }

    #[inline]
    #[rustfmt::skip]
    fn gcross_matrix_tr(self) -> Self::CrossMatTr {
        Matrix3::new(
            N::zero(), self.z, -self.y,
            -self.z, N::zero(), self.x,
            self.y, -self.x, N::zero(),
        )
    }
}

impl<N: SimdRealCopy> CrossProductMatrix for Vector2<N> {
    type CrossMat = Vector2<N>;
    type CrossMatTr = Vector2<N>;

    #[inline]
    fn gcross_matrix(self) -> Self::CrossMat {
        Vector2::new(-self.y, self.x)
    }
    #[inline]
    fn gcross_matrix_tr(self) -> Self::CrossMatTr {
        Vector2::new(-self.y, self.x)
    }
}
impl CrossProductMatrix for Real {
    type CrossMat = Matrix2<Real>;
    type CrossMatTr = Matrix2<Real>;

    #[inline]
    fn gcross_matrix(self) -> Matrix2<Real> {
        Matrix2::new(0.0, -self, self, 0.0)
    }

    #[inline]
    fn gcross_matrix_tr(self) -> Matrix2<Real> {
        Matrix2::new(0.0, self, -self, 0.0)
    }
}

#[cfg(feature = "simd-is-enabled")]
impl CrossProductMatrix for SimdReal {
    type CrossMat = Matrix2<SimdReal>;
    type CrossMatTr = Matrix2<SimdReal>;

    #[inline]
    fn gcross_matrix(self) -> Matrix2<SimdReal> {
        Matrix2::new(SimdReal::zero(), -self, self, SimdReal::zero())
    }

    #[inline]
    fn gcross_matrix_tr(self) -> Matrix2<SimdReal> {
        Matrix2::new(SimdReal::zero(), self, -self, SimdReal::zero())
    }
}

// Glam implementations for SimdCrossMatrix
#[cfg(feature = "dim2")]
impl CrossProductMatrix for Vector {
    type CrossMat = Vector;
    type CrossMatTr = Vector;

    #[inline]
    fn gcross_matrix(self) -> Self::CrossMat {
        Vector::new(-self.y, self.x)
    }
    #[inline]
    fn gcross_matrix_tr(self) -> Self::CrossMatTr {
        Vector::new(-self.y, self.x)
    }
}

#[cfg(feature = "dim3")]
impl CrossProductMatrix for Vector {
    type CrossMat = Matrix;
    type CrossMatTr = Matrix;

    #[inline]
    #[rustfmt::skip]
    fn gcross_matrix(self) -> Self::CrossMat {
        Matrix::from_cols(
            Vector::new(0.0, self.z, -self.y),
            Vector::new(-self.z, 0.0, self.x),
            Vector::new(self.y, -self.x, 0.0),
        )
    }

    #[inline]
    #[rustfmt::skip]
    fn gcross_matrix_tr(self) -> Self::CrossMatTr {
        Matrix::from_cols(
            Vector::new(0.0, -self.z, self.y),
            Vector::new(self.z, 0.0, -self.x),
            Vector::new(-self.y, self.x, 0.0),
        )
    }
}
