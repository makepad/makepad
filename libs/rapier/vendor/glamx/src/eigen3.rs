//! Eigendecomposition for symmetric 3x3 matrices.
//!
//! The eigensolver is a Rust adaptation, with modifications, of the pseudocode and approach described in
//! "A Robust Eigensolver for 3 x 3 Symmetric Matrices" by David Eberly, Geometric Tools, Redmond WA 98052.
//! <https://www.geometrictools.com/Documentation/RobustEigenSymmetric3x3.pdf>
//!
//! Adapted from <https://github.com/Jondolf/barry/tree/main/src/math/eigen>

use glam::Vec3Swizzles;
use num_traits::float::FloatConst;

#[cfg(not(feature = "std"))]
use simba::scalar::ComplexField;

/// Macro to generate SymmetricEigen3 for a specific scalar type.
macro_rules! impl_symmetric_eigen3 {
    ($SymmetricEigen3:ident, $Mat3:ty, $Vec3:ty, $Real:ty $(, #[$attr:meta])*) => {
        #[doc = concat!("The eigen decomposition of a symmetric 3x3 matrix (", stringify!($Real), " precision).")]
        #[derive(Clone, Copy, Debug, PartialEq)]
        #[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
        $(#[$attr])*
        pub struct $SymmetricEigen3 {
            /// The eigenvalues of the symmetric 3x3 matrix.
            pub eigenvalues: $Vec3,
            /// The three eigenvectors of the symmetric 3x3 matrix (as columns).
            pub eigenvectors: $Mat3,
        }

        impl $SymmetricEigen3 {
            /// Computes the eigen decomposition of the given symmetric 3x3 matrix.
            ///
            /// The eigenvalues are returned in ascending order `eigen1 < eigen2 < eigen3`.
            /// This can be reversed with the [`reverse`](Self::reverse) method.
            pub fn new(mat: $Mat3) -> Self {
                let eigenvalues = Self::eigenvalues(mat);
                let eigenvector1 = Self::eigenvector1(mat, eigenvalues.x);
                let eigenvector2 = Self::eigenvector2(mat, eigenvector1, eigenvalues.y);
                let eigenvector3 = Self::eigenvector3(eigenvector1, eigenvector2);

                Self {
                    eigenvalues,
                    eigenvectors: <$Mat3>::from_cols(eigenvector1, eigenvector2, eigenvector3),
                }
            }

            /// Reverses the order of the eigenvalues and their corresponding eigenvectors.
            pub fn reverse(&self) -> Self {
                Self {
                    eigenvalues: self.eigenvalues.zyx(),
                    eigenvectors: <$Mat3>::from_cols(
                        self.eigenvectors.z_axis,
                        self.eigenvectors.y_axis,
                        self.eigenvectors.x_axis,
                    ),
                }
            }

            /// Computes the eigenvalues of a symmetric 3x3 matrix.
            ///
            /// Reference: <https://en.wikipedia.org/wiki/Eigenvalue_algorithm#3%C3%973_matrices>
            pub fn eigenvalues(mat: $Mat3) -> $Vec3 {
                let p1 = mat.y_axis.x.powi(2) + mat.z_axis.x.powi(2) + mat.z_axis.y.powi(2);
                if p1 == 0.0 {
                    // The matrix is diagonal.
                    let mut eigenvalues = [mat.x_axis.x, mat.y_axis.y, mat.z_axis.z];
                    eigenvalues.sort_by(|a, b| a.partial_cmp(b).unwrap_or(core::cmp::Ordering::Equal));
                    <$Vec3>::from_array(eigenvalues)
                } else {
                    let q = (mat.x_axis.x + mat.y_axis.y + mat.z_axis.z) / 3.0;
                    let p2 = (mat.x_axis.x - q).powi(2)
                        + (mat.y_axis.y - q).powi(2)
                        + (mat.z_axis.z - q).powi(2)
                        + 2.0 * p1;
                    let p = (p2 / 6.0).sqrt();
                    let mat_b = 1.0 / p * (mat - q * <$Mat3>::IDENTITY);
                    let r = mat_b.determinant() / 2.0;

                    // r should be in the [-1, 1] range for a symmetric matrix,
                    // but computation error can leave it slightly outside this range.
                    let pi: $Real = <$Real as FloatConst>::PI();
                    let phi = if r <= -1.0 {
                        pi / 3.0
                    } else if r >= 1.0 {
                        0.0
                    } else {
                        r.acos() / 3.0
                    };

                    // The eigenvalues satisfy eigen3 <= eigen2 <= eigen1
                    let eigen1 = q + 2.0 * p * phi.cos();
                    let eigen3 = q + 2.0 * p * (phi + 2.0 * pi / 3.0).cos();
                    let eigen2 = 3.0 * q - eigen1 - eigen3; // trace(mat) = eigen1 + eigen2 + eigen3
                    <$Vec3>::new(eigen3, eigen2, eigen1)
                }
            }

            /// Computes the unit-length eigenvector corresponding to the `eigenvalue1` of `mat` that was
            /// computed from the root of a cubic polynomial with a multiplicity of 1.
            ///
            /// If the other two eigenvalues are well separated, this method can be used for computing
            /// all three eigenvectors. However, to avoid numerical issues when eigenvalues are close to
            /// each other, it's recommended to use the `eigenvector2` method for the second eigenvector.
            ///
            /// The third eigenvector can be computed as the cross product of the first two.
            pub fn eigenvector1(mat: $Mat3, eigenvalue1: $Real) -> $Vec3 {
                let cols = mat - <$Mat3>::from_diagonal(<$Vec3>::splat(eigenvalue1).into());
                let c0xc1 = cols.x_axis.cross(cols.y_axis);
                let c0xc2 = cols.x_axis.cross(cols.z_axis);
                let c1xc2 = cols.y_axis.cross(cols.z_axis);
                let d0 = c0xc1.length_squared();
                let d1 = c0xc2.length_squared();
                let d2 = c1xc2.length_squared();

                let mut d_max = d0;
                let mut i_max = 0;

                if d1 > d_max {
                    d_max = d1;
                    i_max = 1;
                }
                if d2 > d_max {
                    d_max = d2;
                    i_max = 2;
                }

                // When all eigenvalues are equal (multiplicity 3), mat - eigenvalue * I is the
                // zero matrix, so all cross products are zero. Any unit vector is a valid
                // eigenvector in this case.
                if d_max < 1e-20 {
                    return <$Vec3>::X;
                }

                if i_max == 0 {
                    c0xc1 / d0.sqrt()
                } else if i_max == 1 {
                    c0xc2 / d1.sqrt()
                } else {
                    c1xc2 / d2.sqrt()
                }
            }

            /// Computes the unit-length eigenvector corresponding to the `eigenvalue2` of `mat` that was
            /// computed from the root of a cubic polynomial with a potential multiplicity of 2.
            ///
            /// The third eigenvector can be computed as the cross product of the first two.
            pub fn eigenvector2(mat: $Mat3, eigenvector1: $Vec3, eigenvalue2: $Real) -> $Vec3 {
                // Compute right-handed orthonormal set { U, V, W }, where W is eigenvector1.
                let (u, v) = Self::any_orthonormal_pair(eigenvector1);

                // The unit-length eigenvector is E = x0 * U + x1 * V. We need to compute x0 and x1.
                //
                // Define the symmetric 2x2 matrix M = J^T * (mat - eigenvalue2 * I), where J = [U V]
                // and I is a 3x3 identity matrix. This means that E = J * X, where X is a column vector
                // with rows x0 and x1. The 3x3 linear system (mat - eigenvalue2 * I) * E = 0 reduces to
                // the 2x2 linear system M * X = 0.
                //
                // When eigenvalue2 != eigenvalue3, M has rank 1 and is not the zero matrix.
                // Otherwise, it has rank 0, and it is the zero matrix.

                let au = mat * u;
                let av = mat * v;

                let mut m00 = u.dot(au) - eigenvalue2;
                let mut m01 = u.dot(av);
                let mut m11 = v.dot(av) - eigenvalue2;
                let (abs_m00, abs_m01, abs_m11) = (m00.abs(), m01.abs(), m11.abs());

                if abs_m00 >= abs_m11 {
                    let max_abs_component = abs_m00.max(abs_m01);
                    if max_abs_component > 0.0 {
                        if abs_m00 >= abs_m01 {
                            // m00 is the largest component of the row.
                            // Factor it out for normalization and discard to avoid underflow or overflow.
                            m01 /= m00;
                            m00 = 1.0 / (1.0 + m01 * m01).sqrt();
                            m01 *= m00;
                        } else {
                            // m01 is the largest component of the row.
                            // Factor it out for normalization and discard to avoid underflow or overflow.
                            m00 /= m01;
                            m01 = 1.0 / (1.0 + m00 * m00).sqrt();
                            m00 *= m01;
                        }
                        return m01 * u - m00 * v;
                    }
                } else {
                    let max_abs_component = abs_m11.max(abs_m01);
                    if max_abs_component > 0.0 {
                        if abs_m11 >= abs_m01 {
                            // m11 is the largest component of the row.
                            // Factor it out for normalization and discard to avoid underflow or overflow.
                            m01 /= m11;
                            m11 = 1.0 / (1.0 + m01 * m01).sqrt();
                            m01 *= m11;
                        } else {
                            // m01 is the largest component of the row.
                            // Factor it out for normalization and discard to avoid underflow or overflow.
                            m11 /= m01;
                            m01 = 1.0 / (1.0 + m11 * m11).sqrt();
                            m11 *= m01;
                        }
                        return m11 * u - m01 * v;
                    }
                }

                // M is the zero matrix, any unit-length solution suffices.
                u
            }

            /// Computes the third eigenvector as the cross product of the first two.
            /// If the given eigenvectors are valid, the returned vector should be unit length.
            pub fn eigenvector3(eigenvector1: $Vec3, eigenvector2: $Vec3) -> $Vec3 {
                eigenvector1.cross(eigenvector2)
            }

            /// Compute any orthonormal pair of vectors perpendicular to `v`.
            fn any_orthonormal_pair(v: $Vec3) -> ($Vec3, $Vec3) {
                let sign = if v.z >= 0.0 { 1.0 } else { -1.0 };
                let a = -1.0 / (sign + v.z);
                let b = v.x * v.y * a;
                let u = <$Vec3>::new(1.0 + sign * v.x * v.x * a, sign * b, -sign * v.x);
                let w = <$Vec3>::new(b, sign + v.y * v.y * a, -v.y);
                (u, w)
            }
        }
    };
}

impl_symmetric_eigen3!(SymmetricEigen3, glam::Mat3, glam::Vec3, f32);
impl_symmetric_eigen3!(SymmetricEigen3A, glam::Mat3A, glam::Vec3A, f32);
impl_symmetric_eigen3!(DSymmetricEigen3, glam::DMat3, glam::DVec3, f64);

// f32 <-> f64 conversions
impl From<SymmetricEigen3> for DSymmetricEigen3 {
    #[inline]
    fn from(e: SymmetricEigen3) -> Self {
        Self {
            eigenvalues: e.eigenvalues.as_dvec3(),
            eigenvectors: e.eigenvectors.as_dmat3(),
        }
    }
}

impl From<DSymmetricEigen3> for SymmetricEigen3 {
    #[inline]
    fn from(e: DSymmetricEigen3) -> Self {
        Self {
            eigenvalues: e.eigenvalues.as_vec3(),
            eigenvectors: e.eigenvectors.as_mat3(),
        }
    }
}

impl From<SymmetricEigen3A> for DSymmetricEigen3 {
    #[inline]
    fn from(e: SymmetricEigen3A) -> Self {
        Self {
            eigenvalues: e.eigenvalues.as_dvec3(),
            eigenvectors: e.eigenvectors.as_dmat3(),
        }
    }
}

impl From<DSymmetricEigen3> for SymmetricEigen3A {
    #[inline]
    fn from(e: DSymmetricEigen3) -> Self {
        Self {
            eigenvalues: e.eigenvalues.as_vec3a(),
            eigenvectors: e.eigenvectors.as_mat3().into(),
        }
    }
}

// SymmetricEigen3 <-> SymmetricEigen3A conversions
impl From<SymmetricEigen3> for SymmetricEigen3A {
    #[inline]
    fn from(e: SymmetricEigen3) -> Self {
        Self {
            eigenvalues: e.eigenvalues.into(),
            eigenvectors: glam::Mat3A::from(e.eigenvectors),
        }
    }
}

impl From<SymmetricEigen3A> for SymmetricEigen3 {
    #[inline]
    fn from(e: SymmetricEigen3A) -> Self {
        Self {
            eigenvalues: e.eigenvalues.into(),
            eigenvectors: glam::Mat3::from(e.eigenvectors),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_relative_eq;

    #[test]
    fn eigen_3x3() {
        let mat =
            glam::Mat3::from_cols_array_2d(&[[2.0, 7.0, 8.0], [7.0, 6.0, 3.0], [8.0, 3.0, 0.0]]);
        let eigen = SymmetricEigen3::new(mat);

        assert_relative_eq!(
            eigen.eigenvalues,
            glam::Vec3::new(-7.6051016780, 0.5774961930, 15.0276054850),
            epsilon = 1.0e-6
        );
        assert_relative_eq!(
            glam::Mat3::from_cols(
                eigen.eigenvectors.x_axis,
                eigen.eigenvectors.y_axis,
                eigen.eigenvectors.z_axis
            ),
            glam::Mat3::from_cols(
                glam::Vec3::new(1.0754474819, -0.3328260589, -1.0).normalize(),
                glam::Vec3::new(-0.5420669703, 1.2530131898, -1.0).normalize(),
                glam::Vec3::new(1.3587443369, 1.3858835966, 1.0).normalize()
            ),
            epsilon = 1.0e-6
        );
    }

    #[test]
    fn eigen_3x3_diagonal() {
        let mat =
            glam::Mat3::from_cols_array_2d(&[[2.0, 0.0, 0.0], [0.0, 5.0, 0.0], [0.0, 0.0, 3.0]]);
        let eigen = SymmetricEigen3::new(mat);

        assert_eq!(eigen.eigenvalues, glam::Vec3::new(2.0, 3.0, 5.0));
        assert_eq!(
            glam::Mat3::from_cols(
                eigen.eigenvectors.x_axis.normalize().abs(),
                eigen.eigenvectors.y_axis.normalize().abs(),
                eigen.eigenvectors.z_axis.normalize().abs()
            ),
            glam::Mat3::from_cols_array_2d(&[[1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0]])
        );
    }

    #[test]
    fn eigen_3x3_f64() {
        let mat =
            glam::DMat3::from_cols_array_2d(&[[2.0, 7.0, 8.0], [7.0, 6.0, 3.0], [8.0, 3.0, 0.0]]);
        let eigen = DSymmetricEigen3::new(mat);

        assert_relative_eq!(
            eigen.eigenvalues,
            glam::DVec3::new(-7.6051016780, 0.5774961930, 15.0276054850),
            epsilon = 1.0e-8
        );
    }

    #[test]
    fn eigen_3x3_identity() {
        // Identity matrix has all eigenvalues equal to 1 (multiplicity 3)
        // Any orthonormal basis is a valid set of eigenvectors
        let mat = glam::Mat3::IDENTITY;
        let eigen = SymmetricEigen3::new(mat);

        assert_eq!(eigen.eigenvalues, glam::Vec3::ONE);

        // Verify eigenvectors are orthonormal (any orthonormal basis is valid)
        assert_relative_eq!(
            eigen.eigenvectors * eigen.eigenvectors.transpose(),
            glam::Mat3::IDENTITY,
            epsilon = 1e-6
        );
    }

    #[test]
    fn eigen_3x3_uniform_scaling() {
        // Uniform scaling matrix: 5*I has all eigenvalues equal to 5
        let mat = glam::Mat3::from_diagonal(glam::Vec3::splat(5.0));
        let eigen = SymmetricEigen3::new(mat);

        assert_eq!(eigen.eigenvalues, glam::Vec3::splat(5.0));

        // Verify eigenvectors are orthonormal
        assert_relative_eq!(
            eigen.eigenvectors * eigen.eigenvectors.transpose(),
            glam::Mat3::IDENTITY,
            epsilon = 1e-6
        );
    }
}
