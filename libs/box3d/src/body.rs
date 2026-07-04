// Port of box3d/src/body.h + body.c
// Body organizational data, body lifecycle, and the public b3Body_* API.
// Deviations: b3DumpBody (debug dump file) is not ported; B3_REC recording
// hooks are not ported; names are Rust Strings truncated to BODY_NAME_LENGTH.

use crate::b3_assert;
use crate::b3_validate;
use crate::constants::{huge, speculative_distance, BODY_NAME_LENGTH};
use crate::container::array_remove_swap;
use crate::core::{NULL_INDEX, SECRET_COOKIE};
use crate::id::{BodyId, ContactId, JointId, ShapeId, WorldId, NULL_BODY_ID};
use crate::id_pool::{alloc_id, free_id};
use crate::math_functions::{
    aabb_contains, aabb_union, add, clamp_int, conjugate, cross, det, dot_quat, inv_rotate_vector,
    inv_transform_world_point, invert_t, is_valid_float, is_valid_matrix3, is_valid_position,
    is_valid_quat, is_valid_vec3, is_valid_world_transform, length, length_squared,
    make_matrix_from_quat, max, min_float, mul, mul_add, mul_mm, mul_mv, mul_quat, mul_sv,
    negate_quat, normalize, rotate_vector, sub, sub_pos, to_relative_transform, to_vec3,
    transform_world_point, transpose, vec3, Matrix3, Pos, Quat, Vec3, WorldTransform, AABB,
};
use crate::physics_world::{World, AWAKE_SET, DISABLED_SET, FIRST_SLEEPING_SET, STATIC_SET};
use crate::types::{
    BodyDef, BodyType, Capsule, ContactData, DistanceInput, MassData, MotionLocks, QueryFilter,
    RayCastInput, ShapeCastInput, ShapeProxy, SimplexCache, Sweep, BodyCastResult,
    BodyPlaneResult, PlaneResult, ShapeType,
};

// b3BodyFlags
pub const LOCK_LINEAR_X: u32 = 0x00000001;
pub const LOCK_LINEAR_Y: u32 = 0x00000002;
pub const LOCK_LINEAR_Z: u32 = 0x00000004;
pub const LOCK_ANGULAR_X: u32 = 0x00000008;
pub const LOCK_ANGULAR_Y: u32 = 0x00000010;
pub const LOCK_ANGULAR_Z: u32 = 0x00000020;
// This flag is used for debug draw
pub const IS_FAST: u32 = 0x00000040;
// This dynamic body does a final CCD pass against all body types, but not other bullets
pub const IS_BULLET: u32 = 0x00000080;
// This body was speed capped in the current time step
pub const IS_SPEED_CAPPED: u32 = 0x00000100;
// This body had a time of impact event in the current time step
pub const HAD_TIME_OF_IMPACT: u32 = 0x00000200;
// This body has no limit on angular velocity
pub const ALLOW_FAST_ROTATION: u32 = 0x00000400;
// This body needs to have its AABB increased
pub const ENLARGE_BOUNDS: u32 = 0x00000800;
// This body is dynamic so the solver should write to it.
pub const DYNAMIC_FLAG: u32 = 0x00001000;
pub const ENABLE_SLEEP: u32 = 0x00002000;
pub const BODY_ENABLE_CONTACT_RECYCLING: u32 = 0x00004000;

// All lock flags
pub const ALL_LOCKS: u32 =
    LOCK_LINEAR_X | LOCK_LINEAR_Y | LOCK_LINEAR_Z | LOCK_ANGULAR_X | LOCK_ANGULAR_Y | LOCK_ANGULAR_Z;
// If all these flags are set then the body has fixed rotation
pub const FIXED_ROTATION: u32 = LOCK_ANGULAR_X | LOCK_ANGULAR_Y | LOCK_ANGULAR_Z;
// These flags are transient per time step.
pub const BODY_TRANSIENT_FLAGS: u32 = IS_FAST | IS_SPEED_CAPPED | HAD_TIME_OF_IMPACT;

/// Body organizational details that are not used in the solver.
#[derive(Clone, Debug, Default)]
pub struct Body {
    pub user_data: u64,

    /// index of solver set stored in World. May be NULL_INDEX.
    pub set_index: i32,

    /// body sim and state index within set. May be NULL_INDEX.
    pub local_index: i32,

    /// [31 : contactId | 1 : edgeIndex]
    pub head_contact_key: i32,
    pub contact_count: i32,

    pub head_shape_id: i32,
    pub shape_count: i32,

    pub head_chain_id: i32,

    /// [31 : jointId | 1 : edgeIndex]
    pub head_joint_key: i32,
    pub joint_count: i32,

    /// All enabled dynamic and kinematic bodies are in an island.
    pub island_id: i32,

    /// Index into the island's bodies array for O(1) swap-removal.
    /// NULL_INDEX when not in an island.
    pub island_index: i32,

    pub sleep_threshold: f32,
    pub sleep_time: f32,

    pub mass: f32,

    /// local space inertia
    pub inertia: Matrix3,

    /// this is used to adjust the fell_asleep flag in the body move array
    pub body_move_index: i32,

    pub id: i32,

    /// Body flags (consts above)
    pub flags: u32,

    pub body_type: BodyType,

    /// Monotonically advanced when a body is allocated in this slot.
    /// Used to check for invalid BodyId.
    pub generation: u16,

    pub name: String,
}

/// Body State
/// The body state is designed for the performance critical constraint solver.
/// Only awake dynamic and kinematic bodies have a body state.
#[derive(Clone, Copy, Debug)]
pub struct BodyState {
    pub linear_velocity: Vec3,
    pub angular_velocity: Vec3,

    /// Using delta position reduces round-off error far from the origin
    pub delta_position: Vec3,

    /// Using delta rotation because the solver cannot access the full rotation
    /// on static bodies and must use zero delta rotation for static bodies
    pub delta_rotation: Quat,

    /// Body flags. Important flags: locking, dynamic
    pub flags: u32,
}

/// Identity body state, notice the delta_rotation is identity.
pub const IDENTITY_BODY_STATE: BodyState = BodyState {
    linear_velocity: Vec3::ZERO,
    angular_velocity: Vec3::ZERO,
    delta_position: Vec3::ZERO,
    delta_rotation: Quat::IDENTITY,
    flags: 0,
};

impl Default for BodyState {
    fn default() -> Self {
        IDENTITY_BODY_STATE
    }
}

/// Body simulation data used for integration of position and velocity.
/// Transform data used for collision and solver preparation.
#[derive(Clone, Copy, Debug, Default)]
pub struct BodySim {
    /// transform for body origin
    pub transform: WorldTransform,

    /// center of mass position in world space
    pub center: Pos,

    /// previous rotation and COM for TOI
    pub rotation0: Quat,
    pub center0: Pos,

    /// location of center of mass relative to the body origin
    pub local_center: Vec3,

    pub force: Vec3,
    pub torque: Vec3,

    pub inv_mass: f32,

    /// Rotational inertia about the center of mass. The world space inverse inertia
    /// tensor must be updated whenever the body rotation is modified.
    pub inv_inertia_local: Matrix3,
    pub inv_inertia_world: Matrix3,

    pub min_extent: f32,
    pub max_extent: Vec3,
    pub max_angular_velocity: f32,
    pub linear_damping: f32,
    pub angular_damping: f32,
    pub gravity_scale: f32,

    /// Index of Body
    pub body_id: i32,

    /// Body flags
    pub flags: u32,
}

/// Make a sweep relative to a base position to keep TOI in float precision far from the origin.
#[inline]
pub fn make_relative_sweep(body_sim: &BodySim, base: Pos) -> Sweep {
    Sweep {
        c1: crate::math_functions::sub_pos(body_sim.center0, base),
        c2: crate::math_functions::sub_pos(body_sim.center, base),
        q1: body_sim.rotation0,
        q2: body_sim.transform.q,
        local_center: body_sim.local_center,
    }
}

// ---------------------------------------------------------------------------
// body.c
// ---------------------------------------------------------------------------

