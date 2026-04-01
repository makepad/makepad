use super::*;

impl XrPeerSync {
    pub(super) fn touch_signal_text_for_state(
        header_label: &str,
        left_label: &str,
        right_label: &str,
        state: &XrState,
    ) -> String {
        let left_fingers = [
            state.left_hand.finger_bend_degrees(XrHand::INDEX_TIP),
            state.left_hand.finger_bend_degrees(XrHand::MIDDLE_TIP),
            state.left_hand.finger_bend_degrees(XrHand::RING_TIP),
            state.left_hand.finger_bend_degrees(XrHand::LITTLE_TIP),
        ];
        let right_fingers = [
            state.right_hand.finger_bend_degrees(XrHand::INDEX_TIP),
            state.right_hand.finger_bend_degrees(XrHand::MIDDLE_TIP),
            state.right_hand.finger_bend_degrees(XrHand::RING_TIP),
            state.right_hand.finger_bend_degrees(XrHand::LITTLE_TIP),
        ];
        let left_open = state.left_hand.is_open();
        let right_open = state.right_hand.is_open();
        let left_up = state.left_hand.is_upright_for_box_sync();
        let right_up = state.right_hand.is_upright_for_box_sync();
        let arms_out = Self::state_arm_corridor_ready(state);
        let left_pose_ready = left_open && left_up;
        let right_pose_ready = right_open && right_up;
        let ready = left_pose_ready && right_pose_ready && arms_out;
        format!(
            "{}: open L{} R{} | up L{} R{} | arms {} | ready {}\n{}: pass {} | bend {} | avg {} | why {}\n{}: pass {} | bend {} | avg {} | why {}",
            header_label,
            bool_digit(left_open),
            bool_digit(right_open),
            bool_digit(left_up),
            bool_digit(right_up),
            bool_digit(arms_out),
            bool_digit(ready),
            left_label,
            finger_pass_bits_text(&left_fingers),
            finger_bends_text(&left_fingers),
            average_bend_text(&left_fingers),
            open_fail_reason(&left_fingers),
            right_label,
            finger_pass_bits_text(&right_fingers),
            finger_bends_text(&right_fingers),
            average_bend_text(&right_fingers),
            open_fail_reason(&right_fingers),
        )
    }

    pub(super) fn ensure_net_node(&mut self) {
        if self.runtime.net_node.is_some() {
            return;
        }
        let node_result = if let Some(config) = self.net_config_override.clone() {
            XrNetNode::with_config(config)
        } else {
            XrNetNode::new()
        };
        match node_result {
            Ok(node) => {
                self.runtime.shared_objects.set_local_peer_id(node.node_id());
                self.runtime.net_node = Some(node);
                self.runtime.local.last_sent_descriptor_signature = None;
                self.runtime.local.last_sent_descriptor = None;
                self.runtime.local.last_sent_descriptor_at = None;
                self.runtime.metrics.record_node_started();
            }
            Err(err) => {
                self.diagnostics.set_network_bind_failed(&err.to_string());
            }
        }
    }

