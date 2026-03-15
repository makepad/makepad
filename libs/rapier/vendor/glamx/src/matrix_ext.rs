//! Matrix extension traits for glam types.

use crate::eigen2::{DSymmetricEigen2, SymmetricEigen2};
use crate::eigen3::{DSymmetricEigen3, SymmetricEigen3, SymmetricEigen3A};
use crate::svd2::{DSvd2, Svd2};
use crate::svd3::{DSvd3, Svd3, Svd3A};

/// Extension trait for square matrix types.
///
/// Provides additional functionality not available in glam:
/// - `abs()` - element-wise absolute value
/// - `try_inverse()` - inverse with singularity check
/// - `swap_cols()` / `swap_rows()` - column/row swapping
/// - `symmetric_eigen()` - eigendecomposition for symmetric matrices
pub trait MatExt: Sized + Copy {
    /// The scalar type.
    type Scalar;
    /// The vector type.
    type Vector;
    /// The symmetric eigen decomposition type.
    type SymmetricEigen;
    /// The Singular Values Decomposition type.
    type Svd;

    /// Returns a matrix with all components set to their absolute values.
    fn abs(&self) -> Self;

    /// Tries to invert the matrix, returning None if not invertible.
    fn try_inverse(&self) -> Option<Self>;

    /// Swaps two columns of this matrix.
    fn swap_cols(&mut self, a: usize, b: usize);

    /// Swaps two rows of this matrix.
    fn swap_rows(&mut self, a: usize, b: usize);

    /// Computes the symmetric eigendecomposition.
    ///
    /// If `self` isn’t symmetric, expect incorrect results.
    fn symmetric_eigen(&self) -> Self::SymmetricEigen;

    /// Computes the eigenvalues of a symmetric matrix.
    ///
    /// If `self` isn’t symmetric, expect incorrect results.
    fn symmetric_eigenvalues(&self) -> Self::Vector;

    /// Computes the SVD of this matrix.
    fn svd(&self) -> Self::Svd;
}

/// Macro to implement MatExt for a specific matrix type.
macro_rules! impl_mat2_ext {
    ($Mat2:ty, $Vec2:ty, $Real:ty, $SymmetricEigen2:ty, $Svd2:ty) => {
        impl MatExt for $Mat2 {
            type Scalar = $Real;
            type Vector = $Vec2;
            type SymmetricEigen = $SymmetricEigen2;
            type Svd = $Svd2;

            #[inline]
            fn abs(&self) -> Self {
                Self::from_cols(self.x_axis.abs(), self.y_axis.abs())
            }

            #[inline]
            fn try_inverse(&self) -> Option<Self> {
                let det = self.determinant();
                if det.abs() < <$Real>::EPSILON {
                    None
                } else {
                    Some(self.inverse())
                }
            }

            #[inline]
            fn swap_cols(&mut self, a: usize, b: usize) {
                assert!(a < 2, "column index {a} is out of bounds");
                assert!(b < 2, "column index {b} is out of bounds");

                let ca = self.col(a);
                let cb = self.col(b);
                *self.col_mut(a) = cb;
                *self.col_mut(b) = ca;
            }

            #[inline]
            fn swap_rows(&mut self, a: usize, b: usize) {
                assert!(a < 2, "row index {a} is out of bounds");
                assert!(b < 2, "row index {b} is out of bounds");
                self.x_axis.as_mut().swap(a, b);
                self.y_axis.as_mut().swap(a, b);
            }

            #[inline]
            fn symmetric_eigen(&self) -> Self::SymmetricEigen {
                <$SymmetricEigen2>::new(*self)
            }

            #[inline]
            fn symmetric_eigenvalues(&self) -> Self::Vector {
                <$SymmetricEigen2>::eigenvalues(*self)
            }

            #[inline]
            fn svd(&self) -> Self::Svd {
                <$Svd2>::from_matrix(*self)
            }
        }
    };
}