/// C strncpy truncation to BODY_NAME_LENGTH, respecting UTF-8 boundaries.
fn truncate_name(s: &str) -> String {
    let mut end = s.len().min(BODY_NAME_LENGTH);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

/// Get a validated body index from a world using an id.
/// C returns b3Body*; the port returns the raw body index.
pub fn get_body_full_id(world: &World, body_id: BodyId) -> i32 {
    b3_assert!(body_id.is_non_null());
    b3_assert!(body_id.world0 == world.world_id);
    b3_assert!(1 <= body_id.index1 && body_id.index1 <= world.bodies.len() as i32);

    // id index starts at one so that zero can represent null
    let index = body_id.index1 - 1;
    let body = &world.bodies[index as usize];
    b3_assert!(body.set_index != NULL_INDEX);
    b3_assert!(body.generation == body_id.generation);
    index
}

pub fn get_body_transform_quick(world: &World, body_id: i32) -> WorldTransform {
    let body = &world.bodies[body_id as usize];
    let set = &world.solver_sets[body.set_index as usize];
    set.body_sims[body.local_index as usize].transform
}

pub fn get_body_transform(world: &World, body_id: i32) -> WorldTransform {
    get_body_transform_quick(world, body_id)
}

/// Create a BodyId from a raw id.
pub fn make_body_id(world: &World, body_id: i32) -> BodyId {
    let body = &world.bodies[body_id as usize];
    BodyId { index1: body_id + 1, world0: world.world_id, generation: body.generation }
}

pub fn get_body_sim_from_id(world: &World, body_id: i32) -> &BodySim {
    let body = &world.bodies[body_id as usize];
    let set = &world.solver_sets[body.set_index as usize];
    &set.body_sims[body.local_index as usize]
}

pub fn get_body_sim_from_id_mut(world: &mut World, body_id: i32) -> &mut BodySim {
    let body = &world.bodies[body_id as usize];
    let (set_index, local_index) = (body.set_index, body.local_index);
    let set = &mut world.solver_sets[set_index as usize];
    &mut set.body_sims[local_index as usize]
}

/// C b3GetBodyState: NULL unless the body is in the awake set.
pub fn get_body_state_from_id_mut(world: &mut World, body_id: i32) -> Option<&mut BodyState> {
    let body = &world.bodies[body_id as usize];
    if body.set_index == AWAKE_SET {
        let local_index = body.local_index;
        let set = &mut world.solver_sets[AWAKE_SET as usize];
        Some(&mut set.body_states[local_index as usize])
    } else {
        None
    }
}

fn sync_body_flags(world: &mut World, body_id: i32) {
    // Never sync transient flags
    let flags = world.bodies[body_id as usize].flags & !BODY_TRANSIENT_FLAGS;

    let body_sim = get_body_sim_from_id_mut(world, body_id);
    body_sim.flags = flags;

    if let Some(body_state) = get_body_state_from_id_mut(world, body_id) {
        body_state.flags = flags;
    }
}

fn create_island_for_body(world: &mut World, set_index: i32, body_id: i32) {
    b3_assert!(world.bodies[body_id as usize].island_id == NULL_INDEX);
    b3_assert!(set_index != DISABLED_SET);

    let island_id = crate::island::create_island(world, set_index);
    world.islands[island_id as usize].bodies.push(body_id);
    let body = &mut world.bodies[body_id as usize];
    body.island_id = island_id;
    body.island_index = 0;

    crate::island::validate_island(world, island_id);
}

fn remove_body_from_island(world: &mut World, body_id: i32) {
    let (island_id, body_island_index) = {
        let body = &world.bodies[body_id as usize];
        (body.island_id, body.island_index)
    };

    if island_id == NULL_INDEX {
        b3_assert!(world.bodies[body_id as usize].island_index == NULL_INDEX);
        return;
    }

    {
        let local_index = body_island_index;
        let island = &mut world.islands[island_id as usize];
        let count = island.bodies.len();
        let moved_body_id = island.bodies[count - 1];
        island.bodies[local_index as usize] = moved_body_id;
        b3_validate!(world.bodies[moved_body_id as usize].island_index == count as i32 - 1);
        world.bodies[moved_body_id as usize].island_index = local_index;
        world.islands[island_id as usize].bodies.truncate(count - 1);
    }

    let island = &world.islands[island_id as usize];
    if island.bodies.is_empty() {
        // Destroy empty island
        b3_assert!(island.contacts.is_empty());
        b3_assert!(island.joints.is_empty());

        // Free the island
        crate::island::destroy_island(world, island_id);
    } else {
        crate::island::validate_island(world, island_id);
    }

    let body = &mut world.bodies[body_id as usize];
    body.island_id = NULL_INDEX;
    body.island_index = NULL_INDEX;
}

fn destroy_body_contacts(world: &mut World, body_id: i32, wake_bodies: bool) {
    // Destroy the attached contacts
    let mut edge_key = world.bodies[body_id as usize].head_contact_key;
    while edge_key != NULL_INDEX {
        let contact_id = edge_key >> 1;
        let edge_index = edge_key & 1;

        edge_key = world.contacts[contact_id as usize].edges[edge_index as usize].next_key;
        crate::contact::destroy_contact(world, contact_id, wake_bodies);
    }

    crate::physics_world::validate_solver_sets(world);
}

pub fn create_body(world: &mut World, def: &BodyDef) -> BodyId {
    b3_assert!(def.internal_value == SECRET_COOKIE);
    b3_assert!(is_valid_position(def.position));
    b3_assert!(is_valid_quat(def.rotation));
    b3_assert!(is_valid_vec3(def.linear_velocity));
    b3_assert!(is_valid_vec3(def.angular_velocity));
    b3_assert!(is_valid_float(def.linear_damping) && def.linear_damping >= 0.0);
    b3_assert!(is_valid_float(def.angular_damping) && def.angular_damping >= 0.0);
    b3_assert!(is_valid_float(def.sleep_threshold) && def.sleep_threshold >= 0.0);
    b3_assert!(is_valid_float(def.gravity_scale));

    if world.locked {
        b3_assert!(!world.locked);
        return NULL_BODY_ID;
    }

    world.locked = true;

    let is_awake = (def.is_awake || !def.enable_sleep) && def.is_enabled;

    // determine the solver set
    let set_id;
    if !def.is_enabled {
        // any body type can be disabled
        set_id = DISABLED_SET;
    } else if def.body_type == BodyType::Static {
        set_id = STATIC_SET;
    } else if is_awake {
        set_id = AWAKE_SET;
    } else {
        // new set for a sleeping body in its own island
        set_id = alloc_id(&mut world.solver_set_id_pool);
        if set_id == world.solver_sets.len() as i32 {
            // Create a zero initialized solver set. All sub-arrays are also zero initialized.
            world.solver_sets.push(crate::solver_set::SolverSet::default());
            world.solver_sets[set_id as usize].set_index = NULL_INDEX;
        } else {
            b3_assert!(world.solver_sets[set_id as usize].set_index == NULL_INDEX);
        }

        world.solver_sets[set_id as usize].set_index = set_id;
    }

    b3_assert!(0 <= set_id && set_id < world.solver_sets.len() as i32);

    let body_id = alloc_id(&mut world.body_id_pool);

    let mut lock_flags: u32 = 0;
    lock_flags |= if def.motion_locks.linear_x { LOCK_LINEAR_X } else { 0 };
    lock_flags |= if def.motion_locks.linear_y { LOCK_LINEAR_Y } else { 0 };
    lock_flags |= if def.motion_locks.linear_z { LOCK_LINEAR_Z } else { 0 };
    lock_flags |= if def.motion_locks.angular_x { LOCK_ANGULAR_X } else { 0 };
    lock_flags |= if def.motion_locks.angular_y { LOCK_ANGULAR_Y } else { 0 };
    lock_flags |= if def.motion_locks.angular_z { LOCK_ANGULAR_Z } else { 0 };

    let local_index;
    let sim_flags;
    {
        let set = &mut world.solver_sets[set_id as usize];
        let mut body_sim = BodySim::default();
        body_sim.transform.p = def.position;
        body_sim.transform.q = def.rotation;
        body_sim.center = def.position;
        body_sim.rotation0 = body_sim.transform.q;
        body_sim.center0 = body_sim.center;
        body_sim.local_center = Vec3::ZERO;
        body_sim.force = Vec3::ZERO;
        body_sim.torque = Vec3::ZERO;
        body_sim.inv_mass = 0.0;
        body_sim.inv_inertia_local = Matrix3::ZERO;
        body_sim.min_extent = huge();
        body_sim.max_extent = Vec3::ZERO;
        body_sim.linear_damping = def.linear_damping;
        body_sim.angular_damping = def.angular_damping;
        body_sim.gravity_scale = def.gravity_scale;
        body_sim.body_id = body_id;
        body_sim.flags = lock_flags;
        body_sim.flags |= if def.is_bullet { IS_BULLET } else { 0 };
        body_sim.flags |= if def.allow_fast_rotation { ALLOW_FAST_ROTATION } else { 0 };
        body_sim.flags |= if def.body_type == BodyType::Dynamic { DYNAMIC_FLAG } else { 0 };
        body_sim.flags |= if def.enable_sleep { ENABLE_SLEEP } else { 0 };
        body_sim.flags |= if def.enable_contact_recycling { BODY_ENABLE_CONTACT_RECYCLING } else { 0 };

        sim_flags = body_sim.flags;

        if set_id == AWAKE_SET {
            let mut body_state = BodyState::default();
            body_state.linear_velocity = def.linear_velocity;
            body_state.angular_velocity = def.angular_velocity;
            body_state.delta_rotation = Quat::IDENTITY;
            body_state.flags = body_sim.flags;
            set.body_states.push(body_state);

            body_sim.max_angular_velocity = length(def.angular_velocity) + 5.0;
        }

        set.body_sims.push(body_sim);
        local_index = set.body_sims.len() as i32 - 1;
    }

    if body_id == world.bodies.len() as i32 {
        world.bodies.push(Body::default());
        // Fresh slots start with id 0 from Default; mark free like a recycled slot.
        world.bodies[body_id as usize].id = NULL_INDEX;
        world.bodies[body_id as usize].set_index = NULL_INDEX;
    } else {
        b3_assert!(world.bodies[body_id as usize].id == NULL_INDEX);
    }

    let generation;
    {
        let body = &mut world.bodies[body_id as usize];

        body.name = truncate_name(&def.name);

        body.user_data = def.user_data;
        body.set_index = set_id;
        body.local_index = local_index;
        body.generation = body.generation.wrapping_add(1);
        body.head_shape_id = NULL_INDEX;
        body.shape_count = 0;
        body.head_chain_id = NULL_INDEX;
        body.head_contact_key = NULL_INDEX;
        body.contact_count = 0;
        body.head_joint_key = NULL_INDEX;
        body.joint_count = 0;
        body.island_id = NULL_INDEX;
        body.island_index = NULL_INDEX;
        body.body_move_index = NULL_INDEX;
        body.id = body_id;
        body.sleep_threshold = def.sleep_threshold;
        body.sleep_time = 0.0;
        body.mass = 0.0;
        body.inertia = Matrix3::ZERO;
        body.body_type = def.body_type;
        body.flags = sim_flags;

        generation = body.generation;
    }

    // dynamic and kinematic bodies that are enabled need an island
    if set_id >= AWAKE_SET {
        create_island_for_body(world, set_id, body_id);
    }

    crate::physics_world::validate_solver_sets(world);

    let id = BodyId { index1: body_id + 1, world0: world.world_id, generation };

    world.locked = false;

    id
}

pub fn is_body_awake(world: &World, body_id: i32) -> bool {
    world.bodies[body_id as usize].set_index == AWAKE_SET
}

/// careful calling this because it can invalidate body, state, joint, and contact indices
pub fn wake_body(world: &mut World, body_id: i32) -> bool {
    let set_index = world.bodies[body_id as usize].set_index;
    if set_index >= FIRST_SLEEPING_SET {
        crate::solver_set::wake_solver_set(world, set_index);
        crate::physics_world::validate_solver_sets(world);
        return true;
    }

    false
}

pub fn wake_body_with_lock(world: &mut World, body_id: i32) -> bool {
    b3_assert!(!world.locked);
    world.locked = true;
    let woke = wake_body(world, body_id);
    world.locked = false;
    woke
}

pub fn destroy_body(world: &mut World, body_id: BodyId) {
    if world.locked {
        b3_assert!(!world.locked);
        return;
    }

    world.locked = true;

    let body_index = get_body_full_id(world, body_id);

    // Wake bodies attached to this body, even if this body is static.
    let wake_bodies = true;

    // Destroy the attached joints
    let mut edge_key = world.bodies[body_index as usize].head_joint_key;
    while edge_key != NULL_INDEX {
        let joint_id = edge_key >> 1;
        let edge_index = edge_key & 1;

        edge_key = world.joints[joint_id as usize].edges[edge_index as usize].next_key;

        // Careful because this modifies the list being traversed
        crate::joint::destroy_joint_internal(world, joint_id, wake_bodies);
    }

    // Destroy all contacts attached to this body.
    destroy_body_contacts(world, body_index, wake_bodies);

    // Destroy the attached shapes and their broad-phase proxies.
    let mut shape_id = world.bodies[body_index as usize].head_shape_id;
    while shape_id != NULL_INDEX {
        if world.shapes[shape_id as usize].sensor_index != NULL_INDEX {
            crate::sensor::destroy_sensor(world, shape_id);
        }

        crate::shape::destroy_shape_proxy(world, shape_id);

        crate::shape::destroy_shape_allocations(world, shape_id);

        // Return shape to free list.
        free_id(&mut world.shape_id_pool, shape_id);
        let next_shape_id = world.shapes[shape_id as usize].next_shape_id;
        world.shapes[shape_id as usize].id = NULL_INDEX;

        shape_id = next_shape_id;
    }

    remove_body_from_island(world, body_index);

    // Remove body sim from solver set that owns it
    let (set_index, local_index) = {
        let body = &world.bodies[body_index as usize];
        (body.set_index, body.local_index)
    };
    let moved_index = array_remove_swap(&mut world.solver_sets[set_index as usize].body_sims, local_index);
    if moved_index != NULL_INDEX {
        // Fix moved body index
        let moved_id = world.solver_sets[set_index as usize].body_sims[local_index as usize].body_id;
        b3_assert!(world.bodies[moved_id as usize].local_index == moved_index);
        world.bodies[moved_id as usize].local_index = local_index;
    }

    // Remove body state from awake set
    if set_index == AWAKE_SET {
        let result = array_remove_swap(&mut world.solver_sets[set_index as usize].body_states, local_index);
        let _ = result;
        b3_assert!(result == moved_index);
    } else if set_index >= FIRST_SLEEPING_SET && world.solver_sets[set_index as usize].body_sims.is_empty() {
        // Remove solver set if it's now an orphan.
        crate::solver_set::destroy_solver_set(world, set_index);
    }

    // Free body and id (preserve body revision)
    free_id(&mut world.body_id_pool, body_index);

    let body = &mut world.bodies[body_index as usize];
    body.set_index = NULL_INDEX;
    body.local_index = NULL_INDEX;
    body.id = NULL_INDEX;

    crate::physics_world::validate_solver_sets(world);

    world.locked = false;
}

pub fn body_get_contact_capacity(world: &World, body_id: BodyId) -> i32 {
    if world.locked {
        b3_assert!(!world.locked);
        return 0;
    }

    let body = &world.bodies[get_body_full_id(world, body_id) as usize];

    // Conservative and fast
    body.contact_count
}

/// C fills a caller array; the port clears and fills a Vec up to capacity.
pub fn body_get_contact_data<'a>(
    world: &'a World,
    body_id: BodyId,
    contact_data: &mut Vec<ContactData<'a>>,
    capacity: i32,
) -> i32 {
    contact_data.clear();

    if world.locked {
        b3_assert!(!world.locked);
        return 0;
    }

    let body_index = get_body_full_id(world, body_id);
    let body = &world.bodies[body_index as usize];

    let mut contact_key = body.head_contact_key;
    let mut index = 0;
    while contact_key != NULL_INDEX && index < capacity {
        let contact_id = contact_key >> 1;
        let edge_index = contact_key & 1;

        let contact = &world.contacts[contact_id as usize];

        // Is contact touching?
        if contact.flags & crate::contact::CONTACT_TOUCHING_FLAG != 0 {
            let shape_a = &world.shapes[contact.shape_id_a as usize];
            let shape_b = &world.shapes[contact.shape_id_b as usize];

            contact_data.push(ContactData {
                contact_id: ContactId {
                    index1: contact.contact_id + 1,
                    world0: body_id.world0,
                    generation: contact.generation,
                },
                shape_id_a: ShapeId { index1: shape_a.id + 1, world0: body_id.world0, generation: shape_a.generation },
                shape_id_b: ShapeId { index1: shape_b.id + 1, world0: body_id.world0, generation: shape_b.generation },
                manifolds: &contact.manifolds,
            });
            index += 1;
        }

        contact_key = contact.edges[edge_index as usize].next_key;
    }

    b3_assert!(index <= capacity);

    index
}

