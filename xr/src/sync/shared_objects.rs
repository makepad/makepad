use super::*;

impl XrPeerSync {
    pub fn send_local_body_spawn(&mut self, spawn: XrBodySpawn) -> Option<XrBodySpawn> {
        if !self.enabled {
            return None;
        }
        self.ensure_net_node();
        let activity_id = self.runtime.shared_objects.activity_id()?;
        let sent_at = if self.runtime.local.state_time != 0.0 {
            self.runtime.local.state_time
        } else {
            Cx::time_now()
        };
        let (allocation, reused_remote) = self
            .runtime
            .shared_objects
            .prepare_local_spawn_allocation(
                activity_id,
                spawn.widget_uid,
                sent_at,
                spawn.pose,
                spawn.linvel,
                spawn.angvel,
            )?;
        let authority = self.runtime.shared_objects.local_peer_id()?;
        let control = XrNetSharedObjectControl::XrSpawnObject {
            object_id: allocation.shared_object_id,
            epoch: allocation.epoch,
            authority,
            fidelity: allocation.fidelity,
            shape: XrSharedObjectShape::ActivitySpawnable {
                activity_id,
                spawnable_id: allocation.spawnable_object_id,
            },
            pose: spawn.pose,
            linvel: spawn.linvel,
            angvel: spawn.angvel,
        };
        let net_node = self.runtime.net_node.as_mut()?;
        net_node.send_shared_object_control(control);
        if reused_remote {
            net_node.send_shared_object_control(XrNetSharedObjectControl::XrResetObject {
                object_id: allocation.shared_object_id,
                epoch: allocation.epoch,
                pose: spawn.pose,
                linvel: spawn.linvel,
                angvel: spawn.angvel,
            });
        }
        self.runtime
            .metrics
            .record_body_spawn_tx(allocation.shared_object_id.0);
        Some(XrBodySpawn {
            widget_uid: allocation.widget_uid,
            shadow: spawn.shadow,
            mode: spawn.mode,
            pose: spawn.pose,
            linvel: spawn.linvel,
            angvel: spawn.angvel,
        })
    }

    pub fn send_activity_pose_reset(&mut self, pose: Pose) -> bool {
        if !self.enabled {
            return false;
        }
        self.ensure_net_node();
        let Some(activity_id) = self.runtime.shared_objects.activity_id() else {
            return false;
        };
        let Some(net_node) = self.runtime.net_node.as_mut() else {
            return false;
        };
        net_node.send_shared_object_control(XrNetSharedObjectControl::XrResetActivityPose {
            activity_id,
            pose,
        });
        true
    }

    fn local_shared_object_spawn_control(
        activity_id: XrActivityId,
        snapshot: XrLocalSharedObjectSnapshot,
        body: &XrRuntimeBodyState,
    ) -> XrNetSharedObjectControl {
        XrNetSharedObjectControl::XrSpawnObject {
            object_id: snapshot.object_id,
            epoch: snapshot.epoch,
            authority: snapshot.authority,
            fidelity: snapshot.fidelity,
            shape: XrSharedObjectShape::ActivitySpawnable {
                activity_id,
                spawnable_id: snapshot.spawnable_object_id,
            },
            pose: body.pose,
            linvel: body.linvel,
            angvel: body.angvel,
        }
    }

    fn bootstrap_owner_peer_id(
        &self,
        spawnable_object_id: XrSpawnableObjectId,
    ) -> Option<XrNetPeerId> {
        let local_node_id = self.runtime.net_node.as_ref()?.node_id();
        let mut peer_ids = self.runtime.registry.peer_ids();
        if peer_ids.is_empty() {
            return None;
        }
        peer_ids.push(local_node_id);
        peer_ids.sort_by_key(|peer_id| peer_id.0);
        peer_ids.dedup_by_key(|peer_id| peer_id.0);
        let owner_index = (Self::bootstrap_hash_u64(
            spawnable_object_id.0,
            Self::SHARED_OBJECT_BOOTSTRAP_OWNER_TAG,
        ) as usize)
            % peer_ids.len();
        peer_ids.get(owner_index).copied()
    }

    fn bootstrap_hash_u64(hash: u64, value: u64) -> u64 {
        (hash ^ value).wrapping_mul(0x00000100000001b3)
    }

    fn bootstrap_local_shared_scene_objects(
        &mut self,
        runtime_bodies: &HashMap<WidgetUid, XrRuntimeBodyState>,
    ) -> Vec<XrNetSharedObjectControl> {
        if self.runtime.registry.len() == 0 {
            return Vec::new();
        }
        let Some(activity_id) = self.runtime.shared_objects.activity_id() else {
            return Vec::new();
        };
        let Some(local_node_id) = self.runtime.net_node.as_ref().map(|node| node.node_id()) else {
            return Vec::new();
        };
        let mut controls = Vec::new();
        for (spawnable_object_id, widget_uid) in
            self.runtime.shared_objects.bootstrap_shared_candidates()
        {
            if self
                .runtime
                .shared_objects
                .resolve_local_shared_object_for_widget(widget_uid)
                .is_some()
                || self
                    .runtime
                    .shared_objects
                    .resolve_remote_shared_object_for_widget(widget_uid)
                    .is_some()
            {
                continue;
            }
            if self.bootstrap_owner_peer_id(spawnable_object_id) != Some(local_node_id) {
                continue;
            }
            let Some(body) = runtime_bodies.get(&widget_uid) else {
                continue;
            };
            let Some((allocation, is_new)) = self
                .runtime
                .shared_objects
                .ensure_local_shared_object(activity_id, widget_uid)
            else {
                continue;
            };
            if !is_new {
                continue;
            }
            controls.push(XrNetSharedObjectControl::XrSpawnObject {
                object_id: allocation.shared_object_id,
                epoch: allocation.epoch,
                authority: self
                    .runtime
                    .shared_objects
                    .local_peer_id()
                    .unwrap_or_default(),
                fidelity: allocation.fidelity,
                shape: XrSharedObjectShape::ActivitySpawnable {
                    activity_id,
                    spawnable_id: allocation.spawnable_object_id,
                },
                pose: body.pose,
                linvel: body.linvel,
                angvel: body.angvel,
            });
        }
        controls
    }

