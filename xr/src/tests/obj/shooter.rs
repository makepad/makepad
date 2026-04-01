mod tests {
    use super::*;
    use std::rc::Rc;

    fn point_pose(base: Vec3f, z: f32) -> Pose {
        Pose::new(Quat::default(), base + vec3f(0.0, 0.0, z))
    }

    fn make_pointing_hand() -> XrHand {
        let base = vec3f(0.20, 1.22, -0.22);
        let mut hand = XrHand::default();
        hand.flags = XrHand::IN_VIEW | XrHand::AIM_VALID | XrHand::DOMINANT_HAND;
        hand.tips_active = 1 << XrHand::INDEX_TIP;
        hand.tips[XrHand::INDEX_TIP] = 0.038;

        hand.joints[XrHand::CENTER] = Pose::new(Quat::default(), base + vec3f(0.0, -0.03, 0.03));
        hand.joints[XrHand::WRIST] = Pose::new(Quat::default(), base + vec3f(0.0, -0.05, 0.08));
        hand.joints[XrHand::INDEX_BASE] = point_pose(base, 0.0);
        hand.joints[XrHand::INDEX_KNUCKLE1] = point_pose(base, -0.041);
        hand.joints[XrHand::INDEX_KNUCKLE2] =
            point_pose(base + vec3f(0.001, 0.002, 0.0), -0.082);
        hand.joints[XrHand::INDEX_KNUCKLE3] =
            point_pose(base + vec3f(0.002, 0.004, 0.0), -0.122);
        hand.aim_pose = Pose::new(Quat::default(), base + vec3f(0.0, 0.0, -0.16));
        hand
    }

    fn make_curled_hand() -> XrHand {
        let base = vec3f(0.20, 1.22, -0.22);
        let mut hand = XrHand::default();
        hand.flags = XrHand::IN_VIEW | XrHand::AIM_VALID;
        hand.tips_active = 1 << XrHand::INDEX_TIP;
        hand.tips[XrHand::INDEX_TIP] = 0.030;

        hand.joints[XrHand::CENTER] = Pose::new(Quat::default(), base + vec3f(0.0, -0.03, 0.03));
        hand.joints[XrHand::WRIST] = Pose::new(Quat::default(), base + vec3f(0.0, -0.05, 0.08));
        hand.joints[XrHand::INDEX_BASE] = point_pose(base, 0.0);
        hand.joints[XrHand::INDEX_KNUCKLE1] = point_pose(base, -0.030);
        hand.joints[XrHand::INDEX_KNUCKLE2] =
            Pose::new(Quat::default(), base + vec3f(0.018, -0.012, -0.040));
        hand.joints[XrHand::INDEX_KNUCKLE3] =
            Pose::new(Quat::default(), base + vec3f(0.034, -0.030, -0.032));
        hand.aim_pose = Pose::new(Quat::default(), base + vec3f(0.0, 0.0, -0.12));
        hand
    }

    fn make_sparse_tracking_hand() -> XrHand {
        let base = vec3f(0.20, 1.22, -0.22);
        let mut hand = XrHand::default();
        hand.flags = XrHand::IN_VIEW | XrHand::AIM_VALID;
        hand.tips_active = XrHand::GRAB_ACTIVE;
        hand.joints[XrHand::CENTER] = Pose::new(Quat::default(), base + vec3f(0.0, -0.03, 0.03));
        hand.joints[XrHand::WRIST] = Pose::new(Quat::default(), base + vec3f(0.0, -0.05, 0.08));
        hand.aim_pose = Pose::new(Quat::default(), base + vec3f(0.0, 0.0, -0.12));
        hand
    }

    fn make_pointing_hand_with_sideways_aim_pose() -> XrHand {
        let mut hand = make_pointing_hand();
        hand.aim_pose.orientation = Quat::from_axis_angle(vec3f(0.0, 1.0, 0.0), 0.75);
        hand
    }

    #[test]
    fn point_gesture_is_considered_an_emit_gesture() {
        let hand = make_pointing_hand();
        assert!(Shooter::hand_emit_gesture_active(&hand));
        assert!(Shooter::projectile_emitter_pose(&hand).is_some());
    }

    #[test]
    fn generic_grab_bit_does_not_block_emit_gesture_when_pointing_pose_is_valid() {
        let mut hand = make_pointing_hand();
        hand.tips_active |= XrHand::GRAB_ACTIVE;
        assert!(Shooter::hand_emit_gesture_active(&hand));
        assert!(Shooter::projectile_emitter_pose(&hand).is_some());
    }

    #[test]
    fn point_gesture_without_tip_active_bit_still_emits_from_joint_chain() {
        let mut hand = make_pointing_hand();
        hand.tips_active &= !(1 << XrHand::INDEX_TIP);
        let metrics = Shooter::hand_index_finger_stretch_metrics(&hand)
            .expect("joint-chain metrics should still be derivable without the tip-active bit");
        assert!(metrics.max_bend_angle_degrees <= SHOOTER_INDEX_BEND_MAX_DEGREES);
        assert!(Shooter::hand_emit_gesture_active(&hand));
        assert!(Shooter::projectile_emitter_pose(&hand).is_some());
    }

    #[test]
    fn projectile_direction_follows_finger_chain_not_openxr_aim_ray() {
        let hand = make_pointing_hand_with_sideways_aim_pose();
        let (_, direction) = Shooter::projectile_emitter_pose(&hand)
            .expect("pointing hand should emit even if aim pose diverges");
        assert!(direction.z < -0.9, "{direction:?}");
        assert!(direction.x.abs() < 0.2, "{direction:?}");
    }

    #[test]
    fn curled_index_finger_is_rejected() {
        let hand = make_curled_hand();
        assert!(
            Shooter::hand_index_finger_stretch_metrics(&hand).is_some_and(|metrics| {
                metrics.max_bend_angle_degrees > SHOOTER_INDEX_BEND_MAX_DEGREES
            })
        );
        assert!(!Shooter::hand_emit_gesture_active(&hand));
    }

    #[test]
    fn sparse_tracking_sample_is_rejected_for_emit_gesture() {
        let hand = make_sparse_tracking_hand();
        assert!(Shooter::hand_index_finger_stretch_metrics(&hand).is_none());
        assert!(!Shooter::hand_emit_gesture_active(&hand));
        assert!(Shooter::projectile_emitter_pose(&hand).is_none());
    }

    #[test]
    fn dominant_hand_selection_prefers_pointing_hand() {
        let mut update = XrUpdateEvent {
            state: Rc::new(XrState::default()),
            last: Rc::new(XrState::default()),
        };
        let mut left = make_pointing_hand();
        left.flags |= XrHand::DOMINANT_HAND;
        let right = make_curled_hand();
        Rc::make_mut(&mut update.state).left_hand = left;
        Rc::make_mut(&mut update.state).right_hand = right;
        let emitter = Shooter::main_projectile_emitter_pose(&update);
        assert!(emitter.is_some());
        let (pos, dir) = emitter.unwrap();
        assert!(pos.z < -0.30);
        assert!(dir.z < -0.8);
    }
}