pub fn body_compute_aabb(world: &World, body_id: BodyId) -> AABB {
    if world.locked {
        b3_assert!(!world.locked);
        return AABB::default();
    }

    let body_index = get_body_full_id(world, body_id);
    let body = &world.bodies[body_index as usize];
    if body.head_shape_id == NULL_INDEX {
        let transform = get_body_transform(world, body_index);
        let p = to_vec3(transform.p);
        return AABB { lower_bound: p, upper_bound: p };
    }

    let mut shape = &world.shapes[body.head_shape_id as usize];
    let mut aabb = shape.aabb;
    while shape.next_shape_id != NULL_INDEX {
        shape = &world.shapes[shape.next_shape_id as usize];
        aabb = aabb_union(aabb, shape.aabb);
    }

    aabb
}

pub fn body_get_closest_point(world: &World, body_id: BodyId, result: &mut Vec3, target: Vec3) -> f32 {
    if world.locked {
        b3_assert!(!world.locked);
        *result = Vec3::ZERO;
        return 0.0;
    }

    let body_index = get_body_full_id(world, body_id);
    let world_transform = get_body_transform(world, body_index);
    let transform = to_relative_transform(world_transform, crate::math_functions::POS_ZERO);

    let mut closest_distance = f32::MAX;
    let mut closest_point = transform.p;

    let target_points = [target];

    let mut shape_id = world.bodies[body_index as usize].head_shape_id;
    while shape_id != NULL_INDEX {
        let shape = &world.shapes[shape_id as usize];
        shape_id = shape.next_shape_id;

        let shape_type = shape.shape_type();
        if shape_type != ShapeType::Sphere && shape_type != ShapeType::Capsule && shape_type != ShapeType::Hull {
            continue;
        }

        let mut proxy_buffer = [crate::math_functions::Vec3::ZERO; 2];
        let input = DistanceInput {
            proxy_a: ShapeProxy { points: &target_points, radius: 0.0 },
            proxy_b: crate::shape::make_shape_proxy(shape, &mut proxy_buffer),
            // Target rides in frame A at the origin, so the relative pose of the shape in A is the body transform
            transform,
            use_radii: false,
        };

        let mut cache = SimplexCache::default();
        let output = crate::distance::shape_distance(&input, &mut cache, None);
        if output.distance < closest_distance {
            closest_distance = output.distance;
            closest_point = output.point_b;
        }
    }

    *result = closest_point;
    closest_distance
}

pub fn body_cast_ray(
    world: &World,
    body_id: BodyId,
    origin: Pos,
    translation: Vec3,
    filter: QueryFilter,
    max_fraction: f32,
    body_transform: WorldTransform,
) -> BodyCastResult {
    if world.locked {
        b3_assert!(!world.locked);
        return BodyCastResult::default();
    }

    let mut result = BodyCastResult::default();
    let body_index = get_body_full_id(world, body_id);

    // The consistent framing is to center on the ray origin.
    let mut shape_input = RayCastInput {
        origin: Vec3::ZERO,
        translation,
        max_fraction,
    };

    let transform = to_relative_transform(body_transform, origin);

    let mut shape_id = world.bodies[body_index as usize].head_shape_id;
    while shape_id != NULL_INDEX {
        let shape = &world.shapes[shape_id as usize];
        shape_id = shape.next_shape_id;

        if !crate::shape::should_query_collide(shape.filter, filter) {
            continue;
        }

        let shape_output = crate::shape::ray_cast_shape(shape, transform, &shape_input);

        if !shape_output.hit {
            continue;
        }

        if shape_output.fraction > shape_input.max_fraction {
            continue;
        }

        // Careful with id, shape_id is the next shape.
        let id = ShapeId { index1: shape.id + 1, world0: body_id.world0, generation: shape.generation };

        let materials = crate::shape::get_shape_materials(shape);
        let material_index = clamp_int(shape_output.material_index, 0, materials.len() as i32 - 1);
        let user_material_id = materials[material_index as usize].user_material_id;

        result = BodyCastResult {
            shape_id: id,
            point: crate::math_functions::offset_pos(origin, shape_output.point),
            normal: shape_output.normal,
            fraction: shape_output.fraction,
            triangle_index: shape_output.triangle_index,
            user_material_id,
            iterations: shape_output.iterations,
            hit: true,
        };

        shape_input.max_fraction = shape_output.fraction;
    }

    result
}

pub fn body_cast_shape(
    world: &World,
    body_id: BodyId,
    origin: Pos,
    proxy: &ShapeProxy,
    translation: Vec3,
    filter: QueryFilter,
    max_fraction: f32,
    can_encroach: bool,
    body_transform: WorldTransform,
) -> BodyCastResult {
    if world.locked {
        b3_assert!(!world.locked);
        return BodyCastResult::default();
    }

    let mut result = BodyCastResult::default();
    let body_index = get_body_full_id(world, body_id);

    let transform = to_relative_transform(body_transform, origin);

    let mut shape_input = ShapeCastInput {
        proxy: *proxy,
        translation,
        max_fraction,
        can_encroach,
    };

    let mut shape_id = world.bodies[body_index as usize].head_shape_id;
    while shape_id != NULL_INDEX {
        let shape = &world.shapes[shape_id as usize];
        shape_id = shape.next_shape_id;

        if !crate::shape::should_query_collide(shape.filter, filter) {
            continue;
        }

        let shape_output = crate::shape::shape_cast_shape(shape, transform, &shape_input);

        if !shape_output.hit {
            continue;
        }

        if shape_output.fraction > shape_input.max_fraction {
            continue;
        }

        // Careful with id, shape_id is the next shape.
        let id = ShapeId { index1: shape.id + 1, world0: body_id.world0, generation: shape.generation };
        let materials = crate::shape::get_shape_materials(shape);
        let material_index = clamp_int(shape_output.material_index, 0, materials.len() as i32 - 1);
        let user_material_id = materials[material_index as usize].user_material_id;

        result = BodyCastResult {
            shape_id: id,
            point: crate::math_functions::offset_pos(origin, shape_output.point),
            normal: shape_output.normal,
            fraction: shape_output.fraction,
            triangle_index: shape_output.triangle_index,
            user_material_id,
            iterations: shape_output.iterations,
            hit: true,
        };

        shape_input.max_fraction = shape_output.fraction;
    }

    result
}

