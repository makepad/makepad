use super::{XrBodyKind, XrNode};
use crate::prelude::*;
use std::collections::HashMap;

const XR_SHARED_OBJECT_HASH_OFFSET: u64 = 0xcbf29ce484222325;
const XR_SHARED_OBJECT_HASH_PRIME: u64 = 0x00000100000001b3;
const XR_SHARED_OBJECT_ROOT_TAG: u64 = 0x78725f7368617265;
const XR_SHARED_OBJECT_CHILD_TAG: u64 = 0x6368696c645f7061;
const XR_SHARED_OBJECT_INDEX_TAG: u64 = 0x696e6465785f7061;
const XR_SHARED_OBJECT_BODY_TAG: u64 = 0x626f64795f737061;
const XR_SHARED_OBJECT_POOL_GROUP_TAG: u64 = 0x706f6f6c5f677270;

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
    local_shared_object_to_widget: HashMap<XrSharedObjectId, WidgetUid>,
    widget_to_local_shared_object: HashMap<WidgetUid, XrSharedObjectId>,
    remote_object_to_widget: HashMap<XrSharedObjectId, WidgetUid>,
    remote_widget_to_object: HashMap<WidgetUid, XrSharedObjectId>,
}

impl XrSharedObjectRegistry {
    pub fn activity_id(&self) -> Option<XrActivityId> {
        self.activity_id
    }

    pub fn len(&self) -> usize {
        self.object_to_widget.len()
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
        self.local_shared_object_to_widget.clear();
        self.widget_to_local_shared_object.clear();
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
        self.local_shared_object_to_widget.clear();
        self.widget_to_local_shared_object.clear();
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
        let object_id = xr_make_shared_object_id(local_peer_id, self.next_shared_object_counter)?;
        self.next_shared_object_counter.0 = self.next_shared_object_counter.0.saturating_add(1);
        if let Some(previous_object_id) = self
            .widget_to_local_shared_object
            .insert(widget_uid, object_id)
        {
            self.local_shared_object_to_widget
                .remove(&previous_object_id);
        }
        self.local_shared_object_to_widget
            .insert(object_id, widget_uid);
        Some(XrLocalSharedObjectAllocation {
            shared_object_id: object_id,
            spawnable_object_id,
            widget_uid,
        })
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

    if let Some(node) = widget.cast_inner::<XrNode>() {
        if node.body_kind() == XrBodyKind::Dynamic {
            let object_id = XrSpawnableObjectId(non_zero_hash(hash_u64(
                path_hash,
                XR_SHARED_OBJECT_BODY_TAG,
            )));
            let allocation_group_id = if node.projectile_pool() {
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
    }

    let mut child_index = 0usize;
    widget.children(&mut |child_id, child| {
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
    }
}
