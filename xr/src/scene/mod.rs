pub mod xr_body_spawn;
pub mod xr_env;
mod xr_gesture;
pub mod xr_node;
pub mod xr_peer_sync;
pub mod xr_permissions_flow;
pub mod xr_root;
pub mod xr_select;
pub mod xr_shared_object_sync;
pub mod xr_view;

pub(crate) use xr_gesture::{hand_is_palm_down_closed_fist, CLOSED_FIST_GESTURE};

pub use crate::net::{XrActivityId, XrSpawnableObjectId};
pub use xr_body_spawn::XrBodySpawn;
pub use xr_env::{DrawDepthMeshBasic, XrEnv};
pub use xr_node::{
    xr_widget_world_transform, XrBodyKind, XrDrawContext, XrDrawScopeData, XrHandInfluencePoint,
    XrNode, XrPassthroughScopeData, XrRuntimeBodyState, XR_HAND_INFLUENCE_POINTS_PER_HAND,
    XR_HAND_INFLUENCE_POINT_COUNT,
};
pub use xr_peer_sync::{XrPeerSync, XrPeerSyncAction};
pub use xr_permissions_flow::XrPermissionsFlow;
pub use xr_root::{XrCamera, XrRoot};
pub use xr_select::{XrSelect, XrSelectAction};
pub use xr_shared_object_sync::{
    collect_scene_spawnable_objects, XrSharedObjectRegistry, XrSpawnableObjectBinding,
};
pub use xr_view::{DrawXrFingerCursor, XrView, XrViewEventScopeData, XrViewMode};
