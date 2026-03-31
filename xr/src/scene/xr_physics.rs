#![allow(dead_code)]

use super::xr_depth::{DepthQuerySurfaceCollider, DepthQuerySurfaceTarget};
use super::*;
use crate::algorithms::tsdf_query::{
    depth_query_plane_supports_body, DepthQueryColliderGeometry, DepthQueryColliderRole,
};
use rapier3d::dynamics::CoefficientCombineRule;
use rapier3d::pipeline::{ActiveHooks, PairFilterContext, PhysicsHooks};
use rapier3d::prelude::{RigidBodyType, SolverFlags};
use std::collections::HashMap;

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

#[derive(Clone, Copy, Debug)]
struct HandGrabState {
    shared_hand: XrSharedHand,
    pose: Pose,
    previous_pose: Pose,
    linvel: Vec3f,
    tracked: bool,
    gripping: bool,
    held_body: Option<RigidBodyHandle>,
    grab_offset: Pose,
}

impl HandGrabState {
    fn new(shared_hand: XrSharedHand) -> Self {
        Self {
            shared_hand,
            pose: Pose::default(),
            previous_pose: Pose::default(),
            linvel: vec3f(0.0, 0.0, 0.0),
            tracked: false,
            gripping: false,
            held_body: None,
            grab_offset: Pose::default(),
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct ShadowBodyMotion {
    pose: Pose,
    linvel: Vec3f,
    angvel: Vec3f,
    remaining_prediction_seconds: f32,
}

struct DepthQueryBodySurfaceSet {
    owner_widget_uid: WidgetUid,
    body: RigidBodyHandle,
    query_radius: f32,
    surfaces: [DepthQuerySurfaceCollider; XR_DEPTH_QUERY_SURFACES_PER_BODY],
}

#[derive(Clone, Copy, Default)]
struct SettleContactState {
    has_active_contact: bool,
    has_depth_query_contact: bool,
    has_support_contact: bool,
}

#[derive(Clone, Copy, Default)]
pub(crate) struct DepthQueryPhysicsStats {
    pub(crate) surface_count: usize,
}

struct RapierDepthQueryHooks;

const DEPTH_QUERY_BODY_USER_DATA_TAG: u128 = 1u128 << 127;
const DEPTH_QUERY_SURFACE_USER_DATA_TAG: u128 = 1u128 << 126;
const DEPTH_QUERY_USER_DATA_OWNER_MASK: u128 = u64::MAX as u128;
static RAPIER_DEPTH_QUERY_HOOKS: RapierDepthQueryHooks = RapierDepthQueryHooks;
const XR_ENABLE_SYNTHETIC_GROUND_PLANE: bool = false;
const DEPTH_QUERY_SURFACE_IMPACT_ROLE_TAG: u128 = 1u128 << 125;
// Keep shadow extrapolation aligned with XrPeerSync so remote bodies do not drift forever
// when UDP state stalls.
const XR_SHADOW_BODY_MAX_EXTRAPOLATION_SECONDS: f32 = 0.10;

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
    spawn_pool_cube_indices: Vec<usize>,
    spawn_pool_cube_cursor: usize,
    depth_query_surface_sets: Vec<DepthQueryBodySurfaceSet>,
    depth_query_stats: DepthQueryPhysicsStats,
    pub(super) left_hand: Vec<HandColliderBody>,
    pub(super) right_hand: Vec<HandColliderBody>,
    left_hand_grab: HandGrabState,
    right_hand_grab: HandGrabState,
    shadow_body_motion: HashMap<RigidBodyHandle, ShadowBodyMotion>,
    settle_frame_counts: HashMap<RigidBodyHandle, u8>,
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

fn makepad_vec3(v: RapierVector) -> Vec3f {
    vec3f(v.x, v.y, v.z)
}

fn predict_pose(pose: Pose, linvel: Vec3f, angvel: Vec3f, dt: f32) -> Pose {
    let position = pose.position + linvel * dt;
    let angular_speed = angvel.length();
    let orientation = if angular_speed > 1.0e-4 {
        let axis = angvel * (1.0 / angular_speed);
        Quat::multiply(
            &Quat::from_axis_angle(axis, angular_speed * dt),
            &pose.orientation,
        )
    } else {
        pose.orientation
    };
    Pose::new(orientation, position)
}

fn grab_offset_from_body_anchor(
    hand_pose: Pose,
    body_pose: Pose,
    body_anchor_local: Vec3f,
) -> Pose {
    let mut grab_offset = Pose::multiply(&hand_pose.invert(), &body_pose);
    grab_offset.position = grab_offset
        .orientation
        .rotate_vec3(&(body_anchor_local * -1.0));
    grab_offset
}

fn hand_grab_pose(hand: &XrHand) -> Option<Pose> {
    hand.pinch_anchor_pose().or_else(|| hand.tracking_pose())
}

fn depth_query_body_user_data(widget_uid: WidgetUid) -> u128 {
    DEPTH_QUERY_BODY_USER_DATA_TAG | widget_uid.0 as u128
}

fn depth_query_surface_user_data(widget_uid: WidgetUid, role: DepthQueryColliderRole) -> u128 {
    DEPTH_QUERY_SURFACE_USER_DATA_TAG
        | widget_uid.0 as u128
        | match role {
            DepthQueryColliderRole::Support => 0,
            DepthQueryColliderRole::Impact => DEPTH_QUERY_SURFACE_IMPACT_ROLE_TAG,
        }
}

fn decode_depth_query_body_owner(user_data: u128) -> Option<u64> {
    ((user_data & DEPTH_QUERY_BODY_USER_DATA_TAG) != 0)
        .then_some((user_data & DEPTH_QUERY_USER_DATA_OWNER_MASK) as u64)
}

fn decode_depth_query_surface_owner(user_data: u128) -> Option<u64> {
    ((user_data & DEPTH_QUERY_SURFACE_USER_DATA_TAG) != 0)
        .then_some((user_data & DEPTH_QUERY_USER_DATA_OWNER_MASK) as u64)
}

fn decode_depth_query_surface_role(user_data: u128) -> Option<DepthQueryColliderRole> {
    ((user_data & DEPTH_QUERY_SURFACE_USER_DATA_TAG) != 0).then_some(
        if (user_data & DEPTH_QUERY_SURFACE_IMPACT_ROLE_TAG) != 0 {
            DepthQueryColliderRole::Impact
        } else {
            DepthQueryColliderRole::Support
        },
    )
}

fn quat_from_to(from: Vec3f, to: Vec3f) -> Quat {
    let from_len = from.length();
    let to_len = to.length();
    if from_len <= 1.0e-6 || to_len <= 1.0e-6 {
        return Quat::default();
    }
    let from = from * (1.0 / from_len);
    let to = to * (1.0 / to_len);
    let dot = from.dot(to).clamp(-1.0, 1.0);
    if dot >= 0.9999 {
        return Quat::default();
    }
    if dot <= -0.9999 {
        let fallback_axis = if from.x.abs() < 0.8 {
            vec3f(1.0, 0.0, 0.0)
        } else {
            vec3f(0.0, 1.0, 0.0)
        };
        let axis = Vec3f::cross(from, fallback_axis).normalize();
        return Quat::from_axis_angle(axis, std::f32::consts::PI);
    }
    let axis = Vec3f::cross(from, to).normalize();
    Quat::from_axis_angle(axis, dot.acos())
}

fn depth_query_surface_target_should_enable(
    target: DepthQuerySurfaceTarget,
    body_position: Vec3f,
    body_velocity: Vec3f,
    query_radius: f32,
    lateral_margin: f32,
) -> bool {
    let DepthQueryColliderGeometry::HalfSpace(plane) = target.collider.geometry;
    match target.collider.role {
        DepthQueryColliderRole::Support => {
            depth_query_plane_supports_body(plane, body_position, query_radius, lateral_margin)
        }
        DepthQueryColliderRole::Impact => {
            let speed = body_velocity.length();
            let approach_speed = -body_velocity.dot(plane.normal);
            // Impact planes are predictive: the TSDF query may place the quad ahead of the body
            // along its current path, so requiring current-footprint overlap here makes late wall
            // and ceiling hits tunnel through.
            speed >= XR_DEPTH_QUERY_IMPACT_ENABLE_SPEED_MIN
                && approach_speed >= XR_DEPTH_QUERY_IMPACT_ENABLE_APPROACH_SPEED_MIN
        }
    }
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
            spawn_pool_cube_indices: Vec::new(),
            spawn_pool_cube_cursor: 0,
            depth_query_surface_sets: Vec::new(),
            depth_query_stats: DepthQueryPhysicsStats::default(),
            left_hand: Vec::new(),
            right_hand: Vec::new(),
            left_hand_grab: HandGrabState::new(XrSharedHand::LeftHand),
            right_hand_grab: HandGrabState::new(XrSharedHand::RightHand),
            shadow_body_motion: HashMap::new(),
            settle_frame_counts: HashMap::new(),
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

    pub(super) fn sync_tracked_hands(&mut self, left_hand: &XrHand, right_hand: &XrHand) {
        self.left_hand_grab = self.updated_hand_grab_state(self.left_hand_grab, left_hand);
        self.right_hand_grab = self.updated_hand_grab_state(self.right_hand_grab, right_hand);
    }

    fn updated_hand_grab_state(&self, mut state: HandGrabState, hand: &XrHand) -> HandGrabState {
        let Some(pose) = hand_grab_pose(hand) else {
            state.previous_pose = state.pose;
            state.linvel = vec3f(0.0, 0.0, 0.0);
            state.tracked = false;
            state.gripping = false;
            return state;
        };
        let was_tracked = state.tracked;
        let previous_pose = if was_tracked { state.pose } else { pose };
        let dt = self.integration_parameters.dt.max(0.0001);
        state.previous_pose = previous_pose;
        state.pose = pose;
        state.linvel = if was_tracked {
            (pose.position - previous_pose.position) * (1.0 / dt)
        } else {
            vec3f(0.0, 0.0, 0.0)
        };
        state.tracked = true;
        state.gripping = hand.grab_intent();
        state
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
                .user_data(depth_query_surface_user_data(
                    owner_widget_uid,
                    DepthQueryColliderRole::Support,
                ))
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
                owner_widget_uid,
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
                    body.wake_up(true);
                }
            } else if let Some(collider) = collider_set.get_mut(slot.collider) {
                collider.set_enabled(false);
            }
        }
    }