pub fn body_overlap_shape(
    world: &World,
    body_id: BodyId,
    origin: Pos,
    proxy: &ShapeProxy,
    filter: QueryFilter,
    body_transform: WorldTransform,
) -> bool {
    if world.locked {
        b3_assert!(!world.locked);
        return false;
    }

    let body_index = get_body_full_id(world, body_id);
    let transform = to_relative_transform(body_transform, origin);

    let mut shape_id = world.bodies[body_index as usize].head_shape_id;
    while shape_id != NULL_INDEX {
        let shape = &world.shapes[shape_id as usize];
        shape_id = shape.next_shape_id;

        if !crate::shape::should_query_collide(shape.filter, filter) {
            continue;
        }

        let overlaps = crate::shape::overlap_shape(shape, transform, proxy);
        if overlaps {
            return true;
        }
    }

    false
}

/// C takes a plane array + capacity; the port uses the slice length as capacity.
pub fn body_collide_mover(
    world: &World,
    body_id: BodyId,
    body_planes: &mut [BodyPlaneResult],
    origin: Pos,
    mover: &Capsule,
    filter: QueryFilter,
    body_transform: WorldTransform,
) -> i32 {
    if world.locked {
        b3_assert!(!world.locked);
        return 0;
    }

    let plane_capacity = body_planes.len() as i32;
    if plane_capacity == 0 {
        return 0;
    }

    let mut result_count = 0;
    let body_index = get_body_full_id(world, body_id);

    let transform = to_relative_transform(body_transform, origin);

    let mut shape_id = world.bodies[body_index as usize].head_shape_id;
    while shape_id != NULL_INDEX {
        let shape = &world.shapes[shape_id as usize];
        shape_id = shape.next_shape_id;

        if !crate::shape::should_query_collide(shape.filter, filter) {
            continue;
        }

        let shape_type = shape.shape_type();
        if shape_type != ShapeType::Sphere && shape_type != ShapeType::Capsule && shape_type != ShapeType::Hull {
            continue;
        }

        let mut plane = [PlaneResult::default(); 1];
        let count = crate::shape::collide_mover(&mut plane, 1, shape, transform, mover);

        if count > 0 {
            let id = ShapeId { index1: shape.id + 1, world0: body_id.world0, generation: shape.generation };
            body_planes[result_count as usize] = BodyPlaneResult { shape_id: id, result: plane[0] };
            result_count += 1;
            if result_count == plane_capacity {
                return result_count;
            }
        }
    }

    result_count
}

pub fn update_body_mass_data(world: &mut World, body_id: i32) {
    // Compute mass data from shapes. Each shape has its own density.
    {
        let body = &mut world.bodies[body_id as usize];
        body.mass = 0.0;
        body.inertia = Matrix3::ZERO;
    }

    {
        let body_sim = get_body_sim_from_id_mut(world, body_id);
        body_sim.inv_mass = 0.0;
        body_sim.inv_inertia_local = Matrix3::ZERO;
        body_sim.inv_inertia_world = Matrix3::ZERO;
        body_sim.local_center = Vec3::ZERO;
        body_sim.min_extent = huge();
        body_sim.max_extent = Vec3::ZERO;
    }

    let (head_shape_id, shape_count, body_type) = {
        let body = &world.bodies[body_id as usize];
        (body.head_shape_id, body.shape_count, body.body_type)
    };

    if head_shape_id == NULL_INDEX {
        return;
    }

    // Static and kinematic sims have zero mass.
    if body_type != BodyType::Dynamic {
        {
            let body_sim = get_body_sim_from_id_mut(world, body_id);
            body_sim.center = body_sim.transform.p;
            body_sim.center0 = body_sim.center;
        }

        // Need extents for kinematic bodies for sleeping to work correctly.
        if body_type == BodyType::Kinematic {
            let mut min_extent = huge();
            let mut max_extent = Vec3::ZERO;

            let mut shape_id = head_shape_id;
            while shape_id != NULL_INDEX {
                let s = &world.shapes[shape_id as usize];

                let extent = crate::shape::compute_shape_extent(s, Vec3::ZERO);
                min_extent = min_float(min_extent, extent.min_extent);
                max_extent = max(max_extent, extent.max_extent);

                shape_id = s.next_shape_id;
            }

            let body_sim = get_body_sim_from_id_mut(world, body_id);
            body_sim.min_extent = min_extent;
            body_sim.max_extent = max_extent;
        }

        return;
    }

    let mut masses: Vec<MassData> = Vec::with_capacity(shape_count as usize);

    // Accumulate mass over all shapes.
    let mut total_mass = 0.0f32;
    let mut local_center = Vec3::ZERO;
    let mut shape_id = head_shape_id;
    while shape_id != NULL_INDEX {
        let s = &world.shapes[shape_id as usize];
        shape_id = s.next_shape_id;

        if s.density == 0.0 {
            masses.push(MassData::default());
            continue;
        }

        let mass_data = crate::shape::compute_shape_mass(s);
        total_mass += mass_data.mass;
        local_center = mul_add(local_center, mass_data.mass, mass_data.center);

        masses.push(mass_data);
    }

    world.bodies[body_id as usize].mass = total_mass;

    // Compute center of mass.
    let mut inv_mass = 0.0f32;
    if total_mass > 0.0 {
        inv_mass = 1.0 / total_mass;
        local_center = mul_sv(inv_mass, local_center);
    }

    // Second loop to accumulate the rotational inertia about the center of mass
    let mut body_inertia = Matrix3::ZERO;
    for mass_data in &masses {
        if mass_data.mass == 0.0 {
            continue;
        }

        // Shift to center of mass. This is safe because it can only increase.
        let offset = sub(local_center, mass_data.center);
        let inertia = crate::math_functions::add_mm(mass_data.inertia, crate::math_functions::steiner(mass_data.mass, offset));
        body_inertia = crate::math_functions::add_mm(body_inertia, inertia);
    }

    world.bodies[body_id as usize].inertia = body_inertia;
    drop(masses);

    let d = det(body_inertia);
    b3_assert!(d >= 0.0);

    let old_center;
    {
        let body_sim = get_body_sim_from_id_mut(world, body_id);
        body_sim.inv_mass = inv_mass;

        if d > 0.0 {
            // This call is faster than invert_matrix
            body_sim.inv_inertia_local = invert_t(body_inertia);

            let rotation_matrix = make_matrix_from_quat(body_sim.transform.q);
            body_sim.inv_inertia_world = mul_mm(mul_mm(rotation_matrix, body_sim.inv_inertia_local), transpose(rotation_matrix));
        }

        // Move center of mass.
        old_center = body_sim.center;
        body_sim.local_center = local_center;
        body_sim.center = transform_world_point(body_sim.transform, body_sim.local_center);
        body_sim.center0 = body_sim.center;
    }

    // Update center of mass velocity
    let new_center = get_body_sim_from_id(world, body_id).center;
    if let Some(state) = get_body_state_from_id_mut(world, body_id) {
        let delta_linear = cross(state.angular_velocity, sub_pos(new_center, old_center));
        state.linear_velocity = add(state.linear_velocity, delta_linear);
    }

    // Compute body extents relative to center of mass
    let mut min_extent = huge();
    let mut max_extent = Vec3::ZERO;
    let mut shape_id = head_shape_id;
    while shape_id != NULL_INDEX {
        let s = &world.shapes[shape_id as usize];

        let extent = crate::shape::compute_shape_extent(s, local_center);
        min_extent = min_float(min_extent, extent.min_extent);
        max_extent = max(max_extent, extent.max_extent);

        shape_id = s.next_shape_id;
    }

    {
        let body_sim = get_body_sim_from_id_mut(world, body_id);
        body_sim.min_extent = min_extent;
        body_sim.max_extent = max_extent;

        // Apply fixed rotation
        if (body_sim.flags & FIXED_ROTATION) == FIXED_ROTATION {
            body_sim.inv_inertia_local = Matrix3::ZERO;
            body_sim.inv_inertia_world = Matrix3::ZERO;
        }
    }
    if (world.bodies[body_id as usize].flags & FIXED_ROTATION) == FIXED_ROTATION {
        world.bodies[body_id as usize].inertia = Matrix3::ZERO;
    }
}

// b3DumpBody: not ported (debug dump file support).

pub fn body_get_position(world: &World, body_id: BodyId) -> Pos {
    let body_index = get_body_full_id(world, body_id);
    get_body_transform_quick(world, body_index).p
}

pub fn body_get_rotation(world: &World, body_id: BodyId) -> Quat {
    let body_index = get_body_full_id(world, body_id);
    get_body_transform_quick(world, body_index).q
}

pub fn body_get_transform(world: &World, body_id: BodyId) -> WorldTransform {
    let body_index = get_body_full_id(world, body_id);
    get_body_transform_quick(world, body_index)
}

pub fn body_get_local_point(world: &World, body_id: BodyId, world_point: Pos) -> Vec3 {
    let body_index = get_body_full_id(world, body_id);
    let transform = get_body_transform_quick(world, body_index);
    inv_transform_world_point(transform, world_point)
}

pub fn body_get_world_point(world: &World, body_id: BodyId, local_point: Vec3) -> Pos {
    let body_index = get_body_full_id(world, body_id);
    let transform = get_body_transform_quick(world, body_index);
    transform_world_point(transform, local_point)
}

pub fn body_get_local_vector(world: &World, body_id: BodyId, world_vector: Vec3) -> Vec3 {
    let body_index = get_body_full_id(world, body_id);
    let transform = get_body_transform_quick(world, body_index);
    inv_rotate_vector(transform.q, world_vector)
}

pub fn body_get_world_vector(world: &World, body_id: BodyId, local_vector: Vec3) -> Vec3 {
    let body_index = get_body_full_id(world, body_id);
    let transform = get_body_transform_quick(world, body_index);
    rotate_vector(transform.q, local_vector)
}

