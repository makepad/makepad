use crate::{cx_2d::Cx2d, draw_list_2d::ManyInstances, makepad_platform::*};

script_mod! {
    use mod.pod.*
    use mod.math.*
    use mod.shader.*
    use mod.draw
    use mod.geom

    mod.draw.DrawCube = mod.std.set_type_default() do #(DrawCube::script_shader(vm)){
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.CubeVertex, geom.CubeGeom)

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
            let view_pos = self.draw_pass.camera_view * self.world;
            let view_normal4 = self.draw_pass.camera_view * vec4(normal.x, normal.y, normal.z, 0.0);
            let view_normal = normalize(view_normal4.xyz);
            if dot(view_normal, -view_pos.xyz) <= 0.0 {
                self.vertex_pos = vec4(2.0, 2.0, 2.0, 1.0);
                return
            }

            let dp = max(dot(normal, normalize(vec3(0.35, 0.8, 0.45))), 0.0);
            self.lit_color = self.get_color(dp);
            self.vertex_pos = self.draw_pass.camera_projection * view_pos;
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
    #[deref]
    pub draw_vars: DrawVars,
    #[live]
    pub color: Vec4f,
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
    pub fn draw(&mut self, cx: &mut Cx2d) {
        if let Some(mi) = &mut self.many_instances {
            mi.instances.extend_from_slice(self.draw_vars.as_slice());
        } else if self.draw_vars.can_instance() {
            let new_area = cx.add_aligned_instance(&self.draw_vars);
            self.draw_vars.area = cx.update_area_refs(self.draw_vars.area, new_area);
        }
    }

    pub fn new_draw_call(&mut self, cx: &mut Cx2d) {
        cx.new_draw_call(&self.draw_vars);
    }

    pub fn begin_many_instances(&mut self, cx: &mut Cx2d) {
        self.many_instances = cx.begin_many_aligned_instances(&self.draw_vars);
    }

    pub fn end_many_instances(&mut self, cx: &mut Cx2d) {
        if let Some(mi) = self.many_instances.take() {
            let new_area = cx.end_many_instances(mi);
            self.draw_vars.area = cx.update_area_refs(self.draw_vars.area, new_area);
        }
    }
}
