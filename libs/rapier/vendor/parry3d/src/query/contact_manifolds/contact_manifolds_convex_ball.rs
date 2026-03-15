use crate::math::{Pose, Real, Vector};
use crate::query::contact_manifolds::{NormalConstraints, NormalConstraintsPair};
use crate::query::{ContactManifold, Ray, TrackedContact};
use crate::shape::{Ball, PackedFeatureId, Shape};

/// Computes the contact manifold between a convex shape and a ball, both represented as a `Shape` trait-object.
pub fn contact_manifold_convex_ball_shapes<ManifoldData, ContactData>(
    pos12: &Pose,
    shape1: &dyn Shape,
    shape2: &dyn Shape,
    normal_constraints1: Option<&dyn NormalConstraints>,
    normal_constraints2: Option<&dyn NormalConstraints>,
    prediction: Real,
    manifold: &mut ContactManifold<ManifoldData, ContactData>,
) where
    ContactData: Default + Copy,
{
    if let Some(ball1) = shape1.as_ball() {
        contact_manifold_convex_ball(
            &pos12.inverse(),
            shape2,
            ball1,
            normal_constraints2,
            normal_constraints1,
            prediction,
            manifold,
            true,
        );
    } else if let Some(ball2) = shape2.as_ball() {
        contact_manifold_convex_ball(
            pos12,
            shape1,
            ball2,
            normal_constraints1,
            normal_constraints2,
            prediction,
            manifold,
            false,
        );
    }
}

/// Computes the contact manifold between a convex shape and a ball.
pub fn contact_manifold_convex_ball<'a, ManifoldData, ContactData, S1>(
    pos12: &Pose,
    shape1: &'a S1,
    ball2: &'a Ball,
    normal_constraints1: Option<&dyn NormalConstraints>,
    normal_constraints2: Option<&dyn NormalConstraints>,
    prediction: Real,
    manifold: &mut ContactManifold<ManifoldData, ContactData>,
    flipped: bool,
) where
    S1: ?Sized + Shape,
    ContactData: Default + Copy,
{
    let local_p2_1 = pos12.translation;
    let (proj, mut fid1) = shape1.project_local_point_and_get_feature(local_p2_1);
    let mut local_p1 = proj.point;
    let dpos = local_p2_1 - local_p1;

    // local_n1 points from the surface towards our origin if defined, otherwise from the other
    // shape's origin towards our origin if defined, otherwise towards +x
    // NOTE: we used `dpos.normalize_and_length()` here before. But since `normalize_and_length()`
    //       multiplying by the reciprocal (1.0 / length) instead of directly dividing by `length`,
    //       it can introduce tiny numerical errors that result in drift breaking perfectly verticality
    //       of contact normals for, e.g., the 3D spring joint demo in rapier. So we divide ourselves
    //       instead.
    let mut dist = dpos.length();
    let mut local_n1 = if dist == 0.0 {
        pos12.translation.normalize_or(Vector::Y)
    } else {
        dpos / dist
    };

    if proj.is_inside {
        local_n1 = -local_n1;
        dist = -dist;
    }

    if dist <= ball2.radius + prediction {
        let mut local_n2 = pos12.rotation.inverse() * -local_n1;
        let uncorrected_local_n2 = local_n2;

        if !(normal_constraints1, normal_constraints2).project_local_normals(
            pos12,
            &mut local_n1,
            &mut local_n2,
        ) {
            // The contact got completely discarded by the normal correction.
            manifold.clear();
            return;
        }

        let local_p2 = local_n2 * ball2.radius;

        // If a correction happened, adjust the contact point on the first body.
        if uncorrected_local_n2 != local_n2 {
            let ray1 = Ray::new(
                pos12.translation,
                if proj.is_inside { local_n1 } else { -local_n1 },
            );

            if let Some(hit) = shape1.cast_local_ray_and_get_normal(&ray1, Real::MAX, false) {
                local_p1 = ray1.point_at(hit.time_of_impact);
                dist = if proj.is_inside {
                    -hit.time_of_impact
                } else {
                    hit.time_of_impact
                };
                fid1 = hit.feature;
            } else {
                manifold.clear();
                return;
            }
        }

        let contact_point = TrackedContact::flipped(
            local_p1,
            local_p2,
            fid1.into(),
            PackedFeatureId::face(0),
            dist - ball2.radius,
            flipped,
        );

        if manifold.points.len() != 1 {
            manifold.clear();
            manifold.points.push(contact_point);
        } else {
            // Copy only the geometry so we keep the warmstart impulses.
            manifold.points[0].copy_geometry_from(contact_point);
        }

        if flipped {
            manifold.local_n1 = local_n2;
            manifold.local_n2 = local_n1;
        } else {
            manifold.local_n1 = local_n1;
            manifold.local_n2 = local_n2;
        }
    } else {
        manifold.clear();
    }
}
