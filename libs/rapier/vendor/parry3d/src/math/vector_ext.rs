//! Vector extension trait for glam types.

use crate::math::{Matrix, Real, Vector};

#[cfg(not(feature = "std"))]
use simba::scalar::ComplexField;

/// Extension trait for glam vector types to provide additional functionality.
pub trait VectorExt: Sized + Copy {
    /// Creates a vector with the i-th component set to `val` and all others to zero.
    fn ith(i: usize, val: Real) -> Self;

    /// Computes the angle between two vectors in radians.
    fn angle(self, other: Self) -> Real;

    /// Computes the kronecker product between two vectors.
    fn kronecker(self, other: Self) -> Matrix;
}

impl VectorExt for Vector {
    #[inline]
    #[cfg(feature = "dim2")]
    fn ith(i: usize, val: Real) -> Self {
        match i {
            0 => Self::new(val, 0.0),
            1 => Self::new(0.0, val),
            _ => Self::ZERO,
        }
    }
    #[inline]
    #[cfg(feature = "dim3")]
    fn ith(i: usize, val: Real) -> Self {
        match i {
            0 => Self::new(val, 0.0, 0.0),
            1 => Self::new(0.0, val, 0.0),
            2 => Self::new(0.0, 0.0, val),
            _ => Self::ZERO,
        }
    }

    #[inline]
    fn angle(self, other: Self) -> Real {
        let dot = self.dot(other);
        let len_self = self.length();
        let len_other = other.length();
        if len_self > 0.0 && len_other > 0.0 {
            (dot / (len_self * len_other)).clamp(-1.0, 1.0).acos()
        } else {
            0.0
        }
    }

    #[inline]
    #[cfg(feature = "dim2")]
    fn kronecker(self, other: Self) -> Matrix {
        Matrix::from_cols(self * other.x, self * other.y)
    }

    #[inline]
    #[cfg(feature = "dim3")]
    fn kronecker(self, other: Self) -> Matrix {
        Matrix::from_cols(self * other.x, self * other.y, self * other.z)
    }
}
