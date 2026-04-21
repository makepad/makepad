use crate::math::{ComplexField, Pose, Real, Vector};
use crate::query::Contact;
use crate::shape::Ball;
use num::Zero;

/// Contact between balls.
#[inline]
pub fn contact_ball_ball(pos12: &Pose, b1: &Ball, b2: &Ball, prediction: Real) -> Option<Contact> {
    let r1 = b1.radius;
    let r2 = b2.radius;
    let center2_1 = pos12.translation;
    let distance_squared = center2_1.length_squared();
    let sum_radius = r1 + r2;
    let sum_radius_with_error = sum_radius + prediction;

    if distance_squared < sum_radius_with_error * sum_radius_with_error {
        let normal1 = if !distance_squared.is_zero() {
            (center2_1).normalize()
        } else {
            Vector::X
        };
        let normal2 = -(pos12.rotation.inverse() * normal1);
        let point1 = normal1 * r1;
        let point2 = normal2 * r2;

        Some(Contact::new(
            point1,
            point2,
            normal1,
            normal2,
            <Real as ComplexField>::sqrt(distance_squared) - sum_radius,
        ))
    } else {
        None
    }
}
