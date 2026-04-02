use super::*;

#[derive(Default)]
pub(super) struct XrPeerRegistry {
    pub(super) peers: HashMap<XrNetPeerId, RemotePeerState>,
    accepted_local_sync_ids: HashMap<u32, f64>,
    accepted_remote_sync_ids: HashMap<(XrNetPeerId, u32), f64>,
}

impl XrPeerRegistry {
    fn descriptor_with_anchor_height_override(
        descriptor_remote_to_local: Mat4f,
        anchor_remote_to_local: Option<Mat4f>,
    ) -> Mat4f {
        let Some(anchor_remote_to_local) = anchor_remote_to_local else {
            return descriptor_remote_to_local;
        };
        let mut corrected = descriptor_remote_to_local;
        corrected.v[13] = anchor_remote_to_local.v[13];
        corrected
    }

    pub(super) fn len(&self) -> usize {
        self.peers.len()
    }

    pub(super) fn peer_ids(&self) -> Vec<XrNetPeerId> {
        self.peers.keys().copied().collect()
    }

    pub(super) fn preferred_peer(&self) -> Option<(XrNetPeerId, RemotePeerState)> {
        self.peers
            .iter()
            .max_by_key(|(peer_id, peer_state)| {
                (
                    peer_state.remote_to_local.is_some(),
                    peer_state.latest_state.is_some(),
                    std::cmp::Reverse(peer_id.0),
                )
            })
            .map(|(peer_id, peer_state)| (*peer_id, peer_state.clone()))
    }

    pub(super) fn track_join(&mut self, peer: XrNetPeer) {
        self.peers
            .entry(peer.id)
            .or_insert_with(|| RemotePeerState::new(peer));
    }

    pub(super) fn track_leave(&mut self, peer_id: XrNetPeerId) {
        self.peers.remove(&peer_id);
        self.accepted_remote_sync_ids
            .retain(|(accepted_peer_id, _), _| *accepted_peer_id != peer_id);
    }

    pub(super) fn clear_remote_activity_poses(&mut self) {
        for peer_state in self.peers.values_mut() {
            peer_state.remote_activity_pose = None;
            peer_state.last_emitted_local_activity_pose = None;
        }
    }

    pub(super) fn track_state(
        &mut self,
        peer: XrNetPeer,
        frame: XrNetStateFrame,
        local_state_time: f64,
    ) {
        let peer_state = self
            .peers
            .entry(peer.id)
            .or_insert_with(|| RemotePeerState::new(peer));
        let fresh_fist_hold_anchor = XrPeerSync::state_fist_ack_anchor(&frame.state);
        peer_state.peer = peer;
        peer_state.last_state_received_at = local_state_time;
        if let Some(sync_anchor) = frame.state.sync_anchor {
            peer_state.last_sync_anchor_seen_at = Some(local_state_time);
            if let Some(recent_sync) = peer_state
                .recent_sync_anchors
                .back_mut()
                .filter(|recent_sync| recent_sync.sync.id == sync_anchor.id)
            {
                recent_sync.last_seen_at_local_time = local_state_time;
            } else {
                peer_state
                    .recent_sync_anchors
                    .push_back(TimedRemoteSyncAnchor {
                        sync: sync_anchor,
                        first_seen_at_local_time: local_state_time,
                        last_seen_at_local_time: local_state_time,
                    });
            }
        }
        if let Some(anchor) = fresh_fist_hold_anchor {
            peer_state.last_fist_hold_anchor = Some(anchor);
            peer_state.last_fist_hold_seen_at = Some(local_state_time);
        } else if peer_state.last_fist_hold_seen_at.is_some_and(|seen_at| {
            local_state_time - seen_at > XrPeerSync::FIST_ACK_STICKY_WINDOW_SECONDS
        }) {
            peer_state.last_fist_hold_anchor = None;
            peer_state.last_fist_hold_seen_at = None;
            peer_state.recent_sync_anchors.clear();
        }
        while peer_state
            .recent_sync_anchors
            .front()
            .is_some_and(|recent_sync| {
                local_state_time - recent_sync.last_seen_at_local_time
                    > XrPeerSync::SYNC_SAMPLE_HISTORY_SECONDS
            })
        {
            peer_state.recent_sync_anchors.pop_front();
        }
        peer_state.latest_state = Some(frame);
    }

    pub(super) fn track_descriptor(
        &mut self,
        peer: XrNetPeer,
        frame: XrNetAlignmentDescriptorFrame,
    ) {
        let peer_state = self
            .peers
            .entry(peer.id)
            .or_insert_with(|| RemotePeerState::new(peer));
        peer_state.peer = peer;
        peer_state.latest_descriptor = Some(frame);
        peer_state.has_descriptor = true;
    }

    pub(super) fn apply_alignment_results(
        &mut self,
        peer_results: HashMap<XrNetPeerId, AlignmentWorkerPeerResult>,
    ) {
        for peer_state in self.peers.values_mut() {
            peer_state.descriptor_remote_to_local = None;
        }
        for (peer_id, peer_result) in peer_results {
            if let Some(peer_state) = self.peers.get_mut(&peer_id) {
                peer_state.descriptor_remote_to_local = peer_result.remote_to_local;
                peer_state.last_solve_diagnostic = peer_result.last_solve_diagnostic;
                peer_state.last_solve_ms = peer_result.last_solve_ms;
                peer_state.last_solved_local_descriptor_version =
                    peer_result.last_solved_local_descriptor_version;
                peer_state.last_solved_remote_descriptor_seq =
                    peer_result.last_solved_remote_descriptor_seq;
                peer_state.worker_progress = peer_result.worker_progress;
                peer_state.has_descriptor =
                    peer_state.has_descriptor || peer_result.last_solve_diagnostic.is_some();
            }
        }
    }

