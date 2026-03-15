use crate::math::{Pose, Vector};
use crate::shape::SupportMap;
use core::ops::Sub;

/// A point of a Configuration-Space Obstacle.
///
/// A Configuration-Space Obstacle (CSO) is the result of the
/// Minkowski Difference of two solids. In other words, each of its
/// points correspond to the difference of two point, each belonging
/// to a different solid.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct CsoPoint {
    /// The point on the CSO. This is equal to `self.orig1 - self.orig2`, unless this CsoPoint
    /// has been translated with self.translate.
    pub point: Vector,
    /// The original point on the first shape used to compute `self.point`.
    pub orig1: Vector,
    /// The original point on the second shape used to compute `self.point`.
    pub orig2: Vector,
}

impl CsoPoint {
    /// A CSO point where all components are zero.
    pub const ZERO: Self = Self {
        point: Vector::ZERO,
        orig1: Vector::ZERO,
        orig2: Vector::ZERO,
    };

    /// Initializes a CSO point with `orig1 - orig2`.
    pub fn new(orig1: Vector, orig2: Vector) -> Self {
        let point = orig1 - orig2;
        Self::new_with_point(point, orig1, orig2)
    }

    /// Initializes a CSO point with all information provided.
    ///
    /// It is assumed, but not checked, that `point == orig1 - orig2`.
    pub fn new_with_point(point: Vector, orig1: Vector, orig2: Vector) -> Self {
        CsoPoint {
            point,
            orig1,
            orig2,
        }
    }

    /// Initializes a CSO point where both original points are equal.
    pub fn single_point(point: Vector) -> Self {
        Self::new_with_point(point, point, Vector::ZERO)
    }

    /// CSO point where all components are set to zero.
    pub fn origin() -> Self {
        CsoPoint::new(Vector::ZERO, Vector::ZERO)
    }

    /// Computes the support point of the CSO of `g1` and `g2` toward the unit direction `dir`.
    pub fn from_shapes_toward<G1, G2>(pos12: &Pose, g1: &G1, g2: &G2, dir: Vector) -> Self
    where
        G1: ?Sized + SupportMap,
        G2: ?Sized + SupportMap,
    {
        let sp1 = g1.local_support_point_toward(dir);
        let sp2 = g2.support_point_toward(pos12, -dir);

        CsoPoint::new(sp1, sp2)
    }

    /// Computes the support point of the CSO of `g1` and `g2` toward the direction `dir`.
    pub fn from_shapes<G1, G2>(pos12: &Pose, g1: &G1, g2: &G2, dir: Vector) -> Self
    where
        G1: ?Sized + SupportMap,
        G2: ?Sized + SupportMap,
    {
        let sp1 = g1.local_support_point(dir);
        let sp2 = g2.support_point(pos12, -dir);

        CsoPoint::new(sp1, sp2)
    }

    /// Translate the CSO point.
    pub fn translate(&self, dir: Vector) -> Self {
        CsoPoint::new_with_point(self.point + dir, self.orig1, self.orig2)
    }

    /// Translate in-place the CSO point.
    pub fn translate_mut(&mut self, dir: Vector) {
        self.point += dir;
    }
}

impl Sub<CsoPoint> for CsoPoint {
    type Output = Vector;

    #[inline]
    fn sub(self, rhs: CsoPoint) -> Vector {
        self.point - rhs.point
    }
}