    fn reannounce_local_shared_objects(
        &mut self,
        runtime_bodies: &HashMap<WidgetUid, XrRuntimeBodyState>,
    ) -> Vec<XrNetSharedObjectControl> {
        if !self.runtime.local_shared_object_reannounce_needed {
            return Vec::new();
        }
        let Some(activity_id) = self.runtime.shared_objects.activity_id() else {
            return Vec::new();
        };
        let controls = self
            .runtime
            .shared_objects
            .local_shared_object_snapshots()
            .into_iter()
            .filter_map(|snapshot| {
                let body = runtime_bodies.get(&snapshot.widget_uid)?;
                Some(Self::local_shared_object_spawn_control(
                    activity_id,
                    snapshot,
                    body,
                ))
            })
            .collect::<Vec<_>>();
        self.runtime.local_shared_object_reannounce_needed = false;
        controls
    }

    pub fn reset_local_shared_bootstrap_objects(
        &mut self,
        runtime_bodies: &HashMap<WidgetUid, XrRuntimeBodyState>,
    ) -> usize {
        if !self.enabled {
            return 0;
        }
        self.ensure_net_node();
        let Some(activity_id) = self.runtime.shared_objects.activity_id() else {
            return 0;
        };
        let Some(local_node_id) = self.runtime.net_node.as_ref().map(|node| node.node_id()) else {
            return 0;
        };
        let mut controls = Vec::new();
        for (spawnable_object_id, widget_uid) in
            self.runtime.shared_objects.bootstrap_shared_candidates()
        {
            let Some(body) = runtime_bodies.get(&widget_uid) else {
                continue;
            };
            let has_local_object = self
                .runtime
                .shared_objects
                .resolve_local_shared_object_for_widget(widget_uid)
                .is_some();
            if self
                .runtime
                .shared_objects
                .resolve_remote_shared_object_for_widget(widget_uid)
                .is_some()
            {
                continue;
            }
            if !has_local_object
                && self.bootstrap_owner_peer_id(spawnable_object_id) != Some(local_node_id)
            {
                continue;
            }
            let Some(allocation) = self.runtime.shared_objects.force_local_shared_object_reset(
                activity_id,
                widget_uid,
                self.current_local_time(),
                body.pose,
                body.linvel,
                body.angvel,
            ) else {
                continue;
            };
            let Some(snapshot) = self
                .runtime
                .shared_objects
                .local_shared_object_snapshot(allocation.shared_object_id)
            else {
                continue;
            };
            controls.push(Self::local_shared_object_spawn_control(
                activity_id,
                snapshot,
                body,
            ));
            controls.push(XrNetSharedObjectControl::XrResetObject {
                object_id: allocation.shared_object_id,
                epoch: allocation.epoch,
                pose: body.pose,
                linvel: body.linvel,
                angvel: body.angvel,
            });
        }
        let count = controls.len();
        if let Some(net_node) = self.runtime.net_node.as_mut() {
            for control in controls {
                if let Some(object_id) = Self::shared_object_control_object_id(&control) {
                    if matches!(
                        control,
                        XrNetSharedObjectControl::XrSpawnObject { .. }
                            | XrNetSharedObjectControl::XrResetObject { .. }
                    ) {
                        self.runtime.metrics.record_body_spawn_tx(object_id.0);
                    }
                }
                net_node.send_shared_object_control(control);
            }
        }
        count
    }

    pub fn publish_local_shared_object_states(
        &mut self,
        cx: &mut Cx,
        runtime_bodies: &HashMap<WidgetUid, XrRuntimeBodyState>,
    ) -> usize {
        if !self.enabled {
            return 0;
        }
        self.ensure_net_node();
        let authority = if let Some(authority) = self.runtime.shared_objects.local_peer_id() {
            authority
        } else {
            return 0;
        };
        let sent_at = if self.runtime.local.state_time != 0.0 {
            self.runtime.local.state_time
        } else {
            Cx::time_now()
        };
        let physics_tick = self.runtime.next_shared_object_physics_tick;
        self.runtime.next_shared_object_physics_tick =
            self.runtime.next_shared_object_physics_tick.wrapping_add(1);
        let mut action_count = self.service_scheduled_authority_transfers(cx, physics_tick);
        action_count += self.sync_remote_shadow_bodies(cx);
        let mut outgoing_controls = self.reannounce_local_shared_objects(runtime_bodies);
        outgoing_controls.extend(self.bootstrap_local_shared_scene_objects(runtime_bodies));
        outgoing_controls.extend(self.queue_remote_interaction_controls(runtime_bodies));
        if let Some(activity_id) = self.runtime.shared_objects.activity_id() {
            for (&widget_uid, body) in runtime_bodies {
                if body.held_by.is_none() {
                    continue;
                }
                if self
                    .runtime
                    .shared_objects
                    .resolve_remote_shared_object_for_widget(widget_uid)
                    .is_some()
                {
                    continue;
                }
                let Some((allocation, is_new)) = self
                    .runtime
                    .shared_objects
                    .ensure_local_shared_object(activity_id, widget_uid)
                else {
                    continue;
                };
                if is_new {
                    outgoing_controls.push(XrNetSharedObjectControl::XrSpawnObject {
                        object_id: allocation.shared_object_id,
                        epoch: allocation.epoch,
                        authority,
                        fidelity: allocation.fidelity,
                        shape: XrSharedObjectShape::ActivitySpawnable {
                            activity_id,
                            spawnable_id: allocation.spawnable_object_id,
                        },
                        pose: body.pose,
                        linvel: body.linvel,
                        angvel: body.angvel,
                    });
                }
            }
        }
        let despawns = self
            .runtime
            .shared_objects
            .prune_missing_local_shared_objects(runtime_bodies);
        let mut states = self.runtime.shared_objects.local_shared_object_states(
            runtime_bodies,
            sent_at,
            physics_tick,
            authority,
        );
        let count = action_count + outgoing_controls.len() + despawns.len() + states.len();
        for control in &outgoing_controls {
            if let Some(object_id) = Self::shared_object_control_object_id(control) {
                if matches!(control, XrNetSharedObjectControl::XrSpawnObject { .. }) {
                    self.runtime.metrics.record_body_spawn_tx(object_id.0);
                }
            }
        }
        if let Some(net_node) = self.runtime.net_node.as_mut() {
            for control in outgoing_controls {
                net_node.send_shared_object_control(control);
            }
            for (object_id, epoch, _) in despawns {
                net_node.send_shared_object_control(XrNetSharedObjectControl::XrDespawnObject {
                    object_id,
                    epoch,
                });
            }
            if !states.is_empty() {
                net_node.assign_shared_object_state_seqs(&mut states);
                self.runtime
                    .shared_objects
                    .note_published_local_shared_object_states(&states);
                for state in &states {
                    self.runtime
                        .metrics
                        .record_shared_object_state_tx(state.object_id, state.seq);
                }
                net_node.send_prepared_shared_object_states(states);
            }
        }
        count
    }

