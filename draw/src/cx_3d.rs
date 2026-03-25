use {
    crate::{
        cx_draw::CxDraw,
        makepad_math::{Mat4f, Vec3f},
        makepad_platform::{Area, DrawListId},
        scene_3d::{Cx3dState, SceneDrawCallAnchor, SceneScope3D, SceneState3D},
    },
    std::{ops::Deref, ops::DerefMut},
};

pub struct Cx3d<'a, 'b> {
    pub cx: &'b mut CxDraw<'a>,
    scene_3d: Cx3dState,
}

impl<'a, 'b> Deref for Cx3d<'a, 'b> {
    type Target = CxDraw<'a>;
    fn deref(&self) -> &Self::Target {
        self.cx
    }
}
impl<'a, 'b> DerefMut for Cx3d<'a, 'b> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.cx
    }
}

impl<'a, 'b> Cx3d<'a, 'b> {
    pub fn new(cx: &'b mut CxDraw<'a>) -> Self {
        Self {
            cx,
            scene_3d: Cx3dState::default(),
        }
    }

    pub fn scene_3d(&self) -> Option<&SceneScope3D> {
        self.scene_3d.scene_scope.as_ref()
    }

    pub fn scene_3d_mut(&mut self) -> Option<&mut SceneScope3D> {
        self.scene_3d.scene_scope.as_mut()
    }

    pub fn begin_scene_3d(&mut self, scene: SceneState3D) {
        self.scene_3d.scene_scope = Some(SceneScope3D::new(scene));
    }

    pub fn end_scene_3d(&mut self) {
        self.scene_3d.scene_scope = None;
    }

    pub fn scene_state_3d(&self) -> Option<SceneState3D> {
        self.scene_3d().map(|scope| scope.scene)
    }

    pub fn scene_world_transform_3d(&self) -> Mat4f {
        self.scene_3d()
            .map(|scope| scope.world_transform)
            .unwrap_or_else(Mat4f::identity)
    }

    pub fn set_scene_world_transform_3d(&mut self, world_transform: Mat4f) -> Option<Mat4f> {
        let scope = self.scene_3d_mut()?;
        let previous = scope.world_transform;
        scope.world_transform = world_transform;
        Some(previous)
    }

    pub fn scene_draw_call_anchors_3d(&self) -> Option<&[SceneDrawCallAnchor]> {
        self.scene_3d()
            .map(|scope| scope.draw_call_anchors.as_slice())
    }

    pub fn clear_scene_draw_call_anchors_3d(&mut self) {
        if let Some(scope) = self.scene_3d_mut() {
            scope.draw_call_anchors.clear();
        }
    }

    pub fn register_scene_draw_call_anchor_3d(&mut self, area: Area, world_pos: Vec3f) {
        let Some(scope) = self.scene_3d_mut() else {
            return;
        };
        let (draw_list_id, draw_item_id) = match area {
            Area::Instance(inst) => (Some(inst.draw_list_id), Some(inst.draw_item_id)),
            _ => (None, None),
        };
        scope.draw_call_anchors.push(SceneDrawCallAnchor {
            area,
            draw_list_id,
            draw_item_id,
            world_pos,
        });
    }

    pub fn register_last_scene_draw_call_anchor_3d(
        &mut self,
        draw_list_id: DrawListId,
        draw_item_id: usize,
        world_pos: Vec3f,
    ) {
        let Some(scope) = self.scene_3d_mut() else {
            return;
        };
        scope.draw_call_anchors.push(SceneDrawCallAnchor {
            area: Area::Empty,
            draw_list_id: Some(draw_list_id),
            draw_item_id: Some(draw_item_id),
            world_pos,
        });
    }
}
