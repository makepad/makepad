use crate::{makepad_derive_widget::*, makepad_draw::*, widget::*};

use super::scene_3d::{
    apply_scene_to_draw_pbr, chart_data_from_scope, register_last_draw_call_anchor,
    scene_state_from_scope,
};

script_mod! {
    use mod.prelude.widgets_internal.*

    set_type_default() do #(DrawBarPbr::script_shader(vm)){
        ..mod.draw.DrawPbr
        get_vertex_displacement: fn(uv: vec2, local_pos: vec3) {
            // The rounded cube goes from -0.5 to +0.5 on Y.
            // We want the bottom half to stay put and the top half to shift up.
            // Vertices with y > 0 are the top face + top edges/corners — shift them
            // up by (bar_height - 1.0) so the rounded corners move rigidly without stretching.
            // Vertices with y <= 0 stay in place. The flat middle section stretches via
            // a smooth step to connect them.
            //
            // inner = 0.5 - radius = 0.4 for default radius 0.1
            // y in [-0.5, -inner]: bottom corners — no displacement
            // y in [-inner, inner]: flat face region — linearly interpolate
            // y in [inner, 0.5]: top corners — full displacement
            let inner = 0.4;
            let t = clamp((local_pos.y + inner) / (2.0 * inner), 0.0, 1.0);
            return vec3(0.0, t * (self.bar_height - 1.0), 0.0)
        }
    }

    mod.widgets.BarChart3DBase = #(BarChart3D::register_widget(vm))
    mod.widgets.BarChart3D = set_type_default() do mod.widgets.BarChart3DBase{
        draw_bar +: {
            light_dir: vec3(0.35, 0.8, 0.45)
            light_color: vec3(1.0, 1.0, 1.0)
            ambient: 0.16
            spec_power: 64.0
            spec_strength: 0.65
        }
        draw_text_3d +: {
            color: #x1f1f1f
            text_style: theme.font_regular{font_size: 10.0}
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawBarPbr {
    #[deref]
    pub draw_super: DrawPbr,
    #[live(1.0)]
    pub bar_height: f32,
}

#[derive(Script, ScriptHook, Widget)]
pub struct BarChart3D {
    #[uid]
    uid: WidgetUid,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[redraw]
    #[live]
    draw_bar: DrawBarPbr,
    #[redraw]
    #[live]
    draw_pbr: DrawPbr,
    #[redraw]
    #[live]
    draw_text_3d: DrawText3d,

    #[live(vec3(0.0, 0.0, 0.0))]
    position: Vec3f,
    #[live(vec3(0.0, 0.0, 0.0))]
    rotation: Vec3f,
    #[live(vec3(1.0, 1.0, 1.0))]
    scale: Vec3f,

    #[live(0.0)]
    base_y: f32,
    #[live(0.52)]
    spacing: f32,
    #[live(0.28)]
    bar_size: f32,
    #[live(5.6)]
    height_scale: f32,
    #[live(0.38)]
    metallic: f32,
    #[live(0.34)]
    roughness: f32,
    #[live(0.06)]
    corner_radius: f32,
    #[live(4u32)]
    corner_segments: u32,

    #[live(true)]
    show_axes: bool,
    #[live(false)]
    show_labels: bool,
    #[live(true)]
    billboard_labels: bool,
}

impl BarChart3D {
    fn transform_point(model: Mat4f, local: Vec3f) -> Vec3f {
        let p = model.transform_vec4(vec4(local.x, local.y, local.z, 1.0));
        vec3(p.x, p.y, p.z)
    }

    fn draw_axes(&mut self, cx: &mut Cx2d, x_span: f32, z_span: f32, y_top: f32) {
        self.draw_pbr
            .material_rgba(0.22, 0.22, 0.24, 1.0, 0.02, 0.88);

        self.draw_pbr.push_matrix();
        self.draw_pbr.translate(
            0.0,
            self.base_y + y_top * 0.5,
            -z_span * 0.5 - self.spacing * 0.5,
        );
        let _ = self.draw_pbr.draw_cube(cx, vec3(0.03, y_top, 0.03), 1);
        self.draw_pbr.pop_matrix();

        self.draw_pbr.push_matrix();
        self.draw_pbr.translate(0.0, self.base_y, -z_span * 0.5);
        let _ = self
            .draw_pbr
            .draw_cube(cx, vec3(x_span + self.spacing, 0.025, 0.025), 1);
        self.draw_pbr.pop_matrix();

        self.draw_pbr.push_matrix();
        self.draw_pbr.translate(-x_span * 0.5, self.base_y, 0.0);
        let _ = self
            .draw_pbr
            .draw_cube(cx, vec3(0.025, 0.025, z_span + self.spacing), 1);
        self.draw_pbr.pop_matrix();
    }
}

impl Widget for BarChart3D {
    fn draw_3d(&mut self, cx: &mut Cx3d, scope: &mut Scope) -> DrawStep {
        let Some(scene) = scene_state_from_scope(scope) else {
            return DrawStep::done();
        };
        let cx = &mut Cx2d::new(cx.cx);

        let data = chart_data_from_scope(scope).unwrap_or_default();
        if data.bars.is_empty() {
            return DrawStep::done();
        }

        // Set up both draw shaders with the scene state
        apply_scene_to_draw_pbr(&mut self.draw_bar.draw_super, cx, &scene);
        apply_scene_to_draw_pbr(&mut self.draw_pbr, cx, &scene);

        self.draw_bar.push_matrix();
        self.draw_bar.translate_v(self.position);
        self.draw_bar
            .rotate_xyz(self.rotation.x, self.rotation.y, self.rotation.z);
        self.draw_bar
            .scale_xyz(self.scale.x, self.scale.y, self.scale.z);

        // Mirror transforms to draw_pbr for axes
        self.draw_pbr.push_matrix();
        self.draw_pbr.translate_v(self.position);
        self.draw_pbr
            .rotate_xyz(self.rotation.x, self.rotation.y, self.rotation.z);
        self.draw_pbr
            .scale_xyz(self.scale.x, self.scale.y, self.scale.z);
        let node_transform = self.draw_bar.cur_transform;

        let spacing = self.spacing.max(0.001);
        let bar_size = self.bar_size.clamp(0.001, spacing * 0.72);
        let x_span = (data.x_bins.saturating_sub(1) as f32) * spacing;
        let z_span = (data.z_bins.saturating_sub(1) as f32) * spacing;
        let x_origin = -0.5 * x_span;
        let z_origin = -0.5 * z_span;
        let denom = data.max_value.max(0.001);
        let corner_radius = self.corner_radius;
        let corner_segments = self.corner_segments;

        let mut y_top = 0.0_f32;
        for bar in &data.bars {
            let h = ((bar.value.max(0.0)) / denom * self.height_scale).max(0.015);
            y_top = y_top.max(h);

            let x = x_origin + bar.x as f32 * spacing;
            let z = z_origin + bar.z as f32 * spacing;

            // Position at base, scale only XZ — height is handled by the vertex shader
            self.draw_bar.push_matrix();
            self.draw_bar.translate(x, self.base_y, z);
            self.draw_bar
                .material(bar.color, self.metallic, self.roughness);

            // Set the instance bar_height — the displacement shader stretches Y
            self.draw_bar.bar_height = h;

            let half = vec3(bar_size, bar_size, bar_size);
            let draw_result = self.draw_bar.draw_rounded_cube(
                cx,
                half,
                corner_radius,
                1,
                corner_segments as usize,
            );
            if draw_result.is_ok() {
                let world = self
                    .draw_bar
                    .cur_transform
                    .transform_vec4(vec4(0.0, h * 0.5, 0.0, 1.0));
                register_last_draw_call_anchor(cx, scope, vec3(world.x, world.y, world.z));
            }
            self.draw_bar.pop_matrix();
        }

        if self.show_axes {
            self.draw_axes(cx, x_span, z_span, y_top.max(0.05));
        }
        self.draw_bar.pop_matrix();
        self.draw_pbr.pop_matrix();

        if self.show_labels {
            self.draw_text_3d
                .set_camera_state(scene.view, scene.projection, scene.camera_pos);
            self.draw_text_3d.set_viewport_rect(scene.viewport_rect);
            self.draw_text_3d.set_billboard(self.billboard_labels);
            self.draw_text_3d.draw_super.draw_super.draw_clip = vec4(
                scene.viewport_rect.pos.x as f32,
                scene.viewport_rect.pos.y as f32,
                (scene.viewport_rect.pos.x + scene.viewport_rect.size.x) as f32,
                (scene.viewport_rect.pos.y + scene.viewport_rect.size.y) as f32,
            );

            let x_label_pos_local = vec3(0.0, self.base_y + 0.02, z_origin - spacing * 1.6);
            let z_label_pos_local = vec3(x_origin - spacing * 1.4, self.base_y + 0.02, 0.0);
            let y_label_pos_local = vec3(
                x_origin - spacing * 1.15,
                self.base_y + y_top.max(0.3) * 0.8,
                z_origin - spacing * 0.65,
            );
            let top_label_pos_local = vec3(
                x_origin - spacing * 0.95,
                self.base_y + y_top.max(0.3),
                z_origin - spacing * 0.65,
            );
            let x_label_pos = Self::transform_point(node_transform, x_label_pos_local);
            let z_label_pos = Self::transform_point(node_transform, z_label_pos_local);
            let y_label_pos = Self::transform_point(node_transform, y_label_pos_local);
            let top_label_pos = Self::transform_point(node_transform, top_label_pos_local);

            let _ = self
                .draw_text_3d
                .draw_world_text(cx, x_label_pos, &data.x_axis_label);
            let _ = self
                .draw_text_3d
                .draw_world_text(cx, z_label_pos, &data.z_axis_label);
            let _ = self
                .draw_text_3d
                .draw_world_text(cx, y_label_pos, &data.y_axis_label);
            let _ = self.draw_text_3d.draw_world_text(
                cx,
                top_label_pos,
                &format!("{:.0}", data.max_value),
            );
        }

        DrawStep::done()
    }

    fn draw_walk(&mut self, _cx: &mut Cx2d, _scope: &mut Scope, _walk: Walk) -> DrawStep {
        DrawStep::done()
    }
}
