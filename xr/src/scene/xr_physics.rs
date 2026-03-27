#![allow(dead_code)]

use super::xr_depth::{
    depth_query_plane_supports_body, DepthQuerySurfaceCollider, DepthQuerySurfaceTarget,
};
use super::*;
use rapier3d::dynamics::CoefficientCombineRule;
use rapier3d::pipeline::{ActiveHooks, PairFilterContext, PhysicsHooks};
use rapier3d::prelude::SolverFlags;

#[derive(Clone, Copy, Debug)]
pub(super) enum HandCollider {
    Capsule { a: Vec3f, b: Vec3f, radius: f32 },
    Ball { center: Vec3f, radius: f32 },
    Box { pose: Pose, half_extents: Vec3f },
}

#[derive(Clone, Copy)]
pub(crate) struct PhysicsCube {
    pub(crate) widget_uid: WidgetUid,
    pub(crate) body: RigidBodyHandle,
    pub(crate) collider: ColliderHandle,
    pub(crate) half_extents: Vec3f,
    pub(crate) query_radius: f32,
    pub(crate) scale: Vec3f,
    pub(crate) body_kind: XrBodyKind,
    pub(crate) depth_query_surface_set: Option<usize>,
}

#[derive(Clone, Copy)]
pub(super) struct HandColliderBody {
    pub(super) body: RigidBodyHandle,
    pub(super) collider: ColliderHandle,
}

struct DepthQueryBodySurfaceSet {
    body: RigidBodyHandle,
    query_radius: f32,
    surfaces: [DepthQuerySurfaceCollider; XR_DEPTH_QUERY_SURFACES_PER_BODY],
}

#[derive(Clone, Copy, Default)]
pub(crate) struct DepthQueryPhysicsStats {
    pub(crate) active_surface_count: usize,
    pub(crate) vertex_count: usize,
    pub(crate) triangle_count: usize,
}

struct RapierDepthQueryHooks;

const DEPTH_QUERY_BODY_USER_DATA_TAG: u128 = 1u128 << 127;
const DEPTH_QUERY_SURFACE_USER_DATA_TAG: u128 = 1u128 << 126;
const DEPTH_QUERY_USER_DATA_OWNER_MASK: u128 = u64::MAX as u128;
static RAPIER_DEPTH_QUERY_HOOKS: RapierDepthQueryHooks = RapierDepthQueryHooks;
const XR_ENABLE_SYNTHETIC_GROUND_PLANE: bool = false;