    pub fn flush_pending_shared_object_controls(&mut self, cx: &mut Cx) -> usize {
        if self.runtime.pending_shared_object_controls.is_empty() {
            return 0;
        }
        let pending = std::mem::take(&mut self.runtime.pending_shared_object_controls);
        let mut remaining = Vec::new();
        let mut applied = 0usize;
        for (peer, control) in pending {
            if self.apply_shared_object_control(cx, peer, &control) {
                applied += 1;
            } else {
                remaining.push((peer, control));
            }
        }
        self.runtime.pending_shared_object_controls = remaining;
        applied
    }

    fn shared_object_request_id(&mut self) -> u32 {
        let request_id = self.runtime.next_shared_object_request_id;
        self.runtime.next_shared_object_request_id =
            self.runtime.next_shared_object_request_id.wrapping_add(1);
        request_id
    }

    pub(super) fn peer_id_for_authority(&self, authority: XrNetPeerId) -> Option<XrNetPeerId> {
        self.runtime
            .registry
            .peers
            .contains_key(&authority)
            .then_some(authority)
    }

    pub(super) fn peer_time_to_local_time(
        &self,
        peer_id: XrNetPeerId,
        remote_time: f64,
    ) -> Option<f64> {
        let peer_state = self.runtime.registry.peers.get(&peer_id)?;
        let clock_offset = peer_state.clock_offset_seconds?;
        Some(remote_time - clock_offset)
    }

    pub(super) fn normalize_incoming_shared_object_state(
        &self,
        peer_id: XrNetPeerId,
        mut state: XrNetSharedObjectState,
        received_at_local_time: f64,
    ) -> XrNetSharedObjectState {
        if state.sent_at <= 0.0 {
            return state;
        }
        let translated_sent_at = self
            .peer_time_to_local_time(peer_id, state.sent_at)
            .unwrap_or(received_at_local_time);
        state.sent_at =
            Self::clamp_remote_shared_object_local_time(translated_sent_at, received_at_local_time);
        state
    }

    pub(super) fn clamp_remote_shared_object_local_time(
        translated_sent_at: f64,
        received_at_local_time: f64,
    ) -> f64 {
        translated_sent_at.min(received_at_local_time)
    }

    fn predict_pose(pose: Pose, linvel: Vec3f, angvel: Vec3f, dt: f32) -> Pose {
        let position = pose.position + linvel * dt;
        let angular_speed = angvel.length();
        let orientation = if angular_speed > 1.0e-4 {
            let axis = angvel * (1.0 / angular_speed);
            Quat::multiply(
                &Quat::from_axis_angle(axis, angular_speed * dt),
                &pose.orientation,
            )
        } else {
            pose.orientation
        };
        Pose::new(orientation, position)
    }

    fn peer_grab_intent(&self, peer_id: XrNetPeerId, hand: XrSharedHand) -> bool {
        let Some(peer_state) = self.runtime.registry.peers.get(&peer_id) else {
            return false;
        };
        let Some(state) = peer_state.latest_state.as_ref().map(|frame| &frame.state) else {
            return false;
        };
        match hand {
            XrSharedHand::LeftHand => state.left_hand.grab_intent(),
            XrSharedHand::RightHand => state.right_hand.grab_intent(),
            XrSharedHand::LeftController => state.left_controller.grip >= 0.55,
            XrSharedHand::RightController => state.right_controller.grip >= 0.55,
            XrSharedHand::Unknown => false,
        }
    }

    pub(super) fn current_local_time(&self) -> f64 {
        if self.runtime.local.state_time != 0.0 {
            self.runtime.local.state_time
        } else {
            Cx::time_now()
        }
    }

    fn shared_object_state_local_time(
        state: XrNetSharedObjectState,
        fallback_local_time: f64,
    ) -> f64 {
        if state.sent_at > 0.0 {
            state.sent_at
        } else {
            fallback_local_time
        }
    }

    fn shared_object_source_peer_id(
        &self,
        snapshot: &XrRemoteSharedObjectSnapshot,
    ) -> Option<XrNetPeerId> {
        self.peer_id_for_authority(snapshot.state_source_authority)
    }

