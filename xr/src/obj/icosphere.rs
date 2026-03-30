use crate::scene::{xr_widget_world_transform, XrDrawContext, XrNode};
use makepad_widgets::{makepad_derive_widget::*, makepad_draw::*, widget::*};
use std::cell::RefCell;

const ICO_SPHERE_PHYSICS_DIAMETER_SCALE: f32 = 0.88;

thread_local! {
    static ICO_GEOMETRY: RefCell<Option<Geometry>> = const { RefCell::new(None) };
}

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.pod.*
    use mod.math.*
    use mod.shader.*
    use mod.draw
    use mod.geom

    mod.draw.DrawIcoSolid = mod.std.set_type_default() do #(DrawIcoSolid::script_shader(vm)){
        alpha_blend: false
        backface_culling: true
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.IcoVertex, geom.IcoGeom)
        env_texture: texture_cube(float)
        u_has_env_texture: uniform(float(0.0))
        u_light_dir: uniform(vec3(-0.34, 0.88, 0.32))
        u_fill_light_dir: uniform(vec3(0.58, 0.36, -0.73))
        u_ambient: uniform(float(0.11))
        u_key_strength: uniform(float(0.74))
        u_fill_strength: uniform(float(0.24))
        u_reflectivity: uniform(float(0.88))
        u_env_intensity: uniform(float(1.15))

        v_world_clip: varying(vec4f)
        v_world: varying(vec3f)
        v_normal: varying(vec3f)
        v_light: varying(float)

        active_camera_world_pos: fn() -> vec3f {
            let camera_world = self.draw_pass.camera_inv * vec4(0.0, 0.0, 0.0, 1.0);
            return vec3(
                camera_world.x / max(camera_world.w, 0.00001),
                camera_world.y / max(camera_world.w, 0.00001),
                camera_world.z / max(camera_world.w, 0.00001)
            )
        }

        sample_env: fn(dir: vec3f) -> vec3f {
            if self.u_has_env_texture > 0.5 {
                return self.env_texture.sample_as_bgra(dir).xyz
            }
            let t = clamp(dir.y * 0.5 + 0.5, 0.0, 1.0);
            return mix(vec3(0.05, 0.055, 0.065), vec3(0.42, 0.48, 0.56), t)
        }

        vertex: fn() {
            let local_pos = vec3(self.geom.pos.x, self.geom.pos.y, self.geom.pos.z);
            let local_normal = normalize(vec3(
                self.geom.normal.x,
                self.geom.normal.y,
                self.geom.normal.z
            ));
            let model_view = self.draw_list.view_transform * self.transform;
            let world = model_view * vec4(local_pos.x, local_pos.y, local_pos.z, 1.0);
            let world_normal = normalize((model_view * vec4(
                local_normal.x,
                local_normal.y,
                local_normal.z,
                0.0
            )).xyz);
            let key = max(dot(world_normal, normalize(self.u_light_dir)), 0.0) * self.u_key_strength;
            let fill = max(dot(world_normal, normalize(self.u_fill_light_dir)), 0.0) * self.u_fill_strength;
            self.v_light = clamp(self.u_ambient + key + fill, self.u_ambient, 1.0);
            self.v_world = world.xyz;
            self.v_normal = world_normal;
            self.v_world_clip = vec4(world.x, world.y, world.z, 1.0);
            let view_pos = self.draw_pass.camera_view * world;
            self.vertex_pos = self.draw_pass.camera_projection * view_pos;
        }

        pixel: fn() {
            let normal = normalize(self.v_normal);
            let view_dir = normalize(self.active_camera_world_pos() - self.v_world);
            let ndotv = max(dot(normal, view_dir), 0.0);
            let reflection_dir = normalize(normal * (2.0 * ndotv) - view_dir);
            let reflection = self.sample_env(reflection_dir) * self.color.xyz * self.u_env_intensity;
            let fresnel = 0.44 + 0.56 * pow(max(1.0 - ndotv, 0.0), 5.0);
            let base = self.diffuse.xyz * self.v_light;
            let accent = self.color.xyz * self.v_light * (0.10 + 0.08 * self.u_reflectivity);
            let reflect_mix = clamp(self.u_reflectivity * fresnel, 0.0, 1.0);
            let lit = (base + accent) * (1.0 - reflect_mix * 0.45) + reflection * reflect_mix;
            return vec4(lit.x, lit.y, lit.z, self.diffuse.w * self.color.w);
        }

        fragment: fn() {
            self.fb0 = depth_clip(self.v_world_clip, self.pixel(), self.depth_clip);
        }
    }

    mod.widgets.IcoSphereBase = #(IcoSphere::register_widget(vm))
    mod.widgets.IcoSphere = set_type_default() do mod.widgets.IcoSphereBase{
        body: mod.widgets.XrBodyKind.Dynamic
        radius: 0.037
        diffuse: vec4(0.63, 0.65, 0.69, 1.0)
        color: vec4(0.95, 0.62, 0.28, 1.0)
        draw_ico: mod.draw.DrawIcoSolid{
            backface_culling: true
            ambient: 0.16
            key_strength: 0.78
            fill_strength: 0.30
            reflectivity: 0.90
            env_intensity: 1.25
        }
    }
}

