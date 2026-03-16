use crate::math::{Pose, Real, Vector};
use crate::query::Contact;
use crate::shape::{Ball, Shape};

/// Contact between a ball and a convex polyhedron.
///
/// This function panics if the input shape does not implement
/// both the ConvexPolyhedron and PointQuery traits.
#[inline]
pub fn contact_ball_convex_polyhedron(
    pos12: &Pose,
    ball1: &Ball,
    shape2: &(impl Shape + ?Sized),
    prediction: Real,
) -> Option<Contact> {
    contact_convex_polyhedron_ball(&pos12.inverse(), shape2, ball1, prediction).map(|c| c.flipped())
}

/// Contact between a convex polyhedron and a ball.
///
/// This function panics if the input shape does not implement
/// both the ConvexPolyhedron and PointQuery traits.
#[inline]
pub fn contact_convex_polyhedron_ball(
    pos12: &Pose,
    shape1: &(impl Shape + ?Sized),
    ball2: &Ball,
    prediction: Real,
) -> Option<Contact> {
    let center2_1 = pos12.translation;
    let (proj, f1) = shape1.project_local_point_and_get_feature(center2_1);

    let dist;
    let normal1;
    let (dir1, len) = (proj.point - center2_1).normalize_and_length();
    if len >= crate::math::DEFAULT_EPSILON {
        if proj.is_inside {
            dist = -len - ball2.radius;
            normal1 = dir1;
        } else {
            dist = len - ball2.radius;
            normal1 = -dir1;
        }
    } else {
        dist = -ball2.radius;
        normal1 = shape1
            .feature_normal_at_point(f1, proj.point)
            .or_else(|| (proj.point).try_normalize())
            .unwrap_or(Vector::Y);
    }

    if dist <= prediction {
        let normal2 = pos12.rotation.inverse() * -normal1;
        let point2 = normal2 * ball2.radius;
        let point1 = proj.point;
        return Some(Contact::new(point1, point2, normal1, normal2, dist));
    }

    None
}
