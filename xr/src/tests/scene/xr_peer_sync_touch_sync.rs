mod touch_sync_tests {
    use super::*;

    fn make_peer(peer_id: u64) -> XrNetPeer {
        XrNetPeer {
            id: XrNetPeerId(peer_id),
            addr: "127.0.0.1:41547".parse().unwrap(),
        }
    }

    fn make_state_frame(state: XrState) -> XrNetStateFrame {
        XrNetStateFrame {
            seq: 1,
            sent_at: state.time,
            state,
        }
    }

    fn make_sync_anchor(
        id: u32,
        captured_at: f64,
        extrema: XrSyncAnchorExtrema,
        anchor: XrAnchor,
    ) -> XrSyncAnchor {
        XrSyncAnchor {
            id,
            captured_at,
            extrema,
            anchor,
        }
    }

    fn make_local_sync_state(
        now: f64,
        anchor: Option<XrAnchor>,
        sync_anchor: Option<XrSyncAnchor>,
        fist_hold_anchor: Option<XrAnchor>,
    ) -> XrPeerSyncLocalState {
        let mut local = XrPeerSyncLocalState {
            state_time: now,
            anchor,
            sync_anchor,
            fist_hold_anchor,
            ..XrPeerSyncLocalState::default()
        };
        if let Some(sync_anchor) = sync_anchor {
            local.record_sync_anchor(sync_anchor);
        }
        local
    }

    fn pose(position: Vec3f) -> Pose {
        Pose::new(Quat::default(), position)
    }

    fn make_tracking_hand(position: Vec3f) -> XrHand {
        let mut hand = XrHand::default();
        hand.flags = XrHand::IN_VIEW | XrHand::AIM_VALID;
        hand.joints[XrHand::CENTER] = pose(position);
        hand.joints[XrHand::WRIST] = pose(position + vec3f(0.0, -0.040, 0.030));
        hand
    }

    fn make_remote_touch_anchor(local_anchor: XrAnchor, remote_to_local: Mat4f) -> XrAnchor {
        let local_to_remote = remote_to_local.invert();
        XrAnchor {
            left: XrPeerSync::transform_point(&local_to_remote, local_anchor.right),
            right: XrPeerSync::transform_point(&local_to_remote, local_anchor.left),
        }
    }

    fn assert_touch_anchor_mapping_matches(
        solved: Mat4f,
        remote_anchor: XrAnchor,
        local_anchor: XrAnchor,
    ) {
        let mirrored_remote = remote_anchor.mirrored();
        let solved_left = XrPeerSync::transform_point(&solved, mirrored_remote.left);
        let solved_right = XrPeerSync::transform_point(&solved, mirrored_remote.right);
        assert!(
            (solved_left - local_anchor.left).length() <= 0.001,
            "{solved_left:?}"
        );
        assert!(
            (solved_right - local_anchor.right).length() <= 0.001,
            "{solved_right:?}"
        );
    }

    fn assert_anchor_close(left: XrAnchor, right: XrAnchor) {
        assert!(
            (left.left - right.left).length() <= 1.0e-5,
            "{left:?} != {right:?}"
        );
        assert!(
            (left.right - right.right).length() <= 1.0e-5,
            "{left:?} != {right:?}"
        );
    }

    fn far_relocated_anchor_override(saved_anchor: XrAnchor, motion_center: Vec3f) -> XrAnchor {
        let left_distance = (motion_center - saved_anchor.left).length();
        let right_distance = (motion_center - saved_anchor.right).length();
        if left_distance <= right_distance {
            XrAnchor {
                left: motion_center,
                right: saved_anchor.right,
            }
        } else {
            XrAnchor {
                left: saved_anchor.left,
                right: motion_center,
            }
        }
    }

    #[test]
    fn local_sync_anchor_activity_uses_seen_time_not_capture_time() {
        let sync_anchor = make_sync_anchor(
            7,
            10.0,
            XrSyncAnchorExtrema::High,
            XrAnchor {
                left: vec3f(-0.12, 1.1, -0.4),
                right: vec3f(0.14, 1.1, -0.4),
            },
        );
        let mut local = make_local_sync_state(11.2, None, Some(sync_anchor), None);

        assert_eq!(local.active_sync_anchor(), Some(sync_anchor));

        local.state_time = 12.7;
        assert_eq!(local.active_sync_anchor(), None);
    }

    #[test]
    fn far_relocated_sync_samples_replace_closest_saved_marker() {
        let saved_local_anchor = XrAnchor {
            left: vec3f(-0.25, 1.05, -0.30),
            right: vec3f(0.22, 1.05, -0.30),
        };
        let first_far_anchor = XrAnchor {
            left: vec3f(-0.18, 1.18, -1.64),
            right: vec3f(0.16, 1.17, -1.60),
        };
        let second_far_anchor = XrAnchor {
            left: vec3f(-0.20, 1.20, -1.52),
            right: vec3f(0.18, 1.19, -1.48),
        };
        let mut local = make_local_sync_state(10.0, Some(saved_local_anchor), None, None);

        let first = local.record_matched_sync_anchor(first_far_anchor, 10.0);
        let first_center = (first_far_anchor.left + first_far_anchor.right) * 0.5;
        assert_anchor_close(
            first,
            far_relocated_anchor_override(saved_local_anchor, first_center),
        );

        let second = local.record_matched_sync_anchor(second_far_anchor, 10.1);
        let second_center = (first_far_anchor.left
            + first_far_anchor.right
            + second_far_anchor.left
            + second_far_anchor.right)
            * 0.25;
        assert_anchor_close(
            second,
            far_relocated_anchor_override(saved_local_anchor, second_center),
        );
        assert_eq!(local.matched_sync_sample_count(), 2);
    }

    #[test]
    fn far_relocated_sync_samples_can_replace_left_marker_when_it_is_closest() {
        let saved_local_anchor = XrAnchor {
            left: vec3f(-0.42, 1.02, -0.32),
            right: vec3f(0.34, 1.01, -0.28),
        };
        let far_anchor = XrAnchor {
            left: vec3f(-0.64, 1.19, -1.68),
            right: vec3f(-0.26, 1.18, -1.60),
        };
        let mut local = make_local_sync_state(10.0, Some(saved_local_anchor), None, None);

        let matched = local.record_matched_sync_anchor(far_anchor, 10.0);
        let far_center = (far_anchor.left + far_anchor.right) * 0.5;

        assert_anchor_close(
            matched,
            XrAnchor {
                left: far_center,
                right: saved_local_anchor.right,
            },
        );
        assert_eq!(local.matched_sync_sample_count(), 1);
    }

    #[test]
    fn mirrored_remote_touch_anchor_maps_back_to_local_touch_order() {
        let local_anchor = XrAnchor {
            left: vec3f(-0.18, 1.12, -0.44),
            right: vec3f(0.16, 1.11, -0.40),
        };
        let remote_to_local = Pose::new(
            Quat::from_axis_angle(vec3f(0.0, 1.0, 0.0), 0.63),
            vec3f(-0.31, 0.04, 0.27),
        )
        .to_mat4();
        let local_to_remote = remote_to_local.invert();
        let remote_anchor = XrAnchor {
            left: XrPeerSync::transform_point(&local_to_remote, local_anchor.right),
            right: XrPeerSync::transform_point(&local_to_remote, local_anchor.left),
        };
        let mirrored_remote = remote_anchor.mirrored();
        let solved = mirrored_remote.mapping_to(&local_anchor);

        let solved_left = XrPeerSync::transform_point(&solved, mirrored_remote.left);
        let solved_right = XrPeerSync::transform_point(&solved, mirrored_remote.right);

        assert!(
            (solved_left - local_anchor.left).length() <= 0.001,
            "{solved_left:?}"
        );
        assert!(
            (solved_right - local_anchor.right).length() <= 0.001,
            "{solved_right:?}"
        );
        assert!(
            (XrPeerSync::transform_point(&solved, remote_anchor.left) - local_anchor.right)
                .length()
                <= 0.001
        );
    }

    #[test]
    fn arm_corridor_allows_opposing_vertical_wiggle_within_angle_budget() {
        let head_pose = Pose::new(Quat::default(), vec3f(0.0, 1.60, 0.0));
        let left_hand = make_tracking_hand(vec3f(-0.18, 1.40, -0.40));
        let right_hand = make_tracking_hand(vec3f(0.18, 1.75, -0.40));

        assert!(XrPeerSync::arm_corridor_ready(
            head_pose,
            &left_hand,
            &right_hand
        ));
    }

    #[test]
    fn descriptor_alignment_uses_persistent_anchor_height_override() {
        let mut registry = XrPeerRegistry::default();
        let peer = make_peer(0x2a);
        let local_anchor = XrAnchor {
            left: vec3f(-0.18, 1.31, -0.41),
            right: vec3f(0.16, 1.29, -0.39),
        };
        let anchor_remote_to_local = Pose::new(
            Quat::from_axis_angle(vec3f(0.0, 1.0, 0.0), 0.21),
            vec3f(-0.14, 0.28, 0.09),
        )
        .to_mat4();
        let remote_anchor = make_remote_touch_anchor(local_anchor, anchor_remote_to_local);
        let descriptor_remote_to_local = Pose::new(
            Quat::from_axis_angle(vec3f(0.0, 1.0, 0.0), -0.63),
            vec3f(0.48, -0.41, -0.26),
        )
        .to_mat4();

        registry.track_state(
            peer,
            make_state_frame(XrState {
                time: 1.0,
                anchor: Some(remote_anchor),
                ..XrState::default()
            }),
            10.0,
        );
        registry
            .peers
            .get_mut(&peer.id)
            .expect("peer state should exist")
            .descriptor_remote_to_local = Some(descriptor_remote_to_local);

        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut local = make_local_sync_state(10.1, Some(local_anchor), None, None);
        let mut recent_anchor_confirmation = None;
        let changed =
            registry.refresh_transforms(&mut cx, &mut local, &mut recent_anchor_confirmation, 10.1);

        assert!(changed);
        let peer_state = registry
            .peers
            .get(&peer.id)
            .expect("peer state should exist");
        let solved_anchor = peer_state
            .anchor_remote_to_local
            .expect("persistent anchors should resolve");
        assert_touch_anchor_mapping_matches(solved_anchor, remote_anchor, local_anchor);
        assert_eq!(local.anchor_override, None);
        let mut expected = descriptor_remote_to_local;
        expected.v[13] = solved_anchor.v[13];
        assert_eq!(peer_state.remote_to_local, Some(expected));
        assert_eq!(
            peer_state.transform_source,
            RemoteTransformSource::Descriptor
        );
    }

    #[test]
    fn descriptor_alignment_stays_unchanged_without_persistent_anchors() {
        let mut registry = XrPeerRegistry::default();
        let peer = make_peer(0x2a);
        let descriptor_remote_to_local = Pose::new(
            Quat::from_axis_angle(vec3f(0.0, 1.0, 0.0), 0.47),
            vec3f(-0.36, 0.18, 0.22),
        )
        .to_mat4();

        registry.track_state(
            peer,
            make_state_frame(XrState {
                time: 1.0,
                ..XrState::default()
            }),
            10.0,
        );
        registry
            .peers
            .get_mut(&peer.id)
            .expect("peer state should exist")
            .descriptor_remote_to_local = Some(descriptor_remote_to_local);

        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut local = make_local_sync_state(10.1, None, None, None);
        let mut recent_anchor_confirmation = None;
        let changed =
            registry.refresh_transforms(&mut cx, &mut local, &mut recent_anchor_confirmation, 10.1);

        assert!(changed);
        let peer_state = registry
            .peers
            .get(&peer.id)
            .expect("peer state should exist");
        assert_eq!(peer_state.anchor_remote_to_local, None);
        assert_eq!(peer_state.remote_to_local, Some(descriptor_remote_to_local));
        assert_eq!(
            peer_state.transform_source,
            RemoteTransformSource::Descriptor
        );
    }

    #[test]
    fn matched_box_sync_sample_updates_local_anchor_average_immediately() {
        let mut registry = XrPeerRegistry::default();
        let peer = make_peer(0x2a);
        let saved_local_anchor = XrAnchor {
            left: vec3f(-0.25, 1.05, -0.30),
            right: vec3f(0.22, 1.05, -0.30),
        };
        let fresh_local_anchor = XrAnchor {
            left: vec3f(-0.17, 1.16, -0.56),
            right: vec3f(0.15, 1.15, -0.52),
        };
        let saved_remote_to_local = Pose::new(
            Quat::from_axis_angle(vec3f(0.0, 1.0, 0.0), -0.31),
            vec3f(0.42, 0.02, -0.18),
        )
        .to_mat4();
        let fresh_remote_to_local = Pose::new(
            Quat::from_axis_angle(vec3f(0.0, 1.0, 0.0), 0.58),
            vec3f(-0.24, -0.01, 0.33),
        )
        .to_mat4();
        let saved_remote_anchor =
            make_remote_touch_anchor(saved_local_anchor, saved_remote_to_local);
        let fresh_remote_anchor =
            make_remote_touch_anchor(fresh_local_anchor, fresh_remote_to_local);

        registry.track_state(
            peer,
            make_state_frame(XrState {
                time: 1.0,
                anchor: Some(saved_remote_anchor),
                sync_anchor: Some(make_sync_anchor(
                    41,
                    1.0,
                    XrSyncAnchorExtrema::Low,
                    fresh_remote_anchor,
                )),
                ..XrState::default()
            }),
            10.0,
        );
        {
            let peer_state = registry
                .peers
                .get_mut(&peer.id)
                .expect("peer state should exist");
            peer_state.last_fist_hold_anchor = Some(fresh_remote_anchor);
            peer_state.last_fist_hold_seen_at = Some(10.0);
        }

        let mut cx = Cx::new(Box::new(|_, _| {}));
        let local_sync = make_sync_anchor(7, 10.02, XrSyncAnchorExtrema::High, fresh_local_anchor);
        let mut local = make_local_sync_state(
            10.1,
            Some(saved_local_anchor),
            Some(local_sync),
            Some(fresh_local_anchor),
        );
        let mut recent_anchor_confirmation = None;
        let changed =
            registry.refresh_transforms(&mut cx, &mut local, &mut recent_anchor_confirmation, 10.1);

        assert!(changed);
        let peer_state = registry
            .peers
            .get(&peer.id)
            .expect("peer state should exist");
        let solved = peer_state
            .anchor_remote_to_local
            .expect("live override should update the active anchor mapping immediately");
        assert_eq!(
            solved,
            saved_remote_anchor
                .mirrored()
                .mapping_to(&fresh_local_anchor)
        );
        assert_anchor_close(
            local
                .anchor_override
                .expect("anchor override should be present"),
            fresh_local_anchor,
        );
        assert_eq!(local.matched_sync_sample_count(), 1);
        assert_anchor_close(
            recent_anchor_confirmation
                .map(|confirmation| confirmation.anchor)
                .expect("confirmation anchor should be present"),
            fresh_local_anchor,
        );
    }

    #[test]
    fn matched_box_sync_far_from_saved_anchor_replaces_closest_saved_marker() {
        let mut registry = XrPeerRegistry::default();
        let peer = make_peer(0x2a);
        let saved_local_anchor = XrAnchor {
            left: vec3f(-0.42, 1.05, -0.30),
            right: vec3f(0.34, 1.05, -0.30),
        };
        let far_local_anchor = XrAnchor {
            left: vec3f(-0.64, 1.18, -1.64),
            right: vec3f(-0.28, 1.17, -1.60),
        };
        let expected_override = far_relocated_anchor_override(
            saved_local_anchor,
            (far_local_anchor.left + far_local_anchor.right) * 0.5,
        );
        let saved_remote_to_local = Pose::new(
            Quat::from_axis_angle(vec3f(0.0, 1.0, 0.0), -0.31),
            vec3f(0.42, 0.02, -0.18),
        )
        .to_mat4();
        let far_remote_to_local = Pose::new(
            Quat::from_axis_angle(vec3f(0.0, 1.0, 0.0), 0.58),
            vec3f(-0.24, -0.01, 0.33),
        )
        .to_mat4();
        let saved_remote_anchor =
            make_remote_touch_anchor(saved_local_anchor, saved_remote_to_local);
        let far_remote_anchor = make_remote_touch_anchor(far_local_anchor, far_remote_to_local);

        registry.track_state(
            peer,
            make_state_frame(XrState {
                time: 1.0,
                anchor: Some(saved_remote_anchor),
                sync_anchor: Some(make_sync_anchor(
                    41,
                    1.0,
                    XrSyncAnchorExtrema::Low,
                    far_remote_anchor,
                )),
                ..XrState::default()
            }),
            10.0,
        );
        {
            let peer_state = registry
                .peers
                .get_mut(&peer.id)
                .expect("peer state should exist");
            peer_state.last_fist_hold_anchor = Some(far_remote_anchor);
            peer_state.last_fist_hold_seen_at = Some(10.0);
        }

        let mut cx = Cx::new(Box::new(|_, _| {}));
        let local_sync = make_sync_anchor(7, 10.02, XrSyncAnchorExtrema::High, far_local_anchor);
        let mut local = make_local_sync_state(
            10.1,
            Some(saved_local_anchor),
            Some(local_sync),
            Some(far_local_anchor),
        );
        let mut recent_anchor_confirmation = None;
        let changed =
            registry.refresh_transforms(&mut cx, &mut local, &mut recent_anchor_confirmation, 10.1);

        assert!(changed);
        let peer_state = registry
            .peers
            .get(&peer.id)
            .expect("peer state should exist");
        let solved = peer_state
            .anchor_remote_to_local
            .expect("live override should update the active anchor mapping immediately");
        assert_eq!(
            solved,
            saved_remote_anchor
                .mirrored()
                .mapping_to(&expected_override)
        );
        assert_anchor_close(
            local
                .anchor_override
                .expect("anchor override should be present"),
            expected_override,
        );
        assert_eq!(local.matched_sync_sample_count(), 1);
        assert_anchor_close(
            recent_anchor_confirmation
                .map(|confirmation| confirmation.anchor)
                .expect("confirmation anchor should be present"),
            expected_override,
        );
    }

    #[test]
    fn box_sync_requires_both_sides_to_remain_in_box_pose() {
        let mut registry = XrPeerRegistry::default();
        let peer = make_peer(0x2a);
        let local_sync_anchor = XrAnchor {
            left: vec3f(-0.16, 1.14, -0.58),
            right: vec3f(0.14, 1.13, -0.55),
        };
        let remote_sync_anchor = XrAnchor {
            left: vec3f(0.12, 1.14, -0.54),
            right: vec3f(-0.11, 1.13, -0.53),
        };

        registry.track_state(
            peer,
            make_state_frame(XrState {
                time: 1.0,
                sync_anchor: Some(make_sync_anchor(
                    99,
                    1.0,
                    XrSyncAnchorExtrema::Low,
                    remote_sync_anchor,
                )),
                ..XrState::default()
            }),
            10.0,
        );
        {
            let peer_state = registry
                .peers
                .get_mut(&peer.id)
                .expect("peer state should exist");
            peer_state.last_fist_hold_anchor = Some(remote_sync_anchor);
            peer_state.last_fist_hold_seen_at = Some(10.0);
        }

        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut local = make_local_sync_state(
            10.1,
            None,
            Some(make_sync_anchor(
                7,
                10.02,
                XrSyncAnchorExtrema::Low,
                local_sync_anchor,
            )),
            None,
        );
        let mut recent_anchor_confirmation = None;
        let changed =
            registry.refresh_transforms(&mut cx, &mut local, &mut recent_anchor_confirmation, 10.1);

        assert!(!changed);
        assert_eq!(local.anchor_override, None);
        assert_eq!(recent_anchor_confirmation, None);
        assert_eq!(local.matched_sync_sample_count(), 0);
    }

    #[test]
    fn box_sync_without_backend_anchor_updates_local_override_but_not_transform() {
        let mut registry = XrPeerRegistry::default();
        let peer = make_peer(0x2a);
        let fresh_local_anchor = XrAnchor {
            left: vec3f(-0.17, 1.16, -0.56),
            right: vec3f(0.15, 1.15, -0.52),
        };
        let fresh_remote_anchor = XrAnchor {
            left: vec3f(0.10, 1.12, -0.48),
            right: vec3f(-0.12, 1.12, -0.50),
        };

        registry.track_state(
            peer,
            make_state_frame(XrState {
                time: 1.0,
                sync_anchor: Some(make_sync_anchor(
                    41,
                    1.0,
                    XrSyncAnchorExtrema::High,
                    fresh_remote_anchor,
                )),
                ..XrState::default()
            }),
            10.0,
        );
        {
            let peer_state = registry
                .peers
                .get_mut(&peer.id)
                .expect("peer state should exist");
            peer_state.last_fist_hold_anchor = Some(fresh_remote_anchor);
            peer_state.last_fist_hold_seen_at = Some(10.0);
        }

        let mut cx = Cx::new(Box::new(|_, _| {}));
        let mut local = make_local_sync_state(
            10.1,
            None,
            Some(make_sync_anchor(
                7,
                10.03,
                XrSyncAnchorExtrema::Low,
                fresh_local_anchor,
            )),
            Some(fresh_local_anchor),
        );
        let mut recent_anchor_confirmation = None;
        let changed =
            registry.refresh_transforms(&mut cx, &mut local, &mut recent_anchor_confirmation, 10.1);

        assert!(changed);
        let peer_state = registry
            .peers
            .get(&peer.id)
            .expect("peer state should exist");
        assert_eq!(peer_state.anchor_remote_to_local, None);
        assert_eq!(peer_state.remote_to_local, None);
        assert_anchor_close(
            local
                .anchor_override
                .expect("anchor override should be present"),
            fresh_local_anchor,
        );
        assert_anchor_close(
            recent_anchor_confirmation
                .map(|confirmation| confirmation.anchor)
                .expect("confirmation anchor should be present"),
            fresh_local_anchor,
        );
    }

    #[test]
    fn recent_remote_sync_anchor_requires_recent_receive_time() {
        let sync_anchor = make_sync_anchor(
            42,
            1.25,
            XrSyncAnchorExtrema::High,
            XrAnchor {
                left: vec3f(-0.1, 1.0, -0.3),
                right: vec3f(0.1, 1.0, -0.3),
            },
        );
        let mut peer = RemotePeerState::new(make_peer(0x2a));
        peer.last_state_received_at = 10.0;
        peer.recent_sync_anchors.push_back(TimedRemoteSyncAnchor {
            sync: sync_anchor,
            first_seen_at_local_time: 10.0,
            last_seen_at_local_time: 10.0,
        });

        assert_eq!(
            XrPeerSync::recent_remote_sync_anchor(&peer, 10.2),
            Some(sync_anchor)
        );
        assert_eq!(XrPeerSync::recent_remote_sync_anchor(&peer, 10.6), None);
    }
}
