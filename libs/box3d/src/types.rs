// Port of box3d/include/box3d/types.h + src/types.c
// Definitions, events, query types, and geometry data types.
//
// Deviations from C (see PORTING.md):
// - `void* userData` -> u64
// - variable-length trailing data (hull/mesh/height field/compound) -> Vec fields
// - user-owned `const T*` shared geometry -> Arc<T>
// - task system / debug draw callbacks are not ported (serial, no debug draw yet)

use std::sync::Arc;

use crate::constants::MAX_MANIFOLD_POINTS;
use crate::core::SECRET_COOKIE;
use crate::core::get_length_units_per_meter;
use crate::id::{BodyId, ContactId, JointId, ShapeId};
use crate::math_functions::{
    Matrix3, Plane, Pos, Quat, Transform, Vec3, WorldTransform, AABB,
};

pub const DEFAULT_CATEGORY_BITS: u64 = u64::MAX;
pub const DEFAULT_MASK_BITS: u64 = u64::MAX;

/// Task interface (C: types.h b3TaskCallback/b3EnqueueTaskCallback/
/// b3FinishTaskCallback). Defined next to the scheduler; re-exported here to
/// mirror the C header layout.
pub use crate::scheduler::{EnqueueTaskCallback, FinishTaskCallback, TaskCallback};

/// Optional friction mixing callback.
pub type FrictionCallback = fn(friction_a: f32, user_material_id_a: u64, friction_b: f32, user_material_id_b: u64) -> f32;

/// Optional restitution mixing callback.
pub type RestitutionCallback =
    fn(restitution_a: f32, user_material_id_a: u64, restitution_b: f32, user_material_id_b: u64) -> f32;

/// Prototype for a contact filter callback. Return false to disable the collision.
pub type CustomFilterFcn = dyn FnMut(ShapeId, ShapeId) -> bool;

/// Prototype for a pre-solve callback. Return false to disable the contact this step.
pub type PreSolveFcn = dyn FnMut(ShapeId, ShapeId, Pos, Vec3) -> bool;

/// Optional world capacities that can be used to avoid run-time allocations.
#[derive(Clone, Copy, Debug, Default)]
pub struct Capacity {
    pub static_shape_count: i32,
    pub dynamic_shape_count: i32,
    pub static_body_count: i32,
    pub dynamic_body_count: i32,
    pub contact_count: i32,
}

/// World definition used to create a simulation world.
/// Must be initialized using default_world_def().
pub struct WorldDef {
    /// Gravity vector. Box3D has no up-vector defined.
    pub gravity: Vec3,

    /// Restitution speed threshold, usually in m/s.
    pub restitution_threshold: f32,

    /// Hit event speed threshold, usually in m/s.
    pub hit_event_threshold: f32,

    /// Contact stiffness. Cycles per second.
    pub contact_hertz: f32,

    /// Contact bounciness. Non-dimensional.
    pub contact_damping_ratio: f32,

    /// This parameter controls how fast overlap is resolved, usually meters per second.
    pub contact_speed: f32,

    /// Maximum linear speed. Usually meters per second.
    pub maximum_linear_speed: f32,

    /// Optional mixing callback for friction. The default uses sqrt(frictionA * frictionB).
    pub friction_callback: Option<FrictionCallback>,

    /// Optional mixing callback for restitution. The default uses max(restitutionA, restitutionB).
    pub restitution_callback: Option<RestitutionCallback>,

    /// Can bodies go to sleep to improve performance.
    pub enable_sleep: bool,

    /// Enable continuous collision.
    pub enable_continuous: bool,

    /// Number of workers to use with the provided task system. This is clamped
    /// to the range [1, MAX_WORKERS]. Using a value above 1 turns on
    /// multithreading. If task callbacks are provided then Box3D will use the
    /// user provided task system, otherwise it creates its internal scheduler.
    pub worker_count: u32,

    /// Function to spawn a task (C: b3WorldDef::enqueueTask). See
    /// EnqueueTaskCallback for the safety contract. Both enqueue_task and
    /// finish_task must be set (with worker_count > 0) to enable the external
    /// task system.
    pub enqueue_task: Option<EnqueueTaskCallback>,

    /// Function to finish a task (C: b3WorldDef::finishTask). Must block until
    /// the task completes.
    pub finish_task: Option<FinishTaskCallback>,

    /// User context provided to enqueue_task and finish_task
    /// (C: b3WorldDef::userTaskContext). Must stay valid for the world's
    /// lifetime. Note: this raw pointer makes WorldDef !Send/!Sync.
    pub user_task_context: *mut (),

    /// User data associated with a world.
    pub user_data: u64,

    /// Optional initial capacities.
    pub capacity: Capacity,

    /// Used internally to detect a valid definition. DO NOT SET.
    pub internal_value: i32,
}

/// Use this to initialize your world definition.
pub fn default_world_def() -> WorldDef {
    let length_units = get_length_units_per_meter();

    WorldDef {
        gravity: Vec3 { x: 0.0, y: -10.0, z: 0.0 },
        hit_event_threshold: 1.0 * length_units,
        restitution_threshold: 1.0 * length_units,
        contact_speed: 3.0 * length_units,
        contact_hertz: 30.0,
        contact_damping_ratio: 10.0,
        // 400 meters per second, faster than the speed of sound
        maximum_linear_speed: 400.0 * length_units,
        friction_callback: None,
        restitution_callback: None,
        enable_sleep: true,
        enable_continuous: true,
        worker_count: 0,
        enqueue_task: None,
        finish_task: None,
        user_task_context: std::ptr::null_mut(),
        user_data: 0,
        capacity: Capacity::default(),
        internal_value: SECRET_COOKIE,
    }
}

/// The body simulation type.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum BodyType {
    /// zero mass, zero velocity, may be manually moved
    #[default]
    Static = 0,
    /// zero mass, velocity set by user, moved by solver
    Kinematic = 1,
    /// positive mass, velocity determined by forces, moved by solver
    Dynamic = 2,
}

/// number of body types
pub const BODY_TYPE_COUNT: usize = 3;

/// Motion locks to restrict the body movement.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MotionLocks {
    pub linear_x: bool,
    pub linear_y: bool,
    pub linear_z: bool,
    pub angular_x: bool,
    pub angular_y: bool,
    pub angular_z: bool,
}

/// A body definition holds all the data needed to construct a rigid body.
/// Must be initialized using default_body_def().
#[derive(Clone, Debug)]
pub struct BodyDef {
    /// The body type: static, kinematic, or dynamic.
    pub body_type: BodyType,

    /// The initial world position of the body.
    pub position: Pos,

    /// The initial world rotation of the body.
    pub rotation: Quat,

    /// The initial linear velocity of the body's origin. Usually in meters per second.
    pub linear_velocity: Vec3,

    /// The initial angular velocity of the body. Radians per second.
    pub angular_velocity: Vec3,

    /// Linear damping is used to reduce the linear velocity.
    pub linear_damping: f32,

    /// Angular damping is used to reduce the angular velocity.
    pub angular_damping: f32,

    /// Scale the gravity applied to this body. Non-dimensional.
    pub gravity_scale: f32,

    /// Sleep speed threshold, default is 0.05 meters per second.
    pub sleep_threshold: f32,

    /// Optional body name for debugging.
    pub name: String,

    /// Use this to store application specific body data.
    pub user_data: u64,

    /// Motion locks to restrict linear and angular movement.
    pub motion_locks: MotionLocks,

    /// Set this flag to false if this body should never fall asleep.
    pub enable_sleep: bool,

    /// Is this body initially awake or sleeping?
    pub is_awake: bool,

    /// Treat this body as a high speed object that performs continuous collision detection
    /// against dynamic and kinematic bodies, but not other bullet bodies.
    pub is_bullet: bool,

    /// Used to disable a body. A disabled body does not move or collide.
    pub is_enabled: bool,

    /// This allows this body to bypass rotational speed limits.
    pub allow_fast_rotation: bool,

    /// Enable contact recycling. True by default.
    pub enable_contact_recycling: bool,

    /// Used internally to detect a valid definition. DO NOT SET.
    pub internal_value: i32,
}