    pub(crate) fn step(&mut self) {
        self.apply_held_body_targets();
        self.apply_shadow_body_targets();
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
        self.acquire_hand_grabs();
        self.settle_resting_bodies();
    }

    fn apply_shadow_body_targets(&mut self) {
        let dt = self
            .integration_parameters
            .dt
            .clamp(0.0, XR_SHADOW_BODY_MAX_EXTRAPOLATION_SECONDS);
        let shadow_bodies = self
            .shadow_body_motion
            .iter()
            .map(|(&body_handle, &motion)| (body_handle, motion))
            .collect::<Vec<_>>();
        let mut stale = Vec::new();
        for (body_handle, mut motion) in shadow_bodies {
            if self.held_by_for_body(body_handle).is_some() {
                continue;
            }
            let Some(body) = self.bodies.get_mut(body_handle) else {
                stale.push(body_handle);
                continue;
            };
            if !body.is_enabled() {
                stale.push(body_handle);
                continue;
            }
            if body.body_type() != RigidBodyType::KinematicPositionBased {
                stale.push(body_handle);
                continue;
            }
            let advance_dt = dt.min(motion.remaining_prediction_seconds.max(0.0));
            if advance_dt > 0.0 {
                motion.pose = predict_pose(motion.pose, motion.linvel, motion.angvel, advance_dt);
                motion.remaining_prediction_seconds =
                    (motion.remaining_prediction_seconds - advance_dt).max(0.0);
            }
            body.set_next_kinematic_position(rapier_pose(motion.pose));
            body.wake_up(true);
            self.shadow_body_motion.insert(body_handle, motion);
        }
        for body_handle in stale {
            self.shadow_body_motion.remove(&body_handle);
        }
    }

    fn apply_held_body_targets(&mut self) {
        if let (Some(left_body), Some(right_body)) = (
            self.left_hand_grab.held_body,
            self.right_hand_grab.held_body,
        ) {
            if left_body == right_body {
                match (
                    self.left_hand_grab.tracked && self.left_hand_grab.gripping,
                    self.right_hand_grab.tracked && self.right_hand_grab.gripping,
                ) {
                    (true, true) => {
                        self.apply_dual_held_body_target(self.left_hand_grab, self.right_hand_grab);
                        return;
                    }
                    (true, false) => {
                        self.right_hand_grab = Self::drop_hand_hold(self.right_hand_grab);
                        self.left_hand_grab = self.apply_held_body_target(self.left_hand_grab);
                        return;
                    }
                    (false, true) => {
                        self.left_hand_grab = Self::drop_hand_hold(self.left_hand_grab);
                        self.right_hand_grab = self.apply_held_body_target(self.right_hand_grab);
                        return;
                    }
                    (false, false) => {
                        let (left, right) =
                            self.release_dual_hand_hold(self.left_hand_grab, self.right_hand_grab);
                        self.left_hand_grab = left;
                        self.right_hand_grab = right;
                        return;
                    }
                }
            }
        }
        self.left_hand_grab = self.apply_held_body_target(self.left_hand_grab);
        self.right_hand_grab = self.apply_held_body_target(self.right_hand_grab);
    }