pub fn body_set_transform(world: &mut World, body_id: BodyId, position: Pos, rotation: Quat) {
    b3_assert!(is_valid_position(position));
    b3_assert!(is_valid_quat(rotation));
    b3_assert!(!world.locked);

    let body_index = get_body_full_id(world, body_id);

    let transform;
    {
        let body_sim = get_body_sim_from_id_mut(world, body_index);

        body_sim.transform.p = position;
        body_sim.transform.q = rotation;
        body_sim.center = transform_world_point(body_sim.transform, body_sim.local_center);

        let rotation_matrix = make_matrix_from_quat(body_sim.transform.q);
        body_sim.inv_inertia_world = mul_mm(mul_mm(rotation_matrix, body_sim.inv_inertia_local), transpose(rotation_matrix));

        body_sim.rotation0 = body_sim.transform.q;
        body_sim.center0 = body_sim.center;

        transform = body_sim.transform;
    }

    let speculative_distance = speculative_distance();

    let mut shape_id = world.bodies[body_index as usize].head_shape_id;
    while shape_id != NULL_INDEX {
        let aabb = crate::shape::compute_fat_shape_aabb(&world.shapes[shape_id as usize], transform, speculative_distance);
        world.shapes[shape_id as usize].aabb = aabb;

        if !aabb_contains(world.shapes[shape_id as usize].fat_aabb, aabb) {
            let margin = world.shapes[shape_id as usize].aabb_margin;
            let fat_aabb = AABB {
                lower_bound: vec3(aabb.lower_bound.x - margin, aabb.lower_bound.y - margin, aabb.lower_bound.z - margin),
                upper_bound: vec3(aabb.upper_bound.x + margin, aabb.upper_bound.y + margin, aabb.upper_bound.z + margin),
            };
            world.shapes[shape_id as usize].fat_aabb = fat_aabb;

            // The body could be disabled
            let proxy_key = world.shapes[shape_id as usize].proxy_key;
            if proxy_key != NULL_INDEX {
                crate::broad_phase::broad_phase_move_proxy(&mut world.broad_phase, proxy_key, fat_aabb);
            }
        }

        shape_id = world.shapes[shape_id as usize].next_shape_id;
    }
}

pub fn body_get_linear_velocity(world: &World, body_id: BodyId) -> Vec3 {
    let body_index = get_body_full_id(world, body_id);
    let body = &world.bodies[body_index as usize];
    if body.set_index == AWAKE_SET {
        let set = &world.solver_sets[AWAKE_SET as usize];
        return set.body_states[body.local_index as usize].linear_velocity;
    }
    Vec3::ZERO
}

pub fn body_get_angular_velocity(world: &World, body_id: BodyId) -> Vec3 {
    let body_index = get_body_full_id(world, body_id);
    let body = &world.bodies[body_index as usize];
    if body.set_index == AWAKE_SET {
        let set = &world.solver_sets[AWAKE_SET as usize];
        return set.body_states[body.local_index as usize].angular_velocity;
    }
    Vec3::ZERO
}

pub fn body_set_linear_velocity(world: &mut World, body_id: BodyId, linear_velocity: Vec3) {
    b3_assert!(is_valid_vec3(linear_velocity));

    let body_index = get_body_full_id(world, body_id);

    if world.bodies[body_index as usize].body_type == BodyType::Static {
        return;
    }

    if length_squared(linear_velocity) > 0.0 {
        wake_body_with_lock(world, body_index);
    }

    if let Some(state) = get_body_state_from_id_mut(world, body_index) {
        state.linear_velocity = linear_velocity;
    }
}

pub fn body_set_angular_velocity(world: &mut World, body_id: BodyId, angular_velocity: Vec3) {
    b3_assert!(is_valid_vec3(angular_velocity));

    let body_index = get_body_full_id(world, body_id);

    let (body_type, flags) = {
        let body = &world.bodies[body_index as usize];
        (body.body_type, body.flags)
    };

    if body_type == BodyType::Static {
        return;
    }

    // Apply locks to avoid waking
    let w = Vec3 {
        x: if flags & LOCK_ANGULAR_X != 0 { 0.0 } else { angular_velocity.x },
        y: if flags & LOCK_ANGULAR_Y != 0 { 0.0 } else { angular_velocity.y },
        z: if flags & LOCK_ANGULAR_Z != 0 { 0.0 } else { angular_velocity.z },
    };

    if length_squared(w) != 0.0 {
        wake_body_with_lock(world, body_index);
    }

    if let Some(state) = get_body_state_from_id_mut(world, body_index) {
        state.angular_velocity = w;
    }
}

pub fn body_set_target_transform(world: &mut World, body_id: BodyId, target: WorldTransform, time_step: f32, wake: bool) {
    b3_assert!(is_valid_world_transform(target));

    let body_index = get_body_full_id(world, body_id);

    let (set_index, body_type, sleep_threshold) = {
        let body = &world.bodies[body_index as usize];
        (body.set_index, body.body_type, body.sleep_threshold)
    };

    if set_index == DISABLED_SET {
        return;
    }

    if body_type == BodyType::Static || time_step <= 0.0 {
        return;
    }

    if set_index != AWAKE_SET && !wake {
        return;
    }

    let (center1, local_center, q1, max_extent) = {
        let sim = get_body_sim_from_id(world, body_index);
        (sim.center, sim.local_center, sim.transform.q, sim.max_extent)
    };

    // Compute linear velocity
    let center2 = transform_world_point(target, local_center);
    let inv_time_step = 1.0 / time_step;
    let linear_velocity = mul_sv(inv_time_step, sub_pos(center2, center1));

    // Compute angular velocity:
    // q' = 0.5 * w * q
    // <~> ( q2 - q1 ) / dt =  0.5 * w * q1
    // <=> w = 2 * ( q2 - q1 ) * Conjugate( q1 ) / dt
    let mut q2 = target.q;

    // Use the shortest arc quaternion
    if dot_quat(q1, q2) < 0.0 {
        q2 = negate_quat(q2);
    }

    let dq = Quat { v: sub(q2.v, q1.v), s: q2.s - q1.s };
    let omega = mul_quat(dq, conjugate(q1));
    let angular_velocity = mul_sv(2.0 * inv_time_step, omega.v);

    // Early out if the body is asleep already and the desired movement is small
    if set_index != AWAKE_SET {
        let max_velocity = length(linear_velocity) + length(mul(angular_velocity, max_extent));

        // Return if velocity would be sleepy
        if max_velocity < sleep_threshold {
            return;
        }

        // Must wake for state to exist
        wake_body_with_lock(world, body_index);
    }

    b3_assert!(world.bodies[body_index as usize].set_index == AWAKE_SET);

    let state = get_body_state_from_id_mut(world, body_index).unwrap();
    state.linear_velocity = linear_velocity;
    state.angular_velocity = angular_velocity;
}

pub fn body_get_local_point_velocity(world: &World, body_id: BodyId, local_point: Vec3) -> Vec3 {
    let body_index = get_body_full_id(world, body_id);
    let body = &world.bodies[body_index as usize];
    if body.set_index != AWAKE_SET {
        return Vec3::ZERO;
    }

    let set = &world.solver_sets[body.set_index as usize];
    let state = &set.body_states[body.local_index as usize];
    let body_sim = &set.body_sims[body.local_index as usize];

    let r = rotate_vector(body_sim.transform.q, sub(local_point, body_sim.local_center));
    add(state.linear_velocity, cross(state.angular_velocity, r))
}

pub fn body_get_world_point_velocity(world: &World, body_id: BodyId, world_point: Pos) -> Vec3 {
    let body_index = get_body_full_id(world, body_id);
    let body = &world.bodies[body_index as usize];
    if body.set_index != AWAKE_SET {
        return Vec3::ZERO;
    }

    let set = &world.solver_sets[body.set_index as usize];
    let state = &set.body_states[body.local_index as usize];
    let body_sim = &set.body_sims[body.local_index as usize];

    let r = sub_pos(world_point, body_sim.center);
    add(state.linear_velocity, cross(state.angular_velocity, r))
}

pub fn body_apply_force(world: &mut World, body_id: BodyId, force: Vec3, point: Pos, wake: bool) {
    b3_assert!(is_valid_vec3(force));

    let body_index = get_body_full_id(world, body_id);

    if wake && world.bodies[body_index as usize].set_index >= FIRST_SLEEPING_SET {
        wake_body_with_lock(world, body_index);
    }

    if world.bodies[body_index as usize].set_index == AWAKE_SET {
        let body_sim = get_body_sim_from_id_mut(world, body_index);
        body_sim.force = add(body_sim.force, force);
        body_sim.torque = add(body_sim.torque, cross(sub_pos(point, body_sim.center), force));
    }
}

pub fn body_apply_force_to_center(world: &mut World, body_id: BodyId, force: Vec3, wake: bool) {
    b3_assert!(is_valid_vec3(force));

    let body_index = get_body_full_id(world, body_id);

    if wake && world.bodies[body_index as usize].set_index >= FIRST_SLEEPING_SET {
        wake_body_with_lock(world, body_index);
    }

    if world.bodies[body_index as usize].set_index == AWAKE_SET {
        let body_sim = get_body_sim_from_id_mut(world, body_index);
        body_sim.force = add(body_sim.force, force);
    }
}

pub fn body_apply_torque(world: &mut World, body_id: BodyId, torque: Vec3, wake: bool) {
    b3_assert!(is_valid_vec3(torque));

    let body_index = get_body_full_id(world, body_id);

    if wake && world.bodies[body_index as usize].set_index >= FIRST_SLEEPING_SET {
        wake_body_with_lock(world, body_index);
    }

    if world.bodies[body_index as usize].set_index == AWAKE_SET {
        let body_sim = get_body_sim_from_id_mut(world, body_index);
        body_sim.torque = add(body_sim.torque, torque);
    }
}

pub fn body_apply_linear_impulse(world: &mut World, body_id: BodyId, impulse: Vec3, point: Pos, wake: bool) {
    b3_assert!(is_valid_vec3(impulse));
    b3_assert!(is_valid_position(point));

    let body_index = get_body_full_id(world, body_id);

    if wake && world.bodies[body_index as usize].set_index >= FIRST_SLEEPING_SET {
        wake_body_with_lock(world, body_index);
    }

    if world.bodies[body_index as usize].set_index == AWAKE_SET {
        let local_index = world.bodies[body_index as usize].local_index;
        let max_linear_speed = world.max_linear_speed;
        let set = &mut world.solver_sets[AWAKE_SET as usize];
        let body_sim = set.body_sims[local_index as usize];
        let state = &mut set.body_states[local_index as usize];

        state.linear_velocity = mul_add(state.linear_velocity, body_sim.inv_mass, impulse);

        if length_squared(state.linear_velocity) > max_linear_speed * max_linear_speed {
            state.linear_velocity = mul_sv(max_linear_speed, normalize(state.linear_velocity));
        }

        let delta = mul_mv(body_sim.inv_inertia_world, cross(sub_pos(point, body_sim.center), impulse));
        state.angular_velocity = add(state.angular_velocity, delta);
    }
}

