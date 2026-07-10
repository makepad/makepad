use makepad_box3d::body as b3body;
use makepad_box3d::hull as b3hull;
use makepad_box3d::id::{BodyId, ShapeId};
use makepad_box3d::math_functions as b3m;
use makepad_box3d::physics_world as b3world;
use makepad_box3d::shape as b3shape;
use makepad_box3d::types as b3t;
use makepad_widgets::event::TouchState;
use makepad_widgets::*;

use crate::util::scene_draw::{
    apply_scene_to_draw_pbr, ray_from_scene_viewport, scene_state_from_cx, SceneState3D,
};

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.PhysicsWorld3DBase = #(PhysicsWorld3D::register_widget(vm))

    mod.widgets.PhysicsWorld3D = set_type_default() do mod.widgets.PhysicsWorld3DBase{
        draw_pbr +: {
            light_dir: vec3(0.35, 0.8, 0.45)
            light_color: vec3(1.0, 1.0, 1.0)
            ambient: 0.25
            spec_power: 128.0
            spec_strength: 0.9
        }
    }

    mod.widgets.PhysicsView = mod.widgets.PhysicsWorld3D{}
}

const CUBE_COLORS: &[[f32; 3]] = &[
    [0.90, 0.30, 0.25],
    [0.25, 0.75, 0.45],
    [0.30, 0.50, 0.90],
    [0.95, 0.75, 0.20],
    [0.80, 0.40, 0.85],
    [0.20, 0.80, 0.80],
    [0.95, 0.55, 0.25],
    [0.60, 0.85, 0.35],
];

const GROUND_COLOR: [f32; 3] = [0.35, 0.38, 0.42];
const PLATFORM_COLOR: [f32; 3] = [0.10, 0.14, 0.18];
const KICK_IMPULSE_MAGNITUDE: f32 = 0.01;
const KICK_UP_BIAS: f32 = 0.35;
const BODY_LINEAR_DAMPING: f32 = 1.5;
const BODY_ANGULAR_DAMPING: f32 = 6.0;
const BODY_SOLVER_SUB_STEPS: i32 = 4;
const BODY_SNAP_SLEEP_LINEAR_SPEED: f32 = 0.03;
const BODY_SNAP_SLEEP_ANGULAR_SPEED: f32 = 1.0;
const CUBE_HALF_EXTENT: f32 = 0.020;
const PLATFORM_HALF_WIDTH: f32 = 0.64;
const PLATFORM_HALF_HEIGHT: f32 = 0.012;
const PLATFORM_HALF_DEPTH: f32 = 0.16;
const PLATFORM_TOP_Y: f32 = 0.45;
const WALL_BRICK_HALF_WIDTH: f32 = CUBE_HALF_EXTENT * 2.0;
const WALL_BRICK_HALF_HEIGHT: f32 = CUBE_HALF_EXTENT;
const WALL_BRICK_HALF_DEPTH: f32 = CUBE_HALF_EXTENT;
const WALL_FULL_ROW_BRICKS: usize = 12;
const WALL_SHORT_ROW_BRICKS: usize = 11;
const WALL_ROWS: usize = 12;
const WALL_SPAWN_GAP: f32 = 0.0;
const CUBE_ROUND_RADIUS: f32 = 0.0032;
const PLATFORM_ROUND_RADIUS: f32 = 0.005;
const PBR_FACE_SUBDIVISIONS: usize = 1;
const PBR_CORNER_SEGMENTS: usize = 3;

#[derive(Clone, Copy)]
struct PhysicsCube {
    body: BodyId,
    half_extents: Vec3f,
    color_index: usize,
}

struct PhysicsScene {
    world: b3world::World,
    cubes: Vec<PhysicsCube>,
    platform_pose: Pose,
}

fn b3_vec3(v: Vec3f) -> b3m::Vec3 {
    b3m::vec3(v.x, v.y, v.z)
}

fn b3_pos(v: Vec3f) -> b3m::Pos {
    b3m::pos(v.x, v.y, v.z)
}

