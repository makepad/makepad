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
        hand.joints[XrHand::THUMB_KNUCKLE1] =
            pose(vec3f(mirror_x(is_left, 0.060), 1.196, -0.326));
        hand.joints[XrHand::THUMB_KNUCKLE2] =
            pose(vec3f(mirror_x(is_left, 0.064), 1.228, -0.320));
    }

    fn set_open_finger(
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
            pose(vec3f(mirror_x(is_left, x), 1.182 + y_bias, -0.374));
        hand.joints[knuckle2_joint] =
            pose(vec3f(mirror_x(is_left, x), 1.184 + y_bias, -0.404));
        hand.joints[knuckle3_joint] =
            pose(vec3f(mirror_x(is_left, x), 1.186 + y_bias, -0.434));
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

    fn make_ready_open_hand(is_left: bool) -> XrHand {
        let mut hand = XrHand::default();
        hand.flags = XrHand::IN_VIEW | XrHand::AIM_VALID;
        hand.joints[XrHand::CENTER] = pose(vec3f(mirror_x(is_left, 0.0), 1.160, -0.300));
        hand.joints[XrHand::WRIST] = pose(vec3f(mirror_x(is_left, 0.0), 1.120, -0.330));
        set_open_finger(
            &mut hand,
            is_left,
            XrHand::INDEX_BASE,
            XrHand::INDEX_KNUCKLE1,
            XrHand::INDEX_KNUCKLE2,
            XrHand::INDEX_KNUCKLE3,
            0.030,
            0.000,
        );
        set_open_finger(
            &mut hand,
            is_left,
            XrHand::MIDDLE_BASE,
            XrHand::MIDDLE_KNUCKLE1,
            XrHand::MIDDLE_KNUCKLE2,
            XrHand::MIDDLE_KNUCKLE3,
            0.008,
            0.000,
        );
        set_open_finger(
            &mut hand,
            is_left,
            XrHand::RING_BASE,
            XrHand::RING_KNUCKLE1,
            XrHand::RING_KNUCKLE2,
            XrHand::RING_KNUCKLE3,
            -0.015,
            -0.001,
        );
        set_open_finger(
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
    fn ready_open_hand_accepts_low_bend_pose() {
        let hand = make_ready_open_hand(true);
        assert!(hand.is_open());
        assert!(hand
            .average_open_finger_bend_degrees()
            .is_some_and(|average| average <= XrHand::OPEN_MAX_AVERAGE_FINGER_BEND_DEGREES));
    }

    #[test]
    fn ready_open_hand_rejects_curled_index_finger() {
        let mut hand = make_ready_open_hand(true);
        hand.joints[XrHand::INDEX_KNUCKLE1] = pose(vec3f(mirror_x(true, 0.026), 1.178, -0.360));
        hand.joints[XrHand::INDEX_KNUCKLE2] = pose(vec3f(mirror_x(true, 0.016), 1.162, -0.344));
        hand.joints[XrHand::INDEX_KNUCKLE3] = pose(vec3f(mirror_x(true, 0.004), 1.160, -0.322));
        assert!(!hand.is_open());
    }

    #[test]
    fn contact_point_uses_tracking_palm_center_for_open_hand() {
        let hand = make_ready_open_hand(true);
        let forward = vec3f(0.0, 0.0, -1.0);
        let expected_point = hand.tracking_pose().expect("tracking pose").position
            + forward.scale(OPEN_HAND_SYNC_FORWARD_OFFSET_METERS);

        assert_eq!(
            hand_closed_fist_contact_point(&hand, forward, true),
            Some(expected_point)
        );
    }

    #[test]
    fn ready_fist_ignores_thumb_posture() {
        let mut hand = make_ready_fist_hand(true);
        set_thumbs_up_thumb(&mut hand, true);
        assert!(hand.is_fist());
    }
}
