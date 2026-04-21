#![allow(dead_code)]

use super::xr_depth::{DepthQuerySurfaceCollider, DepthQuerySurfaceTarget};
use super::*;
use crate::algorithms::tsdf_query::{
    depth_query_plane_supports_body, DepthQueryColliderGeometry, DepthQueryColliderRole,
};
use rapier3d::control::{DynamicRayCastVehicleController, WheelTuning};
use rapier3d::dynamics::CoefficientCombineRule;
use rapier3d::pipeline::{ActiveHooks, PairFilterContext, PhysicsHooks};
use rapier3d::prelude::{Collider, QueryFilter, RigidBodyType, SolverFlags};
use std::collections::{HashMap, HashSet};

const XR_MAX_LINKED_SUPPORT_BODIES_PER_CUBE: usize = XR_RUNTIME_LINKED_SUPPORT_BODY_COUNT;
pub(super) const XR_MAX_DEPTH_QUERY_KEYS_PER_CUBE: usize =
    XR_MAX_LINKED_SUPPORT_BODIES_PER_CUBE + 1;
const XR_FOUR_WHEEL_FRONT_BACK_FRACTION: f32 = 0.97;
const XR_FOUR_WHEEL_LATERAL_FRACTION: f32 = 0.92;
const XR_FOUR_WHEEL_RADIUS_SCALE: f32 = 3.20;
const XR_FOUR_WHEEL_REST_LENGTH_SCALE: f32 = 0.50;
const XR_FOUR_WHEEL_TRAVEL_SCALE: f32 = 0.50;
const XR_FOUR_WHEEL_MIN_SUSPENSION_LENGTH_FRACTION: f32 = 0.0;
const XR_FOUR_WHEEL_CHASSIS_WIDTH_SCALE: f32 = 0.85;
const XR_FOUR_WHEEL_CHASSIS_HEIGHT_SCALE: f32 = 0.70;
const XR_FOUR_WHEEL_CHASSIS_DEPTH_SCALE: f32 = 0.85;
const XR_FOUR_WHEEL_CHASSIS_UP_OFFSET_SCALE: f32 = 0.20;
const XR_FOUR_WHEEL_MIN_CHASSIS_CLEARANCE_FRACTION: f32 = 0.0;
const XR_CAR_MASS_KG: f32 = 500.0;
const XR_CAR_MAX_STEER_DEG: f32 = 55.0;
const XR_CAR_STEER_SMOOTHING_FACTOR: f32 = 0.1;
const XR_CAR_ACCELERATION_FORCE: f32 = 18.0;
const XR_CAR_BRAKE_FORCE: f32 = 12.0;
const XR_CAR_TOP_SPEED_MPS: f32 = 25.0;
const XR_CAR_DOWNFORCE_GAIN: f32 = 20.0;
const XR_CAR_WHEEL_SUSPENSION_COMPRESSION: f32 = 3.0;
const XR_CAR_WHEEL_SUSPENSION_RELAXATION: f32 = 5.0;
const XR_CAR_WHEEL_SUSPENSION_STIFFNESS: f32 = 50.0;
const XR_CAR_WHEEL_MAX_SUSPENSION_FORCE: f32 = 100_000_000.0;
const XR_CAR_WHEEL_SIDE_FRICTION_STIFFNESS: f32 = 0.7;
const XR_CAR_WHEEL_FRICTION_LOW: f32 = 1.0;
const XR_CAR_WHEEL_FRICTION_HIGH: f32 = 20.0;

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
    pub(crate) friction: f32,
    pub(crate) depth_query_surface_set: Option<usize>,
    linked_support_bodies: [Option<usize>; XR_MAX_LINKED_SUPPORT_BODIES_PER_CUBE],
}

#[derive(Clone, Copy)]
pub(super) struct HandColliderBody {
    pub(super) body: RigidBodyHandle,
    pub(super) collider: ColliderHandle,
}

