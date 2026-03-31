use crate::prelude::*;

const CLOSED_FIST_TOUCH_FORWARD_OFFSET_METERS: f32 = 0.025;
const READY_FIST_TRACKING_FALLBACK_FORWARD_OFFSET_METERS: f32 = 0.040;

pub(crate) fn flat_head_forward(orientation: Quat) -> Vec3f {
    let mut forward = orientation.rotate_vec3(&vec3f(0.0, 0.0, -1.0));
    forward.y = 0.0;
    if forward.length() <= 1.0e-6 {
        vec3f(0.0, 0.0, -1.0)
    } else {
        forward.normalize()
    }
}

pub(crate) fn flat_head_right(orientation: Quat) -> Vec3f {
    let mut right = orientation.rotate_vec3(&vec3f(1.0, 0.0, 0.0));
    right.y = 0.0;
    if right.length() <= 1.0e-6 {
        vec3f(1.0, 0.0, 0.0)
    } else {
        right.normalize()
    }
}

pub(crate) fn hand_closed_fist_contact_point(
    hand: &XrHand,
    forward: Vec3f,
    is_left: bool,
) -> Option<Vec3f> {
    if !(hand.is_fist() && hand.is_palm_down(is_left)) {
        return None;
    }

    let knuckle_center = hand_knuckle_contact_center(hand).or_else(|| {
        hand.tracking_pose().map(|pose| {
            pose.position + forward.scale(READY_FIST_TRACKING_FALLBACK_FORWARD_OFFSET_METERS)
        })
    })?;

    Some(knuckle_center + forward.scale(CLOSED_FIST_TOUCH_FORWARD_OFFSET_METERS))
}

pub(crate) fn hand_closed_fist_contact_point_geometry_only(
    hand: &XrHand,
    forward: Vec3f,
    is_left: bool,
) -> Option<Vec3f> {
    hand_closed_fist_contact_point(hand, forward, is_left)
}

