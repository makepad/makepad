// Port of box3d/src/joint.h (+ joint.c functions to be ported below the structs).

use crate::math_functions::{Matrix3, Quat, Transform, Vec2, Vec3};
use crate::solver::Softness;
use crate::types::JointType;

/// A joint edge is used to connect bodies and joints together
/// in a joint graph where each body is a node and each joint is an edge.
#[derive(Clone, Copy, Debug, Default)]
pub struct JointEdge {
    pub body_id: i32,
    pub prev_key: i32,
    pub next_key: i32,
}

/// Map from JointId to Joint in the solver sets.
#[derive(Clone, Debug)]
pub struct Joint {
    pub user_data: u64,

    /// index of simulation set stored in World. NULL_INDEX when slot is free.
    pub set_index: i32,

    /// index into the constraint graph color array. NULL_INDEX for sleeping/disabled joints.
    pub color_index: i32,

    /// joint index within set or graph color. NULL_INDEX when slot is free.
    pub local_index: i32,

    pub edges: [JointEdge; 2],

    pub joint_id: i32,
    pub island_id: i32,

    /// Index into the island's joints array for O(1) swap-removal.
    pub island_index: i32,

    pub draw_scale: f32,

    pub joint_type: JointType,

    /// Monotonically advanced when a joint is allocated in this slot.
    pub generation: u16,

