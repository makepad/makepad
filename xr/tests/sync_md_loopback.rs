use makepad_xr::makepad_widgets::*;
use makepad_xr::{net::*, scene::*};
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::mpsc,
    time::{Duration, Instant},
};

const TEST_IO_TIMEOUT: Duration = Duration::from_secs(3);

fn wait_for_event<F>(node: &XrNetNode, mut predicate: F) -> Option<XrNetIncoming>
where
    F: FnMut(&XrNetIncoming) -> bool,
{
    let start = Instant::now();
    while start.elapsed() < TEST_IO_TIMEOUT {
        match node
            .incoming_receiver
            .recv_timeout(Duration::from_millis(50))
        {
            Ok(event) if predicate(&event) => return Some(event),
            Ok(_) => continue,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => return None,
        }
    }
    None
}

fn localhost_config(
    node_id: u64,
    discovery_port: u16,
    data_port: u16,
    sync_port: u16,
    discovery_targets: Vec<u16>,
) -> XrNetConfig {
    XrNetConfig {
        node_id: XrNetPeerId(node_id),
        discovery_bind: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), discovery_port),
        data_bind: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), data_port),
        sync_bind: SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), sync_port),
        discovery_targets: discovery_targets
            .into_iter()
            .map(|port| SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port))
            .collect(),
        timing: XrNetTimingConfig {
            discovery_interval: Duration::from_millis(20),
            peer_timeout: Duration::from_millis(150),
            poll_interval: Duration::from_millis(5),
        },
    }
}

fn wait_for_join_pair(left: &XrNetNode, right: &XrNetNode) {
    let _ = wait_for_event(left, |event| matches!(event, XrNetIncoming::Join { .. }))
        .expect("left test client should discover right");
    let _ = wait_for_event(right, |event| matches!(event, XrNetIncoming::Join { .. }))
        .expect("right test client should discover left");
}

fn wait_for_sync_pair(left: &mut XrNetNode, right: &mut XrNetNode) {
    wait_for_join_pair(left, right);

    let left_barrier =
        left.send_activity(XrActivityId(makepad_xr::live_id!(sync_barrier_left)), 0.1);
    let received_left_barrier = wait_for_event(right, |event| {
        matches!(
            event,
            XrNetIncoming::Activity { control, .. }
                if control.state() == left_barrier
        )
    })
    .expect("right test client should receive the left->right sync barrier");
    match received_left_barrier {
        XrNetIncoming::Activity { control, .. } => assert_eq!(control.state(), left_barrier),
        _ => unreachable!(),
    }

    let right_barrier =
        right.send_activity(XrActivityId(makepad_xr::live_id!(sync_barrier_right)), 0.2);
    let received_right_barrier = wait_for_event(left, |event| {
        matches!(
            event,
            XrNetIncoming::Activity { control, .. }
                if control.state() == right_barrier
        )
    })
    .expect("left test client should receive the right->left sync barrier");
    match received_right_barrier {
        XrNetIncoming::Activity { control, .. } => assert_eq!(control.state(), right_barrier),
        _ => unreachable!(),
    }
}

fn shooter_activity_id() -> XrActivityId {
    XrActivityId(makepad_xr::live_id!(ico_shoot_scene))
}

fn shooter_binding(widget_uid: WidgetUid) -> XrSpawnableObjectBinding {
    XrSpawnableObjectBinding {
        object_id: XrSpawnableObjectId(0x44),
        allocation_group_id: XrSpawnableObjectId(0x91),
        widget_uid,
        bootstrap_shared: false,
    }
}