    pub(super) fn refresh_from_local_state(&mut self, cx: &mut Cx, state: &XrState) {
        if !self.enabled {
            return;
        }
        self.ensure_net_node();
        self.runtime.local.state_time = state.time;
        if self
            .runtime
            .recent_anchor_confirmation
            .is_some_and(|confirmation| state.time > confirmation.visible_until)
        {
            self.runtime.recent_anchor_confirmation = None;
        }
        self.runtime.local.anchor = state.anchor;
        self.runtime.local.sync_anchor = state.sync_anchor;
        if let Some(sync_anchor) = state.sync_anchor {
            self.runtime.local.record_sync_anchor(sync_anchor);
        }
        self.runtime.local.prune_recent_sync_anchors();
        let fresh_fist_hold_anchor = Self::state_fist_ack_anchor(state);
        if let Some(anchor) = fresh_fist_hold_anchor {
            self.runtime.local.fist_hold_anchor = Some(anchor);
            self.runtime.local.last_fist_hold_seen_at = Some(state.time);
        } else if self
            .runtime
            .local
            .last_fist_hold_seen_at
            .is_some_and(|seen_at| state.time - seen_at > Self::FIST_ACK_STICKY_WINDOW_SECONDS)
        {
            self.runtime.local.fist_hold_anchor = None;
            self.runtime.local.last_fist_hold_seen_at = None;
            self.runtime.local.sync_anchor_accumulator = None;
            self.runtime.local.last_sync_match_at = None;
        }
        self.runtime.local.previous_xr_state = self.runtime.local.latest_xr_state.take();
        self.runtime.local.latest_xr_state = Some(state.clone());
        if let (Some(local_anchor), Some(override_anchor)) = (
            self.runtime.local.anchor,
            self.runtime.local.anchor_override,
        ) {
            if Self::anchors_match(local_anchor, override_anchor) {
                self.runtime.local.anchor_override = None;
            }
        }
        let effective_local_anchor = self.runtime.local.effective_anchor();
        let mut broadcast_state = state.clone();
        broadcast_state.anchor = effective_local_anchor;
        if let Some(net_node) = self.runtime.net_node.as_mut() {
            net_node.send_state(broadcast_state);
        } else {
            return;
        }
        self.service_clock_ping(state.time);
        self.runtime.metrics.tx_state_count = self.runtime.metrics.tx_state_count.saturating_add(1);

        if !self.auto_alignment_enabled {
            self.runtime.local.descriptor = None;
            self.runtime.local.descriptor_version = None;
            self.runtime.local.slice_preview = None;
            self.runtime.local.last_sent_descriptor_signature = None;
            self.runtime.local.last_sent_descriptor = None;
            self.runtime.local.last_sent_descriptor_at = None;
            if let Some(worker) = self.runtime.alignment_worker.as_mut() {
                worker.clear_local_descriptor();
            }
            self.refresh_peer_transforms(cx);
            return;
        }

        let next_snapshot = cx.xr_tsdf().latest_tsdf_snapshot();
        let next_signature = next_snapshot
            .as_ref()
            .map(|snapshot| (snapshot.generation, snapshot.update_sequence));
        let next_slice_preview = next_snapshot
            .as_ref()
            .and_then(|snapshot| XrDepthAlignSlicePreview::from_tsdf_snapshot(snapshot.as_ref()));
        let next_descriptor = next_snapshot.as_ref().and_then(|snapshot| {
            XrNetAlignmentDescriptorFrame::from_tsdf_snapshot(snapshot.as_ref(), state.time)
        });

        if let (Some(signature), Some(frame)) = (next_signature, next_descriptor) {
            let change_score = self
                .runtime
                .local
                .last_sent_descriptor
                .as_ref()
                .map(|previous| descriptor_change_score(previous, &frame.descriptor))
                .unwrap_or(1.0);
            let should_publish = self.runtime.local.last_sent_descriptor_signature
                != Some(signature)
                && (self.runtime.local.last_sent_descriptor.is_none()
                    || change_score * 100.0 >= Self::DESCRIPTOR_SEND_MIN_CHANGE_PERCENT);
            if should_publish {
                self.runtime.local.descriptor = Some(frame.clone());
                self.runtime.local.descriptor_version = Some(signature);
                self.runtime.local.slice_preview = next_slice_preview;
                if let Some(worker) = self.runtime.alignment_worker.as_mut() {
                    worker.set_local_descriptor(frame.clone(), signature);
                }
                if let Some(net_node) = self.runtime.net_node.as_mut() {
                    net_node.send_alignment_descriptor(frame);
                }
                self.runtime.local.last_sent_descriptor_signature = Some(signature);
                self.runtime.local.last_sent_descriptor = self
                    .runtime
                    .local
                    .descriptor
                    .as_ref()
                    .map(|frame| frame.descriptor.clone());
                self.runtime.local.last_sent_descriptor_at = Some(state.time);
                self.runtime.metrics.tx_descriptor_count =
                    self.runtime.metrics.tx_descriptor_count.saturating_add(1);
            }
        } else {
            self.runtime.local.descriptor = None;
            self.runtime.local.descriptor_version = None;
            self.runtime.local.slice_preview = None;
            self.runtime.local.last_sent_descriptor_signature = None;
            self.runtime.local.last_sent_descriptor = None;
            self.runtime.local.last_sent_descriptor_at = None;
            if let Some(worker) = self.runtime.alignment_worker.as_mut() {
                worker.clear_local_descriptor();
            }
        }
        self.refresh_peer_transforms(cx);
    }

    pub(super) fn timed_event_local_time(event: &Event) -> Option<f64> {
        match event {
            Event::Draw(draw) => Some(draw.time),
            Event::NextFrame(next_frame) => Some(next_frame.time),
            _ => None,
        }
    }

    pub(super) fn service_non_xr_local_clock(&mut self, local_time: f64) {
        if self.runtime.local.latest_xr_state.is_some() {
            return;
        }
        self.runtime.local.state_time = local_time;
        self.runtime.metrics.record_non_xr_draw_clock();
        self.ensure_net_node();
        self.service_clock_ping(local_time);
    }

