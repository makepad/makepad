use super::*;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum RemoteTransformSource {
    #[default]
    Raw,
    Anchor,
    Descriptor,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct LocalSharedHandState {
    pub(super) shared_hand: XrSharedHand,
    pub(super) pose: Pose,
    pub(super) linvel: Vec3f,
    pub(super) gripping: bool,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct TimedRemoteSyncAnchor {
    pub(super) sync: XrSyncAnchor,
    pub(super) first_seen_at_local_time: f64,
    pub(super) last_seen_at_local_time: f64,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct TimedLocalSyncAnchor {
    pub(super) sync: XrSyncAnchor,
    pub(super) last_seen_at_local_time: f64,
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct XrSyncAnchorAccumulator {
    sum_left: Vec3f,
    sum_right: Vec3f,
    pub(super) sample_count: u32,
}

impl XrSyncAnchorAccumulator {
    fn push(&mut self, anchor: XrAnchor) -> XrAnchor {
        self.sum_left += anchor.left;
        self.sum_right += anchor.right;
        self.sample_count = self.sample_count.saturating_add(1);
        XrAnchor {
            left: self.sum_left / self.sample_count as f32,
            right: self.sum_right / self.sample_count as f32,
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct RemotePeerState {
    pub(super) peer: XrNetPeer,
    pub(super) latest_state: Option<XrNetStateFrame>,
    pub(super) last_state_received_at: f64,
    pub(super) last_sync_anchor_seen_at: Option<f64>,
    pub(super) recent_sync_anchors: VecDeque<TimedRemoteSyncAnchor>,
    pub(super) last_fist_hold_anchor: Option<XrAnchor>,
    pub(super) last_fist_hold_seen_at: Option<f64>,
    pub(super) remote_activity_pose: Option<Pose>,
    pub(super) last_emitted_local_activity_pose: Option<Pose>,
    pub(super) latest_descriptor: Option<XrNetAlignmentDescriptorFrame>,
    pub(super) has_descriptor: bool,
    pub(super) anchor_remote_to_local: Option<Mat4f>,
    pub(super) descriptor_remote_to_local: Option<Mat4f>,
    pub(super) remote_to_local: Option<Mat4f>,
    pub(super) transform_source: RemoteTransformSource,
    pub(super) last_solve_diagnostic: Option<XrDepthAlignSolveDiagnostic>,
    pub(super) last_solve_ms: f64,
    pub(super) last_solved_local_descriptor_version: Option<(u64, u64)>,
    pub(super) last_solved_remote_descriptor_seq: Option<u32>,
    pub(super) worker_progress: Option<XrDepthAlignMatcherProgress>,
    pub(super) clock_offset_seconds: Option<f64>,
    pub(super) clock_round_trip_seconds: Option<f64>,
    pub(super) last_clock_sync_at: Option<f64>,
}

impl RemotePeerState {
    pub(super) fn new(peer: XrNetPeer) -> Self {
        Self {
            peer,
            latest_state: None,
            last_state_received_at: 0.0,
            last_sync_anchor_seen_at: None,
            recent_sync_anchors: VecDeque::new(),
            last_fist_hold_anchor: None,
            last_fist_hold_seen_at: None,
            remote_activity_pose: None,
            last_emitted_local_activity_pose: None,
            latest_descriptor: None,
            has_descriptor: false,
            anchor_remote_to_local: None,
            descriptor_remote_to_local: None,
            remote_to_local: None,
            transform_source: RemoteTransformSource::Raw,
            last_solve_diagnostic: None,
            last_solve_ms: 0.0,
            last_solved_local_descriptor_version: None,
            last_solved_remote_descriptor_seq: None,
            worker_progress: None,
            clock_offset_seconds: None,
            clock_round_trip_seconds: None,
            last_clock_sync_at: None,
        }
    }
}

#[derive(Default)]
pub(super) struct XrPeerSyncLocalState {
    pub(super) state_time: f64,
    pub(super) anchor: Option<XrAnchor>,
    pub(super) anchor_override: Option<XrAnchor>,
    pub(super) sync_anchor: Option<XrSyncAnchor>,
    pub(super) last_sync_anchor_seen_at: Option<f64>,
    pub(super) recent_sync_anchors: VecDeque<TimedLocalSyncAnchor>,
    pub(super) sync_anchor_accumulator: Option<XrSyncAnchorAccumulator>,
    pub(super) last_sync_match_at: Option<f64>,
    pub(super) fist_hold_anchor: Option<XrAnchor>,
    pub(super) last_fist_hold_seen_at: Option<f64>,
    pub(super) previous_xr_state: Option<XrState>,
    pub(super) latest_xr_state: Option<XrState>,
    pub(super) descriptor: Option<XrNetAlignmentDescriptorFrame>,
    pub(super) descriptor_version: Option<(u64, u64)>,
    pub(super) slice_preview: Option<XrDepthAlignSlicePreview>,
    pub(super) last_sent_descriptor_signature: Option<(u64, u64)>,
    pub(super) last_sent_descriptor: Option<XrDepthAlignDescriptor>,
    pub(super) last_sent_descriptor_at: Option<f64>,
}

impl XrPeerSyncLocalState {
    fn far_relocated_anchor_override(
        saved_anchor: XrAnchor,
        motion_center: Vec3f,
    ) -> Option<XrAnchor> {
        let left_distance = (motion_center - saved_anchor.left).length();
        let right_distance = (motion_center - saved_anchor.right).length();
        let replace_left = left_distance <= right_distance;
        let preserved_distance = if replace_left {
            right_distance
        } else {
            left_distance
        };
        if preserved_distance <= XrPeerSync::SYNC_EXISTING_MARKER_REUSE_DISTANCE_METERS {
            return None;
        }
        Some(if replace_left {
            XrAnchor {
                left: motion_center,
                right: saved_anchor.right,
            }
        } else {
            XrAnchor {
                left: saved_anchor.left,
                right: motion_center,
            }
        })
    }

    pub(super) fn record_sync_anchor(&mut self, sync_anchor: XrSyncAnchor) {
        self.last_sync_anchor_seen_at = Some(self.state_time);
        if let Some(recent) = self
            .recent_sync_anchors
            .back_mut()
            .filter(|recent| recent.sync.id == sync_anchor.id)
        {
            recent.last_seen_at_local_time = self.state_time;
            return;
        }
        self.recent_sync_anchors.push_back(TimedLocalSyncAnchor {
            sync: sync_anchor,
            last_seen_at_local_time: self.state_time,
        });
    }

    pub(super) fn prune_recent_sync_anchors(&mut self) {
        while self.recent_sync_anchors.front().is_some_and(|sync| {
            self.state_time - sync.last_seen_at_local_time > XrPeerSync::SYNC_SAMPLE_HISTORY_SECONDS
        }) {
            self.recent_sync_anchors.pop_front();
        }
    }

    pub(super) fn record_matched_sync_anchor(&mut self, anchor: XrAnchor, now: f64) -> XrAnchor {
        if self
            .last_sync_match_at
            .is_some_and(|last| now - last > XrPeerSync::SYNC_SAMPLE_SESSION_RESET_SECONDS)
        {
            self.sync_anchor_accumulator = None;
        }
        let mut accumulator = self.sync_anchor_accumulator.unwrap_or_default();
        let averaged_anchor = accumulator.push(anchor);
        self.sync_anchor_accumulator = Some(accumulator);
        self.last_sync_match_at = Some(now);
        if let Some(saved_anchor) = self.anchor {
            let averaged_motion_center = (averaged_anchor.left + averaged_anchor.right) * 0.5;
            if let Some(override_anchor) =
                Self::far_relocated_anchor_override(saved_anchor, averaged_motion_center)
            {
                return override_anchor;
            }
        }
        averaged_anchor
    }

    pub(super) fn matched_sync_sample_count(&self) -> u32 {
        self.sync_anchor_accumulator
            .map(|accumulator| accumulator.sample_count)
            .unwrap_or(0)
    }

    pub(super) fn effective_anchor(&self) -> Option<XrAnchor> {
        self.anchor_override.or(self.anchor)
    }

    pub(super) fn active_sync_anchor(&self) -> Option<XrSyncAnchor> {
        self.sync_anchor.filter(|_| {
            self.last_sync_anchor_seen_at.is_some_and(|seen_at| {
                self.state_time - seen_at <= XrPeerSync::SYNC_MATCH_ACTIVE_WINDOW_SECONDS
            })
        })
    }

    pub(super) fn scene_state(&self) -> LocalSceneState {
        if self.descriptor.is_some() {
            LocalSceneState::Ready
        } else if self.contour_sample_count() != 0 {
            LocalSceneState::PublishPending
        } else {
            LocalSceneState::Missing
        }
    }

    pub(super) fn contour_sample_count(&self) -> usize {
        self.descriptor
            .as_ref()
            .map(|frame| descriptor_contour_sample_count(&frame.descriptor))
            .unwrap_or(0)
    }
}

#[derive(Default)]
pub(super) struct XrPeerSyncRuntime {
    pub(super) net_node: Option<XrNetNode>,
    pub(super) alignment_worker: Option<XrPeopleAlignmentWorker>,
    pub(super) local: XrPeerSyncLocalState,
    pub(super) registry: XrPeerRegistry,
    pub(super) recent_anchor_confirmation: Option<XrRecentAnchorConfirmation>,
    pub(super) shared_objects: XrSharedObjectRegistry,
    pub(super) next_shared_object_physics_tick: u32,
    pub(super) next_shared_object_request_id: u32,
    pub(super) applied_remote_shadow_states: HashMap<XrSharedObjectId, XrAppliedRemoteShadowState>,
    pub(super) pending_shared_object_controls: Vec<(XrNetPeer, XrNetSharedObjectControl)>,
    pub(super) pending_clock_pings: VecDeque<(u32, f64)>,
    pub(super) next_clock_ping_seq: u32,
    pub(super) next_clock_ping_at: f64,
    pub(super) accepted_activity: Option<XrNetActivityState>,
    pub(super) local_shared_object_reannounce_needed: bool,
    pub(super) metrics: XrPeerSyncMetrics,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct XrAppliedRemoteShadowState {
    pub(super) peer_id: XrNetPeerId,
    pub(super) applied_at_local_time: f64,
    pub(super) state_seq: Option<u32>,
    pub(super) mode: XrSharedObjectMode,
    pub(super) pose: Pose,
    pub(super) linvel: Vec3f,
    pub(super) angvel: Vec3f,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct XrRecentAnchorConfirmation {
    pub(super) anchor: XrAnchor,
    pub(super) visible_until: f64,
}