    pub(super) fn predict_remote_shadow_state_from_history(
        playback_time: f64,
        latest_state: XrNetSharedObjectState,
        history: &[XrNetSharedObjectState],
        fallback_local_time: f64,
    ) -> (XrSharedObjectMode, Pose, Vec3f, Vec3f) {
        for samples in history.windows(2) {
            let previous = samples[0];
            let next = samples[1];
            if previous.epoch != latest_state.epoch || next.epoch != latest_state.epoch {
                continue;
            }
            let previous_local_sent_at =
                Self::shared_object_state_local_time(previous, fallback_local_time);
            let next_local_sent_at =
                Self::shared_object_state_local_time(next, fallback_local_time);
            if playback_time < previous_local_sent_at || playback_time > next_local_sent_at {
                continue;
            }
            let interpolation = ((playback_time - previous_local_sent_at)
                / (next_local_sent_at - previous_local_sent_at).max(f64::EPSILON))
            .clamp(0.0, 1.0) as f32;
            return (
                next.mode,
                Pose::from_lerp(previous.pose, next.pose, interpolation),
                Vec3f::from_lerp(previous.linvel, next.linvel, interpolation),
                Vec3f::from_lerp(previous.angvel, next.angvel, interpolation),
            );
        }

        let local_sent_at = Self::shared_object_state_local_time(latest_state, fallback_local_time);
        let dt = (playback_time - local_sent_at).clamp(
            0.0,
            Self::SHARED_OBJECT_SHADOW_MAX_EXTRAPOLATION_SECONDS as f64,
        ) as f32;
        (
            latest_state.mode,
            Self::predict_pose(
                latest_state.pose,
                latest_state.linvel,
                latest_state.angvel,
                dt,
            ),
            latest_state.linvel,
            latest_state.angvel,
        )
    }

    fn predicted_remote_shadow_state(
        &self,
        snapshot: &XrRemoteSharedObjectSnapshot,
    ) -> Option<(XrNetPeerId, XrSharedObjectMode, Pose, Vec3f, Vec3f)> {
        let peer_id = self.shared_object_source_peer_id(snapshot)?;
        let latest_state = snapshot.latest_state?;
        let playback_time =
            self.current_local_time() - Self::SHARED_OBJECT_SHADOW_INTERPOLATION_DELAY_SECONDS;
        let history = self
            .runtime
            .shared_objects
            .remote_shared_object_history(snapshot.object_id);
        let (mode, pose, linvel, angvel) = Self::predict_remote_shadow_state_from_history(
            playback_time,
            latest_state,
            history.as_slice(),
            self.current_local_time(),
        );
        Some((peer_id, mode, pose, linvel, angvel))
    }

    fn shadow_pose_correction_needed(previous: Pose, next: Pose) -> bool {
        (next.position - previous.position).length()
            > Self::SHARED_OBJECT_SHADOW_REAPPLY_POSITION_EPSILON_METERS
            || previous.orientation.get_angle_with(next.orientation)
                > Self::SHARED_OBJECT_SHADOW_REAPPLY_ORIENTATION_EPSILON_DEGREES
    }

    fn shadow_velocity_correction_needed(previous: Vec3f, next: Vec3f, epsilon: f32) -> bool {
        (next - previous).length() > epsilon
    }

    fn note_applied_remote_shadow_state(
        &mut self,
        object_id: XrSharedObjectId,
        peer_id: XrNetPeerId,
        state_seq: Option<u32>,
        mode: XrSharedObjectMode,
        pose: Pose,
        linvel: Vec3f,
        angvel: Vec3f,
    ) {
        self.runtime.applied_remote_shadow_states.insert(
            object_id,
            XrAppliedRemoteShadowState {
                peer_id,
                applied_at_local_time: self.current_local_time(),
                state_seq,
                mode,
                pose,
                linvel,
                angvel,
            },
        );
    }

    fn clear_applied_remote_shadow_state(&mut self, object_id: XrSharedObjectId) {
        self.runtime.applied_remote_shadow_states.remove(&object_id);
    }

    pub(super) fn should_reapply_remote_shadow_state(
        previous: &XrAppliedRemoteShadowState,
        now: f64,
        peer_id: XrNetPeerId,
        _state_seq: Option<u32>,
        mode: XrSharedObjectMode,
        pose: Pose,
        linvel: Vec3f,
        angvel: Vec3f,
    ) -> bool {
        if previous.peer_id != peer_id || previous.mode != mode {
            return true;
        }
        let expected_pose = Self::predict_pose(
            previous.pose,
            previous.linvel,
            previous.angvel,
            (now - previous.applied_at_local_time).clamp(
                0.0,
                Self::SHARED_OBJECT_SHADOW_MAX_EXTRAPOLATION_SECONDS as f64,
            ) as f32,
        );
        Self::shadow_pose_correction_needed(expected_pose, pose)
            || Self::shadow_velocity_correction_needed(
                previous.linvel,
                linvel,
                Self::SHARED_OBJECT_SHADOW_REAPPLY_LINVEL_EPSILON_MPS,
            )
            || Self::shadow_velocity_correction_needed(
                previous.angvel,
                angvel,
                Self::SHARED_OBJECT_SHADOW_REAPPLY_ANGVEL_EPSILON_RADPS,
            )
    }

    fn emit_body_spawn_local_space(
        &mut self,
        cx: &mut Cx,
        widget_uid: WidgetUid,
        shadow: bool,
        mode: XrSharedObjectMode,
        pose: Pose,
        linvel: Vec3f,
        angvel: Vec3f,
    ) {
        cx.widget_action(
            self.widget_uid(),
            XrPeerSyncAction::BodySpawn(XrBodySpawn {
                widget_uid,
                shadow,
                mode,
                pose,
                linvel,
                angvel,
            }),
        );
    }

    fn emit_body_impulse(
        &mut self,
        cx: &mut Cx,
        widget_uid: WidgetUid,
        point: Vec3f,
        impulse: Vec3f,
    ) {
        cx.widget_action(
            self.widget_uid(),
            XrPeerSyncAction::BodyImpulse(XrBodyImpulse {
                widget_uid,
                point,
                impulse,
            }),
        );
    }