    fn service_clock_ping(&mut self, local_time: f64) {
        if local_time < self.runtime.next_clock_ping_at {
            return;
        }
        let Some(net_node) = self.runtime.net_node.as_mut() else {
            return;
        };
        let seq = self.runtime.next_clock_ping_seq;
        self.runtime.next_clock_ping_seq = self.runtime.next_clock_ping_seq.wrapping_add(1);
        self.runtime.next_clock_ping_at = local_time + Self::CLOCK_PING_INTERVAL_SECONDS;
        self.runtime.pending_clock_pings.push_back((seq, local_time));
        while self.runtime.pending_clock_pings.len() > 32 {
            self.runtime.pending_clock_pings.pop_front();
        }
        net_node.send_shared_object_control(XrNetSharedObjectControl::XrClockPing {
            seq,
            sent_at: local_time,
        });
        self.runtime.metrics.record_clock_ping_tx(seq);
    }

    pub(super) fn poll_network(&mut self, cx: &mut Cx) {
        if !self.enabled {
            return;
        }

        let mut disconnected = false;
        let mut received_message = false;
        loop {
            let result = match self.runtime.net_node.as_mut() {
                Some(net_node) => net_node.incoming_receiver.try_recv(),
                None => break,
            };
            match result {
                Ok(message) => {
                    received_message = true;
                    self.handle_network_message(cx, message);
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    disconnected = true;
                    break;
                }
            }
        }

        if disconnected {
            self.runtime.net_node = None;
            self.runtime.accepted_activity = None;
            self.runtime.pending_shared_object_controls.clear();
            self.runtime.applied_remote_shadow_states.clear();
            self.runtime.local.last_sent_descriptor_signature = None;
            self.runtime.local.last_sent_descriptor = None;
            self.runtime.local.last_sent_descriptor_at = None;
            self.diagnostics.set_network_disconnected();
        } else if received_message {
            self.refresh_peer_transforms(cx);
        }
    }

    fn handle_network_message(&mut self, cx: &mut Cx, message: XrNetIncoming) {
        match message {
            XrNetIncoming::Join { peer } => {
                self.runtime.metrics.record_join(peer.id);
                self.runtime.registry.track_join(peer);
                self.runtime.local_shared_object_reannounce_needed = true;
            }
            XrNetIncoming::Leave { peer, .. } => {
                self.runtime.metrics.record_leave(peer.id);
                for widget_uid in self
                    .runtime
                    .shared_objects
                    .release_remote_shared_objects_by_peer_id(peer.id)
                {
                    self.emit_remote_body_despawn(cx, widget_uid);
                }
                self.runtime
                    .applied_remote_shadow_states
                    .retain(|_, applied| applied.peer_id != peer.id);
                self.runtime.registry.track_leave(peer.id);
                self.runtime
                    .pending_shared_object_controls
                    .retain(|(pending_peer, _)| pending_peer.id != peer.id);
                if let Some(worker) = self.runtime.alignment_worker.as_mut() {
                    worker.remove_peer(peer.id);
                }
            }
            XrNetIncoming::State { peer, frame } => {
                self.runtime.metrics.record_state(peer.id, frame.seq);
                self.runtime
                    .registry
                    .track_state(peer, frame, self.runtime.local.state_time);
            }
            XrNetIncoming::AlignmentDescriptor { peer, frame } => {
                self.runtime.metrics.record_descriptor(peer.id, frame.seq);
                self.runtime.registry.track_descriptor(peer, frame.clone());
                if let Some(worker) = self.runtime.alignment_worker.as_mut() {
                    worker.set_peer_descriptor(peer, frame);
                }
            }
            XrNetIncoming::Activity { peer, control } => {
                let activity = control.state();
                self.runtime.metrics.record_activity_rx(peer.id, activity);
                if self
                    .runtime
                    .accepted_activity
                    .is_none_or(|current| activity.is_newer_than(&current))
                {
                    let activity_changed = self
                        .runtime
                        .accepted_activity
                        .is_none_or(|current| current != activity);
                    self.runtime.accepted_activity = Some(activity);
                    if activity_changed {
                        self.runtime.registry.clear_remote_activity_poses();
                        self.runtime.pending_shared_object_controls.clear();
                        cx.widget_action(
                            self.widget_uid(),
                            XrPeerSyncAction::ActivityChanged(activity.activity_id),
                        );
                    }
                }
            }
            XrNetIncoming::BodySpawn { peer, spawn } => {
                self.runtime
                    .metrics
                    .record_body_spawn_rx(peer.id, spawn.object_id.0);
                let Some(widget_uid) = self
                    .runtime
                    .shared_objects
                    .resolve_widget_uid(spawn.activity_id, spawn.object_id)
                else {
                    return;
                };
                self.emit_remote_body_spawn(
                    cx,
                    peer.id,
                    widget_uid,
                    true,
                    XrSharedObjectMode::Dynamic,
                    spawn.pose,
                    spawn.linvel,
                    spawn.angvel,
                );
            }
            XrNetIncoming::SharedObjectControl { peer, control } => {
                if !self.apply_shared_object_control(cx, peer, &control) {
                    self.runtime
                        .pending_shared_object_controls
                        .push((peer, control));
                }
            }
            XrNetIncoming::SharedObjectState { peer, state } => {
                let received_at_local_time = self.current_local_time();
                let state = self.normalize_incoming_shared_object_state(
                    peer.id,
                    state,
                    received_at_local_time,
                );
                self.runtime.metrics.record_shared_object_state_rx(
                    peer.id,
                    state.object_id,
                    state.seq,
                );
                if self
                    .runtime
                    .shared_objects
                    .record_remote_shared_object_state(peer.id, state)
                    .is_none()
                {
                    return;
                }
            }
            XrNetIncoming::Alignment { .. } => {}
        }
    }