/// Use this to initialize your body definition.
pub fn default_body_def() -> BodyDef {
    BodyDef {
        body_type: BodyType::Static,
        position: crate::math_functions::POS_ZERO,
        rotation: Quat::IDENTITY,
        linear_velocity: Vec3::ZERO,
        angular_velocity: Vec3::ZERO,
        linear_damping: 0.0,
        angular_damping: 0.0,
        gravity_scale: 1.0,
        sleep_threshold: 0.05 * get_length_units_per_meter(),
        name: String::new(),
        user_data: 0,
        motion_locks: MotionLocks::default(),
        enable_sleep: true,
        is_awake: true,
        is_bullet: false,
        is_enabled: true,
        allow_fast_rotation: false,
        enable_contact_recycling: true,
        internal_value: SECRET_COOKIE,
    }
}

/// This is used to filter collision on shapes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Filter {
    /// The collision category bits. Normally you would just set one bit.
    pub category_bits: u64,

    /// The collision mask bits. This states the categories that this
    /// shape would accept for collision.
    pub mask_bits: u64,

    /// Collision groups allow a certain group of objects to never collide (negative)
    /// or always collide (positive). Non-zero group filtering always wins against the mask bits.
    pub group_index: i32,
}

/// Use this to initialize your filter.
pub fn default_filter() -> Filter {
    Filter { category_bits: DEFAULT_CATEGORY_BITS, mask_bits: DEFAULT_MASK_BITS, group_index: 0 }
}

impl Default for Filter {
    fn default() -> Self {
        default_filter()
    }
}

/// Material properties supported per triangle on meshes and height fields.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct SurfaceMaterial {
    /// The Coulomb (dry) friction coefficient, usually in the range [0,1].
    pub friction: f32,

    /// The coefficient of restitution (bounce) usually in the range [0,1].
    pub restitution: f32,

    /// The rolling resistance usually in the range [0,1]. Spheres and capsules only.
    pub rolling_resistance: f32,

    /// The tangent velocity for conveyor belts.
    pub tangent_velocity: Vec3,

    /// User material identifier. Not used internally.
    pub user_material_id: u64,

    /// Custom debug draw color. Ignored if 0.
    pub custom_color: u32,
}

/// Use this to initialize your surface material.
pub fn default_surface_material() -> SurfaceMaterial {
    SurfaceMaterial { friction: 0.6, ..Default::default() }
}

/// Shape type.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ShapeType {
    /// A capsule is an extruded sphere
    Capsule = 0,
    /// A compound shape composed of up to 64K spheres, capsules, hulls, and meshes
    Compound = 1,
    /// A height field useful for terrain
    Height = 2,
    /// A convex hull
    Hull = 3,
    /// A triangle soup
    Mesh = 4,
    /// A sphere with an offset
    Sphere = 5,
}

/// The number of shape types.
pub const SHAPE_TYPE_COUNT: usize = 6;

/// Used to create a shape.
#[derive(Clone, Debug)]
pub struct ShapeDef {
    /// Use this to store application specific shape data.
    pub user_data: u64,

    /// Surface material used on mesh shapes per triangle. Ignored for convex/compound shapes.
    pub materials: Vec<SurfaceMaterial>,

    /// The base surface material. Ignored for compound shapes.
    pub base_material: SurfaceMaterial,

    /// The density, usually in kg/m^3.
    pub density: f32,

    /// Explosion scale for world_explode. Non-dimensional.
    pub explosion_scale: f32,

    /// Contact filtering data.
    pub filter: Filter,

    /// Enable custom filtering. Only one of the two shapes needs to enable custom filtering.
    pub enable_custom_filtering: bool,

    /// A sensor shape generates overlap events but never generates a collision response.
    pub is_sensor: bool,

    /// Enable sensor events for this shape. False by default, even for sensors.
    pub enable_sensor_events: bool,

    /// Enable contact events for this shape. Only kinematic and dynamic bodies. False by default.
    pub enable_contact_events: bool,

    /// Enable hit events for this shape. Only kinematic and dynamic bodies. False by default.
    pub enable_hit_events: bool,

    /// Enable pre-solve contact events for this shape. Only dynamic bodies.
    pub enable_pre_solve_events: bool,

    /// When shapes are created they will scan the environment for collision the next time step.
    pub invoke_contact_creation: bool,

    /// Should the body update the mass properties when this shape is created. Default is true.
    pub update_body_mass: bool,

    /// Used internally to detect a valid definition. DO NOT SET.
    pub internal_value: i32,
}

/// Use this to initialize your shape definition.
pub fn default_shape_def() -> ShapeDef {
    let length_units = get_length_units_per_meter();

    ShapeDef {
        user_data: 0,
        materials: Vec::new(),
        base_material: default_surface_material(),
        // density of water
        density: 1000.0 / (length_units * length_units * length_units),
        explosion_scale: 1.0,
        filter: default_filter(),
        enable_custom_filtering: false,
        is_sensor: false,
        enable_sensor_events: false,
        enable_contact_events: false,
        enable_hit_events: false,
        enable_pre_solve_events: false,
        invoke_contact_creation: true,
        update_body_mass: true,
        internal_value: SECRET_COOKIE,
    }
}

/// Profiling data. Times are in milliseconds.
#[derive(Clone, Copy, Debug, Default)]
pub struct Profile {
    pub step: f32,
    pub pairs: f32,
    pub collide: f32,
    pub solve: f32,
    pub solver_setup: f32,
    pub constraints: f32,
    pub prepare_constraints: f32,
    pub integrate_velocities: f32,
    pub warm_start: f32,
    pub solve_impulses: f32,
    pub integrate_positions: f32,
    pub relax_impulses: f32,
    pub apply_restitution: f32,
    pub store_impulses: f32,
    pub split_islands: f32,
    pub transforms: f32,
    pub sensor_hits: f32,
    pub joint_events: f32,
    pub hit_events: f32,
    pub refit: f32,
    pub bullets: f32,
    pub sleep_islands: f32,
    pub sensors: f32,
}

/// Counters that give details of the simulation size.
#[derive(Clone, Copy, Debug, Default)]
pub struct Counters {
    pub body_count: i32,
    pub shape_count: i32,
    pub contact_count: i32,
    pub joint_count: i32,
    pub island_count: i32,
    pub stack_used: i32,
    pub arena_capacity: i32,
    pub static_tree_height: i32,
    pub tree_height: i32,
    pub sat_call_count: i32,
    pub sat_cache_hit_count: i32,
    pub byte_count: i32,
    pub task_count: i32,
    pub color_counts: [i32; 24],
    pub manifold_counts: [i32; crate::constants::CONTACT_MANIFOLD_COUNT_BUCKETS],

    /// Number of contacts touched by the collide pass.
    pub awake_contact_count: i32,

    /// Number of contacts recycled in the most recent step.
    pub recycled_contact_count: i32,

    /// Maximum number of time of impact iterations.
    pub distance_iterations: i32,
    pub push_back_iterations: i32,
    pub root_iterations: i32,
}

/// Joint type enumeration.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum JointType {
    Parallel,
    Distance,
    Filter,
    Motor,
    Prismatic,
    Revolute,
    Spherical,
    Weld,
    Wheel,
}

/// Base joint definition used by all joint types.
#[derive(Clone, Copy, Debug)]
pub struct JointDef {
    /// User data pointer.
    pub user_data: u64,

    /// The first attached body.
    pub body_id_a: BodyId,

    /// The second attached body.
    pub body_id_b: BodyId,

    /// The first local joint frame.
    pub local_frame_a: Transform,

    /// The second local joint frame.
    pub local_frame_b: Transform,

    /// Force threshold for joint events.
    pub force_threshold: f32,

    /// Torque threshold for joint events.
    pub torque_threshold: f32,

    /// Constraint hertz (advanced feature).
    pub constraint_hertz: f32,

    /// Constraint damping ratio (advanced feature).
    pub constraint_damping_ratio: f32,

    /// Debug draw scale.
    pub draw_scale: f32,

    /// Set this flag to true if the attached bodies should collide.
    pub collide_connected: bool,

    /// Used internally to detect a valid definition. DO NOT SET.
    pub internal_value: i32,
}

