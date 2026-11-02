//! One-Euro interaction poses. Apply once per input sample with a monotonic timestamp in
//! seconds; keep the original state for controller/hand rendering and navigation. Buttons,
//! head, tracking flags and timestamps are copied unchanged. Constant work/storage for two
//! hands, with no allocation: aim/grip for controllers, aim/palm/wrist and five tips for hands.

use makepad_widgets::makepad_platform::event::xr::{XrController, XrHand, XrState};
use makepad_widgets::*;

const MIN_CUTOFF: f32 = 1.0;
const DERIVATIVE_CUTOFF: f32 = 1.0;
// At 0.5 m/s the position cutoff is 71 Hz (~2.2 ms steady-state lag).
const POSITION_BETA: f32 = 140.0;
// Angular derivative is in radians/s. Filter the signed vector before taking its magnitude,
// so alternating tracking noise does not look like sustained rotation.
const ANGULAR_BETA: f32 = 8.0;
const TELEPORT_M: f32 = 0.5;

fn alpha(cutoff: f32, dt: f64) -> f32 {
    let x = std::f64::consts::TAU * cutoff as f64 * dt;
    (x / (1.0 + x)) as f32
}

fn unit(mut q: Quat) -> Quat { q.normalized() }

fn mix(a: Vec3f, b: Vec3f, t: f32) -> Vec3f {
    a + (b - a) * t
}

#[derive(Default)]
struct PoseFilter {
    last: Option<(Pose, Pose)>, // raw and filtered
    velocity: Vec3f,
    angular: Vec3f,
}

impl PoseFilter {
    fn apply(&mut self, pose: Pose, dt: f64) -> Pose {
        if !pose.is_finite() || pose.orientation.length() < 1e-6 {
            *self = Self::default();
            return pose;
        }
        let Some((raw, filtered)) = self.last else {
            self.last = Some((pose, pose));
            return pose;
        };
        if dt <= 0.0 {
            return filtered;
        }
        let da = alpha(DERIVATIVE_CUTOFF, dt);
        self.velocity = mix(self.velocity, (pose.position - raw.position) * (1.0 / dt) as f32, da);
        let axis = |old: f32, new: f32, v: f32| old + (new - old) * alpha(MIN_CUTOFF + POSITION_BETA * v.abs(), dt);
        let position = vec3f(
            axis(filtered.position.x, pose.position.x, self.velocity.x),
            axis(filtered.position.y, pose.position.y, self.velocity.y),
            axis(filtered.position.z, pose.position.z, self.velocity.z),
        );
        let q = unit(pose.orientation);
        let previous = unit(raw.orientation);
        let q = if previous.dot(q) < 0.0 { q.neg() } else { q };
        // World-space shortest rotation raw -> new; atan2 remains accurate at tiny angles.
        let delta = Quat::multiply(&previous.invert(), &q);
        let xyz = vec3f(delta.x, delta.y, delta.z);
        let sin = xyz.length();
        let angular = if sin > 1e-8 {
            xyz * (2.0 * sin.atan2(delta.w.max(0.0)) / sin / dt as f32)
        } else {
            Vec3f::default()
        };
        self.angular = mix(self.angular, angular, da);
        let t = alpha(MIN_CUTOFF + ANGULAR_BETA * self.angular.length(), dt);
        let from = filtered.orientation;
        let to = if from.dot(q) < 0.0 { q.neg() } else { q };
        let orientation = Quat {
            x: from.x + (to.x - from.x) * t,
            y: from.y + (to.y - from.y) * t,
            z: from.z + (to.z - from.z) * t,
            w: from.w + (to.w - from.w) * t,
        }.normalized();
        let result = Pose::new(orientation, position);
        self.last = Some((pose, result));
        result
    }

    fn jumped(&self, pose: Pose) -> bool {
        self.last.is_some_and(|(raw, _)| (pose.position - raw.position).length() > TELEPORT_M)
    }
}

#[derive(Default)]
struct HandFilter {
    mode: u8, // 0 inactive, 1 controller, 2 tracked hand
    time: Option<f64>,
    aim: PoseFilter,
    grip: PoseFilter,
    wrist: PoseFilter,
    tips: [PoseFilter; XrHand::TIP_COUNT],
}

