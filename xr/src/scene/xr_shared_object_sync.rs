use super::{xr_widget_children, xr_widget_with_scene_node, XrBodyKind};
use crate::prelude::*;
use std::collections::{HashMap, VecDeque};

const XR_SHARED_OBJECT_HASH_OFFSET: u64 = 0xcbf29ce484222325;
const XR_SHARED_OBJECT_HASH_PRIME: u64 = 0x00000100000001b3;
const XR_SHARED_OBJECT_ROOT_TAG: u64 = 0x78725f7368617265;
const XR_SHARED_OBJECT_CHILD_TAG: u64 = 0x6368696c645f7061;
const XR_SHARED_OBJECT_INDEX_TAG: u64 = 0x696e6465785f7061;
const XR_SHARED_OBJECT_BODY_TAG: u64 = 0x626f64795f737061;
const XR_SHARED_OBJECT_POOL_GROUP_TAG: u64 = 0x706f6f6c5f677270;
const XR_SHARED_OBJECT_HISTORY_MAX_SAMPLES: usize = 48;
const XR_SHARED_OBJECT_HISTORY_MAX_SECONDS: f64 = 0.5;
const XR_SHARED_OBJECT_TAKEOVER_REQUEST_COOLDOWN_SECONDS: f64 = 0.18;
const XR_SHARED_OBJECT_CONTACT_IMPULSE_COOLDOWN_SECONDS: f64 = 0.08;
const XR_SHARED_OBJECT_SLEEPING_PUBLISH_INTERVAL_SECONDS: f64 = 0.25;
const XR_SHARED_OBJECT_PUBLISH_POSITION_EPSILON_METERS: f32 = 0.001;
const XR_SHARED_OBJECT_PUBLISH_ORIENTATION_EPSILON_DEGREES: f32 = 0.25;
const XR_SHARED_OBJECT_PUBLISH_LINVEL_EPSILON_MPS: f32 = 0.02;
const XR_SHARED_OBJECT_PUBLISH_ANGVEL_EPSILON_RADPS: f32 = 0.02;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct XrSpawnableObjectBinding {
    pub object_id: XrSpawnableObjectId,
    pub allocation_group_id: XrSpawnableObjectId,
    pub widget_uid: WidgetUid,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct XrLocalSharedObjectAllocation {
    pub shared_object_id: XrSharedObjectId,
    pub spawnable_object_id: XrSpawnableObjectId,
    pub widget_uid: WidgetUid,
    pub epoch: u32,
    pub fidelity: XrSharedObjectFidelity,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct XrLocalSharedObjectSnapshot {
    pub object_id: XrSharedObjectId,
    pub spawnable_object_id: XrSpawnableObjectId,
    pub widget_uid: WidgetUid,
    pub epoch: u32,
    pub authority: XrPeerId,
    pub fidelity: XrSharedObjectFidelity,
    pub latest_state: Option<XrNetSharedObjectState>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct XrRemoteSharedObjectSnapshot {
    pub object_id: XrSharedObjectId,
    pub spawnable_object_id: XrSpawnableObjectId,
    pub widget_uid: WidgetUid,
    pub epoch: u32,
    pub authority: XrPeerId,
    pub state_source_authority: XrPeerId,
    pub fidelity: XrSharedObjectFidelity,
    pub latest_state: Option<XrNetSharedObjectState>,
    pub pending_takeover_request_id: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct XrScheduledAuthorityTransfer {
    pub object_id: XrSharedObjectId,
    pub widget_uid: WidgetUid,
    pub shadow: bool,
    pub source_authority: XrPeerId,
    pub new_authority: XrPeerId,
    pub epoch: u32,
    pub request_id: u32,
    pub hand: Option<XrSharedHand>,
    pub pose: Pose,
    pub linvel: Vec3f,
    pub angvel: Vec3f,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct XrPendingAuthorityTransfer {
    epoch: u32,
    source_authority: XrPeerId,
    new_authority: XrPeerId,
    effective_at: f64,
    effective_tick: u32,
    request_id: u32,
    hand: Option<XrSharedHand>,
    pose: Pose,
    linvel: Vec3f,
    angvel: Vec3f,
}

#[derive(Clone, Debug, PartialEq)]
struct XrLocalSharedObjectRecord {
    spawnable_object_id: XrSpawnableObjectId,
    widget_uid: WidgetUid,
    epoch: u32,
    authority: XrPeerId,
    fidelity: XrSharedObjectFidelity,
    latest_state: Option<XrNetSharedObjectState>,
    last_published_state: Option<XrNetSharedObjectState>,
    last_published_at: Option<f64>,
    history: VecDeque<XrNetSharedObjectState>,
    pending_transfer: Option<XrPendingAuthorityTransfer>,
}

#[derive(Clone, Debug, PartialEq)]
struct XrRemoteSharedObjectRecord {
    spawnable_object_id: XrSpawnableObjectId,
    widget_uid: WidgetUid,
    epoch: u32,
    authority: XrPeerId,
    state_source_authority: XrPeerId,
    fidelity: XrSharedObjectFidelity,
    latest_state: Option<XrNetSharedObjectState>,
    history: VecDeque<XrNetSharedObjectState>,
    pending_takeover_request_id: Option<u32>,
    last_takeover_request_at: Option<f64>,
    last_contact_impulse_at: Option<f64>,
    pending_transfer: Option<XrPendingAuthorityTransfer>,
}

#[derive(Clone, Debug, Default)]
pub struct XrSharedObjectRegistry {
    activity_id: Option<XrActivityId>,
    local_peer_id: Option<XrPeerId>,
    next_shared_object_counter: XrSharedObjectCounter,
    object_to_widget: HashMap<XrSpawnableObjectId, WidgetUid>,
    object_to_group: HashMap<XrSpawnableObjectId, XrSpawnableObjectId>,
    widget_to_object: HashMap<WidgetUid, XrSpawnableObjectId>,
    group_to_widgets: HashMap<XrSpawnableObjectId, Vec<WidgetUid>>,
    local_objects: HashMap<XrSharedObjectId, XrLocalSharedObjectRecord>,
    local_shared_object_to_widget: HashMap<XrSharedObjectId, WidgetUid>,
    widget_to_local_shared_object: HashMap<WidgetUid, XrSharedObjectId>,
    remote_objects: HashMap<XrSharedObjectId, XrRemoteSharedObjectRecord>,
    remote_object_to_widget: HashMap<XrSharedObjectId, WidgetUid>,
    remote_widget_to_object: HashMap<WidgetUid, XrSharedObjectId>,
}

impl XrSharedObjectRegistry {
    fn release_remote_widget_claim(&mut self, widget_uid: WidgetUid) {
        if let Some(object_id) = self.remote_widget_to_object.remove(&widget_uid) {
            self.remote_object_to_widget.remove(&object_id);
            self.remote_objects.remove(&object_id);
        }
    }

    pub fn activity_id(&self) -> Option<XrActivityId> {
        self.activity_id
    }

    pub fn len(&self) -> usize {
        self.object_to_widget.len()
    }

    pub fn active_count(&self) -> usize {
        self.local_objects.len() + self.remote_object_to_widget.len()
    }

    pub fn local_peer_id(&self) -> Option<XrPeerId> {
        self.local_peer_id
    }

    pub fn set_local_peer_id(&mut self, peer_id: XrPeerId) {
        self.local_peer_id = Some(peer_id);
    }

    pub fn clear(&mut self) {
        self.activity_id = None;
        self.object_to_widget.clear();
        self.object_to_group.clear();
        self.widget_to_object.clear();
        self.group_to_widgets.clear();
        self.local_objects.clear();
        self.local_shared_object_to_widget.clear();
        self.widget_to_local_shared_object.clear();
        self.remote_objects.clear();
        self.remote_object_to_widget.clear();
        self.remote_widget_to_object.clear();
    }

    pub fn replace_spawnables<I>(&mut self, activity_id: XrActivityId, bindings: I)
    where
        I: IntoIterator<Item = XrSpawnableObjectBinding>,
    {
        self.activity_id = Some(activity_id);
        self.object_to_widget.clear();
        self.object_to_group.clear();
        self.widget_to_object.clear();
        self.group_to_widgets.clear();
        self.local_objects.clear();
        self.local_shared_object_to_widget.clear();
        self.widget_to_local_shared_object.clear();
        self.remote_objects.clear();
        self.remote_object_to_widget.clear();
        self.remote_widget_to_object.clear();
        for binding in bindings {
            self.object_to_widget
                .insert(binding.object_id, binding.widget_uid);
            self.object_to_group
                .insert(binding.object_id, binding.allocation_group_id);
            self.widget_to_object
                .insert(binding.widget_uid, binding.object_id);
            self.group_to_widgets
                .entry(binding.allocation_group_id)
                .or_default()
                .push(binding.widget_uid);
        }
    }

    pub fn resolve_object_id(
        &self,
        activity_id: XrActivityId,
        widget_uid: WidgetUid,
    ) -> Option<XrSpawnableObjectId> {
        (self.activity_id == Some(activity_id))
            .then(|| self.widget_to_object.get(&widget_uid).copied())
            .flatten()
    }

    pub fn resolve_widget_uid(
        &self,
        activity_id: XrActivityId,
        object_id: XrSpawnableObjectId,
    ) -> Option<WidgetUid> {
        (self.activity_id == Some(activity_id))
            .then(|| self.object_to_widget.get(&object_id).copied())
            .flatten()
    }

    pub fn resolve_remote_shared_object_widget(
        &self,
        shared_object_id: XrSharedObjectId,
    ) -> Option<WidgetUid> {
        self.remote_object_to_widget.get(&shared_object_id).copied()
    }

    pub fn resolve_remote_shared_object_for_widget(
        &self,
        widget_uid: WidgetUid,
    ) -> Option<XrSharedObjectId> {
        self.remote_widget_to_object.get(&widget_uid).copied()
    }

    pub fn resolve_local_shared_object_for_widget(
        &self,
        widget_uid: WidgetUid,
    ) -> Option<XrSharedObjectId> {
        self.widget_to_local_shared_object.get(&widget_uid).copied()
    }

    pub fn resolve_local_shared_object_widget(
        &self,
        shared_object_id: XrSharedObjectId,
    ) -> Option<WidgetUid> {
        self.local_shared_object_to_widget
            .get(&shared_object_id)
            .copied()
    }

    pub fn release_remote_shared_object(
        &mut self,
        shared_object_id: XrSharedObjectId,
    ) -> Option<WidgetUid> {
        let widget_uid = self.remote_object_to_widget.remove(&shared_object_id)?;
        self.remote_widget_to_object.remove(&widget_uid);
        self.remote_objects.remove(&shared_object_id);
        Some(widget_uid)
    }

    pub fn release_remote_shared_objects_by_peer_id(
        &mut self,
        peer_id: XrPeerId,
    ) -> Vec<WidgetUid> {
        let object_ids = self
            .remote_object_to_widget
            .keys()
            .copied()
            .filter(|object_id| xr_shared_object_peer_id(*object_id) == peer_id)
            .collect::<Vec<_>>();
        let mut widgets = Vec::with_capacity(object_ids.len());
        for object_id in object_ids {
            if let Some(widget_uid) = self.release_remote_shared_object(object_id) {
                widgets.push(widget_uid);
            }
        }
        widgets
    }

    pub fn resolve_remote_widget_uid(
        &mut self,
        activity_id: XrActivityId,
        shared_object_id: XrSharedObjectId,
        spawnable_object_id: XrSpawnableObjectId,
    ) -> Option<WidgetUid> {
        if self.activity_id != Some(activity_id) {
            return None;
        }
        if let Some(widget_uid) = self.remote_object_to_widget.get(&shared_object_id).copied() {
            return Some(widget_uid);
        }
        let preferred_widget = self.object_to_widget.get(&spawnable_object_id).copied()?;
        let group_id = self.object_to_group.get(&spawnable_object_id).copied()?;
        let group_widgets = self.group_to_widgets.get(&group_id)?;
        let widget_uid = if !self.remote_widget_to_object.contains_key(&preferred_widget)
            && !self
                .widget_to_local_shared_object
                .contains_key(&preferred_widget)
        {
            preferred_widget
        } else {
            let start_index = (hash_u64(shared_object_id.0, spawnable_object_id.0) as usize)
                % group_widgets.len();
            let mut fallback = group_widgets[start_index];
            let mut free_widget = None;
            for offset in 0..group_widgets.len() {
                let candidate = group_widgets[(start_index + offset) % group_widgets.len()];
                fallback = candidate;
                if !self.remote_widget_to_object.contains_key(&candidate)
                    && !self.widget_to_local_shared_object.contains_key(&candidate)
                {
                    free_widget = Some(candidate);
                    break;
                }
            }
            free_widget.unwrap_or(fallback)
        };
        if let Some(previous_key) = self
            .remote_widget_to_object
            .insert(widget_uid, shared_object_id)
        {
            self.remote_object_to_widget.remove(&previous_key);
        }
        self.remote_object_to_widget
            .insert(shared_object_id, widget_uid);
        Some(widget_uid)
    }

    pub fn register_remote_shared_object(
        &mut self,
        activity_id: XrActivityId,
        shared_object_id: XrSharedObjectId,
        epoch: u32,
        authority: XrPeerId,
        fidelity: XrSharedObjectFidelity,
        spawnable_object_id: XrSpawnableObjectId,
        pose: Pose,
        linvel: Vec3f,
        angvel: Vec3f,
    ) -> Option<WidgetUid> {
        if self.local_objects.contains_key(&shared_object_id) {
            return None;
        }
        let widget_uid =
            self.resolve_remote_widget_uid(activity_id, shared_object_id, spawnable_object_id)?;
        let state = XrNetSharedObjectState {
            seq: 0,
            sent_at: 0.0,
            physics_tick: 0,
            object_id: shared_object_id,
            epoch,
            authority,
            fidelity,
            mode: XrSharedObjectMode::Dynamic,
            pose,
            linvel,
            angvel,
        };
        let record = self
            .remote_objects
            .entry(shared_object_id)
            .or_insert_with(|| XrRemoteSharedObjectRecord {
                spawnable_object_id,
                widget_uid,
                epoch,
                authority,
                state_source_authority: authority,
                fidelity,
                latest_state: None,
                history: VecDeque::new(),
                pending_takeover_request_id: None,
                last_takeover_request_at: None,
                last_contact_impulse_at: None,
                pending_transfer: None,
            });
        record.spawnable_object_id = spawnable_object_id;
        record.widget_uid = widget_uid;
        record.epoch = epoch;
        record.authority = authority;
        record.state_source_authority = authority;
        record.fidelity = fidelity;
        record.latest_state = Some(state);
        push_shared_object_history(&mut record.history, state);
        Some(widget_uid)
    }

    pub fn record_remote_shared_object_state(
        &mut self,
        state: XrNetSharedObjectState,
    ) -> Option<WidgetUid> {
        let record = self.remote_objects.get_mut(&state.object_id)?;
        record.epoch = state.epoch;
        record.authority = state.authority;
        record.state_source_authority = state.authority;
        record.fidelity = state.fidelity;
        record.latest_state = Some(state);
        push_shared_object_history(&mut record.history, state);
        Some(record.widget_uid)
    }

    pub fn local_shared_object_snapshot(
        &self,
        object_id: XrSharedObjectId,
    ) -> Option<XrLocalSharedObjectSnapshot> {
        let record = self.local_objects.get(&object_id)?;
        Some(XrLocalSharedObjectSnapshot {
            object_id,
            spawnable_object_id: record.spawnable_object_id,
            widget_uid: record.widget_uid,
            epoch: record.epoch,
            authority: record.authority,
            fidelity: record.fidelity,
            latest_state: record.latest_state,
        })
    }

    pub fn remote_shared_object_snapshot(
        &self,
        object_id: XrSharedObjectId,
    ) -> Option<XrRemoteSharedObjectSnapshot> {
        let record = self.remote_objects.get(&object_id)?;
        Some(XrRemoteSharedObjectSnapshot {
            object_id,
            spawnable_object_id: record.spawnable_object_id,
            widget_uid: record.widget_uid,
            epoch: record.epoch,
            authority: record.authority,
            state_source_authority: record.state_source_authority,
            fidelity: record.fidelity,
            latest_state: record.latest_state,
            pending_takeover_request_id: record.pending_takeover_request_id,
        })
    }

    pub fn remote_shared_object_snapshot_for_widget(
        &self,
        widget_uid: WidgetUid,
    ) -> Option<XrRemoteSharedObjectSnapshot> {
        let object_id = self.remote_widget_to_object.get(&widget_uid).copied()?;
        self.remote_shared_object_snapshot(object_id)
    }

    pub fn remote_shared_object_snapshots(&self) -> Vec<XrRemoteSharedObjectSnapshot> {
        self.remote_objects
            .iter()
            .map(|(&object_id, record)| XrRemoteSharedObjectSnapshot {
                object_id,
                spawnable_object_id: record.spawnable_object_id,
                widget_uid: record.widget_uid,
                epoch: record.epoch,
                authority: record.authority,
                state_source_authority: record.state_source_authority,
                fidelity: record.fidelity,
                latest_state: record.latest_state,
                pending_takeover_request_id: record.pending_takeover_request_id,
            })
            .collect()
    }

    pub fn can_send_takeover_request(&self, object_id: XrSharedObjectId, now: f64) -> bool {
        let Some(record) = self.remote_objects.get(&object_id) else {
            return false;
        };
        record.pending_transfer.is_none()
            && record.pending_takeover_request_id.is_none()
            && record
                .last_takeover_request_at
                .is_none_or(|last| now - last >= XR_SHARED_OBJECT_TAKEOVER_REQUEST_COOLDOWN_SECONDS)
    }

    pub fn note_takeover_request(
        &mut self,
        object_id: XrSharedObjectId,
        request_id: u32,
        now: f64,
    ) -> bool {
        let Some(record) = self.remote_objects.get_mut(&object_id) else {
            return false;
        };
        record.pending_takeover_request_id = Some(request_id);
        record.last_takeover_request_at = Some(now);
        true
    }

    pub fn clear_takeover_request(
        &mut self,
        object_id: XrSharedObjectId,
        request_id: Option<u32>,
    ) -> bool {
        let Some(record) = self.remote_objects.get_mut(&object_id) else {
            return false;
        };
        if request_id.is_none() || record.pending_takeover_request_id == request_id {
            record.pending_takeover_request_id = None;
            return true;
        }
        false
    }

    pub fn can_send_contact_impulse_request(&self, object_id: XrSharedObjectId, now: f64) -> bool {
        let Some(record) = self.remote_objects.get(&object_id) else {
            return false;
        };
        record.pending_transfer.is_none()
            && record
                .last_contact_impulse_at
                .is_none_or(|last| now - last >= XR_SHARED_OBJECT_CONTACT_IMPULSE_COOLDOWN_SECONDS)
    }

    pub fn note_contact_impulse_request(&mut self, object_id: XrSharedObjectId, now: f64) -> bool {
        let Some(record) = self.remote_objects.get_mut(&object_id) else {
            return false;
        };
        record.last_contact_impulse_at = Some(now);
        true
    }

    pub fn find_local_state_for_request(
        &self,
        object_id: XrSharedObjectId,
        epoch: u32,
        based_on_seq: u32,
        based_on_tick: u32,
    ) -> Option<XrNetSharedObjectState> {
        let record = self.local_objects.get(&object_id)?;
        if record.epoch != epoch {
            return None;
        }
        record
            .history
            .iter()
            .rev()
            .copied()
            .find(|state| {
                state.epoch == epoch
                    && state_seq_not_after(state.seq, based_on_seq)
                    && state_tick_not_after(state.physics_tick, based_on_tick)
            })
            .or(record.latest_state)
    }

    pub fn schedule_authority_transfer(
        &mut self,
        object_id: XrSharedObjectId,
        epoch: u32,
        source_authority: XrPeerId,
        new_authority: XrPeerId,
        effective_at: f64,
        effective_tick: u32,
        request_id: u32,
        hand: Option<XrSharedHand>,
        pose: Pose,
        linvel: Vec3f,
        angvel: Vec3f,
    ) -> bool {
        let pending = XrPendingAuthorityTransfer {
            epoch,
            source_authority,
            new_authority,
            effective_at,
            effective_tick,
            request_id,
            hand,
            pose,
            linvel,
            angvel,
        };
        if let Some(record) = self.local_objects.get_mut(&object_id) {
            record.pending_transfer = Some(pending);
            return true;
        }
        if let Some(record) = self.remote_objects.get_mut(&object_id) {
            record.pending_transfer = Some(pending);
            record.pending_takeover_request_id = None;
            return true;
        }
        false
    }

    pub fn apply_scheduled_authority_transfers(
        &mut self,
        now: f64,
        current_tick: u32,
    ) -> Vec<XrScheduledAuthorityTransfer> {
        let local_peer_id = self.local_peer_id.unwrap_or_default();
        let local_due = self
            .local_objects
            .iter()
            .filter_map(|(&object_id, record)| {
                transfer_is_due(record.pending_transfer, now, current_tick).then_some(object_id)
            })
            .collect::<Vec<_>>();
        let remote_due = self
            .remote_objects
            .iter()
            .filter_map(|(&object_id, record)| {
                transfer_is_due(record.pending_transfer, now, current_tick).then_some(object_id)
            })
            .collect::<Vec<_>>();
        let mut transfers = Vec::with_capacity(local_due.len() + remote_due.len());

        for object_id in local_due {
            let Some(mut record) = self.local_objects.remove(&object_id) else {
                continue;
            };
            let Some(pending) = record.pending_transfer.take() else {
                self.local_objects.insert(object_id, record);
                continue;
            };
            self.local_shared_object_to_widget.remove(&object_id);
            self.widget_to_local_shared_object
                .remove(&record.widget_uid);
            if pending.new_authority == local_peer_id {
                record.epoch = pending.epoch;
                record.authority = local_peer_id;
                record.latest_state = Some(transfer_state(
                    object_id,
                    pending.epoch,
                    local_peer_id,
                    record.fidelity,
                    pending.pose,
                    pending.linvel,
                    pending.angvel,
                ));
                record.history.clear();
                if let Some(state) = record.latest_state {
                    push_shared_object_history(&mut record.history, state);
                }
                self.widget_to_local_shared_object
                    .insert(record.widget_uid, object_id);
                self.local_shared_object_to_widget
                    .insert(object_id, record.widget_uid);
                self.local_objects.insert(object_id, record.clone());
                transfers.push(XrScheduledAuthorityTransfer {
                    object_id,
                    widget_uid: record.widget_uid,
                    shadow: false,
                    source_authority: pending.source_authority,
                    new_authority: pending.new_authority,
                    epoch: pending.epoch,
                    request_id: pending.request_id,
                    hand: pending.hand,
                    pose: pending.pose,
                    linvel: pending.linvel,
                    angvel: pending.angvel,
                });
                continue;
            }

            self.remote_widget_to_object
                .insert(record.widget_uid, object_id);
            self.remote_object_to_widget
                .insert(object_id, record.widget_uid);
            let state = transfer_state(
                object_id,
                pending.epoch,
                pending.new_authority,
                record.fidelity,
                pending.pose,
                pending.linvel,
                pending.angvel,
            );
            let mut remote_record = XrRemoteSharedObjectRecord {
                spawnable_object_id: record.spawnable_object_id,
                widget_uid: record.widget_uid,
                epoch: pending.epoch,
                authority: pending.new_authority,
                state_source_authority: pending.source_authority,
                fidelity: record.fidelity,
                latest_state: Some(state),
                history: VecDeque::new(),
                pending_takeover_request_id: None,
                last_takeover_request_at: None,
                last_contact_impulse_at: None,
                pending_transfer: None,
            };
            push_shared_object_history(&mut remote_record.history, state);
            self.remote_objects.insert(object_id, remote_record);
            transfers.push(XrScheduledAuthorityTransfer {
                object_id,
                widget_uid: record.widget_uid,
                shadow: true,
                source_authority: pending.source_authority,
                new_authority: pending.new_authority,
                epoch: pending.epoch,
                request_id: pending.request_id,
                hand: pending.hand,
                pose: pending.pose,
                linvel: pending.linvel,
                angvel: pending.angvel,
            });
        }

        for object_id in remote_due {
            let Some(mut record) = self.remote_objects.remove(&object_id) else {
                continue;
            };
            let Some(pending) = record.pending_transfer.take() else {
                self.remote_objects.insert(object_id, record);
                continue;
            };
            let widget_uid = record.widget_uid;
            let spawnable_object_id = record.spawnable_object_id;
            let fidelity = record.fidelity;
            let state = transfer_state(
                object_id,
                pending.epoch,
                pending.new_authority,
                fidelity,
                pending.pose,
                pending.linvel,
                pending.angvel,
            );
            if pending.new_authority == local_peer_id {
                self.remote_object_to_widget.remove(&object_id);
                self.remote_widget_to_object.remove(&widget_uid);
                self.widget_to_local_shared_object
                    .insert(widget_uid, object_id);
                self.local_shared_object_to_widget
                    .insert(object_id, widget_uid);
                let mut local_record = XrLocalSharedObjectRecord {
                    spawnable_object_id,
                    widget_uid,
                    epoch: pending.epoch,
                    authority: local_peer_id,
                    fidelity,
                    latest_state: Some(state),
                    last_published_state: Some(state),
                    last_published_at: Some(state.sent_at),
                    history: VecDeque::new(),
                    pending_transfer: None,
                };
                push_shared_object_history(&mut local_record.history, state);
                self.local_objects.insert(object_id, local_record);
            } else {
                record.epoch = pending.epoch;
                record.authority = pending.new_authority;
                record.state_source_authority = pending.source_authority;
                record.latest_state = Some(state);
                record.pending_takeover_request_id = None;
                record.history.clear();
                push_shared_object_history(&mut record.history, state);
                self.remote_objects.insert(object_id, record);
            }
            transfers.push(XrScheduledAuthorityTransfer {
                object_id,
                widget_uid,
                shadow: pending.new_authority != local_peer_id,
                source_authority: pending.source_authority,
                new_authority: pending.new_authority,
                epoch: pending.epoch,
                request_id: pending.request_id,
                hand: pending.hand,
                pose: pending.pose,
                linvel: pending.linvel,
                angvel: pending.angvel,
            });
        }

        transfers
    }

    pub fn allocate_local_shared_object(
        &mut self,
        activity_id: XrActivityId,
        widget_uid: WidgetUid,
    ) -> Option<XrLocalSharedObjectAllocation> {
        if self.activity_id != Some(activity_id) {
            return None;
        }
        let local_peer_id = self.local_peer_id?;
        let spawnable_object_id = self.widget_to_object.get(&widget_uid).copied()?;
        if let Some(object_id) = self.widget_to_local_shared_object.get(&widget_uid).copied() {
            let record = self.local_objects.get_mut(&object_id)?;
            record.spawnable_object_id = spawnable_object_id;
            record.widget_uid = widget_uid;
            record.epoch = record.epoch.wrapping_add(1);
            record.authority = local_peer_id;
            record.latest_state = None;
            record.last_published_state = None;
            record.last_published_at = None;
            record.history.clear();
            record.pending_transfer = None;
            return Some(XrLocalSharedObjectAllocation {
                shared_object_id: object_id,
                spawnable_object_id,
                widget_uid,
                epoch: record.epoch,
                fidelity: record.fidelity,
            });
        }
        self.release_remote_widget_claim(widget_uid);
        let object_id = xr_make_shared_object_id(local_peer_id, self.next_shared_object_counter)?;
        self.next_shared_object_counter.0 = self.next_shared_object_counter.0.saturating_add(1);
        self.widget_to_local_shared_object
            .insert(widget_uid, object_id);
        self.local_shared_object_to_widget
            .insert(object_id, widget_uid);
        let record = XrLocalSharedObjectRecord {
            spawnable_object_id,
            widget_uid,
            epoch: 0,
            authority: local_peer_id,
            fidelity: XrSharedObjectFidelity::ImpactCritical,
            latest_state: None,
            last_published_state: None,
            last_published_at: None,
            history: VecDeque::new(),
            pending_transfer: None,
        };
        let epoch = record.epoch;
        let fidelity = record.fidelity;
        self.local_objects.insert(object_id, record);
        Some(XrLocalSharedObjectAllocation {
            shared_object_id: object_id,
            spawnable_object_id,
            widget_uid,
            epoch,
            fidelity,
        })
    }

    pub fn ensure_local_shared_object(
        &mut self,
        activity_id: XrActivityId,
        widget_uid: WidgetUid,
    ) -> Option<(XrLocalSharedObjectAllocation, bool)> {
        if self.activity_id != Some(activity_id) {
            return None;
        }
        let local_peer_id = self.local_peer_id?;
        let spawnable_object_id = self.widget_to_object.get(&widget_uid).copied()?;
        if let Some(object_id) = self.widget_to_local_shared_object.get(&widget_uid).copied() {
            let record = self.local_objects.get(&object_id)?;
            return Some((
                XrLocalSharedObjectAllocation {
                    shared_object_id: object_id,
                    spawnable_object_id: record.spawnable_object_id,
                    widget_uid: record.widget_uid,
                    epoch: record.epoch,
                    fidelity: record.fidelity,
                },
                false,
            ));
        }
        self.release_remote_widget_claim(widget_uid);
        let object_id = xr_make_shared_object_id(local_peer_id, self.next_shared_object_counter)?;
        self.next_shared_object_counter.0 = self.next_shared_object_counter.0.saturating_add(1);
        self.widget_to_local_shared_object
            .insert(widget_uid, object_id);
        self.local_shared_object_to_widget
            .insert(object_id, widget_uid);
        let record = XrLocalSharedObjectRecord {
            spawnable_object_id,
            widget_uid,
            epoch: 0,
            authority: local_peer_id,
            fidelity: XrSharedObjectFidelity::ImpactCritical,
            latest_state: None,
            last_published_state: None,
            last_published_at: None,
            history: VecDeque::new(),
            pending_transfer: None,
        };
        let epoch = record.epoch;
        let fidelity = record.fidelity;
        self.local_objects.insert(object_id, record);
        Some((
            XrLocalSharedObjectAllocation {
                shared_object_id: object_id,
                spawnable_object_id,
                widget_uid,
                epoch,
                fidelity,
            },
            true,
        ))
    }

    pub fn local_shared_object_states(
        &mut self,
        runtime_bodies: &HashMap<WidgetUid, XrRuntimeBodyState>,
        sent_at: f64,
        physics_tick: u32,
        authority: XrPeerId,
    ) -> Vec<XrNetSharedObjectState> {
        let mut states = Vec::with_capacity(self.local_objects.len());
        for (&object_id, record) in &mut self.local_objects {
            let Some(body) = runtime_bodies.get(&record.widget_uid) else {
                continue;
            };
            let state = XrNetSharedObjectState {
                seq: 0,
                sent_at,
                physics_tick,
                object_id,
                epoch: record.epoch,
                authority,
                fidelity: record.fidelity,
                mode: if let Some(hand) = body.held_by {
                    XrSharedObjectMode::ContactDominated { authority, hand }
                } else if body.sleeping {
                    XrSharedObjectMode::Sleeping
                } else {
                    XrSharedObjectMode::Dynamic
                },
                pose: body.pose,
                linvel: body.linvel,
                angvel: body.angvel,
            };
            record.authority = authority;
            record.latest_state = Some(state);
            if should_publish_local_shared_object_state(record, state, sent_at) {
                states.push(state);
            }
        }
        states
    }

    pub fn note_published_local_shared_object_states(
        &mut self,
        states: &[XrNetSharedObjectState],
    ) {
        for state in states {
            let Some(record) = self.local_objects.get_mut(&state.object_id) else {
                continue;
            };
            record.epoch = state.epoch;
            record.authority = state.authority;
            record.fidelity = state.fidelity;
            record.latest_state = Some(*state);
            record.last_published_state = Some(*state);
            record.last_published_at = Some(state.sent_at);
            push_shared_object_history(&mut record.history, *state);
        }
    }

    pub fn remote_shared_object_history(
        &self,
        object_id: XrSharedObjectId,
    ) -> Vec<XrNetSharedObjectState> {
        self.remote_objects
            .get(&object_id)
            .map(|record| record.history.iter().copied().collect())
            .unwrap_or_default()
    }

    pub fn prune_missing_local_shared_objects(
        &mut self,
        runtime_bodies: &HashMap<WidgetUid, XrRuntimeBodyState>,
    ) -> Vec<(XrSharedObjectId, u32, WidgetUid)> {
        let mut missing = Vec::new();
        for (&object_id, record) in &self.local_objects {
            if !runtime_bodies.contains_key(&record.widget_uid) {
                missing.push((object_id, record.epoch, record.widget_uid));
            }
        }
        for (object_id, _, widget_uid) in &missing {
            self.local_objects.remove(object_id);
            self.local_shared_object_to_widget.remove(object_id);
            self.widget_to_local_shared_object.remove(widget_uid);
        }
        missing
    }
}

fn push_shared_object_history(
    history: &mut VecDeque<XrNetSharedObjectState>,
    state: XrNetSharedObjectState,
) {
    if history
        .back()
        .is_some_and(|previous| previous.epoch != state.epoch)
    {
        history.clear();
    }
    history.push_back(state);
    while history.len() > XR_SHARED_OBJECT_HISTORY_MAX_SAMPLES {
        history.pop_front();
    }
    while history.front().is_some_and(|front| {
        state.sent_at != 0.0
            && front.sent_at != 0.0
            && state.sent_at - front.sent_at > XR_SHARED_OBJECT_HISTORY_MAX_SECONDS
    }) {
        history.pop_front();
    }
}

fn pose_publish_delta_exceeded(previous: Pose, next: Pose) -> bool {
    (next.position - previous.position).length() > XR_SHARED_OBJECT_PUBLISH_POSITION_EPSILON_METERS
        || previous.orientation.get_angle_with(next.orientation)
            > XR_SHARED_OBJECT_PUBLISH_ORIENTATION_EPSILON_DEGREES
}

fn velocity_publish_delta_exceeded(previous: Vec3f, next: Vec3f, epsilon: f32) -> bool {
    (next - previous).length() > epsilon
}

fn should_publish_local_shared_object_state(
    record: &XrLocalSharedObjectRecord,
    state: XrNetSharedObjectState,
    sent_at: f64,
) -> bool {
    let Some(previous) = record.last_published_state else {
        return true;
    };
    if previous.epoch != state.epoch
        || previous.authority != state.authority
        || previous.fidelity != state.fidelity
        || previous.mode != state.mode
    {
        return true;
    }
    if pose_publish_delta_exceeded(previous.pose, state.pose)
        || velocity_publish_delta_exceeded(
            previous.linvel,
            state.linvel,
            XR_SHARED_OBJECT_PUBLISH_LINVEL_EPSILON_MPS,
        )
        || velocity_publish_delta_exceeded(
            previous.angvel,
            state.angvel,
            XR_SHARED_OBJECT_PUBLISH_ANGVEL_EPSILON_RADPS,
        )
    {
        return true;
    }
    matches!(state.mode, XrSharedObjectMode::Sleeping)
        && record.last_published_at.is_none_or(|last_published_at| {
            sent_at - last_published_at >= XR_SHARED_OBJECT_SLEEPING_PUBLISH_INTERVAL_SECONDS
        })
}

fn state_seq_not_after(history_seq: u32, requested_seq: u32) -> bool {
    history_seq == requested_seq || requested_seq.wrapping_sub(history_seq) < (u32::MAX / 2)
}

fn state_tick_not_after(history_tick: u32, requested_tick: u32) -> bool {
    history_tick == requested_tick || requested_tick.wrapping_sub(history_tick) < (u32::MAX / 2)
}

fn transfer_is_due(
    transfer: Option<XrPendingAuthorityTransfer>,
    now: f64,
    current_tick: u32,
) -> bool {
    transfer.is_some_and(|pending| {
        now >= pending.effective_at || state_tick_not_after(pending.effective_tick, current_tick)
    })
}

fn transfer_state(
    object_id: XrSharedObjectId,
    epoch: u32,
    authority: XrPeerId,
    fidelity: XrSharedObjectFidelity,
    pose: Pose,
    linvel: Vec3f,
    angvel: Vec3f,
) -> XrNetSharedObjectState {
    XrNetSharedObjectState {
        seq: 0,
        sent_at: 0.0,
        physics_tick: 0,
        object_id,
        epoch,
        authority,
        fidelity,
        mode: XrSharedObjectMode::Dynamic,
        pose,
        linvel,
        angvel,
    }
}

pub fn collect_scene_spawnable_objects(
    activity_id: XrActivityId,
    root: &WidgetRef,
) -> Vec<XrSpawnableObjectBinding> {
    let mut bindings = Vec::new();
    let root_hash = root_path_hash(activity_id);
    collect_widget_spawnables(root, root_hash, root_hash, &mut bindings);
    bindings
}

fn collect_widget_spawnables(
    widget: &WidgetRef,
    path_hash: u64,
    parent_hash: u64,
    bindings: &mut Vec<XrSpawnableObjectBinding>,
) {
    if !widget.visible() {
        return;
    }

    if xr_widget_with_scene_node(widget, |node| {
        if node.body_kind() == XrBodyKind::Dynamic {
            let object_id = XrSpawnableObjectId(non_zero_hash(hash_u64(
                path_hash,
                XR_SHARED_OBJECT_BODY_TAG,
            )));
            let allocation_group_id = if node.spawn_pool() {
                XrSpawnableObjectId(non_zero_hash(hash_u64(
                    parent_hash,
                    XR_SHARED_OBJECT_POOL_GROUP_TAG,
                )))
            } else {
                object_id
            };
            bindings.push(XrSpawnableObjectBinding {
                object_id,
                allocation_group_id,
                widget_uid: widget.widget_uid(),
            });
        }
        let mut child_index = 0usize;
        xr_widget_children(widget, &mut |child_id, child| {
            let child_hash = child_path_hash(path_hash, child_id, child_index);
            child_index = child_index.saturating_add(1);
            collect_widget_spawnables(&child, child_hash, path_hash, bindings);
        });
    })
    .is_some()
    {
        return;
    }

    let mut child_index = 0usize;
    xr_widget_children(widget, &mut |child_id, child| {
        let child_hash = child_path_hash(path_hash, child_id, child_index);
        child_index = child_index.saturating_add(1);
        collect_widget_spawnables(&child, child_hash, path_hash, bindings);
    });
}

fn root_path_hash(activity_id: XrActivityId) -> u64 {
    hash_u64(
        hash_u64(XR_SHARED_OBJECT_HASH_OFFSET, XR_SHARED_OBJECT_ROOT_TAG),
        activity_id.to_live_id().0,
    )
}

fn child_path_hash(parent_hash: u64, child_id: LiveId, child_index: usize) -> u64 {
    let hash = hash_u64(parent_hash, XR_SHARED_OBJECT_CHILD_TAG);
    let hash = hash_u64(hash, child_id.0);
    let hash = hash_u64(hash, XR_SHARED_OBJECT_INDEX_TAG);
    hash_u64(hash, child_index as u64)
}

fn hash_u64(hash: u64, value: u64) -> u64 {
    (hash ^ value).wrapping_mul(XR_SHARED_OBJECT_HASH_PRIME)
}

fn non_zero_hash(hash: u64) -> u64 {
    if hash == 0 {
        1
    } else {
        hash
    }
}

#[cfg(test)]
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
                },
                XrSpawnableObjectBinding {
                    object_id: XrSpawnableObjectId(12),
                    allocation_group_id: XrSpawnableObjectId(91),
                    widget_uid: WidgetUid(102),
                },
            ],
        );

        let left = registry.resolve_remote_widget_uid(
            activity_id,
            xr_make_shared_object_id(XrPeerId(1), XrSharedObjectCounter(0)).unwrap(),
            XrSpawnableObjectId(11),
        );
        let right = registry.resolve_remote_widget_uid(
            activity_id,
            xr_make_shared_object_id(XrPeerId(2), XrSharedObjectCounter(0)).unwrap(),
            XrSpawnableObjectId(11),
        );

        assert_eq!(left, Some(WidgetUid(101)));
        assert_eq!(right, Some(WidgetUid(102)));
    }

    #[test]
    fn local_allocations_use_packed_shared_object_ids() {
        let activity_id = XrActivityId(live_id!(ico_shoot_scene));
        let mut registry = XrSharedObjectRegistry::default();
        registry.set_local_peer_id(XrPeerId(55));
        registry.replace_spawnables(
            activity_id,
            [XrSpawnableObjectBinding {
                object_id: XrSpawnableObjectId(11),
                allocation_group_id: XrSpawnableObjectId(91),
                widget_uid: WidgetUid(101),
            }],
        );

        let allocation = registry
            .allocate_local_shared_object(activity_id, WidgetUid(101))
            .expect("local shared-object allocation should succeed");
        assert_eq!(
            xr_shared_object_peer_id(allocation.shared_object_id),
            XrPeerId(55)
        );
        assert_eq!(allocation.spawnable_object_id, XrSpawnableObjectId(11));
        assert_eq!(allocation.epoch, 0);
    }

    #[test]
    fn pooled_local_allocations_reuse_object_id_and_advance_epoch() {
        let activity_id = XrActivityId(live_id!(ico_shoot_scene));
        let mut registry = XrSharedObjectRegistry::default();
        registry.set_local_peer_id(XrPeerId(55));
        registry.replace_spawnables(
            activity_id,
            [XrSpawnableObjectBinding {
                object_id: XrSpawnableObjectId(11),
                allocation_group_id: XrSpawnableObjectId(91),
                widget_uid: WidgetUid(101),
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
    fn prune_missing_local_shared_objects_returns_despawns() {
        let activity_id = XrActivityId(live_id!(ico_shoot_scene));
        let mut registry = XrSharedObjectRegistry::default();
        registry.set_local_peer_id(XrPeerId(55));
        registry.replace_spawnables(
            activity_id,
            [XrSpawnableObjectBinding {
                object_id: XrSpawnableObjectId(11),
                allocation_group_id: XrSpawnableObjectId(91),
                widget_uid: WidgetUid(101),
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
        registry.set_local_peer_id(XrPeerId(55));
        registry.replace_spawnables(
            activity_id,
            [XrSpawnableObjectBinding {
                object_id: XrSpawnableObjectId(11),
                allocation_group_id: XrSpawnableObjectId(91),
                widget_uid: WidgetUid(101),
            }],
        );

        let object_id = xr_make_shared_object_id(XrPeerId(7), XrSharedObjectCounter(3)).unwrap();
        let widget_uid = registry
            .register_remote_shared_object(
                activity_id,
                object_id,
                0,
                XrPeerId(7),
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
            XrPeerId(7),
            XrPeerId(55),
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
        assert_eq!(local.authority, XrPeerId(55));
    }

    #[test]
    fn scheduled_handoff_demotes_local_object_to_remote_shadow() {
        let activity_id = XrActivityId(live_id!(ico_shoot_scene));
        let mut registry = XrSharedObjectRegistry::default();
        registry.set_local_peer_id(XrPeerId(55));
        registry.replace_spawnables(
            activity_id,
            [XrSpawnableObjectBinding {
                object_id: XrSpawnableObjectId(11),
                allocation_group_id: XrSpawnableObjectId(91),
                widget_uid: WidgetUid(101),
            }],
        );

        let allocation = registry
            .allocate_local_shared_object(activity_id, WidgetUid(101))
            .expect("local allocation should succeed");
        assert!(registry.schedule_authority_transfer(
            allocation.shared_object_id,
            allocation.epoch.wrapping_add(1),
            XrPeerId(55),
            XrPeerId(8),
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
        assert_eq!(transfers[0].source_authority, XrPeerId(55));
        assert!(registry
            .local_shared_object_snapshot(allocation.shared_object_id)
            .is_none());
        let remote = registry
            .remote_shared_object_snapshot(allocation.shared_object_id)
            .expect("handoff should preserve the object id as a remote shadow");
        assert_eq!(remote.widget_uid, WidgetUid(101));
        assert_eq!(remote.authority, XrPeerId(8));
        assert_eq!(remote.state_source_authority, XrPeerId(55));
    }
}