    pub collide_connected: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct DistanceJoint {
    pub length: f32,
    pub hertz: f32,
    pub damping_ratio: f32,
    pub lower_spring_force: f32,
    pub upper_spring_force: f32,
    pub min_length: f32,
    pub max_length: f32,

    pub max_motor_force: f32,
    pub motor_speed: f32,

    pub impulse: f32,
    pub lower_impulse: f32,
    pub upper_impulse: f32,
    pub motor_impulse: f32,

    pub index_a: i32,
    pub index_b: i32,
    pub anchor_a: Vec3,
    pub anchor_b: Vec3,
    pub delta_center: Vec3,
    pub distance_softness: Softness,
    pub axial_mass: f32,

    pub enable_spring: bool,
    pub enable_limit: bool,
    pub enable_motor: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct MotorJoint {
    pub linear_velocity: Vec3,
    pub angular_velocity: Vec3,
    pub max_velocity_force: f32,
    pub max_velocity_torque: f32,
    pub linear_hertz: f32,
    pub linear_damping_ratio: f32,
    pub max_spring_force: f32,
    pub angular_hertz: f32,
    pub angular_damping_ratio: f32,
    pub max_spring_torque: f32,

    pub linear_velocity_impulse: Vec3,
    pub angular_velocity_impulse: Vec3,
    pub linear_spring_impulse: Vec3,
    pub angular_spring_impulse: Vec3,

    pub linear_spring: Softness,
    pub angular_spring: Softness,

    pub index_a: i32,
    pub index_b: i32,
    pub frame_a: Transform,
    pub frame_b: Transform,
    pub delta_center: Vec3,
    pub angular_mass: Matrix3,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ParallelJoint {
    pub hertz: f32,
    pub damping_ratio: f32,
    pub max_torque: f32,

    pub perp_impulse: Vec2,
    pub perp_axis_x: Vec3,
    pub perp_axis_y: Vec3,

    pub quat_a: Quat,
    pub quat_b: Quat,
    pub index_a: i32,
    pub index_b: i32,
    pub softness: Softness,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct PrismaticJoint {
    pub perp_impulse: Vec2,
    pub angular_impulse: Vec3,
    pub spring_impulse: f32,
    pub motor_impulse: f32,
    pub lower_impulse: f32,
    pub upper_impulse: f32,
    pub hertz: f32,
    pub damping_ratio: f32,
    pub max_motor_force: f32,
    pub motor_speed: f32,
    pub target_translation: f32,
    pub lower_translation: f32,
    pub upper_translation: f32,

    pub index_a: i32,
    pub index_b: i32,
    pub frame_a: Transform,
    pub frame_b: Transform,
    pub joint_axis: Vec3,
    pub perp_axis_y: Vec3,
    pub perp_axis_z: Vec3,
    pub delta_center: Vec3,
    pub delta_angle: f32,
    pub rotation_mass: Matrix3,
    pub spring_softness: Softness,

    pub enable_spring: bool,
    pub enable_limit: bool,
    pub enable_motor: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct RevoluteJoint {
    pub linear_impulse: Vec3,
    pub perp_impulse: Vec2,
    pub spring_impulse: f32,
    pub motor_impulse: f32,
    pub lower_impulse: f32,
    pub upper_impulse: f32,
    pub hertz: f32,
    pub damping_ratio: f32,
    pub max_motor_torque: f32,
    pub motor_speed: f32,
    pub target_angle: f32,
    pub lower_angle: f32,
    pub upper_angle: f32,

    pub index_a: i32,
    pub index_b: i32,
    pub frame_a: Transform,
    pub frame_b: Transform,
    pub rotation_axis_z: Vec3,
    pub perp_axis_x: Vec3,
    pub perp_axis_y: Vec3,
    pub delta_center: Vec3,
    pub delta_angle: f32,
    pub axial_mass: f32,
    pub spring_softness: Softness,

    pub enable_spring: bool,
    pub enable_motor: bool,
    pub enable_limit: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct SphericalJoint {
    pub linear_impulse: Vec3,
    pub spring_impulse: Vec3,
    pub motor_impulse: Vec3,
    pub lower_twist_impulse: f32,
    pub upper_twist_impulse: f32,
    pub swing_impulse: f32,
    pub hertz: f32,
    pub damping_ratio: f32,
    pub max_motor_torque: f32,
    pub motor_velocity: Vec3,
    pub lower_twist_angle: f32,
    pub upper_twist_angle: f32,
    pub cone_angle: f32,
    pub target_rotation: Quat,

    pub index_a: i32,
    pub index_b: i32,
    pub frame_a: Transform,
    pub frame_b: Transform,
    pub delta_center: Vec3,
    pub swing_axis: Vec3,
    pub twist_jacobian: Vec3,

    pub rotation_mass: Matrix3,
    pub swing_mass: f32,
    pub twist_mass: f32,
    pub spring_softness: Softness,

    pub enable_spring: bool,
    pub enable_motor: bool,
    pub enable_cone_limit: bool,
    pub enable_twist_limit: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct WeldJoint {
    pub linear_hertz: f32,
    pub linear_damping_ratio: f32,
    pub angular_hertz: f32,
    pub angular_damping_ratio: f32,

    pub linear_spring: Softness,
    pub angular_spring: Softness,
    pub linear_impulse: Vec3,
    pub angular_impulse: Vec3,

    pub index_a: i32,
    pub index_b: i32,
    pub frame_a: Transform,
    pub frame_b: Transform,
    pub delta_center: Vec3,

    pub angular_mass: Matrix3,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct WheelJoint {
    pub linear_impulse: Vec2,
    pub angular_impulse: Vec2,
    pub spin_impulse: f32,
    pub max_spin_torque: f32,
    pub spin_speed: f32,
    pub suspension_spring_impulse: f32,
    pub lower_suspension_impulse: f32,
    pub upper_suspension_impulse: f32,
    pub lower_suspension_limit: f32,
    pub upper_suspension_limit: f32,
    pub suspension_hertz: f32,
    pub suspension_damping_ratio: f32,
    pub steering_spring_impulse: f32,
    pub lower_steering_impulse: f32,
    pub upper_steering_impulse: f32,
    pub lower_steering_limit: f32,
    pub upper_steering_limit: f32,
    pub target_steering_angle: f32,
    pub max_steering_torque: f32,
    pub steering_hertz: f32,
    pub steering_damping_ratio: f32,

    pub index_a: i32,
    pub index_b: i32,
    pub frame_a: Transform,
    pub frame_b: Transform,
    pub delta_center: Vec3,
    pub spin_mass: f32,
    pub suspension_mass: f32,
    pub steering_mass: f32,
    pub suspension_softness: Softness,
    pub steering_softness: Softness,

    pub enable_spin_motor: bool,
    pub enable_suspension_spring: bool,
    pub enable_suspension_limit: bool,
    pub enable_steering: bool,
    pub enable_steering_limit: bool,
    pub enable_steering_motor: bool,
}

/// The C b3JointSim union of typed joints.
#[derive(Clone, Copy, Debug)]
pub enum JointUnion {
    Distance(DistanceJoint),
    Motor(MotorJoint),
    Parallel(ParallelJoint),
    Revolute(RevoluteJoint),
    Spherical(SphericalJoint),
    Prismatic(PrismaticJoint),
    Weld(WeldJoint),
    Wheel(WheelJoint),
    Filter,
}

impl Default for JointUnion {
    fn default() -> Self {
        JointUnion::Filter
    }
}

/// The base joint class. Joints are used to constrain two bodies together in
/// various fashions. Some joints also feature limits and motors.
#[derive(Clone, Copy, Debug)]
pub struct JointSim {
    pub joint_id: i32,

    pub body_id_a: i32,
    pub body_id_b: i32,

    pub joint_type: JointType,

    /// Joint frames local to body origin
    pub local_frame_a: Transform,
    pub local_frame_b: Transform,

    pub inv_mass_a: f32,
    pub inv_mass_b: f32,
    pub inv_i_a: Matrix3,
    pub inv_i_b: Matrix3,

    pub constraint_hertz: f32,
    pub constraint_damping_ratio: f32,

    pub constraint_softness: Softness,

    pub force_threshold: f32,
    pub torque_threshold: f32,

    pub fixed_rotation: bool,

    /// The C anonymous union (tag: joint_type).
    pub joint: JointUnion,
}

impl Default for JointSim {
    fn default() -> Self {
        JointSim {
            joint_id: crate::core::NULL_INDEX,
            body_id_a: crate::core::NULL_INDEX,
            body_id_b: crate::core::NULL_INDEX,
            joint_type: JointType::Filter,
            local_frame_a: Transform::IDENTITY,
            local_frame_b: Transform::IDENTITY,
            inv_mass_a: 0.0,
            inv_mass_b: 0.0,
            inv_i_a: Matrix3::ZERO,
            inv_i_b: Matrix3::ZERO,
            constraint_hertz: 0.0,
            constraint_damping_ratio: 0.0,
            constraint_softness: Softness::default(),
            force_threshold: 0.0,
            torque_threshold: 0.0,
            fixed_rotation: false,
            joint: JointUnion::Filter,
        }
    }
}

// ---------------------------------------------------------------------------
// joint.c
// ---------------------------------------------------------------------------

use crate::b3_assert;
use crate::body::BodySim;
use crate::constants::{linear_slop, GRAPH_COLOR_COUNT};
use crate::constraint_graph::OVERFLOW_INDEX;
use crate::container::array_remove_swap;
use crate::core::{get_length_units_per_meter, NULL_INDEX, SECRET_COOKIE};
use crate::id::{BodyId, JointId, WorldId, NULL_JOINT_ID};
use crate::id_pool::{alloc_id, free_id};
use crate::math_functions::{
    abs_float, add, clamp_float, cross, dot, get_quat_angle, get_swing_angle, get_twist_angle,
    inv_mul_quat, is_valid_float, is_valid_quat, is_valid_transform, length, max_float, max_int,
    min_float, mul_add, mul_quat, normalize, perp, rotate_vector, sub_pos, transform_world_point,
    vec3, PI,
};
use crate::physics_world::{World, AWAKE_SET, DISABLED_SET, FIRST_SLEEPING_SET, STATIC_SET};
use crate::solver::{make_soft, StepContext};
use crate::types::{
    BodyType, DistanceJointDef, FilterJointDef, JointDef, MotorJointDef, ParallelJointDef,
    PrismaticJointDef, RevoluteJointDef, SphericalJointDef, WeldJointDef, WheelJointDef,
};

/// C zero-initialization equivalent for a fresh joint slot ((b3Joint){ 0 }).
/// All fields are overwritten by create_joint; joint_type 0 == Parallel in C.
impl Default for Joint {
    fn default() -> Self {
        Joint {
            user_data: 0,
            set_index: 0,
            color_index: 0,
            local_index: 0,
            edges: [JointEdge::default(); 2],
            joint_id: 0,
            island_id: 0,
            island_index: 0,
            draw_scale: 0.0,
            joint_type: JointType::Parallel,
            generation: 0,
            collide_connected: false,
        }
    }
}

fn default_joint_def() -> JointDef {
    JointDef {
        user_data: 0,
        body_id_a: crate::id::NULL_BODY_ID,
        body_id_b: crate::id::NULL_BODY_ID,
        local_frame_a: Transform::IDENTITY,
        local_frame_b: Transform::IDENTITY,
        force_threshold: f32::MAX,
        torque_threshold: f32::MAX,
        constraint_hertz: 60.0,
        constraint_damping_ratio: 2.0,
        draw_scale: get_length_units_per_meter(),
        collide_connected: false,
        internal_value: SECRET_COOKIE,
    }
}

pub fn default_parallel_joint_def() -> ParallelJointDef {
    ParallelJointDef {
        base: default_joint_def(),
        hertz: 1.0,
        damping_ratio: 1.0,
        max_torque: f32::MAX,
    }
}

pub fn default_distance_joint_def() -> DistanceJointDef {
    DistanceJointDef {
        base: default_joint_def(),
        length: 1.0,
        enable_spring: false,
        lower_spring_force: -f32::MAX,
        upper_spring_force: f32::MAX,
        hertz: 0.0,
        damping_ratio: 0.0,
        enable_limit: false,
        min_length: 0.0,
        max_length: crate::constants::huge(),
        enable_motor: false,
        max_motor_force: 0.0,
        motor_speed: 0.0,
    }
}

pub fn default_motor_joint_def() -> MotorJointDef {
    MotorJointDef {
        base: default_joint_def(),
        linear_velocity: Vec3::ZERO,
        max_velocity_force: 0.0,
        angular_velocity: Vec3::ZERO,
        max_velocity_torque: 0.0,
        linear_hertz: 0.0,
        linear_damping_ratio: 0.0,
        max_spring_force: 0.0,
        angular_hertz: 0.0,
        angular_damping_ratio: 0.0,
        max_spring_torque: 0.0,
    }
}

pub fn default_filter_joint_def() -> FilterJointDef {
    FilterJointDef { base: default_joint_def() }
}

pub fn default_prismatic_joint_def() -> PrismaticJointDef {
    PrismaticJointDef {
        base: default_joint_def(),
        enable_spring: false,
        hertz: 0.0,
        damping_ratio: 0.0,
        target_translation: 0.0,
        enable_limit: false,
        lower_translation: 0.0,
        upper_translation: 0.0,
        enable_motor: false,
        max_motor_force: 0.0,
        motor_speed: 0.0,
    }
}

pub fn default_revolute_joint_def() -> RevoluteJointDef {
    RevoluteJointDef {
        base: default_joint_def(),
        target_angle: 0.0,
        enable_spring: false,
        hertz: 0.0,
        damping_ratio: 0.0,
        enable_limit: false,
        lower_angle: 0.0,
        upper_angle: 0.0,
        enable_motor: false,
        max_motor_torque: 0.0,
        motor_speed: 0.0,
    }
}

pub fn default_spherical_joint_def() -> SphericalJointDef {
    SphericalJointDef {
        base: default_joint_def(),
        enable_spring: false,
        hertz: 0.0,
        damping_ratio: 0.0,
        target_rotation: Quat::IDENTITY,
        enable_cone_limit: false,
        cone_angle: 0.0,
        enable_twist_limit: false,
        lower_twist_angle: 0.0,
        upper_twist_angle: 0.0,
        enable_motor: false,
        max_motor_torque: 0.0,
        motor_velocity: Vec3::ZERO,
    }
}

pub fn default_weld_joint_def() -> WeldJointDef {
    WeldJointDef {
        base: default_joint_def(),
        linear_hertz: 0.0,
        angular_hertz: 0.0,
        linear_damping_ratio: 0.0,
        angular_damping_ratio: 0.0,
    }
}

pub fn default_wheel_joint_def() -> WheelJointDef {
    WheelJointDef {
        base: default_joint_def(),
        enable_suspension_spring: true,
        suspension_hertz: 1.0,
        suspension_damping_ratio: 0.7,
        enable_suspension_limit: false,
        lower_suspension_limit: 0.0,
        upper_suspension_limit: 0.0,
        enable_spin_motor: false,
        max_spin_torque: 0.0,
        spin_speed: 0.0,
        enable_steering: false,
        steering_hertz: 1.0,
        steering_damping_ratio: 0.7,
        target_steering_angle: 0.0,
        max_steering_torque: 0.0,
        enable_steering_limit: false,
        lower_steering_limit: 0.0,
        upper_steering_limit: 0.0,
    }
}

/// C returns b3Joint*; the port returns the validated raw joint index.
pub fn get_joint_full_id(world: &World, joint_id: JointId) -> i32 {
    let id = joint_id.index1 - 1;
    let joint = &world.joints[id as usize];
    b3_assert!(joint.joint_id == id && joint.generation == joint_id.generation);
    id
}

/// C b3GetJointSim(world, joint).
pub fn get_joint_sim_from_id(world: &World, joint_id: i32) -> &JointSim {
    let joint = &world.joints[joint_id as usize];
    if joint.set_index == AWAKE_SET {
        b3_assert!(0 <= joint.color_index && joint.color_index < GRAPH_COLOR_COUNT as i32);
        return &world.constraint_graph.colors[joint.color_index as usize].joint_sims
            [joint.local_index as usize];
    }

    let set = &world.solver_sets[joint.set_index as usize];
    &set.joint_sims[joint.local_index as usize]
}

pub fn get_joint_sim_from_id_mut(world: &mut World, joint_id: i32) -> &mut JointSim {
    let joint = &world.joints[joint_id as usize];
    let (set_index, color_index, local_index) = (joint.set_index, joint.color_index, joint.local_index);
    if set_index == AWAKE_SET {
        b3_assert!(0 <= color_index && color_index < GRAPH_COLOR_COUNT as i32);
        return &mut world.constraint_graph.colors[color_index as usize].joint_sims[local_index as usize];
    }

    let set = &mut world.solver_sets[set_index as usize];
    &mut set.joint_sims[local_index as usize]
}

pub fn get_joint_sim_check_type(world: &mut World, joint_id: JointId, joint_type: JointType) -> &mut JointSim {
    let id = get_joint_full_id(world, joint_id);
    b3_assert!(world.joints[id as usize].joint_type == joint_type);
    let joint_sim = get_joint_sim_from_id_mut(world, id);
    b3_assert!(joint_sim.joint_type == joint_type);
    joint_sim
}

/// During the solve the awake-set body sims are moved into the StepContext
/// (see solver.rs); body sims from other sets stay in the world.
pub fn get_solve_body_sim<'a>(
    world: &'a World,
    context: &'a StepContext,
    set_index: i32,
    local_index: i32,
) -> &'a BodySim {
    if set_index == AWAKE_SET {
        &context.sims[local_index as usize]
    } else {
        &world.solver_sets[set_index as usize].body_sims[local_index as usize]
    }
}

// C static b3CreateJoint returning b3JointPair; the port returns the joint id.
fn create_joint(world: &mut World, def: &JointDef, joint_type: JointType) -> i32 {
    b3_assert!(is_valid_transform(def.local_frame_a));
    b3_assert!(is_valid_transform(def.local_frame_b));

    let body_id_a = crate::body::get_body_full_id(world, def.body_id_a);
    let body_id_b = crate::body::get_body_full_id(world, def.body_id_b);

    let set_index_a = world.bodies[body_id_a as usize].set_index;
    let set_index_b = world.bodies[body_id_b as usize].set_index;
    let max_set_index = max_int(set_index_a, set_index_b);

    // Create joint id and joint
    let joint_id = alloc_id(&mut world.joint_id_pool);
    if joint_id == world.joints.len() as i32 {
        world.joints.push(Joint::default());
    }

    {
        let joint = &mut world.joints[joint_id as usize];
        joint.joint_id = joint_id;
        joint.user_data = def.user_data;
        joint.generation += 1;
        joint.set_index = NULL_INDEX;
        joint.color_index = NULL_INDEX;
        joint.local_index = NULL_INDEX;
        joint.island_id = NULL_INDEX;
        joint.island_index = NULL_INDEX;
        joint.draw_scale = def.draw_scale;
        joint.joint_type = joint_type;
        joint.collide_connected = def.collide_connected;
    }

    // Doubly linked list on bodyA
    let head_joint_key_a = world.bodies[body_id_a as usize].head_joint_key;
    world.joints[joint_id as usize].edges[0] =
        JointEdge { body_id: body_id_a, prev_key: NULL_INDEX, next_key: head_joint_key_a };

    let key_a = joint_id << 1;
    if head_joint_key_a != NULL_INDEX {
        let joint_a = &mut world.joints[(head_joint_key_a >> 1) as usize];
        joint_a.edges[(head_joint_key_a & 1) as usize].prev_key = key_a;
    }
    world.bodies[body_id_a as usize].head_joint_key = key_a;
    world.bodies[body_id_a as usize].joint_count += 1;

    // Doubly linked list on bodyB (head read after the body A update, like C)
    let head_joint_key_b = world.bodies[body_id_b as usize].head_joint_key;
    world.joints[joint_id as usize].edges[1] =
        JointEdge { body_id: body_id_b, prev_key: NULL_INDEX, next_key: head_joint_key_b };

    let key_b = (joint_id << 1) | 1;
    if head_joint_key_b != NULL_INDEX {
        let joint_b = &mut world.joints[(head_joint_key_b >> 1) as usize];
        joint_b.edges[(head_joint_key_b & 1) as usize].prev_key = key_b;
    }
    world.bodies[body_id_b as usize].head_joint_key = key_b;
    world.bodies[body_id_b as usize].joint_count += 1;

    let type_a = world.bodies[body_id_a as usize].body_type;
    let type_b = world.bodies[body_id_b as usize].body_type;

    if set_index_a == DISABLED_SET || set_index_b == DISABLED_SET {
        // if either body is disabled, create in disabled set
        let local_index = world.solver_sets[DISABLED_SET as usize].joint_sims.len() as i32;
        world.joints[joint_id as usize].set_index = DISABLED_SET;
        world.joints[joint_id as usize].local_index = local_index;

        world.solver_sets[DISABLED_SET as usize].joint_sims.push(JointSim::default());
        let joint_sim = world.solver_sets[DISABLED_SET as usize].joint_sims.last_mut().unwrap();
        joint_sim.joint_id = joint_id;
        joint_sim.body_id_a = body_id_a;
        joint_sim.body_id_b = body_id_b;
    } else if type_a != BodyType::Dynamic && type_b != BodyType::Dynamic {
        // joint is not attached to a dynamic body
        let local_index = world.solver_sets[STATIC_SET as usize].joint_sims.len() as i32;
        world.joints[joint_id as usize].set_index = STATIC_SET;
        world.joints[joint_id as usize].local_index = local_index;

        world.solver_sets[STATIC_SET as usize].joint_sims.push(JointSim::default());
        let joint_sim = world.solver_sets[STATIC_SET as usize].joint_sims.last_mut().unwrap();
        joint_sim.joint_id = joint_id;
        joint_sim.body_id_a = body_id_a;
        joint_sim.body_id_b = body_id_b;
    } else if set_index_a == AWAKE_SET || set_index_b == AWAKE_SET {
        // if either body is sleeping, wake it
        if max_set_index >= FIRST_SLEEPING_SET {
            crate::solver_set::wake_solver_set(world, max_set_index);
        }

        world.joints[joint_id as usize].set_index = AWAKE_SET;

        // create_joint_in_graph sets joint.color_index/local_index and returns them
        let (color_index, local_index) = crate::constraint_graph::create_joint_in_graph(world, joint_id);
        let joint_sim =
            &mut world.constraint_graph.colors[color_index as usize].joint_sims[local_index as usize];
        joint_sim.joint_id = joint_id;
        joint_sim.body_id_a = body_id_a;
        joint_sim.body_id_b = body_id_b;
    } else {
        // joint connected between sleeping and/or static bodies
        b3_assert!(set_index_a >= FIRST_SLEEPING_SET || set_index_b >= FIRST_SLEEPING_SET);
        b3_assert!(set_index_a != STATIC_SET || set_index_b != STATIC_SET);

        // joint should go into the sleeping set (not static set)
        let mut set_index = max_set_index;

        let local_index = world.solver_sets[set_index as usize].joint_sims.len() as i32;
        world.joints[joint_id as usize].set_index = set_index;
        world.joints[joint_id as usize].local_index = local_index;

        world.solver_sets[set_index as usize].joint_sims.push(JointSim::default());
        {
            let joint_sim = world.solver_sets[set_index as usize].joint_sims.last_mut().unwrap();
            joint_sim.joint_id = joint_id;
            joint_sim.body_id_a = body_id_a;
            joint_sim.body_id_b = body_id_b;
        }

        if set_index_a != set_index_b
            && set_index_a >= FIRST_SLEEPING_SET
            && set_index_b >= FIRST_SLEEPING_SET
        {
            // merge sleeping sets
            crate::solver_set::merge_solver_sets(world, set_index_a, set_index_b);
            b3_assert!(
                world.bodies[body_id_a as usize].set_index == world.bodies[body_id_b as usize].set_index
            );

            // fix potentially invalid set index
            set_index = world.bodies[body_id_a as usize].set_index;

            // Careful! The joint sim location was orphaned by the set merge;
            // it is re-fetched below through the joint record.
        }

        b3_assert!(world.joints[joint_id as usize].set_index == set_index);
    }

    // Common tail: fetch the sim through the joint record (this is exactly the
    // pointer the C code holds at this point, including after a set merge).
    {
        let joint_sim = get_joint_sim_from_id_mut(world, joint_id);
        joint_sim.local_frame_a = def.local_frame_a;
        joint_sim.local_frame_b = def.local_frame_b;
        joint_sim.joint_type = joint_type;
        joint_sim.constraint_hertz = def.constraint_hertz;
        joint_sim.constraint_damping_ratio = def.constraint_damping_ratio;
        joint_sim.constraint_softness = Softness {
            bias_rate: 0.0,
            mass_scale: 1.0,
            impulse_scale: 0.0,
        };

        b3_assert!(is_valid_float(def.force_threshold) && def.force_threshold >= 0.0);
        b3_assert!(is_valid_float(def.torque_threshold) && def.torque_threshold >= 0.0);

        joint_sim.force_threshold = def.force_threshold;
        joint_sim.torque_threshold = def.torque_threshold;

        b3_assert!(joint_sim.joint_id == joint_id);
        b3_assert!(joint_sim.body_id_a == body_id_a);
        b3_assert!(joint_sim.body_id_b == body_id_b);
    }

    if world.joints[joint_id as usize].set_index > DISABLED_SET {
        // Add edge to island graph
        crate::island::link_joint(world, joint_id);
    }

    crate::physics_world::validate_solver_sets(world);

    joint_id
}

fn destroy_contacts_between_bodies(world: &mut World, body_id_a: i32, body_id_b: i32) {
    let mut contact_key;
    let other_body_id;

    // use the smaller of the two contact lists
    if world.bodies[body_id_a as usize].contact_count < world.bodies[body_id_b as usize].contact_count {
        contact_key = world.bodies[body_id_a as usize].head_contact_key;
        other_body_id = body_id_b;
    } else {
        contact_key = world.bodies[body_id_b as usize].head_contact_key;
        other_body_id = body_id_a;
    }

    // no need to wake bodies when a joint removes collision between them
    let wake_bodies = false;

    // destroy the contacts
    while contact_key != NULL_INDEX {
        let contact_id = contact_key >> 1;
        let edge_index = contact_key & 1;

        let other_edge_index = (edge_index ^ 1) as usize;
        let other_edge_body_id;
        {
            let contact = &world.contacts[contact_id as usize];
            contact_key = contact.edges[edge_index as usize].next_key;
            other_edge_body_id = contact.edges[other_edge_index].body_id;
        }

        if other_edge_body_id == other_body_id {
            // Careful, this removes the contact from the current doubly linked list
            crate::contact::destroy_contact(world, contact_id, wake_bodies);
        }
    }

    crate::physics_world::validate_solver_sets(world);
}

pub fn joint_set_constraint_tuning(world: &mut World, joint_id: JointId, hertz: f32, damping_ratio: f32) {
    b3_assert!(is_valid_float(hertz) && hertz >= 0.0);
    b3_assert!(is_valid_float(damping_ratio) && damping_ratio >= 0.0);

    let id = get_joint_full_id(world, joint_id);
    let base = get_joint_sim_from_id_mut(world, id);
    base.constraint_hertz = hertz;
    base.constraint_damping_ratio = damping_ratio;
}

pub fn joint_get_constraint_tuning(world: &World, joint_id: JointId, hertz: &mut f32, damping_ratio: &mut f32) {
    let id = get_joint_full_id(world, joint_id);
    let base = get_joint_sim_from_id(world, id);
    *hertz = base.constraint_hertz;
    *damping_ratio = base.constraint_damping_ratio;
}

pub fn joint_set_force_threshold(world: &mut World, joint_id: JointId, threshold: f32) {
    b3_assert!(is_valid_float(threshold) && threshold >= 0.0);

    let id = get_joint_full_id(world, joint_id);
    let base = get_joint_sim_from_id_mut(world, id);
    base.force_threshold = threshold;
}

pub fn joint_get_force_threshold(world: &World, joint_id: JointId) -> f32 {
    let id = get_joint_full_id(world, joint_id);
    let base = get_joint_sim_from_id(world, id);
    base.force_threshold
}

pub fn joint_set_torque_threshold(world: &mut World, joint_id: JointId, threshold: f32) {
    b3_assert!(is_valid_float(threshold) && threshold >= 0.0);

    let id = get_joint_full_id(world, joint_id);
    let base = get_joint_sim_from_id_mut(world, id);
    base.torque_threshold = threshold;
}

pub fn joint_get_torque_threshold(world: &World, joint_id: JointId) -> f32 {
    let id = get_joint_full_id(world, joint_id);
    let base = get_joint_sim_from_id(world, id);
    base.torque_threshold
}

fn make_public_joint_id(world: &World, joint_id: i32) -> JointId {
    JointId {
        index1: joint_id + 1,
        world0: world.world_id,
        generation: world.joints[joint_id as usize].generation,
    }
}

pub fn create_distance_joint(world: &mut World, def: &DistanceJointDef) -> JointId {
    b3_assert!(def.base.internal_value == SECRET_COOKIE);
    b3_assert!(!world.locked);
    if world.locked {
        return NULL_JOINT_ID;
    }

    b3_assert!(is_valid_float(def.length) && def.length > 0.0);
    b3_assert!(def.lower_spring_force <= def.upper_spring_force);

    let joint_id = create_joint(world, &def.base, JointType::Distance);

    let distance_joint = DistanceJoint {
        length: max_float(def.length, linear_slop()),
        hertz: def.hertz,
        damping_ratio: def.damping_ratio,
        lower_spring_force: def.lower_spring_force,
        upper_spring_force: def.upper_spring_force,
        min_length: max_float(def.min_length, linear_slop()),
        max_length: max_float(def.min_length, def.max_length),
        max_motor_force: def.max_motor_force,
        motor_speed: def.motor_speed,
        enable_spring: def.enable_spring,
        enable_limit: def.enable_limit,
        enable_motor: def.enable_motor,
        impulse: 0.0,
        lower_impulse: 0.0,
        upper_impulse: 0.0,
        motor_impulse: 0.0,
        ..Default::default()
    };
    let joint_sim = get_joint_sim_from_id_mut(world, joint_id);
    joint_sim.joint = JointUnion::Distance(distance_joint);

    make_public_joint_id(world, joint_id)
}

pub fn create_motor_joint(world: &mut World, def: &MotorJointDef) -> JointId {
    b3_assert!(def.base.internal_value == SECRET_COOKIE);
    b3_assert!(!world.locked);
    if world.locked {
        return NULL_JOINT_ID;
    }

    let joint_id = create_joint(world, &def.base, JointType::Motor);

    let motor_joint = MotorJoint {
        linear_velocity: def.linear_velocity,
        max_velocity_force: def.max_velocity_force,
        angular_velocity: def.angular_velocity,
        max_velocity_torque: def.max_velocity_torque,
        linear_hertz: def.linear_hertz,
        linear_damping_ratio: def.linear_damping_ratio,
        max_spring_force: def.max_spring_force,
        angular_hertz: def.angular_hertz,
        angular_damping_ratio: def.angular_damping_ratio,
        max_spring_torque: def.max_spring_torque,
        ..Default::default()
    };
    let joint_sim = get_joint_sim_from_id_mut(world, joint_id);
    joint_sim.joint = JointUnion::Motor(motor_joint);

    make_public_joint_id(world, joint_id)
}

pub fn create_filter_joint(world: &mut World, def: &FilterJointDef) -> JointId {
    b3_assert!(def.base.internal_value == SECRET_COOKIE);
    b3_assert!(!world.locked);
    if world.locked {
        return NULL_JOINT_ID;
    }

    let joint_id = create_joint(world, &def.base, JointType::Filter);

    let joint_sim = get_joint_sim_from_id_mut(world, joint_id);
    joint_sim.joint = JointUnion::Filter;

    make_public_joint_id(world, joint_id)
}

pub fn create_parallel_joint(world: &mut World, def: &ParallelJointDef) -> JointId {
    b3_assert!(def.base.internal_value == SECRET_COOKIE);
    b3_assert!(!world.locked);
    if world.locked {
        return NULL_JOINT_ID;
    }

    b3_assert!(is_valid_float(def.hertz) && def.hertz >= 0.0);
    b3_assert!(is_valid_float(def.damping_ratio) && def.damping_ratio >= 0.0);
    b3_assert!(is_valid_float(def.max_torque) && def.max_torque >= 0.0);

    let joint_id = create_joint(world, &def.base, JointType::Parallel);

    let parallel_joint = ParallelJoint {
        hertz: def.hertz,
        damping_ratio: def.damping_ratio,
        max_torque: def.max_torque,
        ..Default::default()
    };
    let joint_sim = get_joint_sim_from_id_mut(world, joint_id);
    joint_sim.joint = JointUnion::Parallel(parallel_joint);

    make_public_joint_id(world, joint_id)
}

pub fn create_prismatic_joint(world: &mut World, def: &PrismaticJointDef) -> JointId {
    b3_assert!(def.base.internal_value == SECRET_COOKIE);
    b3_assert!(def.lower_translation <= def.upper_translation);
    b3_assert!(!world.locked);
    if world.locked {
        return NULL_JOINT_ID;
    }

    let joint_id = create_joint(world, &def.base, JointType::Prismatic);

    let prismatic_joint = PrismaticJoint {
        hertz: def.hertz,
        damping_ratio: def.damping_ratio,
        target_translation: def.target_translation,
        lower_translation: def.lower_translation,
        upper_translation: def.upper_translation,
        max_motor_force: def.max_motor_force,
        motor_speed: def.motor_speed,
        enable_spring: def.enable_spring,
        enable_limit: def.enable_limit,
        enable_motor: def.enable_motor,
        ..Default::default()
    };
    let joint_sim = get_joint_sim_from_id_mut(world, joint_id);
    joint_sim.joint = JointUnion::Prismatic(prismatic_joint);

    make_public_joint_id(world, joint_id)
}

pub fn create_revolute_joint(world: &mut World, def: &RevoluteJointDef) -> JointId {
    b3_assert!(def.base.internal_value == SECRET_COOKIE);
    b3_assert!(!world.locked);
    if world.locked {
        return NULL_JOINT_ID;
    }

    let joint_id = create_joint(world, &def.base, JointType::Revolute);

    let lower_angle = min_float(def.lower_angle, def.upper_angle);
    let upper_angle = max_float(def.lower_angle, def.upper_angle);

    let revolute_joint = RevoluteJoint {
        hertz: def.hertz,
        damping_ratio: def.damping_ratio,
        target_angle: clamp_float(def.target_angle, -PI, PI),
        lower_angle: clamp_float(lower_angle, -0.99 * PI, 0.99 * PI),
        upper_angle: clamp_float(upper_angle, -0.99 * PI, 0.99 * PI),
        max_motor_torque: def.max_motor_torque,
        motor_speed: def.motor_speed,
        enable_spring: def.enable_spring,
        enable_limit: def.enable_limit,
        enable_motor: def.enable_motor,
        ..Default::default()
    };
    let joint_sim = get_joint_sim_from_id_mut(world, joint_id);
    joint_sim.joint = JointUnion::Revolute(revolute_joint);

    make_public_joint_id(world, joint_id)
}

pub fn create_spherical_joint(world: &mut World, def: &SphericalJointDef) -> JointId {
    b3_assert!(def.base.internal_value == SECRET_COOKIE);
    b3_assert!(0.0 <= def.cone_angle && def.cone_angle <= 0.99 * PI);
    b3_assert!(is_valid_quat(def.target_rotation));
    b3_assert!(!world.locked);
    if world.locked {
        return NULL_JOINT_ID;
    }

    let joint_id = create_joint(world, &def.base, JointType::Spherical);

    let lower_angle = min_float(def.lower_twist_angle, def.upper_twist_angle);
    let upper_angle = max_float(def.lower_twist_angle, def.upper_twist_angle);

    let spherical_joint = SphericalJoint {
        hertz: def.hertz,
        damping_ratio: def.damping_ratio,
        target_rotation: def.target_rotation,
        cone_angle: clamp_float(def.cone_angle, 0.0, 0.5 * PI),
        lower_twist_angle: clamp_float(lower_angle, -0.99 * PI, 0.99 * PI),
        upper_twist_angle: clamp_float(upper_angle, -0.99 * PI, 0.99 * PI),
        max_motor_torque: def.max_motor_torque,
        motor_velocity: def.motor_velocity,
        enable_spring: def.enable_spring,
        enable_cone_limit: def.enable_cone_limit,
        enable_twist_limit: def.enable_twist_limit,
        enable_motor: def.enable_motor,
        ..Default::default()
    };
    let joint_sim = get_joint_sim_from_id_mut(world, joint_id);
    joint_sim.joint = JointUnion::Spherical(spherical_joint);

    make_public_joint_id(world, joint_id)
}

pub fn create_weld_joint(world: &mut World, def: &WeldJointDef) -> JointId {
    b3_assert!(def.base.internal_value == SECRET_COOKIE);
    b3_assert!(0.0 <= def.angular_hertz);
    b3_assert!(0.0 <= def.angular_damping_ratio);
    b3_assert!(0.0 <= def.linear_hertz);
    b3_assert!(0.0 <= def.linear_damping_ratio);
    b3_assert!(!world.locked);
    if world.locked {
        return NULL_JOINT_ID;
    }

    let joint_id = create_joint(world, &def.base, JointType::Weld);

    let weld_joint = WeldJoint {
        linear_hertz: def.linear_hertz,
        linear_damping_ratio: def.linear_damping_ratio,
        angular_hertz: def.angular_hertz,
        angular_damping_ratio: def.angular_damping_ratio,
        ..Default::default()
    };
    let joint_sim = get_joint_sim_from_id_mut(world, joint_id);
    joint_sim.joint = JointUnion::Weld(weld_joint);

    make_public_joint_id(world, joint_id)
}

pub fn create_wheel_joint(world: &mut World, def: &WheelJointDef) -> JointId {
    b3_assert!(def.base.internal_value == SECRET_COOKIE);
    b3_assert!(def.lower_suspension_limit <= def.upper_suspension_limit);
    b3_assert!(!world.locked);
    if world.locked {
        return NULL_JOINT_ID;
    }

    let joint_id = create_joint(world, &def.base, JointType::Wheel);

    let wheel_joint = WheelJoint {
        enable_suspension_spring: def.enable_suspension_spring,
        suspension_hertz: def.suspension_hertz,
        suspension_damping_ratio: def.suspension_damping_ratio,
        enable_suspension_limit: def.enable_suspension_limit,
        lower_suspension_limit: def.lower_suspension_limit,
        upper_suspension_limit: def.upper_suspension_limit,
        enable_spin_motor: def.enable_spin_motor,
        max_spin_torque: def.max_spin_torque,
        spin_speed: def.spin_speed,
        enable_steering: def.enable_steering,
        steering_hertz: def.steering_hertz,
        steering_damping_ratio: def.steering_damping_ratio,
        target_steering_angle: def.target_steering_angle,
        max_steering_torque: def.max_steering_torque,
        enable_steering_limit: def.enable_steering_limit,
        lower_steering_limit: def.lower_steering_limit,
        upper_steering_limit: def.upper_steering_limit,
        ..Default::default()
    };
    let joint_sim = get_joint_sim_from_id_mut(world, joint_id);
    joint_sim.joint = JointUnion::Wheel(wheel_joint);

    make_public_joint_id(world, joint_id)
}

pub fn destroy_joint_internal(world: &mut World, joint_id: i32, wake_bodies: bool) {
    // Remove from body A. Edge values are re-read from the joint record at each
    // use so aliasing through the linked list matches the C pointer behavior.
    let edge_a = world.joints[joint_id as usize].edges[0];
    let id_a = edge_a.body_id;

    if edge_a.prev_key != NULL_INDEX {
        let prev_joint = &mut world.joints[(edge_a.prev_key >> 1) as usize];
        prev_joint.edges[(edge_a.prev_key & 1) as usize].next_key = edge_a.next_key;
    }

    if edge_a.next_key != NULL_INDEX {
        let next_joint = &mut world.joints[(edge_a.next_key >> 1) as usize];
        next_joint.edges[(edge_a.next_key & 1) as usize].prev_key = edge_a.prev_key;
    }

    let edge_key_a = joint_id << 1;
    if world.bodies[id_a as usize].head_joint_key == edge_key_a {
        world.bodies[id_a as usize].head_joint_key = edge_a.next_key;
    }

    world.bodies[id_a as usize].joint_count -= 1;

    // Remove from body B (re-read edge after the A-side surgery)
    let edge_b = world.joints[joint_id as usize].edges[1];
    let id_b = edge_b.body_id;

    if edge_b.prev_key != NULL_INDEX {
        let prev_joint = &mut world.joints[(edge_b.prev_key >> 1) as usize];
        prev_joint.edges[(edge_b.prev_key & 1) as usize].next_key = edge_b.next_key;
    }

    if edge_b.next_key != NULL_INDEX {
        let next_joint = &mut world.joints[(edge_b.next_key >> 1) as usize];
        next_joint.edges[(edge_b.next_key & 1) as usize].prev_key = edge_b.prev_key;
    }

    let edge_key_b = (joint_id << 1) | 1;
    if world.bodies[id_b as usize].head_joint_key == edge_key_b {
        world.bodies[id_b as usize].head_joint_key = edge_b.next_key;
    }

    world.bodies[id_b as usize].joint_count -= 1;

    if world.joints[joint_id as usize].island_id != NULL_INDEX {
        b3_assert!(world.joints[joint_id as usize].set_index > DISABLED_SET);
        crate::island::unlink_joint(world, joint_id);
    } else {
        b3_assert!(world.joints[joint_id as usize].set_index <= DISABLED_SET);
    }

    // Remove joint from solver set that owns it
    let set_index = world.joints[joint_id as usize].set_index;
    let local_index = world.joints[joint_id as usize].local_index;

    if set_index == AWAKE_SET {
        let color_index = world.joints[joint_id as usize].color_index;
        let body_id_a = world.joints[joint_id as usize].edges[0].body_id;
        let body_id_b = world.joints[joint_id as usize].edges[1].body_id;
        crate::constraint_graph::remove_joint_from_graph(world, body_id_a, body_id_b, color_index, local_index);
    } else {
        let moved_index =
            array_remove_swap(&mut world.solver_sets[set_index as usize].joint_sims, local_index);
        if moved_index != NULL_INDEX {
            // Fix moved joint
            let moved_id =
                world.solver_sets[set_index as usize].joint_sims[local_index as usize].joint_id;
            b3_assert!(world.joints[moved_id as usize].local_index == moved_index);
            world.joints[moved_id as usize].local_index = local_index;
        }
    }

    // Free joint and id (preserve joint revision)
    {
        let joint = &mut world.joints[joint_id as usize];
        joint.set_index = NULL_INDEX;
        joint.local_index = NULL_INDEX;
        joint.color_index = NULL_INDEX;
        joint.joint_id = NULL_INDEX;
    }
    free_id(&mut world.joint_id_pool, joint_id);

    if wake_bodies {
        crate::body::wake_body(world, id_a);
        crate::body::wake_body(world, id_b);
    }

    crate::physics_world::validate_solver_sets(world);
}

pub fn destroy_joint(world: &mut World, joint_id: JointId, wake_attached: bool) {
    let id = get_joint_full_id(world, joint_id);
    destroy_joint_internal(world, id, wake_attached);
}

pub fn joint_get_type(world: &World, joint_id: JointId) -> JointType {
    let id = get_joint_full_id(world, joint_id);
    world.joints[id as usize].joint_type
}

pub fn joint_get_body_a(world: &World, joint_id: JointId) -> BodyId {
    let id = get_joint_full_id(world, joint_id);
    crate::body::make_body_id(world, world.joints[id as usize].edges[0].body_id)
}

pub fn joint_get_body_b(world: &World, joint_id: JointId) -> BodyId {
    let id = get_joint_full_id(world, joint_id);
    crate::body::make_body_id(world, world.joints[id as usize].edges[1].body_id)
}

pub fn joint_get_world(world: &World, joint_id: JointId) -> WorldId {
    WorldId { index1: joint_id.world0 + 1, generation: world.generation }
}

pub fn joint_set_local_frame_a(world: &mut World, joint_id: JointId, local_frame: Transform) {
    b3_assert!(is_valid_transform(local_frame));

    let id = get_joint_full_id(world, joint_id);
    let joint_sim = get_joint_sim_from_id_mut(world, id);
    joint_sim.local_frame_a = local_frame;
}

pub fn joint_get_local_frame_a(world: &World, joint_id: JointId) -> Transform {
    let id = get_joint_full_id(world, joint_id);
    get_joint_sim_from_id(world, id).local_frame_a
}

pub fn joint_set_local_frame_b(world: &mut World, joint_id: JointId, local_frame: Transform) {
    b3_assert!(is_valid_transform(local_frame));

    let id = get_joint_full_id(world, joint_id);
    let joint_sim = get_joint_sim_from_id_mut(world, id);
    joint_sim.local_frame_b = local_frame;
}

pub fn joint_get_local_frame_b(world: &World, joint_id: JointId) -> Transform {
    let id = get_joint_full_id(world, joint_id);
    get_joint_sim_from_id(world, id).local_frame_b
}

pub fn joint_set_collide_connected(world: &mut World, joint_id: JointId, should_collide: bool) {
    b3_assert!(!world.locked);
    if world.locked {
        return;
    }

    let id = get_joint_full_id(world, joint_id);
    if world.joints[id as usize].collide_connected == should_collide {
        return;
    }

    world.joints[id as usize].collide_connected = should_collide;

    let body_id_a = world.joints[id as usize].edges[0].body_id;
    let body_id_b = world.joints[id as usize].edges[1].body_id;

    if should_collide {
        // need to tell the broad-phase to look for new pairs for one of the
        // two bodies. Pick the one with the fewest shapes.
        let shape_count_a = world.bodies[body_id_a as usize].shape_count;
        let shape_count_b = world.bodies[body_id_b as usize].shape_count;

        let mut shape_id = if shape_count_a < shape_count_b {
            world.bodies[body_id_a as usize].head_shape_id
        } else {
            world.bodies[body_id_b as usize].head_shape_id
        };
        while shape_id != NULL_INDEX {
            let (proxy_key, next_shape_id) = {
                let shape = &world.shapes[shape_id as usize];
                (shape.proxy_key, shape.next_shape_id)
            };

            if proxy_key != NULL_INDEX {
                crate::broad_phase::buffer_move(&mut world.broad_phase, proxy_key);
            }

            shape_id = next_shape_id;
        }
    } else {
        destroy_contacts_between_bodies(world, body_id_a, body_id_b);
    }
}

pub fn joint_get_collide_connected(world: &World, joint_id: JointId) -> bool {
    let id = get_joint_full_id(world, joint_id);
    world.joints[id as usize].collide_connected
}

pub fn joint_set_user_data(world: &mut World, joint_id: JointId, user_data: u64) {
    let id = get_joint_full_id(world, joint_id);
    world.joints[id as usize].user_data = user_data;
}

pub fn joint_get_user_data(world: &World, joint_id: JointId) -> u64 {
    let id = get_joint_full_id(world, joint_id);
    world.joints[id as usize].user_data
}

pub fn joint_wake_bodies(world: &mut World, joint_id: JointId) {
    b3_assert!(!world.locked);
    if world.locked {
        return;
    }

    world.locked = true;

    let id = get_joint_full_id(world, joint_id);
    let body_id_a = world.joints[id as usize].edges[0].body_id;
    let body_id_b = world.joints[id as usize].edges[1].body_id;

    crate::body::wake_body(world, body_id_a);
    crate::body::wake_body(world, body_id_b);

    world.locked = false;
}

/// Body transform lookup that works both inside the solve (awake-set sims are
/// moved into the StepContext, C reads the live array through context->sims)
/// and outside of it (sims live in the world's solver sets).
fn reaction_body_transform(
    world: &World,
    context: Option<&StepContext>,
    body_id: i32,
) -> crate::math_functions::WorldTransform {
    let body = &world.bodies[body_id as usize];
    if body.set_index == AWAKE_SET {
        if let Some(context) = context {
            return context.sims[body.local_index as usize].transform;
        }
    }
    crate::body::get_body_transform_quick(world, body_id)
}

/// C: b3GetJointReaction. `context` must be Some during the solve because the
/// awake-set body sims are moved into the StepContext (see solver.rs).
pub fn get_joint_reaction(
    world: &World,
    context: Option<&StepContext>,
    sim: &JointSim,
    inv_time_step: f32,
    force: &mut f32,
    torque: &mut f32,
) {
    let mut linear_impulse = 0.0;
    let mut angular_impulse = 0.0;

    match &sim.joint {
        JointUnion::Parallel(joint) => {
            let impulse = vec3(joint.perp_impulse.x, joint.perp_impulse.y, 0.0);
            angular_impulse = length(impulse);
        }

        JointUnion::Distance(joint) => {
            linear_impulse =
                abs_float(joint.impulse + joint.lower_impulse - joint.upper_impulse + joint.motor_impulse);
        }

        JointUnion::Motor(joint) => {
            linear_impulse = length(add(joint.linear_velocity_impulse, joint.linear_spring_impulse));
            angular_impulse = length(add(joint.angular_velocity_impulse, joint.angular_spring_impulse));
        }

        JointUnion::Prismatic(joint) => {
            let impulse = vec3(
                joint.motor_impulse + joint.lower_impulse - joint.upper_impulse,
                joint.perp_impulse.x,
                joint.perp_impulse.y,
            );
            linear_impulse = length(impulse);
            angular_impulse = length(joint.angular_impulse);
        }

        JointUnion::Revolute(joint) => {
            linear_impulse = length(joint.linear_impulse);
            let impulse = vec3(
                joint.perp_impulse.x,
                joint.perp_impulse.y,
                joint.motor_impulse + joint.lower_impulse - joint.upper_impulse,
            );
            angular_impulse = length(impulse);
        }

        JointUnion::Spherical(joint) => {
            // todo improve performance
            linear_impulse = length(joint.linear_impulse);

            let xf_a = reaction_body_transform(world, context, sim.body_id_a);
            let xf_b = reaction_body_transform(world, context, sim.body_id_b);
            let q_a = mul_quat(xf_a.q, sim.local_frame_a.q);
            let q_b = mul_quat(xf_b.q, sim.local_frame_b.q);

            // Cone axis is the z-axis of body A.
            let cone_axis = rotate_vector(q_a, Vec3::AXIS_Z);
            let twist_axis = rotate_vector(q_b, Vec3::AXIS_Z);
            let swing_axis = normalize(cross(cone_axis, twist_axis));

            let mut impulse = add(joint.spring_impulse, joint.motor_impulse);
            impulse = mul_add(impulse, joint.lower_twist_impulse - joint.upper_twist_impulse, twist_axis);
            impulse = mul_add(impulse, joint.swing_impulse, swing_axis);

            angular_impulse = length(impulse);
        }

        JointUnion::Weld(joint) => {
            linear_impulse = length(joint.linear_impulse);
            angular_impulse = length(joint.angular_impulse);
        }

        JointUnion::Wheel(joint) => {
            // todo probably wrong
            let perp_impulse = joint.linear_impulse;
            let axial_impulse = joint.suspension_spring_impulse + joint.lower_suspension_impulse
                - joint.upper_suspension_impulse;
            linear_impulse = (perp_impulse.x * perp_impulse.x
                + perp_impulse.y * perp_impulse.y
                + axial_impulse * axial_impulse)
                .sqrt();
            angular_impulse = abs_float(joint.spin_impulse);
        }

        JointUnion::Filter => {}
    }

    *force = linear_impulse * inv_time_step;
    *torque = angular_impulse * inv_time_step;
}

fn get_joint_constraint_force(world: &World, joint_id: i32) -> Vec3 {
    let base = get_joint_sim_from_id(world, joint_id);

    match world.joints[joint_id as usize].joint_type {
        JointType::Parallel => Vec3::ZERO,
        JointType::Distance => crate::distance_joint::get_distance_joint_force(world, base),
        JointType::Filter => Vec3::ZERO,
        JointType::Motor => crate::motor_joint::get_motor_joint_force(world, base),
        JointType::Prismatic => crate::prismatic_joint::get_prismatic_joint_force(world, base),
        JointType::Revolute => crate::revolute_joint::get_revolute_joint_force(world, base),
        JointType::Spherical => crate::spherical_joint::get_spherical_joint_force(world, base),
        JointType::Weld => crate::weld_joint::get_weld_joint_force(world, base),
        JointType::Wheel => crate::wheel_joint::get_wheel_joint_force(world, base),
    }
}

fn get_joint_constraint_torque(world: &mut World, joint_id: i32) -> Vec3 {
    let joint_type = world.joints[joint_id as usize].joint_type;
    match joint_type {
        JointType::Parallel => {
            // The C getter mutates the sim (stores refreshed perp axes);
            // JointSim is Copy so use copy-out/copy-in around the call.
            let mut sim = *get_joint_sim_from_id(world, joint_id);
            let torque = crate::parallel_joint::get_parallel_joint_torque(world, &mut sim);
            *get_joint_sim_from_id_mut(world, joint_id) = sim;
            torque
        }
        JointType::Distance => Vec3::ZERO,
        JointType::Filter => Vec3::ZERO,
        _ => {
            let base = get_joint_sim_from_id(world, joint_id);
            match joint_type {
                JointType::Motor => crate::motor_joint::get_motor_joint_torque(world, base),
                JointType::Prismatic => crate::prismatic_joint::get_prismatic_joint_torque(world, base),
                JointType::Revolute => crate::revolute_joint::get_revolute_joint_torque(world, base),
                JointType::Spherical => crate::spherical_joint::get_spherical_joint_torque(world, base),
                JointType::Weld => crate::weld_joint::get_weld_joint_torque(world, base),
                JointType::Wheel => crate::wheel_joint::get_wheel_joint_torque(world, base),
                _ => unreachable!(),
            }
        }
    }
}

pub fn joint_get_constraint_force(world: &World, joint_id: JointId) -> Vec3 {
    let id = get_joint_full_id(world, joint_id);
    get_joint_constraint_force(world, id)
}

pub fn joint_get_constraint_torque(world: &mut World, joint_id: JointId) -> Vec3 {
    let id = get_joint_full_id(world, joint_id);
    get_joint_constraint_torque(world, id)
}

pub fn joint_get_linear_separation(world: &World, joint_id: JointId) -> f32 {
    let id = get_joint_full_id(world, joint_id);
    let body_id_a = world.joints[id as usize].edges[0].body_id;
    let body_id_b = world.joints[id as usize].edges[1].body_id;
    let base = get_joint_sim_from_id(world, id);

    let xf_a = crate::body::get_body_transform(world, body_id_a);
    let xf_b = crate::body::get_body_transform(world, body_id_b);

    let p_a = transform_world_point(xf_a, base.local_frame_a.p);
    let p_b = transform_world_point(xf_b, base.local_frame_b.p);
    let dp = sub_pos(p_b, p_a);

    match &base.joint {
        JointUnion::Parallel(_) => 0.0,

        JointUnion::Distance(distance_joint) => {
            let len = length(dp);
            if distance_joint.enable_spring {
                if distance_joint.enable_limit {
                    if len < distance_joint.min_length {
                        return distance_joint.min_length - len;
                    }

                    if len > distance_joint.max_length {
                        return len - distance_joint.max_length;
                    }

                    return 0.0;
                }

                return 0.0;
            }

            abs_float(len - distance_joint.length)
        }

        JointUnion::Motor(_) => 0.0,

        JointUnion::Filter => 0.0,

        JointUnion::Prismatic(prismatic_joint) => {
            let axis_a = rotate_vector(xf_a.q, Vec3::AXIS_X);
            let perp_a = perp(axis_a);
            let perpendicular_separation = abs_float(dot(perp_a, dp));
            let mut limit_separation = 0.0;

            if prismatic_joint.enable_limit {
                let translation = dot(axis_a, dp);
                if translation < prismatic_joint.lower_translation {
                    limit_separation = prismatic_joint.lower_translation - translation;
                }

                if prismatic_joint.upper_translation < translation {
                    limit_separation = translation - prismatic_joint.upper_translation;
                }
            }

            (perpendicular_separation * perpendicular_separation + limit_separation * limit_separation)
                .sqrt()
        }

        JointUnion::Revolute(_) => length(dp),

        JointUnion::Spherical(_) => length(dp),

        JointUnion::Weld(weld_joint) => {
            if weld_joint.linear_hertz == 0.0 {
                return length(dp);
            }

            0.0
        }

        JointUnion::Wheel(wheel_joint) => {
            let axis_a = rotate_vector(xf_a.q, Vec3::AXIS_X);
            let perp_a = perp(axis_a);
            let perpendicular_separation = abs_float(dot(perp_a, dp));
            let mut limit_separation = 0.0;

            if wheel_joint.enable_suspension_limit {
                let translation = dot(axis_a, dp);
                if translation < wheel_joint.lower_suspension_limit {
                    limit_separation = wheel_joint.lower_suspension_limit - translation;
                }

                if wheel_joint.upper_suspension_limit < translation {
                    limit_separation = translation - wheel_joint.upper_suspension_limit;
                }
            }

            (perpendicular_separation * perpendicular_separation + limit_separation * limit_separation)
                .sqrt()
        }
    }
}

pub fn joint_get_angular_separation(world: &World, joint_id: JointId) -> f32 {
    let id = get_joint_full_id(world, joint_id);
    let body_id_a = world.joints[id as usize].edges[0].body_id;
    let body_id_b = world.joints[id as usize].edges[1].body_id;
    let base = get_joint_sim_from_id(world, id);

    let xf_a = crate::body::get_body_transform(world, body_id_a);
    let xf_b = crate::body::get_body_transform(world, body_id_b);

    let rel_q = inv_mul_quat(xf_a.q, xf_b.q);

    match &base.joint {
        JointUnion::Parallel(_) => {
            // Remove hinge angle
            let mut rel_q = rel_q;
            rel_q.v.z = 0.0;
            get_quat_angle(rel_q)
        }

        JointUnion::Distance(_) => 0.0,

        JointUnion::Motor(_) => 0.0,

        JointUnion::Filter => 0.0,

        JointUnion::Prismatic(_) => get_quat_angle(rel_q),

        JointUnion::Revolute(revolute_joint) => {
            if revolute_joint.enable_limit {
                let angle = get_twist_angle(rel_q);
                if angle < revolute_joint.lower_angle {
                    return get_quat_angle(rel_q);
                }

                if revolute_joint.upper_angle < angle {
                    return get_quat_angle(rel_q);
                }
            }

            // Remove hinge angle
            let mut rel_q = rel_q;
            rel_q.v.z = 0.0;
            get_quat_angle(rel_q)
        }

        JointUnion::Spherical(spherical_joint) => {
            let mut sum = 0.0;
            if spherical_joint.enable_cone_limit {
                let swing_angle = get_swing_angle(rel_q);
                sum += max_float(0.0, swing_angle - spherical_joint.cone_angle);
            }

            if spherical_joint.enable_twist_limit {
                let twist_angle = get_twist_angle(rel_q);
                sum += max_float(0.0, spherical_joint.lower_twist_angle - twist_angle);
                sum += max_float(0.0, twist_angle - spherical_joint.upper_twist_angle);
            }

            sum
        }

        JointUnion::Weld(weld_joint) => {
            if weld_joint.angular_hertz == 0.0 {
                return get_quat_angle(rel_q);
            }

            0.0
        }

        JointUnion::Wheel(_) => {
            // todo
            b3_assert!(false);
            0.0
        }
    }
}

pub fn prepare_joint(joint: &mut JointSim, world: &World, context: &StepContext) {
    // Clamp joint hertz based on the time step to reduce jitter.
    let hertz = min_float(joint.constraint_hertz, 0.25 * context.inv_h);
    joint.constraint_softness = make_soft(hertz, joint.constraint_damping_ratio, context.h);

    match joint.joint_type {
        JointType::Parallel => crate::parallel_joint::prepare_parallel_joint(joint, world, context),
        JointType::Distance => crate::distance_joint::prepare_distance_joint(joint, world, context),
        JointType::Filter => {}
        JointType::Motor => crate::motor_joint::prepare_motor_joint(joint, world, context),
        JointType::Prismatic => crate::prismatic_joint::prepare_prismatic_joint(joint, world, context),
        JointType::Revolute => crate::revolute_joint::prepare_revolute_joint(joint, world, context),
        JointType::Spherical => crate::spherical_joint::prepare_spherical_joint(joint, world, context),
        JointType::Weld => crate::weld_joint::prepare_weld_joint(joint, world, context),
        JointType::Wheel => crate::wheel_joint::prepare_wheel_joint(joint, world, context),
    }
}

pub fn warm_start_joint(joint: &mut JointSim, context: &mut StepContext) {
    match joint.joint_type {
        JointType::Parallel => crate::parallel_joint::warm_start_parallel_joint(joint, context),
        JointType::Distance => crate::distance_joint::warm_start_distance_joint(joint, context),
        JointType::Filter => {}
        JointType::Motor => crate::motor_joint::warm_start_motor_joint(joint, context),
        JointType::Prismatic => crate::prismatic_joint::warm_start_prismatic_joint(joint, context),
        JointType::Revolute => crate::revolute_joint::warm_start_revolute_joint(joint, context),
        JointType::Spherical => crate::spherical_joint::warm_start_spherical_joint(joint, context),
        JointType::Weld => crate::weld_joint::warm_start_weld_joint(joint, context),
        JointType::Wheel => crate::wheel_joint::warm_start_wheel_joint(joint, context),
    }
}

pub fn solve_joint(joint: &mut JointSim, context: &mut StepContext, use_bias: bool) {
    match joint.joint_type {
        JointType::Parallel => crate::parallel_joint::solve_parallel_joint(joint, context),
        JointType::Distance => crate::distance_joint::solve_distance_joint(joint, context, use_bias),
        JointType::Filter => {}
        JointType::Motor => crate::motor_joint::solve_motor_joint(joint, context),
        JointType::Prismatic => crate::prismatic_joint::solve_prismatic_joint(joint, context, use_bias),
        JointType::Revolute => crate::revolute_joint::solve_revolute_joint(joint, context, use_bias),
        JointType::Spherical => crate::spherical_joint::solve_spherical_joint(joint, context, use_bias),
        JointType::Weld => crate::weld_joint::solve_weld_joint(joint, context, use_bias),
        JointType::Wheel => crate::wheel_joint::solve_wheel_joint(joint, context, use_bias),
    }
}

// The overflow stages iterate the overflow color's joints. The Vec is moved out
// of the graph for the loop so the joints can be mutated while world is read.
pub fn prepare_joints_overflow(world: &mut World, context: &mut StepContext) {
    let mut joints =
        std::mem::take(&mut world.constraint_graph.colors[OVERFLOW_INDEX as usize].joint_sims);

    for joint in joints.iter_mut() {
        prepare_joint(joint, world, context);
    }

    world.constraint_graph.colors[OVERFLOW_INDEX as usize].joint_sims = joints;
}

pub fn warm_start_joints_overflow(world: &mut World, context: &mut StepContext) {
    let mut joints =
        std::mem::take(&mut world.constraint_graph.colors[OVERFLOW_INDEX as usize].joint_sims);

    for joint in joints.iter_mut() {
        warm_start_joint(joint, context);
    }

    world.constraint_graph.colors[OVERFLOW_INDEX as usize].joint_sims = joints;
}

pub fn solve_joints_overflow(world: &mut World, context: &mut StepContext, use_bias: bool) {
    let mut joints =
        std::mem::take(&mut world.constraint_graph.colors[OVERFLOW_INDEX as usize].joint_sims);

    for joint in joints.iter_mut() {
        solve_joint(joint, context, use_bias);
    }

    world.constraint_graph.colors[OVERFLOW_INDEX as usize].joint_sims = joints;
}

// b3DrawJoint (debug draw) is not ported.
