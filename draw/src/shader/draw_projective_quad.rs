use crate::{cx_2d::*, makepad_platform::*, shader::draw_quad::DrawQuad, turtle::Walk};

// Textured quad that stays attached to the normal 2D draw-list/pass pipeline,
// but submits homogeneous/projective vertex positions inside that pass.
// Basis is CSS/document space: x right, y down, z toward the viewer.

script_mod! {
    use mod.pod.*
    use mod.math.*
    use mod.shader.*
    use mod.draw

    mod.draw.DrawProjectiveQuad = mod.std.set_type_default() do #(DrawProjectiveQuad::script_shader(vm)){
        ..mod.draw.DrawQuad
        depth_write: true
        backface_culling: false
        color_texture: texture_2d(float)
        u_has_color_texture: uniform(float(0.0))
        u_transform_matrix: uniform(mat4x4f(
            1.0, 0.0, 0.0, 0.0,
            0.0, 1.0, 0.0, 0.0,
            0.0, 0.0, 1.0, 0.0,
            0.0, 0.0, 0.0, 1.0
        ))
        u_perspective_matrix: uniform(mat4x4f(
            1.0, 0.0, 0.0, 0.0,
            0.0, 1.0, 0.0, 0.0,
            0.0, 0.0, 1.0, 0.0,
            0.0, 0.0, 0.0, 1.0
        ))
        u_opacity: uniform(float(1.0))
        u_projective_transform: uniform(mat4x4f(
            1.0, 0.0, 0.0, 0.0,
            0.0, 1.0, 0.0, 0.0,
            0.0, 0.0, 1.0, 0.0,
            0.0, 0.0, 0.0, 1.0
        ))
        clip_plane_count: uniform(float(0.0))
        clip_plane_0: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip_plane_1: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip_plane_2: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip_plane_3: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip_plane_4: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip_plane_5: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip_plane_6: uniform(vec4(0.0, 0.0, 0.0, 0.0))
        clip_plane_7: uniform(vec4(0.0, 0.0, 0.0, 0.0))

        v_uv: varying(vec2f)
        v_clip_space: varying(vec4f)

        clip_projected: fn() -> float {
            if self.clip_plane_count > 0.5 && dot(self.v_clip_space, self.clip_plane_0) < 0.0 { return 0.0 }
            if self.clip_plane_count > 1.5 && dot(self.v_clip_space, self.clip_plane_1) < 0.0 { return 0.0 }
            if self.clip_plane_count > 2.5 && dot(self.v_clip_space, self.clip_plane_2) < 0.0 { return 0.0 }
            if self.clip_plane_count > 3.5 && dot(self.v_clip_space, self.clip_plane_3) < 0.0 { return 0.0 }
            if self.clip_plane_count > 4.5 && dot(self.v_clip_space, self.clip_plane_4) < 0.0 { return 0.0 }
            if self.clip_plane_count > 5.5 && dot(self.v_clip_space, self.clip_plane_5) < 0.0 { return 0.0 }
            if self.clip_plane_count > 6.5 && dot(self.v_clip_space, self.clip_plane_6) < 0.0 { return 0.0 }
            if self.clip_plane_count > 7.5 && dot(self.v_clip_space, self.clip_plane_7) < 0.0 { return 0.0 }
            return 1.0
        }

        vertex: fn() {
            let uv = self.geom.pos;
            self.pos = uv;
            self.v_uv = uv;

            let local = vec4(
                uv.x * self.rect_size.x,
                uv.y * self.rect_size.y,
                0.0,
                1.0
            );
            let transformed = self.u_transform_matrix * local;
            let projected = self.u_perspective_matrix * transformed;
            let screen_shift = self.rect_pos;
            let screen_h = vec4(
                screen_shift.x * projected.w + projected.x,
                screen_shift.y * projected.w + projected.y,
                projected.z,
                projected.w
            );
            self.v_clip_space = self.projective_vertex(screen_h, self.u_projective_transform);
            self.vertex_pos = self.v_clip_space;
        }

        pixel: fn() {
            let uv = clamp(self.v_uv, vec2(0.0, 0.0), vec2(1.0, 1.0));
            if self.u_has_color_texture > 0.5 {
                return self.color_texture.sample_as_bgra(uv) * self.u_opacity
            }
            return vec4(1.0, 0.0, 1.0, 1.0)
        }

        fragment: fn() {
            if self.clip_projected() < 0.5 {
                discard()
            }
            self.fb0 = self.pixel()
        }
    }
}

#[derive(Script, ScriptHook, Debug)]
#[repr(C)]
pub struct DrawProjectiveQuad {
    #[rust(0.0)]
    pub has_color_texture: f32,
    #[rust(Mat4f::identity())]
    pub transform_matrix: Mat4f,
    #[rust(Mat4f::identity())]
    pub perspective_matrix: Mat4f,
    #[deref]
    pub draw_super: DrawQuad,
    #[live(1.0)]
    pub opacity: f32,
}

impl DrawProjectiveQuad {
    fn apply_draw_uniforms(&mut self, cx: &mut Cx2d) {
        self.draw_super
            .draw_vars
            .set_uniform(cx.cx, live_id!(u_has_color_texture), &[self.has_color_texture]);
        self.draw_super.draw_vars.set_uniform(
            cx.cx,
            live_id!(u_transform_matrix),
            &self.transform_matrix.v,
        );
        self.draw_super.draw_vars.set_uniform(
            cx.cx,
            live_id!(u_perspective_matrix),
            &self.perspective_matrix.v,
        );
        self.draw_super
            .draw_vars
            .set_uniform(cx.cx, live_id!(u_opacity), &[self.opacity]);
        self.draw_super.draw_vars.set_uniform(
            cx.cx,
            live_id!(u_projective_transform),
            &Mat4f::identity().v,
        );
    }

    pub fn set_texture(&mut self, texture: Option<Texture>) {
        self.has_color_texture = if texture.is_some() { 1.0 } else { 0.0 };
        self.draw_super.draw_vars.texture_slots[0] = texture;
    }

    pub fn set_matrices(&mut self, transform_matrix: Mat4f, perspective_matrix: Mat4f) {
        self.transform_matrix = transform_matrix;
        self.perspective_matrix = perspective_matrix;
    }

    pub fn draw_walk(&mut self, cx: &mut Cx2d, walk: Walk) -> Rect {
        let rect = cx.walk_turtle(walk);
        self.draw_super.rect_pos = rect.pos.into();
        self.draw_super.rect_size = rect.size.into();
        self.draw(cx);
        rect
    }

    pub fn draw_abs(&mut self, cx: &mut Cx2d, rect: Rect) {
        self.draw_super.rect_pos = rect.pos.into();
        self.draw_super.rect_size = rect.size.into();
        self.draw(cx);
    }

    pub fn draw(&mut self, cx: &mut Cx2d) {
        self.apply_draw_uniforms(cx);
        self.draw_super.draw(cx);
    }

    pub fn area(&self) -> Area {
        self.draw_super.draw_vars.area
    }
}

pub type DrawPlane3d = DrawProjectiveQuad;