/// Distance joint definition.
#[derive(Clone, Copy, Debug)]
pub struct DistanceJointDef {
    pub base: JointDef,
    /// The rest length of this joint. Clamped to a stable minimum value.
    pub length: f32,
    /// Enable the distance constraint to behave like a spring.
    pub enable_spring: bool,
    /// The lower spring force controls how much tension it can sustain.
    pub lower_spring_force: f32,
    /// The upper spring force controls how much compression it can sustain.
    pub upper_spring_force: f32,
    /// The spring linear stiffness Hertz, cycles per second.
    pub hertz: f32,
    /// The spring linear damping ratio, non-dimensional.
    pub damping_ratio: f32,
    /// Enable/disable the joint limit.
    pub enable_limit: bool,
    /// Minimum length. Clamped to a stable minimum value.
    pub min_length: f32,
    /// Maximum length. Must be greater than or equal to the minimum length.
    pub max_length: f32,
    /// Enable/disable the joint motor.
    pub enable_motor: bool,
    /// The maximum motor force, usually in newtons.
    pub max_motor_force: f32,
    /// The desired motor speed, usually in meters per second.
    pub motor_speed: f32,
}

/// A motor joint is used to control the relative position and velocity between two bodies.
#[derive(Clone, Copy, Debug)]
pub struct MotorJointDef {
    pub base: JointDef,
    /// The desired linear velocity.
    pub linear_velocity: Vec3,
    /// The maximum motor force in newtons.
    pub max_velocity_force: f32,
    /// The desired angular velocity.
    pub angular_velocity: Vec3,
    /// The maximum motor torque in newton-meters.
    pub max_velocity_torque: f32,
    /// Linear spring hertz for position control.
    pub linear_hertz: f32,
    /// Linear spring damping ratio.
    pub linear_damping_ratio: f32,
    /// Maximum spring force in newtons.
    pub max_spring_force: f32,
    /// Angular spring hertz for position control.
    pub angular_hertz: f32,
    /// Angular spring damping ratio.
    pub angular_damping_ratio: f32,
    /// Maximum spring torque in newton-meters.
    pub max_spring_torque: f32,
}

/// A filter joint is used to disable collision between two specific bodies.
#[derive(Clone, Copy, Debug)]
pub struct FilterJointDef {
    pub base: JointDef,
}

/// Parallel joint definition. Constrains the angle between axis z in body A and
/// axis z in body B using a spring. Useful to keep a body upright.
#[derive(Clone, Copy, Debug)]
pub struct ParallelJointDef {
    pub base: JointDef,
    /// The spring stiffness Hertz, cycles per second.
    pub hertz: f32,
    /// The spring damping ratio, non-dimensional.
    pub damping_ratio: f32,
    /// The maximum spring torque, typically in newton-meters.
    pub max_torque: f32,
}

/// Prismatic joint definition. Body B may slide along the x-axis in local frame A.
#[derive(Clone, Copy, Debug)]
pub struct PrismaticJointDef {
    pub base: JointDef,
    /// Enable a linear spring along the prismatic joint axis.
    pub enable_spring: bool,
    /// The spring stiffness Hertz, cycles per second.
    pub hertz: f32,
    /// The spring damping ratio, non-dimensional.
    pub damping_ratio: f32,
    /// The target translation for the joint in meters.
    pub target_translation: f32,
    /// Enable/disable the joint limit.
    pub enable_limit: bool,
    /// The lower translation limit.
    pub lower_translation: f32,
    /// The upper translation limit.
    pub upper_translation: f32,
    /// Enable/disable the joint motor.
    pub enable_motor: bool,
    /// The maximum motor force, typically in newtons.
    pub max_motor_force: f32,
    /// The desired motor speed, typically in meters per second.
    pub motor_speed: f32,
}

/// Revolute joint definition. A point on body B is fixed to a point on body A.
/// Allows relative rotation about the z-axis.
#[derive(Clone, Copy, Debug)]
pub struct RevoluteJointDef {
    pub base: JointDef,
    /// The bodyB angle minus bodyA angle in the reference state (radians).
    pub target_angle: f32,
    /// Enable a rotational spring on the revolute hinge axis.
    pub enable_spring: bool,
    /// The spring stiffness Hertz, cycles per second.
    pub hertz: f32,
    /// The spring damping ratio, non-dimensional.
    pub damping_ratio: f32,
    /// A flag to enable joint limits.
    pub enable_limit: bool,
    /// The lower angle for the joint limit in radians. Minimum of -0.99*pi radians.
    pub lower_angle: f32,
    /// The upper angle for the joint limit in radians. Maximum of 0.99*pi radians.
    pub upper_angle: f32,
    /// A flag to enable the joint motor.
    pub enable_motor: bool,
    /// The maximum motor torque, typically in newton-meters.
    pub max_motor_torque: f32,
    /// The desired motor speed in radians per second.
    pub motor_speed: f32,
}

/// Spherical joint definition. A point on body B is fixed to a point on body A.
#[derive(Clone, Copy, Debug)]
pub struct SphericalJointDef {
    pub base: JointDef,
    /// Enable a rotational spring that attempts to align the two joint frames.
    pub enable_spring: bool,
    /// The spring stiffness Hertz, cycles per second.
    pub hertz: f32,
    /// The spring damping ratio, non-dimensional.
    pub damping_ratio: f32,
    /// Target spring rotation, joint frame B relative to joint frame A.
    pub target_rotation: Quat,
    /// A flag to enable the cone limit. The cone is centered on the frameA z-axis.
    pub enable_cone_limit: bool,
    /// The angle for the cone limit in radians. Valid range is [0, pi].
    pub cone_angle: f32,
    /// A flag to enable the twist limit. The twist is centered on the frameB z-axis.
    pub enable_twist_limit: bool,
    /// The angle for the lower twist limit in radians.
    pub lower_twist_angle: f32,
    /// The angle for the upper twist limit in radians.
    pub upper_twist_angle: f32,
    /// A flag to enable the joint motor.
    pub enable_motor: bool,
    /// The maximum motor torque, typically in newton-meters.
    pub max_motor_torque: f32,
    /// The desired motor angular velocity in radians per second.
    pub motor_velocity: Vec3,
}

/// Weld joint definition. Connects two bodies together rigidly.
#[derive(Clone, Copy, Debug)]
pub struct WeldJointDef {
    pub base: JointDef,
    /// Linear stiffness expressed as Hertz. Use zero for maximum stiffness.
    pub linear_hertz: f32,
    /// Angular stiffness as Hertz. Use zero for maximum stiffness.
    pub angular_hertz: f32,
    /// Linear damping ratio, non-dimensional. Use 1 for critical damping.
    pub linear_damping_ratio: f32,
    /// Angular damping ratio, non-dimensional. Use 1 for critical damping.
    pub angular_damping_ratio: f32,
}

/// Wheel joint definition. Body A is the chassis and body B is the wheel.
#[derive(Clone, Copy, Debug)]
pub struct WheelJointDef {
    pub base: JointDef,
    /// Enable a linear spring along the local axis.
    pub enable_suspension_spring: bool,
    /// Spring stiffness in Hertz.
    pub suspension_hertz: f32,
    /// Spring damping ratio, non-dimensional.
    pub suspension_damping_ratio: f32,
    /// Enable/disable the joint linear limit.
    pub enable_suspension_limit: bool,
    /// The lower suspension translation limit.
    pub lower_suspension_limit: f32,
    /// The upper translation limit.
    pub upper_suspension_limit: f32,
    /// Enable/disable the joint rotational motor.
    pub enable_spin_motor: bool,
    /// The maximum motor torque, typically in newton-meters.
    pub max_spin_torque: f32,
    /// The desired motor speed in radians per second.
    pub spin_speed: f32,
    /// Enable steering, otherwise the steering is fixed forward.
    pub enable_steering: bool,
    /// Steering stiffness in Hertz.
    pub steering_hertz: f32,
    /// Spring damping ratio, non-dimensional.
    pub steering_damping_ratio: f32,
    /// The target steering angle in radians.
    pub target_steering_angle: f32,
    /// The maximum steering torque in N*m.
    pub max_steering_torque: f32,
    /// Enable/disable the steering angular limit.
    pub enable_steering_limit: bool,
    /// The lower steering angle in radians.
    pub lower_steering_limit: f32,
    /// The upper steering angle in radians.
    pub upper_steering_limit: f32,
}