#[allow(clippy::unnecessary_cast)]
fn makepad_pose_from_b3(transform: b3m::WorldTransform) -> Pose {
    Pose {
        orientation: Quat {
            x: transform.q.v.x,
            y: transform.q.v.y,
            z: transform.q.v.z,
            w: transform.q.s,
        },
        position: vec3f(
            transform.p.x as f32,
            transform.p.y as f32,
            transform.p.z as f32,
        ),
    }
}

impl PhysicsScene {
    fn spawn_dynamic_box(&mut self, center: Vec3f, half_extents: Vec3f) {
        let mut body_def = b3t::default_body_def();
        body_def.body_type = b3t::BodyType::Dynamic;
        body_def.position = b3_pos(center);
        body_def.linear_damping = BODY_LINEAR_DAMPING;
        body_def.angular_damping = BODY_ANGULAR_DAMPING;
        let body = b3body::create_body(&mut self.world, &body_def);
        let mut shape_def = b3t::default_shape_def();
        shape_def.density = 1.0;
        shape_def.base_material.friction = 0.8;
        shape_def.base_material.restitution = 0.0;
        let hull = b3hull::make_box_hull(half_extents.x, half_extents.y, half_extents.z);
        b3shape::create_hull_shape(&mut self.world, body, &shape_def, &hull);
        self.cubes.push(PhysicsCube {
            body,
            half_extents,
            color_index: self.cubes.len() % CUBE_COLORS.len(),
        });
    }

    fn spawn_fixed_box(&mut self, center: Vec3f, half_extents: Vec3f, friction: f32) {
        let mut body_def = b3t::default_body_def();
        body_def.position = b3_pos(center);
        let body = b3body::create_body(&mut self.world, &body_def);
        let mut shape_def = b3t::default_shape_def();
        shape_def.base_material.friction = friction;
        let hull = b3hull::make_box_hull(half_extents.x, half_extents.y, half_extents.z);
        b3shape::create_hull_shape(&mut self.world, body, &shape_def, &hull);
    }

    fn new() -> Self {
        let mut world_def = b3t::default_world_def();
        world_def.gravity = b3m::vec3(0.0, -9.81, 0.0);
        world_def.worker_count = 0;
        let mut scene = Self {
            world: b3world::create_world(&world_def),
            cubes: Vec::new(),
            platform_pose: Pose::new(
                Quat::default(),
                vec3f(0.0, PLATFORM_TOP_Y - PLATFORM_HALF_HEIGHT, 0.0),
            ),
        };

        // Ground plane (box3d has no half-space shape; use a big thin slab
        // whose top face is at y = 0).
        scene.spawn_fixed_box(vec3f(0.0, -0.5, 0.0), vec3f(200.0, 0.5, 200.0), 0.9);

        scene.spawn_fixed_box(
            vec3f(0.0, PLATFORM_TOP_Y - PLATFORM_HALF_HEIGHT, 0.0),
            vec3f(
                PLATFORM_HALF_WIDTH,
                PLATFORM_HALF_HEIGHT,
                PLATFORM_HALF_DEPTH,
            ),
            0.9,
        );

        let brick_half_extents = vec3f(
            WALL_BRICK_HALF_WIDTH,
            WALL_BRICK_HALF_HEIGHT,
            WALL_BRICK_HALF_DEPTH,
        );
        let brick_width = WALL_BRICK_HALF_WIDTH * 2.0 + WALL_SPAWN_GAP;
        let brick_height = WALL_BRICK_HALF_HEIGHT * 2.0 + WALL_SPAWN_GAP;
        for row in 0..WALL_ROWS {
            let bricks_in_row = if row % 2 == 0 {
                WALL_FULL_ROW_BRICKS
            } else {
                WALL_SHORT_ROW_BRICKS
            };
            let row_center_offset = (bricks_in_row as f32 - 1.0) * 0.5;
            for brick in 0..bricks_in_row {
                let center = vec3f(
                    (brick as f32 - row_center_offset) * brick_width,
                    PLATFORM_TOP_Y
                        + WALL_BRICK_HALF_HEIGHT
                        + WALL_SPAWN_GAP
                        + row as f32 * brick_height,
                    0.0,
                );
                scene.spawn_dynamic_box(center, brick_half_extents);
            }
        }

        scene.step();
        scene
    }

