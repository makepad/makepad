mod shared_object_shadow_tests {
    use super::*;

    fn make_shared_state(
        sent_at: f64,
        pose_z: f32,
        linvel_z: f32,
        angvel_y: f32,
    ) -> XrNetSharedObjectState {
        XrNetSharedObjectState {
            seq: 1,
            sent_at,
            physics_tick: 1,
            object_id: xr_make_shared_object_id(XrNetPeerId(9), XrSharedObjectCounter(7))
                .expect("shared object id should pack"),
            epoch: 0,
            authority: XrNetPeerId(9),
            fidelity: XrSharedObjectFidelity::Interpolated,
            mode: XrSharedObjectMode::Dynamic,
            pose: Pose::new(Quat::default(), vec3f(0.0, 0.0, pose_z)),
            linvel: vec3f(0.0, 0.0, linvel_z),
            angvel: vec3f(0.0, angvel_y, 0.0),
        }
    }

    #[test]
    fn shared_object_shadow_prediction_interpolates_between_remote_history_samples() {
        let previous = make_shared_state(1.00, -1.0, -4.0, 0.0);
        let next = XrNetSharedObjectState {
            seq: 2,
            sent_at: 1.10,
            pose: Pose::new(
                Quat::from_axis_angle(vec3f(0.0, 1.0, 0.0), 0.2),
                vec3f(0.0, 0.0, -2.0),
            ),
            linvel: vec3f(0.0, 0.0, -6.0),
            angvel: vec3f(0.0, 0.5, 0.0),
            ..previous
        };

        let (mode, pose, linvel, angvel) = XrPeerSync::predict_remote_shadow_state_from_history(
            1.05,
            next,
            &[previous, next],
            1.05,
        );

        assert_eq!(mode, XrSharedObjectMode::Dynamic);
        assert!((pose.position.z + 1.5).abs() <= 0.001, "{pose:?}");
        assert!((linvel.z + 5.0).abs() <= 0.001, "{linvel:?}");
        assert!((angvel.y - 0.25).abs() <= 0.001, "{angvel:?}");
    }

    #[test]
    fn shared_object_shadow_prediction_extrapolates_latest_sample_with_clamped_horizon() {
        let latest = make_shared_state(1.00, -1.0, -10.0, 0.0);

        let (_, pose, linvel, _) =
            XrPeerSync::predict_remote_shadow_state_from_history(1.30, latest, &[latest], 1.30);

        let expected_z = -1.0 + -10.0 * XrPeerSync::SHARED_OBJECT_SHADOW_MAX_EXTRAPOLATION_SECONDS;
        assert!((pose.position.z - expected_z).abs() <= 0.001, "{pose:?}");
        assert_eq!(linvel.z, -10.0);
    }

    #[test]
    fn shared_object_shadow_prediction_uses_fallback_local_time_when_sample_time_is_zero() {
        let latest = XrNetSharedObjectState {
            sent_at: 0.0,
            pose: Pose::new(Quat::default(), vec3f(0.0, 0.0, -1.0)),
            linvel: vec3f(0.0, 0.0, -2.0),
            ..make_shared_state(0.0, -1.0, -2.0, 0.0)
        };

        let (_, pose, linvel, _) =
            XrPeerSync::predict_remote_shadow_state_from_history(2.00, latest, &[latest], 1.95);

        assert!((pose.position.z + 1.10).abs() <= 0.001, "{pose:?}");
        assert_eq!(linvel.z, -2.0);
    }

    #[test]
    fn incoming_shared_object_state_time_is_clamped_to_local_receive_time() {
        let normalized = XrPeerSync::clamp_remote_shared_object_local_time(5.40, 5.10);

        assert!((normalized - 5.10).abs() <= f64::EPSILON, "{normalized:?}");
    }

    #[test]
    fn incoming_shared_object_state_time_preserves_past_local_sample_time() {
        let normalized = XrPeerSync::clamp_remote_shared_object_local_time(5.00, 5.10);

        assert!((normalized - 5.00).abs() <= f64::EPSILON, "{normalized:?}");
    }

    #[test]
    fn shared_object_shadow_does_not_reapply_for_seq_only_advance_on_predicted_path() {
        let previous = XrAppliedRemoteShadowState {
            peer_id: XrNetPeerId(42),
            applied_at_local_time: 10.0,
            state_seq: Some(7),
            mode: XrSharedObjectMode::Dynamic,
            pose: Pose::new(Quat::default(), vec3f(0.0, 0.0, -1.0)),
            linvel: vec3f(0.0, 0.0, -4.0),
            angvel: vec3f(0.0, 0.0, 0.0),
        };

        assert!(
            !XrPeerSync::should_reapply_remote_shadow_state(
                &previous,
                10.04,
                XrNetPeerId(42),
                Some(8),
                XrSharedObjectMode::Dynamic,
                Pose::new(Quat::default(), vec3f(0.0, 0.0, -1.16)),
                vec3f(0.0, 0.0, -4.0),
                vec3f(0.0, 0.0, 0.0),
            ),
            "a newer authoritative shared-object seq should not force a correction when the local shadow remains on the predicted path"
        );
    }
}
