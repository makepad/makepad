//! 2D rotation types represented as unit complex numbers.

use core::ops::{Mul, MulAssign};
use simba::scalar::{ComplexField, RealField};

/// Macro to generate a 2D rotation type for a specific scalar type.
macro_rules! impl_rot2 {
    ($Rot2:ident, $Real:ty, $Vec2:ty, $Mat2:ty $(, #[$attr:meta])*) => {
        #[doc = concat!("A 2D rotation represented as a unit complex number (", stringify!($Real), " precision).")]
        ///
        /// The rotation is stored as `re + im * i` where `re = cos(angle)` and `im = sin(angle)`.
        #[derive(Copy, Clone, Debug, PartialEq)]
        #[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
        #[cfg_attr(feature = "bytemuck", derive(bytemuck::Pod, bytemuck::Zeroable))]
        #[cfg_attr(feature = "rkyv", derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize))]
        #[repr(C)]
        $(#[$attr])*
        pub struct $Rot2 {
            /// Real part (cosine of the angle).
            pub re: $Real,
            /// Imaginary part (sine of the angle).
            pub im: $Real,
        }

        impl $Rot2 {
            /// The identity rotation (no rotation).
            pub const IDENTITY: Self = Self { re: 1.0, im: 0.0 };

            /// Creates a new unit complex from cosine and sine values.
            ///
            /// The caller must ensure that `re*re + im*im = 1`.
            #[inline]
            pub const fn from_cos_sin_unchecked(re: $Real, im: $Real) -> Self {
                Self { re, im }
            }

            /// Creates a rotation from a rotation matrix (without normalization).
            #[inline]
            pub fn from_matrix_unchecked(mat: $Mat2) -> Self {
                Self {
                    re: mat.x_axis.x,
                    im: mat.x_axis.y,
                }
            }

            /// Creates the identity rotation.
            #[inline]
            pub fn identity() -> Self {
                Self::IDENTITY
            }

            /// Creates a rotation from an angle in radians.
            #[inline]
            pub fn new(angle: $Real) -> Self {
                let (im, re) = <$Real as ComplexField>::sin_cos(angle);
                Self { re, im }
            }

            /// Creates a rotation from an angle in radians.
            #[inline]
            pub fn from_angle(angle: $Real) -> Self {
                Self::new(angle)
            }

            /// Returns the rotation angle in radians.
            #[inline]
            pub fn angle(&self) -> $Real {
                RealField::atan2(self.im, self.re)
            }

            /// Returns the cosine of the rotation angle (the real part).
            #[inline]
            pub fn cos(&self) -> $Real {
                self.re
            }

            /// Returns the sine of the rotation angle (the imaginary part).
            #[inline]
            pub fn sin(&self) -> $Real {
                self.im
            }

            /// Returns the inverse rotation.
            #[inline]
            pub fn inverse(self) -> Self {
                Self {
                    re: self.re,
                    im: -self.im,
                }
            }

            /// Computes the length (magnitude) of `self`.
            ///
            /// For a valid unit rotation, this should always be `1.0`.
            #[inline]
            pub fn length(&self) -> $Real {
                <$Real as ComplexField>::sqrt(self.re * self.re + self.im * self.im)
            }

            /// Computes the squared length of `self`.
            ///
            /// This is faster than `length()` as it avoids a square root.
            /// For a valid unit rotation, this should always be `1.0`.
            #[inline]
            pub fn length_squared(&self) -> $Real {
                self.re * self.re + self.im * self.im
            }

            /// Computes `1.0 / length()`.
            #[inline]
            pub fn length_recip(&self) -> $Real {
                1.0 / self.length()
            }

            /// Returns `self` normalized to length `1.0`.
            ///
            /// For valid unit rotations this should be a no-op.
            #[inline]
            pub fn normalize(&self) -> Self {
                let len = self.length();
                if len > <$Real>::EPSILON {
                    Self {
                        re: self.re / len,
                        im: self.im / len,
                    }
                } else {
                    Self::IDENTITY
                }
            }

            /// Returns `true` if, and only if, all elements are finite.
            #[inline]
            pub fn is_finite(&self) -> bool {
                self.re.is_finite() && self.im.is_finite()
            }

            /// Returns `true` if any element is `NaN`.
            #[inline]
            pub fn is_nan(&self) -> bool {
                self.re.is_nan() || self.im.is_nan()
            }

            /// Returns whether `self` is of length `1.0` or not.
            ///
            /// Uses a precision threshold of approximately `1e-4`.
            #[inline]
            pub fn is_normalized(&self) -> bool {
                (self.length_squared() - 1.0).abs() < 2e-4
            }

            /// Rotates a 2D vector by this rotation.
            #[inline]
            pub fn transform_vector(&self, v: $Vec2) -> $Vec2 {
                <$Vec2>::new(self.re * v.x - self.im * v.y, self.im * v.x + self.re * v.y)
            }

            /// Transforms a 2D vector by the inverse of this rotation.
            #[inline]
            pub fn inverse_transform_vector(&self, v: $Vec2) -> $Vec2 {
                <$Vec2>::new(
                    self.re * v.x + self.im * v.y,
                    -self.im * v.x + self.re * v.y,
                )
            }

            /// Returns the rotation matrix equivalent to this rotation.
            #[inline]
            pub fn to_mat(&self) -> $Mat2 {
                <$Mat2>::from_cols(
                    <$Vec2>::new(self.re, self.im),
                    <$Vec2>::new(-self.im, self.re),
                )
            }

            /// Creates a rotation from a 2x2 rotation matrix.
            ///
            /// The matrix should be a valid rotation matrix (orthogonal with determinant 1).
            #[inline]
            pub fn from_mat(mat: $Mat2) -> Self {
                Self::from_mat_unchecked(mat).normalize()
            }

            /// Creates a rotation from a 2x2 matrix without normalization.
            ///
            /// Alias for `from_matrix_unchecked`.
            #[inline]
            pub fn from_mat_unchecked(mat: $Mat2) -> Self {
                Self::from_matrix_unchecked(mat)
            }

            /// Gets the minimal rotation for transforming `from` to `to`.
            ///
            /// Both vectors must be normalized. The rotation is in the plane
            /// spanned by the two vectors.
            #[inline]
            pub fn from_rotation_arc(from: $Vec2, to: $Vec2) -> Self {
                // For unit vectors, the rotation from a to b has:
                // re = a . b = cos(angle)
                // im = a x b = sin(angle) (2D cross product is scalar)
                let re = from.dot(to);
                let im = from.x * to.y - from.y * to.x;
                Self { re, im }
            }

            /// Spherical linear interpolation between two rotations.
            #[inline]
            pub fn slerp(&self, other: &Self, t: $Real) -> Self {
                let angle_diff = other.angle() - self.angle();
                Self::new(self.angle() + t * angle_diff)
            }

            /// Raises this rotation to a power.
            ///
            /// For example, `powf(2.0)` will return a rotation with double the angle.
            #[inline]
            pub fn powf(&self, n: $Real) -> Self {
                Self::new(self.angle() * n)
            }

            /// Rotates `self` towards `rhs` up to `max_angle` (in radians).
            ///
            /// When `max_angle` is `0.0`, the result will be equal to `self`. When `max_angle` is
            /// equal to `self.angle_between(rhs)`, the result will be equal to `rhs`. If
            /// `max_angle` is negative, rotates towards the exact opposite of `rhs`.
            /// Will not go past the target.
            #[inline]
            pub fn rotate_towards(&self, rhs: &Self, max_angle: $Real) -> Self {
                let angle_between = self.angle_between(rhs);
                if angle_between.abs() <= max_angle.abs() {
                    *rhs
                } else {
                    self.slerp(rhs, max_angle / angle_between)
                }
            }

            /// Returns the angle (in radians) between `self` and `rhs`.
            #[inline]
            pub fn angle_between(&self, rhs: &Self) -> $Real {
                let diff = self.inverse() * *rhs;
                diff.angle()
            }

            /// Computes the dot product of `self` and `rhs`.
            ///
            /// The dot product is equal to the cosine of the angle between two rotations.
            #[inline]
            pub fn dot(&self, rhs: Self) -> $Real {
                self.re * rhs.re + self.im * rhs.im
            }

            /// Performs a linear interpolation between `self` and `rhs` based on
            /// the value `s`.
            ///
            /// When `s` is `0.0`, the result will be equal to `self`. When `s` is
            /// `1.0`, the result will be equal to `rhs`. For rotations, `slerp` is
            /// usually preferred over `lerp`.
            #[inline]
            pub fn lerp(&self, rhs: Self, s: $Real) -> Self {
                Self {
                    re: self.re + (rhs.re - self.re) * s,
                    im: self.im + (rhs.im - self.im) * s,
                }
            }

            /// Normalizes `self` in place.
            ///
            /// Does nothing if `self` is already normalized or zero length.
            #[inline]
            pub fn normalize_mut(&mut self) {
                *self = self.normalize();
            }
        }

        // Rot2 * Rot2
        impl Mul<$Rot2> for $Rot2 {
            type Output = Self;

            #[inline]
            fn mul(self, rhs: Self) -> Self::Output {
                // Complex multiplication: (a + bi)(c + di) = (ac - bd) + (ad + bc)i
                Self {
                    re: self.re * rhs.re - self.im * rhs.im,
                    im: self.re * rhs.im + self.im * rhs.re,
                }
            }
        }

        // &Rot2 * Rot2
        impl Mul<$Rot2> for &$Rot2 {
            type Output = $Rot2;

            #[inline]
            fn mul(self, rhs: $Rot2) -> Self::Output {
                *self * rhs
            }
        }

        impl MulAssign<$Rot2> for $Rot2 {
            #[inline]
            fn mul_assign(&mut self, rhs: Self) {
                *self = *self * rhs;
            }
        }

        // Rot2 * Vec2
        impl Mul<$Vec2> for $Rot2 {
            type Output = $Vec2;

            #[inline]
            fn mul(self, rhs: $Vec2) -> Self::Output {
                self.transform_vector(rhs)
            }
        }

        impl Default for $Rot2 {
            fn default() -> Self {
                Self::identity()
            }
        }

        #[cfg(feature = "approx")]
        impl approx::AbsDiffEq for $Rot2 {
            type Epsilon = $Real;

            fn default_epsilon() -> Self::Epsilon {
                <$Real>::EPSILON
            }

            fn abs_diff_eq(&self, other: &Self, epsilon: Self::Epsilon) -> bool {
                (self.re - other.re).abs() <= epsilon && (self.im - other.im).abs() <= epsilon
            }
        }

        #[cfg(feature = "approx")]
        impl approx::RelativeEq for $Rot2 {
            fn default_max_relative() -> Self::Epsilon {
                <$Real>::EPSILON
            }

            fn relative_eq(
                &self,
                other: &Self,
                epsilon: Self::Epsilon,
                max_relative: Self::Epsilon,
            ) -> bool {
                approx::AbsDiffEq::abs_diff_eq(self, other, epsilon.max(max_relative))
            }
        }

        #[cfg(feature = "approx")]
        impl approx::UlpsEq for $Rot2 {
            fn default_max_ulps() -> u32 {
                4
            }

            fn ulps_eq(&self, other: &Self, epsilon: Self::Epsilon, max_ulps: u32) -> bool {
                approx::AbsDiffEq::abs_diff_eq(self, other, epsilon)
                    || (self.re.to_bits().abs_diff(other.re.to_bits()) <= max_ulps as _
                        && self.im.to_bits().abs_diff(other.im.to_bits()) <= max_ulps as _)
            }
        }
    };
}

impl_rot2!(Rot2, f32, glam::Vec2, glam::Mat2);
impl_rot2!(DRot2, f64, glam::DVec2, glam::DMat2);

// f32 <-> f64 conversions
impl From<Rot2> for DRot2 {
    #[inline]
    fn from(r: Rot2) -> Self {
        Self {
            re: r.re as f64,
            im: r.im as f64,
        }
    }
}

impl From<DRot2> for Rot2 {
    #[inline]
    fn from(r: DRot2) -> Self {
        Self {
            re: r.re as f32,
            im: r.im as f32,
        }
    }
}

// Nalgebra conversions
#[cfg(feature = "nalgebra")]
mod nalgebra_conv {
    use super::*;

    impl From<Rot2> for nalgebra::UnitComplex<f32> {
        fn from(r: Rot2) -> Self {
            nalgebra::UnitComplex::from_cos_sin_unchecked(r.re, r.im)
        }
    }

    impl From<nalgebra::UnitComplex<f32>> for Rot2 {
        fn from(r: nalgebra::UnitComplex<f32>) -> Self {
            Self { re: r.re, im: r.im }
        }
    }

    impl From<DRot2> for nalgebra::UnitComplex<f64> {
        fn from(r: DRot2) -> Self {
            nalgebra::UnitComplex::from_cos_sin_unchecked(r.re, r.im)
        }
    }

    impl From<nalgebra::UnitComplex<f64>> for DRot2 {
        fn from(r: nalgebra::UnitComplex<f64>) -> Self {
            Self { re: r.re, im: r.im }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_relative_eq;
    use core::f32::consts::PI;

    #[test]
    fn test_rot2_identity() {
        let r = Rot2::IDENTITY;
        assert_eq!(r.re, 1.0);
        assert_eq!(r.im, 0.0);
        assert!(r.is_normalized());
    }

    #[test]
    fn test_rot2_new() {
        let r = Rot2::new(PI / 2.0);
        assert_relative_eq!(r.re, 0.0, epsilon = 1e-6);
        assert_relative_eq!(r.im, 1.0, epsilon = 1e-6);
        assert!(r.is_normalized());
    }

    #[test]
    fn test_rot2_angle() {
        let angle = 0.5;
        let r = Rot2::new(angle);
        assert_relative_eq!(r.angle(), angle, epsilon = 1e-6);
        assert_relative_eq!(r.angle(), angle, epsilon = 1e-6);
    }

    #[test]
    fn test_rot2_inverse() {
        let r = Rot2::new(0.5);
        let inv = r.inverse();
        let identity = r * inv;
        assert_relative_eq!(identity.re, 1.0, epsilon = 1e-6);
        assert_relative_eq!(identity.im, 0.0, epsilon = 1e-6);
    }

    #[test]
    fn test_rot2_transform_vector() {
        let r = Rot2::new(PI / 2.0);
        let v = glam::Vec2::new(1.0, 0.0);
        let result = r.transform_vector(v);
        assert_relative_eq!(result.x, 0.0, epsilon = 1e-6);
        assert_relative_eq!(result.y, 1.0, epsilon = 1e-6);
        // Operator should also work
        let result2 = r * v;
        assert_relative_eq!(result2.x, 0.0, epsilon = 1e-6);
        assert_relative_eq!(result2.y, 1.0, epsilon = 1e-6);
    }

    #[test]
    fn test_rot2_length() {
        let r = Rot2::new(0.5);
        assert_relative_eq!(r.length(), 1.0, epsilon = 1e-6);
        assert_relative_eq!(r.length_squared(), 1.0, epsilon = 1e-6);
    }

    #[test]
    fn test_rot2_from_rotation_arc() {
        let from = glam::Vec2::new(1.0, 0.0);
        let to = glam::Vec2::new(0.0, 1.0);
        let r = Rot2::from_rotation_arc(from, to);
        assert_relative_eq!(r.angle(), PI / 2.0, epsilon = 1e-6);
    }

    #[test]
    fn test_rot2_to_mat2() {
        let r = Rot2::new(PI / 4.0);
        let m = r.to_mat();
        let from_mat = Rot2::from_mat(m);
        assert_relative_eq!(r.re, from_mat.re, epsilon = 1e-6);
        assert_relative_eq!(r.im, from_mat.im, epsilon = 1e-6);
    }

    #[test]
    fn test_rot2_angle_between() {
        let r1 = Rot2::new(0.0);
        let r2 = Rot2::new(PI / 2.0);
        assert_relative_eq!(r1.angle_between(&r2), PI / 2.0, epsilon = 1e-6);
    }

    #[test]
    fn test_drot2_new() {
        let r = DRot2::new(core::f64::consts::PI / 2.0);
        assert_relative_eq!(r.re, 0.0, epsilon = 1e-10);
        assert_relative_eq!(r.im, 1.0, epsilon = 1e-10);
        assert!(r.is_normalized());
    }
}