impl HandFilter {
    fn apply(&mut self, c: &mut XrController, h: &mut XrHand, now: f64) {
        let mode = if c.active() { 1 } else if h.tracking_pose().is_some() { 2 } else { 0 };
        let anchor = if mode == 1 { c.grip_pose } else { h.joints[XrHand::CENTER] };
        let aim = if mode == 1 { c.aim_pose } else { h.aim_pose };
        if mode != self.mode || mode == 0 || !now.is_finite()
            || self.grip.jumped(anchor) || self.aim.jumped(aim)
        {
            *self = Self::default();
        }
        if mode == 0 || !now.is_finite() {
            return;
        }
        self.mode = mode;
        let first = self.time.is_none();
        let dt = self.time.map_or(0.0, |t| now - t);
        // A duplicate/out-of-order sample cannot rewind the filter clock.
        if self.time.is_none() || dt > 0.0 {
            self.time = Some(now);
        }
        if mode == 1 {
            c.aim_pose = self.aim.apply(c.aim_pose, dt);
            c.grip_pose = self.grip.apply(c.grip_pose, dt);
        } else {
            // Read tip validity against the original palm before filtering any joints.
            let tips: [Option<Pose>; XrHand::TIP_COUNT] = std::array::from_fn(|i| {
                h.tip_pos_checked(i).map(|p| Pose::new(h.joints[XrHand::END_KNUCKLES[i]].orientation, p))
            });
            if h.aim_valid() {
                h.aim_pose = self.aim.apply(h.aim_pose, dt);
            } else {
                self.aim = PoseFilter::default();
            }
            h.joints[XrHand::CENTER] = self.grip.apply(h.joints[XrHand::CENTER], dt);
            h.joints[XrHand::WRIST] = self.wrist.apply(h.joints[XrHand::WRIST], dt);
            for (i, tip) in tips.into_iter().enumerate() {
                if let Some(tip) = tip {
                    let filtered = self.tips[i].apply(tip, dt);
                    // Repack the filtered endpoint, preserving this sample's tip length.
                    let end = filtered.position - filtered.orientation.rotate_vec3(&vec3f(0.0, 0.0, -h.tips[i]));
                    if !first {
                        h.joints[XrHand::END_KNUCKLES[i]] = Pose::new(filtered.orientation, end);
                    }
                } else {
                    self.tips[i] = PoseFilter::default();
                }
            }
        }
    }
}

/// Per-hand interaction smoothing. Tracking loss, controller/hand switches and jumps over
/// half a metre reset only that hand; the first sample passes unchanged.
#[derive(Default)]
pub struct InteractionFilter {
    hands: [HandFilter; 2],
}

impl InteractionFilter {
    pub fn apply(&mut self, s: &XrState, now: f64) -> XrState {
        let mut out = s.clone();
        self.hands[0].apply(&mut out.left_controller, &mut out.left_hand, now);
        self.hands[1].apply(&mut out.right_controller, &mut out.right_hand, now);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn controller(pose: Pose) -> XrState {
        let mut s = XrState::default();
        s.right_controller = XrController { buttons: XrController::ACTIVE, aim_pose: pose, grip_pose: pose, ..Default::default() };
        s
    }

    fn noise(seed: &mut u32) -> f32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        ((*seed >> 8) as f32 / 16777215.0) * 2.0 - 1.0
    }

    fn angle(q: Quat) -> f32 {
        2.0 * vec3f(q.x, q.y, q.z).length().atan2(q.w.abs())
    }

    #[test]
    fn stationary_position_and_orientation_jitter_fall_below_one_third() {
        let mut filter = InteractionFilter::default();
        let mut seed = 12345;
        let mut raw = [0.0f64; 2];
        let mut smooth = [0.0f64; 2];
        let at = vec3f(0.2, 1.1, -0.4);
        for i in 0..2160 {
            let p = at + vec3f(noise(&mut seed), noise(&mut seed), noise(&mut seed)) * 0.0015;
            let q = Quat::from_axis_angle(vec3f(0.0, 1.0, 0.0), noise(&mut seed) * 0.3f32.to_radians());
            let s = controller(Pose::new(q, p));
            let out = filter.apply(&s, i as f64 / 72.0);
            if i >= 144 {
                raw[0] += (p - at).length().powi(2) as f64;
                raw[1] += angle(q).powi(2) as f64;
                smooth[0] += (out.right_controller.aim_pose.position - at).length().powi(2) as f64;
                smooth[1] += angle(out.right_controller.aim_pose.orientation).powi(2) as f64;
            }
        }
        for i in 0..2 {
            assert!(smooth[i].sqrt() < raw[i].sqrt() / 3.0, "axis {i}: RMS ratio {}", (smooth[i] / raw[i]).sqrt());
        }
    }

    #[test]
    fn metre_per_second_sweep_lags_less_than_eight_mm_including_start() {
        for axis in [vec3f(1.0, 0.0, 0.0), vec3f(0.0, 1.0, 0.0), vec3f(0.0, 0.0, 1.0)] {
            let mut filter = InteractionFilter::default();
            for i in 0..144 {
                let at = axis * (i as f32 / 72.0);
                let s = controller(Pose::new(Quat::default(), at));
                let out = filter.apply(&s, i as f64 / 72.0);
                for pose in [out.right_controller.aim_pose, out.right_controller.grip_pose] {
                    assert!((pose.position - at).length() < 0.008, "sample {i}");
                }
            }
        }
    }

    #[test]
    fn first_sample_and_resets_pass_unchanged_and_duplicate_time_does_not_advance() {
        let mut filter = InteractionFilter::default();
        let pose = Pose::new(Quat::from_axis_angle(vec3f(0.0, 1.0, 0.0), 0.4), vec3f(0.2, 1.1, -0.4));
        let mut s = controller(pose);
        let out = filter.apply(&s, 1.0);
        assert_eq!(out.right_controller.aim_pose, pose);
        assert_eq!(out.right_controller.grip_pose, pose);
        s.right_controller.aim_pose.position.x += 0.01;
        assert_eq!(filter.apply(&s, 1.0).right_controller.aim_pose, pose);
        assert_eq!(filter.apply(&s, 0.5).right_controller.aim_pose, pose);
        s.right_controller.aim_pose.position.x += 0.6;
        assert_eq!(filter.apply(&s, 2.0).right_controller.aim_pose, s.right_controller.aim_pose);
        filter.apply(&XrState::default(), 3.0);
        assert_eq!(filter.apply(&s, 4.0).right_controller.aim_pose, s.right_controller.aim_pose);
    }
}