pub fn body_apply_linear_impulse_to_center(world: &mut World, body_id: BodyId, impulse: Vec3, wake: bool) {
    b3_assert!(is_valid_vec3(impulse));

    let body_index = get_body_full_id(world, body_id);

    if wake && world.bodies[body_index as usize].set_index >= FIRST_SLEEPING_SET {
        wake_body_with_lock(world, body_index);
    }

    if world.bodies[body_index as usize].set_index == AWAKE_SET {
        let local_index = world.bodies[body_index as usize].local_index;
        let max_linear_speed = world.max_linear_speed;
        let set = &mut world.solver_sets[AWAKE_SET as usize];
        let body_sim = set.body_sims[local_index as usize];
        let state = &mut set.body_states[local_index as usize];
        state.linear_velocity = mul_add(state.linear_velocity, body_sim.inv_mass, impulse);

        if length_squared(state.linear_velocity) > max_linear_speed * max_linear_speed {
            state.linear_velocity = mul_sv(max_linear_speed, normalize(state.linear_velocity));
        }
    }
}

pub fn body_apply_angular_impulse(world: &mut World, body_id: BodyId, impulse: Vec3, wake: bool) {
    b3_assert!(is_valid_vec3(impulse));

    let body_index = get_body_full_id(world, body_id);
    b3_assert!(world.bodies[body_index as usize].generation == body_id.generation);

    if wake && world.bodies[body_index as usize].set_index >= FIRST_SLEEPING_SET {
        // this will not invalidate body index
        wake_body_with_lock(world, body_index);
    }

    if world.bodies[body_index as usize].set_index == AWAKE_SET {
        let local_index = world.bodies[body_index as usize].local_index;
        let set = &mut world.solver_sets[AWAKE_SET as usize];
        let body_sim = set.body_sims[local_index as usize];
        let state = &mut set.body_states[local_index as usize];

        let local_impulse = inv_rotate_vector(body_sim.transform.q, impulse);
        let local_angular_velocity_delta = mul_mv(body_sim.inv_inertia_local, local_impulse);
        state.angular_velocity = add(state.angular_velocity, rotate_vector(body_sim.transform.q, local_angular_velocity_delta));
    }
}

pub fn body_get_type(world: &World, body_id: BodyId) -> BodyType {
    let body_index = get_body_full_id(world, body_id);
    world.bodies[body_index as usize].body_type
}

// This should follow similar steps as you would get destroying and recreating the body, shapes, and joints.
// Contacts are difficult to preserve because the broad-phase pairs change, so I just destroy them.
pub fn body_set_type(world: &mut World, body_id: BodyId, body_type: BodyType) {
    if world.locked {
        b3_assert!(!world.locked);
        return;
    }

    world.locked = true;
    let body_index = get_body_full_id(world, body_id);

    let original_type = world.bodies[body_index as usize].body_type;
    if original_type == body_type {
        world.locked = false;
        return;
    }

    if body_type != BodyType::Static {
        let mut shape_id = world.bodies[body_index as usize].head_shape_id;
        while shape_id != NULL_INDEX {
            let shape = &world.shapes[shape_id as usize];
            if shape.shape_type() == ShapeType::Compound || shape.shape_type() == ShapeType::Height {
                // Setting the body type is not supported for bodies with compound shapes
                world.locked = false;
                return;
            }

            shape_id = shape.next_shape_id;
        }
    }

    // Stage 1: skip disabled bodies
    if world.bodies[body_index as usize].set_index == DISABLED_SET {
        // Disabled bodies don't change solver sets or islands when they change type.
        {
            let body = &mut world.bodies[body_index as usize];
            body.body_type = body_type;

            if body_type == BodyType::Dynamic {
                body.flags |= DYNAMIC_FLAG;
            } else {
                body.flags &= !DYNAMIC_FLAG;
            }
        }

        sync_body_flags(world, body_index);

        // Body type affects the mass properties
        update_body_mass_data(world, body_index);
        world.locked = false;
        return;
    }

    // Stage 2: destroy all contacts but don't wake bodies (because we don't need to)
    let wake_bodies = false;
    destroy_body_contacts(world, body_index, wake_bodies);

    // Stage 3: wake this body (does nothing if body is static), otherwise it will also wake
    // all bodies in the same sleeping solver set.
    wake_body(world, body_index);

    // Stage 4: move joints to temporary storage
    let mut joint_key = world.bodies[body_index as usize].head_joint_key;
    while joint_key != NULL_INDEX {
        let joint_id = joint_key >> 1;
        let edge_index = joint_key & 1;

        joint_key = world.joints[joint_id as usize].edges[edge_index as usize].next_key;

        // Joint may be disabled by other body
        if world.joints[joint_id as usize].set_index == DISABLED_SET {
            continue;
        }

        // Wake attached bodies. The wake_body call above does not wake bodies
        // attached to a static body. But it is necessary because the body may have
        // no joints.
        let body_id_a = world.joints[joint_id as usize].edges[0].body_id;
        let body_id_b = world.joints[joint_id as usize].edges[1].body_id;
        wake_body(world, body_id_a);
        wake_body(world, body_id_b);

        // Remove joint from island
        crate::island::unlink_joint(world, joint_id);

        // It is necessary to transfer all joints to the static set
        // so they can be added to the constraint graph below and acquire consistent colors.
        let joint_source_set = world.joints[joint_id as usize].set_index;
        crate::solver_set::transfer_joint(world, STATIC_SET, joint_source_set, joint_id);
    }

    // Stage 5: change the body type and transfer body
    {
        let body = &mut world.bodies[body_index as usize];
        body.body_type = body_type;

        if body_type == BodyType::Dynamic {
            body.flags |= DYNAMIC_FLAG;
        } else {
            body.flags &= !DYNAMIC_FLAG;
        }
    }

    let source_set_index = world.bodies[body_index as usize].set_index;
    let target_set_index = if body_type == BodyType::Static { STATIC_SET } else { AWAKE_SET };

    // Transfer body
    crate::solver_set::transfer_body(world, target_set_index, source_set_index, body_index);

    // Stage 6: update island participation for the body
    if original_type == BodyType::Static {
        // Create island for body
        create_island_for_body(world, AWAKE_SET, body_index);
    } else if body_type == BodyType::Static {
        // Remove body from island.
        remove_body_from_island(world, body_index);
    }

    // Stage 7: Transfer joints to the target set
    let mut joint_key = world.bodies[body_index as usize].head_joint_key;
    while joint_key != NULL_INDEX {
        let joint_id = joint_key >> 1;
        let edge_index = joint_key & 1;

        joint_key = world.joints[joint_id as usize].edges[edge_index as usize].next_key;

        // Joint may be disabled by other body
        if world.joints[joint_id as usize].set_index == DISABLED_SET {
            continue;
        }

        // All joints were transferred to the static set in an earlier stage
        b3_assert!(world.joints[joint_id as usize].set_index == STATIC_SET);

        let body_id_a = world.joints[joint_id as usize].edges[0].body_id;
        let body_id_b = world.joints[joint_id as usize].edges[1].body_id;
        b3_assert!(
            world.bodies[body_id_a as usize].set_index == STATIC_SET
                || world.bodies[body_id_a as usize].set_index == AWAKE_SET
        );
        b3_assert!(
            world.bodies[body_id_b as usize].set_index == STATIC_SET
                || world.bodies[body_id_b as usize].set_index == AWAKE_SET
        );

        if world.bodies[body_id_a as usize].body_type == BodyType::Dynamic
            || world.bodies[body_id_b as usize].body_type == BodyType::Dynamic
        {
            crate::solver_set::transfer_joint(world, AWAKE_SET, STATIC_SET, joint_id);
        }
    }

    // Recreate shape proxies in broadphase
    let transform = get_body_transform_quick(world, body_index);
    let mut shape_id = world.bodies[body_index as usize].head_shape_id;
    while shape_id != NULL_INDEX {
        // Setting the body type is not supported for bodies with compound shapes
        b3_assert!(world.shapes[shape_id as usize].shape_type() != ShapeType::Compound);

        let next_shape_id = world.shapes[shape_id as usize].next_shape_id;
        crate::shape::destroy_shape_proxy(world, shape_id);
        let force_pair_creation = true;
        crate::shape::create_shape_proxy(world, shape_id, body_type, transform, force_pair_creation);
        shape_id = next_shape_id;
    }

    // Relink all joints
    let mut joint_key = world.bodies[body_index as usize].head_joint_key;
    while joint_key != NULL_INDEX {
        let joint_id = joint_key >> 1;
        let edge_index = joint_key & 1;

        joint_key = world.joints[joint_id as usize].edges[edge_index as usize].next_key;

        let other_edge_index = edge_index ^ 1;
        let other_body_id = world.joints[joint_id as usize].edges[other_edge_index as usize].body_id;

        if world.bodies[other_body_id as usize].set_index == DISABLED_SET {
            continue;
        }

        if world.bodies[body_index as usize].body_type != BodyType::Dynamic
            && world.bodies[other_body_id as usize].body_type != BodyType::Dynamic
        {
            continue;
        }

        crate::island::link_joint(world, joint_id);
    }

    sync_body_flags(world, body_index);

    // Body type affects the mass
    update_body_mass_data(world, body_index);

    crate::physics_world::validate_solver_sets(world);
    let island_id = world.bodies[body_index as usize].island_id;
    crate::island::validate_island(world, island_id);

    world.locked = false;
}

pub fn body_set_name(world: &mut World, body_id: BodyId, name: &str) {
    let body_index = get_body_full_id(world, body_id);
    world.bodies[body_index as usize].name = truncate_name(name);
}

pub fn body_get_name(world: &World, body_id: BodyId) -> &str {
    let body_index = get_body_full_id(world, body_id);
    &world.bodies[body_index as usize].name
}

