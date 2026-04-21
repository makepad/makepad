mod tests {
    use super::*;

    #[test]
    fn remote_allocations_are_scoped_per_peer_within_a_pool_group() {
        let activity_id = XrActivityId(live_id!(ico_shoot_scene));
        let mut registry = XrSharedObjectRegistry::default();
        registry.replace_spawnables(
            activity_id,
            [
                XrSpawnableObjectBinding {
                    object_id: XrSpawnableObjectId(11),
                    allocation_group_id: XrSpawnableObjectId(91),
                    widget_uid: WidgetUid(101),
                    bootstrap_shared: true,
                },
                XrSpawnableObjectBinding {
                    object_id: XrSpawnableObjectId(12),
                    allocation_group_id: XrSpawnableObjectId(91),
                    widget_uid: WidgetUid(102),
                    bootstrap_shared: true,
                },
            ],
        );

        let left = registry.resolve_remote_widget_uid(
            activity_id,
            xr_make_shared_object_id(XrNetPeerId(1), XrSharedObjectCounter(0)).unwrap(),
            XrSpawnableObjectId(11),
        );
        let right = registry.resolve_remote_widget_uid(
            activity_id,
            xr_make_shared_object_id(XrNetPeerId(2), XrSharedObjectCounter(0)).unwrap(),
            XrSpawnableObjectId(11),
        );

        assert_eq!(left, Some(WidgetUid(101)));
        assert_eq!(right, Some(WidgetUid(102)));
    }

    #[test]
    fn local_allocations_use_hashed_shared_object_ids() {
        let activity_id = XrActivityId(live_id!(ico_shoot_scene));
        let mut registry = XrSharedObjectRegistry::default();
        registry.set_local_peer_id(XrNetPeerId(55));
        registry.replace_spawnables(
            activity_id,
            [XrSpawnableObjectBinding {
                object_id: XrSpawnableObjectId(11),
                allocation_group_id: XrSpawnableObjectId(91),
                widget_uid: WidgetUid(101),
                bootstrap_shared: true,
            }],
        );

        let allocation = registry
            .allocate_local_shared_object(activity_id, WidgetUid(101))
            .expect("local shared-object allocation should succeed");
        assert_eq!(
            allocation.shared_object_id,
            xr_make_shared_object_id(XrNetPeerId(55), XrSharedObjectCounter(0))
                .expect("hashed shared object id should allocate")
        );
        assert_eq!(allocation.spawnable_object_id, XrSpawnableObjectId(11));
        assert_eq!(allocation.epoch, 0);
    }

    #[test]
    fn pool_overflow_rebind_evicts_stale_remote_record() {
        let activity_id = XrActivityId(live_id!(ico_shoot_scene));
        let mut registry = XrSharedObjectRegistry::default();
        registry.replace_spawnables(
            activity_id,
            [
                XrSpawnableObjectBinding {
                    object_id: XrSpawnableObjectId(11),
                    allocation_group_id: XrSpawnableObjectId(91),
                    widget_uid: WidgetUid(101),
                    bootstrap_shared: false,
                },
                XrSpawnableObjectBinding {
                    object_id: XrSpawnableObjectId(12),
                    allocation_group_id: XrSpawnableObjectId(91),
                    widget_uid: WidgetUid(102),
                    bootstrap_shared: false,
                },
            ],
        );

        let object_a = xr_make_shared_object_id(XrNetPeerId(1), XrSharedObjectCounter(0)).unwrap();
        let object_b = xr_make_shared_object_id(XrNetPeerId(2), XrSharedObjectCounter(0)).unwrap();
        let object_c = xr_make_shared_object_id(XrNetPeerId(3), XrSharedObjectCounter(0)).unwrap();

        let widget_a = registry
            .register_remote_shared_object(
                activity_id,
                1.0,
                object_a,
                0,
                XrNetPeerId(1),
                XrSharedObjectFidelity::ImpactCritical,
                XrSpawnableObjectId(11),
                Pose::new(Quat::default(), vec3f(-0.2, 1.0, -0.5)),
                vec3f(0.0, 0.0, 0.0),
                vec3f(0.0, 0.0, 0.0),
            )
            .expect("first remote object should bind");
        let widget_b = registry
            .register_remote_shared_object(
                activity_id,
                2.0,
                object_b,
                0,
                XrNetPeerId(2),
                XrSharedObjectFidelity::ImpactCritical,
                XrSpawnableObjectId(11),
                Pose::new(Quat::default(), vec3f(0.0, 1.0, -0.5)),
                vec3f(0.0, 0.0, 0.0),
                vec3f(0.0, 0.0, 0.0),
            )
            .expect("second remote object should bind");
        let widget_c = registry
            .register_remote_shared_object(
                activity_id,
                3.0,
                object_c,
                0,
                XrNetPeerId(3),
                XrSharedObjectFidelity::ImpactCritical,
                XrSpawnableObjectId(11),
                Pose::new(Quat::default(), vec3f(0.2, 1.0, -0.5)),
                vec3f(0.0, 0.0, 0.0),
                vec3f(0.0, 0.0, 0.0),
            )
            .expect("third remote object should rebind into the same pool");

        assert_eq!(widget_a, WidgetUid(101));
        assert_eq!(widget_b, WidgetUid(102));
        assert!(widget_c == WidgetUid(101) || widget_c == WidgetUid(102));
        assert_eq!(registry.remote_objects.len(), 2);
        assert_eq!(registry.remote_object_to_widget.len(), 2);
        assert_eq!(registry.remote_widget_to_object.len(), 2);
        assert!(
            registry.remote_shared_object_snapshot(object_c).is_some(),
            "newest remote object should remain tracked"
        );
        assert!(
            registry.remote_shared_object_snapshot(object_a).is_none()
                || registry.remote_shared_object_snapshot(object_b).is_none(),
            "one older pooled remote object must be evicted instead of lingering as a stale shadow"
        );
    }

    #[test]
    fn pooled_local_allocations_reuse_object_id_and_advance_epoch() {
        let activity_id = XrActivityId(live_id!(ico_shoot_scene));
        let mut registry = XrSharedObjectRegistry::default();
        registry.set_local_peer_id(XrNetPeerId(55));
        registry.replace_spawnables(
            activity_id,
            [XrSpawnableObjectBinding {
                object_id: XrSpawnableObjectId(11),
                allocation_group_id: XrSpawnableObjectId(91),
                widget_uid: WidgetUid(101),
                bootstrap_shared: true,
            }],
        );

        let first = registry
            .allocate_local_shared_object(activity_id, WidgetUid(101))
            .expect("first local allocation should succeed");
        let second = registry
            .allocate_local_shared_object(activity_id, WidgetUid(101))
            .expect("pooled widget reuse should reuse the shared object id");

        assert_eq!(first.shared_object_id, second.shared_object_id);
        assert_eq!(first.epoch, 0);
        assert_eq!(second.epoch, 1);
    }

    #[test]
    fn local_spawn_prepares_oldest_pooled_object_before_newer_remote_or_local_slots() {
        let activity_id = XrActivityId(live_id!(ico_shoot_scene));
        let mut registry = XrSharedObjectRegistry::default();
        registry.set_local_peer_id(XrNetPeerId(55));
        registry.replace_spawnables(
            activity_id,
            [
                XrSpawnableObjectBinding {
                    object_id: XrSpawnableObjectId(11),
                    allocation_group_id: XrSpawnableObjectId(91),
                    widget_uid: WidgetUid(101),
                    bootstrap_shared: false,
                },
                XrSpawnableObjectBinding {
                    object_id: XrSpawnableObjectId(12),
                    allocation_group_id: XrSpawnableObjectId(91),
                    widget_uid: WidgetUid(102),
                    bootstrap_shared: false,
                },
            ],
        );

        let remote_object_id =
            xr_make_shared_object_id(XrNetPeerId(7), XrSharedObjectCounter(3)).unwrap();
        registry
            .register_remote_shared_object(
                activity_id,
                1.0,
                remote_object_id,
                0,
                XrNetPeerId(7),
                XrSharedObjectFidelity::ImpactCritical,
                XrSpawnableObjectId(11),
                Pose::new(Quat::default(), vec3f(-0.1, 1.0, -0.5)),
                vec3f(0.0, 0.0, 0.0),
                vec3f(0.0, 0.0, 0.0),
            )
            .expect("remote projectile should bind");

        let local_recent = registry
            .force_local_shared_object_reset(
                activity_id,
                WidgetUid(102),
                2.0,
                Pose::new(Quat::default(), vec3f(0.1, 1.0, -0.4)),
                vec3f(0.0, 0.0, 0.0),
                vec3f(0.0, 0.0, 0.0),
            )
            .expect("local projectile should claim the second slot");

        let (allocation, reused_remote) = registry
            .prepare_local_spawn_allocation(
                activity_id,
                WidgetUid(102),
                3.0,
                Pose::new(Quat::default(), vec3f(0.2, 1.0, -0.3)),
                vec3f(0.0, 0.0, 0.0),
                vec3f(0.0, 0.0, 0.0),
            )
            .expect("pool-full spawn should reclaim the oldest slot");

        assert!(reused_remote, "oldest pooled slot should come from the remote shadow");
        assert_eq!(allocation.widget_uid, WidgetUid(101));
        assert_eq!(allocation.shared_object_id, remote_object_id);
        assert_eq!(
            registry
                .local_shared_object_snapshot(local_recent.shared_object_id)
                .expect("newer local pooled object should remain in place")
                .widget_uid,
            WidgetUid(102)
        );
    }

    #[test]
    fn prune_missing_local_shared_objects_returns_despawns() {
        let activity_id = XrActivityId(live_id!(ico_shoot_scene));
        let mut registry = XrSharedObjectRegistry::default();
        registry.set_local_peer_id(XrNetPeerId(55));
        registry.replace_spawnables(
            activity_id,
            [XrSpawnableObjectBinding {
                object_id: XrSpawnableObjectId(11),
                allocation_group_id: XrSpawnableObjectId(91),
                widget_uid: WidgetUid(101),
                bootstrap_shared: true,
            }],
        );
        let allocation = registry
            .allocate_local_shared_object(activity_id, WidgetUid(101))
            .expect("allocation should succeed");

        let despawns = registry.prune_missing_local_shared_objects(&HashMap::new());
        assert_eq!(despawns.len(), 1);
        assert_eq!(despawns[0].0, allocation.shared_object_id);
        assert_eq!(despawns[0].1, allocation.epoch);
        assert_eq!(despawns[0].2, WidgetUid(101));
        assert_eq!(registry.active_count(), 0);
    }

    #[test]
    fn scheduled_takeover_promotes_remote_object_to_local_authority() {
        let activity_id = XrActivityId(live_id!(ico_shoot_scene));
        let mut registry = XrSharedObjectRegistry::default();
        registry.set_local_peer_id(XrNetPeerId(55));
        registry.replace_spawnables(
            activity_id,
            [XrSpawnableObjectBinding {
                object_id: XrSpawnableObjectId(11),
                allocation_group_id: XrSpawnableObjectId(91),
                widget_uid: WidgetUid(101),
                bootstrap_shared: true,
            }],
        );

        let object_id = xr_make_shared_object_id(XrNetPeerId(7), XrSharedObjectCounter(3)).unwrap();
        let widget_uid = registry
            .register_remote_shared_object(
                activity_id,
                0.0,
                object_id,
                0,
                XrNetPeerId(7),
                XrSharedObjectFidelity::ImpactCritical,
                XrSpawnableObjectId(11),
                Pose::new(Quat::default(), vec3f(0.0, 1.0, -0.5)),
                vec3f(0.0, 0.0, 0.0),
                vec3f(0.0, 0.0, 0.0),
            )
            .expect("remote object should bind into the spawnable pool");
        assert_eq!(widget_uid, WidgetUid(101));

        assert!(registry.schedule_authority_transfer(
            object_id,
            1,
            XrNetPeerId(7),
            XrNetPeerId(55),
            0.0,
            0,
            17,
            Some(XrSharedHand::RightHand),
            Pose::new(Quat::default(), vec3f(0.1, 1.0, -0.45)),
            vec3f(0.2, 0.0, 0.0),
            vec3f(0.0, 0.0, 0.0),
        ));
        let transfers = registry.apply_scheduled_authority_transfers(0.2, 1);

        assert_eq!(transfers.len(), 1);
        assert!(!transfers[0].shadow);
        assert_eq!(transfers[0].object_id, object_id);
        assert_eq!(transfers[0].widget_uid, WidgetUid(101));
        assert!(registry.remote_shared_object_snapshot(object_id).is_none());
        let local = registry
            .local_shared_object_snapshot(object_id)
            .expect("takeover should keep the same object id under local authority");
        assert_eq!(local.widget_uid, WidgetUid(101));
        assert_eq!(local.epoch, 1);
        assert_eq!(local.authority, XrNetPeerId(55));
    }

    #[test]
    fn scheduled_handoff_demotes_local_object_to_remote_shadow() {
        let activity_id = XrActivityId(live_id!(ico_shoot_scene));
        let mut registry = XrSharedObjectRegistry::default();
        registry.set_local_peer_id(XrNetPeerId(55));
        registry.replace_spawnables(
            activity_id,
            [XrSpawnableObjectBinding {
                object_id: XrSpawnableObjectId(11),
                allocation_group_id: XrSpawnableObjectId(91),
                widget_uid: WidgetUid(101),
                bootstrap_shared: true,
            }],
        );

        let allocation = registry
            .allocate_local_shared_object(activity_id, WidgetUid(101))
            .expect("local allocation should succeed");
        assert!(registry.schedule_authority_transfer(
            allocation.shared_object_id,
            allocation.epoch.wrapping_add(1),
            XrNetPeerId(55),
            XrNetPeerId(8),
            0.0,
            0,
            23,
            Some(XrSharedHand::LeftHand),
            Pose::new(Quat::default(), vec3f(-0.1, 1.1, -0.4)),
            vec3f(-0.2, 0.0, 0.1),
            vec3f(0.0, 0.0, 0.0),
        ));
        let transfers = registry.apply_scheduled_authority_transfers(0.2, 1);

        assert_eq!(transfers.len(), 1);
        assert!(transfers[0].shadow);
        assert_eq!(transfers[0].source_authority, XrNetPeerId(55));
        assert!(registry
            .local_shared_object_snapshot(allocation.shared_object_id)
            .is_none());
        let remote = registry
            .remote_shared_object_snapshot(allocation.shared_object_id)
            .expect("handoff should preserve the object id as a remote shadow");
        assert_eq!(remote.widget_uid, WidgetUid(101));
        assert_eq!(remote.authority, XrNetPeerId(8));
        assert_eq!(remote.state_source_authority, XrNetPeerId(55));
    }

    #[test]
    fn replacing_spawnables_for_same_activity_preserves_active_shared_objects() {
        let activity_id = XrActivityId(live_id!(ico_shoot_scene));
        let mut registry = XrSharedObjectRegistry::default();
        registry.set_local_peer_id(XrNetPeerId(55));
        registry.replace_spawnables(
            activity_id,
            [
                XrSpawnableObjectBinding {
                    object_id: XrSpawnableObjectId(11),
                    allocation_group_id: XrSpawnableObjectId(11),
                    widget_uid: WidgetUid(101),
                    bootstrap_shared: true,
                },
                XrSpawnableObjectBinding {
                    object_id: XrSpawnableObjectId(12),
                    allocation_group_id: XrSpawnableObjectId(12),
                    widget_uid: WidgetUid(102),
                    bootstrap_shared: true,
                },
            ],
        );

        let local = registry
            .allocate_local_shared_object(activity_id, WidgetUid(101))
            .expect("local shared object should allocate");
        let remote_object_id =
            xr_make_shared_object_id(XrNetPeerId(7), XrSharedObjectCounter(9)).unwrap();
        registry
            .register_remote_shared_object(
                activity_id,
                0.0,
                remote_object_id,
                0,
                XrNetPeerId(7),
                XrSharedObjectFidelity::ImpactCritical,
                XrSpawnableObjectId(12),
                Pose::new(Quat::default(), vec3f(0.0, 1.0, -0.4)),
                vec3f(0.0, 0.0, -1.0),
                vec3f(0.0, 0.0, 0.0),
            )
            .expect("remote shared object should bind");

        registry.replace_spawnables(
            activity_id,
            [
                XrSpawnableObjectBinding {
                    object_id: XrSpawnableObjectId(11),
                    allocation_group_id: XrSpawnableObjectId(11),
                    widget_uid: WidgetUid(201),
                    bootstrap_shared: true,
                },
                XrSpawnableObjectBinding {
                    object_id: XrSpawnableObjectId(12),
                    allocation_group_id: XrSpawnableObjectId(12),
                    widget_uid: WidgetUid(202),
                    bootstrap_shared: true,
                },
            ],
        );

        assert_eq!(
            registry
                .local_shared_object_snapshot(local.shared_object_id)
                .expect("local shared object should survive refresh")
                .widget_uid,
            WidgetUid(201)
        );
        assert_eq!(
            registry
                .remote_shared_object_snapshot(remote_object_id)
                .expect("remote shared object should survive refresh")
                .widget_uid,
            WidgetUid(202)
        );
    }

    #[test]
    fn remote_state_updates_are_rejected_from_the_wrong_authority() {
        let activity_id = XrActivityId(live_id!(ico_shoot_scene));
        let mut registry = XrSharedObjectRegistry::default();
        registry.replace_spawnables(
            activity_id,
            [XrSpawnableObjectBinding {
                object_id: XrSpawnableObjectId(11),
                allocation_group_id: XrSpawnableObjectId(11),
                widget_uid: WidgetUid(101),
                bootstrap_shared: true,
            }],
        );

        let object_id = xr_make_shared_object_id(XrNetPeerId(7), XrSharedObjectCounter(3)).unwrap();
        registry
            .register_remote_shared_object(
                activity_id,
                0.0,
                object_id,
                0,
                XrNetPeerId(7),
                XrSharedObjectFidelity::ImpactCritical,
                XrSpawnableObjectId(11),
                Pose::new(Quat::default(), vec3f(0.0, 1.0, -0.5)),
                vec3f(0.0, 0.0, -1.0),
                vec3f(0.0, 0.0, 0.0),
            )
            .expect("remote object should bind");

        let stale_state = XrNetSharedObjectState {
            seq: 1,
            sent_at: 0.5,
            physics_tick: 10,
            object_id,
            epoch: 0,
            authority: XrNetPeerId(8),
            fidelity: XrSharedObjectFidelity::ImpactCritical,
            mode: XrSharedObjectMode::Dynamic,
            pose: Pose::new(Quat::default(), vec3f(0.4, 1.0, -0.3)),
            linvel: vec3f(1.0, 0.0, 0.0),
            angvel: vec3f(0.0, 0.0, 0.0),
        };
        assert!(registry
            .record_remote_shared_object_state(XrNetPeerId(8), stale_state)
            .is_none());

        let fresh_state = XrNetSharedObjectState {
            authority: XrNetPeerId(7),
            ..stale_state
        };
        assert_eq!(
            registry.record_remote_shared_object_state(XrNetPeerId(7), fresh_state),
            Some(WidgetUid(101))
        );
        let snapshot = registry
            .remote_shared_object_snapshot(object_id)
            .expect("remote snapshot should still exist");
        assert_eq!(snapshot.authority, XrNetPeerId(7));
        assert_eq!(snapshot.state_source_authority, XrNetPeerId(7));
        assert_eq!(snapshot.latest_state, Some(fresh_state));
    }

    #[test]
    fn remote_state_updates_accept_matching_net_peer_authority_ids() {
        let activity_id = XrActivityId(live_id!(ico_shoot_scene));
        let mut registry = XrSharedObjectRegistry::default();
        registry.replace_spawnables(
            activity_id,
            [XrSpawnableObjectBinding {
                object_id: XrSpawnableObjectId(11),
                allocation_group_id: XrSpawnableObjectId(11),
                widget_uid: WidgetUid(101),
                bootstrap_shared: true,
            }],
        );

        let sender_node_id = XrNetPeerId(0x1234);
        let sender_peer = XrNetPeer {
            id: sender_node_id,
            addr: "192.168.1.42:41547".parse().unwrap(),
        };
        let object_id = xr_make_shared_object_id(sender_node_id, XrSharedObjectCounter(3))
            .expect("shared object id should hash");
        registry
            .register_remote_shared_object(
                activity_id,
                0.0,
                object_id,
                0,
                sender_node_id,
                XrSharedObjectFidelity::ImpactCritical,
                XrSpawnableObjectId(11),
                Pose::new(Quat::default(), vec3f(0.0, 1.0, -0.5)),
                vec3f(0.0, 0.0, -1.0),
                vec3f(0.0, 0.0, 0.0),
            )
            .expect("remote object should bind");

        let updated_state = XrNetSharedObjectState {
            seq: 1,
            sent_at: 0.5,
            physics_tick: 10,
            object_id,
            epoch: 0,
            authority: sender_node_id,
            fidelity: XrSharedObjectFidelity::ImpactCritical,
            mode: XrSharedObjectMode::Dynamic,
            pose: Pose::new(Quat::default(), vec3f(0.4, 1.0, -0.3)),
            linvel: vec3f(1.0, 0.0, 0.0),
            angvel: vec3f(0.0, 0.0, 0.0),
        };

        assert_eq!(
            registry.record_remote_shared_object_state(sender_peer.id, updated_state),
            Some(WidgetUid(101))
        );
    }

    #[test]
    fn force_local_shared_object_reset_reclaims_remote_object_for_local_resetter() {
        let activity_id = XrActivityId(live_id!(ico_shoot_scene));
        let widget_uid = WidgetUid(101);
        let mut registry = XrSharedObjectRegistry::default();
        registry.set_local_peer_id(XrNetPeerId(55));
        registry.replace_spawnables(
            activity_id,
            [XrSpawnableObjectBinding {
                object_id: XrSpawnableObjectId(11),
                allocation_group_id: XrSpawnableObjectId(11),
                widget_uid,
                bootstrap_shared: true,
            }],
        );

        let object_id = xr_make_shared_object_id(XrNetPeerId(7), XrSharedObjectCounter(3))
            .expect("shared object id should hash");
        registry
            .register_remote_shared_object(
                activity_id,
                0.0,
                object_id,
                2,
                XrNetPeerId(7),
                XrSharedObjectFidelity::ImpactCritical,
                XrSpawnableObjectId(11),
                Pose::new(Quat::default(), vec3f(0.0, 1.0, -0.5)),
                vec3f(0.0, 0.0, -1.0),
                vec3f(0.0, 0.0, 0.0),
            )
            .expect("remote block should bind");

        let reset_pose = Pose::new(Quat::default(), vec3f(0.1, 1.2, -0.3));
        let reset_linvel = vec3f(0.2, 0.0, -0.4);
        let reset_angvel = vec3f(0.0, 0.1, 0.0);
        let allocation = registry
            .force_local_shared_object_reset(
                activity_id,
                widget_uid,
                0.0,
                reset_pose,
                reset_linvel,
                reset_angvel,
            )
            .expect("reset should reclaim the remote object for the local peer");

        assert_eq!(allocation.shared_object_id, object_id);
        assert_eq!(allocation.epoch, 3);
        assert!(registry.remote_shared_object_snapshot(object_id).is_none());

        let local = registry
            .local_shared_object_snapshot(object_id)
            .expect("reset should promote the reclaimed brick into local authority");
        assert_eq!(local.authority, XrNetPeerId(55));
        assert_eq!(local.widget_uid, widget_uid);
        assert_eq!(local.epoch, 3);
        let latest_state = local
            .latest_state
            .expect("reclaimed local brick should carry the reset state");
        assert_eq!(latest_state.pose, reset_pose);
        assert_eq!(latest_state.linvel, reset_linvel);
        assert_eq!(latest_state.angvel, reset_angvel);
    }

    #[test]
    fn releasing_remote_shared_objects_uses_current_authority_not_allocator_peer() {
        let activity_id = XrActivityId(live_id!(ico_shoot_scene));
        let widget_uid = WidgetUid(101);
        let mut registry = XrSharedObjectRegistry::default();
        registry.set_local_peer_id(XrNetPeerId(55));
        registry.replace_spawnables(
            activity_id,
            [XrSpawnableObjectBinding {
                object_id: XrSpawnableObjectId(11),
                allocation_group_id: XrSpawnableObjectId(11),
                widget_uid,
                bootstrap_shared: true,
            }],
        );

        let object_id = xr_make_shared_object_id(XrNetPeerId(7), XrSharedObjectCounter(3))
            .expect("shared object id should hash");
        registry
            .register_remote_shared_object(
                activity_id,
                0.0,
                object_id,
                0,
                XrNetPeerId(7),
                XrSharedObjectFidelity::ImpactCritical,
                XrSpawnableObjectId(11),
                Pose::new(Quat::default(), vec3f(0.0, 1.0, -0.5)),
                vec3f(0.0, 0.0, -1.0),
                vec3f(0.0, 0.0, 0.0),
            )
            .expect("remote block should bind");

        assert!(registry.schedule_authority_transfer(
            object_id,
            1,
            XrNetPeerId(7),
            XrNetPeerId(8),
            0.0,
            0,
            17,
            None,
            Pose::new(Quat::default(), vec3f(0.1, 1.0, -0.4)),
            vec3f(0.0, 0.0, -0.5),
            vec3f(0.0, 0.0, 0.0),
        ));
        let transfers = registry.apply_scheduled_authority_transfers(0.1, 1);
        assert_eq!(transfers.len(), 1);
        assert_eq!(transfers[0].new_authority, XrNetPeerId(8));

        assert_eq!(
            registry.release_remote_shared_objects_by_peer_id(XrNetPeerId(7)),
            Vec::<WidgetUid>::new()
        );
        assert!(registry.remote_shared_object_snapshot(object_id).is_some());

        assert_eq!(
            registry.release_remote_shared_objects_by_peer_id(XrNetPeerId(8)),
            vec![widget_uid]
        );
        assert!(registry.remote_shared_object_snapshot(object_id).is_none());
    }
}