#[test]
fn shooter_particle_spawn_state_and_despawn_roundtrip_over_loopback() {
    let activity_id = shooter_activity_id();
    let widget_uid = WidgetUid(101);
    let mut left = XrNetNode::with_config(localhost_config(501, 45046, 45047, 45048, vec![45056]))
        .expect("left test client should bind");
    let mut right = XrNetNode::with_config(localhost_config(502, 45056, 45057, 45058, vec![45046]))
        .expect("right test client should bind");
    let left_peer_id = left.node_id();
    let right_peer_id = right.node_id();
    let mut right_registry = XrSharedObjectRegistry::default();
    right_registry.set_local_peer_id(right_peer_id);
    right_registry.replace_spawnables(activity_id, [shooter_binding(widget_uid)]);

    wait_for_sync_pair(&mut left, &mut right);

    let activity = left.send_activity(activity_id, 1.0);
    let activity_event = wait_for_event(&right, |event| {
        matches!(
            event,
            XrNetIncoming::Activity { control, .. }
                if control.state() == activity
        )
    })
    .expect("right test client should receive the synced shooter activity");
    let received_activity = match activity_event {
        XrNetIncoming::Activity { control, .. } => control.state(),
        _ => unreachable!(),
    };
    assert_eq!(received_activity, activity);

    let object_id = xr_make_shared_object_id(left_peer_id, XrSharedObjectCounter(7))
        .expect("counter should fit");
    let spawn = XrNetSharedObjectControl::XrSpawnObject {
        object_id,
        epoch: 0,
        authority: left_peer_id,
        fidelity: XrSharedObjectFidelity::ImpactCritical,
        shape: XrSharedObjectShape::ActivitySpawnable {
            activity_id,
            spawnable_id: XrSpawnableObjectId(0x44),
        },
        pose: Pose::new(Quat::default(), vec3f(0.12, 1.18, -0.46)),
        linvel: vec3f(0.8, 0.1, -6.4),
        angvel: vec3f(0.0, 1.6, 0.0),
    };
    left.send_shared_object_control(spawn.clone());

    let spawn_event = wait_for_event(&right, |event| {
        matches!(
            event,
            XrNetIncoming::SharedObjectControl {
                control: XrNetSharedObjectControl::XrSpawnObject { object_id: event_id, .. },
                ..
            } if *event_id == object_id
        )
    })
    .expect("right test client should receive the shooter particle spawn");
    let received_spawn = match spawn_event {
        XrNetIncoming::SharedObjectControl { control, .. } => control,
        _ => unreachable!(),
    };
    let remote_widget = match received_spawn {
        XrNetSharedObjectControl::XrSpawnObject {
            object_id,
            epoch,
            authority,
            fidelity,
            shape:
                XrSharedObjectShape::ActivitySpawnable {
                    activity_id,
                    spawnable_id,
                },
            pose,
            linvel,
            angvel,
        } => right_registry
            .register_remote_shared_object(
                activity_id,
                0.0,
                object_id,
                epoch,
                authority,
                fidelity,
                spawnable_id,
                pose,
                linvel,
                angvel,
            )
            .expect("remote shooter particle should bind into the projectile pool"),
        _ => unreachable!(),
    };
    assert_eq!(remote_widget, widget_uid);

    let state = XrNetSharedObjectState {
        seq: 0,
        sent_at: 1.2,
        physics_tick: 17,
        object_id,
        epoch: 0,
        authority: left_peer_id,
        fidelity: XrSharedObjectFidelity::ImpactCritical,
        mode: XrSharedObjectMode::Dynamic,
        pose: Pose::new(Quat::default(), vec3f(0.18, 1.16, -0.73)),
        linvel: vec3f(0.4, -0.2, -6.0),
        angvel: vec3f(0.0, 1.1, 0.0),
    };
    left.send_shared_object_state(state);

    let state_event = wait_for_event(&right, |event| {
        matches!(
            event,
            XrNetIncoming::SharedObjectState { state: received, .. }
                if received.object_id == object_id && received.physics_tick == 17
        )
    })
    .expect("right test client should receive the shooter particle udp state");
    let (received_peer_id, received_state) = match state_event {
        XrNetIncoming::SharedObjectState { peer, state } => (peer.id, state),
        _ => unreachable!(),
    };
    assert_eq!(received_peer_id, left_peer_id);
    assert_eq!(
        right_registry.record_remote_shared_object_state(received_peer_id, received_state),
        Some(widget_uid)
    );
    let remote_snapshot = right_registry
        .remote_shared_object_snapshot(object_id)
        .expect("remote shooter particle snapshot should exist after the udp state");
    assert_eq!(remote_snapshot.widget_uid, widget_uid);
    assert_eq!(remote_snapshot.latest_state, Some(received_state));

    let despawn = XrNetSharedObjectControl::XrDespawnObject {
        object_id,
        epoch: 0,
    };
    left.send_shared_object_control(despawn.clone());

    let despawn_event = wait_for_event(&right, |event| {
        matches!(
            event,
            XrNetIncoming::SharedObjectControl {
                control: XrNetSharedObjectControl::XrDespawnObject { object_id: event_id, .. },
                ..
            } if *event_id == object_id
        )
    })
    .expect("right test client should receive the shooter particle despawn");
    let received_despawn = match despawn_event {
        XrNetIncoming::SharedObjectControl { control, .. } => control,
        _ => unreachable!(),
    };
    assert_eq!(received_despawn, despawn);
    assert_eq!(
        right_registry.release_remote_shared_object(object_id),
        Some(widget_uid)
    );
    assert_eq!(right_registry.active_count(), 0);
}

