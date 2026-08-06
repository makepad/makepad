//! Positional audio: the listener and the pan/attenuation math.
//!
//! Audio was 2D and global — there was no listener at all, so `sfx` from an
//! object across the map arrived at the same volume as one at your feet.
//! The mix stays host-side (the synth is not a sim concern), so this module
//! owns only the geometry: given a listener and a sound's world position,
//! what gain and what stereo pan.
//!
//! The listener is the local player's camera, which makes this Local-tier
//! by construction (game.md): two devices in the same room hear the same
//! game differently, and nothing about that reaches the wire.

use makepad_widgets::*;

/// Where the local player is listening from. `forward` and `right` are the
/// camera's basis on the ground plane.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Listener {
    pub pos: Vec3f,
    pub forward: Vec3f,
    pub right: Vec3f,
}

impl Default for Listener {
    fn default() -> Self {
        Self {
            pos: Vec3f::default(),
            forward: Vec3f {
                x: 0.0,
                y: 0.0,
                z: -1.0,
            },
            right: Vec3f {
                x: 1.0,
                y: 0.0,
                z: 0.0,
            },
        }
    }
}

impl Listener {
    /// Build from a camera yaw (radians) and position. Matches the engine's
    /// camera basis: forward = (sin yaw, 0, -cos yaw), right = perpendicular.
    pub fn from_yaw(pos: Vec3f, yaw: f32) -> Self {
        let (s, c) = (yaw.sin(), yaw.cos());
        Self {
            pos,
            forward: Vec3f { x: s, y: 0.0, z: -c },
            right: Vec3f { x: c, y: 0.0, z: s },
        }
    }
}

/// How a positioned sound should be mixed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    /// 0..1 distance attenuation.
    pub gain: f32,
    /// -1 fully left, 0 centre, +1 fully right.
    pub pan: f32,
}

/// Distance past which a sound is inaudible unless the caller overrides it.
pub const DEFAULT_RANGE: f32 = 40.0;
/// Inside this radius a sound is at full volume and centred — otherwise
/// anything at the listener's own position pans wildly on tiny movements.
const NEAR_FIELD: f32 = 1.0;

/// Gain and pan for a sound at `at`, heard by `listener`, audible out to
/// `range`. Attenuation is linear-in-the-square-root (gentler than inverse
/// square, which drops off too fast to be useful in an arcade game).
pub fn place(listener: &Listener, at: Vec3f, range: f32) -> Placement {
    let range = range.max(0.001);
    let d = Vec3f {
        x: at.x - listener.pos.x,
        y: at.y - listener.pos.y,
        z: at.z - listener.pos.z,
    };
    let dist = (d.x * d.x + d.y * d.y + d.z * d.z).sqrt();
    if dist >= range {
        return Placement { gain: 0.0, pan: 0.0 };
    }
    let t = (dist / range).clamp(0.0, 1.0);
    let gain = 1.0 - t * t;
    if dist <= NEAR_FIELD {
        // Near field: full volume, and pan settles to centre as you close
        // in so a sound at your feet does not flip channels.
        let lateral = d.x * listener.right.x + d.y * listener.right.y + d.z * listener.right.z;
        let pan = (lateral / NEAR_FIELD).clamp(-1.0, 1.0) * (dist / NEAR_FIELD);
        return Placement { gain, pan };
    }
    let lateral = d.x * listener.right.x + d.y * listener.right.y + d.z * listener.right.z;
    // Normalise by distance so pan depends on angle, not how far away it is.
    let pan = (lateral / dist).clamp(-1.0, 1.0);
    Placement { gain, pan }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: f32, y: f32, z: f32) -> Vec3f {
        Vec3f { x, y, z }
    }

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    /// Facing -z (yaw 0): right is +x, forward is -z.
    fn listener() -> Listener {
        Listener::from_yaw(v(0.0, 0.0, 0.0), 0.0)
    }

    #[test]
    fn a_sound_to_the_right_pans_right_and_left_pans_left() {
        let l = listener();
        let right = place(&l, v(10.0, 0.0, 0.0), DEFAULT_RANGE);
        let left = place(&l, v(-10.0, 0.0, 0.0), DEFAULT_RANGE);
        assert!(approx(right.pan, 1.0), "pan {}", right.pan);
        assert!(approx(left.pan, -1.0), "pan {}", left.pan);
        assert!(approx(right.gain, left.gain));
    }

    #[test]
    fn straight_ahead_and_straight_behind_are_centred() {
        let l = listener();
        let ahead = place(&l, v(0.0, 0.0, -10.0), DEFAULT_RANGE);
        let behind = place(&l, v(0.0, 0.0, 10.0), DEFAULT_RANGE);
        assert!(approx(ahead.pan, 0.0));
        assert!(approx(behind.pan, 0.0));
        // Stereo pan cannot express front/back; both are audible and equal.
        assert!(approx(ahead.gain, behind.gain));
    }

    #[test]
    fn gain_falls_with_distance_and_hits_zero_at_range() {
        let l = listener();
        let near = place(&l, v(0.0, 0.0, -2.0), 20.0);
        let mid = place(&l, v(0.0, 0.0, -10.0), 20.0);
        let far = place(&l, v(0.0, 0.0, -19.0), 20.0);
        assert!(near.gain > mid.gain && mid.gain > far.gain);
        assert_eq!(place(&l, v(0.0, 0.0, -20.0), 20.0).gain, 0.0);
        assert_eq!(place(&l, v(0.0, 0.0, -100.0), 20.0).gain, 0.0);
    }

    #[test]
    fn at_the_listener_it_is_full_volume_and_centred() {
        let l = listener();
        let p = place(&l, v(0.0, 0.0, 0.0), DEFAULT_RANGE);
        assert!(approx(p.gain, 1.0));
        assert!(approx(p.pan, 0.0), "pan {}", p.pan);
    }

    #[test]
    fn near_field_pan_eases_in_rather_than_flipping() {
        let l = listener();
        // Just off-centre, very close: pan must be small, not hard right.
        let close = place(&l, v(0.05, 0.0, 0.0), DEFAULT_RANGE);
        assert!(close.pan.abs() < 0.2, "pan {}", close.pan);
        let further = place(&l, v(5.0, 0.0, 0.0), DEFAULT_RANGE);
        assert!(further.pan > close.pan);
    }

    #[test]
    fn turning_the_listener_swaps_the_channels() {
        let facing_forward = Listener::from_yaw(v(0.0, 0.0, 0.0), 0.0);
        let turned = Listener::from_yaw(v(0.0, 0.0, 0.0), std::f32::consts::PI);
        let at = v(10.0, 0.0, 0.0);
        let a = place(&facing_forward, at, DEFAULT_RANGE);
        let b = place(&turned, at, DEFAULT_RANGE);
        assert!(a.pan > 0.5 && b.pan < -0.5, "{} vs {}", a.pan, b.pan);
    }

    #[test]
    fn height_counts_toward_distance() {
        let l = listener();
        let level = place(&l, v(5.0, 0.0, 0.0), 20.0);
        let high = place(&l, v(5.0, 15.0, 0.0), 20.0);
        assert!(high.gain < level.gain);
    }
}