    fn drop_hand_hold(mut hand: HandGrabState) -> HandGrabState {
        hand.held_body = None;
        hand.grab_offset = Pose::default();
        hand
    }

    fn apply_held_body_target(&mut self, mut hand: HandGrabState) -> HandGrabState {
        let Some(body_handle) = hand.held_body else {
            return hand;
        };
        if !hand.tracked || !hand.gripping {
            return self.release_hand_hold(hand);
        }
        let Some(body) = self.bodies.get_mut(body_handle) else {
            hand.held_body = None;
            return hand;
        };
        if !body.is_enabled() {
            hand.held_body = None;
            return hand;
        }
        let target_pose = Pose::multiply(&hand.pose, &hand.grab_offset);
        if !target_pose.is_finite() {
            return self.release_hand_hold(hand);
        }
        if body.body_type() != RigidBodyType::KinematicPositionBased {
            body.set_body_type(RigidBodyType::KinematicPositionBased, true);
            body.set_position(rapier_pose(target_pose), false);
        }
        body.set_next_kinematic_position(rapier_pose(target_pose));
        body.wake_up(true);
        hand
    }

    fn apply_dual_held_body_target(&mut self, primary: HandGrabState, secondary: HandGrabState) {
        let Some(body_handle) = primary.held_body else {
            return;
        };
        let Some(body) = self.bodies.get_mut(body_handle) else {
            self.left_hand_grab = Self::drop_hand_hold(self.left_hand_grab);
            self.right_hand_grab = Self::drop_hand_hold(self.right_hand_grab);
            return;
        };
        if !body.is_enabled() {
            self.left_hand_grab = Self::drop_hand_hold(self.left_hand_grab);
            self.right_hand_grab = Self::drop_hand_hold(self.right_hand_grab);
            return;
        }
        let primary_single_target = Pose::multiply(&primary.pose, &primary.grab_offset);
        let primary_anchor_local = primary.grab_offset.invert().position;
        let secondary_anchor_local = secondary.grab_offset.invert().position;
        let local_anchor_delta = secondary_anchor_local - primary_anchor_local;
        let world_hand_delta = secondary.pose.position - primary.pose.position;
        let target_pose = if local_anchor_delta.length() >= XR_HAND_DUAL_GRAB_MIN_SPAN
            && world_hand_delta.length() >= XR_HAND_DUAL_GRAB_MIN_SPAN
        {
            let baseline_world_delta = primary_single_target
                .orientation
                .rotate_vec3(&local_anchor_delta);
            let orientation = Quat::multiply(
                &quat_from_to(baseline_world_delta, world_hand_delta),
                &primary_single_target.orientation,
            );
            let primary_world_anchor = orientation.rotate_vec3(&primary_anchor_local);
            let secondary_world_anchor = orientation.rotate_vec3(&secondary_anchor_local);
            let position = ((primary.pose.position - primary_world_anchor)
                + (secondary.pose.position - secondary_world_anchor))
                * 0.5;
            Pose::new(orientation, position)
        } else {
            primary_single_target
        };
        if !target_pose.is_finite() {
            let _ = body;
            let (left, right) =
                self.release_dual_hand_hold(self.left_hand_grab, self.right_hand_grab);
            self.left_hand_grab = left;
            self.right_hand_grab = right;
            return;
        }
        if body.body_type() != RigidBodyType::KinematicPositionBased {
            body.set_body_type(RigidBodyType::KinematicPositionBased, true);
            body.set_position(rapier_pose(target_pose), false);
        }
        body.set_next_kinematic_position(rapier_pose(target_pose));
        body.wake_up(true);
    }

    fn release_hand_hold(&mut self, mut hand: HandGrabState) -> HandGrabState {
        let Some(body_handle) = hand.held_body.take() else {
            return hand;
        };
        self.release_body_into_dynamics(
            body_handle,
            hand.linvel * XR_HAND_GRAB_RELEASE_LINEAR_VELOCITY_SCALE,
            vec3f(0.0, 0.0, 0.0),
        );
        hand
    }

    fn release_dual_hand_hold(
        &mut self,
        mut primary: HandGrabState,
        mut secondary: HandGrabState,
    ) -> (HandGrabState, HandGrabState) {
        let Some(body_handle) = primary.held_body else {
            return (
                Self::drop_hand_hold(primary),
                Self::drop_hand_hold(secondary),
            );
        };
        self.release_body_into_dynamics(
            body_handle,
            (primary.linvel + secondary.linvel)
                * (0.5 * XR_HAND_GRAB_RELEASE_LINEAR_VELOCITY_SCALE),
            vec3f(0.0, 0.0, 0.0),
        );
        primary.held_body = None;
        primary.grab_offset = Pose::default();
        secondary.held_body = None;
        secondary.grab_offset = Pose::default();
        (primary, secondary)
    }

    fn acquire_hand_grabs(&mut self) {
        let left_slots = self.left_hand.clone();
        let right_slots = self.right_hand.clone();
        let right_held = self.right_hand_grab.held_body;
        self.left_hand_grab =
            self.try_acquire_hand_hold(self.left_hand_grab, &left_slots, right_held);
        let left_held = self.left_hand_grab.held_body;
        self.right_hand_grab =
            self.try_acquire_hand_hold(self.right_hand_grab, &right_slots, left_held);
    }

    fn try_acquire_hand_hold(
        &mut self,
        mut hand: HandGrabState,
        slots: &[HandColliderBody],
        other_held_body: Option<RigidBodyHandle>,
    ) -> HandGrabState {
        if !hand.tracked || !hand.gripping || hand.held_body.is_some() {
            return hand;
        }

        let mut best_candidate = None;
        for cube in &self.cubes {
            if cube.body_kind != XrBodyKind::Dynamic {
                continue;
            }
            let shared_candidate = other_held_body == Some(cube.body);
            if self.shadow_body_motion.contains_key(&cube.body) {
                continue;
            }
            let Some(body) = self.bodies.get(cube.body) else {
                continue;
            };
            if !body.is_enabled() {
                continue;
            }
            let body_type = body.body_type();
            if !(body_type == RigidBodyType::Dynamic
                || (shared_candidate && body_type == RigidBodyType::KinematicPositionBased))
            {
                continue;
            }
            let Some((distance, body_pose, body_anchor_local)) =
                self.project_hand_anchor_to_body(cube, hand.pose)
            else {
                continue;
            };
            if distance > XR_HAND_GRAB_MAX_DISTANCE {
                continue;
            }
            if !shared_candidate && !self.cube_has_hand_contact(cube.collider, slots) {
                continue;
            }
            if best_candidate
                .map(|(best_distance, _, _, _)| distance < best_distance)
                .unwrap_or(true)
            {
                best_candidate = Some((distance, cube.body, body_pose, body_anchor_local));
            }
        }

        let Some((_, body_handle, body_pose, body_anchor_local)) = best_candidate else {
            return hand;
        };
        hand.grab_offset = grab_offset_from_body_anchor(hand.pose, body_pose, body_anchor_local);
        hand.held_body = Some(body_handle);
        self.release_settle_state(body_handle);
        let target_pose = Pose::multiply(&hand.pose, &hand.grab_offset);
        if !target_pose.is_finite() {
            hand.held_body = None;
            hand.grab_offset = Pose::default();
            return hand;
        }
        if other_held_body != Some(body_handle) {
            if let Some(body) = self.bodies.get_mut(body_handle) {
                body.set_body_type(RigidBodyType::KinematicPositionBased, true);
                body.set_position(rapier_pose(target_pose), false);
                body.set_next_kinematic_position(rapier_pose(target_pose));
                body.wake_up(true);
            }
        } else if let Some(body) = self.bodies.get_mut(body_handle) {
            body.wake_up(true);
        }
        hand
    }