#[test]
fn shared_object_state_roundtrip_preserves_contact_dominated_and_sleeping_modes() {
    let activity_id = shooter_activity_id();
    let widget_uid = WidgetUid(111);
    let mut left = XrNetNode::with_config(localhost_config(506, 45066, 45067, 45068, vec![45076]))
        .expect("left test client should bind");
    let mut right = XrNetNode::with_config(localhost_config(507, 45076, 45077, 45078, vec![45066]))
        .expect("right test client should bind");
    let left_peer_id = left.node_id();
    let right_peer_id = right.node_id();
    let mut right_registry = XrSharedObjectRegistry::default();
    right_registry.set_local_peer_id(right_peer_id);
    right_registry.replace_spawnables(activity_id, [shooter_binding(widget_uid)]);

    wait_for_sync_pair(&mut left, &mut right);

    let object_id = xr_make_shared_object_id(left_peer_id, XrSharedObjectCounter(8))
        .expect("counter should fit");
    let spawn = XrNetSharedObjectControl::XrSpawnObject {
        object_id,
        epoch: 0,
        authority: left_peer_id,
        fidelity: XrSharedObjectFidelity::ImpactCritical,
        shape: XrSharedObjectShape::ActivitySpawnable {
            activity_id,
            spawnable_id: XrSpawnableObjectId(0x44),
        },
        pose: Pose::new(Quat::default(), vec3f(0.05, 1.11, -0.41)),
        linvel: vec3f(0.6, 0.0, -4.8),
        angvel: vec3f(0.0, 1.2, 0.0),
    };
    left.send_shared_object_control(spawn.clone());

    let spawn_event = wait_for_event(&right, |event| {
        matches!(
            event,
            XrNetIncoming::SharedObjectControl {
                control: XrNetSharedObjectControl::XrSpawnObject { object_id: event_id, .. },
                ..
            } if *event_id == object_id
        )
    })
    .expect("right test client should receive the shared-object spawn before state updates");
    match spawn_event {
        XrNetIncoming::SharedObjectControl {
            control:
                XrNetSharedObjectControl::XrSpawnObject {
                    object_id,
                    epoch,
                    authority,
                    fidelity,
                    shape:
                        XrSharedObjectShape::ActivitySpawnable {
                            activity_id,
                            spawnable_id,
                        },
                    pose,
                    linvel,
                    angvel,
                },
            ..
        } => {
            assert_eq!(
                right_registry.register_remote_shared_object(
                    activity_id,
                    0.0,
                    object_id,
                    epoch,
                    authority,
                    fidelity,
                    spawnable_id,
                    pose,
                    linvel,
                    angvel,
                ),
                Some(widget_uid)
            );
        }
        _ => unreachable!(),
    }

    let held_state = XrNetSharedObjectState {
        seq: 0,
        sent_at: 2.0,
        physics_tick: 21,
        object_id,
        epoch: 0,
        authority: left_peer_id,
        fidelity: XrSharedObjectFidelity::ImpactCritical,
        mode: XrSharedObjectMode::ContactDominated {
            authority: left_peer_id,
            hand: XrSharedHand::RightHand,
        },
        pose: Pose::new(Quat::default(), vec3f(0.09, 1.08, -0.55)),
        linvel: vec3f(0.0, 0.0, -0.2),
        angvel: vec3f(0.0, 0.0, 0.0),
    };
    left.send_shared_object_state(held_state);

    let held_event = wait_for_event(&right, |event| {
        matches!(
            event,
            XrNetIncoming::SharedObjectState { state, .. }
                if state.object_id == object_id
                    && state.physics_tick == 21
                    && matches!(
                        state.mode,
                        XrSharedObjectMode::ContactDominated {
                            authority,
                            hand: XrSharedHand::RightHand,
                        } if authority == left_peer_id
                    )
        )
    })
    .expect("right test client should receive the held-body shared-object state");
    let (held_peer_id, held_state) = match held_event {
        XrNetIncoming::SharedObjectState { peer, state } => (peer.id, state),
        _ => unreachable!(),
    };
    assert_eq!(held_peer_id, left_peer_id);
    assert_eq!(
        right_registry.record_remote_shared_object_state(held_peer_id, held_state),
        Some(widget_uid)
    );
    assert_eq!(
        right_registry
            .remote_shared_object_snapshot(object_id)
            .expect("remote shared object should exist after held state")
            .latest_state
            .expect("held state should be stored")
            .mode,
        XrSharedObjectMode::ContactDominated {
            authority: left_peer_id,
            hand: XrSharedHand::RightHand,
        }
    );

    let sleeping_state = XrNetSharedObjectState {
        seq: 0,
        sent_at: 2.1,
        physics_tick: 22,
        object_id,
        epoch: 0,
        authority: left_peer_id,
        fidelity: XrSharedObjectFidelity::ImpactCritical,
        mode: XrSharedObjectMode::Sleeping,
        pose: Pose::new(Quat::default(), vec3f(0.10, 0.82, -0.61)),
        linvel: vec3f(0.0, 0.0, 0.0),
        angvel: vec3f(0.0, 0.0, 0.0),
    };
    left.send_shared_object_state(sleeping_state);

    let sleeping_event = wait_for_event(&right, |event| {
        matches!(
            event,
            XrNetIncoming::SharedObjectState { state, .. }
                if state.object_id == object_id
                    && state.physics_tick == 22
                    && state.mode == XrSharedObjectMode::Sleeping
        )
    })
    .expect("right test client should receive the sleeping shared-object state");
    let (sleeping_peer_id, sleeping_state) = match sleeping_event {
        XrNetIncoming::SharedObjectState { peer, state } => (peer.id, state),
        _ => unreachable!(),
    };
    assert_eq!(sleeping_peer_id, left_peer_id);
    assert_eq!(
        right_registry.record_remote_shared_object_state(sleeping_peer_id, sleeping_state),
        Some(widget_uid)
    );
    let snapshot = right_registry
        .remote_shared_object_snapshot(object_id)
        .expect("remote shared object should still exist after sleeping state");
    assert_eq!(
        snapshot
            .latest_state
            .expect("sleeping state should be stored")
            .mode,
        XrSharedObjectMode::Sleeping
    );
}

