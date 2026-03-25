use crate::{makepad_derive_widget::*, makepad_draw::*, widget::*};
use std::cell::RefCell;

use super::{
    xr_node::xr_widget_world_transform,
    XrNode,
};

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
        geom: vertex_buffer(geom.PbrVertex, geom.PbrGeom)

        v_world_clip: varying(vec4f)
        v_light: varying(float)

        vertex: fn() {
            let safe_scale = vec3(
                max(abs(self.local_scale.x), 0.000001),
                max(abs(self.local_scale.y), 0.000001),
                max(abs(self.local_scale.z), 0.000001)
            );
            let local_pos = vec3(
                self.geom.pos_nx.x * self.local_scale.x,
                self.geom.pos_nx.y * self.local_scale.y,
                self.geom.pos_nx.z * self.local_scale.z
            );
            let local_normal = normalize(vec3(
                self.geom.pos_nx.w / safe_scale.x,
                self.geom.ny_nz_uv.x / safe_scale.y,
                self.geom.ny_nz_uv.y / safe_scale.z
            ));
            let model_view = self.draw_list.view_transform * self.transform;
            let world = model_view * vec4(local_pos.x, local_pos.y, local_pos.z, 1.0);
            let world_normal = normalize((model_view * vec4(
                local_normal.x,
                local_normal.y,
                local_normal.z,
                0.0
            )).xyz);
            let key = max(dot(world_normal, normalize(self.light_dir)), 0.0) * self.key_strength;
            let fill = max(dot(world_normal, normalize(self.fill_light_dir)), 0.0) * self.fill_strength;
            self.v_light = clamp(self.ambient + key + fill, self.ambient, 1.0);
            self.v_world_clip = world;
            let view_pos = self.draw_pass.camera_view * world;
            self.vertex_pos = self.draw_pass.camera_projection * view_pos;
        }

        pixel: fn() {
            let lit = vec3(self.color.x, self.color.y, self.color.z) * self.v_light;
            return vec4(lit.x, lit.y, lit.z, self.color.w);
        }

        fragment: fn() {
            self.fb0 = depth_clip(self.v_world_clip, self.pixel(), self.depth_clip);
        }
    }

    mod.widgets.IcoSphereBase = #(IcoSphere::register_widget(vm))
    mod.widgets.IcoSphere = set_type_default() do mod.widgets.IcoSphereBase{
        body: mod.widgets.XrBodyKind.Dynamic
        radius: 0.037
        color: vec4(0.95, 0.62, 0.28, 1.0)
        draw_ico: mod.draw.DrawIcoSolid{
            backface_culling: true
            light_dir: vec3(-0.34, 0.88, 0.32)
            fill_light_dir: vec3(0.58, 0.36, -0.73)
            ambient: 0.11
            key_strength: 0.74
            fill_strength: 0.24
        }
    }
}

#[derive(Script, ScriptHook, Debug)]
#[repr(C)]
pub struct DrawIcoSolid {
    #[deref]
    pub draw_vars: DrawVars,
    #[live]
    pub color: Vec4f,
    #[live(vec3(-0.34, 0.88, 0.32))]
    pub light_dir: Vec3f,
    #[live(vec3(0.58, 0.36, -0.73))]
    pub fill_light_dir: Vec3f,
    #[live(0.11)]
    pub ambient: f32,
    #[live(0.74)]
    pub key_strength: f32,
    #[live(0.24)]
    pub fill_strength: f32,
    #[live]
    pub transform: Mat4f,
    #[live(vec3(1.0, 1.0, 1.0))]
    pub local_scale: Vec3f,
    #[live(1.0)]
    pub depth_clip: f32,
}

impl DrawIcoSolid {
    fn draw(&mut self, cx: &mut CxDraw, geometry_id: GeometryId) {
        self.draw_vars.geometry_id = Some(geometry_id);
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
        self.node
            .set_implicit_physics_size(vec3f(
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
        self.draw_ico.transform =
            xr_widget_world_transform(cx, scope, self.widget_uid(), &self.node);
        self.draw_ico.local_scale = vec3(radius, radius, radius);
        self.draw_ico.color = self.color;
        self.draw_ico.depth_clip = 1.0;
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

    let mut vertices = Vec::with_capacity(faces.len() * 3 * 16);
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
                position.x,
                position.y,
                position.z,
                normal.x,
                normal.y,
                normal.z,
                0.0,
                0.0,
                1.0,
                1.0,
                1.0,
                1.0,
                1.0,
                0.0,
                0.0,
                1.0,
            ]);
            indices.push(indices.len() as u32);
        }
    }

    (vertices, indices)
}