/// The explosion definition is used to configure options for explosions.
#[derive(Clone, Copy, Debug)]
pub struct ExplosionDef {
    /// Mask bits to filter shapes.
    pub mask_bits: u64,
    /// The center of the explosion in world space.
    pub position: Pos,
    /// The radius of the explosion.
    pub radius: f32,
    /// The falloff distance beyond the radius. Impulse is reduced to zero at this distance.
    pub falloff: f32,
    /// Impulse per unit area.
    pub impulse_per_area: f32,
}

/// Use this to initialize your explosion definition.
/// (C: zero-init with default mask bits, see joint.c b3DefaultExplosionDef.)
pub fn default_explosion_def() -> ExplosionDef {
    ExplosionDef {
        mask_bits: DEFAULT_MASK_BITS,
        position: crate::math_functions::POS_ZERO,
        radius: 0.0,
        falloff: 0.0,
        impulse_per_area: 0.0,
    }
}

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

/// A begin-touch event is generated when a shape starts to overlap a sensor shape.
#[derive(Clone, Copy, Debug)]
pub struct SensorBeginTouchEvent {
    /// The id of the sensor shape.
    pub sensor_shape_id: ShapeId,
    /// The id of the shape that began touching the sensor shape.
    pub visitor_shape_id: ShapeId,
}

/// An end touch event is generated when a shape stops overlapping a sensor shape.
#[derive(Clone, Copy, Debug)]
pub struct SensorEndTouchEvent {
    /// The id of the sensor shape. May have been destroyed.
    pub sensor_shape_id: ShapeId,
    /// The id of the shape that stopped touching the sensor shape. May have been destroyed.
    pub visitor_shape_id: ShapeId,
}

/// Sensor events are buffered in the world and are available
/// as begin/end overlap event arrays after the time step is complete.
#[derive(Clone, Copy, Debug)]
pub struct SensorEvents<'a> {
    /// Array of sensor begin touch events.
    pub begin_events: &'a [SensorBeginTouchEvent],
    /// Array of sensor end touch events.
    pub end_events: &'a [SensorEndTouchEvent],
}

/// A begin-touch event is generated when two shapes begin touching.
#[derive(Clone, Copy, Debug)]
pub struct ContactBeginTouchEvent {
    /// Id of the first shape.
    pub shape_id_a: ShapeId,
    /// Id of the second shape.
    pub shape_id_b: ShapeId,
    /// The transient contact id.
    pub contact_id: ContactId,
}

/// An end touch event is generated when two shapes stop touching.
#[derive(Clone, Copy, Debug)]
pub struct ContactEndTouchEvent {
    /// Id of the first shape. May have been destroyed.
    pub shape_id_a: ShapeId,
    /// Id of the second shape. May have been destroyed.
    pub shape_id_b: ShapeId,
    /// Id of the contact. May have been destroyed.
    pub contact_id: ContactId,
}

/// A hit touch event is generated when two shapes collide with a speed faster
/// than the hit speed threshold.
#[derive(Clone, Copy, Debug)]
pub struct ContactHitEvent {
    /// Id of the first shape.
    pub shape_id_a: ShapeId,
    /// Id of the second shape.
    pub shape_id_b: ShapeId,
    /// Id of the contact. May have been destroyed.
    pub contact_id: ContactId,
    /// Point where the shapes hit at the beginning of the time step.
    pub point: Pos,
    /// Normal vector pointing from shape A to shape B.
    pub normal: Vec3,
    /// The speed the shapes are approaching. Always positive.
    pub approach_speed: f32,
    /// User material on shape A.
    pub user_material_id_a: u64,
    /// User material on shape B.
    pub user_material_id_b: u64,
}

/// Contact events are buffered in the world and are available
/// as event arrays after the time step is complete.
#[derive(Clone, Copy, Debug)]
pub struct ContactEvents<'a> {
    /// Array of begin touch events.
    pub begin_events: &'a [ContactBeginTouchEvent],
    /// Array of end touch events.
    pub end_events: &'a [ContactEndTouchEvent],
    /// Array of hit events.
    pub hit_events: &'a [ContactHitEvent],
}

/// Body move events triggered when a body moves.
#[derive(Clone, Copy, Debug)]
pub struct BodyMoveEvent {
    /// The body user data.
    pub user_data: u64,
    /// The body transform.
    pub transform: WorldTransform,
    /// The body id.
    pub body_id: BodyId,
    /// Did the body fall asleep this time step?
    pub fell_asleep: bool,
}

/// Body events are buffered in the world and are available
/// as event arrays after the time step is complete.
#[derive(Clone, Copy, Debug)]
pub struct BodyEvents<'a> {
    /// Array of move events.
    pub move_events: &'a [BodyMoveEvent],
}

/// Joint events report joints that are awake and have a force and/or torque
/// exceeding the threshold.
#[derive(Clone, Copy, Debug)]
pub struct JointEvent {
    /// The joint id.
    pub joint_id: JointId,
    /// The user data from the joint for convenience.
    pub user_data: u64,
}

/// Joint events are buffered in the world and are available
/// as event arrays after the time step is complete.
#[derive(Clone, Copy, Debug)]
pub struct JointEvents<'a> {
    /// Array of events.
    pub joint_events: &'a [JointEvent],
}

/// The contact data for two shapes.
#[derive(Clone, Copy, Debug)]
pub struct ContactData<'a> {
    /// The contact id.
    pub contact_id: ContactId,
    /// The first shape id.
    pub shape_id_a: ShapeId,
    /// The second shape id.
    pub shape_id_b: ShapeId,
    /// The contact manifolds.
    pub manifolds: &'a [Manifold],
}

// ---------------------------------------------------------------------------
// Query
// ---------------------------------------------------------------------------

/// The query filter is used to filter collisions between queries and shapes.
#[derive(Clone, Copy, Debug)]
pub struct QueryFilter {
    /// The collision category bits of this query. Normally you would just set one bit.
    pub category_bits: u64,

    /// The collision mask bits.
    pub mask_bits: u64,

    /// Optional id to identify this query in a recording. Ignored when not recording.
    pub id: u64,

    /// Optional label to identify this query in a recording. Ignored when not recording.
    pub name: &'static str,
}

/// Use this to initialize your query filter.
pub fn default_query_filter() -> QueryFilter {
    QueryFilter { category_bits: DEFAULT_CATEGORY_BITS, mask_bits: DEFAULT_MASK_BITS, id: 0, name: "" }
}

/// Low level ray cast input data.
#[derive(Clone, Copy, Debug, Default)]
pub struct RayCastInput {
    /// Start point of the ray cast.
    pub origin: Vec3,

    /// Translation of the ray cast. end = start + translation.
    pub translation: Vec3,

    /// The maximum fraction of the translation to consider, typically 1.
    pub max_fraction: f32,
}

/// Result from world_cast_ray_closest.
#[derive(Clone, Copy, Debug, Default)]
pub struct RayResult {
    /// The shape hit.
    pub shape_id: ShapeId,
    /// The world point of the hit.
    pub point: Pos,
    /// The world normal of the shape surface at the hit point.
    pub normal: Vec3,
    /// The user material id at the hit point.
    pub user_material_id: u64,
    /// The fraction of the input ray.
    pub fraction: f32,
    /// The triangle index if the shape is a mesh, height-field, or compound with child mesh.
    pub triangle_index: i32,
    /// The child index if the shape is a compound.
    pub child_index: i32,
    /// The number of BVH nodes visited. Diagnostic.
    pub node_visits: i32,
    /// The number of BVH leaves visited. Diagnostic.
    pub leaf_visits: i32,
    /// Did the ray hit? If false, all other data is invalid.
    pub hit: bool,
}

/// A shape proxy is used by the GJK algorithm. It can represent a convex shape.
#[derive(Clone, Copy, Debug)]
pub struct ShapeProxy<'a> {
    /// The point cloud.
    pub points: &'a [Vec3],

    /// The external radius of the point cloud.
    pub radius: f32,
}

impl<'a> ShapeProxy<'a> {
    #[inline]
    pub fn count(&self) -> i32 {
        self.points.len() as i32
    }
}

