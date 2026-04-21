mod alignment_tests {
    use super::*;

    fn make_solution(
        yaw_radians: f32,
        translation: Vec3f,
        confidence: f32,
        residual_meters: f32,
        matched_samples: usize,
    ) -> XrDepthAlignSolution {
        XrDepthAlignSolution {
            yaw_radians,
            translation,
            confidence,
            symmetry_confidence: 1.0,
            residual_meters,
            matched_samples,
        }
    }

    fn make_diagnostic(solution: XrDepthAlignSolution) -> XrDepthAlignSolveDiagnostic {
        XrDepthAlignSolveDiagnostic {
            local_wall_samples: 8,
            remote_wall_samples: 8,
            best_solution: Some(solution),
            ..XrDepthAlignSolveDiagnostic::default()
        }
    }

    fn make_peer(peer_id: u64) -> XrNetPeer {
        XrNetPeer {
            id: XrNetPeerId(peer_id),
            addr: "127.0.0.1:41547".parse().unwrap(),
        }
    }

    fn make_peer_descriptor(
        wall_count: usize,
        has_vertical_descriptor: bool,
    ) -> XrNetAlignmentDescriptorFrame {
        let samples = (0..wall_count)
            .map(|index| XrDepthAlignSample {
                kind: XrDepthAlignSampleKind::Wall,
                point: vec3(index as f32 * 0.2, 0.0, 0.0),
                normal: vec3(1.0, 0.0, 0.0),
                weight: 1.0,
            })
            .collect();
        XrNetAlignmentDescriptorFrame {
            seq: 7,
            sent_at: 1.0,
            descriptor: XrDepthAlignDescriptor {
                samples,
                vertical_descriptor: has_vertical_descriptor.then_some(
                    XrDepthAlignVerticalDescriptor {
                        origin_x: 0.0,
                        origin_z: 0.0,
                        cell_size_meters: 0.25,
                        size: 1,
                        vertical_surface_masks: vec![1],
                        clutter_surface_masks: vec![0],
                        free_space_masks: vec![0],
                        height_u8: vec![128],
                    },
                ),
                ..XrDepthAlignDescriptor::default()
            },
        }
    }

    fn reference_dump_pair() -> XrNetAlignmentDescriptorDumpPair {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("dump/dumps/align-pair-226a39e4b300-r0097-1774792873191.bin");
        let bytes = std::fs::read(path).expect("reference dump should exist");
        XrNetAlignmentDescriptorDumpPair::from_file_bytes(&bytes)
            .expect("reference dump should decode")
    }

    #[test]
    fn stable_alignment_prefers_existing_solution_over_flip() {
        let previous = make_solution(0.42, vec3(0.28, 0.0, -0.64), 0.41, 0.03, 8);
        let flipped = make_solution(-2.71, vec3(-0.34, 0.0, 0.71), 0.44, 0.03, 8);

        let chosen = choose_stable_alignment_solution(
            Some(previous),
            Some(previous),
            &make_diagnostic(flipped),
        )
        .unwrap();

        assert_eq!(chosen, previous);
    }

    #[test]
    fn stable_alignment_accepts_small_refinement() {
        let previous = make_solution(0.42, vec3(0.28, 0.0, -0.64), 0.28, 0.06, 6);
        let refined = make_solution(0.46, vec3(0.24, 0.0, -0.60), 0.35, 0.03, 8);

        let chosen = choose_stable_alignment_solution(
            Some(previous),
            Some(previous),
            &make_diagnostic(refined),
        )
        .unwrap();

        assert_eq!(chosen, refined);
    }

    #[test]
    fn stable_alignment_holds_stronger_solution_over_weaker_reacquisition() {
        let previous = XrDepthAlignSolution {
            yaw_radians: 0.38,
            translation: vec3(0.18, 0.0, -0.54),
            confidence: 0.74,
            symmetry_confidence: 0.82,
            residual_meters: 0.03,
            matched_samples: 8,
        };
        let weaker = XrDepthAlignSolution {
            yaw_radians: 0.52,
            translation: vec3(0.34, 0.0, -0.30),
            confidence: 0.59,
            symmetry_confidence: 0.21,
            residual_meters: 0.08,
            matched_samples: 2,
        };
        let mut weaker_diag = make_diagnostic(weaker);
        weaker_diag.local_wall_samples = 2;
        weaker_diag.remote_wall_samples = 2;

        let chosen =
            choose_stable_alignment_solution(Some(previous), Some(previous), &weaker_diag).unwrap();

        assert_eq!(chosen, previous);
    }

    #[test]
    fn stable_alignment_switches_when_previous_pose_no_longer_scores_on_current_descriptor() {
        let previous = make_solution(-0.41, vec3(0.58, 0.0, -0.44), 0.42, 0.03, 8);
        let candidate = make_solution(1.18, vec3(-0.62, 0.0, 0.71), 0.39, 0.03, 8);
        let stale_on_current = XrDepthAlignSolution {
            yaw_radians: previous.yaw_radians,
            translation: previous.translation,
            confidence: 0.06,
            symmetry_confidence: 0.01,
            residual_meters: 0.21,
            matched_samples: 1,
        };

        let chosen = choose_stable_alignment_solution(
            Some(previous),
            Some(stale_on_current),
            &make_diagnostic(candidate),
        )
        .unwrap();

        assert_eq!(chosen, candidate);
    }

    #[test]
    fn stable_alignment_clears_previous_when_current_descriptor_no_longer_supports_it() {
        let previous = make_solution(-0.41, vec3(0.58, 0.0, -0.44), 0.42, 0.03, 8);
        let stale_on_current = XrDepthAlignSolution {
            yaw_radians: previous.yaw_radians,
            translation: previous.translation,
            confidence: 0.05,
            symmetry_confidence: 0.0,
            residual_meters: 0.24,
            matched_samples: 0,
        };
        let rejected = XrDepthAlignSolveDiagnostic {
            local_wall_samples: 4,
            remote_wall_samples: 4,
            local_vertical_descriptor: true,
            remote_vertical_descriptor: true,
            best_solution: Some(XrDepthAlignSolution {
                yaw_radians: 1.18,
                translation: vec3(-0.62, 0.0, 0.71),
                confidence: 0.10,
                symmetry_confidence: 0.03,
                residual_meters: 0.18,
                matched_samples: 1,
            }),
            ..XrDepthAlignSolveDiagnostic::default()
        };

        let chosen =
            choose_stable_alignment_solution(Some(previous), Some(stale_on_current), &rejected);

        assert_eq!(chosen, None);
    }

    #[test]
    fn worker_queues_new_local_descriptor_without_interrupting_active_solver() {
        let pair = reference_dump_pair();
        let peer = XrNetPeer {
            id: pair.remote_peer_id,
            addr: "127.0.0.1:41547".parse().unwrap(),
        };
        let mut state = AlignmentWorkerState::default();
        let mut updated_local = pair.local_descriptor.clone();
        let updated_height = updated_local
            .descriptor
            .height_map
            .as_mut()
            .and_then(|height_map| {
                height_map
                    .heights_meters
                    .iter_mut()
                    .find(|height| height.is_finite())
            })
            .expect("reference dump should contain a finite local height sample");
        *updated_height += 0.06;

        state.apply_local_descriptor_update(PendingLocalDescriptorUpdate::Set {
            frame: pair.local_descriptor.clone(),
            version: (1, 0),
        });
        assert!(state.apply_peer_update(
            peer.id,
            PendingPeerDescriptorUpdate::Set {
                peer,
                frame: pair.remote_descriptor.clone(),
            },
        ));

        let peer_state = state.peers.get(&peer.id).unwrap();
        assert!(peer_state.matcher.is_some());
        assert_eq!(peer_state.active_local_descriptor_version, Some((1, 0)));
        assert_eq!(
            peer_state.active_remote_descriptor_seq,
            Some(pair.remote_descriptor.seq)
        );
        assert!(!peer_state.queued_rerun);

        assert!(
            state.apply_local_descriptor_update(PendingLocalDescriptorUpdate::Set {
                frame: updated_local,
                version: (2, 0),
            })
        );

        let peer_state = state.peers.get(&peer.id).unwrap();
        assert!(peer_state.matcher.is_some());
        assert_eq!(peer_state.active_local_descriptor_version, Some((1, 0)));
        assert_eq!(
            peer_state.active_remote_descriptor_seq,
            Some(pair.remote_descriptor.seq)
        );
        assert!(peer_state.queued_rerun);

        let mut guard = 0usize;
        while state.has_pending_work() && guard < 16 {
            let outcome =
                state.advance_pending_alignments(Duration::ZERO, XR_ALIGNMENT_CALLBACK_MAX_STEPS);
            assert!(outcome.did_work);
            guard += 1;
        }

        let peer_state = state.peers.get(&peer.id).unwrap();
        assert_eq!(
            peer_state.last_solved_local_descriptor_version,
            Some((2, 0))
        );
        assert_eq!(
            peer_state.last_solved_remote_descriptor_seq,
            Some(pair.remote_descriptor.seq)
        );
        assert!(peer_state.matcher.is_none());
        assert!(!peer_state.queued_rerun);
    }

    #[test]
    fn pending_alignment_debug_reports_local_descriptor_before_peer_arrives() {
        let text = make_pending_alignment_debug_text(
            "AlignDbg: local slice 2 | desc occ 0 v 0 c 0",
            &HashMap::new(),
        );
        assert_eq!(
            text,
            "AlignDbg: local slice 2 | desc occ 0 v 0 c 0 | waiting for peer heightmap"
        );
    }

    #[test]
    fn pending_alignment_debug_reports_solve_pending_once_peer_descriptor_arrives() {
        let mut peers = HashMap::new();
        let mut peer = RemotePeerState::new(make_peer(0x2a));
        peer.latest_descriptor = Some(make_peer_descriptor(2, true));
        peer.has_descriptor = true;
        peers.insert(peer.peer.id, peer);

        let text = make_pending_alignment_debug_text(
            "AlignDbg: local slice 2 | desc occ 0 v 0 c 0",
            &peers,
        );
        assert_eq!(
            text,
            "AlignDbg: local slice 2 | desc occ 0 v 0 c 0 | 0000002a: remote map seq 7 missing | solve pending"
        );
    }

    #[test]
    fn peer_scene_debug_uses_descriptor_payload_before_solver_runs() {
        let mut peers = HashMap::new();
        let mut peer = RemotePeerState::new(make_peer(0x2a));
        peer.latest_descriptor = Some(make_peer_descriptor(2, true));
        peer.has_descriptor = true;
        peers.insert(peer.peer.id, peer);

        let text = make_peer_scene_debug_text(true, &peers);
        assert_eq!(
            text,
            "PeerMap 0000002a: state no | map yes seq 7 missing | pose raw | solve pending"
        );
    }

    #[test]
    fn peer_scene_debug_prefers_peer_with_descriptor_over_stale_waiting_peer() {
        let mut peers = HashMap::new();
        peers.insert(make_peer(0x01).id, RemotePeerState::new(make_peer(0x01)));

        let mut peer = RemotePeerState::new(make_peer(0x2a));
        peer.latest_descriptor = Some(make_peer_descriptor(2, true));
        peer.has_descriptor = true;
        peers.insert(peer.peer.id, peer);

        let text = make_peer_scene_debug_text(true, &peers);
        assert_eq!(
            text,
            "PeerMap 0000002a: state no | map yes seq 7 missing | pose raw | solve pending"
        );
    }

    #[test]
    fn alignment_state_reports_local_remote_worker_versions() {
        let mut peers = HashMap::new();
        let mut peer = RemotePeerState::new(make_peer(0x2a));
        peer.latest_descriptor = Some(make_peer_descriptor(2, true));
        peer.last_solve_ms = 1.7;
        peer.last_solved_local_descriptor_version = Some((4, 9));
        peer.last_solved_remote_descriptor_seq = Some(7);
        peer.last_solve_diagnostic = Some(XrDepthAlignSolveDiagnostic {
            local_wall_samples: 8,
            remote_wall_samples: 8,
            local_vertical_descriptor: true,
            remote_vertical_descriptor: true,
            best_solution: Some(make_solution(0.15, vec3(0.2, 0.0, -0.1), 0.42, 0.03, 8)),
            ..XrDepthAlignSolveDiagnostic::default()
        });
        peers.insert(peer.peer.id, peer);

        let text = make_alignment_state_text(LocalSceneState::Ready, Some((4, 9)), &peers);
        assert_eq!(
            text,
            "AlignState 0000002a: local map yes v4/9 | remote map yes seq 7 | worker lv4/9 rv7 accepted match 1.7ms | pose raw"
        );
    }

    #[test]
    fn pending_alignment_debug_keeps_worker_diagnostic_text_when_available() {
        let mut peers = HashMap::new();
        let mut peer = RemotePeerState::new(make_peer(0x2a));
        peer.has_descriptor = true;
        peer.last_solve_diagnostic = Some(XrDepthAlignSolveDiagnostic::default());
        peers.insert(peer.peer.id, peer);

        let text = make_pending_alignment_debug_text(
            "AlignDbg: local slice 2 | desc occ 0 v 0 c 0",
            &peers,
        );
        assert_eq!(text, "AlignDbg: local slice 2 | desc occ 0 v 0 c 0");
    }
}
