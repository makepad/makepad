use crate::math::{Pose, Vector};
use crate::query::details::ShapeCastOptions;
use crate::query::{Ray, RayCast, ShapeCastHit, ShapeCastStatus};
use crate::shape::{HalfSpace, RoundShapeRef, SupportMap};

/// Time Of Impact of a halfspace with a support-mapped shape under translational movement.
pub fn cast_shapes_halfspace_support_map<G: ?Sized + SupportMap>(
    pos12: &Pose,
    vel12: Vector,
    halfspace: &HalfSpace,
    other: &G,
    options: ShapeCastOptions,
) -> Option<ShapeCastHit> {
    // TODO: add method to get only the local support point.
    // This would avoid the `inverse_transform_point` later.
    if !options.stop_at_penetration && vel12.dot(halfspace.normal) > 0.0 {
        return None;
    }

    let support_point = if options.target_distance > 0.0 {
        let round_other = RoundShapeRef {
            inner_shape: other,
            border_radius: options.target_distance,
        };
        round_other.support_point(pos12, -halfspace.normal)
    } else {
        other.support_point(pos12, -halfspace.normal)
    };
    let closest_point = support_point;
    let ray = Ray::new(closest_point, vel12);

    if let Some(time_of_impact) = halfspace.cast_local_ray(&ray, options.max_time_of_impact, true) {
        if time_of_impact > options.max_time_of_impact {
            return None;
        }

        let witness2 = support_point + halfspace.normal * options.target_distance;
        let mut witness1 = ray.point_at(time_of_impact);
        // Project the witness point to the halfspace.
        // Note that witness1 is already in the halfspace's local-space.
        witness1 -= halfspace.normal * witness1.dot(halfspace.normal);

        let status = if support_point.dot(halfspace.normal) < 0.0 {
            ShapeCastStatus::PenetratingOrWithinTargetDist
        } else {
            ShapeCastStatus::Converged
        };

        Some(ShapeCastHit {
            time_of_impact,
            normal1: halfspace.normal,
            normal2: pos12.rotation.inverse() * -halfspace.normal,
            witness1,
            witness2: pos12.inverse_transform_point(witness2),
            status,
        })
    } else {
        None
    }
}

/// Time Of Impact of a halfspace with a support-mapped shape under translational movement.
pub fn cast_shapes_support_map_halfspace<G: ?Sized + SupportMap>(
    pos12: &Pose,
    vel12: Vector,
    other: &G,
    halfspace: &HalfSpace,
    options: ShapeCastOptions,
) -> Option<ShapeCastHit> {
    cast_shapes_halfspace_support_map(
        &pos12.inverse(),
        -(pos12.rotation.inverse() * vel12),
        halfspace,
        other,
        options,
    )
    .map(|hit| hit.swapped())
}
