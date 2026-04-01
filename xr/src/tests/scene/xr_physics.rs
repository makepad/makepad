mod tests {
    use super::*;
    use crate::algorithms::tsdf_query::{DepthQueryCollider, DepthQuerySupportPlane};

    fn assert_vec3_close(actual: Vec3f, expected: Vec3f, tolerance: f32) {
        assert!(
            (actual - expected).length() <= tolerance,
            "expected {:?} to be within {} of {:?}",
            actual,
            tolerance,
            expected
        );
    }

    #[test]
    fn impact_surface_enables_before_current_body_overlaps_quad() {
        let plane = DepthQuerySupportPlane {
            point: vec3f(1.0, 0.0, 0.0),
            normal: vec3f(-1.0, 0.0, 0.0),
            tangent: vec3f(0.0, 1.0, 0.0),
            bitangent: vec3f(0.0, 0.0, 1.0),
            half_extent_tangent: 0.08,
            half_extent_bitangent: 0.08,
        };
        let target = DepthQuerySurfaceTarget {
            collider: DepthQueryCollider {
                fingerprint: 1,
                geometry: DepthQueryColliderGeometry::HalfSpace(plane),
                role: DepthQueryColliderRole::Impact,
                restitution: 0.38,
            },
        };

        assert!(depth_query_surface_target_should_enable(
            target,
            vec3f(0.78, 0.0, 0.0),
            vec3f(0.55, 0.0, 0.0),
            0.05,
            0.004,
        ));
    }

    #[test]
    fn respawn_body_applies_shadow_contact_dominated_and_sleeping_modes() {
        let mut scene = RapierScene::new(0.0);
        let widget_uid = WidgetUid(41);
        let pose = Pose::new(Quat::default(), vec3f(0.08, 1.12, -0.44));
        scene.spawn_dynamic_box(
            widget_uid,
            pose,
            vec3f(0.04, 0.04, 0.04),
            vec3f(1.0, 1.0, 1.0),
            1.0,
            0.5,
            0.0,
        );
        let cube = scene.cubes[0];

        scene.respawn_body(
            widget_uid,
            true,
            XrSharedObjectMode::Dynamic,
            pose,
            vec3f(1.0, 2.0, 3.0),
            vec3f(4.0, 5.0, 6.0),
        );
        let body = scene
            .bodies
            .get(cube.body)
            .expect("spawned cube body should exist");
        assert_eq!(body.body_type(), RigidBodyType::KinematicPositionBased);
        assert_eq!(
            scene.shadow_body_motion_for_body(cube.body),
            Some((vec3f(1.0, 2.0, 3.0), vec3f(4.0, 5.0, 6.0)))
        );

        scene.respawn_body(
            widget_uid,
            false,
            XrSharedObjectMode::ContactDominated {
                authority: XrNetPeerId(7),
                hand: XrSharedHand::RightHand,
            },
            pose,
            vec3f(0.5, 0.0, -0.5),
            vec3f(0.0, 1.0, 0.0),
        );
        let body = scene
            .bodies
            .get(cube.body)
            .expect("cube body should still exist after contact-dominated respawn");
        assert_eq!(body.body_type(), RigidBodyType::KinematicPositionBased);
        assert!(!body.is_sleeping());

        scene.respawn_body(
            widget_uid,
            false,
            XrSharedObjectMode::Sleeping,
            pose,
            vec3f(0.0, 0.0, 0.0),
            vec3f(0.0, 0.0, 0.0),
        );
        let body = scene
            .bodies
            .get(cube.body)
            .expect("cube body should still exist after sleeping respawn");
        assert_eq!(body.body_type(), RigidBodyType::Dynamic);
        assert_vec3_close(
            vec3f(body.linvel().x, body.linvel().y, body.linvel().z),
            vec3f(0.0, 0.0, 0.0),
            0.0001,
        );
        assert_vec3_close(
            vec3f(body.angvel().x, body.angvel().y, body.angvel().z),
            vec3f(0.0, 0.0, 0.0),
            0.0001,
        );
    }

    #[test]
    fn shadow_respawn_keeps_projecting_motion_between_corrections() {
        let mut scene = RapierScene::new(0.0);
        let widget_uid = WidgetUid(410);
        let pose = Pose::new(Quat::default(), vec3f(0.0, 1.0, -0.2));
        scene.spawn_dynamic_box(
            widget_uid,
            pose,
            vec3f(0.05, 0.05, 0.05),
            vec3f(1.0, 1.0, 1.0),
            1.0,
            0.5,
            0.0,
        );
        let cube = scene.cubes[0];

        scene.respawn_body(
            widget_uid,
            true,
            XrSharedObjectMode::Dynamic,
            pose,
            vec3f(0.0, 0.0, -1.5),
            vec3f(0.0, 0.0, 0.0),
        );
        scene.step();

        let body = scene
            .bodies
            .get(cube.body)
            .expect("shadow body should exist after a step");
        let stepped_pose = makepad_pose(body.position());
        assert_eq!(body.body_type(), RigidBodyType::KinematicPositionBased);
        assert!(
            stepped_pose.position.z < pose.position.z - 0.0001,
            "shadow body should keep moving forward between network corrections: {stepped_pose:?}"
        );
        assert_eq!(
            scene
                .shadow_body_motion_for_body(cube.body)
                .map(|(linvel, _)| linvel)
                .unwrap_or_default(),
            vec3f(0.0, 0.0, -1.5)
        );
    }

    #[test]
    fn shadow_dynamic_body_ignores_local_wall_until_extrapolation_runs_out() {
        let mut scene = RapierScene::new(0.0);
        let projectile_uid = WidgetUid(411);
        let wall_uid = WidgetUid(412);
        let pose = Pose::new(Quat::default(), vec3f(0.0, 1.0, -0.2));
        scene.spawn_dynamic_sphere(
            projectile_uid,
            pose,
            0.05,
            vec3f(1.0, 1.0, 1.0),
            1.0,
            0.0,
            0.95,
        );
        scene.spawn_fixed_box(
            wall_uid,
            Pose::new(Quat::default(), vec3f(0.0, 1.0, -0.55)),
            vec3f(1.0, 1.0, 0.05),
            vec3f(1.0, 1.0, 1.0),
            0.0,
            0.95,
        );
        let cube = scene.cubes[0];

        scene.respawn_body(
            projectile_uid,
            true,
            XrSharedObjectMode::Dynamic,
            pose,
            vec3f(0.0, 0.0, -2.0),
            vec3f(0.0, 0.0, 0.0),
        );
        for _ in 0..30 {
            scene.step();
        }

        let body = scene
            .bodies
            .get(cube.body)
            .expect("shadow projectile should still exist after stepping");
        assert!(
            makepad_pose(body.position()).position.z <= -0.39,
            "shadow projectile should keep dead-reckoning forward instead of locally bouncing on observer-only walls: {:?}",
            makepad_pose(body.position()).position
        );
        assert!(
            makepad_pose(body.position()).position.z >= -0.45,
            "shadow projectile should stop near the extrapolation horizon when no fresh authority samples arrive: {:?}",
            makepad_pose(body.position()).position
        );
        assert_eq!(body.body_type(), RigidBodyType::KinematicPositionBased);
    }

    #[test]
    fn apply_impulse_only_affects_dynamic_enabled_bodies() {
        let mut scene = RapierScene::new(0.0);
        let widget_uid = WidgetUid(42);
        let pose = Pose::new(Quat::default(), vec3f(0.0, 1.0, 0.0));
        scene.spawn_dynamic_box(
            widget_uid,
            pose,
            vec3f(0.05, 0.05, 0.05),
            vec3f(1.0, 1.0, 1.0),
            1.0,
            0.5,
            0.0,
        );
        let cube = scene.cubes[0];

        assert!(scene.apply_impulse(widget_uid, pose.position, vec3f(0.0, 0.0, -1.0),));
        let body = scene
            .bodies
            .get(cube.body)
            .expect("cube body should exist after impulse");
        assert!(body.linvel().z < 0.0);

        scene.respawn_body(
            widget_uid,
            true,
            XrSharedObjectMode::Dynamic,
            pose,
            vec3f(0.0, 0.0, 0.0),
            vec3f(0.0, 0.0, 0.0),
        );
        assert!(
            !scene.apply_impulse(widget_uid, pose.position, vec3f(0.0, 0.0, -1.0)),
            "shadow bodies should reject dynamic impulses"
        );

        scene.despawn_body(widget_uid);
        assert!(
            !scene.apply_impulse(widget_uid, pose.position, vec3f(0.0, 0.0, -1.0)),
            "disabled bodies should reject impulses"
        );
    }

    #[test]
    fn spawn_pool_respawn_reenables_body_and_survives_a_step() {
        let mut scene = RapierScene::new(0.0);
        let widget_uid = WidgetUid(420);
        let pose = Pose::new(Quat::default(), vec3f(0.0, 1.0, -0.4));
        scene.spawn_dynamic_sphere(widget_uid, pose, 0.04, vec3f(1.0, 1.0, 1.0), 1.0, 0.5, 0.0);
        scene.register_spawn_pool_cube(0);

        let cube = scene.cubes[0];
        let body = scene
            .bodies
            .get(cube.body)
            .expect("projectile body should exist before respawn");
        assert!(!body.is_enabled(), "projectile pool bodies start disabled");

        scene.respawn_body(
            widget_uid,
            false,
            XrSharedObjectMode::Dynamic,
            pose,
            vec3f(0.0, 0.0, -8.0),
            vec3f(0.0, 0.0, 0.0),
        );

        let body = scene
            .bodies
            .get(cube.body)
            .expect("projectile body should still exist after respawn");
        assert!(body.is_enabled(), "respawn should re-enable the pooled body");
        assert_eq!(body.body_type(), RigidBodyType::Dynamic);

        scene.step();

        let body = scene
            .bodies
            .get(cube.body)
            .expect("projectile body should still exist after a step");
        assert!(
            body.is_enabled(),
            "a respawned pooled body should remain enabled after stepping"
        );
    }

    #[test]
    fn hand_contact_grab_and_release_restores_dynamic_body_velocity() {
        let mut scene = RapierScene::new(0.0);
        let widget_uid = WidgetUid(43);
        let pose = Pose::new(Quat::default(), vec3f(0.0, 1.0, 0.0));
        scene.spawn_dynamic_box(
            widget_uid,
            pose,
            vec3f(0.05, 0.05, 0.05),
            vec3f(1.0, 1.0, 1.0),
            1.0,
            0.5,
            0.0,
        );
        let cube = scene.cubes[0];
        let hand_pose = Pose::new(Quat::default(), pose.position);

        RapierScene::sync_hand_bodies(
            &scene.left_hand,
            &[HandCollider::Ball {
                center: pose.position,
                radius: 0.09,
            }],
            &mut scene.bodies,
            &mut scene.colliders,
        );
        scene.left_hand_grab = HandGrabState {
            shared_hand: XrSharedHand::LeftHand,
            pose: hand_pose,
            previous_pose: hand_pose,
            linvel: vec3f(0.0, 0.0, 0.0),
            tracked: true,
            gripping: true,
            held_body: None,
            grab_offset: Pose::default(),
        };

        scene.step();
        assert_eq!(scene.left_hand_grab.held_body, Some(cube.body));
        let body = scene
            .bodies
            .get(cube.body)
            .expect("cube body should exist after grab");
        assert_eq!(body.body_type(), RigidBodyType::KinematicPositionBased);
        assert_eq!(
            scene.held_by_for_body(cube.body),
            Some(XrSharedHand::LeftHand)
        );

        let release_velocity = vec3f(0.6, 0.0, -0.4);
        scene.left_hand_grab.linvel = release_velocity;
        scene.left_hand_grab.gripping = false;
        scene.apply_held_body_targets();

        assert_eq!(scene.left_hand_grab.held_body, None);
        let body = scene
            .bodies
            .get(cube.body)
            .expect("cube body should still exist after release");
        assert_eq!(body.body_type(), RigidBodyType::Dynamic);
        assert_vec3_close(
            vec3f(body.linvel().x, body.linvel().y, body.linvel().z),
            release_velocity,
            0.0001,
        );
    }

    #[test]
    fn hand_grab_anchors_body_surface_to_hand_pose_instead_of_body_center() {
        let mut scene = RapierScene::new(0.0);
        let widget_uid = WidgetUid(430);
        let pose = Pose::new(Quat::default(), vec3f(0.0, 1.0, 0.0));
        let half_extents = vec3f(0.05, 0.05, 0.05);
        scene.spawn_dynamic_box(
            widget_uid,
            pose,
            half_extents,
            vec3f(1.0, 1.0, 1.0),
            1.0,
            0.5,
            0.0,
        );
        let cube = scene.cubes[0];
        let acquire_pose = Pose::new(Quat::default(), pose.position + vec3f(0.0, 0.0, 0.11));
        let moved_pose = Pose::new(Quat::default(), pose.position + vec3f(0.16, 0.0, 0.11));

        RapierScene::sync_hand_bodies(
            &scene.left_hand,
            &[HandCollider::Ball {
                center: acquire_pose.position,
                radius: 0.09,
            }],
            &mut scene.bodies,
            &mut scene.colliders,
        );
        scene.left_hand_grab = HandGrabState {
            shared_hand: XrSharedHand::LeftHand,
            pose: acquire_pose,
            previous_pose: acquire_pose,
            linvel: vec3f(0.0, 0.0, 0.0),
            tracked: true,
            gripping: true,
            held_body: None,
            grab_offset: Pose::default(),
        };

        scene.step();
        assert_eq!(scene.left_hand_grab.held_body, Some(cube.body));

        scene.left_hand_grab.previous_pose = scene.left_hand_grab.pose;
        scene.left_hand_grab.pose = moved_pose;
        scene.apply_held_body_targets();
        scene.step();

        let body = scene
            .bodies
            .get(cube.body)
            .expect("cube body should exist after moving the held hand");
        let body_pose = makepad_pose(body.position());
        let center_distance = (body_pose.position - moved_pose.position).length();
        assert!(
            (center_distance - half_extents.z).abs() <= 0.012,
            "center_distance={center_distance:?} body_pose={body_pose:?} moved_pose={moved_pose:?}"
        );

        let local_anchor = scene.left_hand_grab.grab_offset.invert().position;
        assert!(
            (local_anchor.z - half_extents.z).abs() <= 0.012,
            "local_anchor={local_anchor:?} half_extents={half_extents:?}"
        );
    }

    #[test]
    fn secondary_hand_can_join_existing_hold_and_keep_body_kinematic() {
        let mut scene = RapierScene::new(0.0);
        let widget_uid = WidgetUid(44);
        let pose = Pose::new(Quat::default(), vec3f(0.0, 1.0, 0.0));
        scene.spawn_dynamic_box(
            widget_uid,
            pose,
            vec3f(0.05, 0.05, 0.05),
            vec3f(1.0, 1.0, 1.0),
            1.0,
            0.5,
            0.0,
        );
        let cube = scene.cubes[0];
        let left_pose = Pose::new(Quat::default(), pose.position);
        let right_pose = Pose::new(Quat::default(), pose.position + vec3f(0.08, 0.0, 0.0));

        RapierScene::sync_hand_bodies(
            &scene.left_hand,
            &[HandCollider::Ball {
                center: pose.position,
                radius: 0.09,
            }],
            &mut scene.bodies,
            &mut scene.colliders,
        );
        scene.left_hand_grab = HandGrabState {
            shared_hand: XrSharedHand::LeftHand,
            pose: left_pose,
            previous_pose: left_pose,
            linvel: vec3f(0.0, 0.0, 0.0),
            tracked: true,
            gripping: true,
            held_body: None,
            grab_offset: Pose::default(),
        };
        scene.step();
        assert_eq!(scene.left_hand_grab.held_body, Some(cube.body));

        RapierScene::sync_hand_bodies(
            &scene.right_hand,
            &[HandCollider::Ball {
                center: pose.position,
                radius: 0.09,
            }],
            &mut scene.bodies,
            &mut scene.colliders,
        );
        scene.right_hand_grab = HandGrabState {
            shared_hand: XrSharedHand::RightHand,
            pose: right_pose,
            previous_pose: right_pose,
            linvel: vec3f(0.0, 0.0, 0.0),
            tracked: true,
            gripping: true,
            held_body: None,
            grab_offset: Pose::default(),
        };
        scene.step();

        assert_eq!(scene.left_hand_grab.held_body, Some(cube.body));
        assert_eq!(scene.right_hand_grab.held_body, Some(cube.body));
        let body = scene
            .bodies
            .get(cube.body)
            .expect("cube body should exist while two hands hold it");
        assert_eq!(body.body_type(), RigidBodyType::KinematicPositionBased);

        scene.left_hand_grab.gripping = false;
        scene.apply_held_body_targets();

        assert_eq!(scene.left_hand_grab.held_body, None);
        assert_eq!(scene.right_hand_grab.held_body, Some(cube.body));
        let body = scene
            .bodies
            .get(cube.body)
            .expect("cube body should still exist after primary hand drops");
        assert_eq!(body.body_type(), RigidBodyType::KinematicPositionBased);
        assert_eq!(
            scene.held_by_for_body(cube.body),
            Some(XrSharedHand::RightHand)
        );
    }

    #[test]
    fn sticky_raw_grab_bit_does_not_keep_pointing_hand_in_grab_state() {
        let mut scene = RapierScene::new(0.0);
        let widget_uid = WidgetUid(45);
        let pose = Pose::new(Quat::default(), vec3f(0.0, 1.0, 0.0));
        scene.spawn_dynamic_box(
            widget_uid,
            pose,
            vec3f(0.05, 0.05, 0.05),
            vec3f(1.0, 1.0, 1.0),
            1.0,
            0.5,
            0.0,
        );
        let cube = scene.cubes[0];
        let hand_pose = Pose::new(Quat::default(), pose.position);

        scene.left_hand_grab = HandGrabState {
            shared_hand: XrSharedHand::LeftHand,
            pose: hand_pose,
            previous_pose: hand_pose,
            linvel: vec3f(0.0, 0.0, 0.0),
            tracked: true,
            gripping: true,
            held_body: Some(cube.body),
            grab_offset: Pose::default(),
        };

        let mut hand = XrHand::default();
        hand.flags = XrHand::IN_VIEW | XrHand::AIM_VALID;
        hand.tips_active = XrHand::GRAB_ACTIVE | (1 << XrHand::INDEX_TIP);
        hand.tips[XrHand::INDEX_TIP] = 0.038;
        hand.joints[XrHand::CENTER] = Pose::new(Quat::default(), pose.position);
        hand.joints[XrHand::WRIST] =
            Pose::new(Quat::default(), pose.position + vec3f(0.0, -0.03, 0.05));
        hand.joints[XrHand::INDEX_BASE] = Pose::new(Quat::default(), pose.position);
        hand.joints[XrHand::INDEX_KNUCKLE1] =
            Pose::new(Quat::default(), pose.position + vec3f(0.0, 0.0, -0.041));
        hand.joints[XrHand::INDEX_KNUCKLE2] =
            Pose::new(Quat::default(), pose.position + vec3f(0.001, 0.002, -0.082));
        hand.joints[XrHand::INDEX_KNUCKLE3] =
            Pose::new(Quat::default(), pose.position + vec3f(0.002, 0.004, -0.122));

        scene.sync_tracked_hands(&hand, &XrHand::default());
        scene.apply_held_body_targets();

        assert_eq!(scene.left_hand_grab.held_body, None);
        let body = scene
            .bodies
            .get(cube.body)
            .expect("cube body should still exist after sticky-grab release");
        assert_eq!(body.body_type(), RigidBodyType::Dynamic);
    }

    #[test]
    fn hand_grab_pose_uses_index_tip_when_pinch_midpoint_is_unavailable() {
        let mut scene = RapierScene::new(0.0);
        let widget_uid = WidgetUid(46);
        let pose = Pose::new(Quat::default(), vec3f(0.0, 1.0, 0.0));
        scene.spawn_dynamic_box(
            widget_uid,
            pose,
            vec3f(0.05, 0.05, 0.05),
            vec3f(1.0, 1.0, 1.0),
            1.0,
            0.5,
            0.0,
        );
        let cube = scene.cubes[0];
        let hand_pose = Pose::new(Quat::default(), pose.position);

        scene.left_hand_grab = HandGrabState {
            shared_hand: XrSharedHand::LeftHand,
            pose: hand_pose,
            previous_pose: hand_pose,
            linvel: vec3f(0.0, 0.0, 0.0),
            tracked: true,
            gripping: true,
            held_body: Some(cube.body),
            grab_offset: Pose::default(),
        };

        let mut hand = XrHand::default();
        hand.flags = XrHand::IN_VIEW | XrHand::AIM_VALID;
        hand.tips_active = XrHand::GRAB_ACTIVE | (1 << XrHand::INDEX_TIP);
        hand.tips[XrHand::INDEX_TIP] = 0.034;
        hand.joints[XrHand::CENTER] = Pose::new(Quat::default(), pose.position);
        hand.joints[XrHand::WRIST] =
            Pose::new(Quat::default(), pose.position + vec3f(0.0, -0.03, 0.05));
        hand.joints[XrHand::INDEX_BASE] =
            Pose::new(Quat::default(), pose.position + vec3f(0.0, 0.0, -0.010));
        hand.joints[XrHand::INDEX_KNUCKLE1] =
            Pose::new(Quat::default(), pose.position + vec3f(0.0, -0.005, -0.030));
        hand.joints[XrHand::INDEX_KNUCKLE2] = Pose::new(
            Quat::default(),
            pose.position + vec3f(0.012, -0.018, -0.040),
        );
        hand.joints[XrHand::INDEX_KNUCKLE3] = Pose::new(
            Quat::default(),
            pose.position + vec3f(0.022, -0.032, -0.020),
        );

        assert!(hand.grab_intent(), "curled hand should still report grab intent");
        assert!(
            hand.tracking_pose().is_some(),
            "palm tracking should still be valid for the sample"
        );
        assert!(
            hand.pinch_anchor_pose().is_none(),
            "sample must not expose a pinch anchor"
        );
        let index_tip = hand
            .tip_pos_checked(XrHand::INDEX_TIP)
            .expect("index tip should still be valid");
        let palm_pose = hand
            .tracking_pose()
            .expect("tracking pose should still be valid");

        scene.sync_tracked_hands(&hand, &XrHand::default());
        scene.apply_held_body_targets();

        assert_eq!(scene.left_hand_grab.held_body, Some(cube.body));
        assert!(scene.left_hand_grab.tracked);
        assert!(scene.left_hand_grab.gripping);
        assert_vec3_close(scene.left_hand_grab.pose.position, index_tip, 0.0001);
        assert!(
            (scene.left_hand_grab.pose.position - palm_pose.position).length() > 0.01,
            "grab pose should stay on the finger anchor, not the palm"
        );
        let body = scene
            .bodies
            .get(cube.body)
            .expect("cube body should still exist after index-tip hold update");
        assert_eq!(body.body_type(), RigidBodyType::KinematicPositionBased);
    }
}