/// Low level shape cast input in generic form.
#[derive(Clone, Copy, Debug)]
pub struct ShapeCastInput<'a> {
    /// A generic query shape.
    pub proxy: ShapeProxy<'a>,

    /// The translation of the shape cast.
    pub translation: Vec3,

    /// The maximum fraction of the translation to consider, typically 1.
    pub max_fraction: f32,

    /// Allow shape cast to encroach when initially touching.
    /// This only works if the radius is greater than zero.
    pub can_encroach: bool,
}

/// Input for sweeping an AABB through a dynamic tree.
#[derive(Clone, Copy, Debug, Default)]
pub struct BoxCastInput {
    /// The AABB to cast, in the tree's frame.
    pub box_: AABB,

    /// The sweep translation.
    pub translation: Vec3,

    /// The maximum fraction of the translation to consider, typically 1.
    pub max_fraction: f32,
}

/// Low level ray cast or shape-cast output data.
#[derive(Clone, Copy, Debug)]
pub struct CastOutput {
    /// The surface normal at the hit point.
    pub normal: Vec3,
    /// The surface hit point.
    pub point: Vec3,
    /// The fraction of the input translation at collision.
    pub fraction: f32,
    /// The number of iterations used.
    pub iterations: i32,
    /// The index of the mesh or height field triangle hit.
    pub triangle_index: i32,
    /// The index of the compound child shape.
    pub child_index: i32,
    /// The material index. May be -1 for null.
    pub material_index: i32,
    /// Did the cast hit?
    pub hit: bool,
}

impl Default for CastOutput {
    fn default() -> Self {
        CastOutput {
            normal: Vec3::ZERO,
            point: Vec3::ZERO,
            fraction: 0.0,
            iterations: 0,
            triangle_index: 0,
            child_index: 0,
            material_index: 0,
            hit: false,
        }
    }
}

/// Same type in single precision.
#[cfg(not(feature = "double-precision"))]
pub type WorldCastOutput = CastOutput;

/// Ray cast or shape-cast output in world space. The hit point is a world position so the
/// result stays precise far from the world origin. Mirrors CastOutput with a double
/// precision point.
#[cfg(feature = "double-precision")]
#[derive(Clone, Copy, Debug, Default)]
pub struct WorldCastOutput {
    /// The surface normal at the hit point.
    pub normal: Vec3,
    /// The surface hit point in world space.
    pub point: Pos,
    /// The fraction of the input translation at collision.
    pub fraction: f32,
    /// The number of iterations used.
    pub iterations: i32,
    /// The index of the mesh or height field triangle hit.
    pub triangle_index: i32,
    /// The index of the compound child shape.
    pub child_index: i32,
    /// The material index. May be -1 for null.
    pub material_index: i32,
    /// Did the cast hit?
    pub hit: bool,
}

/// Body cast result for ray and shape casts.
#[derive(Clone, Copy, Debug, Default)]
pub struct BodyCastResult {
    /// The shape hit.
    pub shape_id: ShapeId,
    /// The world point on the shape surface.
    pub point: Pos,
    /// The world normal vector on the shape surface.
    pub normal: Vec3,
    /// The fraction along the ray hit.
    pub fraction: f32,
    /// The triangle index if the shape is a mesh or height-field.
    pub triangle_index: i32,
    /// The user material id at the hit point.
    pub user_material_id: u64,
    /// The number of iterations used. Diagnostic.
    pub iterations: i32,
    /// Did the cast hit? If false, all other fields are invalid.
    pub hit: bool,
}

/// Used to warm start the GJK simplex. Zero initialize this structure for each call.
#[derive(Clone, Copy, Debug, Default)]
pub struct SimplexCache {
    /// Value used to compare length, area, volume of two simplexes.
    pub metric: f32,

    /// The number of stored simplex points.
    pub count: u16,

    /// The cached simplex indices on shape A.
    pub index_a: [u8; 4],

    /// The cached simplex indices on shape B.
    pub index_b: [u8; 4],
}

pub const EMPTY_DISTANCE_CACHE: SimplexCache =
    SimplexCache { metric: 0.0, count: 0, index_a: [0; 4], index_b: [0; 4] };

/// Input parameters for shape_cast.
#[derive(Clone, Copy, Debug)]
pub struct ShapeCastPairInput<'a> {
    /// The proxy for shape A.
    pub proxy_a: ShapeProxy<'a>,
    /// The proxy for shape B.
    pub proxy_b: ShapeProxy<'a>,
    /// Transform of shape B in shape A's frame, the relative pose B in A.
    pub transform: Transform,
    /// The translation of shape B, in A's frame.
    pub translation_b: Vec3,
    /// The fraction of the translation to consider, typically 1.
    pub max_fraction: f32,
    /// Allows shapes with a radius to move slightly closer if already touching.
    pub can_encroach: bool,
}

/// Input for shape_distance.
#[derive(Clone, Copy, Debug)]
pub struct DistanceInput<'a> {
    /// The proxy for shape A.
    pub proxy_a: ShapeProxy<'a>,

    /// The proxy for shape B.
    pub proxy_b: ShapeProxy<'a>,

    /// Transform of shape B in shape A's frame, the relative pose B in A.
    pub transform: Transform,

    /// Should the proxy radius be considered?
    pub use_radii: bool,
}

/// Output for shape_distance.
#[derive(Clone, Copy, Debug, Default)]
pub struct DistanceOutput {
    /// Closest point on shapeA, in shape A's frame.
    pub point_a: Vec3,
    /// Closest point on shapeB, in shape A's frame.
    pub point_b: Vec3,
    /// A to B normal in shape A's frame. Invalid if distance is zero.
    pub normal: Vec3,
    /// The final distance, zero if overlapped.
    pub distance: f32,
    /// Number of GJK iterations used.
    pub iterations: i32,
    /// The number of simplexes stored in the simplex array.
    pub simplex_count: i32,
}

/// Simplex vertex for debugging the GJK algorithm.
#[derive(Clone, Copy, Debug, Default)]
pub struct SimplexVertex {
    /// support point in proxyA
    pub w_a: Vec3,
    /// support point in proxyB
    pub w_b: Vec3,
    /// wB - wA
    pub w: Vec3,
    /// barycentric coordinate
    pub a: f32,
    /// wA index
    pub index_a: i32,
    /// wB index
    pub index_b: i32,
}

/// Simplex from the GJK algorithm.
#[derive(Clone, Copy, Debug, Default)]
pub struct Simplex {
    /// vertices
    pub vertices: [SimplexVertex; 4],
    /// number of valid vertices
    pub count: i32,
}

/// This describes the motion of a body/shape for TOI computation.
#[derive(Clone, Copy, Debug, Default)]
pub struct Sweep {
    /// Local center of mass position.
    pub local_center: Vec3,
    /// Starting center of mass world position.
    pub c1: Vec3,
    /// Ending center of mass world position.
    pub c2: Vec3,
    /// Starting world rotation.
    pub q1: Quat,
    /// Ending world rotation.
    pub q2: Quat,
}

/// Time of impact input.
#[derive(Clone, Copy, Debug)]
pub struct TOIInput<'a> {
    /// The proxy for shape A.
    pub proxy_a: ShapeProxy<'a>,
    /// The proxy for shape B.
    pub proxy_b: ShapeProxy<'a>,
    /// The movement of shape A.
    pub sweep_a: Sweep,
    /// The movement of shape B.
    pub sweep_b: Sweep,
    /// Defines the sweep interval [0, tMax].
    pub max_fraction: f32,
}

/// Describes the TOI output.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum TOIState {
    #[default]
    Unknown,
    Failed,
    Overlapped,
    Hit,
    Separated,
}

/// Time of impact output.
#[derive(Clone, Copy, Debug, Default)]
pub struct TOIOutput {
    /// The type of result.
    pub state: TOIState,
    /// The hit point.
    pub point: Vec3,
    /// The hit normal.
    pub normal: Vec3,
    /// The sweep time of the collision.
    pub fraction: f32,
    /// The final distance.
    pub distance: f32,
    /// Number of outer iterations.
    pub distance_iterations: i32,
    /// Total number of push back iterations.
    pub push_back_iterations: i32,
    /// Total number of root iterations.
    pub root_iterations: i32,
    /// Indicates that the time of impact detected initial overlap and used a
    /// fallback sphere as a last ditch effort to prevent tunneling.
    pub used_fallback: bool,
}

