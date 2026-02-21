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
            grid_line_aa: fn(centered: vec2, px: vec2, scale: float) {
                let g = centered * scale;
                let cell = abs(fract(g - vec2(0.5, 0.5)) - vec2(0.5, 0.5));
                let fw = px * scale;
                let aa = min(
                    cell.x / max(fw.x, 0.000001),
                    cell.y / max(fw.y, 0.000001)
                );
                return 1.0 - min(aa, 1.0)
            }
            get_base_color: fn(uv: vec2, vertex_color: vec4) {
                let base = self.u_base_color_factor * vertex_color;
                let centered = uv - vec2(0.5, 0.5);
                let px = vec2(
                    max(length(vec2(dFdx(centered.x), dFdy(centered.x))), 0.000001),
                    max(length(vec2(dFdx(centered.y), dFdy(centered.y))), 0.000001)
                );

                let micro = self.grid_line_aa(centered, px, 96.0);
                let minor = self.grid_line_aa(centered, px, 24.0);
                let major = self.grid_line_aa(centered, px, 6.0);

                let axis_x = 1.0 - min(abs(centered.x) / px.x, 1.0);
                let axis_z = 1.0 - min(abs(centered.y) / px.y, 1.0);

                let floor_color = mix(base.xyz * 0.54, vec3(0.18, 0.185, 0.195), 0.40);
                let micro_color = vec3(0.26, 0.27, 0.285);
                let minor_color = vec3(0.36, 0.37, 0.40);
                let major_color = vec3(0.50, 0.52, 0.56);

                let mut color = floor_color;
                color = mix(color, micro_color, micro * 0.18);
                color = mix(color, minor_color, minor * 0.50);
                color = mix(color, major_color, major * 0.80);
                color = mix(color, vec3(0.47, 0.30, 0.30), axis_x * 0.60);
                color = mix(color, vec3(0.30, 0.37, 0.52), axis_z * 0.60);
                return vec4(color, base.w)
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
    #[live(vec4(0.58, 0.60, 0.63, 1.0))]
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
        let _ = self.draw_pbr.draw_surface(cx, vec2(self.size, self.size), 1, 1);
        self.draw_pbr.pop_matrix();
        DrawStep::done()
    }

    fn draw_walk(&mut self, _cx: &mut Cx2d, _scope: &mut Scope, _walk: Walk) -> DrawStep {
        DrawStep::done()
    }
}
