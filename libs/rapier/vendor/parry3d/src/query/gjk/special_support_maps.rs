use crate::math::{Pose, Real, Vector};
use crate::shape::SupportMap;

/// A support mapping that is a single point.
pub struct ConstantPoint(pub Vector);

impl SupportMap for ConstantPoint {
    #[inline]
    fn support_point(&self, m: &Pose, _: Vector) -> Vector {
        m * self.0
    }

    #[inline]
    fn support_point_toward(&self, m: &Pose, _: Vector) -> Vector {
        m * self.0
    }

    #[inline]
    fn local_support_point(&self, _: Vector) -> Vector {
        self.0
    }

    #[inline]
    fn local_support_point_toward(&self, _: Vector) -> Vector {
        self.0
    }
}

/// A support mapping that is the point at (0.0, 0.0, 0.0).
pub struct ConstantOrigin;

impl SupportMap for ConstantOrigin {
    #[inline]
    fn support_point(&self, m: &Pose, _: Vector) -> Vector {
        m.translation
    }

    #[inline]
    fn support_point_toward(&self, m: &Pose, _: Vector) -> Vector {
        m.translation
    }

    #[inline]
    fn local_support_point(&self, _: Vector) -> Vector {
        Vector::ZERO
    }

    #[inline]
    fn local_support_point_toward(&self, _: Vector) -> Vector {
        Vector::ZERO
    }
}

/// The Minkowski sum of a shape and a ball.
pub struct DilatedShape<'a, S: ?Sized + SupportMap> {
    /// The shape involved in the Minkowski sum.
    pub shape: &'a S,
    /// The radius of the ball involved in the Minkoski sum.
    pub radius: Real,
}

impl<S: ?Sized + SupportMap> SupportMap for DilatedShape<'_, S> {
    #[inline]
    fn support_point(&self, m: &Pose, dir: Vector) -> Vector {
        let normalized_dir = dir.normalize_or_zero();
        self.support_point_toward(m, normalized_dir)
    }

    #[inline]
    fn support_point_toward(&self, m: &Pose, dir: Vector) -> Vector {
        self.shape.support_point_toward(m, dir) + dir * self.radius
    }

    #[inline]
    fn local_support_point(&self, dir: Vector) -> Vector {
        let normalized_dir = dir.normalize_or_zero();
        self.local_support_point_toward(normalized_dir)
    }

    #[inline]
    fn local_support_point_toward(&self, dir: Vector) -> Vector {
        self.shape.local_support_point_toward(dir) + dir * self.radius
    }
}