// ---------------------------------------------------------------------------
// Dynamic Tree
// ---------------------------------------------------------------------------

/// Flags for tree nodes. For internal usage.
pub const ALLOCATED_NODE: u16 = 0x0001;
pub const ENLARGED_NODE: u16 = 0x0002;
pub const LEAF_NODE: u16 = 0x0004;

/// Tree node child indices. For internal usage.
#[derive(Clone, Copy, Debug, Default)]
pub struct TreeNodeChildren {
    pub child1: i32,
    pub child2: i32,
}

/// A node in the dynamic tree.
/// The C version overlays `children` (internal nodes) and `userData` (leaves) in
/// a union, and `parent`/`next` in another; the port stores them separately.
#[derive(Clone, Copy, Debug, Default)]
pub struct TreeNode {
    /// The node bounding box.
    pub aabb: AABB,

    /// Category bits for collision filtering.
    pub category_bits: u64,

    /// Children (internal node).
    pub children: TreeNodeChildren,

    /// User data (leaf node).
    pub user_data: u64,

    /// The node parent index (allocated node) or freelist next index (free node).
    pub parent: i32,

    /// Height of the node. Leaves have a height of 0.
    pub height: u16,

    /// See tree node flags above.
    pub flags: u16,
}

/// Dynamic tree version for compatibility testing.
pub const DYNAMIC_TREE_VERSION: u64 = 0x93EDAF889FD30B4A;

/// The dynamic tree structure. This should be considered private data.
#[derive(Clone, Debug, Default)]
pub struct DynamicTree {
    /// The tree nodes.
    pub nodes: Vec<TreeNode>,

    /// The root index.
    pub root: i32,

    /// The number of nodes.
    pub node_count: i32,

    /// The allocated node space.
    pub node_capacity: i32,

    /// Number of proxies created.
    pub proxy_count: i32,

    /// Node free list.
    pub free_list: i32,

    /// Leaf indices for rebuild.
    pub leaf_indices: Vec<i32>,

    /// Leaf bounding boxes for rebuild.
    pub leaf_boxes: Vec<AABB>,

    /// Leaf bounding box centers for rebuild.
    pub leaf_centers: Vec<Vec3>,

    /// Bins for sorting during rebuild.
    pub bin_indices: Vec<i32>,

    /// Allocated space for rebuilding.
    pub rebuild_capacity: i32,
}

/// These are performance results returned by dynamic tree queries.
#[derive(Clone, Copy, Debug, Default)]
pub struct TreeStats {
    /// Number of internal nodes visited during the query.
    pub node_visits: i32,
    /// Number of leaf nodes visited during the query.
    pub leaf_visits: i32,
}

// ---------------------------------------------------------------------------
// Character Mover
// ---------------------------------------------------------------------------

/// The plane between a character mover and a shape.
#[derive(Clone, Copy, Debug, Default)]
pub struct PlaneResult {
    /// Outward pointing plane.
    pub plane: Plane,
    /// Closest point on the shape. May not be unique.
    pub point: Vec3,
}

/// These are collision planes that can be fed to solve_planes.
#[derive(Clone, Copy, Debug, Default)]
pub struct CollisionPlane {
    /// The collision plane between the mover and some shape.
    pub plane: Plane,
    /// Setting this to FLT_MAX makes the plane as rigid as possible.
    pub push_limit: f32,
    /// The push on the mover determined by solve_planes. Usually in meters.
    pub push: f32,
    /// Indicates if clip_vector should clip against this plane.
    pub clip_velocity: bool,
}

/// Result returned by solve_planes.
#[derive(Clone, Copy, Debug, Default)]
pub struct PlaneSolverResult {
    /// The final relative translation.
    pub delta: Vec3,
    /// The number of iterations used by the plane solver. For diagnostics.
    pub iteration_count: i32,
}

/// Body plane result for movers.
#[derive(Clone, Copy, Debug, Default)]
pub struct BodyPlaneResult {
    /// The shape id on the body.
    pub shape_id: ShapeId,
    /// The plane result.
    pub result: PlaneResult,
}

// ---------------------------------------------------------------------------
// Geometry
// ---------------------------------------------------------------------------

/// This holds the mass data computed for a shape.
#[derive(Clone, Copy, Debug, Default)]
pub struct MassData {
    /// The shape mass.
    pub mass: f32,
    /// The local center of mass position.
    pub center: Vec3,
    /// The inertia tensor about the shape center of mass.
    pub inertia: Matrix3,
}

/// A solid sphere.
#[derive(Clone, Copy, Debug, Default)]
pub struct Sphere {
    /// The local center.
    pub center: Vec3,
    /// The radius.
    pub radius: f32,
}

/// A solid capsule can be viewed as two hemispheres connected by a rectangle.
#[derive(Clone, Copy, Debug, Default)]
pub struct Capsule {
    /// Local center of the first hemisphere.
    pub center1: Vec3,
    /// Local center of the second hemisphere.
    pub center2: Vec3,
    /// The radius of the hemispheres.
    pub radius: f32,
}

/// A hull vertex. Identified by a half-edge with this vertex as its tail.
#[derive(Clone, Copy, Debug, Default)]
pub struct HullVertex {
    /// A half-edge that has this vertex as the origin.
    pub edge: u8,
}

/// Half-edge for hull data structure.
#[derive(Clone, Copy, Debug, Default)]
pub struct HullHalfEdge {
    /// Next edge index CCW.
    pub next: u8,
    /// Twin edge index.
    pub twin: u8,
    /// Index of origin vertex and point.
    pub origin: u8,
    /// Face to the left of this edge.
    pub face: u8,
}

/// A hull face. Hulls use a half-edge data structure, so a face
/// can be determined from a single half-edge index.
#[derive(Clone, Copy, Debug, Default)]
pub struct HullFace {
    /// An arbitrary half-edge on this face.
    pub edge: u8,
}

/// A convex hull.
/// The C version is a single allocation with trailing arrays accessed through
/// byte offsets; the port uses plain Vec fields.
#[derive(Clone, Debug, Default)]
pub struct HullData {
    /// Hash of this hull (zero when not yet computed).
    pub hash: u32,

    /// Axis-aligned box in local space.
    pub aabb: AABB,

    /// Surface area, typically in squared meters.
    pub surface_area: f32,

    /// Volume, typically in m^3.
    pub volume: f32,

    /// The radius of the largest sphere at the center.
    pub inner_radius: f32,

    /// The local centroid.
    pub center: Vec3,

    /// The inertia tensor about the centroid.
    pub central_inertia: Matrix3,

    /// The hull vertices (one half-edge index each).
    pub vertices: Vec<HullVertex>,

    /// The vertex positions.
    pub points: Vec<Vec3>,

    /// The half-edges (double the edge count).
    pub edges: Vec<HullHalfEdge>,

    /// The faces. Hull faces are convex polygons.
    pub faces: Vec<HullFace>,

    /// The face planes.
    pub planes: Vec<Plane>,
}

impl HullData {
    #[inline]
    pub fn vertex_count(&self) -> i32 {
        self.vertices.len() as i32
    }
    #[inline]
    pub fn edge_count(&self) -> i32 {
        self.edges.len() as i32
    }
    #[inline]
    pub fn face_count(&self) -> i32 {
        self.faces.len() as i32
    }
}

/// This is used to create a re-usable collision mesh.
#[derive(Clone, Copy, Debug, Default)]
pub struct MeshDef<'a> {
    /// Triangle vertices.
    pub vertices: &'a [Vec3],

    /// Triangle vertex indices. 3 for each triangle.
    pub indices: &'a [i32],

    /// Triangle material index. 1 per triangle. May be empty.
    pub material_indices: &'a [u8],

    /// Tolerance for vertex welding in length units.
    pub weld_tolerance: f32,

    /// Optionally weld nearby vertices.
    pub weld_vertices: bool,

    /// Use the median split instead of SAH to speed up mesh creation.
    pub use_median_split: bool,

    /// Compute triangle adjacency information using shared edges.
    pub identify_edges: bool,
}

