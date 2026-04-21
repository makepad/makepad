use crate::math::{Pose, Real, Vector};
use crate::query::{ContactManifold, TrackedContact};
use crate::shape::{Ball, PackedFeatureId, Shape};

/// Computes the contact manifold between two balls given as `Shape` trait-objects.
pub fn contact_manifold_ball_ball_shapes<ManifoldData, ContactData: Default + Copy>(
    pos12: &Pose,
    shape1: &dyn Shape,
    shape2: &dyn Shape,
    prediction: Real,
    manifold: &mut ContactManifold<ManifoldData, ContactData>,
) {
    if let (Some(ball1), Some(ball2)) = (shape1.as_ball(), shape2.as_ball()) {
        contact_manifold_ball_ball(pos12, ball1, ball2, prediction, manifold);
    }
}

/// Computes the contact manifold between two balls.
pub fn contact_manifold_ball_ball<ManifoldData, ContactData: Default + Copy>(
    pos12: &Pose,
    ball1: &Ball,
    ball2: &Ball,
    prediction: Real,
    manifold: &mut ContactManifold<ManifoldData, ContactData>,
) {
    let radius_a = ball1.radius;
    let radius_b = ball2.radius;

    let dcenter = pos12.translation;
    let center_dist = dcenter.length();
    let dist = center_dist - radius_a - radius_b;

    if dist < prediction {
        let local_n1 = if center_dist != 0.0 {
            dcenter / center_dist
        } else {
            Vector::Y
        };

        let local_n2 = pos12.rotation.inverse() * -local_n1;
        let local_p1 = local_n1 * radius_a;
        let local_p2 = local_n2 * radius_b;
        let fid = PackedFeatureId::face(0);
        let contact = TrackedContact::new(local_p1, local_p2, fid, fid, dist);

        if !manifold.points.is_empty() {
            manifold.points[0].copy_geometry_from(contact);
        } else {
            manifold.points.push(contact);
        }

        manifold.local_n1 = local_n1;
        manifold.local_n2 = local_n2;
    } else {
        manifold.points.clear();
    }
}
