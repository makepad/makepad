use crate::surface_scene_3d::{surface_scene_state_from_scope, surface_scene_world_transform_from_scope};
use makepad_widgets::makepad_draw::DrawPbr;
use makepad_widgets::*;

script_mod! {
    use mod.prelude.widgets_internal.*

    mod.widgets.TexturedPlane3DBase = #(TexturedPlane3D::register_widget(vm))
    mod.widgets.TexturedPlane3D = set_type_default() do mod.widgets.TexturedPlane3DBase{}
}

#[derive(Script, ScriptHook, Widget)]
pub struct TexturedPlane3D {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[redraw]
    #[live]
    draw_pbr: DrawPbr,
    #[live]
    texture: Texture,
    #[live(vec3(0.0, 0.0, 0.0))]
    position: Vec3f,
    #[live(vec3(0.0, 0.0, 0.0))]
    rotation: Vec3f,
    #[live(vec3(1.0, 1.0, 1.0))]
    scale: Vec3f,
    #[live(vec2(1.0, 1.0))]
    size: Vec2f,
}

pub(crate) fn textured_plane_model_matrix(position: Vec3f, rotation: Vec3f, scale: Vec3f) -> Mat4f {
    let mut model = Mat4f::translation(position);
    model = Mat4f::mul(&model, &Mat4f::rotation(rotation));
    model = Mat4f::mul(&model, &Mat4f::rotation(vec3(std::f32::consts::FRAC_PI_2, 0.0, 0.0)));
    Mat4f::mul(
        &model,
        &Mat4f::nonuniform_scaled_translation(scale, vec3(0.0, 0.0, 0.0)),
    )
}

#[cfg(test)]
pub(crate) fn textured_plane_local_scale(size: Vec2f) -> Vec3f {
    vec3(size.x, 1.0, size.y)
}

impl TexturedPlane3D {
    pub fn set_texture(&mut self, texture: Texture) {
        self.texture = texture;
    }
}

impl Widget for TexturedPlane3D {
    fn draw_3d(&mut self, cx: &mut Cx3d, scope: &mut Scope) -> DrawStep {
        let Some(scene) = surface_scene_state_from_scope(scope) else {
            return DrawStep::done();
        };
        let parent_world = surface_scene_world_transform_from_scope(scope);
        let cx = &mut Cx2d::new(cx.cx);

        self.draw_pbr.set_camera_state(scene.view, scene.projection_viewport, scene.camera_pos);
        self.draw_pbr.set_clip_ndc(scene.clip_ndc);
        self.draw_pbr
            .set_depth_range(scene.depth_range.x, scene.depth_range.y);
        self.draw_pbr
            .set_depth_forward_bias(scene.depth_forward_bias);
        self.draw_pbr.set_depth_clip(1.0);
        self.draw_pbr.set_use_pass_camera(false);
        self.draw_pbr.set_transform(Mat4f::mul(
            &parent_world,
            &textured_plane_model_matrix(self.position, self.rotation, self.scale),
        ));
        self.draw_pbr.set_base_color_factor(vec4(1.0, 1.0, 1.0, 1.0));
        self.draw_pbr.set_metal_roughness(0.0, 1.0);
        self.draw_pbr.set_base_color_texture(Some(self.texture.clone()));
        self.draw_pbr.light_color = vec3(0.0, 0.0, 0.0);
        self.draw_pbr.env_intensity = 0.0;
        self.draw_pbr.spec_strength = 0.0;
        self.draw_pbr.ambient = 1.0;
        self.draw_pbr.draw_clip = scene.clip_ndc;
        let _ = self.draw_pbr.draw_surface(cx, self.size, 1, 1);
        DrawStep::done()
    }

    fn draw_walk(&mut self, _cx: &mut Cx2d, _scope: &mut Scope, _walk: Walk) -> DrawStep {
        DrawStep::done()
    }
}