/// Triangle mesh edge flags.
pub const CONCAVE_EDGE1: i32 = 0x01;
pub const CONCAVE_EDGE2: i32 = 0x02;
pub const CONCAVE_EDGE3: i32 = 0x04;
pub const INVERSE_CONCAVE_EDGE1: i32 = 0x10;
pub const INVERSE_CONCAVE_EDGE2: i32 = 0x20;
pub const INVERSE_CONCAVE_EDGE3: i32 = 0x40;
pub const ALL_CONCAVE_EDGES: i32 = CONCAVE_EDGE1 | CONCAVE_EDGE2 | CONCAVE_EDGE3;
pub const FLAT_EDGE1: i32 = CONCAVE_EDGE1 | INVERSE_CONCAVE_EDGE1;
pub const FLAT_EDGE2: i32 = CONCAVE_EDGE2 | INVERSE_CONCAVE_EDGE2;
pub const FLAT_EDGE3: i32 = CONCAVE_EDGE3 | INVERSE_CONCAVE_EDGE3;
pub const ALL_FLAT_EDGES: i32 = FLAT_EDGE1 | FLAT_EDGE2 | FLAT_EDGE3;

/// A mesh triangle.
#[derive(Clone, Copy, Debug, Default)]
pub struct MeshTriangle {
    /// Index of vertex 1.
    pub index1: i32,
    /// Index of vertex 2.
    pub index2: i32,
    /// Index of vertex 3.
    pub index3: i32,
}

/// A mesh BVH node. The C version packs (axis|type):2 and
/// (childOffset|triangleCount):30 into a bitfield union; the port keeps the
/// raw u32 with accessors.
#[derive(Clone, Copy, Debug, Default)]
pub struct MeshNode {
    /// The lower bound of the node AABB.
    pub lower_bound: Vec3,

    /// Packed: low 2 bits = split axis (internal) or 3 (leaf);
    /// high 30 bits = child offset (internal) or triangle count (leaf).
    pub data: u32,

    /// The upper bound of the node AABB.
    pub upper_bound: Vec3,

    /// The index of the leaf triangles.
    pub triangle_offset: u32,
}

pub const MESH_NODE_LEAF: u32 = 3;

impl MeshNode {
    #[inline]
    pub fn is_leaf(&self) -> bool {
        (self.data & 3) == MESH_NODE_LEAF
    }
    #[inline]
    pub fn axis(&self) -> u32 {
        self.data & 3
    }
    #[inline]
    pub fn child_offset(&self) -> u32 {
        self.data >> 2
    }
    #[inline]
    pub fn triangle_count(&self) -> u32 {
        self.data >> 2
    }
    #[inline]
    pub fn pack_node(axis: u32, child_offset: u32) -> u32 {
        (axis & 3) | (child_offset << 2)
    }
    #[inline]
    pub fn pack_leaf(triangle_count: u32) -> u32 {
        MESH_NODE_LEAF | (triangle_count << 2)
    }
}

/// This is a sorted triangle collision bounding volume hierarchy.
/// The C version is a single allocation with trailing arrays; the port uses Vecs.
#[derive(Clone, Debug, Default)]
pub struct MeshData {
    /// Hash of this mesh (zero when not yet computed).
    pub hash: u32,

    /// Local axis-aligned box.
    pub bounds: AABB,

    /// Combined surface area of all triangles. Single-sided.
    pub surface_area: f32,

    /// The height of the bounding volume hierarchy.
    pub tree_height: i32,

    /// The number of degenerate triangles. Diagnostic.
    pub degenerate_count: i32,

    /// The BVH nodes.
    pub nodes: Vec<MeshNode>,

    /// The vertices.
    pub vertices: Vec<Vec3>,

    /// The triangles.
    pub triangles: Vec<MeshTriangle>,

    /// Per-triangle material indices. May be empty.
    pub materials: Vec<u8>,

    /// Per-triangle edge flags.
    pub flags: Vec<u8>,
}

impl MeshData {
    #[inline]
    pub fn vertex_count(&self) -> i32 {
        self.vertices.len() as i32
    }
    #[inline]
    pub fn triangle_count(&self) -> i32 {
        self.triangles.len() as i32
    }
    #[inline]
    pub fn node_count(&self) -> i32 {
        self.nodes.len() as i32
    }
    #[inline]
    pub fn material_count(&self) -> i32 {
        self.materials.len() as i32
    }
}

/// This allows mesh data to be re-used with different scales.
#[derive(Clone, Debug)]
pub struct Mesh {
    /// Immutable shared mesh data.
    pub data: Arc<MeshData>,

    /// This scale may be non-uniform and have negative components. However,
    /// no component may be very small in magnitude.
    pub scale: Vec3,
}

/// Data used to create a height field.
#[derive(Clone, Copy, Debug, Default)]
pub struct HeightFieldDef<'a> {
    /// Grid point heights. count = countX * countZ.
    pub heights: &'a [f32],

    /// Grid cell material. A value of 0xFF is reserved for holes.
    /// count = (countX - 1) * (countZ - 1). May be empty.
    pub material_indices: &'a [u8],

    /// The height field scale. All components must be positive values.
    pub scale: Vec3,

    /// The number of grid lines along the x-axis.
    pub count_x: i32,

    /// The number of grid lines along the z-axis.
    pub count_z: i32,

    /// Global minimum and maximum heights used for quantization (unscaled space).
    pub global_minimum_height: f32,

    /// The maximum.
    pub global_maximum_height: f32,

    /// Use clock-wise winding. This effectively inverts the height-field along the y-axis.
    pub clockwise_winding: bool,
}

/// This material index is used to designate holes in a height field.
pub const HEIGHT_FIELD_HOLE: u8 = 0xFF;

/// A height field with compressed storage.
#[derive(Clone, Debug, Default)]
pub struct HeightFieldData {
    /// Hash of this height field (zero when not yet computed).
    pub hash: u32,

    /// The local axis-aligned bounding box.
    pub aabb: AABB,

    /// The minimum y value.
    pub min_height: f32,

    /// The maximum y value.
    pub max_height: f32,

    /// The quantization scale.
    pub height_scale: f32,

    /// The overall scale.
    pub scale: Vec3,

    /// The number of grid columns along the local x-axis.
    pub column_count: i32,

    /// The number of grid rows along the local z-axis.
    pub row_count: i32,

    /// The compressed heights, one per grid point.
    pub heights: Vec<u16>,

    /// The material indices, one per cell. May be empty.
    pub materials: Vec<u8>,

    /// The triangle flags, one per triangle.
    pub flags: Vec<u8>,

    /// Triangle winding.
    pub clockwise: bool,
}

// ---------------------------------------------------------------------------
// Compound
// ---------------------------------------------------------------------------

/// Definition for a capsule in a compound shape.
#[derive(Clone, Copy, Debug)]
pub struct CompoundCapsuleDef {
    /// Local capsule.
    pub capsule: Capsule,
    /// Material properties.
    pub material: SurfaceMaterial,
}

/// Definition for a convex hull in a compound shape.
#[derive(Clone, Debug)]
pub struct CompoundHullDef {
    /// Shared hull.
    pub hull: Arc<HullData>,
    /// Transform of the shared hull into compound local space.
    pub transform: Transform,
    /// Material properties.
    pub material: SurfaceMaterial,
}

/// Definition for a triangle mesh in a compound shape.
#[derive(Clone, Debug)]
pub struct CompoundMeshDef {
    /// Shared mesh.
    pub mesh_data: Arc<MeshData>,
    /// Transform of the shared mesh into compound local space.
    pub transform: Transform,
    /// Local space non-uniform mesh scale. May have negative components.
    pub scale: Vec3,
    /// Material properties. This array must line up with the material indices
    /// on the triangles.
    pub materials: Vec<SurfaceMaterial>,
}

/// Definition for a sphere in a compound shape.
#[derive(Clone, Copy, Debug)]
pub struct CompoundSphereDef {
    /// Local sphere.
    pub sphere: Sphere,
    /// Material properties.
    pub material: SurfaceMaterial,
}

/// Definition for creating a compound shape. All this data is fully cloned
/// into the run-time compound shape.
#[derive(Clone, Debug, Default)]
pub struct CompoundDef {
    /// Capsule instances.
    pub capsules: Vec<CompoundCapsuleDef>,
    /// Hull instances.
    pub hulls: Vec<CompoundHullDef>,
    /// Mesh instances.
    pub meshes: Vec<CompoundMeshDef>,
    /// Sphere instances.
    pub spheres: Vec<CompoundSphereDef>,
}