#[derive(Clone, Copy)]
struct FloorHalfspaceCollider {
    body: RigidBodyHandle,
    collider: ColliderHandle,
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

#[derive(Clone, Copy)]
struct BodyDriveMotion {
    linvel: Vec3f,
    angvel: Vec3f,
    max_linear_accel: f32,
    max_angular_accel: f32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum VehicleDrive {
    All,
    Rear,
    Front,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum VehicleAxle {
    Front,
    Rear,
}

#[derive(Clone, Copy)]
struct VehicleConfig {
    drive: VehicleDrive,
    mass_kg: f32,
    max_steer_deg: f32,
    steer_smoothing_factor: f32,
    acceleration_force: f32,
    brake_force: f32,
    top_speed_mps: f32,
    downforce_gain: f32,
}

#[derive(Clone, Copy)]
struct VehicleWheelRuntime {
    support_index: usize,
    axle: VehicleAxle,
    radius: f32,
    friction_low: f32,
    friction_high: f32,
    depth_query_filter_key: u64,
}

struct VehicleRuntime {
    widget_uid: WidgetUid,
    chassis_body: RigidBodyHandle,
    controller: DynamicRayCastVehicleController,
    config: VehicleConfig,
    wheels: Vec<VehicleWheelRuntime>,
    current_steer: f32,
    steer_input: f32,
    throttle_input: f32,
    brake_input: f32,
    airtime: f32,
}

#[derive(Clone, Copy)]
struct SupportMarkerSpec {
    anchor_local_position: Vec3f,
    local_position: Vec3f,
    radius: f32,
    rest_length: f32,
    min_length: f32,
    max_length: f32,
}

#[derive(Clone, Copy)]
struct LinkedSupportBody {
    owner_widget_uid: WidgetUid,
    depth_query_filter_key: u64,
    owner_body: RigidBodyHandle,
    body: RigidBodyHandle,
    collider: ColliderHandle,
    anchor_local_position: Vec3f,
    local_pose: Pose,
    radius: f32,
    rest_length: f32,
    min_length: f32,
    max_length: f32,
    suspension_length: f32,
    previous_suspension_length: f32,
    query_radius: f32,
    depth_query_surface_set: Option<usize>,
    spin_angle: f32,
    steer_angle: f32,
}

#[derive(Clone, Copy)]
pub(super) struct DepthQuerySource {
    pub(super) set_index: usize,
    pub(super) body: RigidBodyHandle,
    pub(super) query_radius: f32,
}

struct DepthQueryBodySurfaceSet {
    depth_query_filter_key: u64,
    owner_body: RigidBodyHandle,
    query_body: RigidBodyHandle,
    query_radius: f32,
    surfaces: [DepthQuerySurfaceCollider; XR_DEPTH_QUERY_SURFACES_PER_PROBE],
    targets: [Option<DepthQuerySurfaceTarget>; XR_DEPTH_QUERY_SURFACES_PER_PROBE],
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
    linked_support_bodies: Vec<LinkedSupportBody>,
    vehicles: Vec<VehicleRuntime>,
    next_depth_query_filter_key: u64,
    spawn_pool_cube_indices: Vec<usize>,
    spawn_pool_cube_cursor: usize,
    depth_query_surface_sets: Vec<DepthQueryBodySurfaceSet>,
    depth_query_stats: DepthQueryPhysicsStats,
    floor_halfspace: Option<FloorHalfspaceCollider>,
    pub(super) left_hand: Vec<HandColliderBody>,
    pub(super) right_hand: Vec<HandColliderBody>,
    left_hand_grab: HandGrabState,
    right_hand_grab: HandGrabState,
    shadow_body_motion: HashMap<RigidBodyHandle, ShadowBodyMotion>,
    body_drive_motion: HashMap<RigidBodyHandle, BodyDriveMotion>,
    settle_frame_counts: HashMap<RigidBodyHandle, u8>,
    actively_driven_bodies: HashSet<RigidBodyHandle>,
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

fn clamp_vec3_length(value: Vec3f, max_length: f32) -> Vec3f {
    if !max_length.is_finite() || max_length <= 0.0 {
        return vec3f(0.0, 0.0, 0.0);
    }
    let length = value.length();
    if length <= max_length || length <= 1.0e-6 {
        value
    } else {
        value * (max_length / length)
    }
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
    let palm_pose = hand.tracking_pose()?;
    let thumb_tip = hand.tip_pos_checked(XrHand::THUMB_TIP);
    let index_tip = hand.tip_pos_checked(XrHand::INDEX_TIP);
    let position = match (thumb_tip, index_tip) {
        (Some(thumb_tip), Some(index_tip)) => (thumb_tip + index_tip) * 0.5,
        (_, Some(index_tip)) => index_tip,
        _ => return None,
    };
    Some(Pose::new(palm_pose.orientation, position))
}

fn controller_grab_pose(controller: &XrController) -> Option<Pose> {
    let pose = controller.grip_pose;
    (controller.active() && pose.is_finite()).then_some(pose)
}

fn depth_query_body_user_data(filter_key: u64) -> u128 {
    DEPTH_QUERY_BODY_USER_DATA_TAG | filter_key as u128
}

fn depth_query_surface_user_data(filter_key: u64, role: DepthQueryColliderRole) -> u128 {
    DEPTH_QUERY_SURFACE_USER_DATA_TAG
        | filter_key as u128
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

fn sphere_support_radius(half_extents: Vec3f) -> f32 {
    half_extents
        .x
        .min(half_extents.y)
        .min(half_extents.z)
        .max(0.0005)
}

fn four_wheel_support_radius(half_extents: Vec3f) -> f32 {
    (sphere_support_radius(half_extents) * XR_FOUR_WHEEL_RADIUS_SCALE).clamp(0.036, 0.160)
}

fn four_wheel_support_specs(
    half_extents: Vec3f,
) -> [Option<SupportMarkerSpec>; XR_MAX_LINKED_SUPPORT_BODIES_PER_CUBE] {
    let radius = four_wheel_support_radius(half_extents);
    let lateral = (half_extents.x * XR_FOUR_WHEEL_LATERAL_FRACTION).max(radius * 0.75);
    let rest_length = (radius * XR_FOUR_WHEEL_REST_LENGTH_SCALE).clamp(0.024, 0.110);
    let min_length_floor = (rest_length * XR_FOUR_WHEEL_MIN_SUSPENSION_LENGTH_FRACTION).max(0.004);
    let travel = (radius * XR_FOUR_WHEEL_TRAVEL_SCALE).clamp(0.018, 0.090);
    let min_length = (rest_length - travel).max(min_length_floor);
    let max_length = rest_length + travel;
    let local_wheel_center_y = -half_extents.y;
    let front = half_extents.z * XR_FOUR_WHEEL_FRONT_BACK_FRACTION;
    let back = -half_extents.z * XR_FOUR_WHEEL_FRONT_BACK_FRACTION;
    let positions = [
        vec3f(-lateral, local_wheel_center_y, front),
        vec3f(-lateral, local_wheel_center_y, back),
        vec3f(lateral, local_wheel_center_y, front),
        vec3f(lateral, local_wheel_center_y, back),
    ];
    std::array::from_fn(|index| {
        positions
            .get(index)
            .copied()
            .map(|anchor_local_position| SupportMarkerSpec {
                anchor_local_position: anchor_local_position + vec3f(0.0, rest_length, 0.0),
                local_position: anchor_local_position,
                radius,
                rest_length,
                min_length,
                max_length,
            })
    })
}

impl VehicleConfig {
    fn default_four_wheel() -> Self {
        Self {
            drive: VehicleDrive::All,
            mass_kg: XR_CAR_MASS_KG,
            max_steer_deg: XR_CAR_MAX_STEER_DEG,
            steer_smoothing_factor: XR_CAR_STEER_SMOOTHING_FACTOR,
            acceleration_force: XR_CAR_ACCELERATION_FORCE,
            brake_force: XR_CAR_BRAKE_FORCE,
            top_speed_mps: XR_CAR_TOP_SPEED_MPS,
            downforce_gain: XR_CAR_DOWNFORCE_GAIN,
        }
    }
}

fn linked_support_world_pose(owner_pose: Pose, support: LinkedSupportBody) -> Pose {
    Pose::multiply(&owner_pose, &support.local_pose)
}

fn four_wheel_chassis_collider_half_extents(half_extents: Vec3f) -> Vec3f {
    vec3f(
        half_extents.x * XR_FOUR_WHEEL_CHASSIS_WIDTH_SCALE,
        half_extents.y * XR_FOUR_WHEEL_CHASSIS_HEIGHT_SCALE,
        half_extents.z * XR_FOUR_WHEEL_CHASSIS_DEPTH_SCALE,
    )
}

fn four_wheel_chassis_collider_translation(half_extents: Vec3f) -> Vec3f {
    vec3f(
        0.0,
        half_extents.y * XR_FOUR_WHEEL_CHASSIS_UP_OFFSET_SCALE,
        0.0,
    )
}

fn four_wheel_chassis_collider_bottom_y(half_extents: Vec3f) -> f32 {
    let collider_half_extents = four_wheel_chassis_collider_half_extents(half_extents);
    let collider_translation = four_wheel_chassis_collider_translation(half_extents);
    collider_translation.y - collider_half_extents.y
}

fn four_wheel_min_chassis_clearance(radius: f32) -> f32 {
    (radius * XR_FOUR_WHEEL_MIN_CHASSIS_CLEARANCE_FRACTION).clamp(0.018, 0.040)
}

fn linked_support_world_linvel(
    owner_pose: Pose,
    support_pose: Pose,
    owner_linvel: Vec3f,
    owner_angvel: Vec3f,
) -> Vec3f {
    owner_linvel + Vec3f::cross(owner_angvel, support_pose.position - owner_pose.position)
}

fn wheel_query_accepts_collider(
    _handle: ColliderHandle,
    collider: &Collider,
    owner_body: RigidBodyHandle,
    support_body: RigidBodyHandle,
    support_filter_key: u64,
) -> bool {
    if collider.parent() == Some(owner_body) || collider.parent() == Some(support_body) {
        return false;
    }
    if let Some(owner) = decode_depth_query_surface_owner(collider.user_data) {
        return owner == support_filter_key;
    }
    decode_depth_query_body_owner(collider.user_data).is_none()
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

    fn allocate_depth_query_filter_key(&mut self) -> u64 {
        let key = self.next_depth_query_filter_key.max(1);
        self.next_depth_query_filter_key = key.saturating_add(1);
        key
    }

    fn spawn_dynamic_body(&mut self, pose: Pose) -> RigidBodyHandle {
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
        body
    }

    fn spawn_linked_support_marker(
        &mut self,
        owner_widget_uid: WidgetUid,
        owner_body: RigidBodyHandle,
        owner_pose: Pose,
        spec: SupportMarkerSpec,
        density: f32,
        friction: f32,
        restitution: f32,
    ) -> usize {
        let depth_query_filter_key = self.allocate_depth_query_filter_key();
        let local_pose = Pose::new(Quat::default(), spec.local_position);
        let world_pose = Pose::multiply(&owner_pose, &local_pose);
        let body = self.spawn_dynamic_body(world_pose);
        let collider = self.colliders.insert_with_parent(
            ColliderBuilder::ball(spec.radius)
                .user_data(depth_query_body_user_data(depth_query_filter_key))
                .density(density.max(1.0))
                .friction(friction.max(0.0))
                .friction_combine_rule(CoefficientCombineRule::Max)
                .restitution(restitution.max(0.0)),
            body,
            &mut self.bodies,
        );
        if let Some(collider) = self.colliders.get_mut(collider) {
            collider.set_enabled(false);
        }
        if let Some(body) = self.bodies.get_mut(body) {
            body.set_body_type(RigidBodyType::KinematicPositionBased, false);
            body.set_position(rapier_pose(world_pose), false);
            body.set_next_kinematic_position(rapier_pose(world_pose));
        }
        let depth_query_surface_set = XR_ENABLE_DEPTH_QUERY_PHYSICS.then(|| {
            self.spawn_depth_query_surface_set(
                depth_query_filter_key,
                owner_body,
                body,
                spec.radius,
            )
        });
        let index = self.linked_support_bodies.len();
        self.linked_support_bodies.push(LinkedSupportBody {
            owner_widget_uid,
            depth_query_filter_key,
            owner_body,
            body,
            collider,
            anchor_local_position: spec.anchor_local_position,
            local_pose,
            radius: spec.radius,
            rest_length: spec.rest_length,
            min_length: spec.min_length,
            max_length: spec.max_length,
            suspension_length: spec.rest_length,
            previous_suspension_length: spec.rest_length,
            query_radius: spec.radius,
            depth_query_surface_set,
            spin_angle: 0.0,
            steer_angle: 0.0,
        });
        index
    }

    fn attach_four_wheel_support_markers(
        &mut self,
        owner_widget_uid: WidgetUid,
        owner_pose: Pose,
        owner_body: RigidBodyHandle,
        half_extents: Vec3f,
        density: f32,
        friction: f32,
        restitution: f32,
    ) -> [Option<usize>; XR_MAX_LINKED_SUPPORT_BODIES_PER_CUBE] {
        let mut indices = [None; XR_MAX_LINKED_SUPPORT_BODIES_PER_CUBE];
        for (slot, spec) in four_wheel_support_specs(half_extents)
            .into_iter()
            .enumerate()
        {
            let Some(spec) = spec else {
                continue;
            };
            indices[slot] = Some(self.spawn_linked_support_marker(
                owner_widget_uid,
                owner_body,
                owner_pose,
                spec,
                density,
                friction,
                restitution,
            ));
        }
        indices
    }

    pub(crate) fn spawn_dynamic_box_with_support(
        &mut self,
        widget_uid: WidgetUid,
        pose: Pose,
        half_extents: Vec3f,
        scale: Vec3f,
        density: f32,
        friction: f32,
        restitution: f32,
        depth_query_support: XrDepthQuerySupportRig,
    ) {
        let body = self.spawn_dynamic_body(pose);
        let query_radius = half_extents.length().max(0.0005);
        let depth_query_filter_key = matches!(
            depth_query_support,
            XrDepthQuerySupportRig::Body | XrDepthQuerySupportRig::FourWheels
        )
        .then(|| self.allocate_depth_query_filter_key());
        let collider_half_extents =
            if matches!(depth_query_support, XrDepthQuerySupportRig::FourWheels) {
                four_wheel_chassis_collider_half_extents(half_extents)
            } else {
                half_extents
            };
        let collider_translation =
            if matches!(depth_query_support, XrDepthQuerySupportRig::FourWheels) {
                four_wheel_chassis_collider_translation(half_extents)
            } else {
                vec3f(0.0, 0.0, 0.0)
            };
        let collider_builder = ColliderBuilder::cuboid(
            collider_half_extents.x,
            collider_half_extents.y,
            collider_half_extents.z,
        )
        .translation(rapier_vec3(collider_translation))
        .user_data(
            depth_query_filter_key
                .map(depth_query_body_user_data)
                .unwrap_or(0),
        )
        .density(density.max(0.0))
        .friction(friction.max(0.0))
        .restitution(restitution.max(0.0));
        let collider = self
            .colliders
            .insert_with_parent(collider_builder, body, &mut self.bodies);
        let depth_query_surface_set = if XR_ENABLE_DEPTH_QUERY_PHYSICS
            && matches!(
                depth_query_support,
                XrDepthQuerySupportRig::Body | XrDepthQuerySupportRig::FourWheels
            ) {
            Some(
                self.spawn_depth_query_surface_set(
                    depth_query_filter_key
                        .expect("body depth query support should allocate a filter key"),
                    body,
                    body,
                    query_radius,
                ),
            )
        } else {
            None
        };
        let linked_support_bodies = match depth_query_support {
            XrDepthQuerySupportRig::FourWheels => self.attach_four_wheel_support_markers(
                widget_uid,
                pose,
                body,
                half_extents,
                density,
                friction,
                restitution,
            ),
            _ => [None; XR_MAX_LINKED_SUPPORT_BODIES_PER_CUBE],
        };
        self.cubes.push(PhysicsCube {
            widget_uid,
            body,
            collider,
            half_extents,
            query_radius,
            scale,
            body_kind: XrBodyKind::Dynamic,
            friction: friction.max(0.0),
            depth_query_surface_set,
            linked_support_bodies,
        });
        if matches!(depth_query_support, XrDepthQuerySupportRig::FourWheels) {
            self.create_four_wheel_vehicle(widget_uid, body, half_extents, linked_support_bodies);
        }
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
        self.spawn_dynamic_box_with_support(
            widget_uid,
            pose,
            half_extents,
            scale,
            density,
            friction,
            restitution,
            XrDepthQuerySupportRig::Body,
        );
    }

    pub(crate) fn spawn_dynamic_sphere_with_support(
        &mut self,
        widget_uid: WidgetUid,
        pose: Pose,
        radius: f32,
        scale: Vec3f,
        density: f32,
        friction: f32,
        restitution: f32,
        depth_query_support: XrDepthQuerySupportRig,
    ) {
        let body = self.spawn_dynamic_body(pose);
        let radius = radius.max(0.0005);
        let depth_query_filter_key = (!matches!(depth_query_support, XrDepthQuerySupportRig::None))
            .then(|| self.allocate_depth_query_filter_key());
        let collider = self.colliders.insert_with_parent(
            ColliderBuilder::ball(radius)
                .user_data(
                    depth_query_filter_key
                        .map(depth_query_body_user_data)
                        .unwrap_or(0),
                )
                .density(density.max(0.0))
                .friction(friction.max(0.0))
                .restitution(restitution.max(0.0)),
            body,
            &mut self.bodies,
        );
        let depth_query_surface_set = if XR_ENABLE_DEPTH_QUERY_PHYSICS
            && !matches!(depth_query_support, XrDepthQuerySupportRig::None)
        {
            Some(
                self.spawn_depth_query_surface_set(
                    depth_query_filter_key
                        .expect("sphere depth query support should allocate a filter key"),
                    body,
                    body,
                    radius,
                ),
            )
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
            friction: friction.max(0.0),
            depth_query_surface_set,
            linked_support_bodies: [None; XR_MAX_LINKED_SUPPORT_BODIES_PER_CUBE],
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
        self.spawn_dynamic_sphere_with_support(
            widget_uid,
            pose,
            radius,
            scale,
            density,
            friction,
            restitution,
            XrDepthQuerySupportRig::Body,
        );
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
                .user_data(0)
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
            friction: friction.max(0.0),
            depth_query_surface_set: None,
            linked_support_bodies: [None; XR_MAX_LINKED_SUPPORT_BODIES_PER_CUBE],
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
                .user_data(0)
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
            friction: friction.max(0.0),
            depth_query_surface_set: None,
            linked_support_bodies: [None; XR_MAX_LINKED_SUPPORT_BODIES_PER_CUBE],
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
            linked_support_bodies: Vec::new(),
            vehicles: Vec::new(),
            next_depth_query_filter_key: 1,
            spawn_pool_cube_indices: Vec::new(),
            spawn_pool_cube_cursor: 0,
            depth_query_surface_sets: Vec::new(),
            depth_query_stats: DepthQueryPhysicsStats::default(),
            floor_halfspace: None,
            left_hand: Vec::new(),
            right_hand: Vec::new(),
            left_hand_grab: HandGrabState::new(XrSharedHand::LeftHand),
            right_hand_grab: HandGrabState::new(XrSharedHand::RightHand),
            shadow_body_motion: HashMap::new(),
            body_drive_motion: HashMap::new(),
            settle_frame_counts: HashMap::new(),
            actively_driven_bodies: HashSet::new(),
        };

        scene.sync_floor_halfspace(None);
        if XR_ENABLE_HAND_PHYSICS {
            scene.left_hand = scene.spawn_hand_colliders(XR_HAND_COLLIDER_SLOTS_PER_HAND);
            scene.right_hand = scene.spawn_hand_colliders(XR_HAND_COLLIDER_SLOTS_PER_HAND);
        }
        scene.step();
        scene
    }

    fn ensure_floor_halfspace(&mut self) -> FloorHalfspaceCollider {
        if let Some(floor_halfspace) = self.floor_halfspace {
            return floor_halfspace;
        }
        let body = self.bodies.insert(RigidBodyBuilder::fixed().build());
        let collider = self.colliders.insert_with_parent(
            ColliderBuilder::new(SharedShape::halfspace(RapierVector::new(0.0, 1.0, 0.0)))
                .friction(0.9),
            body,
            &mut self.bodies,
        );
        let floor_halfspace = FloorHalfspaceCollider { body, collider };
        self.floor_halfspace = Some(floor_halfspace);
        floor_halfspace
    }

    pub(crate) fn sync_floor_halfspace(&mut self, floor_y: Option<f32>) {
        let floor_y = floor_y
            .filter(|value| value.is_finite())
            .or(XR_ENABLE_SYNTHETIC_GROUND_PLANE.then_some(0.0));
        let Some(floor_y) = floor_y else {
            if let Some(floor_halfspace) = self.floor_halfspace {
                if let Some(collider) = self.colliders.get_mut(floor_halfspace.collider) {
                    collider.set_enabled(false);
                }
            }
            return;
        };
        let floor_halfspace = self.ensure_floor_halfspace();
        if let Some(body) = self.bodies.get_mut(floor_halfspace.body) {
            body.set_position(
                RapierPose::from_parts(
                    RapierVector::new(0.0, floor_y, 0.0),
                    RapierRotation::IDENTITY,
                ),
                false,
            );
        }
        if let Some(collider) = self.colliders.get_mut(floor_halfspace.collider) {
            collider.set_enabled(true);
        }
    }

    fn vehicle_index_for_widget_uid(&self, widget_uid: WidgetUid) -> Option<usize> {
        self.vehicles
            .iter()
            .position(|vehicle| vehicle.widget_uid == widget_uid)
    }

    fn vehicle_index_for_body(&self, body_handle: RigidBodyHandle) -> Option<usize> {
        self.vehicles
            .iter()
            .position(|vehicle| vehicle.chassis_body == body_handle)
    }

    fn create_four_wheel_vehicle(
        &mut self,
        widget_uid: WidgetUid,
        chassis_body: RigidBodyHandle,
        half_extents: Vec3f,
        linked_support_bodies: [Option<usize>; XR_MAX_LINKED_SUPPORT_BODIES_PER_CUBE],
    ) {
        let config = VehicleConfig::default_four_wheel();
        let mut controller = DynamicRayCastVehicleController::new(chassis_body);
        controller.index_up_axis = 1;
        controller.index_forward_axis = 2;

        let mut wheels = Vec::new();
        for (slot, spec) in four_wheel_support_specs(half_extents)
            .into_iter()
            .enumerate()
        {
            let (Some(spec), Some(support_index)) = (spec, linked_support_bodies[slot]) else {
                continue;
            };
            let tuning = WheelTuning {
                suspension_stiffness: XR_CAR_WHEEL_SUSPENSION_STIFFNESS,
                suspension_compression: XR_CAR_WHEEL_SUSPENSION_COMPRESSION,
                suspension_damping: XR_CAR_WHEEL_SUSPENSION_RELAXATION,
                max_suspension_travel: (spec.max_length - spec.rest_length).max(0.0),
                side_friction_stiffness: XR_CAR_WHEEL_SIDE_FRICTION_STIFFNESS,
                friction_slip: XR_CAR_WHEEL_FRICTION_HIGH,
                max_suspension_force: XR_CAR_WHEEL_MAX_SUSPENSION_FORCE,
            };
            controller.add_wheel(
                rapier_vec3(spec.anchor_local_position),
                RapierVector::new(0.0, -1.0, 0.0),
                RapierVector::new(-1.0, 0.0, 0.0),
                spec.rest_length,
                spec.radius,
                &tuning,
            );
            let depth_query_filter_key = self
                .linked_support_bodies
                .get(support_index)
                .map(|support| support.depth_query_filter_key)
                .unwrap_or(0);
            wheels.push(VehicleWheelRuntime {
                support_index,
                axle: if spec.anchor_local_position.z >= 0.0 {
                    VehicleAxle::Front
                } else {
                    VehicleAxle::Rear
                },
                radius: spec.radius,
                friction_low: XR_CAR_WHEEL_FRICTION_LOW,
                friction_high: XR_CAR_WHEEL_FRICTION_HIGH,
                depth_query_filter_key,
            });
        }

        if let Some(body) = self.bodies.get_mut(chassis_body) {
            let additional_mass = (config.mass_kg - body.mass()).max(0.0);
            if additional_mass > 0.0 {
                body.set_additional_mass(additional_mass, true);
            }
            body.wake_up(true);
        }

        self.vehicles.push(VehicleRuntime {
            widget_uid,
            chassis_body,
            controller,
            config,
            wheels,
            current_steer: 0.0,
            steer_input: 0.0,
            throttle_input: 0.0,
            brake_input: 0.0,
            airtime: 0.0,
        });
    }

    pub(crate) fn clear_car_controls(&mut self) {
        for vehicle in &mut self.vehicles {
            vehicle.steer_input = 0.0;
            vehicle.throttle_input = 0.0;
            vehicle.brake_input = 0.0;
        }
    }

    pub(crate) fn apply_car_control(&mut self, control: XrCarControl) -> bool {
        let Some(index) = self.vehicle_index_for_widget_uid(control.widget_uid) else {
            return false;
        };
        let vehicle = &mut self.vehicles[index];
        vehicle.steer_input += control.steer;
        vehicle.throttle_input += control.throttle;
        vehicle.brake_input += control.brake.max(0.0);
        true
    }

    fn sync_vehicle_support_bodies(&mut self, vehicle_index: usize, prefer_controller_pose: bool) {
        let Some(vehicle) = self.vehicles.get(vehicle_index) else {
            return;
        };
        let Some(chassis) = self.bodies.get(vehicle.chassis_body) else {
            return;
        };
        let owner_pose = makepad_pose(chassis.position());
        let owner_linvel = vec3f(chassis.linvel().x, chassis.linvel().y, chassis.linvel().z);
        let owner_angvel = vec3f(chassis.angvel().x, chassis.angvel().y, chassis.angvel().z);
        let wheel_states = vehicle
            .wheels
            .iter()
            .enumerate()
            .filter_map(|(wheel_index, runtime)| {
                let support = self
                    .linked_support_bodies
                    .get(runtime.support_index)
                    .copied()?;
                let wheel = vehicle.controller.wheels().get(wheel_index)?;
                Some((wheel_index, *runtime, support, *wheel))
            })
            .collect::<Vec<_>>();

        for (_wheel_index, wheel_runtime, mut support, wheel) in wheel_states {
            let wheel_center = makepad_vec3(wheel.center());
            let hard_point = makepad_vec3(wheel.raycast_info().hard_point_ws);
            let has_controller_pose = prefer_controller_pose
                && (wheel_center.length() > 1.0e-6 || hard_point.length() > 1.0e-6);
            let support_pose = if has_controller_pose {
                // Match the rest of the XR transform stack:
                // world = owner * (steer * spin_about_local_axle).
                let steering_orientation =
                    Quat::from_axis_angle(vec3f(0.0, 1.0, 0.0), wheel.steering);
                let spin_orientation =
                    Quat::from_axis_angle(vec3f(-1.0, 0.0, 0.0), -wheel.rotation);
                let local_orientation = Quat::multiply(&spin_orientation, &steering_orientation);
                Pose::new(
                    Quat::multiply(&local_orientation, &owner_pose.orientation),
                    wheel_center,
                )
            } else {
                Pose::multiply(&owner_pose, &support.local_pose)
            };
            let point_velocity = owner_linvel
                + Vec3f::cross(owner_angvel, support_pose.position - owner_pose.position);
            support.previous_suspension_length = support.suspension_length;
            support.suspension_length = if has_controller_pose {
                wheel.raycast_info().suspension_length
            } else {
                support.rest_length
            };
            support.spin_angle = wheel.rotation;
            support.steer_angle = wheel.steering;
            self.linked_support_bodies[wheel_runtime.support_index] = support;
            if let Some(body) = self.bodies.get_mut(support.body) {
                body.set_enabled(true);
                if body.body_type() != RigidBodyType::KinematicPositionBased {
                    body.set_body_type(RigidBodyType::KinematicPositionBased, false);
                }
                body.set_position(rapier_pose(support_pose), false);
                body.set_next_kinematic_position(rapier_pose(support_pose));
                body.set_linvel(rapier_vec3(point_velocity), false);
                body.set_angvel(rapier_vec3(owner_angvel), false);
                body.reset_forces(false);
                body.reset_torques(false);
                body.wake_up(true);
            }
        }
    }

    pub(crate) fn sync_vehicle_query_sources_pre_step(&mut self) {
        for vehicle_index in 0..self.vehicles.len() {
            self.sync_vehicle_support_bodies(vehicle_index, true);
        }
    }

    fn update_four_wheel_vehicles(&mut self) {
        let dt = self.integration_parameters.dt.max(1.0 / 480.0);
        let broad_phase = &self.broad_phase;
        let dispatcher = self.narrow_phase.query_dispatcher();
        for vehicle_index in 0..self.vehicles.len() {
            let (
                chassis_body,
                linvel,
                up_ws,
                forward_ws,
                right_ws,
                body_mass,
                body_enabled,
                dynamic_body,
                held,
                shadowed,
            ) = {
                let vehicle = &self.vehicles[vehicle_index];
                let Some(body) = self.bodies.get(vehicle.chassis_body) else {
                    continue;
                };
                let pose = makepad_pose(body.position());
                (
                    vehicle.chassis_body,
                    vec3f(body.linvel().x, body.linvel().y, body.linvel().z),
                    pose.orientation.rotate_vec3(&vec3f(0.0, 1.0, 0.0)),
                    pose.orientation.rotate_vec3(&vec3f(0.0, 0.0, 1.0)),
                    pose.orientation.rotate_vec3(&vec3f(1.0, 0.0, 0.0)),
                    body.mass().max(0.001),
                    body.is_enabled(),
                    body.body_type() == RigidBodyType::Dynamic,
                    self.held_by_for_body(vehicle.chassis_body).is_some(),
                    self.shadow_body_motion.contains_key(&vehicle.chassis_body),
                )
            };
            if !body_enabled || !dynamic_body || held || shadowed {
                continue;
            }

            let (downforce_impulse, driven_active) = {
                let vehicle = &mut self.vehicles[vehicle_index];
                let steer_input = vehicle.steer_input.clamp(-1.0, 1.0);
                if vehicle.config.steer_smoothing_factor > 0.0 {
                    let t = (dt / vehicle.config.steer_smoothing_factor).clamp(0.0, 1.0);
                    vehicle.current_steer += (steer_input - vehicle.current_steer) * t;
                } else {
                    vehicle.current_steer = steer_input;
                }

                let current_acc = vehicle.throttle_input.clamp(-1.0, 1.0);
                let current_brake = vehicle.brake_input.max(0.0);
                let current_speed = vehicle.controller.current_vehicle_speed;
                let speed01 = if vehicle.config.top_speed_mps > 0.0 {
                    (current_speed / vehicle.config.top_speed_mps).clamp(0.0, 1.0)
                } else {
                    0.0
                };

                let downforce_impulse =
                    dt * body_mass * speed01.max(0.0) * vehicle.config.downforce_gain;

                let is_braking =
                    current_acc < 0.0 && current_speed > 0.05 && linvel.dot(forward_ws) > 0.0;
                let mut brake_force = if current_acc == 0.0 { 0.2 } else { 0.0 };
                if is_braking {
                    brake_force = vehicle.config.brake_force * -current_acc;
                }
                brake_force += current_brake * vehicle.config.brake_force;

                let reached_top_speed = current_speed > vehicle.config.top_speed_mps;
                let accel_force = if current_acc != 0.0 && !reached_top_speed {
                    (vehicle.config.acceleration_force / dt) * current_acc
                } else {
                    0.0
                };

                let max_angle = vehicle.config.max_steer_deg
                    + (vehicle.config.max_steer_deg * 0.5 - vehicle.config.max_steer_deg) * speed01;
                let steer = vehicle.current_steer * max_angle * std::f32::consts::PI / 180.0;

                let velocity = if linvel.length() > 1.0 {
                    linvel.normalize()
                } else {
                    linvel
                };
                let grip_amount = (1.0 - velocity.dot(right_ws).abs()).clamp(0.0, 1.0);

                for (wheel_index, wheel_runtime) in vehicle.wheels.iter().enumerate() {
                    let driven = match vehicle.config.drive {
                        VehicleDrive::All => true,
                        VehicleDrive::Front => wheel_runtime.axle == VehicleAxle::Front,
                        VehicleDrive::Rear => wheel_runtime.axle == VehicleAxle::Rear,
                    };
                    let friction = wheel_runtime.friction_low
                        + (wheel_runtime.friction_high - wheel_runtime.friction_low) * grip_amount;
                    if let Some(wheel) = vehicle.controller.wheels_mut().get_mut(wheel_index) {
                        wheel.engine_force = if driven { accel_force } else { 0.0 };
                        wheel.brake = brake_force;
                        wheel.steering = if wheel_runtime.axle == VehicleAxle::Front {
                            -steer
                        } else {
                            0.0
                        };
                        wheel.friction_slip = friction;
                    }
                }

                let driven_active = current_acc.abs() > 0.001
                    || current_brake > 0.001
                    || vehicle.current_steer.abs() > 0.001
                    || vehicle.controller.current_vehicle_speed.abs() > 0.001;
                (downforce_impulse, driven_active)
            };

            if downforce_impulse > 0.0 {
                if let Some(body) = self.bodies.get_mut(chassis_body) {
                    body.apply_impulse(rapier_vec3(-up_ws * downforce_impulse), true);
                }
            }

            {
                let vehicle = &mut self.vehicles[vehicle_index];
                let filter = |wheel_slot: usize, handle: ColliderHandle, collider: &Collider| {
                    let Some(wheel_runtime) = vehicle.wheels.get(wheel_slot) else {
                        return false;
                    };
                    let Some(support) = self.linked_support_bodies.get(wheel_runtime.support_index)
                    else {
                        return false;
                    };
                    wheel_query_accepts_collider(
                        handle,
                        collider,
                        chassis_body,
                        support.body,
                        wheel_runtime.depth_query_filter_key,
                    )
                };
                let query_pipeline = broad_phase.as_query_pipeline_mut(
                    dispatcher,
                    &mut self.bodies,
                    &mut self.colliders,
                    QueryFilter::new(),
                );
                vehicle
                    .controller
                    .update_vehicle_with_filter(dt, query_pipeline, filter);
                if vehicle
                    .controller
                    .wheels()
                    .iter()
                    .any(|wheel| wheel.raycast_info().is_in_contact)
                {
                    vehicle.airtime = 0.0;
                } else {
                    vehicle.airtime += dt;
                }
            }

            if driven_active {
                self.actively_driven_bodies.insert(chassis_body);
            }
        }
    }

    fn sync_vehicle_support_bodies_post_step(&mut self) {
        for vehicle_index in 0..self.vehicles.len() {
            self.sync_vehicle_support_bodies(vehicle_index, true);
        }
    }

    pub(super) fn sync_tracked_hands(
        &mut self,
        left_hand: &XrHand,
        right_hand: &XrHand,
        left_controller: &XrController,
        right_controller: &XrController,
    ) {
        self.left_hand_grab = self.updated_hand_grab_state(
            self.left_hand_grab,
            left_hand,
            left_controller,
            XrSharedHand::LeftHand,
            XrSharedHand::LeftController,
        );
        self.right_hand_grab = self.updated_hand_grab_state(
            self.right_hand_grab,
            right_hand,
            right_controller,
            XrSharedHand::RightHand,
            XrSharedHand::RightController,
        );
    }

    fn updated_hand_grab_state(
        &self,
        mut state: HandGrabState,
        hand: &XrHand,
        controller: &XrController,
        hand_shared: XrSharedHand,
        controller_shared: XrSharedHand,
    ) -> HandGrabState {
        let next_source = controller_grab_pose(controller)
            .map(|pose| (pose, controller.grip >= 0.55, controller_shared))
            .or_else(|| hand_grab_pose(hand).map(|pose| (pose, hand.grab_intent(), hand_shared)));
        let Some((pose, gripping, shared_hand)) = next_source else {
            state.previous_pose = state.pose;
            state.linvel = vec3f(0.0, 0.0, 0.0);
            state.tracked = false;
            state.gripping = false;
            state.shared_hand = hand_shared;
            return state;
        };
        let was_tracked = state.tracked;
        let previous_pose = if was_tracked && state.shared_hand == shared_hand {
            state.pose
        } else {
            pose
        };
        let dt = self.integration_parameters.dt.max(0.0001);
        state.previous_pose = previous_pose;
        state.pose = pose;
        state.linvel = if was_tracked {
            (pose.position - previous_pose.position) * (1.0 / dt)
        } else {
            vec3f(0.0, 0.0, 0.0)
        };
        state.tracked = true;
        state.gripping = gripping;
        state.shared_hand = shared_hand;
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
        depth_query_filter_key: u64,
    ) -> DepthQuerySurfaceCollider {
        let body = self.bodies.insert(RigidBodyBuilder::fixed().build());
        let collider = self.colliders.insert_with_parent(
            ColliderBuilder::new(SharedShape::halfspace(RapierVector::new(0.0, 1.0, 0.0)))
                .user_data(depth_query_surface_user_data(
                    depth_query_filter_key,
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
        depth_query_filter_key: u64,
        owner_body: RigidBodyHandle,
        query_body: RigidBodyHandle,
        query_radius: f32,
    ) -> usize {
        let surfaces =
            std::array::from_fn(|_| self.spawn_depth_query_surface(depth_query_filter_key));
        let index = self.depth_query_surface_sets.len();
        self.depth_query_surface_sets
            .push(DepthQueryBodySurfaceSet {
                depth_query_filter_key,
                owner_body,
                query_body,
                query_radius,
                surfaces,
                targets: [None; XR_DEPTH_QUERY_SURFACES_PER_PROBE],
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

    fn sync_linked_support_bodies(&mut self) {
        for index in 0..self.linked_support_bodies.len() {
            let support = self.linked_support_bodies[index];
            if self.vehicle_index_for_body(support.owner_body).is_some() {
                continue;
            }
            let owner_state = self
                .bodies
                .get(support.owner_body)
                .map(|owner| {
                    (
                        owner.is_enabled(),
                        owner.body_type(),
                        makepad_pose(owner.position()),
                    )
                })
                .unwrap_or((false, RigidBodyType::Dynamic, Pose::default()));
            let owner_enabled = owner_state.0;
            let support_pose = linked_support_world_pose(owner_state.2, support);
            if let Some(body) = self.bodies.get_mut(support.body) {
                if !owner_enabled {
                    body.set_enabled(false);
                    body.set_linvel(RapierVector::ZERO, false);
                    body.set_angvel(RapierVector::ZERO, false);
                    body.reset_forces(false);
                    body.reset_torques(false);
                } else {
                    body.set_enabled(true);
                    if body.body_type() != RigidBodyType::KinematicPositionBased {
                        body.set_body_type(RigidBodyType::KinematicPositionBased, false);
                    }
                    body.set_position(rapier_pose(support_pose), false);
                    body.set_next_kinematic_position(rapier_pose(support_pose));
                    body.set_linvel(RapierVector::ZERO, false);
                    body.set_angvel(RapierVector::ZERO, false);
                    body.reset_forces(false);
                    body.reset_torques(false);
                    body.wake_up(true);
                }
            }
            if let Some(collider) = self.colliders.get_mut(support.collider) {
                collider.set_enabled(false);
            }
        }
    }

    pub(crate) fn step(&mut self) {
        self.apply_held_body_targets();
        self.apply_shadow_body_targets();
        self.sync_linked_support_bodies();
        self.sync_vehicle_query_sources_pre_step();
        self.update_four_wheel_vehicles();
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
        self.sync_vehicle_support_bodies_post_step();
        self.acquire_hand_grabs();
        self.settle_resting_bodies();
        self.actively_driven_bodies.clear();
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
            if !shared_candidate && !self.cube_has_hand_contact(cube, slots) {
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

    fn cube_contact_colliders(
        &self,
        cube: &PhysicsCube,
    ) -> [Option<ColliderHandle>; XR_MAX_DEPTH_QUERY_KEYS_PER_CUBE] {
        let mut colliders = [None; XR_MAX_DEPTH_QUERY_KEYS_PER_CUBE];
        colliders[0] = Some(cube.collider);
        for (slot, support_index) in cube.linked_support_bodies.iter().flatten().enumerate() {
            colliders[slot + 1] = self
                .linked_support_bodies
                .get(*support_index)
                .map(|support| support.collider);
        }
        colliders
    }

    fn cube_depth_query_keys(
        &self,
        cube: PhysicsCube,
    ) -> [Option<u64>; XR_MAX_DEPTH_QUERY_KEYS_PER_CUBE] {
        let mut keys = [None; XR_MAX_DEPTH_QUERY_KEYS_PER_CUBE];
        keys[0] = cube
            .depth_query_surface_set
            .map(RapierScene::depth_query_key);
        for (slot, support_index) in cube.linked_support_bodies.iter().flatten().enumerate() {
            keys[slot + 1] = self
                .linked_support_bodies
                .get(*support_index)
                .and_then(|support| {
                    support
                        .depth_query_surface_set
                        .map(RapierScene::depth_query_key)
                });
        }
        keys
    }

    pub(super) fn cube_depth_query_sources(
        &self,
        cube: PhysicsCube,
    ) -> [Option<DepthQuerySource>; XR_MAX_DEPTH_QUERY_KEYS_PER_CUBE] {
        let mut sources = [None; XR_MAX_DEPTH_QUERY_KEYS_PER_CUBE];
        if let Some(set_index) = cube.depth_query_surface_set {
            sources[0] = Some(DepthQuerySource {
                set_index,
                body: cube.body,
                query_radius: cube.query_radius,
            });
        }
        for (slot, support_index) in cube.linked_support_bodies.iter().flatten().enumerate() {
            if let Some(support) = self.linked_support_bodies.get(*support_index) {
                if let Some(set_index) = support.depth_query_surface_set {
                    sources[slot + 1] = Some(DepthQuerySource {
                        set_index,
                        body: support.body,
                        query_radius: support.query_radius,
                    });
                }
            }
        }
        sources
    }

    fn sync_cube_linked_support_body_poses(
        &mut self,
        cube: PhysicsCube,
        owner_pose: Pose,
        linvel: Vec3f,
        angvel: Vec3f,
    ) {
        for support_index in cube.linked_support_bodies.iter().flatten() {
            let Some(mut support) = self.linked_support_bodies.get(*support_index).copied() else {
                continue;
            };
            let support_pose = linked_support_world_pose(owner_pose, support);
            support.suspension_length = support.rest_length;
            support.previous_suspension_length = support.rest_length;
            self.linked_support_bodies[*support_index] = support;
            if let Some(body) = self.bodies.get_mut(support.body) {
                body.set_enabled(true);
                body.set_body_type(RigidBodyType::KinematicPositionBased, false);
                body.set_position(rapier_pose(support_pose), false);
                body.set_next_kinematic_position(rapier_pose(support_pose));
                body.set_linvel(rapier_vec3(linvel), false);
                body.set_angvel(rapier_vec3(angvel), false);
                body.reset_forces(false);
                body.reset_torques(false);
                body.wake_up(true);
            }
        }
    }

    pub(crate) fn cube_linked_support_local_poses(
        &self,
        cube: PhysicsCube,
    ) -> [Option<Pose>; XR_MAX_LINKED_SUPPORT_BODIES_PER_CUBE] {
        let mut local_poses = [None; XR_MAX_LINKED_SUPPORT_BODIES_PER_CUBE];
        let Some(owner_body) = self.bodies.get(cube.body) else {
            return local_poses;
        };
        let owner_pose = makepad_pose(owner_body.position());
        let owner_inverse = owner_pose.invert();
        let inverse_scale = vec3f(
            if cube.scale.x.abs() > 1.0e-6 {
                1.0 / cube.scale.x
            } else {
                0.0
            },
            if cube.scale.y.abs() > 1.0e-6 {
                1.0 / cube.scale.y
            } else {
                0.0
            },
            if cube.scale.z.abs() > 1.0e-6 {
                1.0 / cube.scale.z
            } else {
                0.0
            },
        );
        for (slot, support_index) in cube.linked_support_bodies.iter().flatten().enumerate() {
            let Some(support) = self.linked_support_bodies.get(*support_index).copied() else {
                continue;
            };
            let Some(support_body) = self.bodies.get(support.body) else {
                continue;
            };
            if !support_body.is_enabled() {
                continue;
            }
            let support_pose = makepad_pose(support_body.position());
            let mut local_pose = Pose::multiply(&owner_inverse, &support_pose);
            local_pose.position = vec3f(
                local_pose.position.x * inverse_scale.x,
                local_pose.position.y * inverse_scale.y,
                local_pose.position.z * inverse_scale.z,
            );
            local_poses[slot] = Some(local_pose);
        }
        local_poses
    }

    pub(crate) fn cube_linked_support_spin_angles(
        &self,
        cube: PhysicsCube,
    ) -> [Option<f32>; XR_MAX_LINKED_SUPPORT_BODIES_PER_CUBE] {
        let mut spin_angles = [None; XR_MAX_LINKED_SUPPORT_BODIES_PER_CUBE];
        for (slot, support_index) in cube.linked_support_bodies.iter().flatten().enumerate() {
            spin_angles[slot] = self
                .linked_support_bodies
                .get(*support_index)
                .map(|support| support.spin_angle);
        }
        spin_angles
    }

    pub(crate) fn cube_linked_support_steer_angles(
        &self,
        cube: PhysicsCube,
    ) -> [Option<f32>; XR_MAX_LINKED_SUPPORT_BODIES_PER_CUBE] {
        let mut steer_angles = [None; XR_MAX_LINKED_SUPPORT_BODIES_PER_CUBE];
        for (slot, support_index) in cube.linked_support_bodies.iter().flatten().enumerate() {
            steer_angles[slot] = self
                .linked_support_bodies
                .get(*support_index)
                .map(|support| support.steer_angle);
        }
        steer_angles
    }

    fn disable_cube_linked_support_bodies(&mut self, cube: PhysicsCube) {
        for support_index in cube.linked_support_bodies.iter().flatten() {
            let Some(support) = self.linked_support_bodies.get(*support_index).copied() else {
                continue;
            };
            if let Some(body) = self.bodies.get_mut(support.body) {
                body.set_enabled(false);
                body.set_linvel(RapierVector::ZERO, false);
                body.set_angvel(RapierVector::ZERO, false);
                body.reset_forces(false);
                body.reset_torques(false);
            }
            if let Some(collider) = self.colliders.get_mut(support.collider) {
                collider.set_enabled(false);
            }
        }
    }

    fn cube_has_hand_contact(&self, cube: &PhysicsCube, slots: &[HandColliderBody]) -> bool {
        self.cube_contact_colliders(cube)
            .into_iter()
            .flatten()
            .any(|cube_collider| {
                self.narrow_phase
                    .contact_pairs_with(cube_collider)
                    .any(|pair| {
                        pair.has_any_active_contact()
                            && slots.iter().any(|slot| {
                                self.colliders
                                    .get(slot.collider)
                                    .map(|collider| collider.is_enabled())
                                    .unwrap_or(false)
                                    && (pair.collider1 == slot.collider
                                        || pair.collider2 == slot.collider)
                            })
                    })
            })
    }

    fn cube_settle_contact_state(&self, cube: &PhysicsCube) -> SettleContactState {
        let mut state = SettleContactState::default();
        for cube_collider in self.cube_contact_colliders(cube).into_iter().flatten() {
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
        }
        state
    }

    fn widget_uid_for_collider(&self, collider: ColliderHandle) -> Option<WidgetUid> {
        self.cubes
            .iter()
            .find(|cube| cube.collider == collider)
            .map(|cube| cube.widget_uid)
            .or_else(|| {
                self.linked_support_bodies
                    .iter()
                    .find(|support| support.collider == collider)
                    .map(|support| support.owner_widget_uid)
            })
    }

    pub(crate) fn snapshot_active_contacts(&self, contacts: &mut Vec<(WidgetUid, WidgetUid)>) {
        contacts.clear();
        for cube in &self.cubes {
            let Some(body) = self.bodies.get(cube.body) else {
                continue;
            };
            if !body.is_enabled() {
                continue;
            }
            for cube_collider in self.cube_contact_colliders(cube).into_iter().flatten() {
                for pair in self.narrow_phase.contact_pairs_with(cube_collider) {
                    if !pair.has_any_active_contact() {
                        continue;
                    }
                    let Some(other_uid) =
                        self.widget_uid_for_collider(if pair.collider1 == cube_collider {
                            pair.collider2
                        } else {
                            pair.collider1
                        })
                    else {
                        continue;
                    };
                    let a = cube.widget_uid;
                    let b = other_uid;
                    if a.0 < b.0 {
                        contacts.push((a, b));
                    }
                }
            }
        }
        contacts.sort_unstable_by_key(|(a, b)| (a.0, b.0));
        contacts.dedup();
    }

    fn release_settle_state(&mut self, body_handle: RigidBodyHandle) {
        self.settle_frame_counts.remove(&body_handle);
    }

    fn release_drive_state(&mut self, body_handle: RigidBodyHandle) {
        self.body_drive_motion.remove(&body_handle);
        self.actively_driven_bodies.remove(&body_handle);
    }

    fn release_body_into_dynamics(
        &mut self,
        body_handle: RigidBodyHandle,
        linvel: Vec3f,
        angvel: Vec3f,
    ) {
        self.shadow_body_motion.remove(&body_handle);
        self.release_drive_state(body_handle);
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
        self.release_drive_state(body_handle);
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
            let contact_state = self.cube_settle_contact_state(cube);
            if !contact_state.has_active_contact {
                to_reset.push(cube.body);
                continue;
            }
            if self.actively_driven_bodies.contains(&cube.body) {
                to_reset.push(cube.body);
                continue;
            }
            let has_hand_contact = self.cube_has_hand_contact(cube, &self.left_hand)
                || self.cube_has_hand_contact(cube, &self.right_hand);

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
        if cube.linked_support_bodies.iter().any(Option::is_some) {
            // Let Rapier consume the newly inserted suspension joints before we park the pooled body.
            self.step();
        }
        self.spawn_pool_cube_indices.push(cube_index);
        self.shadow_body_motion.remove(&cube.body);
        self.release_drive_state(cube.body);
        if let Some(body) = self.bodies.get_mut(cube.body) {
            body.set_enabled(false);
            body.set_linvel(RapierVector::ZERO, false);
            body.set_angvel(RapierVector::ZERO, false);
            body.reset_forces(false);
            body.reset_torques(false);
        }
        if let Some(collider) = self.colliders.get_mut(cube.collider) {
            collider.set_enabled(false);
        }
        self.disable_cube_linked_support_bodies(cube);
        self.sync_linked_support_bodies();
    }

    pub(crate) fn respawn_body(
        &mut self,
        widget_uid: WidgetUid,
        shadow: bool,
        mode: XrSharedObjectMode,
        pose: Pose,
        linvel: Vec3f,
        angvel: Vec3f,
    ) -> [Option<u64>; XR_MAX_DEPTH_QUERY_KEYS_PER_CUBE] {
        let cube = self
            .cubes
            .iter()
            .find(|cube| cube.widget_uid == widget_uid)
            .copied();
        let Some(cube) = cube else {
            return [None; XR_MAX_DEPTH_QUERY_KEYS_PER_CUBE];
        };
        self.clear_held_body(cube.body);
        self.shadow_body_motion.remove(&cube.body);
        self.release_drive_state(cube.body);
        self.release_settle_state(cube.body);
        if let Some(vehicle_index) = self.vehicle_index_for_body(cube.body) {
            let vehicle = &mut self.vehicles[vehicle_index];
            vehicle.current_steer = 0.0;
            vehicle.steer_input = 0.0;
            vehicle.throttle_input = 0.0;
            vehicle.brake_input = 0.0;
            vehicle.airtime = 0.0;
        }
        if let Some(body) = self.bodies.get_mut(cube.body) {
            body.set_enabled(true);
            body.set_position(rapier_pose(pose), false);
            if shadow {
                let remaining_prediction_seconds = if matches!(mode, XrSharedObjectMode::Sleeping) {
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
        if let Some(collider) = self.colliders.get_mut(cube.collider) {
            collider.set_enabled(true);
        }
        self.sync_cube_linked_support_body_poses(cube, pose, linvel, angvel);
        self.sync_linked_support_bodies();
        self.cube_depth_query_keys(cube)
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
        self.release_drive_state(cube.body);
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

    pub(crate) fn apply_wrench(
        &mut self,
        widget_uid: WidgetUid,
        force: Vec3f,
        torque: Vec3f,
    ) -> bool {
        let Some(cube) = self
            .cubes
            .iter()
            .find(|cube| cube.widget_uid == widget_uid)
            .copied()
        else {
            return false;
        };
        self.release_drive_state(cube.body);
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
        body.add_force(rapier_vec3(force), true);
        body.add_torque(rapier_vec3(torque), true);
        body.wake_up(true);
        true
    }

    pub(crate) fn apply_drive(
        &mut self,
        widget_uid: WidgetUid,
        target_linvel: Vec3f,
        target_angvel: Vec3f,
        max_linear_accel: f32,
        max_angular_accel: f32,
        preserve_vertical_linvel: bool,
        dt: f32,
    ) -> bool {
        let Some(cube) = self
            .cubes
            .iter()
            .find(|cube| cube.widget_uid == widget_uid)
            .copied()
        else {
            return false;
        };
        if self.vehicle_index_for_body(cube.body).is_some() {
            return false;
        }
        if self.held_by_for_body(cube.body).is_some() {
            return false;
        }
        self.release_settle_state(cube.body);
        if self.shadow_body_motion.contains_key(&cube.body) {
            return false;
        }
        let (physical_linvel, physical_angvel) = {
            let Some(body) = self.bodies.get(cube.body) else {
                return false;
            };
            if !body.is_enabled() || body.body_type() != RigidBodyType::Dynamic {
                return false;
            }
            let linvel = body.linvel();
            let angvel = body.angvel();
            (
                vec3f(linvel.x, linvel.y, linvel.z),
                vec3f(angvel.x, angvel.y, angvel.z),
            )
        };
        let dt = dt.max(1.0 / 480.0);
        let drive_motion =
            self.body_drive_motion
                .get(&cube.body)
                .copied()
                .unwrap_or(BodyDriveMotion {
                    linvel: physical_linvel,
                    angvel: physical_angvel,
                    max_linear_accel: max_linear_accel.max(0.0),
                    max_angular_accel: max_angular_accel.max(0.0),
                });
        let mut current_linvel = drive_motion.linvel;
        let current_angvel = drive_motion.angvel;

        let mut desired_linvel = target_linvel;
        if preserve_vertical_linvel {
            current_linvel.y = physical_linvel.y;
            desired_linvel.y = physical_linvel.y;
        }
        let linear_delta = desired_linvel - current_linvel;
        let angular_delta = target_angvel - current_angvel;
        let next_linvel =
            current_linvel + clamp_vec3_length(linear_delta, max_linear_accel.max(0.0) * dt);
        let next_angvel =
            current_angvel + clamp_vec3_length(angular_delta, max_angular_accel.max(0.0) * dt);

        let target_planar_speed = vec3f(target_linvel.x, 0.0, target_linvel.z).length();
        let next_planar_speed = vec3f(next_linvel.x, 0.0, next_linvel.z).length();
        let next_angular_speed = next_angvel.length();
        let drive_is_active =
            target_planar_speed > 0.001 || next_planar_speed > 0.001 || next_angular_speed > 0.001;
        if drive_is_active {
            self.body_drive_motion.insert(
                cube.body,
                BodyDriveMotion {
                    linvel: next_linvel,
                    angvel: next_angvel,
                    max_linear_accel: max_linear_accel.max(0.0),
                    max_angular_accel: max_angular_accel.max(0.0),
                },
            );
            self.actively_driven_bodies.insert(cube.body);
        } else {
            self.body_drive_motion.remove(&cube.body);
            self.actively_driven_bodies.remove(&cube.body);
        }
        let Some(body) = self.bodies.get_mut(cube.body) else {
            return false;
        };
        body.set_linvel(rapier_vec3(next_linvel), true);
        body.set_angvel(rapier_vec3(next_angvel), true);
        body.wake_up(true);
        true
    }

    pub(crate) fn despawn_body(
        &mut self,
        widget_uid: WidgetUid,
    ) -> [Option<u64>; XR_MAX_DEPTH_QUERY_KEYS_PER_CUBE] {
        let cube = self
            .cubes
            .iter()
            .find(|cube| cube.widget_uid == widget_uid)
            .copied();
        let Some(cube) = cube else {
            return [None; XR_MAX_DEPTH_QUERY_KEYS_PER_CUBE];
        };
        self.clear_held_body(cube.body);
        self.shadow_body_motion.remove(&cube.body);
        self.release_drive_state(cube.body);
        self.release_settle_state(cube.body);
        if let Some(body) = self.bodies.get_mut(cube.body) {
            body.set_enabled(false);
            body.set_linvel(RapierVector::ZERO, false);
            body.set_angvel(RapierVector::ZERO, false);
            body.reset_forces(false);
            body.reset_torques(false);
        }
        if let Some(collider) = self.colliders.get_mut(cube.collider) {
            collider.set_enabled(false);
        }
        self.disable_cube_linked_support_bodies(cube);
        self.sync_linked_support_bodies();
        self.cube_depth_query_keys(cube)
    }

    pub(super) fn sync_depth_query_surface_set(
        &mut self,
        set_index: usize,
        targets: &[Option<DepthQuerySurfaceTarget>; XR_DEPTH_QUERY_SURFACES_PER_PROBE],
    ) {
        let Some(surface_set) = self.depth_query_surface_sets.get_mut(set_index) else {
            return;
        };
        let query_body_enabled = self
            .bodies
            .get(surface_set.query_body)
            .map(|body| body.is_enabled())
            .unwrap_or(false);
        if self
            .shadow_body_motion
            .contains_key(&surface_set.owner_body)
            || !query_body_enabled
        {
            surface_set.targets = [None; XR_DEPTH_QUERY_SURFACES_PER_PROBE];
            for surface in &surface_set.surfaces {
                if let Some(collider) = self.colliders.get_mut(surface.collider) {
                    collider.set_enabled(false);
                }
            }
            return;
        }
        let body_position = self
            .bodies
            .get(surface_set.query_body)
            .map(|body| makepad_pose(body.position()).position)
            .unwrap_or(vec3f(0.0, 0.0, 0.0));
        let body_velocity = self
            .bodies
            .get(surface_set.query_body)
            .map(|body| {
                let linvel = body.linvel();
                vec3f(linvel.x, linvel.y, linvel.z)
            })
            .unwrap_or(vec3f(0.0, 0.0, 0.0));
        let physics_edge_margin = (surface_set.query_radius * 0.08).clamp(0.002, 0.008);
        for (index, (surface, target)) in surface_set
            .surfaces
            .iter_mut()
            .zip(targets.iter())
            .enumerate()
        {
            let Some(target) = target else {
                if let Some(collider) = self.colliders.get_mut(surface.collider) {
                    collider.set_enabled(false);
                }
                surface.fingerprint = 0;
                surface_set.targets[index] = None;
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
                    surface_set.depth_query_filter_key,
                    target.collider.role,
                );
                collider.set_restitution(target.collider.restitution.max(0.0));
                collider.set_enabled(supports_body);
                surface_set.targets[index] = supports_body.then_some(*target);
                if supports_body {
                    self.depth_query_stats.surface_count += 1;
                }
            }
        }
    }
}

#[cfg(test)]
include!("../tests/scene/xr_physics.rs");
