use crate::math::{Pose, Real, Vector};
use crate::query::details;
use crate::query::details::ShapeCastOptions;
use crate::query::gjk::{self, VoronoiSimplex};
use crate::query::{ShapeCastHit, ShapeCastStatus};
use crate::shape::{RoundShapeRef, SupportMap};
use num::Zero;

/// Time of impacts between two support-mapped shapes under translational movement.
pub fn cast_shapes_support_map_support_map<G1, G2>(
    pos12: &Pose,
    vel12: Vector,
    g1: &G1,
    g2: &G2,
    options: ShapeCastOptions,
) -> Option<ShapeCastHit>
where
    G1: ?Sized + SupportMap,
    G2: ?Sized + SupportMap,
{
    let gjk_result = if options.target_distance > 0.0 {
        let round_g1 = RoundShapeRef {
            inner_shape: g1,
            border_radius: options.target_distance,
        };
        gjk::directional_distance(pos12, &round_g1, g2, vel12, &mut VoronoiSimplex::new())
    } else {
        gjk::directional_distance(pos12, g1, g2, vel12, &mut VoronoiSimplex::new())
    };

    gjk_result.and_then(|(time_of_impact, normal1, witness1, witness2)| {
        if time_of_impact > options.max_time_of_impact {
            None
        } else if (options.compute_impact_geometry_on_penetration || !options.stop_at_penetration)
            && time_of_impact < 1.0e-5
        {
            let contact = details::contact_support_map_support_map(pos12, g1, g2, Real::MAX)?;
            let normal_vel = contact.normal1.dot(vel12);

            if !options.stop_at_penetration && normal_vel >= 0.0 {
                None
            } else {
                Some(ShapeCastHit {
                    time_of_impact,
                    normal1: contact.normal1,
                    normal2: contact.normal2,
                    witness1: contact.point1,
                    witness2: contact.point2,
                    status: ShapeCastStatus::PenetratingOrWithinTargetDist,
                })
            }
        } else {
            Some(ShapeCastHit {
                time_of_impact,
                normal1,
                normal2: pos12.rotation.inverse() * -normal1,
                witness1: witness1 - normal1 * options.target_distance,
                witness2: pos12.inverse_transform_point(witness2),
                status: if time_of_impact.is_zero() {
                    ShapeCastStatus::PenetratingOrWithinTargetDist
                } else {
                    ShapeCastStatus::Converged
                },
            })
        }
    })
}