    fn project_hand_anchor_to_body(
        &self,
        cube: &PhysicsCube,
        hand_pose: Pose,
    ) -> Option<(f32, Pose, Vec3f)> {
        let body = self.bodies.get(cube.body)?;
        let collider = self.colliders.get(cube.collider)?;
        let body_pose = makepad_pose(body.position());
        let surface_projection = collider.shape().project_point(
            collider.position(),
            rapier_vec3(hand_pose.position),
            false,
        );
        let world_anchor = makepad_vec3(surface_projection.point);
        let body_anchor_local = body_pose.invert().transform_vec3(&world_anchor);
        let distance = (world_anchor - hand_pose.position).length();
        (distance.is_finite() && body_anchor_local.is_finite()).then_some((
            distance,
            body_pose,
            body_anchor_local,
        ))
    }

    fn cube_has_hand_contact(
        &self,
        cube_collider: ColliderHandle,
        slots: &[HandColliderBody],
    ) -> bool {
        self.narrow_phase
            .contact_pairs_with(cube_collider)
            .any(|pair| {
                pair.has_any_active_contact()
                    && slots.iter().any(|slot| {
                        self.colliders
                            .get(slot.collider)
                            .map(|collider| collider.is_enabled())
                            .unwrap_or(false)
                            && (pair.collider1 == slot.collider || pair.collider2 == slot.collider)
                    })
            })
    }

    fn cube_settle_contact_state(&self, cube_collider: ColliderHandle) -> SettleContactState {
        let mut state = SettleContactState::default();
        for pair in self.narrow_phase.contact_pairs_with(cube_collider) {
            if !pair.has_any_active_contact() {
                continue;
            }
            state.has_active_contact = true;
            let other = if pair.collider1 == cube_collider {
                pair.collider2
            } else {
                pair.collider1
            };
            let Some(other_collider) = self.colliders.get(other) else {
                continue;
            };
            if let Some(role) = decode_depth_query_surface_role(other_collider.user_data) {
                state.has_depth_query_contact = true;
                if role == DepthQueryColliderRole::Support {
                    state.has_support_contact = true;
                }
            }
        }
        state
    }

    fn release_settle_state(&mut self, body_handle: RigidBodyHandle) {
        self.settle_frame_counts.remove(&body_handle);
    }

    fn release_body_into_dynamics(
        &mut self,
        body_handle: RigidBodyHandle,
        linvel: Vec3f,
        angvel: Vec3f,
    ) {
        self.shadow_body_motion.remove(&body_handle);
        self.release_settle_state(body_handle);
        let Some(body) = self.bodies.get_mut(body_handle) else {
            return;
        };
        if body.body_type() != RigidBodyType::Dynamic {
            body.set_body_type(RigidBodyType::Dynamic, true);
        }
        body.set_linvel(rapier_vec3(linvel), true);
        body.set_angvel(rapier_vec3(angvel), true);
        body.wake_up(true);
    }

    fn clear_held_body(&mut self, body_handle: RigidBodyHandle) {
        self.release_settle_state(body_handle);
        if self.left_hand_grab.held_body == Some(body_handle) {
            self.left_hand_grab = self.release_hand_hold(self.left_hand_grab);
        }
        if self.right_hand_grab.held_body == Some(body_handle) {
            self.right_hand_grab = self.release_hand_hold(self.right_hand_grab);
        }
    }

    pub(crate) fn held_by_for_body(&self, body_handle: RigidBodyHandle) -> Option<XrSharedHand> {
        if self.left_hand_grab.held_body == Some(body_handle) {
            Some(self.left_hand_grab.shared_hand)
        } else if self.right_hand_grab.held_body == Some(body_handle) {
            Some(self.right_hand_grab.shared_hand)
        } else {
            None
        }
    }

    pub(crate) fn shadow_body_motion_for_body(
        &self,
        body_handle: RigidBodyHandle,
    ) -> Option<(Vec3f, Vec3f)> {
        let motion = self.shadow_body_motion.get(&body_handle)?;
        if motion.remaining_prediction_seconds > 0.0 {
            Some((motion.linvel, motion.angvel))
        } else {
            Some((vec3f(0.0, 0.0, 0.0), vec3f(0.0, 0.0, 0.0)))
        }
    }

    pub(crate) fn is_shadow_body(&self, body_handle: RigidBodyHandle) -> bool {
        self.shadow_body_motion.contains_key(&body_handle)
    }

