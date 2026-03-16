//! 3D pose types for rigid body transformations.

use crate::rot3::{DRot3, Rot3};
use core::ops::{Mul, MulAssign};

/// Macro to generate a 3D pose type for a specific scalar type.
macro_rules! impl_pose3 {
    ($Pose3:ident, $Rot3:ident, $Real:ty, $Vec3:ty, $Mat4: ty $(, #[$attr:meta])*) => {
        #[doc = concat!("A 3D pose (rotation + translation), representing a rigid body transformation (", stringify!($Real), " precision).")]
        #[derive(Copy, Clone, Debug, PartialEq)]
        #[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
        #[cfg_attr(feature = "rkyv", derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize))]
        $(#[$attr])*
        pub struct $Pose3 {
            /// The rotational part of the pose.
            pub rotation: $Rot3,
            /// The translational part of the pose.
            pub translation: $Vec3,
        }

        impl $Pose3 {
            /// The identity pose (no rotation, no translation).
            pub const IDENTITY: Self = Self {
                rotation: $Rot3::IDENTITY,
                translation: <$Vec3>::ZERO,
            };

            /// Creates the identity pose.
            #[inline]
            pub fn identity() -> Self {
                Self::IDENTITY
            }

            /// Creates a pose from a translation vector.
            #[inline]
            pub fn from_translation(translation: $Vec3) -> Self {
                Self {
                    rotation: $Rot3::IDENTITY,
                    translation,
                }
            }

            /// Creates a pose from translation components.
            #[inline]
            pub fn translation(x: $Real, y: $Real, z: $Real) -> Self {
                Self::from_translation(<$Vec3>::new(x, y, z))
            }

            /// Creates a pose from a rotation.
            #[inline]
            pub fn from_rotation(rotation: $Rot3) -> Self {
                Self {
                    rotation,
                    translation: <$Vec3>::ZERO,
                }
            }

            /// Creates a pose from translation and rotation parts.
            #[inline]
            pub fn from_parts(translation: $Vec3, rotation: $Rot3) -> Self {
                Self {
                    rotation,
                    translation,
                }
            }

            /// Creates a pose from translation and axis-angle rotation.
            #[inline]
            pub fn new(translation: $Vec3, axisangle: $Vec3) -> Self {
                let rotation = $Rot3::from_scaled_axis(axisangle.into());
                Self {
                    rotation,
                    translation,
                }
            }

            /// Creates a pose from axis-angle rotation only (no translation).
            #[inline]
            pub fn rotation(axisangle: $Vec3) -> Self {
                Self {
                    rotation: $Rot3::from_scaled_axis(axisangle.into()),
                    translation: <$Vec3>::ZERO,
                }
            }

            /// Prepends a translation to this pose (applies translation in local frame).
            #[inline]
            pub fn prepend_translation(self, translation: $Vec3) -> Self {
                Self {
                    rotation: self.rotation,
                    translation: self.translation + self.rotation * translation,
                }
            }

            /// Appends a translation to this pose (applies translation in world frame).
            #[inline]
            pub fn append_translation(self, translation: $Vec3) -> Self {
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
                    translation: inv_rot * (-self.translation),
                }
            }

            /// Computes `self.inverse() * rhs`.
            #[inline]
            pub fn inv_mul(&self, rhs: &Self) -> Self {
                let inv_rot1 = self.rotation.inverse();
                let tr_12 = rhs.translation - self.translation;
                $Pose3 {
                    translation: inv_rot1 * tr_12,
                    rotation: inv_rot1 * rhs.rotation,
                }
            }

            /// Transforms a 3D point by this pose.
            ///
            /// This applies both the rotation and translation.
            #[inline]
            pub fn transform_point(&self, p: $Vec3) -> $Vec3 {
                self.rotation * p + self.translation
            }

            /// Transforms a 3D vector by this pose.
            ///
            /// This applies only the rotation (ignores translation).
            #[inline]
            pub fn transform_vector(&self, v: $Vec3) -> $Vec3 {
                self.rotation * v
            }

            /// Transforms a point by the inverse of this pose.
            #[inline]
            pub fn inverse_transform_point(&self, p: $Vec3) -> $Vec3 {
                self.rotation.inverse() * (p - self.translation)
            }

            /// Transforms a vector by the inverse of this pose.
            #[inline]
            pub fn inverse_transform_vector(&self, v: $Vec3) -> $Vec3 {
                self.rotation.inverse() * v
            }

            /// Linearly interpolates between two poses.
            ///
            /// Uses spherical linear interpolation for the rotation part.
            #[inline]
            pub fn lerp(&self, other: &Self, t: $Real) -> Self {
                Self {
                    rotation: self.rotation.slerp(other.rotation, t),
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

            /// Converts this pose to a homogeneous 4x4 matrix.
            #[inline]
            pub fn to_mat4(&self) -> $Mat4 {
                <$Mat4>::from_rotation_translation(self.rotation, self.translation.into())
            }

            /// Creates a pose from a homogeneous 4x4 matrix.
            ///
            /// The matrix is assumed to represent a rigid body transformation
            /// (rotation + translation only, no scaling or shearing).
            #[inline]
            pub fn from_mat4(mat: $Mat4) -> Self {
                let (_, rotation, translation) = mat.to_scale_rotation_translation();
                Self {
                    rotation,
                    translation: translation.into(),
                }
            }

            /// Builds a right-handed look-at view matrix.
            ///
            /// It maps the view direction target - eye to the negative z axis to and the eye to the
            /// origin. This conforms to the common notion of right-handed camera look-at view matrix from the computer graphics community, i.e. the camera is assumed to look toward its local -z axis.
            ///
            /// # Arguments
            /// - `eye`: The eye position.
            /// - `target`: The target position.
            /// - `up`: A vector approximately aligned with required the vertical axis. The only
            ///   requirement of this parameter is to not be collinear to target - eye.
            #[inline]
            pub fn look_at_rh(eye: $Vec3, target: $Vec3, up: $Vec3) -> $Pose3 {
                let rotation = <$Rot3>::look_at_rh(eye.into(), target.into(), up.into());
                let translation = rotation * (-eye);

                $Pose3 {
                    rotation,
                    translation,
                }
            }

            /// Creates a pose that corresponds to the local frame of an observer standing at the
            /// point eye and looking toward target.
            ///
            /// It maps the `z` axis to the view direction target - eye and the origin to the eye.
            ///
            /// # Arguments
            /// - `eye`: The observer position.
            /// - `target`: The target position.
            /// - `up`: Vertical direction. The only requirement of this parameter is to not be
            ///   collinear to eye - at. Non-collinearity is not checked.
            #[inline]
            pub fn face_towards(eye: $Vec3, target: $Vec3, up: $Vec3) -> $Pose3 {
                $Pose3 {
                    rotation: <$Rot3>::look_at_lh(eye.into(), target.into(), up.into()).inverse(),
                    translation: eye,
                }
            }
        }

        // Pose3 * Pose3
        impl Mul<$Pose3> for $Pose3 {
            type Output = Self;

            #[inline]
            fn mul(self, rhs: Self) -> Self::Output {
                Self {
                    rotation: self.rotation * rhs.rotation,
                    translation: self.translation + self.rotation * rhs.translation,
                }
            }
        }

        impl MulAssign<$Pose3> for $Pose3 {
            #[inline]
            fn mul_assign(&mut self, rhs: Self) {
                *self = *self * rhs;
            }
        }

        // Pose3 * Vec3 (transform point - applies rotation and translation)
        impl Mul<$Vec3> for $Pose3 {
            type Output = $Vec3;

            #[inline]
            fn mul(self, rhs: $Vec3) -> Self::Output {
                self.transform_point(rhs)
            }
        }

        impl Mul<$Vec3> for &$Pose3 {
            type Output = $Vec3;

            #[inline]
            fn mul(self, rhs: $Vec3) -> Self::Output {
                self.transform_point(rhs)
            }
        }

        impl Mul<&$Vec3> for $Pose3 {
            type Output = $Vec3;

            #[inline]
            fn mul(self, rhs: &$Vec3) -> Self::Output {
                self.transform_point(*rhs)
            }
        }

        // &Pose3 * &Pose3
        impl Mul<&$Pose3> for &$Pose3 {
            type Output = $Pose3;

            #[inline]
            fn mul(self, rhs: &$Pose3) -> Self::Output {
                *self * *rhs
            }
        }

        // Pose3 * &Pose3
        impl Mul<&$Pose3> for $Pose3 {
            type Output = $Pose3;

            #[inline]
            fn mul(self, rhs: &$Pose3) -> Self::Output {
                self * *rhs
            }
        }

        // &Pose3 * Pose3
        impl Mul<$Pose3> for &$Pose3 {
            type Output = $Pose3;

            #[inline]
            fn mul(self, rhs: $Pose3) -> Self::Output {
                *self * rhs
            }
        }

        impl Default for $Pose3 {
            fn default() -> Self {
                Self::IDENTITY
            }
        }

        impl Mul<$Pose3> for $Rot3 {
            type Output = $Pose3;

            #[inline]
            fn mul(self, rhs: $Pose3) -> Self::Output {
                $Pose3 {
                    translation: self * rhs.translation,
                    rotation: self * rhs.rotation,
                }
            }
        }

        impl Mul<$Rot3> for $Pose3 {
            type Output = $Pose3;

            #[inline]
            fn mul(self, rhs: $Rot3) -> Self::Output {
                $Pose3 {
                    translation: self.translation,
                    rotation: self.rotation * rhs,
                }
            }
        }

        impl MulAssign<$Rot3> for $Pose3 {
            #[inline]
            fn mul_assign(&mut self, rhs: $Rot3) {
                *self = *self * rhs;
            }
        }

        impl From<$Rot3> for $Pose3 {
            #[inline]
            fn from(rotation: $Rot3) -> Self {
                Self::from_rotation(rotation)
            }
        }

        #[cfg(feature = "approx")]
        impl approx::AbsDiffEq for $Pose3 {
            type Epsilon = $Real;

            fn default_epsilon() -> Self::Epsilon {
                <$Real>::EPSILON
            }

            fn abs_diff_eq(&self, other: &Self, epsilon: Self::Epsilon) -> bool {
                self.rotation.abs_diff_eq(other.rotation, epsilon)
                    && self.translation.abs_diff_eq(other.translation, epsilon)
            }
        }

        #[cfg(feature = "approx")]
        impl approx::RelativeEq for $Pose3 {
            fn default_max_relative() -> Self::Epsilon {
                <$Real>::EPSILON
            }

            fn relative_eq(
                &self,
                other: &Self,
                epsilon: Self::Epsilon,
                max_relative: Self::Epsilon,
            ) -> bool {
                self.rotation.abs_diff_eq(other.rotation, epsilon.max(max_relative))
                    && self.translation.abs_diff_eq(other.translation, epsilon.max(max_relative))
            }
        }
    };
}

impl_pose3!(Pose3, Rot3, f32, glam::Vec3, glam::Mat4);
impl_pose3!(Pose3A, Rot3, f32, glam::Vec3A, glam::Mat4);
impl_pose3!(DPose3, DRot3, f64, glam::DVec3, glam::DMat4);

// f32 <-> f64 conversions
impl From<Pose3> for DPose3 {
    #[inline]
    fn from(p: Pose3) -> Self {
        Self {
            rotation: p.rotation.as_dquat(),
            translation: p.translation.as_dvec3(),
        }
    }
}

impl From<DPose3> for Pose3 {
    #[inline]
    fn from(p: DPose3) -> Self {
        Self {
            rotation: p.rotation.as_quat(),
            translation: p.translation.as_vec3(),
        }
    }
}

impl From<Pose3A> for DPose3 {
    #[inline]
    fn from(p: Pose3A) -> Self {
        Self {
            rotation: p.rotation.as_dquat(),
            translation: p.translation.as_dvec3(),
        }
    }
}

impl From<DPose3> for Pose3A {
    #[inline]
    fn from(p: DPose3) -> Self {
        Self {
            rotation: p.rotation.as_quat(),
            translation: p.translation.as_vec3a(),
        }
    }
}

// Pose3 <-> Pose3A conversions
impl From<Pose3> for Pose3A {
    #[inline]
    fn from(p: Pose3) -> Self {
        Self {
            rotation: p.rotation,
            translation: p.translation.into(),
        }
    }
}

impl From<Pose3A> for Pose3 {
    #[inline]
    fn from(p: Pose3A) -> Self {
        Self {
            rotation: p.rotation,
            translation: p.translation.into(),
        }
    }
}

// Nalgebra conversions
#[cfg(feature = "nalgebra")]
mod nalgebra_conv {
    use super::*;

    impl From<Pose3> for nalgebra::Isometry3<f32> {
        fn from(p: Pose3) -> Self {
            nalgebra::Isometry3 {
                translation: p.translation.into(),
                rotation: p.rotation.into(),
            }
        }
    }

    impl From<nalgebra::Isometry3<f32>> for Pose3 {
        fn from(iso: nalgebra::Isometry3<f32>) -> Self {
            Self {
                rotation: glam::Quat::from_xyzw(
                    iso.rotation.i,
                    iso.rotation.j,
                    iso.rotation.k,
                    iso.rotation.w,
                ),
                translation: glam::Vec3::new(
                    iso.translation.x,
                    iso.translation.y,
                    iso.translation.z,
                ),
            }
        }
    }

    impl From<DPose3> for nalgebra::Isometry3<f64> {
        fn from(p: DPose3) -> Self {
            nalgebra::Isometry3 {
                translation: p.translation.into(),
                rotation: p.rotation.into(),
            }
        }
    }

    impl From<nalgebra::Isometry3<f64>> for DPose3 {
        fn from(iso: nalgebra::Isometry3<f64>) -> Self {
            Self {
                rotation: iso.rotation.into(),
                translation: iso.translation.into(),
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
    fn test_pose3_identity() {
        let p = Pose3::IDENTITY;
        assert_eq!(p.rotation, Rot3::IDENTITY);
        assert_eq!(p.translation, glam::Vec3::ZERO);
        assert!(p.is_finite());
    }

    #[test]
    fn test_pose3_from_translation() {
        let t = glam::Vec3::new(1.0, 2.0, 3.0);
        let p = Pose3::from_translation(t);
        assert_eq!(p.translation, t);
        assert_eq!(p.rotation, Rot3::IDENTITY);
    }

    #[test]
    fn test_pose3_from_rotation_translation() {
        let t = glam::Vec3::new(1.0, 2.0, 3.0);
        let r = Rot3::from_rotation_z(PI / 4.0);
        let p = Pose3::from_parts(t, r);
        assert_eq!(p.translation, t);
        assert_eq!(p.rotation, r);
    }

    #[test]
    fn test_pose3_transform_point() {
        let axisangle = glam::Vec3::new(0.0, 0.0, PI / 2.0);
        let p = Pose3::new(glam::Vec3::new(1.0, 0.0, 0.0), axisangle);
        let point = glam::Vec3::new(1.0, 0.0, 0.0);
        let result = p.transform_point(point);
        assert_relative_eq!(result.x, 1.0, epsilon = 1e-6);
        assert_relative_eq!(result.y, 1.0, epsilon = 1e-6);
        assert_relative_eq!(result.z, 0.0, epsilon = 1e-6);
        // Operator should also work
        let result2 = p * point;
        assert_relative_eq!(result2.x, 1.0, epsilon = 1e-6);
        assert_relative_eq!(result2.y, 1.0, epsilon = 1e-6);
        assert_relative_eq!(result2.z, 0.0, epsilon = 1e-6);
    }

    #[test]
    fn test_pose3_transform_vector() {
        let axisangle = glam::Vec3::new(0.0, 0.0, PI / 2.0);
        let p = Pose3::new(glam::Vec3::new(100.0, 200.0, 300.0), axisangle);
        let vector = glam::Vec3::new(1.0, 0.0, 0.0);
        // Vector transformation ignores translation
        let result = p.transform_vector(vector);
        assert_relative_eq!(result.x, 0.0, epsilon = 1e-6);
        assert_relative_eq!(result.y, 1.0, epsilon = 1e-6);
        assert_relative_eq!(result.z, 0.0, epsilon = 1e-6);
    }

    #[test]
    fn test_pose3_inverse() {
        let axisangle = glam::Vec3::new(0.1, 0.2, 0.3);
        let p = Pose3::new(axisangle, glam::Vec3::new(1.0, 2.0, 3.0));
        let inv = p.inverse();
        let identity = p * inv;
        assert_relative_eq!(identity.rotation, Rot3::IDENTITY, epsilon = 1e-6);
        assert_relative_eq!(identity.translation.x, 0.0, epsilon = 1e-6);
        assert_relative_eq!(identity.translation.y, 0.0, epsilon = 1e-6);
        assert_relative_eq!(identity.translation.z, 0.0, epsilon = 1e-6);
    }

    #[test]
    fn test_pose3_to_from_mat4() {
        let axisangle = glam::Vec3::new(0.1, 0.2, 0.3);
        let p = Pose3::new(axisangle, glam::Vec3::new(1.0, 2.0, 3.0));
        let m = p.to_mat4();
        let p2 = Pose3::from_mat4(m);
        assert_relative_eq!(p.rotation.x, p2.rotation.x, epsilon = 1e-5);
        assert_relative_eq!(p.rotation.y, p2.rotation.y, epsilon = 1e-5);
        assert_relative_eq!(p.rotation.z, p2.rotation.z, epsilon = 1e-5);
        assert_relative_eq!(p.rotation.w, p2.rotation.w, epsilon = 1e-5);
        assert_relative_eq!(p.translation.x, p2.translation.x, epsilon = 1e-6);
        assert_relative_eq!(p.translation.y, p2.translation.y, epsilon = 1e-6);
        assert_relative_eq!(p.translation.z, p2.translation.z, epsilon = 1e-6);
    }

    #[test]
    fn test_dpose3_new() {
        let axisangle = glam::DVec3::new(0.1, 0.2, 0.3);
        let p = DPose3::new(glam::DVec3::new(1.0, 2.0, 3.0), axisangle);
        assert_relative_eq!(p.translation.x, 1.0, epsilon = 1e-10);
        assert_relative_eq!(p.translation.y, 2.0, epsilon = 1e-10);
        assert_relative_eq!(p.translation.z, 3.0, epsilon = 1e-10);
    }
}