pub fn body_set_user_data(world: &mut World, body_id: BodyId, user_data: u64) {
    let body_index = get_body_full_id(world, body_id);
    world.bodies[body_index as usize].user_data = user_data;
}

pub fn body_get_user_data(world: &World, body_id: BodyId) -> u64 {
    let body_index = get_body_full_id(world, body_id);
    world.bodies[body_index as usize].user_data
}

pub fn body_get_mass(world: &World, body_id: BodyId) -> f32 {
    let body_index = get_body_full_id(world, body_id);
    world.bodies[body_index as usize].mass
}

pub fn body_get_local_rotational_inertia(world: &World, body_id: BodyId) -> Matrix3 {
    let body_index = get_body_full_id(world, body_id);
    world.bodies[body_index as usize].inertia
}

pub fn body_get_inverse_mass(world: &World, body_id: BodyId) -> f32 {
    let body_index = get_body_full_id(world, body_id);
    get_body_sim_from_id(world, body_index).inv_mass
}

pub fn body_get_world_inverse_rotational_inertia(world: &World, body_id: BodyId) -> Matrix3 {
    let body_index = get_body_full_id(world, body_id);
    get_body_sim_from_id(world, body_index).inv_inertia_world
}

pub fn body_get_local_center_of_mass(world: &World, body_id: BodyId) -> Vec3 {
    let body_index = get_body_full_id(world, body_id);
    get_body_sim_from_id(world, body_index).local_center
}

pub fn body_get_world_center_of_mass(world: &World, body_id: BodyId) -> Pos {
    let body_index = get_body_full_id(world, body_id);
    get_body_sim_from_id(world, body_index).center
}

pub fn body_set_mass_data(world: &mut World, body_id: BodyId, mass_data: MassData) {
    b3_assert!(is_valid_float(mass_data.mass) && mass_data.mass >= 0.0);
    b3_assert!(is_valid_matrix3(mass_data.inertia));
    b3_assert!(is_valid_vec3(mass_data.center));

    if world.locked {
        b3_assert!(!world.locked);
        return;
    }

    let body_index = get_body_full_id(world, body_id);

    {
        let body = &mut world.bodies[body_index as usize];
        body.mass = mass_data.mass;
        body.inertia = mass_data.inertia;
    }

    let body_sim = get_body_sim_from_id_mut(world, body_index);
    body_sim.local_center = mass_data.center;

    let center = transform_world_point(body_sim.transform, mass_data.center);
    body_sim.center = center;
    body_sim.center0 = center;

    body_sim.inv_mass = if mass_data.mass > 0.0 { 1.0 / mass_data.mass } else { 0.0 };
    body_sim.inv_inertia_local = if det(mass_data.inertia) > 0.0 { invert_t(mass_data.inertia) } else { Matrix3::ZERO };
}

pub fn body_get_mass_data(world: &World, body_id: BodyId) -> MassData {
    let body_index = get_body_full_id(world, body_id);
    let body = &world.bodies[body_index as usize];
    let body_sim = get_body_sim_from_id(world, body_index);
    MassData { mass: body.mass, center: body_sim.local_center, inertia: body.inertia }
}

pub fn body_apply_mass_from_shapes(world: &mut World, body_id: BodyId) {
    if world.locked {
        b3_assert!(!world.locked);
        return;
    }

    let body_index = get_body_full_id(world, body_id);
    update_body_mass_data(world, body_index);
}

pub fn body_set_linear_damping(world: &mut World, body_id: BodyId, linear_damping: f32) {
    b3_assert!(is_valid_float(linear_damping) && linear_damping >= 0.0);

    if world.locked {
        b3_assert!(!world.locked);
        return;
    }

    let body_index = get_body_full_id(world, body_id);
    get_body_sim_from_id_mut(world, body_index).linear_damping = linear_damping;
}

pub fn body_get_linear_damping(world: &World, body_id: BodyId) -> f32 {
    let body_index = get_body_full_id(world, body_id);
    get_body_sim_from_id(world, body_index).linear_damping
}

pub fn body_set_angular_damping(world: &mut World, body_id: BodyId, angular_damping: f32) {
    b3_assert!(is_valid_float(angular_damping) && angular_damping >= 0.0);

    if world.locked {
        b3_assert!(!world.locked);
        return;
    }

    let body_index = get_body_full_id(world, body_id);
    get_body_sim_from_id_mut(world, body_index).angular_damping = angular_damping;
}

pub fn body_get_angular_damping(world: &World, body_id: BodyId) -> f32 {
    let body_index = get_body_full_id(world, body_id);
    get_body_sim_from_id(world, body_index).angular_damping
}

pub fn body_set_gravity_scale(world: &mut World, body_id: BodyId, gravity_scale: f32) {
    b3_assert!(is_valid_float(gravity_scale));

    if world.locked {
        b3_assert!(!world.locked);
        return;
    }

    let body_index = get_body_full_id(world, body_id);
    get_body_sim_from_id_mut(world, body_index).gravity_scale = gravity_scale;
}

pub fn body_get_gravity_scale(world: &World, body_id: BodyId) -> f32 {
    let body_index = get_body_full_id(world, body_id);
    get_body_sim_from_id(world, body_index).gravity_scale
}

pub fn body_is_awake(world: &World, body_id: BodyId) -> bool {
    let body_index = get_body_full_id(world, body_id);
    world.bodies[body_index as usize].set_index == AWAKE_SET
}

pub fn body_set_awake(world: &mut World, body_id: BodyId, awake: bool) {
    if world.locked {
        b3_assert!(!world.locked);
        return;
    }

    world.locked = true;

    let body_index = get_body_full_id(world, body_id);
    let set_index = world.bodies[body_index as usize].set_index;

    if awake && set_index >= FIRST_SLEEPING_SET {
        wake_body(world, body_index);
    } else if !awake && set_index == AWAKE_SET {
        let island_id = world.bodies[body_index as usize].island_id;
        let island = &world.islands[island_id as usize];
        if island.constraint_remove_count > 0 {
            // Must split the island before sleeping. This is expensive.
            crate::island::split_island(world, island_id);
        }

        crate::solver_set::try_sleep_island(world, island_id);
    }

    world.locked = false;
}

pub fn body_is_enabled(world: &World, body_id: BodyId) -> bool {
    let body_index = get_body_full_id(world, body_id);
    world.bodies[body_index as usize].set_index != DISABLED_SET
}

pub fn body_is_sleep_enabled(world: &World, body_id: BodyId) -> bool {
    let body_index = get_body_full_id(world, body_id);
    (world.bodies[body_index as usize].flags & ENABLE_SLEEP) == ENABLE_SLEEP
}

pub fn body_set_sleep_threshold(world: &mut World, body_id: BodyId, sleep_threshold: f32) {
    let body_index = get_body_full_id(world, body_id);
    world.bodies[body_index as usize].sleep_threshold = sleep_threshold;
}

pub fn body_get_sleep_threshold(world: &World, body_id: BodyId) -> f32 {
    let body_index = get_body_full_id(world, body_id);
    world.bodies[body_index as usize].sleep_threshold
}

pub fn body_enable_sleep(world: &mut World, body_id: BodyId, enable_sleep: bool) {
    if world.locked {
        b3_assert!(!world.locked);
        return;
    }

    let body_index = get_body_full_id(world, body_id);

    let flag = (world.bodies[body_index as usize].flags & ENABLE_SLEEP) == ENABLE_SLEEP;
    if enable_sleep == flag {
        return;
    }

    world.locked = true;

    {
        let body = &mut world.bodies[body_index as usize];
        body.flags = if enable_sleep { body.flags | ENABLE_SLEEP } else { body.flags & !ENABLE_SLEEP };
    }
    sync_body_flags(world, body_index);

    if !enable_sleep {
        wake_body(world, body_index);
    }

    world.locked = false;
}

// Disabling a body requires a lot of detailed bookkeeping, but it is a valuable feature.
// The most challenging aspect is that joints may connect to bodies that are not disabled.
pub fn body_disable(world: &mut World, body_id: BodyId) {
    if world.locked {
        b3_assert!(!world.locked);
        return;
    }

    world.locked = true;

    let body_index = get_body_full_id(world, body_id);
    if world.bodies[body_index as usize].set_index == DISABLED_SET {
        world.locked = false;
        return;
    }

    // Destroy contacts and wake bodies touching this body. This avoid floating bodies.
    // This is necessary even for static bodies.
    let wake_bodies = true;
    destroy_body_contacts(world, body_index, wake_bodies);

    // The current solver set of the body
    let set_index = world.bodies[body_index as usize].set_index;

    // Unlink joints and transfer them to the disabled set
    let mut joint_key = world.bodies[body_index as usize].head_joint_key;
    while joint_key != NULL_INDEX {
        let joint_id = joint_key >> 1;
        let edge_index = joint_key & 1;

        joint_key = world.joints[joint_id as usize].edges[edge_index as usize].next_key;

        // joint may already be disabled by other body
        if world.joints[joint_id as usize].set_index == DISABLED_SET {
            continue;
        }

        b3_assert!(world.joints[joint_id as usize].set_index == set_index || set_index == STATIC_SET);

        // Remove joint from island
        crate::island::unlink_joint(world, joint_id);

        // Transfer joint to disabled set
        let joint_set_index = world.joints[joint_id as usize].set_index;
        crate::solver_set::transfer_joint(world, DISABLED_SET, joint_set_index, joint_id);
    }

    // Remove shapes from broad-phase
    let mut shape_id = world.bodies[body_index as usize].head_shape_id;
    while shape_id != NULL_INDEX {
        let next_shape_id = world.shapes[shape_id as usize].next_shape_id;
        crate::shape::destroy_shape_proxy(world, shape_id);
        shape_id = next_shape_id;
    }

    // Disabled bodies are not in an island. If the island becomes empty it will be destroyed.
    remove_body_from_island(world, body_index);

    // Transfer body sim
    let set_index = world.bodies[body_index as usize].set_index;
    crate::solver_set::transfer_body(world, DISABLED_SET, set_index, body_index);

    crate::physics_world::validate_connectivity(world);
    crate::physics_world::validate_solver_sets(world);

    world.locked = false;
}

