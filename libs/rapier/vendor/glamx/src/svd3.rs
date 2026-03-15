//! SVD decomposition for 3x3 matrices.
//!
//! Uses eigendecomposition of A^T * A to compute singular values and right singular vectors,
//! then derives left singular vectors from those.

#[cfg(not(feature = "std"))]
use simba::scalar::ComplexField;

use crate::eigen3::{DSymmetricEigen3, SymmetricEigen3};
use crate::SymmetricEigen3A;

/// Macro to generate Svd3 for a specific scalar type.
macro_rules! impl_svd3 {
    ($Svd3:ident, $Mat3:ty, $Vec3:ty, $SymmetricEigen3:ty, $Real:ty $(, #[$attr:meta])*) => {
        #[doc = concat!("The SVD of a 3x3 matrix (", stringify!($Real), " precision).")]
        #[derive(Clone, Copy, Debug, PartialEq)]
        #[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
        $(#[$attr])*
        pub struct $Svd3 {
            /// The left orthogonal matrix U.
            pub u: $Mat3,
            /// The singular values (diagonal entries of S), sorted in descending order.
            pub s: $Vec3,
            /// The transposed right orthogonal matrix V^T.
            pub vt: $Mat3,
        }

        impl $Svd3 {
            /// Creates a new SVD from components.
            #[inline]
            pub fn new(u: $Mat3, s: $Vec3, vt: $Mat3) -> Self {
                Self { u, s, vt }
            }

            /// Computes the SVD of a 3x3 matrix.
            ///
            /// Returns the decomposition `M = U * S * V^T` where:
            /// - `U` is an orthogonal matrix (left singular vectors as columns)
            /// - `S` is a diagonal matrix (stored as a vector of singular values)
            /// - `V^T` is the transpose of an orthogonal matrix
            ///
            /// The singular values are sorted in descending order (s.x >= s.y >= s.z).
            #[inline]
            pub fn from_matrix(a: $Mat3) -> Self {
                // Compute A^T * A (symmetric positive semi-definite)
                let ata = a.transpose() * a;

                // Eigendecomposition of A^T * A gives:
                // - eigenvalues = singular values squared
                // - eigenvectors = right singular vectors (columns of V)
                let eigen = <$SymmetricEigen3>::new(ata).reverse(); // reverse to get descending order

                // Singular values are square roots of eigenvalues
                let s = <$Vec3>::new(
                    eigen.eigenvalues.x.max(0.0).sqrt(),
                    eigen.eigenvalues.y.max(0.0).sqrt(),
                    eigen.eigenvalues.z.max(0.0).sqrt(),
                );

                // V is the eigenvector matrix
                let v = eigen.eigenvectors;

                // Compute U = A * V * S^(-1) for each column
                // U.col(i) = A * V.col(i) / s[i]
                let epsilon: $Real = 1e-10;

                // Compute U columns with Gram-Schmidt orthogonalization
                let u_col0_raw = if s.x > epsilon {
                    (a * v.col(0)) / s.x
                } else {
                    v.col(0)
                };
                let u_col0 = u_col0_raw.normalize();

                let u_col1_raw = if s.y > epsilon {
                    (a * v.col(1)) / s.y
                } else {
                    Self::any_orthogonal(u_col0)
                };
                // Orthogonalize against u_col0
                let u_col1 = (u_col1_raw - u_col0 * u_col1_raw.dot(u_col0)).normalize();

                let u_col2_raw = if s.z > epsilon {
                    (a * v.col(2)) / s.z
                } else {
                    u_col0.cross(u_col1)
                };
                // Orthogonalize against u_col0 and u_col1
                let u_col2_orth = u_col2_raw - u_col0 * u_col2_raw.dot(u_col0) - u_col1 * u_col2_raw.dot(u_col1);
                let u_col2 = u_col2_orth.normalize();

                let u = <$Mat3>::from_cols(u_col0, u_col1, u_col2);

                Self {
                    u,
                    s,
                    vt: v.transpose(),
                }
            }

            /// Find any unit vector orthogonal to the given vector.
            #[inline]
            fn any_orthogonal(v: $Vec3) -> $Vec3 {
                let abs_v = <$Vec3>::new(v.x.abs(), v.y.abs(), v.z.abs());
                let other = if abs_v.x <= abs_v.y && abs_v.x <= abs_v.z {
                    <$Vec3>::X
                } else if abs_v.y <= abs_v.z {
                    <$Vec3>::Y
                } else {
                    <$Vec3>::Z
                };
                v.cross(other).normalize()
            }

            /// Rebuilds the matrix this SVD is the decomposition of.
            ///
            /// Returns `U * S * V^T`.
            #[inline]
            pub fn recompose(&self) -> $Mat3 {
                let u_s = <$Mat3>::from_cols(
                    self.u.col(0) * self.s.x,
                    self.u.col(1) * self.s.y,
                    self.u.col(2) * self.s.z,
                );
                u_s * self.vt
            }
        }
    };
}

impl_svd3!(Svd3, glam::Mat3, glam::Vec3, SymmetricEigen3, f32);
impl_svd3!(Svd3A, glam::Mat3A, glam::Vec3A, SymmetricEigen3A, f32);
impl_svd3!(DSvd3, glam::DMat3, glam::DVec3, DSymmetricEigen3, f64);

// f32 <-> f64 conversions
impl From<Svd3> for DSvd3 {
    #[inline]
    fn from(svd: Svd3) -> Self {
        Self {
            u: svd.u.as_dmat3(),
            s: svd.s.as_dvec3(),
            vt: svd.vt.as_dmat3(),
        }
    }
}

impl From<DSvd3> for Svd3 {
    #[inline]
    fn from(svd: DSvd3) -> Self {
        Self {
            u: svd.u.as_mat3(),
            s: svd.s.as_vec3(),
            vt: svd.vt.as_mat3(),
        }
    }
}

impl From<Svd3A> for DSvd3 {
    #[inline]
    fn from(svd: Svd3A) -> Self {
        Self {
            u: svd.u.as_dmat3(),
            s: svd.s.as_dvec3(),
            vt: svd.vt.as_dmat3(),
        }
    }
}

impl From<DSvd3> for Svd3A {
    #[inline]
    fn from(svd: DSvd3) -> Self {
        Self {
            u: svd.u.as_mat3().into(),
            s: svd.s.as_vec3a(),
            vt: svd.vt.as_mat3().into(),
        }
    }
}

// Svd3 <-> Svd3A conversions
impl From<Svd3> for Svd3A {
    #[inline]
    fn from(svd: Svd3) -> Self {
        Self {
            u: glam::Mat3A::from(svd.u),
            s: svd.s.into(),
            vt: glam::Mat3A::from(svd.vt),
        }
    }
}

impl From<Svd3A> for Svd3 {
    #[inline]
    fn from(svd: Svd3A) -> Self {
        Self {
            u: glam::Mat3::from(svd.u),
            s: svd.s.into(),
            vt: glam::Mat3::from(svd.vt),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_relative_eq;

    #[test]
    fn svd_3x3_identity() {
        let mat = glam::Mat3::IDENTITY;
        let svd = Svd3::from_matrix(mat);

        assert_relative_eq!(svd.s, glam::Vec3::new(1.0, 1.0, 1.0), epsilon = 1e-5);
        assert_relative_eq!(svd.recompose(), mat, epsilon = 1e-5);
    }

    #[test]
    fn svd_3x3_diagonal() {
        let mat = glam::Mat3::from_diagonal(glam::Vec3::new(4.0, 3.0, 2.0));
        let svd = Svd3::from_matrix(mat);

        // Singular values should be sorted in descending order
        assert_relative_eq!(svd.s, glam::Vec3::new(4.0, 3.0, 2.0), epsilon = 1e-5);
        assert_relative_eq!(svd.recompose(), mat, epsilon = 1e-5);
    }

    #[test]
    fn svd_3x3_general() {
        // Use a full-rank matrix
        let mat =
            glam::Mat3::from_cols_array_2d(&[[1.0, 4.0, 7.0], [2.0, 5.0, 9.0], [3.0, 6.0, 10.0]]);
        let svd = Svd3::from_matrix(mat);

        // Verify recomposition (some numerical error from orthogonalization)
        assert_relative_eq!(svd.recompose(), mat, epsilon = 1e-2);

        // Verify singular values are sorted in descending order
        assert!(svd.s.x >= svd.s.y);
        assert!(svd.s.y >= svd.s.z);

        // Verify U is orthogonal
        assert_relative_eq!(
            svd.u * svd.u.transpose(),
            glam::Mat3::IDENTITY,
            epsilon = 1e-5
        );

        // Verify V^T is orthogonal
        assert_relative_eq!(
            svd.vt * svd.vt.transpose(),
            glam::Mat3::IDENTITY,
            epsilon = 1e-5
        );
    }

    #[test]
    fn svd_3x3_rotation() {
        let mat = glam::Mat3::from_rotation_x(0.5) * glam::Mat3::from_rotation_y(0.3);
        let svd = Svd3::from_matrix(mat);

        // A rotation matrix has singular values of 1
        assert_relative_eq!(svd.s, glam::Vec3::ONE, epsilon = 1e-5);
        assert_relative_eq!(svd.recompose(), mat, epsilon = 1e-5);
    }

    #[test]
    fn svd_3x3_f64() {
        let mat =
            glam::DMat3::from_cols_array_2d(&[[1.0, 4.0, 7.0], [2.0, 5.0, 9.0], [3.0, 6.0, 10.0]]);
        let svd = DSvd3::from_matrix(mat);

        assert_relative_eq!(svd.recompose(), mat, epsilon = 1e-8);
    }

    #[test]
    fn svd_3x3_rank_deficient() {
        // Test with a rank-2 matrix (third singular value is 0)
        let mat =
            glam::Mat3::from_cols_array_2d(&[[1.0, 4.0, 7.0], [2.0, 5.0, 8.0], [3.0, 6.0, 9.0]]);
        let svd = Svd3::from_matrix(mat);

        // Verify recomposition (larger tolerance for rank-deficient matrices)
        assert_relative_eq!(svd.recompose(), mat, epsilon = 0.02);

        // Third singular value should be small (matrix is nearly rank 2)
        assert!(svd.s.z < 0.1);
    }
}