    pub(super) fn apply_alignment_results(&mut self, cx: &mut Cx) {
        let Some(worker) = self.runtime.alignment_worker.as_mut() else {
            return;
        };
        let Some(result) = worker.take_latest_result() else {
            return;
        };

        self.runtime
            .registry
            .apply_alignment_results(result.peer_results);
        self.diagnostics.alignment_debug_status = result.alignment_debug_text;
        self.refresh_peer_transforms(cx);
    }

    fn local_sync_status_text(&self) -> String {
        self.runtime
            .local
            .active_sync_anchor()
            .map(|sync| format!("armed {} {}", sync_extrema_label(sync.extrema), sync.id))
            .unwrap_or_else(|| "idle".to_string())
    }

    fn poses_match(left: Pose, right: Pose) -> bool {
        let translation_delta = (left.position - right.position).length();
        let rotation_dot = left
            .orientation
            .dot(right.orientation)
            .abs()
            .clamp(0.0, 1.0);
        let rotation_delta_degrees = (2.0 * rotation_dot.acos()).to_degrees();
        translation_delta <= 0.01 && rotation_delta_degrees <= 1.5
    }

    pub(super) fn emit_authoritative_remote_activity_pose_if_changed(&mut self, cx: &mut Cx) {
        let Some(activity) = self.runtime.accepted_activity else {
            return;
        };
        if self
            .runtime
            .net_node
            .as_ref()
            .is_some_and(|node| node.node_id() == activity.changed_by)
        {
            return;
        }
        let Some(peer_state) = self.runtime.registry.peers.get_mut(&activity.changed_by) else {
            return;
        };
        let (Some(remote_pose), Some(remote_to_local)) =
            (peer_state.remote_activity_pose, peer_state.remote_to_local)
        else {
            return;
        };
        let local_pose = Self::transform_pose(&remote_to_local, remote_pose);
        if peer_state
            .last_emitted_local_activity_pose
            .is_some_and(|previous| Self::poses_match(previous, local_pose))
        {
            return;
        }
        peer_state.last_emitted_local_activity_pose = Some(local_pose);
        cx.widget_action(self.widget_uid(), XrPeerSyncAction::ActivityPoseReset(local_pose));
    }

    pub(super) fn remote_state_is_recent(peer_state: &RemotePeerState, now: f64) -> bool {
        now - peer_state.last_state_received_at <= Self::SYNC_MATCH_RECEIVE_WINDOW_SECONDS
    }

    pub(super) fn recent_remote_sync_anchor(
        peer_state: &RemotePeerState,
        now: f64,
    ) -> Option<XrSyncAnchor> {
        if !Self::remote_state_is_recent(peer_state, now) {
            return None;
        }
        peer_state
            .recent_sync_anchors
            .back()
            .filter(|recent_sync| {
                now - recent_sync.last_seen_at_local_time <= Self::SYNC_MATCH_RECEIVE_WINDOW_SECONDS
            })
            .map(|recent_sync| recent_sync.sync)
    }

    pub(super) fn recent_remote_fist_hold_anchor(
        peer_state: &RemotePeerState,
        now: f64,
    ) -> Option<XrAnchor> {
        if !Self::remote_state_is_recent(peer_state, now) {
            return None;
        }
        peer_state.last_fist_hold_anchor.filter(|_| {
            peer_state
                .last_fist_hold_seen_at
                .is_some_and(|seen_at| now - seen_at <= Self::FIST_ACK_STICKY_WINDOW_SECONDS)
        })
    }