    fn prune_accepted_sync_ids(&mut self, now: f64) {
        self.accepted_local_sync_ids
            .retain(|_, accepted_at| now - *accepted_at <= XrPeerSync::SYNC_SAMPLE_HISTORY_SECONDS);
        self.accepted_remote_sync_ids
            .retain(|_, accepted_at| now - *accepted_at <= XrPeerSync::SYNC_SAMPLE_HISTORY_SECONDS);
    }

    fn translated_remote_sync_time(
        peer_state: &RemotePeerState,
        remote_sync: &TimedRemoteSyncAnchor,
    ) -> f64 {
        peer_state
            .clock_offset_seconds
            .map(|clock_offset| remote_sync.sync.captured_at - clock_offset)
            .unwrap_or(remote_sync.first_seen_at_local_time)
    }

    fn best_sync_match(
        peer_id: XrNetPeerId,
        peer_state: &RemotePeerState,
        accepted_local_sync_ids: &HashMap<u32, f64>,
        accepted_remote_sync_ids: &HashMap<(XrNetPeerId, u32), f64>,
        local: &XrPeerSyncLocalState,
        now: f64,
    ) -> Option<(XrSyncAnchor, TimedRemoteSyncAnchor)> {
        if !XrPeerSync::remote_state_is_recent(peer_state, now)
            || local.fist_hold_anchor.is_none()
            || XrPeerSync::recent_remote_fist_hold_anchor(peer_state, now).is_none()
        {
            return None;
        }

        let mut best_match = None;
        let mut best_dt = f64::MAX;
        for local_sync in local.recent_sync_anchors.iter().rev() {
            if accepted_local_sync_ids.contains_key(&local_sync.sync.id) {
                continue;
            }
            for remote_sync in peer_state.recent_sync_anchors.iter().rev() {
                if accepted_remote_sync_ids.contains_key(&(peer_id, remote_sync.sync.id))
                    || local_sync.sync.extrema == remote_sync.sync.extrema
                {
                    continue;
                }
                let remote_sync_local_time =
                    Self::translated_remote_sync_time(peer_state, remote_sync);
                let dt = (local_sync.sync.captured_at - remote_sync_local_time).abs();
                if dt > XrPeerSync::SYNC_SAMPLE_PAIR_WINDOW_SECONDS || dt >= best_dt {
                    continue;
                }
                best_match = Some((local_sync.sync, *remote_sync));
                best_dt = dt;
            }
        }
        best_match
    }

    pub(super) fn refresh_transforms(
        &mut self,
        cx: &mut Cx,
        local: &mut XrPeerSyncLocalState,
        recent_anchor_confirmation: &mut Option<XrRecentAnchorConfirmation>,
        now: f64,
    ) -> bool {
        let mut changed = false;
        self.prune_accepted_sync_ids(now);

        for (peer_id, peer_state) in self.peers.iter_mut() {
            peer_state.anchor_remote_to_local = None;
            if let Some((local_sync_anchor, remote_sync_anchor)) = {
                let accepted_local_sync_ids = &self.accepted_local_sync_ids;
                let accepted_remote_sync_ids = &self.accepted_remote_sync_ids;
                Self::best_sync_match(
                    *peer_id,
                    peer_state,
                    accepted_local_sync_ids,
                    accepted_remote_sync_ids,
                    local,
                    now,
                )
            } {
                self.accepted_local_sync_ids
                    .insert(local_sync_anchor.id, now);
                self.accepted_remote_sync_ids
                    .insert((*peer_id, remote_sync_anchor.sync.id), now);
                let averaged_anchor =
                    local.record_matched_sync_anchor(local_sync_anchor.anchor, now);
                local.anchor_override = Some(averaged_anchor);
                cx.xr_set_local_anchor(averaged_anchor);
                *recent_anchor_confirmation = Some(XrRecentAnchorConfirmation {
                    anchor: averaged_anchor,
                    visible_until: now + XrPeerSync::ANCHOR_CONFIRMATION_SECONDS,
                });
                changed = true;
            }

            if peer_state.anchor_remote_to_local.is_none() {
                let effective_local_anchor = local.effective_anchor();
                if let (Some(local_anchor), Some(state_frame)) =
                    (effective_local_anchor, peer_state.latest_state.as_ref())
                {
                    if let Some(remote_anchor) = state_frame.state.anchor {
                        peer_state.anchor_remote_to_local =
                            Some(remote_anchor.mirrored().mapping_to(&local_anchor));
                    }
                }
            }

            let next_transform =
                if let Some(descriptor_remote_to_local) = peer_state.descriptor_remote_to_local {
                    Some(Self::descriptor_with_anchor_height_override(
                        descriptor_remote_to_local,
                        peer_state.anchor_remote_to_local,
                    ))
                } else {
                    peer_state.anchor_remote_to_local
                };
            let next_source = if peer_state.descriptor_remote_to_local.is_some() {
                RemoteTransformSource::Descriptor
            } else if peer_state.anchor_remote_to_local.is_some() {
                RemoteTransformSource::Anchor
            } else {
                RemoteTransformSource::Raw
            };
            if peer_state.remote_to_local != next_transform
                || peer_state.transform_source != next_source
            {
                peer_state.remote_to_local = next_transform;
                peer_state.transform_source = next_source;
                changed = true;
            }
        }

        changed
    }
}