#[test]
fn two_clients_handoff_shared_object_back_and_forth_without_reallocating_identity() {
    let activity_id = shooter_activity_id();
    let widget_uid = WidgetUid(101);
    let mut left = XrNetNode::with_config(localhost_config(511, 45146, 45147, 45148, vec![45156]))
        .expect("left test client should bind");
    let mut right = XrNetNode::with_config(localhost_config(512, 45156, 45157, 45158, vec![45146]))
        .expect("right test client should bind");
    let left_peer_id = left.node_id();
    let right_peer_id = right.node_id();

    let mut left_registry = XrSharedObjectRegistry::default();
    left_registry.set_local_peer_id(left_peer_id);
    left_registry.replace_spawnables(activity_id, [shooter_binding(widget_uid)]);

    let mut right_registry = XrSharedObjectRegistry::default();
    right_registry.set_local_peer_id(right_peer_id);
    right_registry.replace_spawnables(activity_id, [shooter_binding(widget_uid)]);

    wait_for_sync_pair(&mut left, &mut right);

    let allocation = left_registry
        .allocate_local_shared_object(activity_id, widget_uid)
        .expect("left shooter particle should allocate a local shared object id");
    let object_id = allocation.shared_object_id;
    let initial_pose = Pose::new(Quat::default(), vec3f(0.10, 1.04, -0.52));
    let initial_linvel = vec3f(0.9, 0.1, -5.8);
    let initial_angvel = vec3f(0.0, 0.8, 0.0);
    let spawn = XrNetSharedObjectControl::XrSpawnObject {
        object_id,
        epoch: allocation.epoch,
        authority: left_peer_id,
        fidelity: allocation.fidelity,
        shape: XrSharedObjectShape::ActivitySpawnable {
            activity_id,
            spawnable_id: allocation.spawnable_object_id,
        },
        pose: initial_pose,
        linvel: initial_linvel,
        angvel: initial_angvel,
    };
    left.send_shared_object_control(spawn);

    let spawn_event = wait_for_event(&right, |event| {
        matches!(
            event,
            XrNetIncoming::SharedObjectControl {
                control: XrNetSharedObjectControl::XrSpawnObject { object_id: event_id, .. },
                ..
            } if *event_id == object_id
        )
    })
    .expect("right test client should receive the initial shared object spawn");
    match spawn_event {
        XrNetIncoming::SharedObjectControl {
            control:
                XrNetSharedObjectControl::XrSpawnObject {
                    object_id,
                    epoch,
                    authority,
                    fidelity,
                    shape:
                        XrSharedObjectShape::ActivitySpawnable {
                            activity_id,
                            spawnable_id,
                        },
                    pose,
                    linvel,
                    angvel,
                },
            ..
        } => {
            let widget = right_registry
                .register_remote_shared_object(
                    activity_id,
                    0.0,
                    object_id,
                    epoch,
                    authority,
                    fidelity,
                    spawnable_id,
                    pose,
                    linvel,
                    angvel,
                )
                .expect("right registry should bind the remote shared object");
            assert_eq!(widget, widget_uid);
        }
        _ => unreachable!(),
    }

    let request_id = 17u32;
    let accepted_pose = Pose::new(Quat::default(), vec3f(0.24, 1.01, -0.42));
    let accepted_linvel = vec3f(0.2, 0.0, -1.0);
    let accepted_angvel = vec3f(0.0, 0.0, 0.0);
    let takeover_request = XrNetSharedObjectControl::XrTakeoverRequest {
        object_id,
        epoch: allocation.epoch,
        request_id,
        based_on_seq: 0,
        based_on_tick: 0,
        candidate_owner: right_peer_id,
        hand: XrSharedHand::RightHand,
        hand_pose: accepted_pose,
        hand_linvel: accepted_linvel,
    };
    right.send_shared_object_control(takeover_request.clone());

    let received_request = wait_for_event(&left, |event| {
        matches!(
            event,
            XrNetIncoming::SharedObjectControl {
                control: XrNetSharedObjectControl::XrTakeoverRequest { request_id: event_id, .. },
                ..
            } if *event_id == request_id
        )
    })
    .expect("left test client should receive the takeover request");
    match received_request {
        XrNetIncoming::SharedObjectControl { control, .. } => {
            assert_eq!(control, takeover_request);
        }
        _ => unreachable!(),
    }

    assert!(left_registry.schedule_authority_transfer(
        object_id,
        allocation.epoch.wrapping_add(1),
        left_peer_id,
        right_peer_id,
        0.0,
        0,
        request_id,
        Some(XrSharedHand::RightHand),
        accepted_pose,
        accepted_linvel,
        accepted_angvel,
    ));
    let takeover_accept = XrNetSharedObjectControl::XrTakeoverAccept {
        object_id,
        epoch: allocation.epoch.wrapping_add(1),
        request_id,
        new_authority: right_peer_id,
        effective_at: 0.0,
        effective_tick: 0,
        pose: accepted_pose,
        linvel: accepted_linvel,
        angvel: accepted_angvel,
    };
    left.send_shared_object_control(takeover_accept.clone());

    let received_accept = wait_for_event(&right, |event| {
        matches!(
            event,
            XrNetIncoming::SharedObjectControl {
                control: XrNetSharedObjectControl::XrTakeoverAccept { request_id: event_id, .. },
                ..
            } if *event_id == request_id
        )
    })
    .expect("right test client should receive the takeover accept");
    match received_accept {
        XrNetIncoming::SharedObjectControl { control, .. } => {
            assert_eq!(control, takeover_accept);
        }
        _ => unreachable!(),
    }
    assert!(right_registry.schedule_authority_transfer(
        object_id,
        allocation.epoch.wrapping_add(1),
        left_peer_id,
        right_peer_id,
        0.0,
        0,
        request_id,
        None,
        accepted_pose,
        accepted_linvel,
        accepted_angvel,
    ));

    let left_transfer = left_registry.apply_scheduled_authority_transfers(0.1, 1);
    let right_transfer = right_registry.apply_scheduled_authority_transfers(0.1, 1);
    assert_eq!(left_transfer.len(), 1);
    assert_eq!(right_transfer.len(), 1);
    assert!(left_transfer[0].shadow);
    assert!(!right_transfer[0].shadow);
    assert_eq!(left_transfer[0].object_id, object_id);
    assert_eq!(right_transfer[0].object_id, object_id);
    assert_eq!(
        left_registry
            .remote_shared_object_snapshot(object_id)
            .expect("left registry should demote the object into a remote shadow")
            .authority,
        right_peer_id
    );
    assert_eq!(
        right_registry
            .local_shared_object_snapshot(object_id)
            .expect("right registry should promote the same object id into local authority")
            .authority,
        right_peer_id
    );

    let request_back_id = 18u32;
    let handoff_pose = Pose::new(Quat::default(), vec3f(-0.18, 1.03, -0.40));
    let handoff_linvel = vec3f(-0.1, 0.0, -0.4);
    let handoff_angvel = vec3f(0.0, 0.0, 0.0);
    let takeover_back = XrNetSharedObjectControl::XrTakeoverRequest {
        object_id,
        epoch: allocation.epoch.wrapping_add(1),
        request_id: request_back_id,
        based_on_seq: 0,
        based_on_tick: 0,
        candidate_owner: left_peer_id,
        hand: XrSharedHand::LeftHand,
        hand_pose: handoff_pose,
        hand_linvel: handoff_linvel,
    };
    left.send_shared_object_control(takeover_back.clone());

    let received_back_request = wait_for_event(&right, |event| {
        matches!(
            event,
            XrNetIncoming::SharedObjectControl {
                control: XrNetSharedObjectControl::XrTakeoverRequest { request_id: event_id, .. },
                ..
            } if *event_id == request_back_id
        )
    })
    .expect("right test client should receive the return takeover request");
    match received_back_request {
        XrNetIncoming::SharedObjectControl { control, .. } => {
            assert_eq!(control, takeover_back);
        }
        _ => unreachable!(),
    }

    assert!(right_registry.schedule_authority_transfer(
        object_id,
        allocation.epoch.wrapping_add(2),
        right_peer_id,
        left_peer_id,
        0.0,
        0,
        request_back_id,
        Some(XrSharedHand::LeftHand),
        handoff_pose,
        handoff_linvel,
        handoff_angvel,
    ));
    let takeover_back_accept = XrNetSharedObjectControl::XrTakeoverAccept {
        object_id,
        epoch: allocation.epoch.wrapping_add(2),
        request_id: request_back_id,
        new_authority: left_peer_id,
        effective_at: 0.0,
        effective_tick: 0,
        pose: handoff_pose,
        linvel: handoff_linvel,
        angvel: handoff_angvel,
    };
    right.send_shared_object_control(takeover_back_accept.clone());

    let received_back_accept = wait_for_event(&left, |event| {
        matches!(
            event,
            XrNetIncoming::SharedObjectControl {
                control: XrNetSharedObjectControl::XrTakeoverAccept { request_id: event_id, .. },
                ..
            } if *event_id == request_back_id
        )
    })
    .expect("left test client should receive the return takeover accept");
    match received_back_accept {
        XrNetIncoming::SharedObjectControl { control, .. } => {
            assert_eq!(control, takeover_back_accept);
        }
        _ => unreachable!(),
    }
    assert!(left_registry.schedule_authority_transfer(
        object_id,
        allocation.epoch.wrapping_add(2),
        right_peer_id,
        left_peer_id,
        0.0,
        0,
        request_back_id,
        None,
        handoff_pose,
        handoff_linvel,
        handoff_angvel,
    ));

    let left_return = left_registry.apply_scheduled_authority_transfers(0.2, 2);
    let right_return = right_registry.apply_scheduled_authority_transfers(0.2, 2);
    assert_eq!(left_return.len(), 1);
    assert_eq!(right_return.len(), 1);
    assert!(!left_return[0].shadow);
    assert!(right_return[0].shadow);
    assert_eq!(left_return[0].object_id, object_id);
    assert_eq!(right_return[0].object_id, object_id);
    assert_eq!(
        left_registry
            .local_shared_object_snapshot(object_id)
            .expect("left registry should regain local authority on the same object id")
            .authority,
        left_peer_id
    );
    assert_eq!(
        right_registry
            .remote_shared_object_snapshot(object_id)
            .expect("right registry should demote the object back into a remote shadow")
            .authority,
        left_peer_id
    );
}

