use ::rapier3d::prelude::{
    BroadPhaseBvh, CCDSolver, ColliderBuilder, ColliderSet, ImpulseJointSet, IntegrationParameters,
    IslandManager, MultibodyJointSet, NarrowPhase, PhysicsPipeline, QueryFilter, Ray as RapierRay,
    Real as RapierReal, RigidBodyBuilder, RigidBodyHandle, RigidBodySet,
    Rotation as RapierRotation, SharedShape, Vector as RapierVector,
};
use makepad_widgets::*;
use makepad_widgets::event::TouchState;

use crate::scene_3d::{
    apply_scene_to_draw_pbr, ray_from_scene_viewport, scene_state_from_scope, SceneState3D,
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
const BODY_ADDITIONAL_SOLVER_ITERATIONS: usize = 4;
const BODY_SLEEP_ANGULAR_THRESHOLD: f32 = 2.0;
const BODY_SLEEP_TIME: f32 = 0.35;
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
    body: RigidBodyHandle,
    collider: ::rapier3d::prelude::ColliderHandle,
    half_extents: Vec3f,
    color_index: usize,
}

struct RapierScene {
    gravity: RapierVector,
    integration_parameters: IntegrationParameters,
    pipeline: PhysicsPipeline,
    islands: IslandManager,
    broad_phase: BroadPhaseBvh,
    narrow_phase: NarrowPhase,
    bodies: RigidBodySet,
    colliders: ColliderSet,
    impulse_joints: ImpulseJointSet,
    multibody_joints: MultibodyJointSet,
    ccd_solver: CCDSolver,
    cubes: Vec<PhysicsCube>,
    platform_pose: Pose,
}

impl RapierScene {
    fn spawn_dynamic_box(&mut self, center: RapierVector, half_extents: Vec3f) {
        let body = self.bodies.insert(
            RigidBodyBuilder::dynamic()
                .translation(center)
                .linear_damping(BODY_LINEAR_DAMPING)
                .angular_damping(BODY_ANGULAR_DAMPING)
                .additional_solver_iterations(BODY_ADDITIONAL_SOLVER_ITERATIONS),
        );
        if let Some(rigid_body) = self.bodies.get_mut(body) {
            let activation = rigid_body.activation_mut();
            activation.angular_threshold = BODY_SLEEP_ANGULAR_THRESHOLD;
            activation.time_until_sleep = BODY_SLEEP_TIME;
        }
        let collider = self.colliders.insert_with_parent(
            ColliderBuilder::cuboid(half_extents.x, half_extents.y, half_extents.z)
                .density(1.0)
                .friction(0.8)
                .restitution(0.0),
            body,
            &mut self.bodies,
        );
        self.cubes.push(PhysicsCube {
            body,
            collider,
            half_extents,
            color_index: self.cubes.len() % CUBE_COLORS.len(),
        });
    }

