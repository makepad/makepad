use crate::{
    makepad_math::{Mat4f, Rect, Vec3f},
    makepad_platform::{Area, DrawListId},
};

#[derive(Clone, Copy, Debug, Default)]
pub struct SceneState3D {
    pub time: f64,
    pub camera_pos: Vec3f,
    pub view: Mat4f,
    pub projection: Mat4f,
    pub viewport_rect: Rect,
}

#[derive(Clone, Copy, Debug)]
pub struct SceneDrawCallAnchor {
    pub area: Area,
    pub draw_list_id: Option<DrawListId>,
    pub draw_item_id: Option<usize>,
    pub world_pos: Vec3f,
}

#[derive(Clone, Debug, Default)]
pub struct SceneScope3D {
    pub scene: SceneState3D,
    pub world_transform: Mat4f,
    pub draw_call_anchors: Vec<SceneDrawCallAnchor>,
}

impl SceneScope3D {
    pub fn new(scene: SceneState3D) -> Self {
        Self {
            scene,
            world_transform: Mat4f::identity(),
            draw_call_anchors: Vec::new(),
        }
    }
}

#[derive(Default)]
pub(crate) struct Cx3dState {
    pub scene_scope: Option<SceneScope3D>,
}