pub fn body_enable(world: &mut World, body_id: BodyId) {
    if world.locked {
        b3_assert!(!world.locked);
        return;
    }

    let body_index = get_body_full_id(world, body_id);
    if world.bodies[body_index as usize].set_index != DISABLED_SET {
        return;
    }

    let body_type = world.bodies[body_index as usize].body_type;
    let set_id = if body_type == BodyType::Static { STATIC_SET } else { AWAKE_SET };

    crate::solver_set::transfer_body(world, set_id, DISABLED_SET, body_index);

    let transform = get_body_transform_quick(world, body_index);

    // Add shapes to broad-phase
    let proxy_type = body_type;
    let force_pair_creation = true;
    let mut shape_id = world.bodies[body_index as usize].head_shape_id;
    while shape_id != NULL_INDEX {
        let next_shape_id = world.shapes[shape_id as usize].next_shape_id;
        crate::shape::create_shape_proxy(world, shape_id, proxy_type, transform, force_pair_creation);
        shape_id = next_shape_id;
    }

    if set_id != STATIC_SET {
        create_island_for_body(world, set_id, body_index);
    }

    // Transfer joints. If the other body is disabled, don't transfer.
    // If the other body is sleeping, wake it.
    let mut joint_key = world.bodies[body_index as usize].head_joint_key;
    while joint_key != NULL_INDEX {
        let joint_id = joint_key >> 1;
        let edge_index = joint_key & 1;

        b3_assert!(world.joints[joint_id as usize].set_index == DISABLED_SET);
        b3_assert!(world.joints[joint_id as usize].island_id == NULL_INDEX);

        joint_key = world.joints[joint_id as usize].edges[edge_index as usize].next_key;

        let body_id_a = world.joints[joint_id as usize].edges[0].body_id;
        let body_id_b = world.joints[joint_id as usize].edges[1].body_id;
        let set_index_a = world.bodies[body_id_a as usize].set_index;
        let set_index_b = world.bodies[body_id_b as usize].set_index;

        if set_index_a == DISABLED_SET || set_index_b == DISABLED_SET {
            // one body is still disabled
            continue;
        }

        // Transfer joint first
        let joint_set_id = if set_index_a == STATIC_SET && set_index_b == STATIC_SET {
            STATIC_SET
        } else if set_index_a == STATIC_SET {
            set_index_b
        } else {
            set_index_a
        };

        crate::solver_set::transfer_joint(world, joint_set_id, DISABLED_SET, joint_id);

        // Now that the joint is in the correct set, I can link the joint in the island.
        if joint_set_id != STATIC_SET {
            crate::island::link_joint(world, joint_id);
        }
    }

    crate::physics_world::validate_solver_sets(world);
}

pub fn body_set_motion_locks(world: &mut World, body_id: BodyId, locks: MotionLocks) {
    if world.locked {
        b3_assert!(!world.locked);
        return;
    }

    let mut new_locks: u32 = 0;
    new_locks |= if locks.linear_x { LOCK_LINEAR_X } else { 0 };
    new_locks |= if locks.linear_y { LOCK_LINEAR_Y } else { 0 };
    new_locks |= if locks.linear_z { LOCK_LINEAR_Z } else { 0 };
    new_locks |= if locks.angular_x { LOCK_ANGULAR_X } else { 0 };
    new_locks |= if locks.angular_y { LOCK_ANGULAR_Y } else { 0 };
    new_locks |= if locks.angular_z { LOCK_ANGULAR_Z } else { 0 };

    let body_index = get_body_full_id(world, body_id);
    if (world.bodies[body_index as usize].flags & ALL_LOCKS) == new_locks {
        return;
    }

    let fixed_rotation1;
    let fixed_rotation2;
    {
        let body = &mut world.bodies[body_index as usize];
        fixed_rotation1 = (body.flags & FIXED_ROTATION) == FIXED_ROTATION;
        fixed_rotation2 = (new_locks & FIXED_ROTATION) == FIXED_ROTATION;

        body.flags &= !ALL_LOCKS;
        body.flags |= new_locks;
    }

    sync_body_flags(world, body_index);

    if let Some(state) = get_body_state_from_id_mut(world, body_index) {
        if locks.linear_x {
            state.linear_velocity.x = 0.0;
        }

        if locks.linear_y {
            state.linear_velocity.y = 0.0;
        }

        if locks.linear_z {
            state.linear_velocity.z = 0.0;
        }

        if locks.angular_x {
            state.angular_velocity.x = 0.0;
        }

        if locks.angular_y {
            state.angular_velocity.y = 0.0;
        }

        if locks.angular_z {
            state.angular_velocity.z = 0.0;
        }
    }

    if fixed_rotation1 != fixed_rotation2 {
        update_body_mass_data(world, body_index);
    }
}

pub fn body_get_motion_locks(world: &World, body_id: BodyId) -> MotionLocks {
    let body_index = get_body_full_id(world, body_id);
    let body = &world.bodies[body_index as usize];

    MotionLocks {
        linear_x: (body.flags & LOCK_LINEAR_X) != 0,
        linear_y: (body.flags & LOCK_LINEAR_Y) != 0,
        linear_z: (body.flags & LOCK_LINEAR_Z) != 0,
        angular_x: (body.flags & LOCK_ANGULAR_X) != 0,
        angular_y: (body.flags & LOCK_ANGULAR_Y) != 0,
        angular_z: (body.flags & LOCK_ANGULAR_Z) != 0,
    }
}

pub fn body_set_bullet(world: &mut World, body_id: BodyId, flag: bool) {
    if world.locked {
        b3_assert!(!world.locked);
        return;
    }

    let new_flag = if flag { IS_BULLET } else { 0 };

    let body_index = get_body_full_id(world, body_id);
    if (world.bodies[body_index as usize].flags & IS_BULLET) == new_flag {
        return;
    }

    {
        let body = &mut world.bodies[body_index as usize];
        body.flags &= !IS_BULLET;
        body.flags |= new_flag;
    }

    sync_body_flags(world, body_index);
}

pub fn body_is_bullet(world: &World, body_id: BodyId) -> bool {
    let body_index = get_body_full_id(world, body_id);
    (world.bodies[body_index as usize].flags & IS_BULLET) != 0
}

pub fn body_enable_contact_recycling(world: &mut World, body_id: BodyId, flag: bool) {
    if world.locked {
        b3_assert!(!world.locked);
        return;
    }

    let new_flag = if flag { BODY_ENABLE_CONTACT_RECYCLING } else { 0 };

    let body_index = get_body_full_id(world, body_id);
    if (world.bodies[body_index as usize].flags & BODY_ENABLE_CONTACT_RECYCLING) == new_flag {
        return;
    }

    {
        let body = &mut world.bodies[body_index as usize];
        body.flags &= !BODY_ENABLE_CONTACT_RECYCLING;
        body.flags |= new_flag;
    }

    sync_body_flags(world, body_index);
}

pub fn body_is_contact_recycling_enabled(world: &World, body_id: BodyId) -> bool {
    let body_index = get_body_full_id(world, body_id);
    (world.bodies[body_index as usize].flags & BODY_ENABLE_CONTACT_RECYCLING) != 0
}

pub fn body_enable_hit_events(world: &mut World, body_id: BodyId, enable_hit_events: bool) {
    let body_index = get_body_full_id(world, body_id);
    let mut shape_id = world.bodies[body_index as usize].head_shape_id;
    while shape_id != NULL_INDEX {
        let shape = &mut world.shapes[shape_id as usize];
        shape.enable_hit_events = enable_hit_events;
        shape_id = shape.next_shape_id;
    }
}

pub fn body_get_world(world: &World, body_id: BodyId) -> WorldId {
    let _ = get_body_full_id(world, body_id);
    WorldId { index1: world.world_id + 1, generation: world.generation }
}

pub fn body_get_shape_count(world: &World, body_id: BodyId) -> i32 {
    let body_index = get_body_full_id(world, body_id);
    world.bodies[body_index as usize].shape_count
}

pub fn body_get_shapes(world: &World, body_id: BodyId, shape_array: &mut [ShapeId]) -> i32 {
    let capacity = shape_array.len() as i32;
    let body_index = get_body_full_id(world, body_id);
    let mut shape_id = world.bodies[body_index as usize].head_shape_id;
    let mut shape_count = 0;
    while shape_id != NULL_INDEX && shape_count < capacity {
        let shape = &world.shapes[shape_id as usize];
        let id = ShapeId { index1: shape.id + 1, world0: body_id.world0, generation: shape.generation };
        shape_array[shape_count as usize] = id;
        shape_count += 1;

        shape_id = shape.next_shape_id;
    }

    shape_count
}

pub fn body_get_joint_count(world: &World, body_id: BodyId) -> i32 {
    let body_index = get_body_full_id(world, body_id);
    world.bodies[body_index as usize].joint_count
}

pub fn body_get_joints(world: &World, body_id: BodyId, joint_array: &mut [JointId]) -> i32 {
    let capacity = joint_array.len() as i32;
    let body_index = get_body_full_id(world, body_id);
    let mut joint_key = world.bodies[body_index as usize].head_joint_key;

    let mut joint_count = 0;
    while joint_key != NULL_INDEX && joint_count < capacity {
        let joint_id = joint_key >> 1;
        let edge_index = joint_key & 1;

        let joint = &world.joints[joint_id as usize];

        let id = JointId { index1: joint_id + 1, world0: body_id.world0, generation: joint.generation };
        joint_array[joint_count as usize] = id;
        joint_count += 1;

        joint_key = joint.edges[edge_index as usize].next_key;
    }

    joint_count
}

pub fn should_bodies_collide(world: &World, body_id_a: i32, body_id_b: i32) -> bool {
    let body_a = &world.bodies[body_id_a as usize];
    let body_b = &world.bodies[body_id_b as usize];

    if body_a.body_type != BodyType::Dynamic && body_b.body_type != BodyType::Dynamic {
        return false;
    }

    let mut joint_key;
    let other_body_id;
    if body_a.joint_count < body_b.joint_count {
        joint_key = body_a.head_joint_key;
        other_body_id = body_b.id;
    } else {
        joint_key = body_b.head_joint_key;
        other_body_id = body_a.id;
    }

    while joint_key != NULL_INDEX {
        let joint_id = joint_key >> 1;
        let edge_index = joint_key & 1;
        let other_edge_index = edge_index ^ 1;

        let joint = &world.joints[joint_id as usize];
        if !joint.collide_connected && joint.edges[other_edge_index as usize].body_id == other_body_id {
            return false;
        }

        joint_key = joint.edges[edge_index as usize].next_key;
    }

    true
}
