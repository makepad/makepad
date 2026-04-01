pub mod xr_body_spawn;
pub mod xr_env;
mod xr_gesture;
pub mod xr_node;
pub mod xr_permissions_flow;
pub mod xr_root;
pub mod xr_select;
pub mod xr_shared_object_sync;
pub mod xr_view;

pub mod xr_peer_sync {
    pub use crate::sync::xr_peer_sync::*;
}

pub(crate) use xr_gesture::{
    arm_pair_metrics, flat_head_forward, hand_closed_fist_contact_point,
    hand_closed_fist_contact_point_geometry_only,
};

pub use crate::net::{XrActivityId, XrSpawnableObjectId};
pub use xr_body_spawn::{XrBodyImpulse, XrBodySpawn};
pub use xr_env::{DrawDepthMeshBasic, XrEnv};
pub use xr_node::{
    xr_draw_list_depth, xr_sort_child_draw_order, xr_widget_children, xr_widget_is_transparent,
    xr_widget_local_sort_center, xr_widget_with_scene_node, xr_widget_world_transform, XrBodyKind,
    XrDrawContext, XrDrawScopeData, XrHandInfluencePoint, XrNode, XrNodeAction,
    XrPassthroughScopeData, XrPhysicsShape, XrRenderClass, XrRuntimeBodyState,
    XrSharedObjectPolicy, XR_HAND_INFLUENCE_POINTS_PER_HAND, XR_HAND_INFLUENCE_POINT_COUNT,
};
pub use crate::sync::xr_peer_sync::{XrPeerSync, XrPeerSyncAction};
pub use xr_permissions_flow::XrPermissionsFlow;
pub use xr_root::{XrCamera, XrRoot, XrRootAction};
pub use xr_select::{XrSelect, XrSelectAction};
pub use xr_shared_object_sync::{
    collect_scene_spawnable_objects, XrLocalSharedObjectSnapshot, XrRemoteSharedObjectSnapshot,
    XrScheduledAuthorityTransfer, XrSharedObjectRegistry, XrSpawnableObjectBinding,
};
pub use xr_view::{DrawXrFingerCursor, XrView, XrViewEventScopeData, XrViewMode};
