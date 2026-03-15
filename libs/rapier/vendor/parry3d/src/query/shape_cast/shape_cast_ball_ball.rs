use crate::math::{Pose, Vector};
use crate::query::details::ShapeCastOptions;
use crate::query::{self, Ray, ShapeCastHit, ShapeCastStatus};
use crate::shape::Ball;
use num::Zero;

/// Time Of Impact of two balls under translational movement.
#[inline]
pub fn cast_shapes_ball_ball(
    pos12: &Pose,
    vel12: Vector,
    b1: &Ball,
    b2: &Ball,
    options: ShapeCastOptions,
) -> Option<ShapeCastHit> {
    let rsum = b1.radius + b2.radius + options.target_distance;
    let radius = rsum;
    let center = -pos12.translation;
    let ray = Ray::new(Vector::ZERO, vel12);

    if let (inside, Some(time_of_impact)) =
        query::details::ray_toi_with_ball(center, radius, &ray, true)
    {
        if time_of_impact > options.max_time_of_impact {
            return None;
        }

        let dpt = ray.point_at(time_of_impact) - center;
        let normal1;
        let normal2;
        let witness1;
        let witness2;

        if radius.is_zero() {
            normal1 = Vector::X;
            normal2 = pos12.rotation.inverse() * -Vector::X;
            witness1 = Vector::ZERO;
            witness2 = Vector::ZERO;
        } else {
            normal1 = dpt / radius;
            normal2 = pos12.rotation.inverse() * -normal1;
            witness1 = normal1 * b1.radius;
            witness2 = normal2 * b2.radius;
        }

        if !options.stop_at_penetration && time_of_impact < 1.0e-5 && normal1.dot(vel12) >= 0.0 {
            return None;
        }

        let status = if inside && center.length_squared() < rsum * rsum {
            ShapeCastStatus::PenetratingOrWithinTargetDist
        } else {
            ShapeCastStatus::Converged
        };

        Some(ShapeCastHit {
            time_of_impact,
            normal1,
            normal2,
            witness1,
            witness2,
            status,
        })
    } else {
        None
    }
}
