//! SVD decomposition for 2x2 matrices.

#[cfg(not(feature = "std"))]
use simba::scalar::{ComplexField, RealField};

/// Macro to generate Svd2 for a specific scalar type.
macro_rules! impl_svd2 {
    ($Svd2:ident, $Mat2:ty, $Vec2:ty, $Real:ty $(, #[$attr:meta])*) => {
        #[doc = concat!("The SVD of a 2x2 matrix (", stringify!($Real), " precision).")]
        #[derive(Clone, Copy, Debug, PartialEq)]
        #[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
        $(#[$attr])*
        pub struct $Svd2 {
            /// The left orthogonal matrix U.
            pub u: $Mat2,
            /// The singular values (diagonal entries of S).
            pub s: $Vec2,
            /// The transposed right orthogonal matrix V^T.
            pub vt: $Mat2,
        }

        impl $Svd2 {
            /// Creates a new SVD from components.
            #[inline]
            pub fn new(u: $Mat2, s: $Vec2, vt: $Mat2) -> Self {
                Self { u, s, vt }
            }

            /// Computes the SVD of a 2x2 matrix.
            ///
            /// Returns the decomposition `M = U * S * V^T` where:
            /// - `U` is an orthogonal matrix
            /// - `S` is a diagonal matrix (stored as a vector of singular values)
            /// - `V^T` is the transpose of an orthogonal matrix
            ///
            /// The singular values are sorted in descending order (s.x >= s.y).
            #[inline]
            pub fn from_matrix(m: $Mat2) -> Self {
                let e = (m.col(0).x + m.col(1).y) * 0.5;
                let f = (m.col(0).x - m.col(1).y) * 0.5;
                let g = (m.col(0).y + m.col(1).x) * 0.5;
                let h = (m.col(0).y - m.col(1).x) * 0.5;
                let q = (e * e + h * h).sqrt();
                let r = (f * f + g * g).sqrt();

                // Note that the singular values are always sorted because sx >= sy
                // because q >= 0 and r >= 0.
                let sx = q + r;
                let sy = q - r;
                let sy_sign: $Real = if sy < 0.0 { -1.0 } else { 1.0 };
                let singular_values = <$Vec2>::new(sx, sy * sy_sign);

                let a2 = h.atan2(e);
                // When r ≈ 0 (pure rotation case), a1 is arbitrary. Set a1 = a2
                // so that theta = 0 and phi = a2 (the rotation angle).
                let a1 = if r < 1e-10 { a2 } else { g.atan2(f) };
                let theta = (a2 - a1) * 0.5;
                let phi = (a2 + a1) * 0.5;
                let st = theta.sin();
                let ct = theta.cos();
                let sp = phi.sin();
                let cp = phi.cos();

                let u = <$Mat2>::from_cols(<$Vec2>::new(cp, sp), <$Vec2>::new(-sp, cp));
                let vt = <$Mat2>::from_cols(
                    <$Vec2>::new(ct, st * sy_sign),
                    <$Vec2>::new(-st, ct * sy_sign),
                );

                Self {
                    u,
                    s: singular_values,
                    vt,
                }
            }

            /// Rebuilds the matrix this SVD is the decomposition of.
            ///
            /// Returns `U * S * V^T`.
            #[inline]
            pub fn recompose(&self) -> $Mat2 {
                let u_s = <$Mat2>::from_cols(self.u.col(0) * self.s.x, self.u.col(1) * self.s.y);
                u_s * self.vt.transpose()
            }

        }
    };
}

impl_svd2!(Svd2, glam::Mat2, glam::Vec2, f32);
impl_svd2!(DSvd2, glam::DMat2, glam::DVec2, f64);

// f32 <-> f64 conversions
impl From<Svd2> for DSvd2 {
    #[inline]
    fn from(svd: Svd2) -> Self {
        Self {
            u: svd.u.as_dmat2(),
            s: svd.s.as_dvec2(),
            vt: svd.vt.as_dmat2(),
        }
    }
}

impl From<DSvd2> for Svd2 {
    #[inline]
    fn from(svd: DSvd2) -> Self {
        Self {
            u: svd.u.as_mat2(),
            s: svd.s.as_vec2(),
            vt: svd.vt.as_mat2(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_relative_eq;

    #[test]
    fn svd_2x2_identity() {
        let mat = glam::Mat2::IDENTITY;
        let svd = Svd2::from_matrix(mat);

        assert_relative_eq!(svd.s, glam::Vec2::new(1.0, 1.0), epsilon = 1e-6);
        assert_relative_eq!(svd.recompose(), mat, epsilon = 1e-6);
    }

    #[test]
    fn svd_2x2_diagonal() {
        let mat = glam::Mat2::from_diagonal(glam::Vec2::new(3.0, 2.0));
        let svd = Svd2::from_matrix(mat);

        assert_relative_eq!(svd.s, glam::Vec2::new(3.0, 2.0), epsilon = 1e-6);
        assert_relative_eq!(svd.recompose(), mat, epsilon = 1e-6);
    }

    #[test]
    fn svd_2x2_general() {
        let mat = glam::Mat2::from_cols_array_2d(&[[4.0, 3.0], [2.0, 1.0]]);
        let svd = Svd2::from_matrix(mat);

        // Verify recomposition
        assert_relative_eq!(svd.recompose(), mat, epsilon = 1e-5);

        // Verify singular values are non-negative and sorted
        assert!(svd.s.x >= svd.s.y);
        assert!(svd.s.y >= 0.0);

        // Verify U and V^T are orthogonal
        assert_relative_eq!(
            svd.u * svd.u.transpose(),
            glam::Mat2::IDENTITY,
            epsilon = 1e-6
        );
        assert_relative_eq!(
            svd.vt * svd.vt.transpose(),
            glam::Mat2::IDENTITY,
            epsilon = 1e-6
        );
    }

    #[test]
    fn svd_2x2_rotation() {
        let angle = 0.5_f32;
        let mat = glam::Mat2::from_angle(angle);
        let svd = Svd2::from_matrix(mat);

        // A rotation matrix has singular values of 1
        assert_relative_eq!(svd.s, glam::Vec2::new(1.0, 1.0), epsilon = 1e-6);
        assert_relative_eq!(svd.recompose(), mat, epsilon = 1e-6);
    }

    #[test]
    fn svd_2x2_f64() {
        let mat = glam::DMat2::from_cols_array_2d(&[[4.0, 3.0], [2.0, 1.0]]);
        let svd = DSvd2::from_matrix(mat);

        assert_relative_eq!(svd.recompose(), mat, epsilon = 1e-10);
    }
}
