use crate::{makepad_derive_widget::*, makepad_draw::*, widget::*};

use super::{
    scene_draw::{apply_scene_to_draw_pbr, scene_state_from_cx},
    xr_node::{xr_widget_world_transform, XrDrawContext, XrNode},
};

const XR_REFRACTIVE_CAMERA_FOV_Y_DEGREES: f32 = 92.0;
const XR_REFRACTIVE_CAMERA_PROJECTION_SCALE: f32 = 0.6825;
const XR_REFRACTIVE_CAMERA_EXPOSURE: f32 = 0.68;
const XR_REFRACTIVE_FACE_SUBDIVISIONS: usize = 1;
const XR_REFRACTIVE_CORNER_SEGMENTS: usize = 3;

script_mod! {
    use mod.prelude.widgets_internal.*

    mod.widgets.RefractiveCubeBase = #(RefractiveCube::register_widget(vm))
    mod.widgets.RefractiveCube = set_type_default() do mod.widgets.RefractiveCubeBase{
        body: mod.widgets.XrBodyKind.Dynamic
        size: vec3(0.12, 0.12, 0.12)
        color: vec4(0.80, 0.92, 1.0, 0.18)
        corner_radius: 0.024
        roughness: 0.04
        env_intensity: 1.2
        spec_strength: 0.6
        focus_distance: 1.8
        draw_pbr +: {
            light_dir: vec3(0.35, 0.8, 0.45)
            light_color: vec3(1.0, 1.0, 1.0)
            ambient: 0.02
            spec_power: 128.0
            spec_strength: 1.0
            env_intensity: 1.2
        }
    }
}

#[derive(Script, Widget)]
pub struct RefractiveCube {
    #[redraw]
    #[live]
    draw_pbr: DrawPbrRefractive,
    #[live(vec3(0.12, 0.12, 0.12))]
    size: Vec3f,
    #[live(vec4(0.80, 0.92, 1.0, 0.18))]
    color: Vec4f,
    #[live(0.024)]
    corner_radius: f32,
    #[live(0.04)]
    roughness: f32,
    #[live(0.6)]
    spec_strength: f32,
    #[live(1.2)]
    env_intensity: f32,
    #[live(1.8)]
    focus_distance: f32,
    #[cast]
    #[deref]
    node: XrNode,
}

impl RefractiveCube {
    pub fn half_extents(&self) -> Vec3f {
        vec3f(
            self.size.x.max(0.0) * 0.5,
            self.size.y.max(0.0) * 0.5,
            self.size.z.max(0.0) * 0.5,
        )
    }

    pub fn node(&self) -> &XrNode {
        &self.node
    }
}

impl ScriptHook for RefractiveCube {
    fn on_after_apply(
        &mut self,
        _vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
        self.node.set_implicit_physics_size(self.size);
    }
}

impl Widget for RefractiveCube {
    fn draw_3d(&mut self, cx: &mut Cx3d, scope: &mut Scope) -> DrawStep {
        if scene_state_from_cx(cx).is_none() {
            return DrawStep::done();
        }
        let world = xr_widget_world_transform(cx, scope, self.widget_uid(), &self.node);
        let half_extents = self.half_extents();

        let _ = apply_scene_to_draw_pbr(&mut self.draw_pbr.draw_super, cx);
        let draw_context = XrDrawContext::from_scope(scope);
        let passthrough = draw_context.passthrough();
        self.draw_pbr.source_size = passthrough.source_size;
        self.draw_pbr.camera_enabled = if passthrough.enabled { 1.0 } else { 0.0 };
        self.draw_pbr.rotation_steps = passthrough.rotation_steps;
        self.draw_pbr.camera_fov_y_degrees = XR_REFRACTIVE_CAMERA_FOV_Y_DEGREES;
        self.draw_pbr.camera_projection_scale = XR_REFRACTIVE_CAMERA_PROJECTION_SCALE;
        self.draw_pbr.camera_exposure = XR_REFRACTIVE_CAMERA_EXPOSURE;
        self.draw_pbr.camera_center_offset_uv = passthrough.center_offset_uv;
        self.draw_pbr.object_center = world.transform_vec4(vec4f(0.0, 0.0, 0.0, 1.0)).to_vec3f();
        self.draw_pbr.object_right = world
            .transform_vec4(vec4f(1.0, 0.0, 0.0, 0.0))
            .to_vec3f()
            .normalize();
        self.draw_pbr.object_up = world
            .transform_vec4(vec4f(0.0, 1.0, 0.0, 0.0))
            .to_vec3f()
            .normalize();
        self.draw_pbr.object_forward = world
            .transform_vec4(vec4f(0.0, 0.0, 1.0, 0.0))
            .to_vec3f()
            .normalize();
        self.draw_pbr.object_half_extents = half_extents;
        self.draw_pbr.object_corner_radius = self.corner_radius;
        self.draw_pbr.transmission_focus_distance = self.focus_distance;
        self.draw_pbr.set_depth_write(true);
        self.draw_pbr.set_camera_texture(passthrough.camera_texture);
        if let Some(env_texture) = draw_context.env_texture() {
            self.draw_pbr.set_env_face_textures(None);
            self.draw_pbr.set_env_texture(Some(env_texture));
            self.draw_pbr.set_env_atlas_texture(None);
        } else {
            self.draw_pbr.set_env_face_textures(None);
            let env_tex = self.draw_pbr.default_env_texture(cx);
            self.draw_pbr.set_env_texture(Some(env_tex));
            self.draw_pbr.set_env_atlas_texture(None);
        }
        self.draw_pbr.ambient = 0.002;
        self.draw_pbr.spec_strength = self.spec_strength;
        self.draw_pbr.env_intensity = self.env_intensity;
        self.draw_pbr.light_color = vec3(0.10, 0.10, 0.10);
        self.draw_pbr.set_base_color_factor(self.color);
        self.draw_pbr.set_metal_roughness(0.0, self.roughness);
        self.draw_pbr.set_transform(world);
        let _ = self.draw_pbr.draw_rounded_cube(
            cx,
            half_extents,
            self.corner_radius,
            XR_REFRACTIVE_FACE_SUBDIVISIONS,
            XR_REFRACTIVE_CORNER_SEGMENTS,
        );

        self.node.draw_3d(cx, scope)
    }

    fn draw_walk(&mut self, _cx: &mut Cx2d, _scope: &mut Scope, _walk: Walk) -> DrawStep {
        DrawStep::done()
    }
}