fn hand_knuckle_contact_center(hand: &XrHand) -> Option<Vec3f> {
    let mut sum = vec3f(0.0, 0.0, 0.0);
    let mut count = 0usize;
    for joint in [XrHand::INDEX_BASE, XrHand::MIDDLE_BASE] {
        if let Some(position) = hand.joint_pose_checked(joint).map(|pose| pose.position) {
            sum += position;
            count += 1;
        }
    }
    (count > 0).then_some(sum / count as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pose(position: Vec3f) -> Pose {
        Pose::new(Quat::default(), position)
    }

    fn mirror_x(is_left: bool, x: f32) -> f32 {
        if is_left {
            -x
        } else {
            x
        }
    }

    fn set_curled_finger(
        hand: &mut XrHand,
        is_left: bool,
        base_joint: usize,
        knuckle1_joint: usize,
        knuckle2_joint: usize,
        knuckle3_joint: usize,
        x: f32,
        y_bias: f32,
    ) {
        hand.joints[base_joint] = pose(vec3f(mirror_x(is_left, x), 1.18 + y_bias, -0.344));
        hand.joints[knuckle1_joint] =
            pose(vec3f(mirror_x(is_left, x * 0.88), 1.174 + y_bias, -0.365));
        hand.joints[knuckle2_joint] =
            pose(vec3f(mirror_x(is_left, x * 0.40), 1.153 + y_bias, -0.344));
        hand.joints[knuckle3_joint] =
            pose(vec3f(mirror_x(is_left, x * 0.12), 1.156 + y_bias, -0.320));
    }

    fn set_thumbs_up_thumb(hand: &mut XrHand, is_left: bool) {
        hand.joints[XrHand::THUMB_BASE] = pose(vec3f(mirror_x(is_left, 0.055), 1.168, -0.332));
        hand.joints[XrHand::THUMB_KNUCKLE1] = pose(vec3f(mirror_x(is_left, 0.060), 1.196, -0.326));
        hand.joints[XrHand::THUMB_KNUCKLE2] = pose(vec3f(mirror_x(is_left, 0.064), 1.228, -0.320));
    }

    fn make_ready_fist_hand(is_left: bool) -> XrHand {
        let mut hand = XrHand::default();
        hand.flags = XrHand::IN_VIEW | XrHand::AIM_VALID;
        hand.joints[XrHand::CENTER] = pose(vec3f(mirror_x(is_left, 0.0), 1.160, -0.300));
        hand.joints[XrHand::WRIST] = pose(vec3f(mirror_x(is_left, 0.0), 1.120, -0.330));
        set_curled_finger(
            &mut hand,
            is_left,
            XrHand::INDEX_BASE,
            XrHand::INDEX_KNUCKLE1,
            XrHand::INDEX_KNUCKLE2,
            XrHand::INDEX_KNUCKLE3,
            0.030,
            0.000,
        );
        set_curled_finger(
            &mut hand,
            is_left,
            XrHand::MIDDLE_BASE,
            XrHand::MIDDLE_KNUCKLE1,
            XrHand::MIDDLE_KNUCKLE2,
            XrHand::MIDDLE_KNUCKLE3,
            0.008,
            0.000,
        );
        set_curled_finger(
            &mut hand,
            is_left,
            XrHand::RING_BASE,
            XrHand::RING_KNUCKLE1,
            XrHand::RING_KNUCKLE2,
            XrHand::RING_KNUCKLE3,
            -0.015,
            -0.001,
        );
        set_curled_finger(
            &mut hand,
            is_left,
            XrHand::LITTLE_BASE,
            XrHand::LITTLE_KNUCKLE1,
            XrHand::LITTLE_KNUCKLE2,
            XrHand::LITTLE_KNUCKLE3,
            -0.038,
            -0.002,
        );
        hand
    }

    #[test]
    fn ready_fist_shape_accepts_joint_only_closed_pose_without_tip_bits() {
        let hand = make_ready_fist_hand(true);
        assert!(hand.is_fist());
    }

    #[test]
    fn ready_fist_rejects_straightened_index_finger() {
        let mut hand = make_ready_fist_hand(true);
        hand.joints[XrHand::INDEX_KNUCKLE1] = pose(vec3f(mirror_x(true, 0.028), 1.176, -0.366));
        hand.joints[XrHand::INDEX_KNUCKLE2] = pose(vec3f(mirror_x(true, 0.026), 1.172, -0.390));
        hand.joints[XrHand::INDEX_KNUCKLE3] = pose(vec3f(mirror_x(true, 0.024), 1.168, -0.414));
        assert!(!hand.is_fist());
    }

    #[test]
    fn finger_joint_only_helpers_survive_missing_outer_knuckle() {
        let mut hand = make_ready_fist_hand(true);
        let expected_index_end = hand.joints[XrHand::INDEX_KNUCKLE2].position;
        hand.joints[XrHand::INDEX_KNUCKLE3] = Pose::default();
        assert_eq!(
            hand.finger_end_joint_position(XrHand::INDEX_TIP),
            Some(expected_index_end)
        );
        assert!(hand
            .finger_max_bend_angle_degrees_joint_only(XrHand::INDEX_TIP)
            .is_some());
    }

    #[test]
    fn fist_contact_point_uses_index_and_middle_base_joints() {
        let mut hand = make_ready_fist_hand(true);
        hand.joints[XrHand::INDEX_BASE] = pose(vec3f(-0.032, 1.120, -0.290));
        hand.joints[XrHand::MIDDLE_BASE] = pose(vec3f(-0.008, 1.118, -0.286));
        hand.joints[XrHand::INDEX_KNUCKLE3] = pose(vec3f(-0.060, 1.180, -0.360));
        hand.joints[XrHand::MIDDLE_KNUCKLE3] = pose(vec3f(0.010, 1.184, -0.372));

        let expected_center =
            (hand.joints[XrHand::INDEX_BASE].position + hand.joints[XrHand::MIDDLE_BASE].position)
                * 0.5;

        assert_eq!(hand_knuckle_contact_center(&hand), Some(expected_center));
    }

    #[test]
    fn ready_fist_ignores_thumb_posture() {
        let mut hand = make_ready_fist_hand(true);
        set_thumbs_up_thumb(&mut hand, true);
        assert!(hand.is_fist());
    }
}