    fn new() -> Self {
        let mut scene = Self {
            gravity: RapierVector::new(0.0, -9.81, 0.0),
            integration_parameters: IntegrationParameters {
                dt: 1.0 / 120.0,
                ..IntegrationParameters::default()
            },
            pipeline: PhysicsPipeline::new(),
            islands: IslandManager::new(),
            broad_phase: BroadPhaseBvh::new(),
            narrow_phase: NarrowPhase::new(),
            bodies: RigidBodySet::new(),
            colliders: ColliderSet::new(),
            impulse_joints: ImpulseJointSet::new(),
            multibody_joints: MultibodyJointSet::new(),
            ccd_solver: CCDSolver::new(),
            cubes: Vec::new(),
            platform_pose: Pose::new(
                Quat::default(),
                vec3f(0.0, PLATFORM_TOP_Y - PLATFORM_HALF_HEIGHT, 0.0),
            ),
        };

        let ground = scene.bodies.insert(RigidBodyBuilder::fixed().build());
        scene.colliders.insert_with_parent(
            ColliderBuilder::new(SharedShape::halfspace(RapierVector::new(0.0, 1.0, 0.0)))
                .friction(0.9),
            ground,
            &mut scene.bodies,
        );

        let platform =
            scene
                .bodies
                .insert(RigidBodyBuilder::fixed().translation(RapierVector::new(
                    0.0,
                    PLATFORM_TOP_Y - PLATFORM_HALF_HEIGHT,
                    0.0,
                )));
        scene.colliders.insert_with_parent(
            ColliderBuilder::cuboid(
                PLATFORM_HALF_WIDTH,
                PLATFORM_HALF_HEIGHT,
                PLATFORM_HALF_DEPTH,
            )
            .friction(0.9),
            platform,
            &mut scene.bodies,
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
                let center = RapierVector::new(
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
        self.pipeline.step(
            self.gravity,
            &self.integration_parameters,
            &mut self.islands,
            &mut self.broad_phase,
            &mut self.narrow_phase,
            &mut self.bodies,
            &mut self.colliders,
            &mut self.impulse_joints,
            &mut self.multibody_joints,
            &mut self.ccd_solver,
            &(),
            &(),
        );
        self.settle_resting_bodies();
    }

    fn settle_resting_bodies(&mut self) {
        let linear_speed_sq = BODY_SNAP_SLEEP_LINEAR_SPEED * BODY_SNAP_SLEEP_LINEAR_SPEED;
        let angular_speed_sq = BODY_SNAP_SLEEP_ANGULAR_SPEED * BODY_SNAP_SLEEP_ANGULAR_SPEED;
        let mut to_sleep = Vec::new();

        for cube in &self.cubes {
            let has_active_contact = self
                .narrow_phase
                .contact_pairs_with(cube.collider)
                .any(|pair| pair.has_any_active_contact());
            if !has_active_contact {
                continue;
            }

            let Some(body) = self.bodies.get(cube.body) else {
                continue;
            };
            if body.is_sleeping() {
                continue;
            }

            let linvel = body.linvel();
            let angvel = body.angvel();
            let linvel_sq = linvel.x * linvel.x + linvel.y * linvel.y + linvel.z * linvel.z;
            let angvel_sq = angvel.x * angvel.x + angvel.y * angvel.y + angvel.z * angvel.z;
            if linvel_sq <= linear_speed_sq && angvel_sq <= angular_speed_sq {
                to_sleep.push(cube.body);
            }
        }

        for handle in to_sleep {
            if let Some(body) = self.bodies.get_mut(handle) {
                body.set_linvel(RapierVector::ZERO, false);
                body.set_angvel(RapierVector::ZERO, false);
            }
        }
    }

    fn apply_kick(&mut self, ray_origin: Vec3f, ray_dir: Vec3f, time: f64) -> bool {
        let hit_body = {
            let query_pipeline = self.broad_phase.as_query_pipeline(
                self.narrow_phase.query_dispatcher(),
                &self.bodies,
                &self.colliders,
                QueryFilter::only_dynamic().exclude_sensors(),
            );
            let ray = RapierRay::new(rapier_vec3(ray_origin), rapier_vec3(ray_dir.normalize()));

            query_pipeline
                .cast_ray(&ray, RapierReal::MAX, true)
                .and_then(|(collider_handle, _)| {
                    self.colliders
                        .get(collider_handle)
                        .and_then(|collider| collider.parent())
                })
        };

        if let Some(body_handle) = hit_body {
            let seed = (time * 1000.0) as u32 ^ (body_handle.into_raw_parts().0 * 2654435761);
            let rx = ((seed & 0xFF) as f32 / 127.5) - 1.0;
            let rz = (((seed >> 8) & 0xFF) as f32 / 127.5) - 1.0;
            let kick_dir = vec3f(rx, KICK_UP_BIAS + 0.5, rz).normalize();
            if let Some(body) = self.bodies.get_mut(body_handle) {
                body.apply_impulse(rapier_vec3(kick_dir * KICK_IMPULSE_MAGNITUDE), true);
                return true;
            }
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
    scene: Option<RapierScene>,
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
    fn ensure_initialized(&mut self, cx: &mut Cx2d) {
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

        self.scene = Some(RapierScene::new());
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

    fn draw_scene(&mut self, cx: &mut Cx2d, scene_state: &SceneState3D) {
        if scene_state.viewport_rect.size.x <= 1.0 || scene_state.viewport_rect.size.y <= 1.0 {
            return;
        }

        apply_scene_to_draw_pbr(&mut self.draw_pbr, cx, scene_state);
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
                if let Some(body) = scene.bodies.get(cube.body) {
                    let color = CUBE_COLORS[cube.color_index];
                    let pose = makepad_pose_from_rapier(body.translation(), *body.rotation());
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

    fn draw_3d(&mut self, cx: &mut Cx3d, scope: &mut Scope) -> DrawStep {
        let Some(scene_state) = scene_state_from_scope(scope) else {
            return DrawStep::done();
        };
        let cx = &mut Cx2d::new(cx.cx);
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

fn rapier_vec3(v: Vec3f) -> RapierVector {
    RapierVector::new(v.x, v.y, v.z)
}

fn makepad_pose_from_rapier(translation: RapierVector, rotation: RapierRotation) -> Pose {
    Pose {
        orientation: Quat {
            x: rotation.x,
            y: rotation.y,
            z: rotation.z,
            w: rotation.w,
        },
        position: vec3f(translation.x, translation.y, translation.z),
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