    fn step(&mut self) {
        b3world::world_step(&mut self.world, 1.0 / 120.0, BODY_SOLVER_SUB_STEPS);
        self.settle_resting_bodies();
    }

    fn settle_resting_bodies(&mut self) {
        let linear_speed_sq = BODY_SNAP_SLEEP_LINEAR_SPEED * BODY_SNAP_SLEEP_LINEAR_SPEED;
        let angular_speed_sq = BODY_SNAP_SLEEP_ANGULAR_SPEED * BODY_SNAP_SLEEP_ANGULAR_SPEED;
        let mut to_sleep = Vec::new();

        for cube in &self.cubes {
            let has_active_contact =
                b3body::body_get_contact_capacity(&self.world, cube.body) > 0;
            if !has_active_contact {
                continue;
            }
            if !b3body::body_is_awake(&self.world, cube.body) {
                continue;
            }

            let linvel = b3body::body_get_linear_velocity(&self.world, cube.body);
            let angvel = b3body::body_get_angular_velocity(&self.world, cube.body);
            let linvel_sq = linvel.x * linvel.x + linvel.y * linvel.y + linvel.z * linvel.z;
            let angvel_sq = angvel.x * angvel.x + angvel.y * angvel.y + angvel.z * angvel.z;
            if linvel_sq <= linear_speed_sq && angvel_sq <= angular_speed_sq {
                to_sleep.push(cube.body);
            }
        }

        for handle in to_sleep {
            b3body::body_set_linear_velocity(&mut self.world, handle, b3m::Vec3::ZERO);
            b3body::body_set_angular_velocity(&mut self.world, handle, b3m::Vec3::ZERO);
        }
    }