#[derive(Script, ScriptHook, Debug)]
#[repr(C)]
pub struct DrawIcoSolid {
    #[rust(0.0)]
    pub has_env_texture: f32,
    #[rust(vec3(-0.34, 0.88, 0.32))]
    pub light_dir: Vec3f,
    #[rust(vec3(0.58, 0.36, -0.73))]
    pub fill_light_dir: Vec3f,
    #[rust(0.11)]
    pub ambient: f32,
    #[rust(0.74)]
    pub key_strength: f32,
    #[rust(0.24)]
    pub fill_strength: f32,
    #[rust(0.88)]
    pub reflectivity: f32,
    #[rust(1.15)]
    pub env_intensity: f32,
    #[deref]
    pub draw_vars: DrawVars,
    #[live]
    pub diffuse: Vec4f,
    #[live]
    pub color: Vec4f,
    #[live]
    pub transform: Mat4f,
    #[live(1.0)]
    pub depth_clip: f32,
}

impl DrawIcoSolid {
    fn set_env_texture(&mut self, texture: Option<Texture>) {
        self.has_env_texture = if texture.is_some() { 1.0 } else { 0.0 };
        self.draw_vars.texture_slots[0] = texture;
    }

    fn apply_uniforms(&mut self, cx: &mut CxDraw) {
        let light_dir = if self.light_dir.length() > 0.000_01 {
            self.light_dir.normalize()
        } else {
            vec3(0.0, 1.0, 0.0)
        };
        let fill_light_dir = if self.fill_light_dir.length() > 0.000_01 {
            self.fill_light_dir.normalize()
        } else {
            vec3(0.0, 1.0, 0.0)
        };
        self.draw_vars
            .set_uniform(cx.cx, live_id!(u_has_env_texture), &[self.has_env_texture]);
        self.draw_vars.set_uniform(
            cx.cx,
            live_id!(u_light_dir),
            &[light_dir.x, light_dir.y, light_dir.z],
        );
        self.draw_vars.set_uniform(
            cx.cx,
            live_id!(u_fill_light_dir),
            &[fill_light_dir.x, fill_light_dir.y, fill_light_dir.z],
        );
        self.draw_vars
            .set_uniform(cx.cx, live_id!(u_ambient), &[self.ambient]);
        self.draw_vars
            .set_uniform(cx.cx, live_id!(u_key_strength), &[self.key_strength]);
        self.draw_vars
            .set_uniform(cx.cx, live_id!(u_fill_strength), &[self.fill_strength]);
        self.draw_vars
            .set_uniform(cx.cx, live_id!(u_reflectivity), &[self.reflectivity]);
        self.draw_vars
            .set_uniform(cx.cx, live_id!(u_env_intensity), &[self.env_intensity]);
    }

    fn draw(&mut self, cx: &mut CxDraw, geometry_id: GeometryId) {
        self.draw_vars.geometry_id = Some(geometry_id);
        self.apply_uniforms(cx);
        if self.draw_vars.can_instance() {
            let new_area = cx.add_instance(&self.draw_vars);
            self.draw_vars.area = cx.update_area_refs(self.draw_vars.area, new_area);
        }
    }
}

#[derive(Script, Widget)]
pub struct IcoSphere {
    #[redraw]
    #[live]
    draw_ico: DrawIcoSolid,
    #[live(0.037)]
    radius: f32,
    #[live(vec4(0.63, 0.65, 0.69, 1.0))]
    diffuse: Vec4f,
    #[live(vec4(0.95, 0.62, 0.28, 1.0))]
    color: Vec4f,
    #[cast]
    #[deref]
    node: XrNode,
}

impl IcoSphere {
    pub fn node(&self) -> &XrNode {
        &self.node
    }
}