    fn emit_authority_space_body_spawn(
        &mut self,
        cx: &mut Cx,
        source_authority: XrNetPeerId,
        widget_uid: WidgetUid,
        shadow: bool,
        mode: XrSharedObjectMode,
        pose: Pose,
        linvel: Vec3f,
        angvel: Vec3f,
    ) {
        if self.runtime.shared_objects.local_peer_id() == Some(source_authority) {
            self.emit_body_spawn_local_space(cx, widget_uid, shadow, mode, pose, linvel, angvel);
        } else if let Some(peer_id) = self.peer_id_for_authority(source_authority) {
            self.emit_remote_body_spawn(
                cx, peer_id, widget_uid, shadow, mode, pose, linvel, angvel,
            );
        } else {
            self.emit_body_spawn_local_space(cx, widget_uid, shadow, mode, pose, linvel, angvel);
        }
    }

    fn service_scheduled_authority_transfers(&mut self, cx: &mut Cx, physics_tick: u32) -> usize {
        let transfers = self
            .runtime
            .shared_objects
            .apply_scheduled_authority_transfers(self.runtime.local.state_time, physics_tick);
        for transfer in &transfers {
            self.emit_authority_space_body_spawn(
                cx,
                transfer.source_authority,
                transfer.widget_uid,
                transfer.shadow,
                XrSharedObjectMode::Dynamic,
                transfer.pose,
                transfer.linvel,
                transfer.angvel,
            );
            if transfer.shadow {
                if let Some(peer_id) = self.peer_id_for_authority(transfer.source_authority) {
                    self.note_applied_remote_shadow_state(
                        transfer.object_id,
                        peer_id,
                        None,
                        XrSharedObjectMode::Dynamic,
                        transfer.pose,
                        transfer.linvel,
                        transfer.angvel,
                    );
                } else {
                    self.clear_applied_remote_shadow_state(transfer.object_id);
                }
            } else {
                self.clear_applied_remote_shadow_state(transfer.object_id);
            }
        }
        transfers.len()
    }

    fn sync_remote_shadow_bodies(&mut self, cx: &mut Cx) -> usize {
        let snapshots = self.runtime.shared_objects.remote_shared_object_snapshots();
        let now = self.current_local_time();
        let mut applied = 0usize;
        let mut live_object_ids = Vec::with_capacity(snapshots.len());
        for snapshot in snapshots {
            live_object_ids.push(snapshot.object_id);
            let latest_state = snapshot.latest_state;
            let Some((peer_id, mode, pose, linvel, angvel)) =
                self.predicted_remote_shadow_state(&snapshot)
            else {
                self.clear_applied_remote_shadow_state(snapshot.object_id);
                continue;
            };
            if self
                .runtime
                .applied_remote_shadow_states
                .get(&snapshot.object_id)
                .is_some_and(|previous| {
                    !Self::should_reapply_remote_shadow_state(
                        previous,
                        now,
                        peer_id,
                        latest_state.map(|state| state.seq),
                        mode,
                        pose,
                        linvel,
                        angvel,
                    )
                })
            {
                continue;
            }
            self.emit_remote_body_spawn(
                cx,
                peer_id,
                snapshot.widget_uid,
                true,
                mode,
                pose,
                linvel,
                angvel,
            );
            self.note_applied_remote_shadow_state(
                snapshot.object_id,
                peer_id,
                latest_state.map(|state| state.seq),
                mode,
                pose,
                linvel,
                angvel,
            );
            self.runtime.metrics.record_remote_shadow_apply(
                snapshot.object_id,
                latest_state.map(|state| state.seq),
            );
            applied += 1;
        }
        self.runtime
            .applied_remote_shadow_states
            .retain(|object_id, _| live_object_ids.contains(object_id));
        applied
    }

    fn queue_remote_interaction_controls(
        &mut self,
        runtime_bodies: &HashMap<WidgetUid, XrRuntimeBodyState>,
    ) -> Vec<XrNetSharedObjectControl> {
        let local_peer_id = if let Some(peer_id) = self.runtime.shared_objects.local_peer_id() {
            peer_id
        } else {
            return Vec::new();
        };
        let hands = self.local_shared_hands();
        if hands.is_empty() {
            return Vec::new();
        }
        let snapshots = self.runtime.shared_objects.remote_shared_object_snapshots();
        let mut controls = Vec::new();
        for snapshot in snapshots {
            let Some(latest_state) = snapshot.latest_state else {
                continue;
            };
            let Some((_, _, predicted_pose, predicted_linvel, _)) =
                self.predicted_remote_shadow_state(&snapshot)
            else {
                continue;
            };
            if runtime_bodies
                .get(&snapshot.widget_uid)
                .is_some_and(|body| body.held_by.is_some())
            {
                continue;
            }
            for hand in &hands {
                let distance = (predicted_pose.position - hand.pose.position).length();
                let relative_speed = (predicted_linvel - hand.linvel).length();
                if hand.gripping
                    && matches!(
                        latest_state.mode,
                        XrSharedObjectMode::ContactDominated { .. }
                    )
                    && distance <= Self::SHARED_OBJECT_TAKEOVER_DISTANCE_METERS
                    && relative_speed <= Self::SHARED_OBJECT_TAKEOVER_RELATIVE_SPEED_MAX
                    && self.runtime.shared_objects.can_send_takeover_request(
                        snapshot.object_id,
                        self.runtime.local.state_time,
                    )
                {
                    let request_id = self.shared_object_request_id();
                    self.runtime.shared_objects.note_takeover_request(
                        snapshot.object_id,
                        request_id,
                        self.runtime.local.state_time,
                    );
                    controls.push(XrNetSharedObjectControl::XrTakeoverRequest {
                        object_id: snapshot.object_id,
                        epoch: snapshot.epoch,
                        request_id,
                        based_on_seq: latest_state.seq,
                        based_on_tick: latest_state.physics_tick,
                        candidate_owner: local_peer_id,
                        hand: hand.shared_hand,
                        hand_pose: hand.pose,
                        hand_linvel: hand.linvel,
                    });
                    break;
                }
                if !hand.gripping
                    && matches!(
                        latest_state.mode,
                        XrSharedObjectMode::Dynamic | XrSharedObjectMode::Sleeping
                    )
                    && hand.linvel.length() >= Self::SHARED_OBJECT_IMPULSE_MIN_HAND_SPEED
                    && distance <= Self::SHARED_OBJECT_IMPULSE_DISTANCE_METERS
                    && self
                        .runtime
                        .shared_objects
                        .can_send_contact_impulse_request(
                            snapshot.object_id,
                            self.runtime.local.state_time,
                        )
                {
                    self.runtime.shared_objects.note_contact_impulse_request(
                        snapshot.object_id,
                        self.runtime.local.state_time,
                    );
                    controls.push(XrNetSharedObjectControl::XrContactImpulse {
                        object_id: snapshot.object_id,
                        epoch: snapshot.epoch,
                        based_on_seq: latest_state.seq,
                        based_on_tick: latest_state.physics_tick,
                        hand: hand.shared_hand,
                        hand_pose: hand.pose,
                        point: hand.pose.position,
                        impulse: hand.linvel * Self::SHARED_OBJECT_IMPULSE_SCALE,
                    });
                    break;
                }
            }
        }
        controls
    }

