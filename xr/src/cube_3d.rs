use crate::{makepad_derive_widget::*, makepad_draw::*, widget::*};

use crate::scene_3d::{
    apply_scene_to_draw_pbr, scene_node_world_transform_from_scope, scene_state_from_scope,
};

script_mod! {
    use mod.prelude.widgets_internal.*

    mod.widgets.Cube3DBase = #(Cube3D::register_widget(vm))
    mod.widgets.Cube3D = set_type_default() do mod.widgets.Cube3DBase{
        draw_pbr +: {
            light_dir: vec3(0.35, 0.8, 0.45)
            light_color: vec3(1.0, 1.0, 1.0)
            ambient: 0.22
            spec_power: 128.0
            spec_strength: 0.85
        }
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct Cube3D {
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
    #[live(vec3(0.0, 0.0, 0.0))]
    position: Vec3f,
    #[live(vec3(0.0, 0.0, 0.0))]
    rotation: Vec3f,
    #[live(vec3(1.0, 1.0, 1.0))]
    scale: Vec3f,
    #[live(vec3(0.18, 0.18, 0.18))]
    half_extents: Vec3f,
    #[live(vec4(0.82, 0.48, 0.28, 1.0))]
    color: Vec4f,
    #[live(0.02)]
    metallic: f32,
    #[live(0.52)]
    roughness: f32,
    #[live(0.02)]
    corner_radius: f32,
    #[live(3u32)]
    corner_segments: u32,
}

impl Widget for Cube3D {
    fn draw_3d(&mut self, cx: &mut Cx3d, scope: &mut Scope) -> DrawStep {
        let Some(scene) = scene_state_from_scope(scope) else {
            return DrawStep::done();
        };
        let cx = &mut Cx2d::new(cx.cx);
        let parent_world = scene_node_world_transform_from_scope(scope);

        apply_scene_to_draw_pbr(&mut self.draw_pbr, cx, &scene);
        self.draw_pbr.push_matrix();
        self.draw_pbr.apply_transform(parent_world);
        self.draw_pbr.translate_v(self.position);
        self.draw_pbr
            .rotate_xyz(self.rotation.x, self.rotation.y, self.rotation.z);
        self.draw_pbr
            .scale_xyz(self.scale.x, self.scale.y, self.scale.z);
        self.draw_pbr.set_base_color_factor(self.color);
        self.draw_pbr
            .set_metal_roughness(self.metallic, self.roughness);
        let _ = self.draw_pbr.draw_rounded_cube(
            cx,
            self.half_extents,
            self.corner_radius,
            1,
            self.corner_segments as usize,
        );
        self.draw_pbr.pop_matrix();
        DrawStep::done()
    }

    fn draw_walk(&mut self, _cx: &mut Cx2d, _scope: &mut Scope, _walk: Walk) -> DrawStep {
        DrawStep::done()
    }
}
