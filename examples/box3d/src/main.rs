// Box3D physics rendered in 3D with makepad.
//
// A pile of boxes (a square pyramid) plus a few heavy spheres dropping into
// it, simulated by the pure-Rust Box3D port (libs/box3d, multithreaded solver)
// and rendered as instanced lit meshes into an offscreen 3D pass (same
// viewport pattern as examples/cad: DrawPass + color/depth textures + XrCamera
// orbit, composited into the UI through DrawXrSceneTexture).
//
// Controls: drag = orbit, wheel = zoom, Space = reset the scene.

pub use makepad_box3d;
pub use makepad_widgets;
pub use makepad_xr;

use makepad_widgets::*;
use makepad_xr::scene::*;

use makepad_box3d::body::{body_get_transform, body_is_awake, create_body};
use makepad_box3d::hull::make_box_hull;
use makepad_box3d::id::BodyId;
use makepad_box3d::physics_world::{create_world, world_step, World};
use makepad_box3d::shape::{create_hull_shape, create_sphere_shape};
use makepad_box3d::types::{
    default_body_def, default_shape_def, default_world_def, BodyType, HullData, Sphere,
};

use std::sync::Arc;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    use mod.math.*
    use mod.shader.*
    use mod.draw
    use mod.geom

    mod.draw.DrawPhysMesh = mod.std.set_type_default() do #(DrawPhysMesh::script_shader(vm)){
        alpha_blend: false
        backface_culling: true
        vertex_pos: vertex_position(vec4f)
        fb0: fragment_output(0, vec4f)
        draw_call: uniform_buffer(draw.DrawCallUniforms)
        draw_pass: uniform_buffer(draw.DrawPassUniforms)
        draw_list: uniform_buffer(draw.DrawListUniforms)
        geom: vertex_buffer(geom.IcoVertex, geom.IcoGeom)
        u_light_dir: uniform(vec3(-0.35, 0.84, 0.42))
        u_fill_dir: uniform(vec3(0.58, 0.35, -0.62))
        v_world_clip: varying(vec4f)
        v_world: varying(vec3f)
        v_normal: varying(vec3f)

        active_camera_world_pos: fn() -> vec3f {
            let camera_world = self.draw_pass.camera_inv * vec4(0.0, 0.0, 0.0, 1.0)
            return vec3(
                camera_world.x / max(camera_world.w, 0.00001),
                camera_world.y / max(camera_world.w, 0.00001),
                camera_world.z / max(camera_world.w, 0.00001)
            )
        }

        vertex: fn() {
            let local_pos = vec3(
                self.geom.pos.x * self.scale.x,
                self.geom.pos.y * self.scale.y,
                self.geom.pos.z * self.scale.z
            )
            let local_normal = normalize(vec3(
                self.geom.normal.x / max(self.scale.x, 0.00001),
                self.geom.normal.y / max(self.scale.y, 0.00001),
                self.geom.normal.z / max(self.scale.z, 0.00001)
            ))
            let model_view = self.draw_list.view_transform * self.transform
            let world = model_view * vec4(local_pos.x, local_pos.y, local_pos.z, 1.0)
            let world_normal = normalize((model_view * vec4(local_normal.x, local_normal.y, local_normal.z, 0.0)).xyz)
            self.v_world = world.xyz
            self.v_normal = world_normal
            self.v_world_clip = vec4(world.x, world.y, world.z, 1.0)
            let view_pos = self.draw_pass.camera_view * world
            self.vertex_pos = self.draw_pass.camera_projection * view_pos
        }

        pixel: fn() {
            let normal = normalize(self.v_normal)
            let view_dir = normalize(self.active_camera_world_pos() - self.v_world)
            let key = max(dot(normal, normalize(self.u_light_dir)), 0.0)
            let fill = max(dot(normal, normalize(self.u_fill_dir)), 0.0)
            let rim = pow(max(1.0 - max(dot(normal, view_dir), 0.0), 0.0), 2.5)
            let lit = 0.18 + key * 0.72 + fill * 0.20 + rim * 0.22
            let color = self.color.xyz * lit + vec3(0.04, 0.05, 0.07) * rim
            return vec4(color, self.color.w)
        }

        fragment: fn() {
            self.fb0 = depth_clip(self.v_world_clip, self.pixel(), self.depth_clip)
        }
    }

    mod.widgets.Box3dViewportBase = #(Box3dViewport::register_widget(vm))
    mod.widgets.Box3dViewport = set_type_default() do mod.widgets.Box3dViewportBase{
        width: Fill
        height: Fill
        clear_color: #x0b1016
        draw_bg: mod.draw.DrawXrSceneTexture{}
        draw_mesh: mod.draw.DrawPhysMesh{
            backface_culling: true
        }
        camera: mod.widgets.XrCamera{
            fov_y: 42.0
            desktop_target: vec3(0.0, 2.4, 0.0)
            distance: 24.0
            distance_min: 4.0
            distance_max: 80.0
            wheel_zoom_step: 0.08
        }
    }

    load_all_resources() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(1200, 760)
                body +: {
                    app_view := SolidView{
                        width: Fill
                        height: Fill
                        flow: Down
                        draw_bg +: {color: #x0d1116}

                        header := SolidView{
                            width: Fill
                            height: 44.0
                            flow: Right
                            align: Align{x: 0.0 y: 0.5}
                            padding: Inset{left: 14.0 right: 14.0}
                            spacing: 12.0
                            draw_bg +: {color: #x171d24}

                            title := H3{
                                text: "Box3D — pure Rust physics"
                                draw_text +: {color: #xdfe7ee}
                            }
                            hint := Label{
                                text: "drag: orbit   wheel: zoom   space: reset"
                                draw_text +: {color: #x8391a0}
                            }
                        }

                        viewport := mod.widgets.Box3dViewport{}
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Instanced lit mesh: non-instance data before #[deref], instance fields after.
// ---------------------------------------------------------------------------

#[derive(Script, ScriptHook, Debug)]
#[repr(C)]
pub struct DrawPhysMesh {
    #[rust(vec3(-0.35, 0.84, 0.42))]
    light_dir: Vec3f,
    #[rust(vec3(0.58, 0.35, -0.62))]
    fill_dir: Vec3f,
    #[deref]
    draw_vars: DrawVars,
    #[live]
    color: Vec4f,
    #[live]
    transform: Mat4f,
    #[live(vec3(1.0, 1.0, 1.0))]
    scale: Vec3f,
    #[live(1.0)]
    depth_clip: f32,
}

impl DrawPhysMesh {
    fn apply_uniforms(&mut self, cx: &mut CxDraw) {
        let light_dir = self.light_dir.normalize();
        let fill_dir = self.fill_dir.normalize();
        self.draw_vars.set_uniform(
            cx.cx,
            live_id!(u_light_dir),
            &[light_dir.x, light_dir.y, light_dir.z],
        );
        self.draw_vars.set_uniform(
            cx.cx,
            live_id!(u_fill_dir),
            &[fill_dir.x, fill_dir.y, fill_dir.z],
        );
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

// ---------------------------------------------------------------------------
// Geometry: unit cube (extents ±1) and unit sphere, 8 floats per vertex
// (pos.xyzw, normal.xyzw) matching geom.IcoVertex.
// ---------------------------------------------------------------------------

fn ensure_cube_geometry(cx: &mut Cx, slot: &mut Option<Geometry>) -> GeometryId {
    let geometry = slot.get_or_insert_with(|| {
        let geometry = Geometry::new(cx);
        let faces: [([f32; 3], [f32; 3], [f32; 3]); 6] = [
            // normal, u axis, v axis
            ([1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]),
            ([-1.0, 0.0, 0.0], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0]),
            ([0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]),
            ([0.0, -1.0, 0.0], [1.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
            ([0.0, 0.0, 1.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]),
            ([0.0, 0.0, -1.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0]),
        ];
        let mut vertices = Vec::with_capacity(6 * 4 * 8);
        let mut indices = Vec::with_capacity(36);
        for (n, u, v) in faces {
            let base = vertices.len() as u32 / 8;
            for (su, sv) in [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                let p = [
                    n[0] + u[0] * su + v[0] * sv,
                    n[1] + u[1] * su + v[1] * sv,
                    n[2] + u[2] * su + v[2] * sv,
                ];
                vertices.extend_from_slice(&[p[0], p[1], p[2], 1.0, n[0], n[1], n[2], 0.0]);
            }
            indices.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
        }
        geometry.update(cx, indices, vertices);
        geometry
    });
    geometry.geometry_id()
}

fn ensure_sphere_geometry(cx: &mut Cx, slot: &mut Option<Geometry>) -> GeometryId {
    let geometry = slot.get_or_insert_with(|| {
        let geometry = Geometry::new(cx);
        let rings = 14usize;
        let sectors = 22usize;
        let mut vertices = Vec::with_capacity((rings + 1) * (sectors + 1) * 8);
        let mut indices = Vec::with_capacity(rings * sectors * 6);
        for r in 0..=rings {
            let phi = std::f32::consts::PI * r as f32 / rings as f32;
            let (sp, cp) = phi.sin_cos();
            for s in 0..=sectors {
                let theta = 2.0 * std::f32::consts::PI * s as f32 / sectors as f32;
                let (st, ct) = theta.sin_cos();
                let n = [sp * ct, cp, sp * st];
                vertices.extend_from_slice(&[n[0], n[1], n[2], 1.0, n[0], n[1], n[2], 0.0]);
            }
        }
        let stride = (sectors + 1) as u32;
        for r in 0..rings as u32 {
            for s in 0..sectors as u32 {
                let a = r * stride + s;
                let b = (r + 1) * stride + s;
                indices.extend_from_slice(&[a, b, b + 1, a, b + 1, a + 1]);
            }
        }
        geometry.update(cx, indices, vertices);
        geometry
    });
    geometry.geometry_id()
}

// ---------------------------------------------------------------------------
// Physics scene
// ---------------------------------------------------------------------------

const PYRAMID_BASE: i32 = 8;
const BOX_HALF: f32 = 0.45;
const SPHERE_RADIUS: f32 = 0.65;
const SPHERE_COUNT: i32 = 8;
const TIME_STEP: f32 = 1.0 / 60.0;
const SUB_STEPS: i32 = 4;

enum ShapeKind {
    Box(Vec3f),
    Sphere(f32),
}

struct PhysBody {
    id: BodyId,
    kind: ShapeKind,
    color: Vec4f,
}

struct PhysicsSim {
    world: World,
    bodies: Vec<PhysBody>,
}

impl PhysicsSim {
    fn new() -> Self {
        let mut world_def = default_world_def();
        world_def.worker_count = 4;
        let mut world = create_world(&world_def);
        let mut bodies = Vec::new();

        let shape_def = default_shape_def();

        // Static ground slab.
        let ground_def = default_body_def();
        let ground_id = create_body(&mut world, &ground_def);
        let ground_hull: Arc<HullData> = make_box_hull(16.0, 0.5, 16.0);
        create_hull_shape(&mut world, ground_id, &shape_def, &ground_hull);
        bodies.push(PhysBody {
            id: ground_id,
            kind: ShapeKind::Box(vec3(16.0, 0.5, 16.0)),
            color: vec4(0.16, 0.19, 0.23, 1.0),
        });

        // Square pyramid of boxes, one shared hull (exercises the hull database).
        let box_hull: Arc<HullData> = make_box_hull(BOX_HALF, BOX_HALF, BOX_HALF);
        let spacing = 2.0 * BOX_HALF + 0.02;
        for level in 0..PYRAMID_BASE {
            let n = PYRAMID_BASE - level;
            let offset = (n - 1) as f32 * spacing * 0.5;
            let y = BOX_HALF + 0.01 + level as f32 * (2.0 * BOX_HALF + 0.01);
            for i in 0..n {
                for j in 0..n {
                    let mut body_def = default_body_def();
                    body_def.body_type = BodyType::Dynamic;
                    body_def.position = makepad_box3d::math_functions::vec3(
                        i as f32 * spacing - offset,
                        y,
                        j as f32 * spacing - offset,
                    );
                    let id = create_body(&mut world, &body_def);
                    create_hull_shape(&mut world, id, &shape_def, &box_hull);

                    let t = level as f32 / (PYRAMID_BASE - 1) as f32;
                    let wob = ((i * 7 + j * 13 + level * 3) % 9) as f32 / 9.0;
                    bodies.push(PhysBody {
                        id,
                        kind: ShapeKind::Box(vec3(BOX_HALF, BOX_HALF, BOX_HALF)),
                        color: vec4(
                            0.82 + 0.10 * t - 0.05 * wob,
                            0.38 + 0.30 * t,
                            0.16 + 0.10 * wob,
                            1.0,
                        ),
                    });
                }
            }
        }

        // Heavy spheres dropped onto the pile.
        for k in 0..SPHERE_COUNT {
            let angle = k as f32 * (2.0 * std::f32::consts::PI / SPHERE_COUNT as f32);
            let mut body_def = default_body_def();
            body_def.body_type = BodyType::Dynamic;
            body_def.position = makepad_box3d::math_functions::vec3(
                2.6 * angle.cos(),
                12.0 + 1.7 * k as f32,
                2.6 * angle.sin(),
            );
            body_def.linear_velocity = makepad_box3d::math_functions::vec3(0.0, -4.0, 0.0);
            let id = create_body(&mut world, &body_def);
            let sphere = Sphere {
                center: makepad_box3d::math_functions::vec3(0.0, 0.0, 0.0),
                radius: SPHERE_RADIUS,
            };
            let mut sphere_def = default_shape_def();
            sphere_def.density = 4.0 * sphere_def.density;
            create_sphere_shape(&mut world, id, &sphere_def, &sphere);
            bodies.push(PhysBody {
                id,
                kind: ShapeKind::Sphere(SPHERE_RADIUS),
                color: vec4(0.24, 0.72, 0.86, 1.0),
            });
        }

        Self { world, bodies }
    }

    fn step(&mut self) {
        world_step(&mut self.world, TIME_STEP, SUB_STEPS);
    }
}

fn body_mat4(world: &World, id: BodyId) -> Mat4f {
    let t = body_get_transform(world, id);
    let pose = Pose::new(
        Quat {
            x: t.q.v.x,
            y: t.q.v.y,
            z: t.q.v.z,
            w: t.q.s,
        },
        vec3(t.p.x as f32, t.p.y as f32, t.p.z as f32),
    );
    pose.to_mat4()
}

// ---------------------------------------------------------------------------
// Viewport widget (pattern from examples/cad CadViewport)
// ---------------------------------------------------------------------------

fn set_pass_camera(cx: &mut Cx, pass: &DrawPass, scene: &SceneState3D) {
    let camera_inv = scene.view.invert();
    let pass_uniforms = &mut cx.passes[pass.draw_pass_id()].pass_uniforms;
    pass_uniforms.camera_projection = scene.projection;
    pass_uniforms.camera_projection_r = scene.projection;
    pass_uniforms.camera_view = scene.view;
    pass_uniforms.camera_view_r = scene.view;
    pass_uniforms.depth_projection = scene.projection;
    pass_uniforms.depth_projection_r = scene.projection;
    pass_uniforms.depth_view = scene.view;
    pass_uniforms.depth_view_r = scene.view;
    pass_uniforms.camera_inv = camera_inv;
    pass_uniforms.camera_inv_r = camera_inv;
}

#[derive(Script, ScriptHook, WidgetRef, WidgetRegister)]
pub struct Box3dViewport {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[live]
    draw_bg: DrawXrSceneTexture,
    #[live]
    draw_mesh: DrawPhysMesh,
    #[live(vec4(0.043, 0.063, 0.086, 1.0))]
    clear_color: Vec4f,
    #[live]
    camera: XrCamera,
    #[new]
    pass: DrawPass,
    #[new]
    draw_list: DrawList,
    #[new]
    color_texture: Texture,
    #[new]
    depth_texture: Texture,
    #[rust]
    area: Area,
    #[rust(false)]
    initialized: bool,
    #[rust]
    cube_geometry: Option<Geometry>,
    #[rust]
    sphere_geometry: Option<Geometry>,
    #[rust]
    sim: Option<PhysicsSim>,
    #[rust]
    next_frame: NextFrame,
}

impl Box3dViewport {
    fn ensure_initialized(&mut self, cx: &mut Cx) {
        if self.initialized {
            return;
        }
        self.initialized = true;
        self.camera.orbit_yaw = 0.72;
        self.camera.orbit_pitch = -0.34;
        self.color_texture = Texture::new_with_format(
            cx,
            TextureFormat::RenderBGRAu8 {
                size: TextureSize::Auto,
                initial: true,
            },
        );
        self.depth_texture = Texture::new_with_format(
            cx,
            TextureFormat::DepthD32 {
                size: TextureSize::Auto,
                initial: true,
            },
        );
        self.pass.set_color_texture(
            cx,
            &self.color_texture,
            DrawPassClearColor::ClearWith(self.clear_color),
        );
        self.pass
            .set_depth_texture(cx, &self.depth_texture, DrawPassClearDepth::ClearWith(1.0));
        cx.passes[self.pass.draw_pass_id()].keep_camera_matrix = true;

        self.sim = Some(PhysicsSim::new());
        self.next_frame = cx.new_next_frame();
    }

    fn reset(&mut self, cx: &mut Cx) {
        self.sim = Some(PhysicsSim::new());
        self.area.redraw(cx);
    }

    fn draw_scene(&mut self, cx: &mut Cx3d, scene_state: SceneState3D) {
        self.draw_list.begin_always(cx);
        cx.begin_scene_3d(scene_state);
        let previous_world = cx.set_scene_world_transform_3d(Mat4f::identity());

        let cube_id = ensure_cube_geometry(cx.cx, &mut self.cube_geometry);
        let sphere_id = ensure_sphere_geometry(cx.cx, &mut self.sphere_geometry);

        if let Some(sim) = &self.sim {
            // Boxes first, then spheres, so instances batch per geometry.
            for round in 0..2 {
                for body in &sim.bodies {
                    let (geometry_id, scale) = match (&body.kind, round) {
                        (ShapeKind::Box(half), 0) => (cube_id, *half),
                        (ShapeKind::Sphere(radius), 1) => (sphere_id, vec3(*radius, *radius, *radius)),
                        _ => continue,
                    };
                    let mut color = body.color;
                    if !body_is_awake(&sim.world, body.id) {
                        color = vec4(color.x * 0.55, color.y * 0.55, color.z * 0.55, color.w);
                    }
                    self.draw_mesh.transform = body_mat4(&sim.world, body.id);
                    self.draw_mesh.scale = scale;
                    self.draw_mesh.color = color;
                    self.draw_mesh.depth_clip = 0.0;
                    self.draw_mesh.draw(cx, geometry_id);
                }
            }
        }

        if let Some(previous_world) = previous_world {
            let _ = cx.set_scene_world_transform_3d(previous_world);
        }
        cx.end_scene_3d();
        self.draw_list.end(cx);
    }
}

impl WidgetNode for Box3dViewport {
    fn widget_uid(&self) -> WidgetUid {
        self.uid
    }

    fn walk(&mut self, _cx: &mut Cx) -> Walk {
        self.walk
    }

    fn area(&self) -> Area {
        self.area
    }

    fn redraw(&mut self, cx: &mut Cx) {
        self.area.redraw(cx);
    }
}

impl Widget for Box3dViewport {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        self.camera.handle_desktop_interaction(cx, event);

        if self.next_frame.is_event(event).is_some() {
            if let Some(sim) = &mut self.sim {
                sim.step();
            }
            self.next_frame = cx.new_next_frame();
            self.area.redraw(cx);
        }

        if let Event::KeyDown(ke) = event {
            if ke.key_code == KeyCode::Space {
                self.reset(cx);
            }
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let rect = cx.walk_turtle_with_area(&mut self.area, walk);
        if rect.size.x <= 1.0 || rect.size.y <= 1.0 {
            return DrawStep::done();
        }

        self.ensure_initialized(cx.cx);
        self.camera.set_desktop_viewport_rect(rect);
        self.pass.set_size(cx, rect.size);
        self.pass.set_color_texture(
            cx,
            &self.color_texture,
            DrawPassClearColor::ClearWith(self.clear_color),
        );
        self.pass
            .set_depth_texture(cx, &self.depth_texture, DrawPassClearDepth::ClearWith(1.0));

        cx.make_child_pass(&self.pass);
        cx.begin_pass(&self.pass, None);
        if let Some(scene_state) = self.camera.desktop_scene_state(rect, cx.time()) {
            set_pass_camera(cx.cx, &self.pass, &scene_state);
            let cx3d = &mut Cx3d::new(cx.cx);
            self.draw_scene(cx3d, scene_state);
        }
        cx.end_pass(&self.pass);

        self.draw_bg.set_scene_texture(&self.color_texture);
        self.draw_bg.draw_abs(cx, rect);
        self.area = self.draw_bg.area();
        cx.set_pass_area(&self.pass, self.area);
        DrawStep::done()
    }
}

// ---------------------------------------------------------------------------
// App
// ---------------------------------------------------------------------------

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
}

impl MatchEvent for App {}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        crate::makepad_xr::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
