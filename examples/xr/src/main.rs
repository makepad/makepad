pub use makepad_widgets;

use ::rapier3d::prelude::{
    BroadPhaseBvh, CCDSolver, ColliderBuilder, ColliderHandle, ColliderSet, ImpulseJointSet,
    IntegrationParameters, IslandManager, MultibodyJointSet, NarrowPhase, PhysicsPipeline,
    Pose as RapierPose, Real as RapierReal, RigidBodyBuilder, RigidBodyHandle, RigidBodySet,
    Rotation as RapierRotation, SharedShape, Vector as RapierVector,
};
use makepad_widgets::makepad_platform::{
    permission::{Permission, PermissionStatus},
    XrDepthPhysicsBox, XrDepthPhysicsChunk, XrDepthPhysicsChunkKey, XrDepthVoxels,
};
use makepad_widgets::*;
use std::{
    collections::{hash_map::DefaultHasher, HashMap, HashSet},
    hash::{Hash, Hasher},
};

app_main!(App);

script_mod! {
    use mod.pod.*
    use mod.math.*
    use mod.shader.*
    use mod.draw
    use mod.geom
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.XrSceneBase = #(XrScene::register_widget(vm))
    mod.widgets.XrScene = set_type_default() do mod.widgets.XrSceneBase{
        draw_cube +: {}
        draw_depth_box +: {}
        draw_pbr +: {
            light_dir: vec3(0.35, 0.8, 0.45)
            light_color: vec3(1.0, 1.0, 1.0)
            ambient: 0.25
            spec_power: 128.0
            spec_strength: 0.9
            env_intensity: 1.8
        }
    }

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(1280, 820)
                body +: {
                    phase_view := AdaptiveView{
                        width: Fill
                        height: Fill
                        retain_unused_variants: false

                        Preflight := View{
                            width: Fill
                            height: Fill
                            flow: Down
                            align: Align{x: 0.5 y: 0.5}
                            padding: Inset{left: 36 right: 36 top: 36 bottom: 36}
                            spacing: 14
                            show_bg: true
                            draw_bg +: {
                                color_top: uniform(#x0b1422)
                                color_bottom: uniform(#x051018)
                                color_glow: uniform(#x1b4663)
                                pixel: fn() {
                                    let uv = self.pos;
                                    let base = mix(self.color_top, self.color_bottom, uv.y);
                                    let glow = smoothstep(0.72, 0.0, length(uv - vec2(0.18, 0.24)));
                                    return mix(base, self.color_glow, glow * 0.24);
                                }
                            }

                            panel := RoundedView{
                                width: 560
                                height: Fit
                                flow: Down
                                spacing: 10
                                padding: Inset{left: 22 right: 22 top: 20 bottom: 20}
                                draw_bg.color: #x09131cdd
                                draw_bg.radius: 16.0

                                title := H1{
                                    text: "XR Preflight"
                                    draw_text.color: #xeff7ff
                                }

                                detail_label := Label{
                                    width: Fill
                                    text: "Allow Quest scene access here before starting XR. The passthrough depth path uses Meta's scene permission for environment depth and occlusion."
                                    draw_text.color: #xb8c8d8
                                }

                                View{
                                    width: Fill
                                    height: Fit
                                    flow: Right
                                    spacing: 10

                                    allow_button := Button{
                                        width: Fill
                                        text: "Allow Quest Scene Access"
                                    }

                                    start_xr_button := Button{
                                        width: Fill
                                        text: "Start XR"
                                    }
                                }

                                status_label := Label{
                                    width: Fill
                                    text: "Checking startup requirements."
                                    draw_text.color: #x8fe4d6
                                }
                            }
                        }

                        XrRuntime := View{
                            width: 0
                            height: 0
                        }
                    }
                }
            }

            xr_scene := mod.widgets.XrScene{}
        }
    }
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

const PLATFORM_COLOR: [f32; 3] = [0.10, 0.14, 0.18];
const XR_CUBE_HALF_EXTENT: f32 = 0.020;
const XR_PLATFORM_HALF_WIDTH: f32 = 0.64;
const XR_PLATFORM_HALF_HEIGHT: f32 = 0.006;
const XR_PLATFORM_HALF_DEPTH: f32 = 0.16;
const XR_SCENE_FORWARD_OFFSET: f32 = 0.55;
const XR_SCENE_VERTICAL_OFFSET: f32 = 0.30;
const XR_SCENE_HEAD_HEIGHT_SCALE: f32 = 0.5;
const XR_SIMULATION_DT: f32 = 1.0 / 120.0;
const XR_ENABLE_HAND_PHYSICS: bool = true;
const XR_RENDER_HAND_GEOMETRY: bool = false;
const XR_RENDER_DEPTH_DEBUG: bool = true;
const XR_HAND_COLLIDER_SLOTS_PER_HAND: usize = 25;
const XR_HAND_COLLIDER_FRICTION: f32 = 0.8;
const XR_HAND_PLATE_HALF_WIDTH: f32 = 0.045;
const XR_HAND_PLATE_HALF_HEIGHT: f32 = 0.005;
const XR_HAND_PLATE_HALF_DEPTH: f32 = 0.028;
const XR_HAND_PLATE_FORWARD_OFFSET: f32 = 0.004;
const XR_HAND_TIP_RADIUS_SCALE: f32 = 0.72;
const XR_BODY_LINEAR_DAMPING: f32 = 1.5;
const XR_BODY_ANGULAR_DAMPING: f32 = 6.0;
const XR_BODY_ADDITIONAL_SOLVER_ITERATIONS: usize = 4;
const XR_BODY_SLEEP_ANGULAR_THRESHOLD: f32 = 2.0;
const XR_BODY_SLEEP_TIME: f32 = 0.35;
const XR_BODY_SNAP_SLEEP_LINEAR_SPEED: f32 = 0.03;
const XR_BODY_SNAP_SLEEP_ANGULAR_SPEED: f32 = 1.0;
const XR_PLATFORM_SPAWN_GAP: f32 = 0.0;
const XR_WALL_BRICK_HALF_WIDTH: f32 = XR_CUBE_HALF_EXTENT * 2.0;
const XR_WALL_BRICK_HALF_HEIGHT: f32 = XR_CUBE_HALF_EXTENT;
const XR_WALL_BRICK_HALF_DEPTH: f32 = XR_CUBE_HALF_EXTENT;
const XR_WALL_FULL_ROW_BRICKS: usize = 12;
const XR_WALL_SHORT_ROW_BRICKS: usize = 11;
const XR_WALL_ROWS: usize = 12;
const XR_WALL_ROTATION_Y: f32 = std::f32::consts::FRAC_PI_2;
const XR_PLATFORM_ROUND_RADIUS: f32 = 0.005;
const XR_PBR_FACE_SUBDIVISIONS: usize = 1;
const XR_PBR_CORNER_SEGMENTS: usize = 3;
const XR_PBR_HAND_CAPSULE_SUBDIVISIONS: usize = 8;
const XR_PBR_HAND_SPHERE_SUBDIVISIONS: usize = 8;
const XR_BRICK_VISUAL_SCALE: f32 = 0.98;
const XR_DEPTH_BOX_DEBUG_SCALE: f32 = 0.98;
const XR_DEPTH_BOX_DEBUG_ALPHA: f32 = 0.16;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum AppPhase {
    #[default]
    Preflight,
    XrRuntime,
}

#[derive(Clone, Copy, Debug)]
enum HandCollider {
    Capsule { a: Vec3f, b: Vec3f, radius: f32 },
    Ball { center: Vec3f, radius: f32 },
    Box { pose: Pose, half_extents: Vec3f },
}

#[derive(Clone, Copy)]
struct PhysicsCube {
    body: RigidBodyHandle,
    collider: ColliderHandle,
    half_extents: Vec3f,
    color_index: usize,
}

#[derive(Clone, Copy)]
struct HandColliderBody {
    body: RigidBodyHandle,
    collider: ColliderHandle,
}

#[derive(Clone, Copy)]
struct DepthChunkCollider {
    collider: ColliderHandle,
    fingerprint: u64,
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
    left_hand: Vec<HandColliderBody>,
    right_hand: Vec<HandColliderBody>,
    platform_pose: Pose,
    depth_body: RigidBodyHandle,
    depth_chunk_colliders: HashMap<XrDepthPhysicsChunkKey, DepthChunkCollider>,
    depth_physics_generation: u64,
}

fn rapier_vec3(v: Vec3f) -> RapierVector {
    RapierVector::new(v.x, v.y, v.z)
}

fn rapier_rotation(q: Quat) -> RapierRotation {
    RapierRotation::from_xyzw(q.x, q.y, q.z, q.w)
}

fn rapier_pose(pose: Pose) -> RapierPose {
    RapierPose::from_parts(
        rapier_vec3(pose.position),
        rapier_rotation(pose.orientation),
    )
}

fn makepad_pose(pose: &RapierPose) -> Pose {
    Pose::new(
        Quat {
            x: pose.rotation.x,
            y: pose.rotation.y,
            z: pose.rotation.z,
            w: pose.rotation.w,
        },
        vec3f(pose.translation.x, pose.translation.y, pose.translation.z),
    )
}

fn capsule_pose(a: Vec3f, b: Vec3f) -> (RapierPose, RapierReal) {
    let delta = b - a;
    let length = delta.length();
    let rotation = if length > 1.0e-4 {
        RapierRotation::from_rotation_arc(RapierVector::Y, rapier_vec3(delta * (1.0 / length)))
    } else {
        RapierRotation::IDENTITY
    };
    (
        RapierPose::from_parts(rapier_vec3((a + b) * 0.5), rotation),
        (length * 0.5).max(0.0005),
    )
}

impl RapierScene {
    fn physics_chunk_fingerprint(chunk: &XrDepthPhysicsChunk) -> u64 {
        let mut hasher = DefaultHasher::new();
        chunk.key.hash(&mut hasher);
        chunk.boxes.len().hash(&mut hasher);
        for physics_box in &chunk.boxes {
            physics_box.center.x.to_bits().hash(&mut hasher);
            physics_box.center.y.to_bits().hash(&mut hasher);
            physics_box.center.z.to_bits().hash(&mut hasher);
            physics_box.half_extents.x.to_bits().hash(&mut hasher);
            physics_box.half_extents.y.to_bits().hash(&mut hasher);
            physics_box.half_extents.z.to_bits().hash(&mut hasher);
        }
        hasher.finish()
    }

    fn spawn_dynamic_box(&mut self, pose: Pose, half_extents: Vec3f) {
        let body = self.bodies.insert(
            RigidBodyBuilder::dynamic()
                .pose(rapier_pose(pose))
                .linear_damping(XR_BODY_LINEAR_DAMPING)
                .angular_damping(XR_BODY_ANGULAR_DAMPING)
                .additional_solver_iterations(XR_BODY_ADDITIONAL_SOLVER_ITERATIONS),
        );
        if let Some(rigid_body) = self.bodies.get_mut(body) {
            let activation = rigid_body.activation_mut();
            activation.angular_threshold = XR_BODY_SLEEP_ANGULAR_THRESHOLD;
            activation.time_until_sleep = XR_BODY_SLEEP_TIME;
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

    fn new(center: Vec3f) -> Self {
        let scene_rotation = Quat::from_axis_angle(vec3f(0.0, 1.0, 0.0), XR_WALL_ROTATION_Y);
        let platform_center = center + vec3f(0.0, -XR_PLATFORM_HALF_HEIGHT, 0.0);
        let mut scene = Self {
            gravity: RapierVector::new(0.0, -9.81, 0.0),
            integration_parameters: IntegrationParameters {
                dt: XR_SIMULATION_DT,
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
            left_hand: Vec::new(),
            right_hand: Vec::new(),
            platform_pose: Pose::new(scene_rotation, platform_center),
            depth_body: RigidBodyHandle::invalid(),
            depth_chunk_colliders: HashMap::new(),
            depth_physics_generation: 0,
        };

        let platform = scene
            .bodies
            .insert(RigidBodyBuilder::fixed().pose(rapier_pose(scene.platform_pose)));
        scene.colliders.insert_with_parent(
            ColliderBuilder::cuboid(
                XR_PLATFORM_HALF_WIDTH,
                XR_PLATFORM_HALF_HEIGHT,
                XR_PLATFORM_HALF_DEPTH,
            )
            .friction(0.9),
            platform,
            &mut scene.bodies,
        );

        // Invisible floor at XR ground level (y=0).
        let floor = scene.bodies.insert(RigidBodyBuilder::fixed().build());
        scene.colliders.insert_with_parent(
            ColliderBuilder::new(SharedShape::halfspace(RapierVector::new(0.0, 1.0, 0.0)))
                .friction(0.9),
            floor,
            &mut scene.bodies,
        );

        scene.depth_body = scene.bodies.insert(RigidBodyBuilder::fixed().build());

        let brick_half_extents = vec3f(
            XR_WALL_BRICK_HALF_WIDTH,
            XR_WALL_BRICK_HALF_HEIGHT,
            XR_WALL_BRICK_HALF_DEPTH,
        );
        let brick_width = XR_WALL_BRICK_HALF_WIDTH * 2.0 + XR_PLATFORM_SPAWN_GAP;
        let brick_height = XR_WALL_BRICK_HALF_HEIGHT * 2.0 + XR_PLATFORM_SPAWN_GAP;
        let wall_origin = center;
        let row_axis = vec3f(0.0, 0.0, 1.0);
        let brick_rotation = scene_rotation;
        for row in 0..XR_WALL_ROWS {
            let bricks_in_row = if row % 2 == 0 {
                XR_WALL_FULL_ROW_BRICKS
            } else {
                XR_WALL_SHORT_ROW_BRICKS
            };
            let row_center_offset = (bricks_in_row as f32 - 1.0) * 0.5;
            for brick in 0..bricks_in_row {
                let brick_center = wall_origin
                    + row_axis * ((brick as f32 - row_center_offset) * brick_width)
                    + vec3f(
                        0.0,
                        XR_WALL_BRICK_HALF_HEIGHT
                            + XR_PLATFORM_SPAWN_GAP
                            + row as f32 * brick_height,
                        0.0,
                    );
                scene
                    .spawn_dynamic_box(Pose::new(brick_rotation, brick_center), brick_half_extents);
            }
        }

        if XR_ENABLE_HAND_PHYSICS {
            scene.left_hand = scene.spawn_hand_colliders(XR_HAND_COLLIDER_SLOTS_PER_HAND);
            scene.right_hand = scene.spawn_hand_colliders(XR_HAND_COLLIDER_SLOTS_PER_HAND);
        }
        scene.step();
        scene
    }

    fn spawn_hand_colliders(&mut self, count: usize) -> Vec<HandColliderBody> {
        let mut result = Vec::with_capacity(count);
        for _ in 0..count {
            let body = self
                .bodies
                .insert(RigidBodyBuilder::kinematic_position_based().pose(RapierPose::IDENTITY));
            let collider = self.colliders.insert_with_parent(
                ColliderBuilder::capsule_y(0.01, 0.01)
                    .friction(XR_HAND_COLLIDER_FRICTION)
                    .restitution(0.0),
                body,
                &mut self.bodies,
            );
            if let Some(collider) = self.colliders.get_mut(collider) {
                collider.set_enabled(false);
            }
            result.push(HandColliderBody { body, collider });
        }
        result
    }

    fn sync_hand_bodies(
        bodies: &[HandColliderBody],
        colliders: &[HandCollider],
        rigid_bodies: &mut RigidBodySet,
        collider_set: &mut ColliderSet,
    ) {
        for (index, slot) in bodies.iter().enumerate() {
            let active = index < colliders.len();
            if active {
                let (target_pose, shape) = match colliders[index] {
                    HandCollider::Capsule { a, b, radius } => {
                        let (target_pose, half_height) = capsule_pose(a, b);
                        (target_pose, SharedShape::capsule_y(half_height, radius))
                    }
                    HandCollider::Ball { center, radius } => (
                        RapierPose::from_parts(rapier_vec3(center), RapierRotation::IDENTITY),
                        SharedShape::ball(radius),
                    ),
                    HandCollider::Box { pose, half_extents } => (
                        rapier_pose(pose),
                        SharedShape::cuboid(half_extents.x, half_extents.y, half_extents.z),
                    ),
                };
                let was_enabled = collider_set
                    .get(slot.collider)
                    .map(|collider| collider.is_enabled())
                    .unwrap_or(false);
                if let Some(collider) = collider_set.get_mut(slot.collider) {
                    collider.set_shape(shape);
                    collider.set_enabled(true);
                }
                if let Some(body) = rigid_bodies.get_mut(slot.body) {
                    if !was_enabled {
                        // Reset the body pose on reacquire so tracking loss doesn't inject a huge velocity spike.
                        body.set_position(target_pose, false);
                    }
                    body.set_next_kinematic_position(target_pose);
                }
            } else if let Some(collider) = collider_set.get_mut(slot.collider) {
                collider.set_enabled(false);
            }
        }
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
        let linear_speed_sq = XR_BODY_SNAP_SLEEP_LINEAR_SPEED * XR_BODY_SNAP_SLEEP_LINEAR_SPEED;
        let angular_speed_sq = XR_BODY_SNAP_SLEEP_ANGULAR_SPEED * XR_BODY_SNAP_SLEEP_ANGULAR_SPEED;
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

    fn sync_depth_physics_chunks(
        &mut self,
        generation: u64,
        chunks: &[XrDepthPhysicsChunk],
    ) -> bool {
        if self.depth_physics_generation == generation {
            return false;
        }

        self.depth_physics_generation = generation;
        let live_keys: HashSet<_> = chunks.iter().map(|chunk| chunk.key).collect();
        let stale_keys: Vec<_> = self
            .depth_chunk_colliders
            .keys()
            .copied()
            .filter(|key| !live_keys.contains(key))
            .collect();

        let mut changed = false;
        for key in stale_keys {
            if let Some(chunk) = self.depth_chunk_colliders.remove(&key) {
                self.colliders
                    .remove(chunk.collider, &mut self.islands, &mut self.bodies, false);
                changed = true;
            }
        }

        for physics_chunk in chunks {
            let fingerprint = Self::physics_chunk_fingerprint(physics_chunk);
            if let Some(existing) = self.depth_chunk_colliders.get_mut(&physics_chunk.key) {
                if existing.fingerprint != fingerprint {
                    if let Some(collider) = self.colliders.get_mut(existing.collider) {
                        collider.set_shape(physics_chunk.shape.clone());
                        collider.set_enabled(true);
                    }
                    existing.fingerprint = fingerprint;
                    changed = true;
                }
                continue;
            }

            let collider = self.colliders.insert_with_parent(
                ColliderBuilder::new(physics_chunk.shape.clone())
                    .friction(0.9)
                    .restitution(0.0),
                self.depth_body,
                &mut self.bodies,
            );
            self.depth_chunk_colliders.insert(
                physics_chunk.key,
                DepthChunkCollider {
                    collider,
                    fingerprint,
                },
            );
            changed = true;
        }

        if changed {
            for cube in &self.cubes {
                if let Some(body) = self.bodies.get_mut(cube.body) {
                    body.wake_up(true);
                }
            }
        }
        changed
    }
}

#[derive(Script, ScriptHook, Widget)]
pub struct XrScene {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[redraw]
    #[live]
    draw_cube: DrawCube,
    #[redraw]
    #[live]
    draw_pbr: DrawPbr,
    #[redraw]
    #[live]
    draw_depth_box: DrawCube,
    #[rust]
    scene: Option<RapierScene>,
    #[rust]
    depth_box_generation: u64,
    #[rust]
    depth_debug_boxes: Vec<XrDepthPhysicsBox>,
    #[rust]
    depth_debug_hidden: bool,
}

impl XrScene {
    fn depth_debug_enabled(&self) -> bool {
        XR_RENDER_DEPTH_DEBUG && !self.depth_debug_hidden
    }

    fn draw_pose_box(
        &mut self,
        cx: &mut Cx2d,
        pose: Pose,
        size: Vec3f,
        color: Vec4f,
        depth_clip: f32,
    ) {
        self.draw_cube.transform = pose.to_mat4();
        self.draw_cube.cube_pos = vec3(0.0, 0.0, 0.0);
        self.draw_cube.cube_size = size;
        self.draw_cube.color = color;
        self.draw_cube.depth_clip = depth_clip;
        self.draw_cube.draw(cx);
    }

    fn prepare_pbr(&mut self, cx: &mut Cx2d) {
        self.draw_pbr.begin();
        self.draw_pbr.set_use_pass_camera(true);
        self.draw_pbr.set_depth_clip(1.0);
        self.draw_pbr.set_base_color_texture(None);
        self.draw_pbr.set_metal_roughness_texture(None);
        self.draw_pbr.set_normal_texture(None);
        self.draw_pbr.set_occlusion_texture(None);
        self.draw_pbr.set_emissive_texture(None);
        let env_tex = self.draw_pbr.default_env_texture(cx);
        self.draw_pbr.set_env_texture(Some(env_tex));
    }

    fn prepare_depth_boxes(&mut self) {
        self.draw_depth_box.draw_vars.options.depth_write = false;
        self.draw_depth_box.depth_clip = 0.0;
    }

    fn draw_pbr_rounded_cube(
        &mut self,
        cx: &mut Cx2d,
        pose: Pose,
        half_extents: Vec3f,
        radius: f32,
        color: Vec4f,
        roughness: f32,
    ) {
        self.draw_pbr.set_transform(pose.to_mat4());
        self.draw_pbr.set_base_color_factor(color);
        self.draw_pbr.set_metal_roughness(0.0, roughness);
        let _ = self.draw_pbr.draw_rounded_cube(
            cx,
            half_extents,
            radius,
            XR_PBR_FACE_SUBDIVISIONS,
            XR_PBR_CORNER_SEGMENTS,
        );
    }

    fn draw_pbr_capsule(
        &mut self,
        cx: &mut Cx2d,
        pose: Pose,
        radius: f32,
        half_height: f32,
        color: Vec4f,
        roughness: f32,
    ) {
        self.draw_pbr.set_transform(pose.to_mat4());
        self.draw_pbr.set_base_color_factor(color);
        self.draw_pbr.set_metal_roughness(0.0, roughness);
        let _ =
            self.draw_pbr
                .draw_capsule(cx, radius, half_height, XR_PBR_HAND_CAPSULE_SUBDIVISIONS);
    }

    fn draw_pbr_sphere(
        &mut self,
        cx: &mut Cx2d,
        center: Vec3f,
        radius: f32,
        color: Vec4f,
        roughness: f32,
    ) {
        self.draw_pbr
            .set_transform(Pose::new(Quat::default(), center).to_mat4());
        self.draw_pbr.set_base_color_factor(color);
        self.draw_pbr.set_metal_roughness(0.0, roughness);
        let _ = self
            .draw_pbr
            .draw_sphere(cx, radius, XR_PBR_HAND_SPHERE_SUBDIVISIONS);
    }

    fn pose_point_world(pose: Pose, local: Vec3f) -> Vec3f {
        pose.to_mat4().transform_vec4(local.to_vec4()).to_vec3f()
    }

    fn append_capsule_collider(colliders: &mut Vec<HandCollider>, a: Vec3f, b: Vec3f, radius: f32) {
        colliders.push(HandCollider::Capsule { a, b, radius });
    }

    fn append_ball_collider(colliders: &mut Vec<HandCollider>, center: Vec3f, radius: f32) {
        colliders.push(HandCollider::Ball { center, radius });
    }

    fn append_box_collider(colliders: &mut Vec<HandCollider>, pose: Pose, half_extents: Vec3f) {
        colliders.push(HandCollider::Box { pose, half_extents });
    }

    fn hand_plate_pose(hand: &XrHand) -> Pose {
        let palm_pose = hand.joints[XrHand::CENTER];
        Pose::new(
            palm_pose.orientation,
            Self::pose_point_world(palm_pose, vec3f(0.0, 0.0, XR_HAND_PLATE_FORWARD_OFFSET)),
        )
    }

    fn hand_tip_world(hand: &XrHand, finger_index: usize) -> Vec3f {
        let tip_len = hand.tips[finger_index].max(0.0);
        hand.joints[XrHand::END_KNUCKLES[finger_index]]
            .to_mat4()
            .transform_vec4(vec4(0.0, 0.0, -tip_len, 1.0))
            .to_vec3f()
    }

    fn append_finger_chain_colliders(
        colliders: &mut Vec<HandCollider>,
        hand: &XrHand,
        chain: &[usize],
        tip_index: usize,
        radius: f32,
    ) {
        for segment in chain.windows(2) {
            Self::append_capsule_collider(
                colliders,
                hand.joints[segment[0]].position,
                hand.joints[segment[1]].position,
                radius,
            );
        }
        if hand.tip_active(tip_index) {
            let end_joint = *chain.last().unwrap_or(&XrHand::CENTER);
            Self::append_capsule_collider(
                colliders,
                hand.joints[end_joint].position,
                Self::hand_tip_world(hand, tip_index),
                radius * 0.85,
            );
        }
    }

    fn append_fingertip_collider(
        colliders: &mut Vec<HandCollider>,
        hand: &XrHand,
        tip_index: usize,
        radius: f32,
    ) {
        if hand.tip_active(tip_index) {
            Self::append_ball_collider(colliders, Self::hand_tip_world(hand, tip_index), radius);
        }
    }

    fn build_hand_colliders(hand: &XrHand) -> Vec<HandCollider> {
        let mut colliders = Vec::with_capacity(XR_HAND_COLLIDER_SLOTS_PER_HAND);
        if !hand.in_view() {
            return colliders;
        }

        Self::append_box_collider(
            &mut colliders,
            Self::hand_plate_pose(hand),
            vec3f(
                XR_HAND_PLATE_HALF_WIDTH,
                XR_HAND_PLATE_HALF_HEIGHT,
                XR_HAND_PLATE_HALF_DEPTH,
            ),
        );

        Self::append_finger_chain_colliders(
            &mut colliders,
            hand,
            &[
                XrHand::THUMB_BASE,
                XrHand::THUMB_KNUCKLE1,
                XrHand::THUMB_KNUCKLE2,
            ],
            XrHand::THUMB_TIP,
            0.015,
        );
        Self::append_fingertip_collider(
            &mut colliders,
            hand,
            XrHand::THUMB_TIP,
            0.015 * XR_HAND_TIP_RADIUS_SCALE,
        );
        Self::append_finger_chain_colliders(
            &mut colliders,
            hand,
            &[
                XrHand::INDEX_BASE,
                XrHand::INDEX_KNUCKLE1,
                XrHand::INDEX_KNUCKLE2,
                XrHand::INDEX_KNUCKLE3,
            ],
            XrHand::INDEX_TIP,
            0.014,
        );
        Self::append_fingertip_collider(
            &mut colliders,
            hand,
            XrHand::INDEX_TIP,
            0.014 * XR_HAND_TIP_RADIUS_SCALE,
        );
        Self::append_finger_chain_colliders(
            &mut colliders,
            hand,
            &[
                XrHand::MIDDLE_BASE,
                XrHand::MIDDLE_KNUCKLE1,
                XrHand::MIDDLE_KNUCKLE2,
                XrHand::MIDDLE_KNUCKLE3,
            ],
            XrHand::MIDDLE_TIP,
            0.015,
        );
        Self::append_fingertip_collider(
            &mut colliders,
            hand,
            XrHand::MIDDLE_TIP,
            0.015 * XR_HAND_TIP_RADIUS_SCALE,
        );
        Self::append_finger_chain_colliders(
            &mut colliders,
            hand,
            &[
                XrHand::RING_BASE,
                XrHand::RING_KNUCKLE1,
                XrHand::RING_KNUCKLE2,
                XrHand::RING_KNUCKLE3,
            ],
            XrHand::RING_TIP,
            0.014,
        );
        Self::append_fingertip_collider(
            &mut colliders,
            hand,
            XrHand::RING_TIP,
            0.014 * XR_HAND_TIP_RADIUS_SCALE,
        );
        Self::append_finger_chain_colliders(
            &mut colliders,
            hand,
            &[
                XrHand::LITTLE_BASE,
                XrHand::LITTLE_KNUCKLE1,
                XrHand::LITTLE_KNUCKLE2,
                XrHand::LITTLE_KNUCKLE3,
            ],
            XrHand::LITTLE_TIP,
            0.013,
        );
        Self::append_fingertip_collider(
            &mut colliders,
            hand,
            XrHand::LITTLE_TIP,
            0.013 * XR_HAND_TIP_RADIUS_SCALE,
        );

        colliders
    }

    fn collect_live_hand_colliders(
        scene: &RapierScene,
        slots: &[HandColliderBody],
    ) -> Vec<HandCollider> {
        let mut colliders = Vec::with_capacity(slots.len());
        for slot in slots {
            let Some(collider) = scene.colliders.get(slot.collider) else {
                continue;
            };
            if !collider.is_enabled() {
                continue;
            }

            let pose = makepad_pose(collider.position());
            let shape = collider.shape();
            if let Some(capsule) = shape.as_capsule() {
                colliders.push(HandCollider::Capsule {
                    a: Self::pose_point_world(
                        pose,
                        vec3f(
                            capsule.segment.a.x,
                            capsule.segment.a.y,
                            capsule.segment.a.z,
                        ),
                    ),
                    b: Self::pose_point_world(
                        pose,
                        vec3f(
                            capsule.segment.b.x,
                            capsule.segment.b.y,
                            capsule.segment.b.z,
                        ),
                    ),
                    radius: capsule.radius,
                });
            } else if let Some(ball) = shape.as_ball() {
                colliders.push(HandCollider::Ball {
                    center: pose.position,
                    radius: ball.radius,
                });
            } else if let Some(cuboid) = shape.as_cuboid() {
                colliders.push(HandCollider::Box {
                    pose,
                    half_extents: vec3f(
                        cuboid.half_extents.x,
                        cuboid.half_extents.y,
                        cuboid.half_extents.z,
                    ),
                });
            }
        }
        colliders
    }

    fn draw_hand_shapes(&mut self, cx: &mut Cx2d, colliders: &[HandCollider], is_left: bool) {
        let color = if is_left {
            vec4(0.18, 0.72, 1.0, 1.0)
        } else {
            vec4(1.0, 0.62, 0.20, 1.0)
        };
        for collider in colliders {
            match collider {
                HandCollider::Capsule { a, b, radius } => {
                    let (pose, half_height) = capsule_pose(*a, *b);
                    self.draw_pbr_capsule(
                        cx,
                        makepad_pose(&pose),
                        *radius,
                        half_height,
                        color,
                        0.58,
                    );
                }
                HandCollider::Ball { center, radius } => {
                    self.draw_pbr_sphere(cx, *center, *radius, color, 0.56);
                }
                HandCollider::Box { pose, half_extents } => {
                    self.draw_pbr_rounded_cube(cx, *pose, *half_extents, 0.0, color, 0.60);
                }
            }
        }
    }

    fn draw_hand(
        &mut self,
        cx: &mut Cx2d,
        hand: &XrHand,
        physics_colliders: Option<&[HandCollider]>,
        is_left: bool,
    ) {
        if !XR_RENDER_HAND_GEOMETRY || !hand.in_view() {
            return;
        }

        let joint_color = if is_left {
            vec4(0.22, 0.78, 1.0, 1.0)
        } else {
            vec4(1.0, 0.68, 0.30, 1.0)
        };
        let raw_colliders;
        let colliders = if let Some(physics_colliders) = physics_colliders {
            physics_colliders
        } else {
            raw_colliders = Self::build_hand_colliders(hand);
            &raw_colliders
        };
        self.draw_hand_shapes(cx, colliders, is_left);

        self.draw_cube.begin_many_instances(cx);
        for joint in &hand.joints {
            self.draw_pose_box(cx, *joint, vec3(0.011, 0.011, 0.016), joint_color, 0.0);
        }
        self.draw_cube.end_many_instances(cx);
    }

    fn ensure_scene(&mut self, state: &XrState) -> bool {
        if self.scene.is_some() {
            return false;
        }

        let mut forward = state.vec_in_head_space(vec3(0.0, 0.0, -1.0)) - state.head_pose.position;
        forward.y = 0.0;
        if forward.length() <= 1.0e-4 {
            forward = vec3f(0.0, 0.0, -1.0);
        } else {
            forward = forward.normalize();
        }
        let center = vec3f(
            state.head_pose.position.x,
            state.head_pose.position.y * XR_SCENE_HEAD_HEIGHT_SCALE,
            state.head_pose.position.z,
        ) + forward * XR_SCENE_FORWARD_OFFSET
            + vec3f(0.0, XR_SCENE_VERTICAL_OFFSET, 0.0);

        log!(
            "XR physics wall spawned at ({:.2}, {:.2}, {:.2})",
            center.x,
            center.y,
            center.z
        );
        self.scene = Some(RapierScene::new(center));
        true
    }

    fn reset_scene(&mut self, state: &XrState) {
        self.scene = None;
        self.ensure_scene(state);
    }

    fn sync_hands(&mut self, state: &XrState) {
        if !XR_ENABLE_HAND_PHYSICS {
            return;
        }

        let Some(scene) = self.scene.as_mut() else {
            return;
        };

        let left = Self::build_hand_colliders(&state.left_hand);
        let right = Self::build_hand_colliders(&state.right_hand);
        let RapierScene {
            bodies,
            colliders,
            left_hand,
            right_hand,
            ..
        } = scene;
        RapierScene::sync_hand_bodies(left_hand, &left, bodies, colliders);
        RapierScene::sync_hand_bodies(right_hand, &right, bodies, colliders);
    }

    fn sync_depth_physics(&mut self, cx: &mut Cx) {
        let Some(scene) = self.scene.as_mut() else {
            return;
        };
        let Some(volume) = cx.xr_depth_voxels().latest_voxels() else {
            let _ = scene.sync_depth_physics_chunks(0, &[]);
            return;
        };
        let Ok(volume) = volume.read() else {
            return;
        };
        let _ = scene.sync_depth_physics_chunks(volume.physics_generation, &volume.physics_chunks);
    }

    fn draw_platform(&mut self, cx: &mut Cx2d) {
        let Some(scene) = self.scene.as_ref() else {
            return;
        };

        self.draw_pbr_rounded_cube(
            cx,
            scene.platform_pose,
            vec3f(
                XR_PLATFORM_HALF_WIDTH,
                XR_PLATFORM_HALF_HEIGHT,
                XR_PLATFORM_HALF_DEPTH,
            ),
            XR_PLATFORM_ROUND_RADIUS,
            vec4(PLATFORM_COLOR[0], PLATFORM_COLOR[1], PLATFORM_COLOR[2], 1.0),
            0.85,
        );
    }

    fn draw_bodies(&mut self, cx: &mut Cx2d) {
        let Some(scene) = self.scene.as_ref() else {
            return;
        };

        let cubes: Vec<_> = scene
            .cubes
            .iter()
            .filter_map(|cube| {
                scene.bodies.get(cube.body).map(|body| {
                    let phys_pose = makepad_pose(body.position());
                    let visual_pose = Pose::new(phys_pose.orientation, phys_pose.position);
                    (visual_pose, cube.half_extents, cube.color_index)
                })
            })
            .collect();

        self.draw_cube.begin_many_instances(cx);
        for (pose, half_extents, color_index) in cubes {
            let color = CUBE_COLORS[color_index];
            self.draw_pose_box(
                cx,
                pose,
                vec3(
                    half_extents.x * 2.0 * XR_BRICK_VISUAL_SCALE,
                    half_extents.y * 2.0 * XR_BRICK_VISUAL_SCALE,
                    half_extents.z * 2.0 * XR_BRICK_VISUAL_SCALE,
                ),
                vec4(color[0], color[1], color[2], 1.0),
                1.0,
            );
        }
        self.draw_cube.end_many_instances(cx);
    }

    fn rebuild_depth_debug_boxes(&mut self, voxels: &XrDepthVoxels) {
        self.depth_debug_boxes.clear();
        let total_boxes: usize = voxels
            .physics_chunks
            .iter()
            .map(|chunk| chunk.boxes.len())
            .sum();
        self.depth_debug_boxes.reserve(total_boxes);
        for physics_chunk in &voxels.physics_chunks {
            self.depth_debug_boxes
                .extend_from_slice(&physics_chunk.boxes);
        }
    }

    fn clear_depth_debug_boxes(&mut self) {
        self.depth_box_generation = 0;
        self.depth_debug_boxes.clear();
    }

    fn sync_depth_debug_boxes(&mut self, cx: &mut Cx2d) {
        if !self.depth_debug_enabled() {
            self.clear_depth_debug_boxes();
            return;
        }
        let Some(voxels) = cx.xr_depth_voxels().latest_voxels() else {
            self.clear_depth_debug_boxes();
            return;
        };
        let Ok(voxels) = voxels.read() else {
            return;
        };
        if self.depth_box_generation == voxels.physics_generation {
            return;
        }
        self.depth_box_generation = voxels.physics_generation;
        self.rebuild_depth_debug_boxes(&voxels);
    }

    fn draw_depth_debug_boxes(&mut self, cx: &mut Cx2d) {
        if !self.depth_debug_enabled() || self.depth_debug_boxes.is_empty() {
            return;
        }

        let draw = &mut self.draw_depth_box;
        draw.transform = Mat4f::identity();
        draw.begin_many_instances(cx);
        for physics_box in &self.depth_debug_boxes {
            draw.cube_pos = vec3(
                physics_box.center.x,
                physics_box.center.y,
                physics_box.center.z,
            );
            draw.cube_size = vec3(
                physics_box.half_extents.x * 2.0 * XR_DEPTH_BOX_DEBUG_SCALE,
                physics_box.half_extents.y * 2.0 * XR_DEPTH_BOX_DEBUG_SCALE,
                physics_box.half_extents.z * 2.0 * XR_DEPTH_BOX_DEBUG_SCALE,
            );
            draw.color = vec4(0.18, 0.92, 0.98, XR_DEPTH_BOX_DEBUG_ALPHA);
            draw.draw(cx);
        }
        draw.end_many_instances(cx);
    }
}

impl Widget for XrScene {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if let Event::XrUpdate(e) = event {
            if e.menu_pressed() {
                self.depth_debug_hidden = !self.depth_debug_hidden;
                if self.depth_debug_hidden {
                    self.clear_depth_debug_boxes();
                    log!("XR depth debug hidden");
                } else {
                    log!("XR depth debug shown");
                }
            }
            let scene_reset = if e.clicked_menu() {
                self.reset_scene(&e.state);
                true
            } else {
                self.ensure_scene(&e.state)
            };
            self.sync_depth_physics(cx);
            self.sync_hands(&e.state);
            if !scene_reset {
                if let Some(scene) = &mut self.scene {
                    scene.step();
                }
            }
            if scene_reset {
                if let Some(scene) = &self.scene {
                    log!("XR wall scene reset with {} bodies", scene.bodies.len());
                }
            }
            self.redraw(cx);
        }
    }

    fn draw_walk(&mut self, _cx: &mut Cx2d, _scope: &mut Scope, _walk: Walk) -> DrawStep {
        DrawStep::done()
    }

    fn draw_3d(&mut self, cx: &mut Cx3d, _scope: &mut Scope) -> DrawStep {
        let Some(state) = cx.draw_event.xr_state.as_ref() else {
            return DrawStep::done();
        };

        let cx = &mut Cx2d::new(cx.cx);
        self.ensure_scene(state);
        self.sync_depth_debug_boxes(cx);
        let (left_physics, right_physics) = if XR_RENDER_HAND_GEOMETRY && XR_ENABLE_HAND_PHYSICS {
            if let Some(scene) = self.scene.as_ref() {
                (
                    Some(Self::collect_live_hand_colliders(scene, &scene.left_hand)),
                    Some(Self::collect_live_hand_colliders(scene, &scene.right_hand)),
                )
            } else {
                (None, None)
            }
        } else {
            (None, None)
        };
        self.prepare_pbr(cx);
        if XR_RENDER_HAND_GEOMETRY {
            self.draw_hand(cx, &state.left_hand, left_physics.as_deref(), true);
            self.draw_hand(cx, &state.right_hand, right_physics.as_deref(), false);
        }
        self.draw_platform(cx);
        self.draw_bodies(cx);
        self.prepare_depth_boxes();
        self.draw_depth_debug_boxes(cx);

        DrawStep::done()
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    phase: AppPhase,
    #[rust]
    scene_access: Option<PermissionStatus>,
    #[rust]
    pending_scene_access_check: Option<i32>,
    #[rust]
    pending_scene_access_request: Option<i32>,
    #[rust]
    ui_refresh_next_frame: Option<NextFrame>,
    #[rust]
    xr_start_next_frame: Option<NextFrame>,
}

impl App {
    fn is_android_preflight() -> bool {
        cfg!(target_os = "android")
    }

    fn scene_access_granted(&self) -> bool {
        !Self::is_android_preflight()
            || matches!(self.scene_access, Some(PermissionStatus::Granted))
    }

    fn phase_variant(&self) -> LiveId {
        match self.phase {
            AppPhase::Preflight => live_id!(Preflight),
            AppPhase::XrRuntime => live_id!(XrRuntime),
        }
    }

    fn apply_phase(&mut self, cx: &mut Cx) {
        let phase_variant = self.phase_variant();
        self.ui
            .adaptive_view(cx, ids!(phase_view))
            .set_variant_selector(move |_cx, _parent_size| phase_variant);
        cx.redraw_all();
    }

    fn schedule_ui_refresh(&mut self, cx: &mut Cx) {
        self.ui_refresh_next_frame = Some(cx.new_next_frame());
        cx.redraw_all();
    }

    fn allow_button_text(&self) -> &'static str {
        if self.pending_scene_access_check.is_some() {
            "Checking Quest Scene Access..."
        } else if self.pending_scene_access_request.is_some() {
            "Waiting for Quest Permission..."
        } else if matches!(self.scene_access, Some(PermissionStatus::Granted)) {
            "Re-check Quest Scene Access"
        } else {
            "Allow Quest Scene Access"
        }
    }

    fn detail_text(&self) -> &'static str {
        if !Self::is_android_preflight() {
            "This build can start XR directly from the splash screen."
        } else {
            match self.scene_access {
                Some(PermissionStatus::Granted) => {
                    "Quest scene access is granted. Start XR when you are ready."
                }
                Some(PermissionStatus::DeniedCanRetry) => {
                    "Quest scene access was denied. Use the allow button to ask again."
                }
                Some(PermissionStatus::DeniedPermanent) => {
                    "Quest scene access was denied again. Retry is still available here, but Android may require system settings before the dialog reappears."
                }
                Some(PermissionStatus::NotDetermined) | None => {
                    "Allow Quest scene access before starting XR. This unlocks environment depth and passthrough occlusion."
                }
            }
        }
    }

    fn status_text(&self) -> &'static str {
        if self.pending_scene_access_check.is_some() {
            "Checking current Quest permission status."
        } else if self.pending_scene_access_request.is_some() {
            "Approve the Quest permission dialog to continue."
        } else if !Self::is_android_preflight() {
            "XR is ready to launch from this splash screen."
        } else {
            match self.scene_access {
                Some(PermissionStatus::Granted) => "Quest scene access granted.",
                Some(PermissionStatus::DeniedCanRetry) => {
                    "Quest scene access denied. You can request it again."
                }
                Some(PermissionStatus::DeniedPermanent) => {
                    "Quest scene access denied. Retry may require Android settings."
                }
                Some(PermissionStatus::NotDetermined) | None => {
                    "Quest scene access has not been granted yet."
                }
            }
        }
    }

    fn refresh_preflight_ui(&mut self, cx: &mut Cx) {
        if self.phase != AppPhase::Preflight {
            return;
        }
        self.ui
            .label(cx, ids!(detail_label))
            .set_text(cx, self.detail_text());
        self.ui
            .label(cx, ids!(status_label))
            .set_text(cx, self.status_text());

        let allow_button = self.ui.button(cx, ids!(allow_button));
        allow_button.set_visible(cx, Self::is_android_preflight());
        allow_button.set_enabled(
            cx,
            Self::is_android_preflight()
                && self.pending_scene_access_check.is_none()
                && self.pending_scene_access_request.is_none(),
        );
        self.ui
            .widget(cx, ids!(allow_button))
            .set_text(cx, self.allow_button_text());

        self.ui
            .button(cx, ids!(start_xr_button))
            .set_enabled(cx, self.scene_access_granted());
    }

    fn begin_scene_access_check(&mut self, cx: &mut Cx) {
        if !Self::is_android_preflight() || self.pending_scene_access_check.is_some() {
            return;
        }
        self.pending_scene_access_check = Some(cx.check_permission(Permission::SceneAccess));
        self.schedule_ui_refresh(cx);
    }

    fn request_scene_access(&mut self, cx: &mut Cx) {
        if !Self::is_android_preflight()
            || self.pending_scene_access_check.is_some()
            || self.pending_scene_access_request.is_some()
        {
            return;
        }
        self.pending_scene_access_request = Some(cx.request_permission(Permission::SceneAccess));
        self.schedule_ui_refresh(cx);
    }

    fn begin_xr_runtime(&mut self, cx: &mut Cx) {
        if self.phase == AppPhase::XrRuntime {
            return;
        }
        self.phase = AppPhase::XrRuntime;
        self.apply_phase(cx);
        self.xr_start_next_frame = Some(cx.new_next_frame());
    }

    fn maybe_start_xr_on_ready(&mut self, cx: &mut Cx) -> bool {
        if self.phase != AppPhase::Preflight || !self.scene_access_granted() {
            return false;
        }
        self.begin_xr_runtime(cx);
        true
    }
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        self.phase = AppPhase::Preflight;
        if !Self::is_android_preflight() {
            self.scene_access = Some(PermissionStatus::Granted);
            self.maybe_start_xr_on_ready(cx);
            return;
        }
        self.apply_phase(cx);
        self.schedule_ui_refresh(cx);
        self.begin_scene_access_check(cx);
    }

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        if self.ui.button(cx, ids!(allow_button)).clicked(actions) {
            self.request_scene_access(cx);
        }

        if self.ui.button(cx, ids!(start_xr_button)).clicked(actions) && self.scene_access_granted()
        {
            self.begin_xr_runtime(cx);
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());

        match event {
            Event::NextFrame(ne) => {
                if self
                    .ui_refresh_next_frame
                    .is_some_and(|next_frame| ne.set.contains(&next_frame))
                {
                    self.ui_refresh_next_frame = None;
                    self.refresh_preflight_ui(cx);
                }

                if self
                    .xr_start_next_frame
                    .is_some_and(|next_frame| ne.set.contains(&next_frame))
                {
                    self.xr_start_next_frame = None;
                    cx.xr_start_presenting();
                }
            }
            Event::PermissionResult(result) if result.permission == Permission::SceneAccess => {
                if self.pending_scene_access_check == Some(result.request_id) {
                    self.pending_scene_access_check = None;
                } else if self.pending_scene_access_request == Some(result.request_id) {
                    self.pending_scene_access_request = None;
                } else {
                    return;
                }
                self.scene_access = Some(result.status);
                if !self.maybe_start_xr_on_ready(cx) {
                    self.schedule_ui_refresh(cx);
                }
            }
            Event::Resume => {
                if Self::is_android_preflight() && self.pending_scene_access_request.is_none() {
                    self.begin_scene_access_check(cx);
                }
            }
            _ => {}
        }
    }
}