    fn apply_kick(&mut self, ray_origin: Vec3f, ray_dir: Vec3f, time: f64) -> bool {
        let hit_body = {
            let world = &self.world;
            let mut best: Option<(ShapeId, f32)> = None;
            {
                let best = &mut best;
                let mut callback = |shape_id: ShapeId,
                                    _point: b3m::Pos,
                                    _normal: b3m::Vec3,
                                    fraction: f32,
                                    _user_material_id: u64,
                                    _triangle_index: i32,
                                    _child_index: i32|
                 -> f32 {
                    // Only kick dynamic bodies.
                    let body = b3shape::shape_get_body(world, shape_id);
                    if b3body::body_get_type(world, body) != b3t::BodyType::Dynamic {
                        return -1.0;
                    }
                    *best = Some((shape_id, fraction));
                    fraction
                };
                b3world::world_cast_ray(
                    world,
                    b3_pos(ray_origin),
                    b3_vec3(ray_dir.normalize() * 1000.0),
                    b3t::default_query_filter(),
                    &mut callback,
                );
            }
            best.map(|(shape_id, _)| b3shape::shape_get_body(world, shape_id))
        };

        if let Some(body_handle) = hit_body {
            let seed = (time * 1000.0) as u32 ^ ((body_handle.index1 as u32) * 2654435761);
            let rx = ((seed & 0xFF) as f32 / 127.5) - 1.0;
            let rz = (((seed >> 8) & 0xFF) as f32 / 127.5) - 1.0;
            let kick_dir = vec3f(rx, KICK_UP_BIAS + 0.5, rz).normalize();
            b3body::body_apply_linear_impulse_to_center(
                &mut self.world,
                body_handle,
                b3_vec3(kick_dir * KICK_IMPULSE_MAGNITUDE),
                true,
            );
            return true;
        }

        false
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct PhysicsWorld3D {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[redraw]
    #[live]
    draw_pbr: DrawPbr,
    #[rust]
    ground_mesh: Option<usize>,
    #[rust]
    scene: Option<PhysicsScene>,
    #[rust]
    next_frame: NextFrame,
    #[rust]
    time: f64,
    #[rust]
    last_scene_state: Option<SceneState3D>,
    #[rust]
    initialized: bool,
}

impl PhysicsWorld3D {
    fn ensure_initialized(&mut self, cx: &mut Cx3d) {
        if self.initialized {
            return;
        }
        self.initialized = true;

        let (ground_positions, ground_normals, ground_indices) = build_ground_grid_mesh(64, 24.0);
        match self.draw_pbr.upload_indexed_triangles_mesh(
            cx,
            &ground_positions[..],
            Some(&ground_normals[..]),
            None,
            None,
            None,
            &ground_indices[..],
        ) {
            Ok(handle) => self.ground_mesh = Some(handle),
            Err(error) => log!("Failed to upload ground mesh: {}", error),
        }

        self.scene = Some(PhysicsScene::new());
    }

    fn kick_cube_at(&mut self, abs: DVec2) -> bool {
        let Some(scene_state) = self.last_scene_state else {
            return false;
        };
        let Some((ray_origin, ray_dir)) = ray_from_scene_viewport(&scene_state, abs) else {
            return false;
        };
        let Some(scene) = self.scene.as_mut() else {
            return false;
        };
        scene.apply_kick(ray_origin, ray_dir, self.time)
    }

    fn draw_scene(&mut self, cx: &mut Cx3d, scene_state: &SceneState3D) {
        if scene_state.viewport_rect.size.x <= 1.0 || scene_state.viewport_rect.size.y <= 1.0 {
            return;
        }

        self.draw_pbr.set_base_color_texture(None);
        self.draw_pbr.set_metal_roughness_texture(None);
        self.draw_pbr.set_normal_texture(None);
        self.draw_pbr.set_occlusion_texture(None);
        self.draw_pbr.set_emissive_texture(None);
        let env_texture = self.draw_pbr.default_env_texture(cx);
        self.draw_pbr.set_env_texture(Some(env_texture));

        if let Some(ground_mesh) = self.ground_mesh {
            let ground_pose = Pose {
                position: vec3f(0.0, -0.002, 0.0),
                orientation: Quat::default(),
            };
            self.draw_pbr
                .set_transform(pose_scaled_model(&ground_pose, vec3f(1.0, 1.0, 1.0)));
            self.draw_pbr.set_base_color_factor(vec4(
                GROUND_COLOR[0],
                GROUND_COLOR[1],
                GROUND_COLOR[2],
                1.0,
            ));
            self.draw_pbr.set_metal_roughness(0.0, 0.85);
            let _ = self.draw_pbr.draw_mesh(cx, ground_mesh);
        }

        if let Some(scene) = &self.scene {
            self.draw_pbr.set_transform(pose_scaled_model(
                &scene.platform_pose,
                vec3f(1.0, 1.0, 1.0),
            ));
            self.draw_pbr.set_base_color_factor(vec4(
                PLATFORM_COLOR[0],
                PLATFORM_COLOR[1],
                PLATFORM_COLOR[2],
                1.0,
            ));
            self.draw_pbr.set_metal_roughness(0.0, 0.55);
            let _ = self.draw_pbr.draw_rounded_cube(
                cx,
                vec3f(
                    PLATFORM_HALF_WIDTH,
                    PLATFORM_HALF_HEIGHT,
                    PLATFORM_HALF_DEPTH,
                ),
                PLATFORM_ROUND_RADIUS,
                PBR_FACE_SUBDIVISIONS,
                PBR_CORNER_SEGMENTS,
            );
        }

        self.draw_pbr.set_metal_roughness(0.0, 0.55);
        if let Some(scene) = &self.scene {
            for cube in &scene.cubes {
                let color = CUBE_COLORS[cube.color_index];
                let pose =
                    makepad_pose_from_b3(b3body::body_get_transform(&scene.world, cube.body));
                self.draw_pbr
                    .set_transform(pose_scaled_model(&pose, vec3(1.0, 1.0, 1.0)));
                self.draw_pbr
                    .set_base_color_factor(vec4(color[0], color[1], color[2], 1.0));
                let _ = self.draw_pbr.draw_rounded_cube(
                    cx,
                    cube.half_extents,
                    CUBE_ROUND_RADIUS,
                    PBR_FACE_SUBDIVISIONS,
                    PBR_CORNER_SEGMENTS,
                );
            }
        }
    }
}

impl Widget for PhysicsWorld3D {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        match event {
            Event::MouseDown(event) => {
                if event.button == MouseButton::PRIMARY
                    && event.handled.get().is_empty()
                    && self.kick_cube_at(event.abs)
                {
                    cx.redraw_all();
                }
            }
            Event::TouchUpdate(event) => {
                for touch in &event.touches {
                    if touch.state == TouchState::Start
                        && touch.handled.get().is_empty()
                        && self.kick_cube_at(touch.abs)
                    {
                        cx.redraw_all();
                        break;
                    }
                }
            }
            Event::NextFrame(event) => {
                self.time = event.time;
                if let Some(scene) = &mut self.scene {
                    scene.step();
                }
                cx.redraw_all();
                self.next_frame = cx.new_next_frame();
            }
            Event::Startup => {
                self.next_frame = cx.new_next_frame();
            }
            _ => {}
        }
    }

    fn draw_3d(&mut self, cx: &mut Cx3d, _scope: &mut Scope) -> DrawStep {
        let Some(scene_state) = scene_state_from_cx(cx) else {
            return DrawStep::done();
        };
        let _ = apply_scene_to_draw_pbr(&mut self.draw_pbr, cx);
        self.ensure_initialized(cx);
        self.last_scene_state = Some(scene_state);
        self.draw_scene(cx, &scene_state);
        DrawStep::done()
    }

    fn draw_walk(&mut self, _cx: &mut Cx2d, _scope: &mut Scope, _walk: Walk) -> DrawStep {
        DrawStep::done()
    }
}

fn pose_scaled_model(pose: &Pose, scale: Vec3f) -> Mat4f {
    let pose_mat = pose.to_mat4();
    Mat4f {
        v: [
            pose_mat.v[0] * scale.x,
            pose_mat.v[1] * scale.x,
            pose_mat.v[2] * scale.x,
            pose_mat.v[3],
            pose_mat.v[4] * scale.y,
            pose_mat.v[5] * scale.y,
            pose_mat.v[6] * scale.y,
            pose_mat.v[7],
            pose_mat.v[8] * scale.z,
            pose_mat.v[9] * scale.z,
            pose_mat.v[10] * scale.z,
            pose_mat.v[11],
            pose_mat.v[12],
            pose_mat.v[13],
            pose_mat.v[14],
            pose_mat.v[15],
        ],
    }
}

fn build_ground_grid_mesh(
    subdiv: usize,
    half_extent: f32,
) -> (Vec<[f32; 3]>, Vec<[f32; 3]>, Vec<u32>) {
    let n = subdiv.max(1);
    let row = n + 1;
    let mut positions = Vec::with_capacity(row * row);
    let mut normals = Vec::with_capacity(row * row);
    let mut indices = Vec::with_capacity(n * n * 6);

    for z in 0..=n {
        let tz = z as f32 / n as f32;
        let pz = -half_extent + tz * (2.0 * half_extent);
        for x in 0..=n {
            let tx = x as f32 / n as f32;
            let px = -half_extent + tx * (2.0 * half_extent);
            positions.push([px, 0.0, pz]);
            normals.push([0.0, 1.0, 0.0]);
        }
    }

    for z in 0..n {
        for x in 0..n {
            let i0 = (z * row + x) as u32;
            let i1 = (z * row + x + 1) as u32;
            let i2 = ((z + 1) * row + x + 1) as u32;
            let i3 = ((z + 1) * row + x) as u32;
            indices.extend_from_slice(&[i0, i3, i2, i2, i1, i0]);
        }
    }

    (positions, normals, indices)
}
