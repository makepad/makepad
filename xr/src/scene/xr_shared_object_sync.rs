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
    pub bootstrap_shared: bool,
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
    pub authority: XrNetPeerId,
    pub fidelity: XrSharedObjectFidelity,
    pub latest_state: Option<XrNetSharedObjectState>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct XrRemoteSharedObjectSnapshot {
    pub object_id: XrSharedObjectId,
    pub spawnable_object_id: XrSpawnableObjectId,
    pub widget_uid: WidgetUid,
    pub epoch: u32,
    pub authority: XrNetPeerId,
    pub state_source_authority: XrNetPeerId,
    pub fidelity: XrSharedObjectFidelity,
    pub latest_state: Option<XrNetSharedObjectState>,
    pub pending_takeover_request_id: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct XrScheduledAuthorityTransfer {
    pub object_id: XrSharedObjectId,
    pub widget_uid: WidgetUid,
    pub shadow: bool,
    pub source_authority: XrNetPeerId,
    pub new_authority: XrNetPeerId,
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
    source_authority: XrNetPeerId,
    new_authority: XrNetPeerId,
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
    authority: XrNetPeerId,
    fidelity: XrSharedObjectFidelity,
    activated_at_local_time: f64,
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
    authority: XrNetPeerId,
    state_source_authority: XrNetPeerId,
    fidelity: XrSharedObjectFidelity,
    activated_at_local_time: f64,
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
    local_peer_id: Option<XrNetPeerId>,
    next_shared_object_counter: XrSharedObjectCounter,
    object_to_widget: HashMap<XrSpawnableObjectId, WidgetUid>,
    object_to_group: HashMap<XrSpawnableObjectId, XrSpawnableObjectId>,
    object_bootstrap_shared: HashMap<XrSpawnableObjectId, bool>,
    widget_to_object: HashMap<WidgetUid, XrSpawnableObjectId>,
    widget_bootstrap_shared: HashMap<WidgetUid, bool>,
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

    fn clear_active_shared_objects(&mut self) {
        self.local_objects.clear();
        self.local_shared_object_to_widget.clear();
        self.widget_to_local_shared_object.clear();
        self.remote_objects.clear();
        self.remote_object_to_widget.clear();
        self.remote_widget_to_object.clear();
    }

    fn rebuild_spawnable_bindings<I>(&mut self, bindings: I)
    where
        I: IntoIterator<Item = XrSpawnableObjectBinding>,
    {
        self.object_to_widget.clear();
        self.object_to_group.clear();
        self.object_bootstrap_shared.clear();
        self.widget_to_object.clear();
        self.widget_bootstrap_shared.clear();
        self.group_to_widgets.clear();
        for binding in bindings {
            self.object_to_widget
                .insert(binding.object_id, binding.widget_uid);
            self.object_to_group
                .insert(binding.object_id, binding.allocation_group_id);
            self.object_bootstrap_shared
                .insert(binding.object_id, binding.bootstrap_shared);
            self.widget_to_object
                .insert(binding.widget_uid, binding.object_id);
            self.widget_bootstrap_shared
                .insert(binding.widget_uid, binding.bootstrap_shared);
            self.group_to_widgets
                .entry(binding.allocation_group_id)
                .or_default()
                .push(binding.widget_uid);
        }
    }

    fn remap_local_shared_objects(&mut self) {
        let local_objects = std::mem::take(&mut self.local_objects);
        self.local_shared_object_to_widget.clear();
        self.widget_to_local_shared_object.clear();
        for (object_id, mut record) in local_objects {
            let Some(&widget_uid) = self.object_to_widget.get(&record.spawnable_object_id) else {
                continue;
            };
            record.widget_uid = widget_uid;
            self.widget_to_local_shared_object
                .insert(widget_uid, object_id);
            self.local_shared_object_to_widget
                .insert(object_id, widget_uid);
            self.local_objects.insert(object_id, record);
        }
    }

    fn remap_remote_shared_objects(&mut self, activity_id: XrActivityId) {
        let mut remote_objects = std::mem::take(&mut self.remote_objects);
        let mut object_ids = remote_objects.keys().copied().collect::<Vec<_>>();
        object_ids.sort_by_key(|object_id| object_id.0);
        self.remote_object_to_widget.clear();
        self.remote_widget_to_object.clear();
        for object_id in object_ids {
            let Some(mut record) = remote_objects.remove(&object_id) else {
                continue;
            };
            let Some(widget_uid) =
                self.resolve_remote_widget_uid(activity_id, object_id, record.spawnable_object_id)
            else {
                continue;
            };
            record.widget_uid = widget_uid;
            self.remote_objects.insert(object_id, record);
        }
    }

    fn remote_state_sender_matches(
        record: &XrRemoteSharedObjectRecord,
        sender_authority: XrNetPeerId,
        state: XrNetSharedObjectState,
    ) -> bool {
        sender_authority == record.authority
            && state.authority == record.authority
            && state.epoch == record.epoch
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

    pub fn local_peer_id(&self) -> Option<XrNetPeerId> {
        self.local_peer_id
    }

    pub fn set_local_peer_id(&mut self, peer_id: XrNetPeerId) {
        self.local_peer_id = Some(peer_id);
    }

    pub fn clear(&mut self) {
        self.activity_id = None;
        self.object_to_widget.clear();
        self.object_to_group.clear();
        self.object_bootstrap_shared.clear();
        self.widget_to_object.clear();
        self.widget_bootstrap_shared.clear();
        self.group_to_widgets.clear();
        self.clear_active_shared_objects();
    }

    pub fn replace_spawnables<I>(&mut self, activity_id: XrActivityId, bindings: I)
    where
        I: IntoIterator<Item = XrSpawnableObjectBinding>,
    {
        let same_activity = self.activity_id == Some(activity_id);
        self.activity_id = Some(activity_id);
        self.rebuild_spawnable_bindings(bindings);
        if same_activity {
            self.remap_local_shared_objects();
            self.remap_remote_shared_objects(activity_id);
        } else {
            self.clear_active_shared_objects();
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

    pub fn bootstrap_shared_candidates(&self) -> Vec<(XrSpawnableObjectId, WidgetUid)> {
        let mut candidates = self
            .object_to_widget
            .iter()
            .filter_map(|(&object_id, &widget_uid)| {
                self.object_bootstrap_shared
                    .get(&object_id)
                    .copied()
                    .unwrap_or(false)
                    .then_some((object_id, widget_uid))
            })
            .collect::<Vec<_>>();
        candidates.sort_by_key(|(object_id, _)| object_id.0);
        candidates
    }

    fn spawnable_group_widgets(&self, widget_uid: WidgetUid) -> Option<&Vec<WidgetUid>> {
        let object_id = self.widget_to_object.get(&widget_uid).copied()?;
        let group_id = self.object_to_group.get(&object_id).copied()?;
        self.group_to_widgets.get(&group_id)
    }

    fn widget_is_local(&self, widget_uid: WidgetUid) -> bool {
        self.widget_to_local_shared_object.contains_key(&widget_uid)
    }

    fn widget_is_remote(&self, widget_uid: WidgetUid) -> bool {
        self.remote_widget_to_object.contains_key(&widget_uid)
    }

    fn widget_activation_time(&self, widget_uid: WidgetUid) -> Option<f64> {
        if let Some(object_id) = self.widget_to_local_shared_object.get(&widget_uid).copied() {
            return self
                .local_objects
                .get(&object_id)
                .map(|record| record.activated_at_local_time);
        }
        if let Some(object_id) = self.remote_widget_to_object.get(&widget_uid).copied() {
            return self
                .remote_objects
                .get(&object_id)
                .map(|record| record.activated_at_local_time);
        }
        None
    }

    fn first_free_group_widget(&self, group_widgets: &[WidgetUid]) -> Option<WidgetUid> {
        group_widgets
            .iter()
            .copied()
            .find(|widget_uid| !self.widget_is_local(*widget_uid) && !self.widget_is_remote(*widget_uid))
    }

    fn oldest_group_widget(
        &self,
        group_widgets: &[WidgetUid],
        allow_local: bool,
        allow_remote: bool,
    ) -> Option<WidgetUid> {
        let mut best = None::<(f64, usize, WidgetUid)>;
        for (index, &widget_uid) in group_widgets.iter().enumerate() {
            let eligible = (allow_local && self.widget_is_local(widget_uid))
                || (allow_remote && self.widget_is_remote(widget_uid));
            if !eligible {
                continue;
            }
            let Some(activated_at) = self.widget_activation_time(widget_uid) else {
                continue;
            };
            if best.is_none_or(|current| (activated_at, index) < (current.0, current.1)) {
                best = Some((activated_at, index, widget_uid));
            }
        }
        best.map(|(_, _, widget_uid)| widget_uid)
    }

    fn select_local_spawn_widget_uid(
        &self,
        activity_id: XrActivityId,
        preferred_widget_uid: WidgetUid,
    ) -> Option<WidgetUid> {
        if self.activity_id != Some(activity_id) {
            return None;
        }
        let group_widgets = self.spawnable_group_widgets(preferred_widget_uid)?;
        if !self.widget_is_local(preferred_widget_uid) && !self.widget_is_remote(preferred_widget_uid) {
            return Some(preferred_widget_uid);
        }
        self.first_free_group_widget(group_widgets)
            .or_else(|| self.oldest_group_widget(group_widgets, true, true))
    }

    fn select_remote_spawn_widget_uid(
        &self,
        activity_id: XrActivityId,
        preferred_widget_uid: WidgetUid,
    ) -> Option<WidgetUid> {
        if self.activity_id != Some(activity_id) {
            return None;
        }
        let group_widgets = self.spawnable_group_widgets(preferred_widget_uid)?;
        if !self.widget_is_local(preferred_widget_uid) && !self.widget_is_remote(preferred_widget_uid) {
            return Some(preferred_widget_uid);
        }
        self.first_free_group_widget(group_widgets)
            .or_else(|| self.oldest_group_widget(group_widgets, false, true))
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
        peer_id: XrNetPeerId,
    ) -> Vec<WidgetUid> {
        let object_ids = self
            .remote_objects
            .iter()
            .filter_map(|(&object_id, record)| (record.authority == peer_id).then_some(object_id))
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
        let widget_uid = self.select_remote_spawn_widget_uid(activity_id, preferred_widget)?;
        if let Some(previous_key) = self
            .remote_widget_to_object
            .insert(widget_uid, shared_object_id)
        {
            self.remote_object_to_widget.remove(&previous_key);
            self.remote_objects.remove(&previous_key);
        }
        self.remote_object_to_widget
            .insert(shared_object_id, widget_uid);
        Some(widget_uid)
    }

    pub fn register_remote_shared_object(
        &mut self,
        activity_id: XrActivityId,
        activated_at_local_time: f64,
        shared_object_id: XrSharedObjectId,
        epoch: u32,
        authority: XrNetPeerId,
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
                activated_at_local_time,
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
        record.activated_at_local_time = activated_at_local_time;
        record.latest_state = Some(state);
        push_shared_object_history(&mut record.history, state);
        Some(widget_uid)
    }

    pub fn prepare_local_spawn_allocation(
        &mut self,
        activity_id: XrActivityId,
        preferred_widget_uid: WidgetUid,
        activated_at_local_time: f64,
        pose: Pose,
        linvel: Vec3f,
        angvel: Vec3f,
    ) -> Option<(XrLocalSharedObjectAllocation, bool)> {
        if self.activity_id != Some(activity_id) {
            return None;
        }
        let widget_uid = self.select_local_spawn_widget_uid(activity_id, preferred_widget_uid)?;
        let reused_remote = self.remote_widget_to_object.contains_key(&widget_uid);
        let allocation = self.force_local_shared_object_reset(
            activity_id,
            widget_uid,
            activated_at_local_time,
            pose,
            linvel,
            angvel,
        )?;
        Some((allocation, reused_remote))
    }

    pub fn record_remote_shared_object_state(
        &mut self,
        sender_authority: XrNetPeerId,
        state: XrNetSharedObjectState,
    ) -> Option<WidgetUid> {
        let record = self.remote_objects.get_mut(&state.object_id)?;
        if !Self::remote_state_sender_matches(record, sender_authority, state) {
            return None;
        }
        record.epoch = state.epoch;
        record.authority = sender_authority;
        record.state_source_authority = sender_authority;
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

    pub fn local_shared_object_snapshots(&self) -> Vec<XrLocalSharedObjectSnapshot> {
        let mut object_ids = self.local_objects.keys().copied().collect::<Vec<_>>();
        object_ids.sort_by_key(|object_id| object_id.0);
        object_ids
            .into_iter()
            .filter_map(|object_id| self.local_shared_object_snapshot(object_id))
            .collect()
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
        source_authority: XrNetPeerId,
        new_authority: XrNetPeerId,
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
                activated_at_local_time: record.activated_at_local_time,
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
                    activated_at_local_time: record.activated_at_local_time,
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

    pub fn force_local_shared_object_reset(
        &mut self,
        activity_id: XrActivityId,
        widget_uid: WidgetUid,
        activated_at_local_time: f64,
        pose: Pose,
        linvel: Vec3f,
        angvel: Vec3f,
    ) -> Option<XrLocalSharedObjectAllocation> {
        if self.activity_id != Some(activity_id) {
            return None;
        }
        let local_peer_id = self.local_peer_id?;
        if let Some(object_id) = self.widget_to_local_shared_object.get(&widget_uid).copied() {
            let record = self.local_objects.get_mut(&object_id)?;
            record.epoch = record.epoch.wrapping_add(1);
            record.authority = local_peer_id;
            record.latest_state = Some(transfer_state(
                object_id,
                record.epoch,
                local_peer_id,
                record.fidelity,
                pose,
                linvel,
                angvel,
            ));
            record.activated_at_local_time = activated_at_local_time;
            record.last_published_state = None;
            record.last_published_at = None;
            record.history.clear();
            record.pending_transfer = None;
            return Some(XrLocalSharedObjectAllocation {
                shared_object_id: object_id,
                spawnable_object_id: record.spawnable_object_id,
                widget_uid: record.widget_uid,
                epoch: record.epoch,
                fidelity: record.fidelity,
            });
        }
        if let Some(object_id) = self.remote_widget_to_object.remove(&widget_uid) {
            self.remote_object_to_widget.remove(&object_id);
            let remote = self.remote_objects.remove(&object_id)?;
            self.widget_to_local_shared_object
                .insert(widget_uid, object_id);
            self.local_shared_object_to_widget
                .insert(object_id, widget_uid);
            let epoch = remote.epoch.wrapping_add(1);
            self.local_objects.insert(
                object_id,
                XrLocalSharedObjectRecord {
                    spawnable_object_id: remote.spawnable_object_id,
                    widget_uid,
                    epoch,
                    authority: local_peer_id,
                    fidelity: remote.fidelity,
                    activated_at_local_time,
                    latest_state: Some(transfer_state(
                        object_id,
                        epoch,
                        local_peer_id,
                        remote.fidelity,
                        pose,
                        linvel,
                        angvel,
                    )),
                    last_published_state: None,
                    last_published_at: None,
                    history: VecDeque::new(),
                    pending_transfer: None,
                },
            );
            return self
                .local_shared_object_snapshot(object_id)
                .map(|snapshot| XrLocalSharedObjectAllocation {
                    shared_object_id: snapshot.object_id,
                    spawnable_object_id: snapshot.spawnable_object_id,
                    widget_uid: snapshot.widget_uid,
                    epoch: snapshot.epoch,
                    fidelity: snapshot.fidelity,
                });
        }
        let (allocation, _) = self.ensure_local_shared_object(activity_id, widget_uid)?;
        let record = self.local_objects.get_mut(&allocation.shared_object_id)?;
        record.authority = local_peer_id;
        record.activated_at_local_time = activated_at_local_time;
        record.latest_state = Some(transfer_state(
            allocation.shared_object_id,
            allocation.epoch,
            local_peer_id,
            record.fidelity,
            pose,
            linvel,
            angvel,
        ));
        record.last_published_state = None;
        record.last_published_at = None;
        record.history.clear();
        record.pending_transfer = None;
        Some(XrLocalSharedObjectAllocation {
            shared_object_id: allocation.shared_object_id,
            spawnable_object_id: allocation.spawnable_object_id,
            widget_uid: allocation.widget_uid,
            epoch: allocation.epoch,
            fidelity: allocation.fidelity,
        })
    }

    pub fn apply_remote_shared_object_reset(
        &mut self,
        activity_id: XrActivityId,
        activated_at_local_time: f64,
        source_authority: XrNetPeerId,
        object_id: XrSharedObjectId,
        epoch: u32,
        pose: Pose,
        linvel: Vec3f,
        angvel: Vec3f,
    ) -> Option<WidgetUid> {
        if self.activity_id != Some(activity_id) {
            return None;
        }
        if let Some(record) = self.local_objects.remove(&object_id) {
            self.local_shared_object_to_widget.remove(&object_id);
            self.widget_to_local_shared_object
                .remove(&record.widget_uid);
            let widget_uid = record.widget_uid;
            self.remote_widget_to_object.insert(widget_uid, object_id);
            self.remote_object_to_widget.insert(object_id, widget_uid);
            let state = transfer_state(
                object_id,
                epoch,
                source_authority,
                record.fidelity,
                pose,
                linvel,
                angvel,
            );
            let mut remote_record = XrRemoteSharedObjectRecord {
                spawnable_object_id: record.spawnable_object_id,
                widget_uid,
                epoch,
                authority: source_authority,
                state_source_authority: source_authority,
                fidelity: record.fidelity,
                activated_at_local_time,
                latest_state: Some(state),
                history: VecDeque::new(),
                pending_takeover_request_id: None,
                last_takeover_request_at: None,
                last_contact_impulse_at: None,
                pending_transfer: None,
            };
            push_shared_object_history(&mut remote_record.history, state);
            self.remote_objects.insert(object_id, remote_record);
            return Some(widget_uid);
        }
        let record = self.remote_objects.get_mut(&object_id)?;
        let state = transfer_state(
            object_id,
            epoch,
            source_authority,
            record.fidelity,
            pose,
            linvel,
            angvel,
        );
        record.epoch = epoch;
        record.authority = source_authority;
        record.state_source_authority = source_authority;
        record.latest_state = Some(state);
        record.activated_at_local_time = activated_at_local_time;
        record.pending_takeover_request_id = None;
        record.pending_transfer = None;
        record.history.clear();
        push_shared_object_history(&mut record.history, state);
        Some(record.widget_uid)
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
            activated_at_local_time: 0.0,
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
            activated_at_local_time: 0.0,
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
        authority: XrNetPeerId,
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

    pub fn note_published_local_shared_object_states(&mut self, states: &[XrNetSharedObjectState]) {
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
    authority: XrNetPeerId,
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
                bootstrap_shared: node.bootstrap_shared(),
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