#[test]
fn takeover_reject_roundtrip_over_loopback_preserves_request_identity() {
    let mut left = XrNetNode::with_config(localhost_config(521, 45246, 45247, 45248, vec![45256]))
        .expect("left test client should bind");
    let mut right = XrNetNode::with_config(localhost_config(522, 45256, 45257, 45258, vec![45246]))
        .expect("right test client should bind");
    let object_id = xr_make_shared_object_id(left.node_id(), XrSharedObjectCounter(9)).unwrap();

    wait_for_sync_pair(&mut left, &mut right);

    let request = XrNetSharedObjectControl::XrTakeoverRequest {
        object_id,
        epoch: 3,
        request_id: 31,
        based_on_seq: 14,
        based_on_tick: 22,
        candidate_owner: right.node_id(),
        hand: XrSharedHand::RightHand,
        hand_pose: Pose::new(Quat::default(), vec3f(0.16, 1.02, -0.34)),
        hand_linvel: vec3f(0.2, 0.0, -0.1),
    };
    right.send_shared_object_control(request.clone());

    let received_request = wait_for_event(&left, |event| {
        matches!(
            event,
            XrNetIncoming::SharedObjectControl {
                control: XrNetSharedObjectControl::XrTakeoverRequest { request_id, .. },
                ..
            } if *request_id == 31
        )
    })
    .expect("left test client should receive the takeover request");
    match received_request {
        XrNetIncoming::SharedObjectControl { control, .. } => assert_eq!(control, request),
        _ => unreachable!(),
    }

    let reject = XrNetSharedObjectControl::XrTakeoverReject {
        object_id,
        epoch: 3,
        request_id: 31,
        authoritative_seq: 15,
        authoritative_tick: 23,
    };
    left.send_shared_object_control(reject.clone());

    let received_reject = wait_for_event(&right, |event| {
        matches!(
            event,
            XrNetIncoming::SharedObjectControl {
                control: XrNetSharedObjectControl::XrTakeoverReject { request_id, .. },
                ..
            } if *request_id == 31
        )
    })
    .expect("right test client should receive the takeover reject");
    match received_reject {
        XrNetIncoming::SharedObjectControl { control, .. } => assert_eq!(control, reject),
        _ => unreachable!(),
    }
}

