use crate::{makepad_derive_widget::*, makepad_draw::*, widget::*};

use super::scene_3d::{apply_scene_to_draw_pbr, scene_state_from_scope};

script_mod! {
    use mod.prelude.widgets_internal.*

    mod.widgets.Grid3DBase = #(Grid3D::register_widget(vm))
    mod.widgets.Grid3D = set_type_default() do mod.widgets.Grid3DBase{
        draw_pbr +: {
            light_dir: vec3(0.35, 0.8, 0.45)
            light_color: vec3(1.0, 1.0, 1.0)
            ambient: 0.18
            spec_power: 64.0
            spec_strength: 0.35
            get_base_color: fn(uv: vec2, vertex_color: vec4) {
                let base = self.u_base_color_factor * vertex_color;
                let tiled = uv * 22.0;
                let fu = fract(tiled.x);
                let fv = fract(tiled.y);
                let du = min(fu, 1.0 - fu);
                let dv = min(fv, 1.0 - fv);
                let minor = 1.0 - smoothstep(0.0, 0.015, min(du, dv));

                let major_u = min(fract(uv.x * 5.5), 1.0 - fract(uv.x * 5.5));
                let major_v = min(fract(uv.y * 5.5), 1.0 - fract(uv.y * 5.5));
                let major = 1.0 - smoothstep(0.0, 0.02, min(major_u, major_v));

                let line_mix = clamp(minor * 0.65 + major * 0.55, 0.0, 1.0);
                let line_color = vec3(0.78, 0.80, 0.84);
                let floor_color = base.xyz * 0.78;
                return vec4(mix(floor_color, line_color, line_mix), base.w)
            }
        }
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct Grid3D {
    #[uid]
    uid: WidgetUid,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[redraw]
    #[live]
    draw_pbr: DrawPbr,

    #[live(8.0)]
    size: f32,
    #[live(vec3(0.0, -0.02, 0.0))]
    position: Vec3f,
    #[live(vec3(0.0, 0.0, 0.0))]
    rotation: Vec3f,
    #[live(vec3(1.0, 1.0, 1.0))]
    scale: Vec3f,
    #[live(vec4(0.72, 0.74, 0.78, 1.0))]
    color: Vec4f,
}

impl Widget for Grid3D {
    fn draw_3d(&mut self, cx: &mut Cx3d, scope: &mut Scope) -> DrawStep {
        let Some(scene) = scene_state_from_scope(scope) else {
            return DrawStep::done();
        };
        let cx = &mut Cx2d::new(cx.cx);

        apply_scene_to_draw_pbr(&mut self.draw_pbr, cx, &scene);
        self.draw_pbr.env_intensity = 0.25;
        self.draw_pbr.spec_strength = 0.08;
        self.draw_pbr.push_matrix();
        self.draw_pbr.translate_v(self.position);
        self.draw_pbr
            .rotate_xyz(self.rotation.x, self.rotation.y, self.rotation.z);
        self.draw_pbr
            .scale_xyz(self.scale.x, self.scale.y, self.scale.z);
        self.draw_pbr.fill(self.color);
        self.draw_pbr.set_metal_roughness(0.02, 0.96);
        let _ = self
            .draw_pbr
            .draw_surface(cx, vec2(self.size, self.size), 1, 1);
        self.draw_pbr.pop_matrix();
        DrawStep::done()
    }

    fn draw_walk(&mut self, _cx: &mut Cx2d, _scope: &mut Scope, _walk: Walk) -> DrawStep {
        DrawStep::done()
    }
}