    pub(super) fn emit_remote_body_spawn(
        &mut self,
        cx: &mut Cx,
        peer_id: XrNetPeerId,
        widget_uid: WidgetUid,
        shadow: bool,
        mode: XrSharedObjectMode,
        pose: Pose,
        linvel: Vec3f,
        angvel: Vec3f,
    ) {
        let transform = self.peer_remote_to_local_transform(peer_id);
        cx.widget_action(
            self.widget_uid(),
            XrPeerSyncAction::BodySpawn(XrBodySpawn {
                widget_uid,
                shadow,
                mode,
                pose: Self::transform_pose(&transform, pose),
                linvel: Self::transform_direction(&transform, linvel),
                angvel: Self::transform_direction(&transform, angvel),
            }),
        );
    }

    pub(super) fn emit_remote_body_despawn(&mut self, cx: &mut Cx, widget_uid: WidgetUid) {
        cx.widget_action(self.widget_uid(), XrPeerSyncAction::BodyDespawn(widget_uid));
    }

    fn shared_object_control_object_id(
        control: &XrNetSharedObjectControl,
    ) -> Option<XrSharedObjectId> {
        match control {
            XrNetSharedObjectControl::XrSpawnObject { object_id, .. }
            | XrNetSharedObjectControl::XrDespawnObject { object_id, .. }
            | XrNetSharedObjectControl::XrTakeoverRequest { object_id, .. }
            | XrNetSharedObjectControl::XrTakeoverAccept { object_id, .. }
            | XrNetSharedObjectControl::XrTakeoverReject { object_id, .. }
            | XrNetSharedObjectControl::XrContactImpulse { object_id, .. }
            | XrNetSharedObjectControl::XrResetObject { object_id, .. } => Some(*object_id),
            XrNetSharedObjectControl::XrResetActivityPose { .. }
            | XrNetSharedObjectControl::XrClockPing { .. }
            | XrNetSharedObjectControl::XrClockPong { .. } => None,
        }
    }

    fn send_takeover_reject(
        &mut self,
        object_id: XrSharedObjectId,
        epoch: u32,
        request_id: u32,
        authoritative_state: Option<XrNetSharedObjectState>,
    ) {
        let authoritative_seq = authoritative_state.map(|state| state.seq).unwrap_or(0);
        let authoritative_tick = authoritative_state
            .map(|state| state.physics_tick)
            .unwrap_or(self.runtime.next_shared_object_physics_tick);
        if let Some(net_node) = self.runtime.net_node.as_mut() {
            net_node.send_shared_object_control(XrNetSharedObjectControl::XrTakeoverReject {
                object_id,
                epoch,
                request_id,
                authoritative_seq,
                authoritative_tick,
            });
        }
    }

