use crate::bounding_volume::Aabb;
use crate::math::{Pose, Real, Vector};
use crate::shape::Ball;

/// Computes the Axis-Aligned Bounding Box of a ball transformed by `center`.
#[inline]
pub fn ball_aabb(center: Vector, radius: Real) -> Aabb {
    Aabb::new(
        center + Vector::splat(-radius),
        center + Vector::splat(radius),
    )
}

/// Computes the Axis-Aligned Bounding Box of a ball.
#[inline]
pub fn local_ball_aabb(radius: Real) -> Aabb {
    let half_extents = Vector::splat(radius);

    Aabb::new(-half_extents, half_extents)
}

impl Ball {
    /// Computes the world-space [`Aabb`] of this ball transformed by `pos`.
    #[inline]
    pub fn aabb(&self, pos: &Pose) -> Aabb {
        ball_aabb(pos.translation, self.radius)
    }

    /// Computes the local-space [`Aabb`] of this ball.
    #[inline]
    pub fn local_aabb(&self) -> Aabb {
        local_ball_aabb(self.radius)
    }
}