/// Meshes used in compounds have limited space for materials.
pub const MAX_COMPOUND_MESH_MATERIALS: usize = 4;

/// A capsule that lives in a compound.
#[derive(Clone, Copy, Debug)]
pub struct CompoundCapsule {
    /// Local capsule.
    pub capsule: Capsule,
    /// Index to a shared material.
    pub material_index: i32,
}

/// A hull that lives in a compound.
#[derive(Clone, Debug)]
pub struct CompoundHull {
    /// The unique shared hull.
    pub hull: Arc<HullData>,
    /// The transform of this hull instance.
    pub transform: Transform,
    /// Index to a shared material.
    pub material_index: i32,
}

/// A mesh with non-uniform scale that lives in a compound.
#[derive(Clone, Debug)]
pub struct CompoundMesh {
    /// The unique shared mesh.
    pub mesh_data: Arc<MeshData>,
    /// The transform of this mesh instance.
    pub transform: Transform,
    /// Non-uniform scale of this mesh instance.
    pub scale: Vec3,
    /// materialIndex = material_indices[triangle material index]
    pub material_indices: [i32; MAX_COMPOUND_MESH_MATERIALS],
}

/// A sphere that lives in a compound.
#[derive(Clone, Copy, Debug)]
pub struct CompoundSphere {
    /// Local sphere.
    pub sphere: Sphere,
    /// Index to a shared material.
    pub material_index: i32,
}

/// The runtime data for a compound shape.
#[derive(Clone, Debug, Default)]
pub struct CompoundData {
    /// Immutable dynamic tree over the child shapes.
    pub tree: DynamicTree,

    /// The materials.
    pub materials: Vec<SurfaceMaterial>,

    /// The capsules.
    pub capsules: Vec<CompoundCapsule>,

    /// The hull instances.
    pub hulls: Vec<CompoundHull>,

    /// The number of unique hulls. Diagnostic.
    pub shared_hull_count: i32,

    /// The mesh instances.
    pub meshes: Vec<CompoundMesh>,

    /// The number of unique meshes. Diagnostic.
    pub shared_mesh_count: i32,

    /// The spheres.
    pub spheres: Vec<CompoundSphere>,
}

/// Child shape geometry of a compound (the C tagged union).
#[derive(Clone, Debug)]
pub enum ChildShapeGeom {
    Capsule(Capsule),
    Hull(Arc<HullData>),
    Mesh(Mesh),
    Sphere(Sphere),
}

/// Child shape of a compound.
#[derive(Clone, Debug)]
pub struct ChildShape {
    /// The shape geometry (tagged union in C).
    pub geom: ChildShapeGeom,

    /// Transform of the shape into compound local space.
    pub transform: Transform,

    /// Material indices. Index 0 is used for convex shapes.
    pub material_indices: [i32; MAX_COMPOUND_MESH_MATERIALS],
}

impl ChildShape {
    /// The shape type (union tag).
    pub fn shape_type(&self) -> ShapeType {
        match &self.geom {
            ChildShapeGeom::Capsule(_) => ShapeType::Capsule,
            ChildShapeGeom::Hull(_) => ShapeType::Hull,
            ChildShapeGeom::Mesh(_) => ShapeType::Mesh,
            ChildShapeGeom::Sphere(_) => ShapeType::Sphere,
        }
    }
}

// ---------------------------------------------------------------------------
// Shape Collision
// ---------------------------------------------------------------------------

/// A manifold point is a contact point belonging to a contact manifold.
#[derive(Clone, Copy, Debug, Default)]
pub struct ManifoldPoint {
    /// Location of the contact point relative to the bodyA center of mass in world space.
    pub anchor_a: Vec3,

    /// Location of the contact point relative to the bodyB center of mass in world space.
    pub anchor_b: Vec3,

    /// The separation of the contact point, negative if penetrating.
    pub separation: f32,

    /// Cached separation used for contact recycling.
    pub base_separation: f32,

    /// The impulse along the manifold normal vector (final sub-step).
    pub normal_impulse: f32,

    /// The total normal impulse applied during sub-stepping.
    pub total_normal_impulse: f32,

    /// Relative normal velocity pre-solve. Used for hit events.
    pub normal_velocity: f32,

    /// Uniquely identifies a contact point between two shapes.
    pub feature_id: u32,

    /// Triangle index if one of the shapes is a mesh or height field.
    pub triangle_index: i32,

    /// Did this contact point exist in the previous step?
    pub persisted: bool,
}

/// A contact manifold describes the contact points between colliding shapes.
#[derive(Clone, Copy, Debug, Default)]
pub struct Manifold {
    /// The manifold points. There may be 1 to 4 valid points.
    pub points: [ManifoldPoint; MAX_MANIFOLD_POINTS],

    /// The unit normal vector in world space, points from shape A to shape B.
    pub normal: Vec3,

    /// Central friction angular impulse (applied about the normal).
    pub twist_impulse: f32,

    /// Central friction linear impulse.
    pub friction_impulse: Vec3,

    /// Rolling resistance angular impulse.
    pub rolling_impulse: Vec3,

    /// The number of contact points, will be 0 to 4.
    pub point_count: i32,
}

/// Cached separating axis feature.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[repr(u8)]
pub enum SeparatingFeature {
    #[default]
    InvalidAxis = 0,
    BacksideAxis,
    FaceAxisA,
    FaceAxisB,
    EdgePairAxis,
    ClosestPointsAxis,

    // These are for testing
    ManualFaceAxisA,
    ManualFaceAxisB,
    ManualEdgePairAxis,
}

/// Cached triangle feature.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum TriangleFeature {
    #[default]
    None = 0,
    TriangleFace,
    HullFace,
    /// v1-v2
    Edge1,
    /// v2-v3
    Edge2,
    /// v3-v1
    Edge3,
    Vertex1,
    Vertex2,
    Vertex3,
}

/// Separating axis test cache. Provides temporal acceleration of collision routines.
#[derive(Clone, Copy, Debug, Default)]
pub struct SATCache {
    /// The separation when the cache is populated. Negative for overlap.
    pub separation: f32,

    /// SeparatingFeature.
    pub type_: u8,

    /// Index of the feature on shape A.
    pub index_a: u8,

    /// Index of the feature on shape B.
    pub index_b: u8,

    /// Was the cache re-used?
    pub hit: u8,
}

/// Contact points are always the result of two edges intersecting.
/// The feature pair is used to identify contact points for temporal coherence
/// and warm starting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FeaturePair {
    /// Incoming type (either edge on shape A or shape B).
    pub owner1: u8,
    /// Incoming edge index (into associated shape array).
    pub index1: u8,
    /// Outgoing type (either edge on shape A or shape B).
    pub owner2: u8,
    /// Outgoing edge index (into associated shape array).
    pub index2: u8,
}

/// A local manifold point and normal in frame A.
#[derive(Clone, Copy, Debug, Default)]
pub struct LocalManifoldPoint {
    /// Local point in frame A.
    pub point: Vec3,

    /// The contact point separation. Negative for overlap.
    pub separation: f32,

    /// The feature pair for this point.
    pub pair: FeaturePair,

    /// The triangle index when colliding with a mesh or height-field.
    pub triangle_index: i32,
}

/// A local manifold with no dynamic information. Used by collide functions.
/// The C version points into a caller-provided buffer; the port owns a Vec.
#[derive(Clone, Debug, Default)]
pub struct LocalManifold {
    /// Local normal in frame A.
    pub normal: Vec3,

    /// The triangle normal.
    pub triangle_normal: Vec3,

    /// The manifold points.
    pub points: Vec<LocalManifoldPoint>,

    /// The index of the triangle.
    pub triangle_index: i32,

    /// Vertex 1 index.
    pub i1: i32,
    /// Vertex 2 index.
    pub i2: i32,
    /// Vertex 3 index.
    pub i3: i32,

    /// The squared distance of a sphere from a triangle. For ghost collision reduction.
    pub squared_distance: f32,

    /// The triangle feature involved.
    pub feature: TriangleFeature,

    /// Mesh edge flags.
    pub triangle_flags: i32,
}

impl LocalManifold {
    #[inline]
    pub fn point_count(&self) -> i32 {
        self.points.len() as i32
    }
}