    fn handle_takeover_request(
        &mut self,
        peer: XrNetPeer,
        object_id: XrSharedObjectId,
        epoch: u32,
        request_id: u32,
        based_on_seq: u32,
        based_on_tick: u32,
        candidate_owner: XrNetPeerId,
        hand: XrSharedHand,
        hand_pose: Pose,
        hand_linvel: Vec3f,
    ) -> bool {
        let Some(local_snapshot) = self
            .runtime
            .shared_objects
            .local_shared_object_snapshot(object_id)
        else {
            return true;
        };
        let authoritative_state = self.runtime.shared_objects.find_local_state_for_request(
            object_id,
            epoch,
            based_on_seq,
            based_on_tick,
        );
        if epoch != local_snapshot.epoch
            || candidate_owner != peer.id
            || !self.peer_grab_intent(peer.id, hand)
        {
            self.send_takeover_reject(object_id, epoch, request_id, authoritative_state);
            return true;
        }
        let Some(authoritative_state) = authoritative_state else {
            self.send_takeover_reject(object_id, epoch, request_id, None);
            return true;
        };
        let peer_transform = self.peer_remote_to_local_transform(peer.id);
        let local_hand_pose = Self::transform_pose(&peer_transform, hand_pose);
        let local_hand_linvel = Self::transform_direction(&peer_transform, hand_linvel);
        let distance = (authoritative_state.pose.position - local_hand_pose.position).length();
        let relative_speed = (authoritative_state.linvel - local_hand_linvel).length();
        if distance > Self::SHARED_OBJECT_TAKEOVER_DISTANCE_METERS
            || relative_speed > Self::SHARED_OBJECT_TAKEOVER_RELATIVE_SPEED_MAX
        {
            self.send_takeover_reject(object_id, epoch, request_id, Some(authoritative_state));
            return true;
        }
        let effective_at = (if self.runtime.local.state_time != 0.0 {
            self.runtime.local.state_time
        } else {
            Cx::time_now()
        }) + Self::SHARED_OBJECT_TAKEOVER_EFFECTIVE_DELAY_SECONDS;
        let effective_tick = self
            .runtime
            .next_shared_object_physics_tick
            .wrapping_add(Self::SHARED_OBJECT_TAKEOVER_EFFECTIVE_TICK_OFFSET);
        let new_epoch = local_snapshot.epoch.wrapping_add(1);
        self.runtime.shared_objects.schedule_authority_transfer(
            object_id,
            new_epoch,
            local_snapshot.authority,
            candidate_owner,
            effective_at,
            effective_tick,
            request_id,
            Some(hand),
            authoritative_state.pose,
            authoritative_state.linvel,
            authoritative_state.angvel,
        );
        if let Some(net_node) = self.runtime.net_node.as_mut() {
            net_node.send_shared_object_control(XrNetSharedObjectControl::XrTakeoverAccept {
                object_id,
                epoch: new_epoch,
                request_id,
                new_authority: candidate_owner,
                effective_at,
                effective_tick,
                pose: authoritative_state.pose,
                linvel: authoritative_state.linvel,
                angvel: authoritative_state.angvel,
            });
        }
        true
    }

    fn handle_takeover_accept(
        &mut self,
        object_id: XrSharedObjectId,
        epoch: u32,
        request_id: u32,
        source_authority: XrNetPeerId,
        new_authority: XrNetPeerId,
        effective_at: f64,
        effective_tick: u32,
        pose: Pose,
        linvel: Vec3f,
        angvel: Vec3f,
    ) -> bool {
        let local_effective_at = self
            .peer_id_for_authority(source_authority)
            .and_then(|peer_id| self.peer_time_to_local_time(peer_id, effective_at))
            .unwrap_or(effective_at);
        self.runtime
            .shared_objects
            .clear_takeover_request(object_id, Some(request_id));
        self.runtime.shared_objects.schedule_authority_transfer(
            object_id,
            epoch,
            source_authority,
            new_authority,
            local_effective_at,
            effective_tick,
            request_id,
            None,
            pose,
            linvel,
            angvel,
        )
    }

    fn handle_takeover_reject(&mut self, object_id: XrSharedObjectId, request_id: u32) -> bool {
        let _ = self
            .runtime
            .shared_objects
            .clear_takeover_request(object_id, Some(request_id));
        true
    }

    fn handle_contact_impulse(
        &mut self,
        cx: &mut Cx,
        peer: XrNetPeer,
        object_id: XrSharedObjectId,
        epoch: u32,
        based_on_seq: u32,
        based_on_tick: u32,
        hand: XrSharedHand,
        hand_pose: Pose,
        point: Vec3f,
        impulse: Vec3f,
    ) -> bool {
        let Some(local_snapshot) = self
            .runtime
            .shared_objects
            .local_shared_object_snapshot(object_id)
        else {
            return true;
        };
        if epoch != local_snapshot.epoch || self.peer_grab_intent(peer.id, hand) {
            return true;
        }
        let Some(authoritative_state) = self.runtime.shared_objects.find_local_state_for_request(
            object_id,
            epoch,
            based_on_seq,
            based_on_tick,
        ) else {
            return true;
        };
        if !matches!(
            authoritative_state.mode,
            XrSharedObjectMode::Dynamic | XrSharedObjectMode::Sleeping
        ) {
            return true;
        }
        let peer_transform = self.peer_remote_to_local_transform(peer.id);
        let local_hand_pose = Self::transform_pose(&peer_transform, hand_pose);
        let local_point = Self::transform_point(&peer_transform, point);
        let local_impulse = Self::transform_direction(&peer_transform, impulse);
        let distance = (authoritative_state.pose.position - local_hand_pose.position).length();
        if distance > Self::SHARED_OBJECT_IMPULSE_DISTANCE_METERS * 1.25
            || (authoritative_state.pose.position - local_point).length()
                > Self::SHARED_OBJECT_IMPULSE_DISTANCE_METERS * 1.5
        {
            return true;
        }
        self.emit_body_impulse(cx, local_snapshot.widget_uid, local_point, local_impulse);
        true
    }

    fn handle_reset_object(
        &mut self,
        cx: &mut Cx,
        source_authority: XrNetPeerId,
        object_id: XrSharedObjectId,
        epoch: u32,
        pose: Pose,
        linvel: Vec3f,
        angvel: Vec3f,
    ) -> bool {
        let Some(activity_id) = self.runtime.shared_objects.activity_id() else {
            return false;
        };
        let Some(widget_uid) = self
            .runtime
            .shared_objects
            .apply_remote_shared_object_reset(
                activity_id,
                self.current_local_time(),
                source_authority,
                object_id,
                epoch,
                pose,
                linvel,
                angvel,
            )
        else {
            return false;
        };
        self.emit_authority_space_body_spawn(
            cx,
            source_authority,
            widget_uid,
            self.runtime.shared_objects.local_peer_id() != Some(source_authority),
            XrSharedObjectMode::Dynamic,
            pose,
            linvel,
            angvel,
        );
        if let Some(peer_id) = self.peer_id_for_authority(source_authority) {
            self.note_applied_remote_shadow_state(
                object_id,
                peer_id,
                None,
                XrSharedObjectMode::Dynamic,
                pose,
                linvel,
                angvel,
            );
        } else {
            self.clear_applied_remote_shadow_state(object_id);
        }
        true
    }