#[test]
fn contact_impulse_and_reset_controls_roundtrip_over_loopback() {
    let mut left = XrNetNode::with_config(localhost_config(531, 45346, 45347, 45348, vec![45356]))
        .expect("left test client should bind");
    let mut right = XrNetNode::with_config(localhost_config(532, 45356, 45357, 45358, vec![45346]))
        .expect("right test client should bind");
    let object_id = xr_make_shared_object_id(left.node_id(), XrSharedObjectCounter(11)).unwrap();

    wait_for_sync_pair(&mut left, &mut right);

    let impulse = XrNetSharedObjectControl::XrContactImpulse {
        object_id,
        epoch: 2,
        based_on_seq: 8,
        based_on_tick: 13,
        hand: XrSharedHand::LeftHand,
        hand_pose: Pose::new(Quat::default(), vec3f(-0.14, 1.04, -0.30)),
        point: vec3f(-0.12, 0.98, -0.46),
        impulse: vec3f(0.3, 0.1, -0.8),
    };
    right.send_shared_object_control(impulse.clone());

    let received_impulse = wait_for_event(&left, |event| {
        matches!(
            event,
            XrNetIncoming::SharedObjectControl {
                control: XrNetSharedObjectControl::XrContactImpulse { object_id: event_id, .. },
                ..
            } if *event_id == object_id
        )
    })
    .expect("left test client should receive the contact impulse control");
    match received_impulse {
        XrNetIncoming::SharedObjectControl { control, .. } => assert_eq!(control, impulse),
        _ => unreachable!(),
    }

    let reset = XrNetSharedObjectControl::XrResetObject {
        object_id,
        epoch: 3,
        pose: Pose::new(Quat::default(), vec3f(0.02, 1.10, -0.40)),
        linvel: vec3f(0.0, 0.0, 0.0),
        angvel: vec3f(0.0, 0.0, 0.0),
    };
    left.send_shared_object_control(reset.clone());

    let received_reset = wait_for_event(&right, |event| {
        matches!(
            event,
            XrNetIncoming::SharedObjectControl {
                control: XrNetSharedObjectControl::XrResetObject { object_id: event_id, .. },
                ..
            } if *event_id == object_id
        )
    })
    .expect("right test client should receive the reset control");
    match received_reset {
        XrNetIncoming::SharedObjectControl { control, .. } => assert_eq!(control, reset),
        _ => unreachable!(),
    }
}

