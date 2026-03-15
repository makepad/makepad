//! Eigendecomposition for symmetric 2x2 matrices.
//!
//! Adapted from <https://github.com/Jondolf/barry/tree/main/src/math/eigen>

use glam::Vec2Swizzles;

#[cfg(not(feature = "std"))]
use simba::scalar::ComplexField;

/// Macro to generate SymmetricEigen2 for a specific scalar type.
macro_rules! impl_symmetric_eigen2 {
    ($SymmetricEigen2:ident, $Mat2:ty, $Vec2:ty, $Real:ty $(, #[$attr:meta])*) => {
        #[doc = concat!("The eigen decomposition of a symmetric 2x2 matrix (", stringify!($Real), " precision).")]
        #[derive(Clone, Copy, Debug, PartialEq)]
        #[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
        $(#[$attr])*
        pub struct $SymmetricEigen2 {
            /// The eigenvalues of the symmetric 2x2 matrix.
            pub eigenvalues: $Vec2,
            /// The two eigenvectors of the symmetric 2x2 matrix (as columns).
            pub eigenvectors: $Mat2,
        }

        impl $SymmetricEigen2 {
            /// Computes the eigen decomposition of the given symmetric 2x2 matrix.
            ///
            /// The eigenvalues are returned in descending order `eigen1 > eigen2`.
            /// This can be reversed with the [`reverse`](Self::reverse) method.
            pub fn new(mat: $Mat2) -> Self {
                let eigenvalues = Self::eigenvalues(mat);
                let eigenvector1 = Self::eigenvector(mat, eigenvalues.x);
                let eigenvector2 = Self::eigenvector(mat, eigenvalues.y);

                Self {
                    eigenvalues,
                    eigenvectors: <$Mat2>::from_cols(eigenvector1, eigenvector2),
                }
            }

            /// Reverses the order of the eigenvalues and their corresponding eigenvectors.
            pub fn reverse(&self) -> Self {
                Self {
                    eigenvalues: self.eigenvalues.yx(),
                    eigenvectors: <$Mat2>::from_cols(self.eigenvectors.y_axis, self.eigenvectors.x_axis),
                }
            }

            /// Computes the eigenvalues of a symmetric 2x2 matrix.
            ///
            /// Reference: <https://croninprojects.org/Vince/Geodesy/FindingEigenvectors.pdf>
            pub fn eigenvalues(mat: $Mat2) -> $Vec2 {
                let [a, b, c] = [
                    1.0,
                    -(mat.x_axis.x + mat.y_axis.y),
                    mat.x_axis.x * mat.y_axis.y - mat.x_axis.y * mat.y_axis.x,
                ];
                // The eigenvalues are the roots of the quadratic equation:
                // ax^2 + bx + c = 0
                // x = (-b +/- sqrt(b^2 - 4ac)) / 2a
                let sqrt_part = (b.powi(2) - 4.0 * a * c).sqrt();
                let eigen1 = (-b + sqrt_part) / (2.0 * a);
                let eigen2 = (-b - sqrt_part) / (2.0 * a);
                <$Vec2>::new(eigen1, eigen2)
            }

            /// Computes the unit-length eigenvector corresponding to the given `eigenvalue`
            /// of the symmetric 2x2 `mat`.
            ///
            /// Reference: <https://croninprojects.org/Vince/Geodesy/FindingEigenvectors.pdf>
            pub fn eigenvector(mat: $Mat2, eigenvalue: $Real) -> $Vec2 {
                <$Vec2>::new(1.0, (eigenvalue - mat.x_axis.x) / mat.y_axis.x).normalize()
            }
        }
    };
}

impl_symmetric_eigen2!(SymmetricEigen2, glam::Mat2, glam::Vec2, f32);
impl_symmetric_eigen2!(DSymmetricEigen2, glam::DMat2, glam::DVec2, f64);

// f32 <-> f64 conversions
impl From<SymmetricEigen2> for DSymmetricEigen2 {
    #[inline]
    fn from(e: SymmetricEigen2) -> Self {
        Self {
            eigenvalues: e.eigenvalues.as_dvec2(),
            eigenvectors: e.eigenvectors.as_dmat2(),
        }
    }
}

impl From<DSymmetricEigen2> for SymmetricEigen2 {
    #[inline]
    fn from(e: DSymmetricEigen2) -> Self {
        Self {
            eigenvalues: e.eigenvalues.as_vec2(),
            eigenvectors: e.eigenvectors.as_mat2(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_relative_eq;

    #[test]
    fn eigen_2x2() {
        let mat = glam::Mat2::from_cols_array_2d(&[[6.0, 3.0], [3.0, 4.0]]);
        let eigen = SymmetricEigen2::new(mat);

        assert_relative_eq!(
            eigen.eigenvalues,
            glam::Vec2::new(8.16228, 1.83772),
            epsilon = 0.001
        );
        assert_relative_eq!(
            glam::Mat2::from_cols(eigen.eigenvectors.x_axis, eigen.eigenvectors.y_axis,),
            glam::Mat2::from_cols(
                glam::Vec2::new(0.811242, 0.58471),
                glam::Vec2::new(0.58471, -0.811242),
            ),
            epsilon = 0.001
        );
    }

    #[test]
    fn eigen_2x2_f64() {
        let mat = glam::DMat2::from_cols_array_2d(&[[6.0, 3.0], [3.0, 4.0]]);
        let eigen = DSymmetricEigen2::new(mat);

        assert_relative_eq!(
            eigen.eigenvalues,
            glam::DVec2::new(8.16227766016838, 1.8377223398316202),
            epsilon = 1e-10
        );
    }
}