    pub(super) fn manual_touch_sync_status_text(&self) -> String {
        let local_sync_pending = self.runtime.local.active_sync_anchor().is_some();
        let local_fists_ready = self.runtime.local.fist_hold_anchor.is_some();
        let matched_sample_count = self.runtime.local.matched_sync_sample_count();
        let Some((_, peer_state)) = self.runtime.registry.preferred_peer() else {
            return if local_sync_pending {
                "Touch sync: box sample armed".to_string()
            } else if local_fists_ready {
                "Touch sync: hold both hands open".to_string()
            } else if matched_sample_count != 0 {
                format!("Touch sync: stored {matched_sample_count} samples")
            } else {
                "Touch sync: idle".to_string()
            };
        };
        let now = self.runtime.local.state_time;
        let remote_sync_pending = Self::recent_remote_sync_anchor(&peer_state, now).is_some();
        let remote_fists_ready = Self::recent_remote_fist_hold_anchor(&peer_state, now).is_some();

        if peer_state.transform_source == RemoteTransformSource::Anchor {
            return if self.runtime.recent_anchor_confirmation.is_some() {
                "Touch sync: anchor set".to_string()
            } else if peer_state
                .latest_state
                .as_ref()
                .is_some_and(|state| state.state.anchor.is_some())
            {
                "Touch sync: using persistent anchors".to_string()
            } else {
                "Touch sync: anchor aligned".to_string()
            };
        }
        if local_sync_pending || remote_sync_pending {
            return format!("Touch sync: box sampling {matched_sample_count}");
        }
        if local_fists_ready && remote_fists_ready {
            return "Touch sync: both players ready".to_string();
        }
        if remote_fists_ready {
            return "Touch sync: remote open hands ready".to_string();
        }
        if local_fists_ready {
            return "Touch sync: hold both hands open".to_string();
        }
        "Touch sync: idle".to_string()
    }

    fn manual_peer_scene_text(&self) -> String {
        let Some((peer_id, peer_state)) = self.runtime.registry.preferred_peer() else {
            return "PeerScene: waiting for peer".to_string();
        };
        let state_text = if peer_state.latest_state.is_some() {
            "yes"
        } else {
            "no"
        };
        let anchor_text = peer_state
            .latest_state
            .as_ref()
            .and_then(|state| state.state.anchor)
            .map(|_| "yes")
            .unwrap_or("no");
        let sync_text = Self::recent_remote_sync_anchor(&peer_state, self.runtime.local.state_time)
            .map(|sync| format!("yes {} {}", sync_extrema_label(sync.extrema), sync.id))
            .unwrap_or_else(|| "no".to_string());
        format!(
            "PeerScene {:08x}: state {} | anchor {} | sync {} | pose {}",
            peer_id.0,
            state_text,
            anchor_text,
            sync_text,
            transform_source_label(peer_state.transform_source),
        )
    }

    fn manual_alignment_state_text(&self) -> String {
        let local_anchor_text = if self.runtime.local.effective_anchor().is_some() {
            "yes"
        } else {
            "no"
        };
        let local_sync_text = self.local_sync_status_text();
        let Some((peer_id, peer_state)) = self.runtime.registry.preferred_peer() else {
            return format!(
                "AlignState: local anchor {} | sync {} | waiting for peer",
                local_anchor_text, local_sync_text
            );
        };
        let remote_anchor_text = peer_state
            .latest_state
            .as_ref()
            .and_then(|state| state.state.anchor)
            .map(|_| "yes")
            .unwrap_or("no");
        let remote_sync_text =
            Self::recent_remote_sync_anchor(&peer_state, self.runtime.local.state_time)
                .map(|sync| format!("armed {} {}", sync_extrema_label(sync.extrema), sync.id))
                .unwrap_or_else(|| "idle".to_string());
        format!(
            "AlignState {:08x}: local anchor {} | local sync {} | remote anchor {} | remote sync {} | samples {} | pose {}",
            peer_id.0,
            local_anchor_text,
            local_sync_text,
            remote_anchor_text,
            remote_sync_text,
            self.runtime.local.matched_sync_sample_count(),
            transform_source_label(peer_state.transform_source),
        )
    }