#[test]
fn clock_ping_and_pong_controls_roundtrip_over_loopback() {
    let mut left = XrNetNode::with_config(localhost_config(541, 45446, 45447, 45448, vec![45456]))
        .expect("left test client should bind");
    let mut right = XrNetNode::with_config(localhost_config(542, 45456, 45457, 45458, vec![45446]))
        .expect("right test client should bind");

    wait_for_sync_pair(&mut left, &mut right);

    let ping = XrNetSharedObjectControl::XrClockPing {
        seq: 41,
        sent_at: 12.5,
    };
    left.send_shared_object_control(ping.clone());

    let received_ping = wait_for_event(&right, |event| {
        matches!(
            event,
            XrNetIncoming::SharedObjectControl {
                control: XrNetSharedObjectControl::XrClockPing { seq, .. },
                ..
            } if *seq == 41
        )
    })
    .expect("right test client should receive the clock ping");
    match received_ping {
        XrNetIncoming::SharedObjectControl { control, .. } => assert_eq!(control, ping),
        _ => unreachable!(),
    }

    let pong = XrNetSharedObjectControl::XrClockPong {
        seq: 41,
        echoed_at: 12.5,
        replied_at: 12.56,
    };
    right.send_shared_object_control(pong.clone());

    let received_pong = wait_for_event(&left, |event| {
        matches!(
            event,
            XrNetIncoming::SharedObjectControl {
                control: XrNetSharedObjectControl::XrClockPong { seq, .. },
                ..
            } if *seq == 41
        )
    })
    .expect("left test client should receive the clock pong");
    match received_pong {
        XrNetIncoming::SharedObjectControl { control, .. } => assert_eq!(control, pong),
        _ => unreachable!(),
    }
}
