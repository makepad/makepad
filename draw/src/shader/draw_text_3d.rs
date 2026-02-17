use crate::{cx_2d::Cx2d, makepad_platform::*, shader::draw_rotated_text::DrawRotatedText};

script_mod! {
    use mod.pod.*
    use mod.math.*
    use mod.shader.*
    use mod.draw
    use mod.geom

    mod.draw.DrawText3d = mod.std.set_type_default() do #(DrawText3d::script_shader(vm)){
        ..mod.draw.DrawRotatedText
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawText3d {
    #[deref]
    pub draw_super: DrawRotatedText,
    #[rust(Mat4f::identity())]
    pub view_matrix: Mat4f,
    #[rust(Mat4f::identity())]
    pub projection_matrix: Mat4f,
    #[rust(vec3(0.0, 0.0, 5.0))]
    pub camera_pos: Vec3f,
    #[rust(Rect::default())]
    pub viewport_rect: Rect,
    #[rust(false)]
    pub billboard: bool,
    #[rust(vec2(14.0, -18.0))]
    pub billboard_offset: Vec2f,
    #[rust(true)]
    pub billboard_flag: bool,
    #[rust(10.0)]
    pub billboard_flag_spacing: f32,
    #[rust(0.0)]
    pub text_rotation: f32,
}

impl DrawText3d {
    pub fn set_camera_state(&mut self, view: Mat4f, projection: Mat4f, camera_pos: Vec3f) {
        self.view_matrix = view;
        self.projection_matrix = projection;
        self.camera_pos = camera_pos;
    }

    pub fn set_viewport_rect(&mut self, viewport_rect: Rect) {
        self.viewport_rect = viewport_rect;
    }

    pub fn set_billboard(&mut self, billboard: bool) {
        self.billboard = billboard;
    }

    pub fn draw_world_text(&mut self, cx: &mut Cx2d, world_pos: Vec3f, text: &str) -> bool {
        if self.billboard {
            self.draw_billboard_text(cx, world_pos, text)
        } else {
            self.draw_camera_facing_text(cx, world_pos, text)
        }
    }

    pub fn draw_camera_facing_text(&mut self, cx: &mut Cx2d, world_pos: Vec3f, text: &str) -> bool {
        self.draw_camera_facing_text_with_rotation(cx, world_pos, text, self.text_rotation)
    }

    pub fn draw_camera_facing_text_with_rotation(
        &mut self,
        cx: &mut Cx2d,
        world_pos: Vec3f,
        text: &str,
        rotation: f32,
    ) -> bool {
        let Some((screen, _depth)) = self.project_world_to_screen(world_pos) else {
            return false;
        };
        self.draw_screen_text_aligned(cx, screen, text, rotation, 1.0, 0.5)
    }

    pub fn draw_billboard_text(&mut self, cx: &mut Cx2d, world_pos: Vec3f, text: &str) -> bool {
        self.draw_billboard_text_with_rotation(cx, world_pos, text, self.text_rotation)
    }

    pub fn draw_billboard_text_with_rotation(
        &mut self,
        cx: &mut Cx2d,
        world_pos: Vec3f,
        text: &str,
        rotation: f32,
    ) -> bool {
        let Some((screen, _depth)) = self.project_world_to_screen(world_pos) else {
            return false;
        };

        // Flip billboard side from camera X to reduce overlap with model center.
        let side = if (self.camera_pos.x - world_pos.x) >= 0.0 {
            1.0
        } else {
            -1.0
        };
        let offset = vec2(self.billboard_offset.x * side, self.billboard_offset.y);
        let label_anchor = screen + offset;

        if self.billboard_flag {
            let marker = if side > 0.0 { ">" } else { "<" };
            let _ = self.draw_screen_text_aligned(cx, label_anchor, marker, rotation, 1.0, 0.5);
        }

        let text_anchor = vec2(
            label_anchor.x + self.billboard_flag_spacing * side,
            label_anchor.y,
        );
        let align = if side > 0.0 { 0.0 } else { 1.0 };
        self.draw_screen_text_aligned(cx, text_anchor, text, rotation, 1.0, align)
    }

    pub fn project_world_to_screen(&self, world_pos: Vec3f) -> Option<(Vec2f, f32)> {
        if self.viewport_rect.size.x <= 0.0 || self.viewport_rect.size.y <= 0.0 {
            return None;
        }

        let world = vec4(world_pos.x, world_pos.y, world_pos.z, 1.0);
        let view = self.view_matrix.transform_vec4(world);
        let clip = self.projection_matrix.transform_vec4(view);
        if clip.w.abs() < 0.00001 || clip.w <= 0.0 {
            return None;
        }

        let inv_w = 1.0 / clip.w;
        let ndc_x = clip.x * inv_w;
        let ndc_y = clip.y * inv_w;
        let ndc_z = clip.z * inv_w;
        if ndc_z < -1.0 || ndc_z > 1.0 {
            return None;
        }

        let x = self.viewport_rect.pos.x as f32
            + (ndc_x * 0.5 + 0.5) * self.viewport_rect.size.x as f32;
        let y = self.viewport_rect.pos.y as f32
            + (1.0 - (ndc_y * 0.5 + 0.5)) * self.viewport_rect.size.y as f32;
        Some((vec2(x, y), ndc_z))
    }

    fn draw_screen_text_aligned(
        &mut self,
        cx: &mut Cx2d,
        anchor: Vec2f,
        text: &str,
        rotation: f32,
        label_scale: f32,
        horizontal_align: f32,
    ) -> bool {
        let Some(run) = self.draw_super.draw_super.prepare_single_line_run(cx, text) else {
            return false;
        };

        let align = horizontal_align.clamp(0.0, 1.0);
        let baseline_x = anchor.x - run.width_in_lpxs * align;
        let baseline = crate::text::geom::Point::new(baseline_x, anchor.y);
        let rotation_origin = crate::text::geom::Point::new(anchor.x, anchor.y);

        for glyph in &run.glyphs {
            let glyph_origin = crate::text::geom::Point::new(
                baseline.x + glyph.pen_x_in_lpxs + glyph.offset_x_in_lpxs,
                baseline.y,
            );
            self.draw_super.draw_glyph_at(
                cx,
                glyph_origin,
                rotation_origin,
                glyph.font_size_in_lpxs,
                glyph.rasterized,
                rotation,
                label_scale,
            );
        }
        true
    }
}
