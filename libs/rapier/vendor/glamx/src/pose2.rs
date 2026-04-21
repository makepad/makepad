//! 2D pose types for rigid body transformations.

use crate::rot2::{DRot2, Rot2};
use core::ops::{Mul, MulAssign};

/// Macro to generate a 2D pose type for a specific scalar type.
macro_rules! impl_pose2 {
    ($Pose2:ident, $Rot2:ident, $Real:ty, $Vec2:ty, $Vec3:ty, $Mat2:ty, $Mat3:ty $(, #[$attr:meta])*) => {
        #[doc = concat!("A 2D pose (rotation + translation), representing a rigid body transformation (", stringify!($Real), " precision).")]
        #[derive(Copy, Clone, Debug, PartialEq)]
        #[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
        #[cfg_attr(feature = "rkyv", derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize))]
        $(#[$attr])*
        pub struct $Pose2 {
            /// The rotational part of the pose.
            pub rotation: $Rot2,
            /// The translational part of the pose.
            pub translation: $Vec2,
        }

        impl $Pose2 {
            /// The identity pose (no rotation, no translation).
            pub const IDENTITY: Self = Self {
                rotation: $Rot2::IDENTITY,
                translation: <$Vec2>::ZERO,
            };

            /// Creates the identity pose.
            #[inline]
            pub fn identity() -> Self {
                Self::IDENTITY
            }

            /// Creates a pose from a translation vector.
            #[inline]
            pub fn from_translation(translation: $Vec2) -> Self {
                Self {
                    rotation: $Rot2::IDENTITY,
                    translation,
                }
            }

            /// Creates a pose from translation components.
            #[inline]
            pub fn translation(x: $Real, y: $Real) -> Self {
                Self::from_translation(<$Vec2>::new(x, y))
            }

            /// Creates a pose from a rotation.
            #[inline]
            pub fn from_rotation(rotation: $Rot2) -> Self {
                Self {
                    rotation,
                    translation: <$Vec2>::ZERO,
                }
            }

            /// Creates a pose that is a pure rotation with the given angle (in radians).
            #[inline]
            pub fn rotation(angle: $Real) -> Self {
                Self::from_rotation(<$Rot2>::new(angle))
            }

            /// Creates a pose from translation and rotation parts.
            #[inline]
            pub fn from_parts(translation: $Vec2, rotation: $Rot2) -> Self {
                Self {
                    rotation,
                    translation,
                }
            }

            /// Creates a pose from translation and angle.
            #[inline]
            pub fn new(translation: $Vec2, angle: $Real) -> Self {
                Self {
                    rotation: $Rot2::new(angle),
                    translation,
                }
            }

            /// Prepends a translation to this pose (applies translation in local frame).
            #[inline]
            pub fn prepend_translation(self, translation: $Vec2) -> Self {
                Self {
                    rotation: self.rotation,
                    translation: self.translation + self.rotation * translation,
                }
            }

            /// Appends a translation to this pose (applies translation in world frame).
            #[inline]
            pub fn append_translation(self, translation: $Vec2) -> Self {
                Self {
                    rotation: self.rotation,
                    translation: self.translation + translation,
                }
            }

            /// Returns the inverse of this pose.
            #[inline]
            pub fn inverse(&self) -> Self {
                let inv_rot = self.rotation.inverse();
                Self {
                    rotation: inv_rot,
                    translation: inv_rot.transform_vector(-self.translation),
                }
            }

            /// Computes `self.inverse() * rhs`.
            #[inline]
            pub fn inv_mul(&self, rhs: &Self) -> Self {
                self.inverse() * *rhs
            }

            /// Transforms a 2D point by this pose.
            ///
            /// This applies both the rotation and translation.
            #[inline]
            pub fn transform_point(&self, p: $Vec2) -> $Vec2 {
                self.rotation.transform_vector(p) + self.translation
            }

            /// Transforms a 2D vector by this pose.
            ///
            /// This applies only the rotation (ignores translation).
            #[inline]
            pub fn transform_vector(&self, v: $Vec2) -> $Vec2 {
                self.rotation.transform_vector(v)
            }

            /// Transforms a point by the inverse of this pose.
            #[inline]
            pub fn inverse_transform_point(&self, p: $Vec2) -> $Vec2 {
                self.rotation.inverse_transform_vector(p - self.translation)
            }

            /// Transforms a vector by the inverse of this pose.
            #[inline]
            pub fn inverse_transform_vector(&self, v: $Vec2) -> $Vec2 {
                self.rotation.inverse_transform_vector(v)
            }

            /// Linearly interpolates between two poses.
            ///
            /// Uses spherical linear interpolation for the rotation part.
            #[inline]
            pub fn lerp(&self, other: &Self, t: $Real) -> Self {
                Self {
                    rotation: self.rotation.slerp(&other.rotation, t),
                    translation: self.translation.lerp(other.translation, t),
                }
            }

            /// Returns `true` if, and only if, all elements are finite.
            #[inline]
            pub fn is_finite(&self) -> bool {
                self.rotation.is_finite() && self.translation.is_finite()
            }

            /// Returns `true` if any element is `NaN`.
            #[inline]
            pub fn is_nan(&self) -> bool {
                self.rotation.is_nan() || self.translation.is_nan()
            }

            /// Converts this pose to a homogeneous 3x3 matrix.
            #[inline]
            pub fn to_mat3(&self) -> $Mat3 {
                let (sin, cos) = (self.rotation.sin(), self.rotation.cos());
                <$Mat3>::from_cols(
                    <$Vec3>::new(cos, sin, 0.0),
                    <$Vec3>::new(-sin, cos, 0.0),
                    <$Vec3>::new(self.translation.x, self.translation.y, 1.0),
                )
            }

            /// Creates a pose from a homogeneous 3x3 matrix.
            ///
            /// The matrix is assumed to represent a rigid body transformation
            /// (rotation + translation only, no scaling or shearing).
            #[inline]
            pub fn from_mat3(mat: $Mat3) -> Self {
                let rotation = $Rot2::from_mat(<$Mat2>::from_cols(
                    mat.x_axis.truncate(),
                    mat.y_axis.truncate(),
                ));
                let translation = mat.z_axis.truncate();
                Self { rotation, translation }
            }
        }

        // Pose2 * Pose2
        impl Mul<$Pose2> for $Pose2 {
            type Output = Self;

            #[inline]
            fn mul(self, rhs: Self) -> Self::Output {
                Self {
                    rotation: self.rotation * rhs.rotation,
                    translation: self.translation + self.rotation.transform_vector(rhs.translation),
                }
            }
        }

        impl MulAssign<$Pose2> for $Pose2 {
            #[inline]
            fn mul_assign(&mut self, rhs: Self) {
                *self = *self * rhs;
            }
        }

        // Pose2 * Vec2 (transform point - applies rotation and translation)
        impl Mul<$Vec2> for $Pose2 {
            type Output = $Vec2;

            #[inline]
            fn mul(self, rhs: $Vec2) -> Self::Output {
                self.transform_point(rhs)
            }
        }

        impl Mul<$Vec2> for &$Pose2 {
            type Output = $Vec2;

            #[inline]
            fn mul(self, rhs: $Vec2) -> Self::Output {
                self.transform_point(rhs)
            }
        }

        impl Mul<&$Vec2> for $Pose2 {
            type Output = $Vec2;

            #[inline]
            fn mul(self, rhs: &$Vec2) -> Self::Output {
                self.transform_point(*rhs)
            }
        }

        impl Default for $Pose2 {
            fn default() -> Self {
                Self::IDENTITY
            }
        }

        // &Pose2 * &Pose2
        impl Mul<&$Pose2> for &$Pose2 {
            type Output = $Pose2;

            #[inline]
            fn mul(self, rhs: &$Pose2) -> Self::Output {
                *self * *rhs
            }
        }

        // Pose2 * &Pose2
        impl Mul<&$Pose2> for $Pose2 {
            type Output = $Pose2;

            #[inline]
            fn mul(self, rhs: &$Pose2) -> Self::Output {
                self * *rhs
            }
        }

        // &Pose2 * Pose2
        impl Mul<$Pose2> for &$Pose2 {
            type Output = $Pose2;

            #[inline]
            fn mul(self, rhs: $Pose2) -> Self::Output {
                *self * rhs
            }
        }

        impl Mul<$Pose2> for $Rot2 {
            type Output = $Pose2;

            #[inline]
            fn mul(self, rhs: $Pose2) -> Self::Output {
                $Pose2 {
                    translation: self * rhs.translation,
                    rotation: self * rhs.rotation,
                }
            }
        }

        impl Mul<$Rot2> for $Pose2 {
            type Output = $Pose2;

            #[inline]
            fn mul(self, rhs: $Rot2) -> Self::Output {
                $Pose2 {
                    translation: self.translation,
                    rotation: self.rotation * rhs,
                }
            }
        }

        impl MulAssign<$Rot2> for $Pose2 {
            #[inline]
            fn mul_assign(&mut self, rhs: $Rot2) {
                *self = *self * rhs;
            }
        }

        impl From<$Rot2> for $Pose2 {
            #[inline]
            fn from(rotation: $Rot2) -> Self {
                Self::from_rotation(rotation)
            }
        }

        #[cfg(feature = "approx")]
        impl approx::AbsDiffEq for $Pose2 {
            type Epsilon = $Real;

            fn default_epsilon() -> Self::Epsilon {
                <$Real>::EPSILON
            }

            fn abs_diff_eq(&self, other: &Self, epsilon: Self::Epsilon) -> bool {
                self.rotation.abs_diff_eq(&other.rotation, epsilon)
                    && self.translation.abs_diff_eq(other.translation, epsilon)
            }
        }

        #[cfg(feature = "approx")]
        impl approx::RelativeEq for $Pose2 {
            fn default_max_relative() -> Self::Epsilon {
                <$Real>::EPSILON
            }

            fn relative_eq(
                &self,
                other: &Self,
                epsilon: Self::Epsilon,
                max_relative: Self::Epsilon,
            ) -> bool {
                self.rotation.relative_eq(&other.rotation, epsilon, max_relative)
                    && self.translation.abs_diff_eq(other.translation, epsilon.max(max_relative))
            }
        }
    };
}

impl_pose2!(
    Pose2,
    Rot2,
    f32,
    glam::Vec2,
    glam::Vec3,
    glam::Mat2,
    glam::Mat3
);
impl_pose2!(
    DPose2,
    DRot2,
    f64,
    glam::DVec2,
    glam::DVec3,
    glam::DMat2,
    glam::DMat3
);

// f32 <-> f64 conversions
impl From<Pose2> for DPose2 {
    #[inline]
    fn from(p: Pose2) -> Self {
        Self {
            rotation: p.rotation.into(),
            translation: p.translation.into(),
        }
    }
}

impl From<DPose2> for Pose2 {
    #[inline]
    fn from(p: DPose2) -> Self {
        Self {
            rotation: p.rotation.into(),
            translation: p.translation.as_vec2(),
        }
    }
}

// Nalgebra conversions
#[cfg(feature = "nalgebra")]
mod nalgebra_conv {
    use super::*;

    impl From<Pose2> for nalgebra::Isometry2<f32> {
        fn from(p: Pose2) -> Self {
            nalgebra::Isometry2 {
                translation: p.translation.into(),
                rotation: p.rotation.into(),
            }
        }
    }

    impl From<nalgebra::Isometry2<f32>> for Pose2 {
        fn from(iso: nalgebra::Isometry2<f32>) -> Self {
            Self {
                rotation: Rot2::from_cos_sin_unchecked(iso.rotation.re, iso.rotation.im),
                translation: glam::Vec2::new(iso.translation.x, iso.translation.y),
            }
        }
    }

    impl From<DPose2> for nalgebra::Isometry2<f64> {
        fn from(p: DPose2) -> Self {
            nalgebra::Isometry2 {
                translation: p.translation.into(),
                rotation: p.rotation.into(),
            }
        }
    }

    impl From<nalgebra::Isometry2<f64>> for DPose2 {
        fn from(iso: nalgebra::Isometry2<f64>) -> Self {
            Self {
                rotation: DRot2::from_cos_sin_unchecked(iso.rotation.re, iso.rotation.im),
                translation: glam::DVec2::new(iso.translation.x, iso.translation.y),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use approx::assert_relative_eq;
    use core::f32::consts::PI;

    #[test]
    fn test_pose2_identity() {
        let p = Pose2::IDENTITY;
        assert_eq!(p.rotation, Rot2::IDENTITY);
        assert_eq!(p.translation, glam::Vec2::ZERO);
        assert!(p.is_finite());
    }

    #[test]
    fn test_pose2_from_translation() {
        let t = glam::Vec2::new(1.0, 2.0);
        let p = Pose2::from_translation(t);
        assert_eq!(p.translation, t);
        assert_eq!(p.rotation, Rot2::IDENTITY);
    }

    #[test]
    fn test_pose2_from_rotation_translation() {
        let t = glam::Vec2::new(1.0, 2.0);
        let r = Rot2::new(PI / 4.0);
        let p = Pose2::from_parts(t, r);
        assert_eq!(p.translation, t);
        assert_eq!(p.rotation, r);
    }

    #[test]
    fn test_pose2_transform_point() {
        let p = Pose2::new(glam::Vec2::new(1.0, 0.0), PI / 2.0);
        let point = glam::Vec2::new(1.0, 0.0);
        let result = p.transform_point(point);
        assert_relative_eq!(result.x, 1.0, epsilon = 1e-6);
        assert_relative_eq!(result.y, 1.0, epsilon = 1e-6);
        // Operator should also work
        let result2 = p * point;
        assert_relative_eq!(result2.x, 1.0, epsilon = 1e-6);
        assert_relative_eq!(result2.y, 1.0, epsilon = 1e-6);
    }

    #[test]
    fn test_pose2_transform_vector() {
        let p = Pose2::new(glam::Vec2::new(100.0, 200.0), PI / 2.0);
        let vector = glam::Vec2::new(1.0, 0.0);
        // Vector transformation ignores translation
        let result = p.transform_vector(vector);
        assert_relative_eq!(result.x, 0.0, epsilon = 1e-6);
        assert_relative_eq!(result.y, 1.0, epsilon = 1e-6);
    }

    #[test]
    fn test_pose2_inverse() {
        let p = Pose2::new(glam::Vec2::new(1.0, 2.0), 0.5);
        let inv = p.inverse();
        let identity = p * inv;
        assert_relative_eq!(identity.rotation.re, 1.0, epsilon = 1e-6);
        assert_relative_eq!(identity.rotation.im, 0.0, epsilon = 1e-6);
        assert_relative_eq!(identity.translation.x, 0.0, epsilon = 1e-6);
        assert_relative_eq!(identity.translation.y, 0.0, epsilon = 1e-6);
    }

    #[test]
    fn test_pose2_to_from_mat3() {
        let p = Pose2::new(glam::Vec2::new(1.0, 2.0), PI / 4.0);
        let m = p.to_mat3();
        let p2 = Pose2::from_mat3(m);
        assert_relative_eq!(p.rotation.re, p2.rotation.re, epsilon = 1e-6);
        assert_relative_eq!(p.rotation.im, p2.rotation.im, epsilon = 1e-6);
        assert_relative_eq!(p.translation.x, p2.translation.x, epsilon = 1e-6);
        assert_relative_eq!(p.translation.y, p2.translation.y, epsilon = 1e-6);
    }

    #[test]
    fn test_dpose2_new() {
        let p = DPose2::new(glam::DVec2::new(1.0, 2.0), core::f64::consts::PI / 4.0);
        assert_relative_eq!(p.translation.x, 1.0, epsilon = 1e-10);
        assert_relative_eq!(p.translation.y, 2.0, epsilon = 1e-10);
    }
}