pub(crate) struct RapierScene {
    gravity: RapierVector,
    integration_parameters: IntegrationParameters,
    pipeline: PhysicsPipeline,
    islands: IslandManager,
    broad_phase: BroadPhaseBvh,
    narrow_phase: NarrowPhase,
    pub(crate) bodies: RigidBodySet,
    pub(super) colliders: ColliderSet,
    impulse_joints: ImpulseJointSet,
    multibody_joints: MultibodyJointSet,
    ccd_solver: CCDSolver,
    pub(crate) cubes: Vec<PhysicsCube>,
    projectile_cube_indices: Vec<usize>,
    projectile_cube_cursor: usize,
    depth_query_surface_sets: Vec<DepthQueryBodySurfaceSet>,
    depth_query_stats: DepthQueryPhysicsStats,
    pub(super) left_hand: Vec<HandColliderBody>,
    pub(super) right_hand: Vec<HandColliderBody>,
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

fn depth_query_body_user_data(widget_uid: WidgetUid) -> u128 {
    DEPTH_QUERY_BODY_USER_DATA_TAG | widget_uid.0 as u128
}

fn depth_query_surface_user_data(widget_uid: WidgetUid) -> u128 {
    DEPTH_QUERY_SURFACE_USER_DATA_TAG | widget_uid.0 as u128
}

fn decode_depth_query_body_owner(user_data: u128) -> Option<u64> {
    ((user_data & DEPTH_QUERY_BODY_USER_DATA_TAG) != 0)
        .then_some((user_data & DEPTH_QUERY_USER_DATA_OWNER_MASK) as u64)
}

fn decode_depth_query_surface_owner(user_data: u128) -> Option<u64> {
    ((user_data & DEPTH_QUERY_SURFACE_USER_DATA_TAG) != 0)
        .then_some((user_data & DEPTH_QUERY_USER_DATA_OWNER_MASK) as u64)
}

impl PhysicsHooks for RapierDepthQueryHooks {
    fn filter_contact_pair(&self, context: &PairFilterContext) -> Option<SolverFlags> {
        let collider1 = context.colliders.get(context.collider1)?;
        let collider2 = context.colliders.get(context.collider2)?;
        match (
            decode_depth_query_surface_owner(collider1.user_data),
            decode_depth_query_surface_owner(collider2.user_data),
        ) {
            (Some(owner1), None) => (decode_depth_query_body_owner(collider2.user_data)
                == Some(owner1))
            .then_some(SolverFlags::COMPUTE_IMPULSES),
            (None, Some(owner2)) => (decode_depth_query_body_owner(collider1.user_data)
                == Some(owner2))
            .then_some(SolverFlags::COMPUTE_IMPULSES),
            (Some(_), Some(_)) => None,
            (None, None) => Some(SolverFlags::COMPUTE_IMPULSES),
        }
    }
}

pub(crate) fn makepad_pose(pose: &RapierPose) -> Pose {
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

pub(super) fn capsule_pose(a: Vec3f, b: Vec3f) -> (RapierPose, RapierReal) {
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
    pub(crate) fn gravity_vector(&self) -> Vec3f {
        vec3f(self.gravity.x, self.gravity.y, self.gravity.z)
    }

    pub(crate) fn set_simulation_dt(&mut self, dt: f32) {
        self.integration_parameters.dt = dt.max(0.0001);
    }

    pub(crate) fn simulation_dt(&self) -> f32 {
        self.integration_parameters.dt
    }

    pub(crate) fn spawn_dynamic_box(
        &mut self,
        widget_uid: WidgetUid,
        pose: Pose,
        half_extents: Vec3f,
        scale: Vec3f,
        density: f32,
        friction: f32,
        restitution: f32,
    ) {
        let body = self.bodies.insert(
            RigidBodyBuilder::dynamic()
                .pose(rapier_pose(pose))
                .ccd_enabled(true)
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
                .user_data(depth_query_body_user_data(widget_uid))
                .density(density.max(0.0))
                .friction(friction.max(0.0))
                .restitution(restitution.max(0.0)),
            body,
            &mut self.bodies,
        );
        let query_radius = half_extents.length().max(0.0005);
        let depth_query_surface_set = if XR_ENABLE_DEPTH_QUERY_PHYSICS {
            Some(self.spawn_depth_query_surface_set(widget_uid, body, query_radius))
        } else {
            None
        };
        self.cubes.push(PhysicsCube {
            widget_uid,
            body,
            collider,
            half_extents,
            query_radius,
            scale,
            body_kind: XrBodyKind::Dynamic,
            depth_query_surface_set,
        });
    }

    pub(crate) fn spawn_dynamic_sphere(
        &mut self,
        widget_uid: WidgetUid,
        pose: Pose,
        radius: f32,
        scale: Vec3f,
        density: f32,
        friction: f32,
        restitution: f32,
    ) {
        let body = self.bodies.insert(
            RigidBodyBuilder::dynamic()
                .pose(rapier_pose(pose))
                .ccd_enabled(true)
                .linear_damping(XR_BODY_LINEAR_DAMPING)
                .angular_damping(XR_BODY_ANGULAR_DAMPING)
                .additional_solver_iterations(XR_BODY_ADDITIONAL_SOLVER_ITERATIONS),
        );
        if let Some(rigid_body) = self.bodies.get_mut(body) {
            let activation = rigid_body.activation_mut();
            activation.angular_threshold = XR_BODY_SLEEP_ANGULAR_THRESHOLD;
            activation.time_until_sleep = XR_BODY_SLEEP_TIME;
        }
        let radius = radius.max(0.0005);
        let collider = self.colliders.insert_with_parent(
            ColliderBuilder::ball(radius)
                .user_data(depth_query_body_user_data(widget_uid))
                .density(density.max(0.0))
                .friction(friction.max(0.0))
                .restitution(restitution.max(0.0)),
            body,
            &mut self.bodies,
        );
        let depth_query_surface_set = if XR_ENABLE_DEPTH_QUERY_PHYSICS {
            Some(self.spawn_depth_query_surface_set(widget_uid, body, radius))
        } else {
            None
        };
        self.cubes.push(PhysicsCube {
            widget_uid,
            body,
            collider,
            half_extents: vec3f(radius, radius, radius),
            query_radius: radius,
            scale,
            body_kind: XrBodyKind::Dynamic,
            depth_query_surface_set,
        });
    }

    pub(crate) fn spawn_fixed_box(
        &mut self,
        widget_uid: WidgetUid,
        pose: Pose,
        half_extents: Vec3f,
        scale: Vec3f,
        friction: f32,
        restitution: f32,
    ) {
        let body = self
            .bodies
            .insert(RigidBodyBuilder::fixed().pose(rapier_pose(pose)));
        let collider = self.colliders.insert_with_parent(
            ColliderBuilder::cuboid(half_extents.x, half_extents.y, half_extents.z)
                .user_data(depth_query_body_user_data(widget_uid))
                .friction(friction.max(0.0))
                .restitution(restitution.max(0.0)),
            body,
            &mut self.bodies,
        );
        self.cubes.push(PhysicsCube {
            widget_uid,
            body,
            collider,
            half_extents,
            query_radius: half_extents.length().max(0.0005),
            scale,
            body_kind: XrBodyKind::Fixed,
            depth_query_surface_set: None,
        });
    }

    pub(crate) fn spawn_fixed_sphere(
        &mut self,
        widget_uid: WidgetUid,
        pose: Pose,
        radius: f32,
        scale: Vec3f,
        friction: f32,
        restitution: f32,
    ) {
        let body = self
            .bodies
            .insert(RigidBodyBuilder::fixed().pose(rapier_pose(pose)));
        let radius = radius.max(0.0005);
        let collider = self.colliders.insert_with_parent(
            ColliderBuilder::ball(radius)
                .user_data(depth_query_body_user_data(widget_uid))
                .friction(friction.max(0.0))
                .restitution(restitution.max(0.0)),
            body,
            &mut self.bodies,
        );
        self.cubes.push(PhysicsCube {
            widget_uid,
            body,
            collider,
            half_extents: vec3f(radius, radius, radius),
            query_radius: radius,
            scale,
            body_kind: XrBodyKind::Fixed,
            depth_query_surface_set: None,
        });
    }

    pub(crate) fn new(gravity: f32) -> Self {
        let mut scene = Self {
            gravity: RapierVector::new(0.0, -gravity, 0.0),
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
            projectile_cube_indices: Vec::new(),
            projectile_cube_cursor: 0,
            depth_query_surface_sets: Vec::new(),
            depth_query_stats: DepthQueryPhysicsStats::default(),
            left_hand: Vec::new(),
            right_hand: Vec::new(),
        };

        if XR_ENABLE_SYNTHETIC_GROUND_PLANE {
            // Prefer depth-derived floor support in XR; keep this only as an opt-in fallback.
            let floor = scene.bodies.insert(RigidBodyBuilder::fixed().build());
            scene.colliders.insert_with_parent(
                ColliderBuilder::new(SharedShape::halfspace(RapierVector::new(0.0, 1.0, 0.0)))
                    .friction(0.9),
                floor,
                &mut scene.bodies,
            );
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

    fn spawn_depth_query_surface(
        &mut self,
        owner_widget_uid: WidgetUid,
    ) -> DepthQuerySurfaceCollider {
        let body = self.bodies.insert(RigidBodyBuilder::fixed().build());
        let collider = self.colliders.insert_with_parent(
            ColliderBuilder::new(SharedShape::halfspace(RapierVector::new(0.0, 1.0, 0.0)))
                .user_data(depth_query_surface_user_data(owner_widget_uid))
                .active_hooks(ActiveHooks::FILTER_CONTACT_PAIRS)
                .friction(XR_DEPTH_QUERY_FRICTION)
                .restitution(0.0)
                .restitution_combine_rule(CoefficientCombineRule::Max),
            body,
            &mut self.bodies,
        );
        if let Some(collider) = self.colliders.get_mut(collider) {
            collider.set_enabled(false);
        }
        DepthQuerySurfaceCollider {
            collider,
            fingerprint: 0,
        }
    }

    fn spawn_depth_query_surface_set(
        &mut self,
        owner_widget_uid: WidgetUid,
        body: RigidBodyHandle,
        query_radius: f32,
    ) -> usize {
        let surfaces = std::array::from_fn(|_| self.spawn_depth_query_surface(owner_widget_uid));
        let index = self.depth_query_surface_sets.len();
        self.depth_query_surface_sets
            .push(DepthQueryBodySurfaceSet {
                body,
                query_radius,
                surfaces,
            });
        index
    }

    pub(super) fn sync_hand_bodies(
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

    pub(crate) fn step(&mut self) {
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
            &RAPIER_DEPTH_QUERY_HOOKS,
            &(),
        );
        self.settle_resting_bodies();
    }

    fn settle_resting_bodies(&mut self) {
        let linear_speed_sq = XR_BODY_SNAP_SLEEP_LINEAR_SPEED * XR_BODY_SNAP_SLEEP_LINEAR_SPEED;
        let angular_speed_sq = XR_BODY_SNAP_SLEEP_ANGULAR_SPEED * XR_BODY_SNAP_SLEEP_ANGULAR_SPEED;
        let mut to_sleep = Vec::new();

        for cube in &self.cubes {
            if !matches!(cube.body_kind, XrBodyKind::Dynamic) {
                continue;
            }
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

    pub(super) fn depth_query_key(index: usize) -> u64 {
        index as u64 + 1
    }

    pub(super) fn depth_query_surface_set_count(&self) -> usize {
        self.depth_query_surface_sets.len()
    }

    pub(super) fn begin_depth_query_stats_frame(&mut self) {
        self.depth_query_stats = DepthQueryPhysicsStats::default();
    }

    pub(crate) fn depth_query_stats(&self) -> DepthQueryPhysicsStats {
        self.depth_query_stats
    }

    pub(crate) fn register_projectile_cube(&mut self, cube_index: usize) {
        let Some(cube) = self.cubes.get(cube_index).copied() else {
            return;
        };
        self.projectile_cube_indices.push(cube_index);
        if let Some(body) = self.bodies.get_mut(cube.body) {
            body.set_enabled(false);
            body.set_linvel(RapierVector::ZERO, false);
            body.set_angvel(RapierVector::ZERO, false);
            body.reset_forces(false);
            body.reset_torques(false);
        }
    }

    pub(crate) fn respawn_body(
        &mut self,
        widget_uid: WidgetUid,
        pose: Pose,
        linvel: Vec3f,
        angvel: Vec3f,
    ) -> Option<u64> {
        let cube = self
            .cubes
            .iter()
            .find(|cube| cube.widget_uid == widget_uid)
            .copied()?;
        if let Some(body) = self.bodies.get_mut(cube.body) {
            body.set_enabled(true);
            body.set_position(rapier_pose(pose), false);
            body.set_linvel(rapier_vec3(linvel), true);
            body.set_angvel(rapier_vec3(angvel), true);
            body.reset_forces(false);
            body.reset_torques(false);
            body.wake_up(true);
        }
        cube.depth_query_surface_set
            .map(RapierScene::depth_query_key)
    }

    pub(super) fn sync_depth_query_surface_set(
        &mut self,
        set_index: usize,
        targets: &[Option<DepthQuerySurfaceTarget>; XR_DEPTH_QUERY_SURFACES_PER_BODY],
    ) {
        let Some(surface_set) = self.depth_query_surface_sets.get_mut(set_index) else {
            return;
        };
        let body_position = self
            .bodies
            .get(surface_set.body)
            .map(|body| makepad_pose(body.position()).position)
            .unwrap_or(vec3f(0.0, 0.0, 0.0));
        let body_velocity = self
            .bodies
            .get(surface_set.body)
            .map(|body| {
                let linvel = body.linvel();
                vec3f(linvel.x, linvel.y, linvel.z)
            })
            .unwrap_or(vec3f(0.0, 0.0, 0.0));
        let physics_edge_margin = (surface_set.query_radius * 0.08).clamp(0.002, 0.008);
        for (surface, target) in surface_set.surfaces.iter_mut().zip(targets.iter()) {
            let Some(target) = target else {
                if let Some(collider) = self.colliders.get_mut(surface.collider) {
                    collider.set_enabled(false);
                }
                surface.fingerprint = 0;
                continue;
            };
            if let Some(collider) = self.colliders.get_mut(surface.collider) {
                let XrDepthMeshQueryColliderGeometry::HalfSpace(plane) = target.collider.geometry;
                let footprint_supports_body = depth_query_plane_supports_body(
                    plane,
                    body_position,
                    surface_set.query_radius,
                    physics_edge_margin,
                );
                let supports_body = match target.collider.role {
                    XrDepthMeshQueryColliderRole::Support => footprint_supports_body,
                    XrDepthMeshQueryColliderRole::Impact => {
                        let speed = body_velocity.length();
                        let approach_speed = -body_velocity.dot(plane.normal);
                        footprint_supports_body
                            && speed >= XR_DEPTH_QUERY_IMPACT_ENABLE_SPEED_MIN
                            && approach_speed >= XR_DEPTH_QUERY_IMPACT_ENABLE_APPROACH_SPEED_MIN
                    }
                };
                if surface.fingerprint != target.collider.fingerprint {
                    collider.set_shape(SharedShape::halfspace(rapier_vec3(plane.normal)));
                    collider.set_position_wrt_parent(RapierPose::from_parts(
                        rapier_vec3(plane.point),
                        RapierRotation::IDENTITY,
                    ));
                    surface.fingerprint = target.collider.fingerprint;
                }
                collider.set_restitution(target.collider.restitution.max(0.0));
                collider.set_enabled(supports_body);
                if supports_body {
                    self.depth_query_stats.active_surface_count += 1;
                    self.depth_query_stats.vertex_count += target.collider.vertex_count();
                    self.depth_query_stats.triangle_count += target.collider.triangle_count();
                }
            }
        }
    }
}