    fn manual_alignment_debug_text(&self) -> String {
        let local_sync = self.runtime.local.active_sync_anchor();
        let local_fist_hold = self.runtime.local.fist_hold_anchor;
        let Some((peer_id, peer_state)) = self.runtime.registry.preferred_peer() else {
            return match (local_sync, local_fist_hold) {
                (Some(sync), _) => format!(
                    "AlignDbg: box sample {} {} armed | waiting for peer sample",
                    sync_extrema_label(sync.extrema),
                    sync.id
                ),
                (None, Some(_)) => "AlignDbg: local double-open-hands ready".to_string(),
                _ => "AlignDbg: manual sync idle".to_string(),
            };
        };
        let now = self.runtime.local.state_time;
        let remote_sync = Self::recent_remote_sync_anchor(&peer_state, now);
        let remote_fist_hold = Self::recent_remote_fist_hold_anchor(&peer_state, now);
        if peer_state.transform_source == RemoteTransformSource::Anchor {
            if self.runtime.recent_anchor_confirmation.is_some()
                || self.runtime.local.anchor_override.is_some()
            {
                if let Some(sync) = local_sync {
                    return format!(
                        "AlignDbg {:08x}: matched {} {} -> averaged anchor applied ({} samples)",
                        peer_id.0,
                        sync_extrema_label(sync.extrema),
                        sync.id,
                        self.runtime.local.matched_sync_sample_count()
                    );
                }
                return format!(
                    "AlignDbg {:08x}: averaged anchor active ({} samples)",
                    peer_id.0,
                    self.runtime.local.matched_sync_sample_count()
                );
            }
            if peer_state
                .latest_state
                .as_ref()
                .is_some_and(|state| state.state.anchor.is_some())
            {
                return format!("AlignDbg {:08x}: using saved anchors", peer_id.0);
            }
            if let Some(sync) = local_sync {
                return format!(
                    "AlignDbg {:08x}: matched {} {} -> persistent anchor requested",
                    peer_id.0,
                    sync_extrema_label(sync.extrema),
                    sync.id
                );
            }
            return format!("AlignDbg {:08x}: anchor transform active", peer_id.0);
        }
        if let Some(sync) = remote_sync {
            return format!(
                "AlignDbg {:08x}: remote {} sample {} seen | match your box extrema in 100ms",
                peer_id.0,
                sync_extrema_label(sync.extrema),
                sync.id
            );
        }
        if remote_fist_hold.is_some() {
            return format!(
                "AlignDbg {:08x}: remote double-open-hands ready | start box motion",
                peer_id.0
            );
        }
        if let Some(sync) = local_sync {
            return format!(
                "AlignDbg {:08x}: local {} sample {} armed | waiting for matching remote extrema",
                peer_id.0,
                sync_extrema_label(sync.extrema),
                sync.id
            );
        }
        if local_fist_hold.is_some() {
            return format!("AlignDbg {:08x}: local double-open-hands ready", peer_id.0);
        }
        format!("AlignDbg {:08x}: manual sync idle", peer_id.0)
    }

    fn local_descriptor_debug_text(&self) -> String {
        let Some(descriptor) = self
            .runtime
            .local
            .descriptor
            .as_ref()
            .map(|frame| &frame.descriptor)
        else {
            return match self.runtime.local.scene_state() {
                LocalSceneState::Missing => "AlignDbg: waiting for local heightmap".to_string(),
                LocalSceneState::PublishPending => {
                    "AlignDbg: local heightmap ready | publish pending".to_string()
                }
                LocalSceneState::Ready => "AlignDbg: local map missing".to_string(),
            };
        };
        let map_status = descriptor_height_map_status(descriptor);
        match self.runtime.local.scene_state() {
            LocalSceneState::Missing => "AlignDbg: waiting for local heightmap".to_string(),
            LocalSceneState::PublishPending => format!(
                "AlignDbg: local map {} | signal {} | publish pending",
                map_status,
                self.runtime.local.contour_sample_count(),
            ),
            LocalSceneState::Ready => format!(
                "AlignDbg: local map {} | signal {}",
                map_status,
                self.runtime.local.contour_sample_count(),
            ),
        }
    }

