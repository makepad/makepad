//! Carrying a tracked pose ahead by its velocity (the XR uplink's controllers): the pose and
//! velocity as the runtime located them for a display time, moved to a later target.
use crate::XrPose;

/// The angular velocity to carry with: of this sample's and the one before, the slower
/// (a one-sample spike in the runtime's angular velocity carried tens of ms was a one-sample
/// swing of up to 59 degrees; slowing down, the slower one is the new one: less overshoot at a
/// stop). Both in the reference space, rad/s.
pub(crate) fn steadier(now: [f32; 3], before: [f32; 3]) -> [f32; 3] {
    let m = |v: [f32; 3]| v[0] * v[0] + v[1] * v[1] + v[2] * v[2];
    if m(before) < m(now) { before } else { now }
}

/// The most a carried pose is turned by its angular velocity (radians: 10 degrees).
const CARRY_MAX_TURN: f32 = 0.1745;

/// A pose located with its velocity (`(pose, m/s, rad/s, flags)`: location flags, velocity
/// flags << 8) carried `lead_s` ahead: the position by the linear velocity, the orientation by
/// the angular velocity (in the reference space: applied on the left) over `angular_share` of
/// it, at most `CARRY_MAX_TURN`. `None` when it is not tracked.
pub(crate) fn carried(located: (XrPose, [f32; 3], [f32; 3], u32), lead_s: f32, angular_share: f32) -> Option<XrPose> {
    let (p, lin, ang, flags) = located;
    if flags & 0b11 != 0b11 { return None; }
    let (lin_ok, ang_ok) = ((flags >> 8) & 1 == 1, (flags >> 8) & 2 == 2);
    let mut out = p;
    if lin_ok { for i in 0..3 { out.position[i] += lin[i] * lead_s; } }
    if ang_ok {
        let t = lead_s * angular_share;
        let w = (ang[0] * ang[0] + ang[1] * ang[1] + ang[2] * ang[2]).sqrt();
        if w * t > 1e-6 {
            let turn = (w * t).min(CARRY_MAX_TURN);
            let (s, c) = (turn * 0.5).sin_cos();
            let k = s / w;
            let d = [ang[0] * k, ang[1] * k, ang[2] * k, c];
            let q = p.orientation;
            // (the angular velocity is in the reference space: the turn applies on the left)
            out.orientation = [
                d[3] * q[0] + d[0] * q[3] + d[1] * q[2] - d[2] * q[1],
                d[3] * q[1] - d[0] * q[2] + d[1] * q[3] + d[2] * q[0],
                d[3] * q[2] + d[0] * q[1] - d[1] * q[0] + d[2] * q[3],
                d[3] * q[3] - d[0] * q[0] - d[1] * q[1] - d[2] * q[2]];
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn qmul(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
        [a[3] * b[0] + a[0] * b[3] + a[1] * b[2] - a[2] * b[1], a[3] * b[1] - a[0] * b[2] + a[1] * b[3] + a[2] * b[0],
         a[3] * b[2] + a[0] * b[1] - a[1] * b[0] + a[2] * b[3], a[3] * b[3] - a[0] * b[0] - a[1] * b[1] - a[2] * b[2]]
    }
    fn axis_angle(axis: [f32; 3], angle: f32) -> [f32; 4] {
        let n = (axis[0] * axis[0] + axis[1] * axis[1] + axis[2] * axis[2]).sqrt();
        let (s, c) = (angle * 0.5).sin_cos();
        [axis[0] / n * s, axis[1] / n * s, axis[2] / n * s, c]
    }
    fn angle_between(a: [f32; 4], b: [f32; 4]) -> f32 {
        let d: f32 = (0..4).map(|i| a[i] * b[i]).sum();
        2.0 * d.abs().min(1.0).acos()
    }

    /// A steady turn about a world axis, sampled every 11.1 ms from several starting
    /// orientations: the carried pose turns by the same angle every sample, and leads the
    /// located one by the angular share of the lead.
    #[test]
    fn steady_turn_carries_evenly_at_any_orientation() {
        let (dt, lead, share) = (1.0 / 90.0f32, 0.06f32, 0.5f32);
        for (axis, rate) in [([0.0f32, 1.0, 0.0], 2.0f32), ([1.0, 0.3, -0.2], 1.0), ([0.0, 0.0, 1.0], 3.0)] {
            let n = (axis[0] * axis[0] + axis[1] * axis[1] + axis[2] * axis[2]).sqrt();
            let w = [axis[0] / n * rate, axis[1] / n * rate, axis[2] / n * rate];
            for start in [[0.0, 0.0, 0.0, 1.0], axis_angle([1.0, 0.0, 0.0], 1.2), axis_angle([0.3, -0.7, 0.5], 2.5)] {
                let mut last: Option<[f32; 4]> = None;
                for k in 0..20 {
                    let q = qmul(axis_angle(axis, rate * dt * k as f32), start);
                    let out = carried((XrPose { orientation: q, position: [0.0; 3] }, [0.0; 3], w, 0b11 | (0b11 << 8)), lead, share).unwrap();
                    assert!((angle_between(out.orientation, q) - rate * lead * share).abs() < 1e-3);
                    if let Some(l) = last {
                        assert!((angle_between(out.orientation, l) - rate * dt).abs() < 1e-3, "per-sample angle {} vs {}", angle_between(out.orientation, l), rate * dt);
                    }
                    last = Some(out.orientation);
                }
            }
        }
    }

    /// A one-sample spike in the angular velocity is not carried, and no carry turns more
    /// than `CARRY_MAX_TURN`.
    #[test]
    fn spikes_and_large_turns_are_bounded() {
        assert_eq!(steadier([50.0, 0.0, 0.0], [1.0, 0.0, 0.0]), [1.0, 0.0, 0.0]);
        let q = [0.0, 0.0, 0.0, 1.0];
        let out = carried((XrPose { orientation: q, position: [0.0; 3] }, [0.0; 3], [0.0, 40.0, 0.0], 0b11 | (0b11 << 8)), 0.08, 1.0).unwrap();
        assert!(angle_between(out.orientation, q) <= CARRY_MAX_TURN + 1e-4);
    }
}
