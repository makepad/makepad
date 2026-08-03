//! THE heading convention. Read this before writing any motion code.
//!
//! Sign and handedness mistakes have been made independently in this engine
//! several times — the car steered backwards, and the DSL docs have to warn
//! twice that entities face one way while the camera faces another. Every one
//! of those was a separate hand-rolled `sin`/`cos` with a guessed sign. This
//! module exists so there is exactly one place to get it right.
//!
//! # The convention
//!
//! - **Forward is −Z.** A heading of `0` faces `(0, 0, -1)`. This is inherited
//!   from the Godot port the engine grew out of, and it is not worth changing:
//!   every fixture, tape and authored scene assumes it.
//! - **Right is +X**, up is +Y. Standard right-handed axes.
//! - Therefore `forward(yaw) = (-sin yaw, 0, -cos yaw)`, and
//!   `yaw = atan2(-x, -z)`.
//! - **Positive yaw turns LEFT** (anticlockwise seen from above). At
//!   `yaw = π/2` forward is `(-1, 0, 0)`, which is −X, which is the player's
//!   left. This is the one that keeps catching people out, because "positive
//!   means right" is the intuition for a steering wheel.
//!
//! # Consequences you must respect
//!
//! - **Steering input is not a yaw rate.** A positive steer input means "turn
//!   right", and turning right means yaw *decreases*. Use [`steer_to_yaw_rate`]
//!   rather than negating by hand at the call site, or the next vehicle — the
//!   plane, the boat, the turret — will get it wrong on its own.
//! - **Camera-relative movement uses the same yaw**, so a character walking
//!   "forward" relative to a camera at `cam_yaw` moves along `forward(cam_yaw)`.
//!
//! Everything that turns something should call into here. If you find yourself
//! writing `sin`/`cos` on a yaw, use these instead.

use crate::math as gm;
use makepad_math::*;

/// Unit forward vector for a heading. `yaw = 0` faces −Z.
#[inline]
pub fn heading_to_forward(yaw: f32) -> Vec3f {
    let (s, c) = gm::sincos(yaw);
    vec3f(-s, 0.0, -c)
}

/// Unit right vector for a heading — forward rotated a quarter turn toward +X.
#[inline]
pub fn heading_to_right(yaw: f32) -> Vec3f {
    let (s, c) = gm::sincos(yaw);
    vec3f(-c, 0.0, s)
}

/// Heading that faces the given direction. The y component is ignored; a zero
/// direction returns 0 rather than a NaN, because "no direction" is a thing a
/// stationary body legitimately has.
#[inline]
pub fn forward_to_heading(dir: Vec3f) -> f32 {
    if dir.x == 0.0 && dir.z == 0.0 {
        return 0.0;
    }
    gm::atan2(-dir.x, -dir.z)
}

/// Yaw rate for a steering input, in radians/second.
///
/// **This is where the sign lives.** A positive `steer` means the driver wants
/// to go right, and going right means the heading decreases — see the module
/// docs. Callers pass driver intent and get an angular velocity; nobody else
/// should be negating anything.
#[inline]
pub fn steer_to_yaw_rate(steer: f32, rate: f32) -> f32 {
    -steer * rate
}

/// Shortest signed difference `to - from`, wrapped to (−π, π]. Positive means
/// "turn left to get there", consistent with the rest of this module.
#[inline]
pub fn heading_delta(from: f32, to: f32) -> f32 {
    let mut d = to - from;
    while d > std::f32::consts::PI {
        d -= std::f32::consts::TAU;
    }
    while d <= -std::f32::consts::PI {
        d += std::f32::consts::TAU;
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    /// These read as statements of the convention, not as arithmetic. If one
    /// fails, someone changed what the engine means by "forward" — which is a
    /// decision, not a bug fix, and every fixture assumes the current answer.
    #[test]
    fn heading_zero_faces_negative_z() {
        let f = heading_to_forward(0.0);
        assert!((f.x - 0.0).abs() < 1e-6, "forward(0).x = {}", f.x);
        assert!((f.z + 1.0).abs() < 1e-6, "forward(0).z = {} (want -1)", f.z);
    }

    #[test]
    fn positive_yaw_turns_left() {
        // Left of "facing -Z" is -X.
        let f = heading_to_forward(std::f32::consts::FRAC_PI_2);
        assert!(f.x < -0.99, "yaw +90 should face -X (left), got x={}", f.x);
    }

    #[test]
    fn right_is_plus_x_when_facing_forward() {
        let r = heading_to_right(0.0);
        assert!(r.x < -0.99 || r.x > 0.99, "right must be an X axis, got {r:?}");
    }

    #[test]
    fn steering_right_decreases_heading() {
        // The bug this module exists to prevent: a driver pulling right must
        // not turn the car left.
        let rate = steer_to_yaw_rate(1.0, 2.0);
        assert!(rate < 0.0, "steer right must lower yaw, got {rate}");
        assert!(steer_to_yaw_rate(-1.0, 2.0) > 0.0);
    }

    #[test]
    fn forward_and_heading_round_trip() {
        for deg in [-179.0f32, -90.0, -1.0, 0.0, 1.0, 90.0, 179.0] {
            let yaw = deg.to_radians();
            let back = forward_to_heading(heading_to_forward(yaw));
            assert!(
                heading_delta(yaw, back).abs() < 1e-4,
                "round trip failed at {deg} deg: {yaw} -> {back}"
            );
        }
    }

    #[test]
    fn stationary_direction_is_not_nan() {
        assert_eq!(forward_to_heading(vec3f(0.0, 0.0, 0.0)), 0.0);
    }

    #[test]
    fn heading_delta_takes_the_short_way() {
        let d = heading_delta(3.0, -3.0);
        assert!(d > 0.0 && d < 0.6, "should wrap the short way, got {d}");
    }
}