    pub(super) fn refresh_status(&mut self) {
        if !self.enabled {
            self.diagnostics.set_disabled();
            return;
        }
        if self.runtime.net_node.is_none() {
            if self.diagnostics.status.is_empty() {
                self.diagnostics.status = "AlignSync: network unavailable".to_string();
            }
            if self.diagnostics.network_status.is_empty() {
                self.diagnostics.network_status = "Network: unavailable".to_string();
            }
            if self.diagnostics.peer_scene_status.is_empty() {
                self.diagnostics.peer_scene_status = "PeerMap: network unavailable".to_string();
            }
            return;
        }

        let peer_count = self.runtime.registry.len();
        let visible_count = self
            .runtime
            .registry
            .peers
            .values()
            .filter(|peer| peer.latest_state.is_some())
            .count();
        let descriptor_count = self
            .runtime
            .registry
            .peers
            .values()
            .filter(|peer| peer.has_descriptor)
            .count();
        let aligned_count = self
            .runtime
            .registry
            .peers
            .values()
            .filter(|peer| peer.latest_state.is_some() && peer.remote_to_local.is_some())
            .count();
        let anchor_aligned_count = self
            .runtime
            .registry
            .peers
            .values()
            .filter(|peer| {
                peer.latest_state.is_some()
                    && peer.transform_source == RemoteTransformSource::Anchor
            })
            .count();
        let local_scene_state = self.runtime.local.scene_state();

        if !self.auto_alignment_enabled {
            self.diagnostics.status = if peer_count == 0 {
                "Peers: scanning LAN for clients".to_string()
            } else {
                format!(
                    "Peers: {peer_count} seen | {visible_count} state | {anchor_aligned_count} anchor-aligned"
                )
            };
            let last_event = self.runtime.metrics.last_event_label();
            self.diagnostics.network_status = format!(
                "Network: tx s{} d{} a{} b{} | rx j{} l{} s{} d{} a{} b{} | peers {} vis {} anchor {} | local anchor {} sync {} | objects {} | last {}",
                self.runtime.metrics.tx_state_count,
                self.runtime.metrics.tx_descriptor_count,
                self.runtime.metrics.tx_activity_count,
                self.runtime.metrics.tx_body_spawn_count,
                self.runtime.metrics.rx_join_count,
                self.runtime.metrics.rx_leave_count,
                self.runtime.metrics.rx_state_count,
                self.runtime.metrics.rx_descriptor_count,
                self.runtime.metrics.rx_activity_count,
                self.runtime.metrics.rx_body_spawn_count,
                peer_count,
                visible_count,
                anchor_aligned_count,
                if self.runtime.local.effective_anchor().is_some() {
                    "yes"
                } else {
                    "no"
                },
                self.local_sync_status_text(),
                self.runtime.shared_objects.len(),
                last_event,
            );
            self.diagnostics.peer_scene_status = self.manual_peer_scene_text();
            self.diagnostics.alignment_state_status = self.manual_alignment_state_text();
            self.diagnostics.alignment_debug_status = self.manual_alignment_debug_text();
            return;
        }

        self.diagnostics.status = if peer_count == 0 {
            "AlignSync: waiting for peer heightmap".to_string()
        } else if local_scene_state == LocalSceneState::Ready {
            format!(
                "AlignSync: peers {peer_count} | visible {visible_count} | remote maps {descriptor_count} | solved {aligned_count}"
            )
        } else if local_scene_state == LocalSceneState::PublishPending {
            format!(
                "AlignSync: peers {peer_count} | local map signal {} ready | publish pending",
                self.runtime.local.contour_sample_count()
            )
        } else {
            format!("AlignSync: peers {peer_count} | waiting for local heightmap")
        };

        let last_event = self.runtime.metrics.last_event_label();
        self.diagnostics.network_status = format!(
            "Network: tx state {} map {} activity {} spawn {} | rx join {} leave {} state {} map {} activity {} spawn {} | peers {} vis {} maps {} solved {} | local map {} signal {} | objects {} | last {}",
            self.runtime.metrics.tx_state_count,
            self.runtime.metrics.tx_descriptor_count,
            self.runtime.metrics.tx_activity_count,
            self.runtime.metrics.tx_body_spawn_count,
            self.runtime.metrics.rx_join_count,
            self.runtime.metrics.rx_leave_count,
            self.runtime.metrics.rx_state_count,
            self.runtime.metrics.rx_descriptor_count,
            self.runtime.metrics.rx_activity_count,
            self.runtime.metrics.rx_body_spawn_count,
            peer_count,
            visible_count,
            descriptor_count,
            aligned_count,
            match local_scene_state {
                LocalSceneState::Ready => "yes",
                LocalSceneState::PublishPending => "pending",
                LocalSceneState::Missing => "no",
            },
            self.runtime.local.contour_sample_count(),
            self.runtime.shared_objects.len(),
            last_event,
        );
        self.diagnostics.peer_scene_status = make_peer_scene_debug_text(
            local_scene_state == LocalSceneState::Ready,
            &self.runtime.registry.peers,
        );
        self.diagnostics.alignment_state_status = make_alignment_state_text(
            local_scene_state,
            self.runtime.local.descriptor_version,
            &self.runtime.registry.peers,
        );
        let has_alignment_diagnostic = self
            .runtime
            .registry
            .peers
            .values()
            .any(|peer| peer.last_solve_diagnostic.is_some());
        let has_alignment_worker_progress = self
            .runtime
            .registry
            .peers
            .values()
            .any(|peer| peer.worker_progress.is_some());
        if local_scene_state != LocalSceneState::Ready
            || (!has_alignment_diagnostic && !has_alignment_worker_progress)
        {
            let local_descriptor_text = self.local_descriptor_debug_text();
            self.diagnostics.alignment_debug_status = if local_scene_state == LocalSceneState::Ready
            {
                make_pending_alignment_debug_text(
                    &local_descriptor_text,
                    &self.runtime.registry.peers,
                )
            } else {
                local_descriptor_text
            };
        }
    }

    pub(super) fn state_arm_corridor_ready(state: &XrState) -> bool {
        Self::arm_corridor_ready(state.head_pose, &state.left_hand, &state.right_hand)
    }

