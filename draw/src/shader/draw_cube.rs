use crate::{cx_draw::CxDraw, draw_list_2d::ManyInstances, makepad_platform::*};

script_mod! {
    use mod.pod.*
    use mod.math.*
    use mod.shader.*
    use mod.draw
    use mod.geom

    mod.draw.DrawCube = mod.std.set_type_default() do #(DrawCube::script_shader(vm)){
        backface_culling: true
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.CubeVertex, geom.CubeGeom)
        view_matrix: uniform(mat4x4f(
            1.0, 0.0, 0.0, 0.0,
            0.0, 1.0, 0.0, 0.0,
            0.0, 0.0, 1.0, 0.0,
            0.0, 0.0, 0.0, 1.0
        ))
        projection_matrix: uniform(mat4x4f(
            1.0, 0.0, 0.0, 0.0,
            0.0, 1.0, 0.0, 0.0,
            0.0, 0.0, 1.0, 0.0,
            0.0, 0.0, 0.0, 1.0
        ))
        clip_ndc: uniform(vec4(-1.0, -1.0, 1.0, 1.0))
        use_pass_camera: uniform(float(0.0))

        lit_color: varying(vec4f)
        world: varying(vec4f)

        get_size: fn() {
            return self.cube_size
        }

        get_pos: fn() {
            return self.cube_pos
        }

        get_color: fn(dp: float) {
            let ambient = self.color.xyz * 0.28;
            let color = ambient + self.color.xyz * dp * 0.72;
            return vec4(color, self.color.w);
        }

        view_with_camera: fn(world: vec4) {
            if self.use_pass_camera > 0.5 {
                return self.draw_pass.camera_view * world
            }
            return self.view_matrix * world
        }

        transform_with_camera: fn(view_pos: vec4) {
            let clip = if self.use_pass_camera > 0.5 {
                self.draw_pass.camera_projection * view_pos
            } else {
                self.projection_matrix * view_pos
            };
            if self.use_pass_camera > 0.5 {
                return clip
            }
            let inv_w = 1.0 / max(abs(clip.w), 0.00001);
            let ndc = vec2(clip.x * inv_w, clip.y * inv_w);
            let clip_min = vec2(self.clip_ndc.x, self.clip_ndc.y);
            let clip_max = vec2(self.clip_ndc.z, self.clip_ndc.w);
            let clip_scale = (clip_max - clip_min) * 0.5;
            let clip_center = (clip_max + clip_min) * 0.5;
            let remapped_ndc = ndc * clip_scale + clip_center;
            return vec4(remapped_ndc.x * clip.w, remapped_ndc.y * clip.w, clip.z, clip.w)
        }

        vertex: fn() {
            let pos = self.get_size() * self.geom.geom_pos + self.get_pos();
            let model_view = self.draw_list.view_transform * self.transform;
            let normal4 = model_view * vec4(
                self.geom.geom_normal.x,
                self.geom.geom_normal.y,
                self.geom.geom_normal.z,
                0.0
            );
            let normal = normalize(normal4.xyz);
            self.world = model_view * vec4(pos.x, pos.y, pos.z, 1.0);
            let view_pos = self.view_with_camera(self.world);
            let dp = max(dot(normal, normalize(self.light_dir)), 0.0);
            self.lit_color = self.get_color(dp);
            self.vertex_pos = self.transform_with_camera(view_pos);
        }

        pixel: fn() {
            return self.lit_color;
        }

        fragment: fn() {
            self.fb0 = depth_clip(self.world, self.pixel(), self.depth_clip);
        }
    }
}

#[derive(Script, ScriptHook, Debug)]
#[repr(C)]
pub struct DrawCube {
    #[rust]
    pub many_instances: Option<ManyInstances>,
    #[rust(Mat4f::identity())]
    pub view_matrix: Mat4f,
    #[rust(Mat4f::identity())]
    pub projection_matrix: Mat4f,
    #[rust(vec4(-1.0, -1.0, 1.0, 1.0))]
    pub clip_ndc: Vec4f,
    #[rust(0.0)]
    pub use_pass_camera: f32,
    #[deref]
    pub draw_vars: DrawVars,
    #[live]
    pub color: Vec4f,
    #[live(vec3(0.35, 0.8, 0.45))]
    pub light_dir: Vec3f,
    #[live]
    pub transform: Mat4f,
    #[live(vec3(1.0, 1.0, 1.0))]
    pub cube_size: Vec3f,
    #[live(vec3(0.0, 0.0, 0.0))]
    pub cube_pos: Vec3f,
    #[live(1.0)]
    pub depth_clip: f32,
}

impl DrawCube {
    pub fn set_use_pass_camera(&mut self, use_pass_camera: bool) {
        self.use_pass_camera = if use_pass_camera { 1.0 } else { 0.0 };
    }

    pub fn set_camera_state(&mut self, view: Mat4f, projection: Mat4f) {
        self.view_matrix = view;
        self.projection_matrix = projection;
    }

    pub fn set_clip_ndc(&mut self, clip_ndc: Vec4f) {
        self.clip_ndc = clip_ndc;
    }

    fn apply_draw_uniforms(&mut self, cx: &mut CxDraw) {
        self.draw_vars
            .set_uniform(cx.cx, live_id!(view_matrix), &self.view_matrix.v);
        self.draw_vars.set_uniform(
            cx.cx,
            live_id!(projection_matrix),
            &self.projection_matrix.v,
        );
        self.draw_vars.set_uniform(
            cx.cx,
            live_id!(clip_ndc),
            &[
                self.clip_ndc.x,
                self.clip_ndc.y,
                self.clip_ndc.z,
                self.clip_ndc.w,
            ],
        );
        self.draw_vars
            .set_uniform(cx.cx, live_id!(use_pass_camera), &[self.use_pass_camera]);
    }

    pub fn draw(&mut self, cx: &mut CxDraw) {
        self.apply_draw_uniforms(cx);
        if let Some(mi) = &mut self.many_instances {
            mi.instances.extend_from_slice(self.draw_vars.as_slice());
        } else if self.draw_vars.can_instance() {
            let new_area = cx.add_instance(&self.draw_vars);
            self.draw_vars.area = cx.update_area_refs(self.draw_vars.area, new_area);
        }
    }

    pub fn new_draw_call(&mut self, cx: &mut CxDraw) {
        self.apply_draw_uniforms(cx);
        cx.new_draw_call(&self.draw_vars);
    }

    pub fn begin_many_instances(&mut self, cx: &mut CxDraw) {
        self.apply_draw_uniforms(cx);
        self.many_instances = cx.begin_many_instances(&self.draw_vars);
    }

    pub fn end_many_instances(&mut self, cx: &mut CxDraw) {
        if let Some(mi) = self.many_instances.take() {
            let new_area = cx.end_many_instances(mi);
            self.draw_vars.area = cx.update_area_refs(self.draw_vars.area, new_area);
        }
    }
}
