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

/// Unit right vector for a heading — the body's own right hand.
///
/// This is `forward × up` for the right-handed frame, which at `yaw = 0`
/// (facing −Z, +Y up) is `+X`. It returned `−X` — the LEFT vector — until it
/// was checked against the cross product; the doc comment claimed +X the whole
/// time, and the test asserted `r.x < -0.99 || r.x > 0.99`, which accepts both
/// signs and so could never have failed. If you change this, change it because
/// the cross product says so.
#[inline]
pub fn heading_to_right(yaw: f32) -> Vec3f {
    let (s, c) = gm::sincos(yaw);
    vec3f(c, 0.0, -s)
}

/// Yaw for the *renderer's* camera rig, given a sim heading.
///
/// The two conventions are mirrored in X: a heading faces `(−sin y, −cos y)`
/// and positive yaw turns LEFT, while the renderer looks along
/// `(sin Y, −cos Y)` so positive yaw turns RIGHT. Both face −Z at zero, which
/// is precisely why the disagreement survives casual testing — it is invisible
/// until something is off-axis.
///
/// Convert here, once, at a named boundary. A negation written at a call site
/// teaches the next call site nothing, and this module exists because that
/// mistake has been made independently several times already.
#[inline]
pub fn heading_to_camera_yaw(yaw: f32) -> f32 {
    -yaw
}

/// Sim heading, given an angle already in the *renderer's* camera-yaw
/// convention — the other side of [`heading_to_camera_yaw`].
///
/// The classic-world importers publish every facing they carry (a map's
/// spawn, a placed thing's angle, a teleport's exit) in the camera
/// convention: `makepad_render::level::yaw_forward` = `(sin y, 0, −cos y)`,
/// which is what a walker's camera turns to. A body's `yaw` is a heading —
/// `(−sin y, 0, −cos y)` — so anything that turns declared map data into a
/// body's facing must come through here. Feeding the raw declared angle to
/// `Entity::yaw` mirrors the body about the world's X axis, which is
/// invisible on the north/south axis and a HALF TURN on the east/west one:
/// that is exactly how a level's monsters came to walk at the player
/// showing their backs.
#[inline]
pub fn camera_yaw_to_heading(yaw: f32) -> f32 {
    -yaw
}

/// Pitch for the renderer's camera rig, given a "positive lifts the camera"
/// pitch.
///
/// Same mirror: the renderer places the eye at `target − forward·distance` with
/// `forward.y = sin P`, so *negative* render pitch puts the camera above its
/// target looking down. A rig that thinks "positive is higher" — the intuitive
/// direction, and the one [`crate::camera_boom_limit`]'s callers use — must
/// flip. The renderer's own clamps corroborate it: it allows pitch in
/// −1.2..0.25, a nearly all-negative range, for an over-the-shoulder camera.
#[inline]
pub fn heading_to_camera_pitch(pitch: f32) -> f32 {
    -pitch
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
        // Was `r.x < -0.99 || r.x > 0.99`, which accepts the left vector too
        // and passed for as long as the function returned it. A test that
        // cannot fail is worse than no test: it reads as coverage.
        assert!(r.x > 0.99, "right must be +X when facing -Z, got {r:?}");
    }

    #[test]
    fn right_is_forward_crossed_with_up() {
        // The definition, not a spot value — this is what makes the sign a
        // derivation rather than a preference.
        for deg in [-179.0f32, -90.0, -33.0, 0.0, 45.0, 90.0, 179.0] {
            let yaw = deg.to_radians();
            let f = heading_to_forward(yaw);
            let r = heading_to_right(yaw);
            // f x (0,1,0)
            let (cx, cz) = (-f.z, f.x);
            assert!(
                (r.x - cx).abs() < 1e-5 && (r.z - cz).abs() < 1e-5,
                "at {deg} deg right={r:?} but forward x up=({cx},{cz})"
            );
        }
    }

    #[test]
    fn camera_yaw_mirrors_the_heading_basis() {
        // Pins the render conversion against the renderer's own expression:
        // it looks along (sin Y cos P, sin P, -cos Y cos P). Converting a
        // heading must produce a camera aimed exactly where that heading faces.
        for deg in [-179.0f32, -90.0, -33.0, 0.0, 45.0, 90.0, 179.0] {
            let yaw = deg.to_radians();
            let f = heading_to_forward(yaw);
            let cam = heading_to_camera_yaw(yaw);
            let (cx, cz) = (cam.sin(), -cam.cos());
            assert!(
                (f.x - cx).abs() < 1e-5 && (f.z - cz).abs() < 1e-5,
                "at {deg} deg heading faces {f:?} but camera looks ({cx},{cz})"
            );
        }
    }

    #[test]
    fn camera_yaw_round_trips_through_the_heading_basis() {
        // The declared-map-data direction: an importer's camera-convention
        // angle must become the heading that faces the same way it does.
        for deg in [-179.0f32, -90.0, -33.0, 0.0, 45.0, 90.0, 179.0] {
            let cam = deg.to_radians();
            let h = camera_yaw_to_heading(cam);
            let f = heading_to_forward(h);
            let (cx, cz) = (cam.sin(), -cam.cos());
            assert!(
                (f.x - cx).abs() < 1e-5 && (f.z - cz).abs() < 1e-5,
                "at {deg} deg camera yaw looks ({cx},{cz}) but the heading faces {f:?}"
            );
            assert!(heading_delta(cam, heading_to_camera_yaw(h)).abs() < 1e-5);
        }
    }

    #[test]
    fn camera_pitch_flips_so_positive_still_means_higher() {
        // A rig raising its eye by +pitch must hand the renderer a pitch that
        // puts the eye above the target, since the renderer subtracts forward.
        let p = 0.6f32;
        let render = heading_to_camera_pitch(p);
        // eye.y = target.y - sin(render) * distance
        assert!(
            -render.sin() > 0.0,
            "positive rig pitch must lift the render eye, got {render}"
        );
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