    pub(super) fn arm_corridor_ready(
        head_pose: Pose,
        left_hand: &XrHand,
        right_hand: &XrHand,
    ) -> bool {
        let left_pose = left_hand.tracking_pose();
        let right_pose = right_hand.tracking_pose();
        let (Some(left_pose), Some(right_pose)) = (left_pose, right_pose) else {
            return false;
        };
        let Some(metrics) = arm_pair_metrics(head_pose, left_pose.position, right_pose.position)
        else {
            return false;
        };
        if metrics.left_lateral >= metrics.right_lateral
            || metrics.hand_gap < Self::FIST_ACK_MIN_HAND_GAP_METERS
            || metrics.hand_gap > Self::FIST_ACK_MAX_HAND_GAP_METERS
            || metrics.left_forward < Self::FIST_ACK_MIN_CHEST_DISTANCE_METERS
            || metrics.left_forward > Self::FIST_ACK_MAX_CHEST_DISTANCE_METERS
            || metrics.right_forward < Self::FIST_ACK_MIN_CHEST_DISTANCE_METERS
            || metrics.right_forward > Self::FIST_ACK_MAX_CHEST_DISTANCE_METERS
            || metrics.left_elevation_degrees > Self::FIST_ACK_MAX_ARM_ELEVATION_DEGREES
            || metrics.right_elevation_degrees > Self::FIST_ACK_MAX_ARM_ELEVATION_DEGREES
            || (left_pose.position.y - right_pose.position.y).abs()
                > Self::FIST_ACK_MAX_VERTICAL_DELTA_METERS * 2.0
            || (metrics.left_forward - metrics.right_forward).abs()
                > Self::FIST_ACK_MAX_DEPTH_DELTA_METERS * 2.0
        {
            return false;
        }
        (Self::FIST_ACK_MIN_CHEST_DISTANCE_METERS..=Self::FIST_ACK_MAX_CHEST_DISTANCE_METERS)
            .contains(&metrics.average_forward_distance)
    }

    pub(super) fn state_fist_ack_anchor(state: &XrState) -> Option<XrAnchor> {
        Self::fist_ack_anchor(state.head_pose, &state.left_hand, &state.right_hand)
    }

    fn fist_ack_anchor(head_pose: Pose, left_hand: &XrHand, right_hand: &XrHand) -> Option<XrAnchor> {
        let forward = flat_head_forward(head_pose.orientation);
        let left_point = hand_closed_fist_contact_point_geometry_only(left_hand, forward, true)?;
        let right_point = hand_closed_fist_contact_point_geometry_only(right_hand, forward, false)?;
        let metrics = arm_pair_metrics(head_pose, left_point, right_point)?;
        if metrics.left_lateral >= metrics.right_lateral
            || metrics.hand_gap < Self::FIST_ACK_MIN_HAND_GAP_METERS
            || metrics.hand_gap > Self::FIST_ACK_MAX_HAND_GAP_METERS
            || metrics.left_forward < Self::FIST_ACK_MIN_CHEST_DISTANCE_METERS
            || metrics.left_forward > Self::FIST_ACK_MAX_CHEST_DISTANCE_METERS
            || metrics.right_forward < Self::FIST_ACK_MIN_CHEST_DISTANCE_METERS
            || metrics.right_forward > Self::FIST_ACK_MAX_CHEST_DISTANCE_METERS
            || metrics.left_elevation_degrees > Self::FIST_ACK_MAX_ARM_ELEVATION_DEGREES
            || metrics.right_elevation_degrees > Self::FIST_ACK_MAX_ARM_ELEVATION_DEGREES
            || (left_point.y - right_point.y).abs() > Self::FIST_ACK_MAX_VERTICAL_DELTA_METERS * 2.0
            || (metrics.left_forward - metrics.right_forward).abs()
                > Self::FIST_ACK_MAX_DEPTH_DELTA_METERS * 2.0
        {
            return None;
        }
        if !(Self::FIST_ACK_MIN_CHEST_DISTANCE_METERS..=Self::FIST_ACK_MAX_CHEST_DISTANCE_METERS)
            .contains(&metrics.average_forward_distance)
        {
            return None;
        }
        Some(XrAnchor {
            left: left_point,
            right: right_point,
        })
    }

    fn anchors_match(left: XrAnchor, right: XrAnchor) -> bool {
        (left.left - right.left).length() <= 0.025 && (left.right - right.right).length() <= 0.025
    }

    fn refresh_peer_transforms(&mut self, cx: &mut Cx) {
        if let (Some(local_anchor), Some(override_anchor)) = (
            self.runtime.local.anchor,
            self.runtime.local.anchor_override,
        ) {
            if Self::anchors_match(local_anchor, override_anchor) {
                self.runtime.local.anchor_override = None;
            }
        }

        let now = self.runtime.local.state_time;
        let changed = {
            let (registry, local) = (&mut self.runtime.registry, &mut self.runtime.local);
            registry.refresh_transforms(
                cx,
                local,
                &mut self.runtime.recent_anchor_confirmation,
                now,
            )
        };

        if changed {
            self.redraw(cx);
        }
        self.emit_authoritative_remote_activity_pose_if_changed(cx);
    }
}