    fn settle_resting_bodies(&mut self) {
        let linear_speed_sq = XR_BODY_SNAP_SLEEP_LINEAR_SPEED * XR_BODY_SNAP_SLEEP_LINEAR_SPEED;
        let angular_speed_sq = XR_BODY_SNAP_SLEEP_ANGULAR_SPEED * XR_BODY_SNAP_SLEEP_ANGULAR_SPEED;
        let mut to_sleep = Vec::new();
        let mut to_reset = Vec::new();

        for cube in &self.cubes {
            if !matches!(cube.body_kind, XrBodyKind::Dynamic) {
                continue;
            }
            if self.left_hand_grab.held_body == Some(cube.body)
                || self.right_hand_grab.held_body == Some(cube.body)
            {
                to_reset.push(cube.body);
                continue;
            }
            let contact_state = self.cube_settle_contact_state(cube.collider);
            if !contact_state.has_active_contact {
                to_reset.push(cube.body);
                continue;
            }
            let has_hand_contact = self.cube_has_hand_contact(cube.collider, &self.left_hand)
                || self.cube_has_hand_contact(cube.collider, &self.right_hand);

            let Some(body) = self.bodies.get(cube.body) else {
                to_reset.push(cube.body);
                continue;
            };
            let body_type = body.body_type();
            let body_is_sleeping = body.is_sleeping();
            let linvel = body.linvel();
            let angvel = body.angvel();
            if body_type != RigidBodyType::Dynamic {
                to_reset.push(cube.body);
                continue;
            }
            if body_is_sleeping {
                if has_hand_contact {
                    if let Some(body) = self.bodies.get_mut(cube.body) {
                        body.wake_up(true);
                    }
                }
                to_reset.push(cube.body);
                continue;
            }

            let linvel_sq = linvel.x * linvel.x + linvel.y * linvel.y + linvel.z * linvel.z;
            let angvel_sq = angvel.x * angvel.x + angvel.y * angvel.y + angvel.z * angvel.z;
            let allow_settle = (!contact_state.has_depth_query_contact
                || contact_state.has_support_contact)
                && !has_hand_contact;
            if allow_settle && linvel_sq <= linear_speed_sq && angvel_sq <= angular_speed_sq {
                let frames = self
                    .settle_frame_counts
                    .entry(cube.body)
                    .and_modify(|frames| *frames = frames.saturating_add(1))
                    .or_insert(1);
                if *frames >= XR_BODY_SNAP_SLEEP_SETTLE_FRAMES {
                    to_sleep.push(cube.body);
                }
            } else {
                to_reset.push(cube.body);
            }
        }

        for handle in to_reset {
            self.release_settle_state(handle);
        }
        for handle in to_sleep {
            if let Some(body) = self.bodies.get_mut(handle) {
                body.set_linvel(RapierVector::ZERO, false);
                body.set_angvel(RapierVector::ZERO, false);
            }
            self.release_settle_state(handle);
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

    pub(crate) fn register_spawn_pool_cube(&mut self, cube_index: usize) {
        let Some(cube) = self.cubes.get(cube_index).copied() else {
            return;
        };
        self.spawn_pool_cube_indices.push(cube_index);
        self.shadow_body_motion.remove(&cube.body);
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
        shadow: bool,
        mode: XrSharedObjectMode,
        pose: Pose,
        linvel: Vec3f,
        angvel: Vec3f,
    ) -> Option<u64> {
        let cube = self
            .cubes
            .iter()
            .find(|cube| cube.widget_uid == widget_uid)
            .copied()?;
        self.clear_held_body(cube.body);
        self.shadow_body_motion.remove(&cube.body);
        self.release_settle_state(cube.body);
        if let Some(body) = self.bodies.get_mut(cube.body) {
            body.set_enabled(true);
            body.set_position(rapier_pose(pose), false);
            if shadow {
                let remaining_prediction_seconds = if matches!(mode, XrSharedObjectMode::Sleeping)
                {
                    0.0
                } else {
                    XR_SHADOW_BODY_MAX_EXTRAPOLATION_SECONDS
                };
                body.set_body_type(RigidBodyType::KinematicPositionBased, true);
                body.set_next_kinematic_position(rapier_pose(pose));
                body.set_linvel(RapierVector::ZERO, false);
                body.set_angvel(RapierVector::ZERO, false);
                body.wake_up(true);
                self.shadow_body_motion.insert(
                    cube.body,
                    ShadowBodyMotion {
                        pose,
                        linvel,
                        angvel,
                        remaining_prediction_seconds,
                    },
                );
            } else {
                match mode {
                    XrSharedObjectMode::ContactDominated { .. } => {
                        body.set_body_type(RigidBodyType::KinematicPositionBased, true);
                        body.set_next_kinematic_position(rapier_pose(pose));
                        body.set_linvel(RapierVector::ZERO, false);
                        body.set_angvel(RapierVector::ZERO, false);
                        body.wake_up(true);
                    }
                    XrSharedObjectMode::Dynamic => {
                        body.set_body_type(RigidBodyType::Dynamic, true);
                        body.set_linvel(rapier_vec3(linvel), true);
                        body.set_angvel(rapier_vec3(angvel), true);
                        body.wake_up(true);
                    }
                    XrSharedObjectMode::Sleeping => {
                        body.set_body_type(RigidBodyType::Dynamic, true);
                        body.set_linvel(rapier_vec3(linvel), false);
                        body.set_angvel(rapier_vec3(angvel), false);
                    }
                }
            }
            body.reset_forces(false);
            body.reset_torques(false);
        }
        cube.depth_query_surface_set
            .map(RapierScene::depth_query_key)
    }

    pub(crate) fn apply_impulse(
        &mut self,
        widget_uid: WidgetUid,
        point: Vec3f,
        impulse: Vec3f,
    ) -> bool {
        let Some(cube) = self
            .cubes
            .iter()
            .find(|cube| cube.widget_uid == widget_uid)
            .copied()
        else {
            return false;
        };
        self.release_settle_state(cube.body);
        if self.shadow_body_motion.contains_key(&cube.body) {
            return false;
        }
        let Some(body) = self.bodies.get_mut(cube.body) else {
            return false;
        };
        if !body.is_enabled() || body.body_type() != RigidBodyType::Dynamic {
            return false;
        }
        body.apply_impulse_at_point(rapier_vec3(impulse), rapier_vec3(point), true);
        body.wake_up(true);
        true
    }

    pub(crate) fn despawn_body(&mut self, widget_uid: WidgetUid) -> Option<u64> {
        let cube = self
            .cubes
            .iter()
            .find(|cube| cube.widget_uid == widget_uid)
            .copied()?;
        self.clear_held_body(cube.body);
        self.shadow_body_motion.remove(&cube.body);
        self.release_settle_state(cube.body);
        if let Some(body) = self.bodies.get_mut(cube.body) {
            body.set_enabled(false);
            body.set_linvel(RapierVector::ZERO, false);
            body.set_angvel(RapierVector::ZERO, false);
            body.reset_forces(false);
            body.reset_torques(false);
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
        if self.shadow_body_motion.contains_key(&surface_set.body) {
            for surface in &surface_set.surfaces {
                if let Some(collider) = self.colliders.get_mut(surface.collider) {
                    collider.set_enabled(false);
                }
            }
            return;
        }
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
                let DepthQueryColliderGeometry::HalfSpace(plane) = target.collider.geometry;
                let supports_body = depth_query_surface_target_should_enable(
                    *target,
                    body_position,
                    body_velocity,
                    surface_set.query_radius,
                    physics_edge_margin,
                );
                if surface.fingerprint != target.collider.fingerprint {
                    collider.set_shape(SharedShape::halfspace(rapier_vec3(plane.normal)));
                    collider.set_position_wrt_parent(RapierPose::from_parts(
                        rapier_vec3(plane.point),
                        RapierRotation::IDENTITY,
                    ));
                    surface.fingerprint = target.collider.fingerprint;
                }
                collider.user_data = depth_query_surface_user_data(
                    surface_set.owner_widget_uid,
                    target.collider.role,
                );
                collider.set_restitution(target.collider.restitution.max(0.0));
                collider.set_enabled(supports_body);
                if supports_body {
                    self.depth_query_stats.surface_count += 1;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::algorithms::tsdf_query::{DepthQueryCollider, DepthQuerySupportPlane};

    fn assert_vec3_close(actual: Vec3f, expected: Vec3f, tolerance: f32) {
        assert!(
            (actual - expected).length() <= tolerance,
            "expected {:?} to be within {} of {:?}",
            actual,
            tolerance,
            expected
        );
    }

    #[test]
    fn impact_surface_enables_before_current_body_overlaps_quad() {
        let plane = DepthQuerySupportPlane {
            point: vec3f(1.0, 0.0, 0.0),
            normal: vec3f(-1.0, 0.0, 0.0),
            tangent: vec3f(0.0, 1.0, 0.0),
            bitangent: vec3f(0.0, 0.0, 1.0),
            half_extent_tangent: 0.08,
            half_extent_bitangent: 0.08,
        };
        let target = DepthQuerySurfaceTarget {
            collider: DepthQueryCollider {
                fingerprint: 1,
                geometry: DepthQueryColliderGeometry::HalfSpace(plane),
                role: DepthQueryColliderRole::Impact,
                restitution: 0.38,
            },
        };

        assert!(depth_query_surface_target_should_enable(
            target,
            vec3f(0.78, 0.0, 0.0),
            vec3f(0.55, 0.0, 0.0),
            0.05,
            0.004,
        ));
    }

    #[test]
    fn respawn_body_applies_shadow_contact_dominated_and_sleeping_modes() {
        let mut scene = RapierScene::new(0.0);
        let widget_uid = WidgetUid(41);
        let pose = Pose::new(Quat::default(), vec3f(0.08, 1.12, -0.44));
        scene.spawn_dynamic_box(
            widget_uid,
            pose,
            vec3f(0.04, 0.04, 0.04),
            vec3f(1.0, 1.0, 1.0),
            1.0,
            0.5,
            0.0,
        );
        let cube = scene.cubes[0];

        scene.respawn_body(
            widget_uid,
            true,
            XrSharedObjectMode::Dynamic,
            pose,
            vec3f(1.0, 2.0, 3.0),
            vec3f(4.0, 5.0, 6.0),
        );
        let body = scene
            .bodies
            .get(cube.body)
            .expect("spawned cube body should exist");
        assert_eq!(body.body_type(), RigidBodyType::KinematicPositionBased);
        assert_eq!(
            scene.shadow_body_motion_for_body(cube.body),
            Some((vec3f(1.0, 2.0, 3.0), vec3f(4.0, 5.0, 6.0)))
        );

        scene.respawn_body(
            widget_uid,
            false,
            XrSharedObjectMode::ContactDominated {
                authority: XrPeerId(7),
                hand: XrSharedHand::RightHand,
            },
            pose,
            vec3f(0.5, 0.0, -0.5),
            vec3f(0.0, 1.0, 0.0),
        );
        let body = scene
            .bodies
            .get(cube.body)
            .expect("cube body should still exist after contact-dominated respawn");
        assert_eq!(body.body_type(), RigidBodyType::KinematicPositionBased);
        assert!(!body.is_sleeping());

        scene.respawn_body(
            widget_uid,
            false,
            XrSharedObjectMode::Sleeping,
            pose,
            vec3f(0.0, 0.0, 0.0),
            vec3f(0.0, 0.0, 0.0),
        );
        let body = scene
            .bodies
            .get(cube.body)
            .expect("cube body should still exist after sleeping respawn");
        assert_eq!(body.body_type(), RigidBodyType::Dynamic);
        assert_vec3_close(
            vec3f(body.linvel().x, body.linvel().y, body.linvel().z),
            vec3f(0.0, 0.0, 0.0),
            0.0001,
        );
        assert_vec3_close(
            vec3f(body.angvel().x, body.angvel().y, body.angvel().z),
            vec3f(0.0, 0.0, 0.0),
            0.0001,
        );
    }

    #[test]
    fn shadow_respawn_keeps_projecting_motion_between_corrections() {
        let mut scene = RapierScene::new(0.0);
        let widget_uid = WidgetUid(410);
        let pose = Pose::new(Quat::default(), vec3f(0.0, 1.0, -0.2));
        scene.spawn_dynamic_box(
            widget_uid,
            pose,
            vec3f(0.05, 0.05, 0.05),
            vec3f(1.0, 1.0, 1.0),
            1.0,
            0.5,
            0.0,
        );
        let cube = scene.cubes[0];

        scene.respawn_body(
            widget_uid,
            true,
            XrSharedObjectMode::Dynamic,
            pose,
            vec3f(0.0, 0.0, -1.5),
            vec3f(0.0, 0.0, 0.0),
        );
        scene.step();

        let body = scene
            .bodies
            .get(cube.body)
            .expect("shadow body should exist after a step");
        let stepped_pose = makepad_pose(body.position());
        assert_eq!(body.body_type(), RigidBodyType::KinematicPositionBased);
        assert!(
            stepped_pose.position.z < pose.position.z - 0.0001,
            "shadow body should keep moving forward between network corrections: {stepped_pose:?}"
        );
        assert_eq!(
            scene
                .shadow_body_motion_for_body(cube.body)
                .map(|(linvel, _)| linvel)
                .unwrap_or_default(),
            vec3f(0.0, 0.0, -1.5)
        );
    }

    #[test]
    fn shadow_dynamic_body_ignores_local_wall_until_extrapolation_runs_out() {
        let mut scene = RapierScene::new(0.0);
        let projectile_uid = WidgetUid(411);
        let wall_uid = WidgetUid(412);
        let pose = Pose::new(Quat::default(), vec3f(0.0, 1.0, -0.2));
        scene.spawn_dynamic_sphere(
            projectile_uid,
            pose,
            0.05,
            vec3f(1.0, 1.0, 1.0),
            1.0,
            0.0,
            0.95,
        );
        scene.spawn_fixed_box(
            wall_uid,
            Pose::new(Quat::default(), vec3f(0.0, 1.0, -0.55)),
            vec3f(1.0, 1.0, 0.05),
            vec3f(1.0, 1.0, 1.0),
            0.0,
            0.95,
        );
        let cube = scene.cubes[0];

        scene.respawn_body(
            projectile_uid,
            true,
            XrSharedObjectMode::Dynamic,
            pose,
            vec3f(0.0, 0.0, -2.0),
            vec3f(0.0, 0.0, 0.0),
        );
        for _ in 0..30 {
            scene.step();
        }

        let body = scene
            .bodies
            .get(cube.body)
            .expect("shadow projectile should still exist after stepping");
        assert!(
            makepad_pose(body.position()).position.z <= -0.39,
            "shadow projectile should keep dead-reckoning forward instead of locally bouncing on observer-only walls: {:?}",
            makepad_pose(body.position()).position
        );
        assert!(
            makepad_pose(body.position()).position.z >= -0.45,
            "shadow projectile should stop near the extrapolation horizon when no fresh authority samples arrive: {:?}",
            makepad_pose(body.position()).position
        );
        assert_eq!(body.body_type(), RigidBodyType::KinematicPositionBased);
    }

    #[test]
    fn apply_impulse_only_affects_dynamic_enabled_bodies() {
        let mut scene = RapierScene::new(0.0);
        let widget_uid = WidgetUid(42);
        let pose = Pose::new(Quat::default(), vec3f(0.0, 1.0, 0.0));
        scene.spawn_dynamic_box(
            widget_uid,
            pose,
            vec3f(0.05, 0.05, 0.05),
            vec3f(1.0, 1.0, 1.0),
            1.0,
            0.5,
            0.0,
        );
        let cube = scene.cubes[0];

        assert!(scene.apply_impulse(widget_uid, pose.position, vec3f(0.0, 0.0, -1.0),));
        let body = scene
            .bodies
            .get(cube.body)
            .expect("cube body should exist after impulse");
        assert!(body.linvel().z < 0.0);

        scene.respawn_body(
            widget_uid,
            true,
            XrSharedObjectMode::Dynamic,
            pose,
            vec3f(0.0, 0.0, 0.0),
            vec3f(0.0, 0.0, 0.0),
        );
        assert!(
            !scene.apply_impulse(widget_uid, pose.position, vec3f(0.0, 0.0, -1.0)),
            "shadow bodies should reject dynamic impulses"
        );

        scene.despawn_body(widget_uid);
        assert!(
            !scene.apply_impulse(widget_uid, pose.position, vec3f(0.0, 0.0, -1.0)),
            "disabled bodies should reject impulses"
        );
    }

    #[test]
    fn spawn_pool_respawn_reenables_body_and_survives_a_step() {
        let mut scene = RapierScene::new(0.0);
        let widget_uid = WidgetUid(420);
        let pose = Pose::new(Quat::default(), vec3f(0.0, 1.0, -0.4));
        scene.spawn_dynamic_sphere(widget_uid, pose, 0.04, vec3f(1.0, 1.0, 1.0), 1.0, 0.5, 0.0);
        scene.register_spawn_pool_cube(0);

        let cube = scene.cubes[0];
        let body = scene
            .bodies
            .get(cube.body)
            .expect("projectile body should exist before respawn");
        assert!(!body.is_enabled(), "projectile pool bodies start disabled");

        scene.respawn_body(
            widget_uid,
            false,
            XrSharedObjectMode::Dynamic,
            pose,
            vec3f(0.0, 0.0, -8.0),
            vec3f(0.0, 0.0, 0.0),
        );

        let body = scene
            .bodies
            .get(cube.body)
            .expect("projectile body should still exist after respawn");
        assert!(
            body.is_enabled(),
            "respawn should re-enable the pooled body"
        );
        assert_eq!(body.body_type(), RigidBodyType::Dynamic);

        scene.step();

        let body = scene
            .bodies
            .get(cube.body)
            .expect("projectile body should still exist after a step");
        assert!(
            body.is_enabled(),
            "a respawned pooled body should remain enabled after stepping"
        );
    }

    #[test]
    fn hand_contact_grab_and_release_restores_dynamic_body_velocity() {
        let mut scene = RapierScene::new(0.0);
        let widget_uid = WidgetUid(43);
        let pose = Pose::new(Quat::default(), vec3f(0.0, 1.0, 0.0));
        scene.spawn_dynamic_box(
            widget_uid,
            pose,
            vec3f(0.05, 0.05, 0.05),
            vec3f(1.0, 1.0, 1.0),
            1.0,
            0.5,
            0.0,
        );
        let cube = scene.cubes[0];
        let hand_pose = Pose::new(Quat::default(), pose.position);

        RapierScene::sync_hand_bodies(
            &scene.left_hand,
            &[HandCollider::Ball {
                center: pose.position,
                radius: 0.09,
            }],
            &mut scene.bodies,
            &mut scene.colliders,
        );
        scene.left_hand_grab = HandGrabState {
            shared_hand: XrSharedHand::LeftHand,
            pose: hand_pose,
            previous_pose: hand_pose,
            linvel: vec3f(0.0, 0.0, 0.0),
            tracked: true,
            gripping: true,
            held_body: None,
            grab_offset: Pose::default(),
        };

        scene.step();
        assert_eq!(scene.left_hand_grab.held_body, Some(cube.body));
        let body = scene
            .bodies
            .get(cube.body)
            .expect("cube body should exist after grab");
        assert_eq!(body.body_type(), RigidBodyType::KinematicPositionBased);
        assert_eq!(
            scene.held_by_for_body(cube.body),
            Some(XrSharedHand::LeftHand)
        );

        let release_velocity = vec3f(0.6, 0.0, -0.4);
        scene.left_hand_grab.linvel = release_velocity;
        scene.left_hand_grab.gripping = false;
        scene.apply_held_body_targets();

        assert_eq!(scene.left_hand_grab.held_body, None);
        let body = scene
            .bodies
            .get(cube.body)
            .expect("cube body should still exist after release");
        assert_eq!(body.body_type(), RigidBodyType::Dynamic);
        assert_vec3_close(
            vec3f(body.linvel().x, body.linvel().y, body.linvel().z),
            release_velocity,
            0.0001,
        );
    }

    #[test]
    fn hand_grab_anchors_body_surface_to_hand_pose_instead_of_body_center() {
        let mut scene = RapierScene::new(0.0);
        let widget_uid = WidgetUid(430);
        let pose = Pose::new(Quat::default(), vec3f(0.0, 1.0, 0.0));
        let half_extents = vec3f(0.05, 0.05, 0.05);
        scene.spawn_dynamic_box(
            widget_uid,
            pose,
            half_extents,
            vec3f(1.0, 1.0, 1.0),
            1.0,
            0.5,
            0.0,
        );
        let cube = scene.cubes[0];
        let acquire_pose = Pose::new(Quat::default(), pose.position + vec3f(0.0, 0.0, 0.11));
        let moved_pose = Pose::new(Quat::default(), pose.position + vec3f(0.16, 0.0, 0.11));

        RapierScene::sync_hand_bodies(
            &scene.left_hand,
            &[HandCollider::Ball {
                center: acquire_pose.position,
                radius: 0.09,
            }],
            &mut scene.bodies,
            &mut scene.colliders,
        );
        scene.left_hand_grab = HandGrabState {
            shared_hand: XrSharedHand::LeftHand,
            pose: acquire_pose,
            previous_pose: acquire_pose,
            linvel: vec3f(0.0, 0.0, 0.0),
            tracked: true,
            gripping: true,
            held_body: None,
            grab_offset: Pose::default(),
        };

        scene.step();
        assert_eq!(scene.left_hand_grab.held_body, Some(cube.body));

        scene.left_hand_grab.previous_pose = scene.left_hand_grab.pose;
        scene.left_hand_grab.pose = moved_pose;
        scene.apply_held_body_targets();
        scene.step();

        let body = scene
            .bodies
            .get(cube.body)
            .expect("cube body should exist after moving the held hand");
        let body_pose = makepad_pose(body.position());
        let center_distance = (body_pose.position - moved_pose.position).length();
        assert!(
            (center_distance - half_extents.z).abs() <= 0.012,
            "center_distance={center_distance:?} body_pose={body_pose:?} moved_pose={moved_pose:?}"
        );

        let local_anchor = scene.left_hand_grab.grab_offset.invert().position;
        assert!(
            (local_anchor.z - half_extents.z).abs() <= 0.012,
            "local_anchor={local_anchor:?} half_extents={half_extents:?}"
        );
    }

    #[test]
    fn secondary_hand_can_join_existing_hold_and_keep_body_kinematic() {
        let mut scene = RapierScene::new(0.0);
        let widget_uid = WidgetUid(44);
        let pose = Pose::new(Quat::default(), vec3f(0.0, 1.0, 0.0));
        scene.spawn_dynamic_box(
            widget_uid,
            pose,
            vec3f(0.05, 0.05, 0.05),
            vec3f(1.0, 1.0, 1.0),
            1.0,
            0.5,
            0.0,
        );
        let cube = scene.cubes[0];
        let left_pose = Pose::new(Quat::default(), pose.position);
        let right_pose = Pose::new(Quat::default(), pose.position + vec3f(0.08, 0.0, 0.0));

        RapierScene::sync_hand_bodies(
            &scene.left_hand,
            &[HandCollider::Ball {
                center: pose.position,
                radius: 0.09,
            }],
            &mut scene.bodies,
            &mut scene.colliders,
        );
        scene.left_hand_grab = HandGrabState {
            shared_hand: XrSharedHand::LeftHand,
            pose: left_pose,
            previous_pose: left_pose,
            linvel: vec3f(0.0, 0.0, 0.0),
            tracked: true,
            gripping: true,
            held_body: None,
            grab_offset: Pose::default(),
        };
        scene.step();
        assert_eq!(scene.left_hand_grab.held_body, Some(cube.body));

        RapierScene::sync_hand_bodies(
            &scene.right_hand,
            &[HandCollider::Ball {
                center: pose.position,
                radius: 0.09,
            }],
            &mut scene.bodies,
            &mut scene.colliders,
        );
        scene.right_hand_grab = HandGrabState {
            shared_hand: XrSharedHand::RightHand,
            pose: right_pose,
            previous_pose: right_pose,
            linvel: vec3f(0.0, 0.0, 0.0),
            tracked: true,
            gripping: true,
            held_body: None,
            grab_offset: Pose::default(),
        };
        scene.step();

        assert_eq!(scene.left_hand_grab.held_body, Some(cube.body));
        assert_eq!(scene.right_hand_grab.held_body, Some(cube.body));
        let body = scene
            .bodies
            .get(cube.body)
            .expect("cube body should exist while two hands hold it");
        assert_eq!(body.body_type(), RigidBodyType::KinematicPositionBased);

        scene.left_hand_grab.gripping = false;
        scene.apply_held_body_targets();

        assert_eq!(scene.left_hand_grab.held_body, None);
        assert_eq!(scene.right_hand_grab.held_body, Some(cube.body));
        let body = scene
            .bodies
            .get(cube.body)
            .expect("cube body should still exist after primary hand drops");
        assert_eq!(body.body_type(), RigidBodyType::KinematicPositionBased);
        assert_eq!(
            scene.held_by_for_body(cube.body),
            Some(XrSharedHand::RightHand)
        );
    }

    #[test]
    fn sticky_raw_grab_bit_does_not_keep_pointing_hand_in_grab_state() {
        let mut scene = RapierScene::new(0.0);
        let widget_uid = WidgetUid(45);
        let pose = Pose::new(Quat::default(), vec3f(0.0, 1.0, 0.0));
        scene.spawn_dynamic_box(
            widget_uid,
            pose,
            vec3f(0.05, 0.05, 0.05),
            vec3f(1.0, 1.0, 1.0),
            1.0,
            0.5,
            0.0,
        );
        let cube = scene.cubes[0];
        let hand_pose = Pose::new(Quat::default(), pose.position);

        scene.left_hand_grab = HandGrabState {
            shared_hand: XrSharedHand::LeftHand,
            pose: hand_pose,
            previous_pose: hand_pose,
            linvel: vec3f(0.0, 0.0, 0.0),
            tracked: true,
            gripping: true,
            held_body: Some(cube.body),
            grab_offset: Pose::default(),
        };

        let mut hand = XrHand::default();
        hand.flags = XrHand::IN_VIEW | XrHand::AIM_VALID;
        hand.tips_active = XrHand::GRAB_ACTIVE | (1 << XrHand::INDEX_TIP);
        hand.tips[XrHand::INDEX_TIP] = 0.038;
        hand.joints[XrHand::CENTER] = Pose::new(Quat::default(), pose.position);
        hand.joints[XrHand::WRIST] =
            Pose::new(Quat::default(), pose.position + vec3f(0.0, -0.03, 0.05));
        hand.joints[XrHand::INDEX_BASE] = Pose::new(Quat::default(), pose.position);
        hand.joints[XrHand::INDEX_KNUCKLE1] =
            Pose::new(Quat::default(), pose.position + vec3f(0.0, 0.0, -0.041));
        hand.joints[XrHand::INDEX_KNUCKLE2] =
            Pose::new(Quat::default(), pose.position + vec3f(0.001, 0.002, -0.082));
        hand.joints[XrHand::INDEX_KNUCKLE3] =
            Pose::new(Quat::default(), pose.position + vec3f(0.002, 0.004, -0.122));

        scene.sync_tracked_hands(&hand, &XrHand::default());
        scene.apply_held_body_targets();

        assert_eq!(scene.left_hand_grab.held_body, None);
        let body = scene
            .bodies
            .get(cube.body)
            .expect("cube body should still exist after sticky-grab release");
        assert_eq!(body.body_type(), RigidBodyType::Dynamic);
    }
}