    pub(super) fn apply_shared_object_control(
        &mut self,
        cx: &mut Cx,
        peer: XrNetPeer,
        control: &XrNetSharedObjectControl,
    ) -> bool {
        match control {
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
                ..
            } => {
                self.runtime
                    .metrics
                    .record_body_spawn_rx(peer.id, object_id.0);
                if self
                    .runtime
                    .shared_objects
                    .resolve_local_shared_object_widget(*object_id)
                    .is_some()
                {
                    return true;
                }
                let Some(widget_uid) = self.runtime.shared_objects.register_remote_shared_object(
                    *activity_id,
                    self.current_local_time(),
                    *object_id,
                    *epoch,
                    *authority,
                    *fidelity,
                    *spawnable_id,
                    *pose,
                    *linvel,
                    *angvel,
                ) else {
                    return false;
                };
                self.emit_remote_body_spawn(
                    cx,
                    peer.id,
                    widget_uid,
                    true,
                    XrSharedObjectMode::Dynamic,
                    *pose,
                    *linvel,
                    *angvel,
                );
                self.note_applied_remote_shadow_state(
                    *object_id,
                    peer.id,
                    None,
                    XrSharedObjectMode::Dynamic,
                    *pose,
                    *linvel,
                    *angvel,
                );
                true
            }
            XrNetSharedObjectControl::XrDespawnObject { object_id, .. } => {
                self.runtime
                    .pending_shared_object_controls
                    .retain(|(_, pending_control)| {
                        Self::shared_object_control_object_id(pending_control) != Some(*object_id)
                    });
                if let Some(widget_uid) = self
                    .runtime
                    .shared_objects
                    .release_remote_shared_object(*object_id)
                {
                    self.emit_remote_body_despawn(cx, widget_uid);
                }
                self.clear_applied_remote_shadow_state(*object_id);
                true
            }
            XrNetSharedObjectControl::XrTakeoverAccept {
                object_id,
                epoch,
                request_id,
                new_authority,
                effective_at,
                effective_tick,
                pose,
                linvel,
                angvel,
            } => self.handle_takeover_accept(
                *object_id,
                *epoch,
                *request_id,
                peer.id,
                *new_authority,
                *effective_at,
                *effective_tick,
                *pose,
                *linvel,
                *angvel,
            ),
            XrNetSharedObjectControl::XrResetObject {
                object_id,
                epoch,
                pose,
                linvel,
                angvel,
            } => self.handle_reset_object(cx, peer.id, *object_id, *epoch, *pose, *linvel, *angvel),
            XrNetSharedObjectControl::XrResetActivityPose { activity_id, pose } => {
                if self.runtime.shared_objects.activity_id() != Some(*activity_id) {
                    return false;
                }
                let Some(peer_state) = self.runtime.registry.peers.get_mut(&peer.id) else {
                    return false;
                };
                peer_state.remote_activity_pose = Some(*pose);
                peer_state.last_emitted_local_activity_pose = None;
                self.emit_authoritative_remote_activity_pose_if_changed(cx);
                true
            }
            XrNetSharedObjectControl::XrClockPing { seq, sent_at } => {
                self.runtime.metrics.record_clock_ping_rx(peer.id, *seq);
                let replied_at = if self.runtime.local.state_time != 0.0 {
                    self.runtime.local.state_time
                } else {
                    Cx::time_now()
                };
                if let Some(net_node) = self.runtime.net_node.as_mut() {
                    net_node.send_shared_object_control(XrNetSharedObjectControl::XrClockPong {
                        seq: *seq,
                        echoed_at: *sent_at,
                        replied_at,
                    });
                    self.runtime.metrics.record_clock_pong_tx(*seq);
                }
                true
            }
            XrNetSharedObjectControl::XrClockPong {
                seq,
                echoed_at: _,
                replied_at,
            } => {
                self.runtime.metrics.record_clock_pong_rx(peer.id, *seq);
                let Some(local_sent_at) = self
                    .runtime
                    .pending_clock_pings
                    .iter()
                    .find_map(|(pending_seq, sent_at)| (*pending_seq == *seq).then_some(*sent_at))
                else {
                    return true;
                };
                let now = if self.runtime.local.state_time != 0.0 {
                    self.runtime.local.state_time
                } else {
                    Cx::time_now()
                };
                let round_trip_seconds = (now - local_sent_at).max(0.0);
                let midpoint = local_sent_at + round_trip_seconds * 0.5;
                let clock_offset_seconds = *replied_at - midpoint;
                if let Some(peer_state) = self.runtime.registry.peers.get_mut(&peer.id) {
                    peer_state.clock_offset_seconds = Some(clock_offset_seconds);
                    peer_state.clock_round_trip_seconds = Some(round_trip_seconds);
                    peer_state.last_clock_sync_at = Some(now);
                }
                true
            }
            XrNetSharedObjectControl::XrTakeoverRequest {
                object_id,
                epoch,
                request_id,
                based_on_seq,
                based_on_tick,
                candidate_owner,
                hand,
                hand_pose,
                hand_linvel,
            } => self.handle_takeover_request(
                peer,
                *object_id,
                *epoch,
                *request_id,
                *based_on_seq,
                *based_on_tick,
                *candidate_owner,
                *hand,
                *hand_pose,
                *hand_linvel,
            ),
            XrNetSharedObjectControl::XrTakeoverReject {
                object_id,
                request_id,
                ..
            } => self.handle_takeover_reject(*object_id, *request_id),
            XrNetSharedObjectControl::XrContactImpulse {
                object_id,
                epoch,
                based_on_seq,
                based_on_tick,
                hand,
                hand_pose,
                point,
                impulse,
            } => self.handle_contact_impulse(
                cx,
                peer,
                *object_id,
                *epoch,
                *based_on_seq,
                *based_on_tick,
                *hand,
                *hand_pose,
                *point,
                *impulse,
            ),
        }
    }
}