/// Macro to implement MatExt for a specific matrix type.
macro_rules! impl_mat3_ext {
    ($Mat3:ty, $Vec3:ty, $Real:ty, $SymmetricEigen3:ty, $Svd3:ty) => {
        impl MatExt for $Mat3 {
            type Scalar = $Real;
            type Vector = $Vec3;
            type SymmetricEigen = $SymmetricEigen3;
            type Svd = $Svd3;

            #[inline]
            fn abs(&self) -> Self {
                Self::from_cols(self.x_axis.abs(), self.y_axis.abs(), self.z_axis.abs())
            }

            #[inline]
            fn try_inverse(&self) -> Option<Self> {
                let det = self.determinant();
                if det.abs() < <$Real>::EPSILON {
                    None
                } else {
                    Some(self.inverse())
                }
            }

            fn swap_cols(&mut self, a: usize, b: usize) {
                assert!(a < 3, "column index {a} is out of bounds");
                assert!(b < 3, "column index {b} is out of bounds");

                let ca = self.col(a);
                let cb = self.col(b);
                *self.col_mut(a) = cb;
                *self.col_mut(b) = ca;
            }

            fn swap_rows(&mut self, a: usize, b: usize) {
                assert!(a < 3, "row index {a} is out of bounds");
                assert!(b < 3, "row index {b} is out of bounds");
                self.x_axis.as_mut().swap(a, b);
                self.y_axis.as_mut().swap(a, b);
                self.z_axis.as_mut().swap(a, b);
            }

            fn symmetric_eigen(&self) -> Self::SymmetricEigen {
                <$SymmetricEigen3>::new(*self)
            }

            fn symmetric_eigenvalues(&self) -> Self::Vector {
                <$SymmetricEigen3>::eigenvalues(*self)
            }

            #[inline]
            fn svd(&self) -> Self::Svd {
                <$Svd3>::from_matrix(*self)
            }
        }
    };
}

impl_mat2_ext!(glam::Mat2, glam::Vec2, f32, SymmetricEigen2, Svd2);
impl_mat2_ext!(glam::DMat2, glam::DVec2, f64, DSymmetricEigen2, DSvd2);
impl_mat3_ext!(glam::Mat3, glam::Vec3, f32, SymmetricEigen3, Svd3);
impl_mat3_ext!(glam::Mat3A, glam::Vec3A, f32, SymmetricEigen3A, Svd3A);
impl_mat3_ext!(glam::DMat3, glam::DVec3, f64, DSymmetricEigen3, DSvd3);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mat2_abs() {
        let m = glam::Mat2::from_cols(glam::Vec2::new(-1.0, 2.0), glam::Vec2::new(3.0, -4.0));
        let abs_m = m.abs();
        assert_eq!(abs_m.x_axis, glam::Vec2::new(1.0, 2.0));
        assert_eq!(abs_m.y_axis, glam::Vec2::new(3.0, 4.0));
    }

    #[test]
    fn test_mat2_try_inverse() {
        let m = glam::Mat2::from_cols(glam::Vec2::new(1.0, 2.0), glam::Vec2::new(3.0, 4.0));
        let inv = m.try_inverse();
        assert!(inv.is_some());

        // Singular matrix
        let singular = glam::Mat2::from_cols(glam::Vec2::new(1.0, 2.0), glam::Vec2::new(2.0, 4.0));
        let no_inv = singular.try_inverse();
        assert!(no_inv.is_none());
    }

    #[test]
    fn test_mat3_abs() {
        let m = glam::Mat3::from_cols(
            glam::Vec3::new(-1.0, 2.0, -3.0),
            glam::Vec3::new(4.0, -5.0, 6.0),
            glam::Vec3::new(-7.0, 8.0, -9.0),
        );
        let abs_m = m.abs();
        assert_eq!(abs_m.x_axis, glam::Vec3::new(1.0, 2.0, 3.0));
        assert_eq!(abs_m.y_axis, glam::Vec3::new(4.0, 5.0, 6.0));
        assert_eq!(abs_m.z_axis, glam::Vec3::new(7.0, 8.0, 9.0));
    }

    #[test]
    fn test_mat3_swap_cols() {
        let mut m = glam::Mat3::from_cols(
            glam::Vec3::new(1.0, 2.0, 3.0),
            glam::Vec3::new(4.0, 5.0, 6.0),
            glam::Vec3::new(7.0, 8.0, 9.0),
        );
        m.swap_cols(0, 2);
        assert_eq!(m.x_axis, glam::Vec3::new(7.0, 8.0, 9.0));
        assert_eq!(m.z_axis, glam::Vec3::new(1.0, 2.0, 3.0));
    }

    #[test]
    fn test_dmat2_abs() {
        let m = glam::DMat2::from_cols(glam::DVec2::new(-1.0, 2.0), glam::DVec2::new(3.0, -4.0));
        let abs_m = m.abs();
        assert_eq!(abs_m.x_axis, glam::DVec2::new(1.0, 2.0));
        assert_eq!(abs_m.y_axis, glam::DVec2::new(3.0, 4.0));
    }

    #[test]
    fn test_mat3_symmetric_eigen() {
        use approx::assert_relative_eq;
        let m =
            glam::Mat3::from_cols_array_2d(&[[2.0, 0.0, 0.0], [0.0, 3.0, 0.0], [0.0, 0.0, 5.0]]);
        let eigenvalues = m.symmetric_eigenvalues();
        // Eigenvalues of a diagonal matrix are its diagonal entries, sorted ascending
        assert_relative_eq!(eigenvalues.x, 2.0, epsilon = 0.001);
        assert_relative_eq!(eigenvalues.y, 3.0, epsilon = 0.001);
        assert_relative_eq!(eigenvalues.z, 5.0, epsilon = 0.001);
    }
}