impl ScriptHook for IcoSphere {
    fn on_after_apply(
        &mut self,
        _vm: &mut ScriptVm,
        _apply: &Apply,
        _scope: &mut Scope,
        _value: ScriptValue,
    ) {
        let diameter = self.radius.max(0.0) * 2.0;
        self.node.set_implicit_physics_size(vec3f(
            diameter * ICO_SPHERE_PHYSICS_DIAMETER_SCALE,
            diameter * ICO_SPHERE_PHYSICS_DIAMETER_SCALE,
            diameter * ICO_SPHERE_PHYSICS_DIAMETER_SCALE,
        ));
    }
}

impl Widget for IcoSphere {
    fn draw_3d(&mut self, cx: &mut Cx3d, scope: &mut Scope) -> DrawStep {
        if cx.scene_state_3d().is_none() {
            return DrawStep::done();
        }

        let radius = self.radius.max(0.001);
        let geometry_id = shared_ico_geometry_id(cx.cx);
        let world = xr_widget_world_transform(cx, scope, self.widget_uid(), &self.node);
        let local_scale =
            Mat4f::nonuniform_scaled_translation(vec3(radius, radius, radius), vec3(0.0, 0.0, 0.0));
        self.draw_ico.transform = Mat4f::mul(&world, &local_scale);
        self.draw_ico.diffuse = self.diffuse;
        self.draw_ico.color = self.color;
        self.draw_ico.depth_clip = 1.0;
        let draw_context = XrDrawContext::from_scope(scope);
        self.draw_ico.set_env_texture(draw_context.env_texture());
        self.draw_ico.draw(cx, geometry_id);

        self.node.draw_3d(cx, scope)
    }

    fn draw_walk(&mut self, _cx: &mut Cx2d, _scope: &mut Scope, _walk: Walk) -> DrawStep {
        DrawStep::done()
    }
}

fn shared_ico_geometry_id(cx: &mut Cx) -> GeometryId {
    ICO_GEOMETRY.with(|slot| {
        let mut slot = slot.borrow_mut();
        let geometry = slot.get_or_insert_with(|| {
            let (vertices, indices) = build_unit_icosahedron_geometry();
            let geometry = Geometry::new(cx);
            geometry.update(cx, indices, vertices);
            geometry
        });
        geometry.geometry_id()
    })
}

fn build_unit_icosahedron_geometry() -> (Vec<f32>, Vec<u32>) {
    let phi = (1.0 + 5.0_f32.sqrt()) * 0.5;
    let scale = 1.0 / (1.0 + phi * phi).sqrt();
    let verts = [
        vec3f(-1.0, phi, 0.0),
        vec3f(1.0, phi, 0.0),
        vec3f(-1.0, -phi, 0.0),
        vec3f(1.0, -phi, 0.0),
        vec3f(0.0, -1.0, phi),
        vec3f(0.0, 1.0, phi),
        vec3f(0.0, -1.0, -phi),
        vec3f(0.0, 1.0, -phi),
        vec3f(phi, 0.0, -1.0),
        vec3f(phi, 0.0, 1.0),
        vec3f(-phi, 0.0, -1.0),
        vec3f(-phi, 0.0, 1.0),
    ]
    .map(|v| vec3f(v.x * scale, v.y * scale, v.z * scale));
    let faces = [
        [0usize, 11, 5],
        [0, 5, 1],
        [0, 1, 7],
        [0, 7, 10],
        [0, 10, 11],
        [1, 5, 9],
        [5, 11, 4],
        [11, 10, 2],
        [10, 7, 6],
        [7, 1, 8],
        [3, 9, 4],
        [3, 4, 2],
        [3, 2, 6],
        [3, 6, 8],
        [3, 8, 9],
        [4, 9, 5],
        [2, 4, 11],
        [6, 2, 10],
        [8, 6, 7],
        [9, 8, 1],
    ];

    let mut vertices = Vec::with_capacity(faces.len() * 3 * 8);
    let mut indices = Vec::with_capacity(faces.len() * 3);

    for face in faces {
        let p0 = verts[face[0]];
        let mut p1 = verts[face[1]];
        let mut p2 = verts[face[2]];
        let outward = (p0 + p1 + p2).normalize();
        let mut normal = Vec3f::cross(p1 - p0, p2 - p0).normalize();
        if normal.dot(outward) < 0.0 {
            std::mem::swap(&mut p1, &mut p2);
            normal = normal.scale(-1.0);
        }

        let face_vertices = [p0, p1, p2];
        for position in face_vertices {
            vertices.extend_from_slice(&[
                position.x, position.y, position.z, 1.0, normal.x, normal.y, normal.z, 0.0,
            ]);
            indices.push(indices.len() as u32);
        }
    }

    (vertices, indices)
}
