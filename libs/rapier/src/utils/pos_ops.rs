//! SimdPose trait for pose (isometry) operations.

#[cfg(feature = "simd-is-enabled")]
use crate::math::SimdReal;
use crate::math::{AngVector, Pose, Real, Rotation, Vector};
use crate::utils::ScalarType;

/// Trait for pose types (isometry) providing access to rotation and translation.
pub trait PoseOps<N: ScalarType>: Copy {
    /// Get the rotation component of this pose.
    fn rotation(&self) -> N::Rotation;
    /// Get the translation component of this pose as a vector.
    fn translation(&self) -> N::Vector;
    /// Set the translation component of this pose.
    fn set_translation(&mut self, tra: N::Vector);
    /// Prepend a translation to this pose.
    fn prepend_translation(&self, translation: N::Vector) -> Self;
    /// Append a translation to this pose.
    fn append_translation(&self, translation: N::Vector) -> Self;
    /// Prepend a rotation (as an axis-angle) to this pose.
    fn prepend_rotation(&self, axisangle: N::AngVector) -> Self;
    /// Append a rotation (as an axis-angle) to this pose.
    fn append_rotation(&self, axisangle: N::AngVector) -> Self;
}

#[cfg(all(feature = "dim3", feature = "simd-is-enabled"))]
impl PoseOps<SimdReal> for na::Isometry3<SimdReal> {
    #[inline]
    fn rotation(&self) -> na::UnitQuaternion<SimdReal> {
        self.rotation
    }
    #[inline]
    fn translation(&self) -> na::Vector3<SimdReal> {
        self.translation.vector
    }

    #[inline]
    fn set_translation(&mut self, tra: na::Vector3<SimdReal>) {
        self.translation.vector = tra;
    }

    #[inline]
    fn prepend_translation(&self, translation: na::Vector3<SimdReal>) -> Self {
        self * na::Translation3::from(translation)
    }

    #[inline]
    fn append_translation(&self, translation: na::Vector3<SimdReal>) -> Self {
        na::Translation3::from(translation) * self
    }

    #[inline]
    fn prepend_rotation(&self, rotation: na::Vector3<SimdReal>) -> Self {
        self * na::UnitQuaternion::new(rotation)
    }

    #[inline]
    fn append_rotation(&self, rotation: na::Vector3<SimdReal>) -> Self {
        na::UnitQuaternion::new(rotation) * self
    }
}

#[cfg(all(feature = "dim2", feature = "simd-is-enabled"))]
impl PoseOps<SimdReal> for na::Isometry2<SimdReal> {
    #[inline]
    fn rotation(&self) -> na::UnitComplex<SimdReal> {
        self.rotation
    }
    #[inline]
    fn translation(&self) -> na::Vector2<SimdReal> {
        self.translation.vector
    }

    #[inline]
    fn set_translation(&mut self, tra: na::Vector2<SimdReal>) {
        self.translation.vector = tra;
    }

    #[inline]
    fn prepend_translation(&self, translation: na::Vector2<SimdReal>) -> Self {
        self * na::Translation2::from(translation)
    }

    #[inline]
    fn append_translation(&self, translation: na::Vector2<SimdReal>) -> Self {
        na::Translation2::from(translation) * self
    }

    #[inline]
    fn prepend_rotation(&self, rotation: SimdReal) -> Self {
        self * na::UnitComplex::new(rotation)
    }

    #[inline]
    fn append_rotation(&self, rotation: SimdReal) -> Self {
        na::UnitComplex::new(rotation) * self
    }
}

impl PoseOps<Real> for Pose {
    #[inline]
    fn rotation(&self) -> Rotation {
        self.rotation
    }
    #[inline]
    fn translation(&self) -> Vector {
        self.translation
    }

    #[inline]
    fn set_translation(&mut self, tra: Vector) {
        self.translation = tra;
    }

    #[inline]
    fn prepend_translation(&self, translation: Vector) -> Self {
        (*self).prepend_translation(translation)
    }

    #[inline]
    fn append_translation(&self, translation: Vector) -> Self {
        (*self).append_translation(translation)
    }

    #[inline]
    fn prepend_rotation(&self, rotation: AngVector) -> Self {
        #[cfg(feature = "dim2")]
        return *self * Rotation::from_angle(rotation);
        #[cfg(feature = "dim3")]
        return *self * Rotation::from_scaled_axis(rotation);
    }

    #[inline]
    fn append_rotation(&self, rotation: AngVector) -> Self {
        #[cfg(feature = "dim2")]
        return Rotation::from_angle(rotation) * *self;
        #[cfg(feature = "dim3")]
        return Rotation::from_scaled_axis(rotation) * *self;
    }
}
