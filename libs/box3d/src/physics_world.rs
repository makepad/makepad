// Port of box3d/src/physics_world.h (+ physics_world.c to be ported below).
//
// Deviations (see PORTING.md):
// - No global world registry: World is an owned struct, the world parameter is
//   explicit in the public API. world_id is kept for id validation.
// - No scheduler/threads (serial), no recording, no debug draw callbacks yet.
// - The manifold block allocators are replaced by Vec<Manifold> per contact.
// - The hull database (verstable map keyed by content hash) becomes a std
//   HashMap from content hash to a list of Arc<HullData> with matching hash;
//   Arc handles lifetime, the map provides dedup.

use std::collections::HashMap;
use std::sync::Arc;

use crate::bitset::BitSet;
use crate::broad_phase::BroadPhase;
use crate::constants::CONTACT_MANIFOLD_COUNT_BUCKETS;
use crate::constraint_graph::ConstraintGraph;
use crate::contact::Contact;
use crate::id_pool::IdPool;
use crate::island::Island;
use crate::joint::Joint;
use crate::math_functions::{Pos, Vec3};
use crate::sensor::{Sensor, SensorHit, SensorTaskContext};
use crate::shape::Shape;
use crate::solver_set::SolverSet;
use crate::types::{
    BodyMoveEvent, Capacity, ContactBeginTouchEvent, ContactEndTouchEvent, ContactHitEvent,
    FrictionCallback, HullData, JointEvent, Profile, RestitutionCallback, SensorBeginTouchEvent,
    SensorEndTouchEvent,
};

pub const DEBUG_POINT_CAPACITY: usize = 64;
pub const DEBUG_LINE_CAPACITY: usize = 64;

// b3SetType
pub const STATIC_SET: i32 = 0;
pub const DISABLED_SET: i32 = 1;
pub const AWAKE_SET: i32 = 2;
pub const FIRST_SLEEPING_SET: i32 = 3;

pub type HexColor = u32;

#[derive(Clone, Copy, Debug, Default)]
pub struct DebugPoint {
    pub p: Pos,
    pub label: i32,
    pub value: f32,
    pub color: HexColor,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct DebugLine {
    pub p1: Pos,
    pub p2: Pos,
    pub label: i32,
    pub color: HexColor,
}

/// Per thread task storage (a single worker in the Rust port).
#[derive(Clone, Debug, Default)]
pub struct TaskContext {
    /// Collect per thread sensor continuous hit events.
    pub sensor_hits: Vec<SensorHit>,

    /// These bits align with the ConstraintGraph contact blocks and signal a
    /// change in contact status.
    pub contact_state_bit_set: BitSet,

    /// These bits align with the joint id capacity and signal a change in status.
    pub joint_state_bit_set: BitSet,

    /// These bits align with the contact id capacity and signal a hit event.
    pub hit_event_bit_set: BitSet,

    /// Fast-path flag: true when this worker set at least one bit in
    /// hit_event_bit_set this step.
    pub has_hit_events: bool,

    /// Used to track bodies with shapes that have enlarged AABBs.
    pub enlarged_sim_bit_set: BitSet,

    /// Used to put islands to sleep.
    pub awake_island_bit_set: BitSet,

    /// Per worker split island candidate.
    pub split_sleep_time: f32,
    pub split_island_id: i32,

    /// Profiling
    pub sat_call_count: i32,
    pub sat_cache_hit_count: i32,
    pub distance_iterations: i32,
    pub push_back_iterations: i32,
    pub root_iterations: i32,

    /// Number of contacts recycled this step (collide pass).
    pub recycled_contact_count: i32,

    pub points: Vec<DebugPoint>,
    pub lines: Vec<DebugLine>,

    pub manifold_counts: [i32; CONTACT_MANIFOLD_COUNT_BUCKETS],

    /// Deferred (color_index, local_index, manifold_count) mesh contact spec
    /// updates recorded during the parallel collide pass. C writes the specs
    /// inline from workers (each touching contact owns exactly one spec slot);
    /// the port defers them so workers never alias the constraint graph.
    /// Applied serially after the parallel-for in worker order; slot-disjoint,
    /// so the values are order independent.
    pub mesh_spec_updates: Vec<(i32, i32, u16)>,
}

/// The world struct manages all physics entities and dynamic simulation.
#[derive(Default)]
pub struct World {
    pub broad_phase: BroadPhase,
    pub constraint_graph: ConstraintGraph,

    /// The body id pool is used to allocate and recycle body ids.
    pub body_id_pool: IdPool,

    /// This is a sparse array that maps body ids to the body data stored in
    /// solver sets.
    pub bodies: Vec<crate::body::Body>,

    /// Provides free list for solver sets.
    pub solver_set_id_pool: IdPool,

    /// Solver sets allow sims to be stored in contiguous arrays.
    pub solver_sets: Vec<SolverSet>,

    /// Used to create stable ids for joints.
    pub joint_id_pool: IdPool,

    /// This is a sparse array that maps joint ids to the joint data stored in
    /// the constraint graph or in the solver sets.
    pub joints: Vec<Joint>,

    /// Used to create stable ids for contacts.
    pub contact_id_pool: IdPool,

    /// This is a sparse array that maps contact ids to the contact data stored
    /// in the constraint graph or in the solver sets.
    pub contacts: Vec<Contact>,

    /// Used to create stable ids for islands.
    pub island_id_pool: IdPool,

    /// This is a sparse array that maps island ids to the island data stored in
    /// the solver sets.
    pub islands: Vec<Island>,

    pub shape_id_pool: IdPool,

    /// These are sparse arrays that point into the pools above.
    pub shapes: Vec<Shape>,

    /// Reference counted store of shared hull data keyed by content hash.
    /// Each entry carries an explicit reference count (one per shape using it),
    /// matching the C database semantics.
    pub hull_database: HashMap<u32, Vec<(Arc<HullData>, i32)>>,

    /// This is a dense array of sensor data.
    pub sensors: Vec<Sensor>,

    /// Per thread storage (single worker in the Rust port).
    pub task_contexts: Vec<TaskContext>,
    pub sensor_task_contexts: Vec<SensorTaskContext>,

    /// Persistent solver step scratch (C: reusable arena). Moved into the
    /// StepContext for the duration of world_step; contents are transient.
    pub solver_scratch: crate::solver::SolverScratch,

    pub body_move_events: Vec<BodyMoveEvent>,
    pub sensor_begin_events: Vec<SensorBeginTouchEvent>,
    pub contact_begin_events: Vec<ContactBeginTouchEvent>,

    /// End events are double buffered so that the user doesn't need to flush events.
    pub sensor_end_events: [Vec<SensorEndTouchEvent>; 2],
    pub contact_end_events: [Vec<ContactEndTouchEvent>; 2],
    pub end_event_array_index: i32,

    pub contact_hit_events: Vec<ContactHitEvent>,
    pub joint_events: Vec<JointEvent>,

    /// Id that is incremented every time step.
    pub step_index: u64,

    /// Identify islands for splitting.
    pub split_island_id: i32,

    pub gravity: Vec3,
    pub hit_event_threshold: f32,
    pub restitution_threshold: f32,
    pub max_linear_speed: f32,
    pub contact_speed: f32,
    pub contact_hertz: f32,
    pub contact_damping_ratio: f32,
    pub contact_recycle_distance: f32,

    pub friction_callback: Option<FrictionCallback>,
    pub restitution_callback: Option<RestitutionCallback>,

    pub generation: u16,

    pub profile: Profile,
    pub sat_call_count: i32,
    pub sat_cache_hit_count: i32,
    pub manifold_counts: [i32; CONTACT_MANIFOLD_COUNT_BUCKETS],

    pub max_capacity: Capacity,

    pub pre_solve_fcn: Option<Box<crate::types::PreSolveFcn>>,
    pub custom_filter_fcn: Option<Box<crate::types::CustomFilterFcn>>,

    pub worker_count: i32,

    /// The task dispatch: serial, built-in scheduler, or user task system
    /// (C: world->enqueueTaskFcn/finishTaskFcn/userTaskContext + scheduler).
    pub task_system: crate::scheduler::TaskSystem,

    /// Pending dynamic/kinematic tree rebuild task (handle + owned trees).
    /// Enqueued by update_broad_phase_pairs to overlap contact creation and
    /// the narrow phase; joined by crate::broad_phase::finish_tree_task
    /// before the solve (C: world->userTreeTask).
    pub tree_rebuild_task: Option<(crate::scheduler::TaskHandle, Box<crate::broad_phase::TreeRebuildJob>)>,

    pub user_data: u64,

    /// latest inverse sub-step
    pub inv_h: f32,

    /// latest inverse full-step
    pub inv_dt: f32,

    pub active_task_count: i32,
    pub task_count: i32,

    pub world_id: u16,

    pub enable_sleep: bool,

    /// This indicates there is a world write operation in progress. This is for
    /// debugging and not a real mutex.
    pub locked: bool,

    pub enable_warm_starting: bool,
    pub enable_continuous: bool,
    pub enable_speculative: bool,
    pub in_use: bool,
}

impl std::fmt::Debug for World {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("World")
            .field("world_id", &self.world_id)
            .field("step_index", &self.step_index)
            .field("bodies", &self.bodies.len())
            .field("shapes", &self.shapes.len())
            .field("contacts", &self.contacts.len())
            .field("joints", &self.joints.len())
            .finish()
    }
}

/// The C b3AllocateManifolds/b3FreeManifolds block allocation becomes a plain Vec.
#[inline]
pub fn allocate_manifolds(count: i32) -> Vec<crate::types::Manifold> {
    vec![crate::types::Manifold::default(); count as usize]
}

// ---------------------------------------------------------------------------
// Port of physics_world.c
// ---------------------------------------------------------------------------
//
// Skipped (see PORTING.md): the global world registry (b3_worlds and the
// b3GetWorld* family — World is owned and passed explicitly), the scheduler /
// external task system, recording (B3_REC hooks, Start/StopRecording,
// b3HashWorldState), debug draw (b3World_Draw, DrawQueryCallback, debug bit
// sets, debug shape callbacks), and the memory dump helpers
// (b3World_DumpMemoryStats, b3World_DumpShapeBounds, b3World_DumpAwake,
// b3World_Dump). b3World_SetWorkerCount is a stub: the port is always serial
// with one worker context.

use crate::b3_assert;
use crate::b3_validate;
use crate::bitset::{count_set_bits, create_bit_set, get_bit, set_bit, set_bit_count_and_clear};
use crate::body::{get_body_transform_quick, wake_body, BODY_TRANSIENT_FLAGS, DYNAMIC_FLAG};
use crate::constants::{
    linear_slop, mesh_rest_offset, speculative_distance, contact_recycle_distance,
    CONTACT_RECYCLE_ANGULAR_DISTANCE, GRAPH_COLOR_COUNT, MAX_WORKERS,
};
use crate::constraint_graph::{add_contact_to_graph, remove_contact_from_graph, OVERFLOW_INDEX};
use crate::contact::{
    update_contact, CONTACT_ENABLE_CONTACT_EVENTS, CONTACT_TOUCHING_FLAG, RELATIVE_TRANSFORM_VALID,
    CONTACT_RECYCLE_FLAG, SIM_DISJOINT, SIM_MESH_CONTACT, SIM_STARTED_TOUCHING, SIM_STOPPED_TOUCHING,
    SIM_TOUCHING_FLAG,
};
use crate::core::{get_byte_count, NULL_INDEX, SECRET_COOKIE};
use crate::ctz::ctz64;
use crate::dynamic_tree::{
    dynamic_tree_box_cast, dynamic_tree_get_height, dynamic_tree_get_proxy_count,
    dynamic_tree_get_root_bounds, dynamic_tree_query, dynamic_tree_ray_cast, dynamic_tree_rebuild,
};
use crate::id::{BodyId, ContactId, JointId, ShapeId, WorldId};
use crate::id_pool::{alloc_id, create_id_pool, get_id_capacity, get_id_count, validate_free_id};
use crate::island::{link_contact, unlink_contact};
use crate::math_functions::{
    aabb_overlaps, aabb_union, abs as abs_vec3, add, clamp_float, clamp_int, cross, dot_quat,
    conjugate, distance_squared, inv_mul_quat, inv_mul_world_transforms, inv_transform_world_point,
    is_valid_aabb, is_valid_float, is_valid_position, is_valid_vec3, length_squared,
    make_aabb, make_matrix_from_quat, max as max_vec3, max_float, max_int, min as min_vec3,
    min_float, min_int, mul_add, mul_mv, mul_quat, mul_sv, normalize, offset_aabb, offset_pos,
    rotate_vector, sub, sub_pos, to_relative_transform, to_vec3, vec3, Transform, Vec3 as MVec3,
    AABB,
};
use crate::math_internal::modified_cross;
use crate::shape::{
    get_shape_centroid, get_shape_materials, get_shape_projected_area, make_shape_proxy,
    overlap_shape, ray_cast_shape, shape_cast_shape, shape_material_count, should_query_collide,
};
use crate::solver::{make_soft, StepContext};
use crate::solver_set::wake_solver_set;
use crate::timer::{get_milliseconds, get_ticks};
use crate::types::{
    BodyEvents, BodyType, ContactEvents, Counters, CustomFilterFcn, DistanceInput,
    ExplosionDef, JointEvents, PlaneResult, QueryFilter, RayCastInput, RayResult,
    SensorEvents, ShapeCastInput, ShapeProxy, SimplexCache, TreeStats, WorldDef, BODY_TYPE_COUNT,
};

fn default_friction_callback(friction_a: f32, _material_a: u64, friction_b: f32, _material_b: u64) -> f32 {
    (friction_a * friction_b).sqrt()
}

fn default_restitution_callback(restitution_a: f32, _material_a: u64, restitution_b: f32, _material_b: u64) -> f32 {
    max_float(restitution_a, restitution_b)
}

fn create_worker_contexts(world: &mut World) {
    let worker_count = world.worker_count as usize;
    world.task_contexts = vec![TaskContext::default(); worker_count];
    world.sensor_task_contexts = vec![SensorTaskContext::default(); worker_count];

    for i in 0..worker_count {
        world.task_contexts[i].sensor_hits = Vec::with_capacity(8);
        world.task_contexts[i].contact_state_bit_set = create_bit_set(1024);
        world.task_contexts[i].hit_event_bit_set = create_bit_set(1024);
        world.task_contexts[i].has_hit_events = false;
        world.task_contexts[i].joint_state_bit_set = create_bit_set(1024);
        world.task_contexts[i].enlarged_sim_bit_set = create_bit_set(256);
        world.task_contexts[i].awake_island_bit_set = create_bit_set(256);
        world.task_contexts[i].split_island_id = NULL_INDEX;

        world.sensor_task_contexts[i].event_bits = create_bit_set(128);
    }
}

fn destroy_worker_contexts(world: &mut World) {
    world.task_contexts = Vec::new();
    world.sensor_task_contexts = Vec::new();
}

/// C: b3CreateWorld(def) returning a world id into the global registry.
/// The port returns the owned world.
pub fn create_world(def: &WorldDef) -> World {
    b3_assert!(def.internal_value == SECRET_COOKIE);

    b3_assert!(linear_slop() <= mesh_rest_offset());
    b3_assert!(mesh_rest_offset() < speculative_distance());

    crate::contact::initialize_contact_registers();

    let mut world = World::default();

    world.world_id = 0;
    world.generation = 0;
    world.in_use = true;

    world.broad_phase = crate::broad_phase::create_broad_phase(&def.capacity);
    world.constraint_graph = crate::constraint_graph::create_graph(16);

    // pools
    world.body_id_pool = create_id_pool();

    let body_capacity = max_int(16, def.capacity.static_body_count + def.capacity.dynamic_body_count);
    world.bodies = Vec::with_capacity(body_capacity as usize);
    world.solver_sets = Vec::with_capacity(8);

    // add empty static, active, and disabled body sets
    world.solver_set_id_pool = create_id_pool();

    // static set
    {
        let mut set = SolverSet::default();
        set.set_index = alloc_id(&mut world.solver_set_id_pool);
        set.body_sims = Vec::with_capacity(max_int(16, def.capacity.static_body_count) as usize);
        world.solver_sets.push(set);
        b3_assert!(world.solver_sets[STATIC_SET as usize].set_index == STATIC_SET);
    }

    // disabled set
    {
        let mut set = SolverSet::default();
        set.set_index = alloc_id(&mut world.solver_set_id_pool);
        world.solver_sets.push(set);
        b3_assert!(world.solver_sets[DISABLED_SET as usize].set_index == DISABLED_SET);
    }

    // awake set
    {
        let mut set = SolverSet::default();
        set.set_index = alloc_id(&mut world.solver_set_id_pool);
        set.body_sims = Vec::with_capacity(max_int(16, def.capacity.dynamic_body_count) as usize);
        set.body_states = Vec::with_capacity(max_int(16, def.capacity.dynamic_body_count) as usize);
        set.contact_indices = Vec::with_capacity(max_int(16, def.capacity.contact_count) as usize);
        world.solver_sets.push(set);
        b3_assert!(world.solver_sets[AWAKE_SET as usize].set_index == AWAKE_SET);
    }

    world.shape_id_pool = create_id_pool();

    let shape_capacity = max_int(16, def.capacity.static_shape_count + def.capacity.dynamic_shape_count);
    world.shapes = Vec::with_capacity(shape_capacity as usize);

    world.hull_database = HashMap::new();

    world.contact_id_pool = create_id_pool();
    world.contacts = Vec::with_capacity(max_int(16, def.capacity.contact_count) as usize);

    world.joint_id_pool = create_id_pool();
    world.joints = Vec::with_capacity(16);

    world.island_id_pool = create_id_pool();
    world.islands = Vec::with_capacity(max_int(16, def.capacity.dynamic_body_count) as usize);

    world.sensors = Vec::with_capacity(4);

    world.body_move_events = Vec::with_capacity(4);
    world.sensor_begin_events = Vec::with_capacity(4);
    world.sensor_end_events = [Vec::with_capacity(4), Vec::with_capacity(4)];
    world.contact_begin_events = Vec::with_capacity(4);
    world.contact_end_events = [Vec::with_capacity(4), Vec::with_capacity(4)];
    world.contact_hit_events = Vec::with_capacity(4);
    world.joint_events = Vec::with_capacity(4);
    world.end_event_array_index = 0;

    world.step_index = 0;
    world.split_island_id = NULL_INDEX;
    world.active_task_count = 0;
    world.task_count = 0;
    world.gravity = def.gravity;
    world.hit_event_threshold = def.hit_event_threshold;
    world.restitution_threshold = def.restitution_threshold;
    world.max_linear_speed = def.maximum_linear_speed;
    world.contact_speed = def.contact_speed;
    world.contact_hertz = def.contact_hertz;
    world.contact_damping_ratio = def.contact_damping_ratio;
    world.contact_recycle_distance = contact_recycle_distance();

    world.friction_callback = Some(def.friction_callback.unwrap_or(default_friction_callback));
    world.restitution_callback = Some(def.restitution_callback.unwrap_or(default_restitution_callback));

    world.enable_sleep = def.enable_sleep;
    world.locked = false;
    world.enable_warm_starting = true;
    world.enable_continuous = def.enable_continuous;
    world.enable_speculative = true;
    world.user_data = def.user_data;

    // C: worker count clamped to [1, B3_MAX_WORKERS]; the built-in scheduler
    // runs worker_count - 1 background threads, the calling thread is worker 0.
    // worker_count == 1 with no user callbacks keeps the fully serial path.
    if def.worker_count > 0 && def.enqueue_task.is_some() && def.finish_task.is_some() {
        // External task system (C: def->enqueueTask/finishTask/userTaskContext)
        world.worker_count = clamp_int(def.worker_count as i32, 1, MAX_WORKERS as i32);
        world.task_system = crate::scheduler::TaskSystem::external(
            def.enqueue_task.unwrap(),
            def.finish_task.unwrap(),
            def.user_task_context,
        );
    } else if def.worker_count as i32 > 1 {
        // Built-in scheduler
        world.worker_count = clamp_int(def.worker_count as i32, 1, MAX_WORKERS as i32);
        world.task_system = crate::scheduler::TaskSystem::internal(world.worker_count);
    } else {
        // Serial fallback (C: b3DefaultAddTaskFcn/b3DefaultFinishTaskFcn)
        world.worker_count = 1;
        world.task_system = crate::scheduler::TaskSystem::serial();
    }

    create_worker_contexts(&mut world);

    world
}

/// C: b3DestroyWorld(worldId). The port consumes the world; most cleanup is
/// Drop. The shape allocation teardown is kept so the hull database empties
/// the way the C assert expects.
pub fn destroy_world(mut world: World) {
    b3_assert!(!world.locked);
    world.locked = true;

    // C: b3DestroyScheduler — joins the background worker threads (no-op for
    // the serial and external task systems).
    world.task_system = crate::scheduler::TaskSystem::serial();

    destroy_worker_contexts(&mut world);

    let shape_capacity = world.shapes.len();
    for i in 0..shape_capacity {
        if world.shapes[i].id != NULL_INDEX {
            crate::shape::destroy_shape_allocations(&mut world, i as i32);
        }
    }

    // Destroying every shape above released all hull references, so the database is empty.
    b3_assert!(world.hull_database.is_empty());

    // Everything else is dropped.
}

fn add_non_touching_contact(world: &mut World, contact_id: i32) {
    b3_assert!(world.contacts[contact_id as usize].set_index == AWAKE_SET);
    let local_index = world.solver_sets[AWAKE_SET as usize].contact_indices.len() as i32;
    {
        let contact = &mut world.contacts[contact_id as usize];
        contact.color_index = NULL_INDEX;
        contact.local_index = local_index;
        contact.body_sim_index_a = NULL_INDEX;
        contact.body_sim_index_b = NULL_INDEX;
    }
    world.solver_sets[AWAKE_SET as usize].contact_indices.push(contact_id);
}

fn remove_non_touching_contact(world: &mut World, set_index: i32, local_index: i32) {
    let moved_index =
        crate::container::array_remove_swap(&mut world.solver_sets[set_index as usize].contact_indices, local_index);
    if moved_index != NULL_INDEX {
        let moved_contact_index = world.solver_sets[set_index as usize].contact_indices[local_index as usize];
        let moved_contact = &mut world.contacts[moved_contact_index as usize];
        b3_assert!(moved_contact.set_index == set_index);
        b3_assert!(moved_contact.color_index == NULL_INDEX);
        b3_assert!(moved_contact.local_index == moved_index);
        moved_contact.local_index = local_index;
    }
}

/// C: b3CollideTask — run serially over the whole contact range with worker 0.
/// Shared context for the parallel collide pass. The world reference has its
/// contacts, task_contexts and pre_solve_fcn taken out for the duration; the
/// SyncSlice disjointness comes from awake_contact_indices holding each contact
/// id exactly once (a contact is either in one graph color slot or in the
/// awake non-touching list, never both) and worker_index being exclusive per
/// task (parallel_for contract).
struct CollideCtx<'a> {
    world: &'a World,
    indices: &'a [i32],
    contacts: &'a crate::sync::SyncSlice<'a, Contact>,
    task_contexts: &'a crate::sync::SyncSlice<'a, TaskContext>,
    /// Some only when running single-worker (serial fallback); see use_pre_solve.
    pre_solve: crate::sync::SyncPtr<Option<Box<crate::types::PreSolveFcn>>>,
    use_pre_solve: bool,
}

// C: b3CollideTask trampoline for the scheduler.
unsafe fn collide_task_trampoline(start_index: i32, end_index: i32, worker_index: i32, context: *mut ()) {
    // SAFETY: the CollideCtx lives on the collide() stack frame, which blocks
    // in parallel_for until every block completes.
    let ctx = unsafe { &*(context as *const CollideCtx) };
    collide_task(ctx, start_index, end_index, worker_index);
}

fn collide_task(ctx: &CollideCtx, start_index: i32, end_index: i32, worker_index: i32) {
    b3_assert!(start_index < end_index);

    let world = ctx.world;
    // SAFETY: worker_index is exclusive to this task for the whole range
    // (parallel_for contract), so the per-worker context is unaliased.
    let task_context = unsafe { ctx.task_contexts.get_mut(worker_index as usize) };

    let recycle_distance = world.contact_recycle_distance;
    let speculative_distance = speculative_distance();
    let recycle_distance_non_touching = min_float(recycle_distance, speculative_distance);

    for i in start_index..end_index {
        let contact_index = ctx.indices[i as usize];
        b3_assert!(contact_index < ctx.contacts.len() as i32);

        // SAFETY: each contact id appears exactly once in the awake contact
        // index array, so no other worker touches this element.
        let contact = unsafe { ctx.contacts.get_mut(contact_index as usize) };

        b3_validate!(contact.contact_id == contact_index);

        let (shape_id_a, shape_id_b, contact_flags) = (contact.shape_id_a, contact.shape_id_b, contact.flags);

        // Do proxies still overlap?
        let (fat_aabb_a, fat_aabb_b, body_id_a, body_id_b) = {
            let shape_a = &world.shapes[shape_id_a as usize];
            let shape_b = &world.shapes[shape_id_b as usize];
            (shape_a.fat_aabb, shape_b.fat_aabb, shape_a.body_id, shape_b.body_id)
        };

        let overlap = aabb_overlaps(fat_aabb_a, fat_aabb_b);
        if !overlap {
            // This contact will be destroyed
            contact.flags |= SIM_DISJOINT;
            contact.flags &= !SIM_TOUCHING_FLAG;
            set_bit(&mut task_context.contact_state_bit_set, contact_index as u32);
            continue;
        }

        // Update contact respecting shape/body order (A,B). Bodies behind awake-set
        // contacts are always either awake or static (when touching).
        let (type_a, set_index_a, local_index_a) = {
            let body_a = &world.bodies[body_id_a as usize];
            (body_a.body_type, body_a.set_index, body_a.local_index)
        };
        let (type_b, set_index_b, local_index_b) = {
            let body_b = &world.bodies[body_id_b as usize];
            (body_b.body_type, body_b.set_index, body_b.local_index)
        };
        let is_static_a = type_a == BodyType::Static;
        let is_static_b = type_b == BodyType::Static;
        let was_touching = (contact_flags & SIM_TOUCHING_FLAG) != 0;
        let is_mesh_contact = (contact_flags & SIM_MESH_CONTACT) != 0;

        if was_touching {
            b3_assert!(set_index_a == AWAKE_SET || set_index_a == STATIC_SET);
            b3_assert!(set_index_b == AWAKE_SET || set_index_b == STATIC_SET);
        }

        // There can be non-touching contacts between awake bodies and sleeping bodies.
        let body_sim_a = world.solver_sets[set_index_a as usize].body_sims[local_index_a as usize];
        let body_sim_b = world.solver_sets[set_index_b as usize].body_sims[local_index_b as usize];

        let transform_a = body_sim_a.transform;
        let transform_b = body_sim_b.transform;

        let is_fast = (body_sim_a.flags & crate::body::IS_FAST) != 0 || (body_sim_b.flags & crate::body::IS_FAST) != 0;

        // These are used by the contact solver. If the contact is between an awake body
        // and a sleeping body and the contact begins to touch, these will be invalid
        // but fixed when linked in the constraint graph.
        contact.body_sim_index_a = if is_static_a { NULL_INDEX } else { local_index_a };
        contact.body_sim_index_b = if is_static_b { NULL_INDEX } else { local_index_b };
        let recycle_tolerance = if was_touching { recycle_distance } else { recycle_distance_non_touching };

        // Contact recycling optimization. Please cite this library if you use this optimization.
        // This is inspired by persistent contact manifolds used in some physics engines, such as PhysX.
        // However, this allows larger relative motion and has fewer tuning parameters (just one).
        if (!is_fast || !is_mesh_contact)
            && recycle_distance > 0.0
            && (contact_flags & RELATIVE_TRANSFORM_VALID) != 0
            && (contact_flags & CONTACT_RECYCLE_FLAG) != 0
        {
            let angle_a = dot_quat(transform_a.q, contact.cached_rotation_a);
            let angle_b = dot_quat(transform_b.q, contact.cached_rotation_b);
            let angular_distance = min_float(angle_a * angle_a, angle_b * angle_b);

            let xf = inv_mul_world_transforms(transform_a, transform_b);
            let xfc = contact.cached_relative_pose;
            let max_extent_a = if is_static_a { MVec3::ZERO } else { body_sim_a.max_extent };
            let max_extent_b = if is_static_b { MVec3::ZERO } else { body_sim_b.max_extent };
            let max_extent = max_vec3(max_extent_a, max_extent_b);

            // Variation of Conservative Advancement
            // distance + 2 * length(modified_cross(|qr.v|, maxExtent)) < recycleTolerance.
            // 2*|qr.v| == 2*|sin(theta/2)| ~= theta for small angles.
            let dist_squared = distance_squared(xf.p, xfc.p);

            if angular_distance > CONTACT_RECYCLE_ANGULAR_DISTANCE && dist_squared < recycle_tolerance * recycle_tolerance {
                let distance = dist_squared.sqrt();
                let slack = recycle_tolerance - distance;

                // qr = inv( inv(qA0) * qB0 ) * inv(qA) * qB
                let qr = inv_mul_quat(xfc.q, xf.q);
                let arc = modified_cross(abs_vec3(qr.v), max_extent);

                let arc_sq = 4.0 * length_squared(arc);
                if arc_sq < slack * slack {
                    let dq_a = mul_quat(transform_a.q, conjugate(contact.cached_rotation_a));
                    let dq_b = mul_quat(transform_b.q, conjugate(contact.cached_rotation_b));
                    let matrix_a = make_matrix_from_quat(dq_a);
                    let matrix_b = make_matrix_from_quat(dq_b);

                    // Minimize round-off
                    let dc = sub_pos(body_sim_b.center, body_sim_a.center);

                    let manifold_count = contact.manifold_count();
                    for manifold_index in 0..manifold_count {
                        let manifold = &mut contact.manifolds[manifold_index as usize];
                        let normal = manifold.normal;

                        let point_count = manifold.point_count;
                        for point_index in 0..point_count {
                            // Keep anchors but update separation, same as sub-stepping. This eliminates jitter.
                            let mp = &mut manifold.points[point_index as usize];
                            let r_a = mul_mv(matrix_a, mp.anchor_a);
                            let r_b = mul_mv(matrix_b, mp.anchor_b);
                            let dp = add(dc, sub(r_b, r_a));
                            mp.separation = mp.base_separation + crate::math_functions::dot(dp, normal);
                            mp.persisted = true;
                        }
                    }

                    // Diagnostics
                    task_context.recycled_contact_count += 1;
                    let bucket_index = min_int(manifold_count, CONTACT_MANIFOLD_COUNT_BUCKETS as i32 - 1);
                    if bucket_index > 0 {
                        task_context.manifold_counts[bucket_index as usize - 1] += 1;
                    }

                    // Contact is recycled. This also skips updating other aspects of the contact
                    // such as material parameters.
                    continue;
                }
            }
        }

        // Caching for contact recycling.
        contact.cached_rotation_a = transform_a.q;
        contact.cached_rotation_b = transform_b.q;
        contact.cached_relative_pose = inv_mul_world_transforms(transform_a, transform_b);
        contact.flags |= RELATIVE_TRANSFORM_VALID;

        // The pre-solve slot is only used on the serial fallback path (a
        // single worker), so the SyncPtr access is exclusive.
        let pre_solve = if ctx.use_pre_solve {
            // SAFETY: use_pre_solve forces a single worker; exclusive access.
            Some(unsafe { ctx.pre_solve.get() })
        } else {
            None
        };

        // This updates solid contacts
        let touching = update_contact(
            world,
            task_context,
            contact,
            pre_solve,
            shape_id_a,
            body_sim_a.local_center,
            transform_a,
            shape_id_b,
            body_sim_b.local_center,
            transform_b,
            is_fast,
        );

        let manifold_count = contact.manifold_count();
        let bucket_index = min_int(manifold_count, CONTACT_MANIFOLD_COUNT_BUCKETS as i32 - 1);
        if bucket_index > 0 {
            task_context.manifold_counts[bucket_index as usize - 1] += 1;
        }

        // Update the mesh contact spec. C writes the constraint graph slot
        // inline (slot-disjoint per contact); the port defers to keep workers
        // from aliasing the graph.
        let contact_flags = contact.flags;
        if touching && was_touching && (contact_flags & SIM_MESH_CONTACT) != 0 {
            let (color_index, local_index) = (contact.color_index, contact.local_index);
            b3_assert!(color_index != NULL_INDEX);
            b3_assert!(0 <= color_index && color_index < GRAPH_COLOR_COUNT as i32);
            task_context.mesh_spec_updates.push((color_index, local_index, manifold_count as u16));
        }

        // State changes that affect island connectivity. Also affects contact events.
        if touching && !was_touching {
            contact.flags |= SIM_STARTED_TOUCHING;
            set_bit(&mut task_context.contact_state_bit_set, contact_index as u32);
        } else if !touching && was_touching {
            contact.flags |= SIM_STOPPED_TOUCHING;
            set_bit(&mut task_context.contact_state_bit_set, contact_index as u32);
        }

        {
            let manifold_count = contact.manifold_count();
            for manifold_index in 0..manifold_count {
                let manifold = &mut contact.manifolds[manifold_index as usize];
                for point_index in 0..manifold.point_count {
                    // Cache separation
                    let mp = &mut manifold.points[point_index as usize];
                    mp.base_separation = mp.separation;
                }
            }
        }
    }
}

// Narrow-phase collision
fn collide(world: &mut World, context: &mut StepContext) {
    b3_assert!(world.worker_count > 0);

    // Gather contacts from all the graph colors into a single array for easier parallel-for
    let mut touching_count = 0;
    for i in 0..GRAPH_COLOR_COUNT {
        let color = &world.constraint_graph.colors[i];
        touching_count += color.convex_contacts.len() + color.contacts.len();
    }

    let non_touching_count = world.solver_sets[AWAKE_SET as usize].contact_indices.len();

    let contact_count = touching_count + non_touching_count;

    if contact_count == 0 {
        return;
    }

    // Reuse the scratch buffer (C: arena allocation).
    let mut contact_indices: Vec<i32> = std::mem::take(&mut context.awake_contact_indices);
    contact_indices.clear();
    contact_indices.reserve(contact_count);

    for i in 0..GRAPH_COLOR_COUNT {
        let color = &world.constraint_graph.colors[i];
        for &id in &color.convex_contacts {
            contact_indices.push(id);
        }
        for spec in &color.contacts {
            contact_indices.push(spec.contact_id);
        }
    }

    b3_assert!(contact_indices.len() == touching_count);

    contact_indices.extend_from_slice(&world.solver_sets[AWAKE_SET as usize].contact_indices);

    context.awake_contact_indices = contact_indices;

    // Contact bit set on ids because contact pointers are unstable as they move between touching and not touching.
    let contact_id_capacity = get_id_capacity(&world.contact_id_pool);
    for i in 0..world.worker_count as usize {
        let task_context = &mut world.task_contexts[i];
        set_bit_count_and_clear(&mut task_context.contact_state_bit_set, contact_id_capacity as u32);
        task_context.sat_call_count = 0;
        task_context.sat_cache_hit_count = 0;
        task_context.recycled_contact_count = 0;
        task_context.manifold_counts = [0; CONTACT_MANIFOLD_COUNT_BUCKETS];
        task_context.mesh_spec_updates.clear();
    }

    // C: b3ParallelFor(world, b3CollideTask, contactCount, 20, context, "collide").
    // The contacts, per-worker task contexts, and the pre-solve callback are
    // taken out of the world so the parallel workers share a read-only &World;
    // per-contact and per-worker mutation goes through SyncSlice (see
    // CollideCtx invariants). A world with a pre-solve callback falls back to
    // a single worker because Box<dyn FnMut> is not Sync (the C requires the
    // callback to be thread-safe instead).
    {
        let mut contacts = std::mem::take(&mut world.contacts);
        let mut task_contexts = std::mem::take(&mut world.task_contexts);
        let mut pre_solve = world.pre_solve_fcn.take();

        let use_pre_solve = pre_solve.is_some();
        let effective_workers = if use_pre_solve { 1 } else { world.worker_count };

        {
            let contacts_slice = crate::sync::SyncSlice::new(&mut contacts);
            let task_contexts_slice = crate::sync::SyncSlice::new(&mut task_contexts);
            let collide_ctx = CollideCtx {
                world: &*world,
                indices: &context.awake_contact_indices,
                contacts: &contacts_slice,
                task_contexts: &task_contexts_slice,
                pre_solve: crate::sync::SyncPtr::new(&mut pre_solve),
                use_pre_solve,
            };

            // Task should take at least 40us on a 4GHz CPU (10K cycles)
            let min_range = 20;
            // SAFETY: the callback partitions [0, contact_count) into disjoint
            // ranges; per-element access is disjoint by contact id and worker
            // index (see CollideCtx). The context outlives parallel_for, which
            // blocks until all blocks complete.
            unsafe {
                crate::parallel_for::parallel_for(
                    &collide_ctx.world.task_system,
                    effective_workers,
                    collide_task_trampoline,
                    contact_count as i32,
                    min_range,
                    &collide_ctx as *const CollideCtx as *mut (),
                    "collide",
                );
            }
        }

        world.contacts = contacts;
        world.task_contexts = task_contexts;
        world.pre_solve_fcn = pre_solve;
    }

    // Apply the deferred mesh contact spec updates (slot-disjoint, so worker
    // order does not matter; iterate in worker order like the C merges).
    for i in 0..world.worker_count as usize {
        for k in 0..world.task_contexts[i].mesh_spec_updates.len() {
            let (color_index, local_index, manifold_count) = world.task_contexts[i].mesh_spec_updates[k];
            world.constraint_graph.colors[color_index as usize].contacts[local_index as usize].manifold_count =
                manifold_count;
        }
    }

    // C releases the arena allocation here; the port clears but keeps capacity.
    context.awake_contact_indices.clear();

    // Serially update contact state
    let sat_multiplier = if context.dt > 0.0 { 1 } else { 0 };

    // Bitwise OR all contact bits into worker 0 and sum the counters in
    // worker order (C: b3InPlaceUnion loop). Order independent: union and
    // sums are commutative.
    world.sat_call_count = sat_multiplier * world.task_contexts[0].sat_call_count;
    world.sat_cache_hit_count = sat_multiplier * world.task_contexts[0].sat_cache_hit_count;
    world.manifold_counts = world.task_contexts[0].manifold_counts;
    {
        let (first, rest) = world.task_contexts.split_at_mut(1);
        for i in 1..world.worker_count as usize {
            crate::bitset::in_place_union(&mut first[0].contact_state_bit_set, &rest[i - 1].contact_state_bit_set);
            world.sat_call_count += sat_multiplier * rest[i - 1].sat_call_count;
            world.sat_cache_hit_count += sat_multiplier * rest[i - 1].sat_cache_hit_count;
            for j in 0..CONTACT_MANIFOLD_COUNT_BUCKETS {
                world.manifold_counts[j] += rest[i - 1].manifold_counts[j];
            }
        }
    }

    let end_event_array_index = world.end_event_array_index;

    let world_id = world.world_id;

    // Process contact state changes. Iterate over set bits.
    // The bit set is moved out of the worker context during the loop because the
    // loop mutates the world (C iterates through a stable pointer).
    let bit_set = std::mem::take(&mut world.task_contexts[0].contact_state_bit_set);

    for k in 0..bit_set.bits.len() {
        let mut bits = bit_set.bits[k];
        while bits != 0 {
            let ctz = ctz64(bits);
            let contact_id = (64 * k as u32 + ctz) as i32;

            let (shape_id_a, shape_id_b, generation, flags) = {
                let contact = &world.contacts[contact_id as usize];
                b3_assert!(contact.set_index == AWAKE_SET);
                (contact.shape_id_a, contact.shape_id_b, contact.generation, contact.flags)
            };

            let shape_full_id_a = {
                let shape_a = &world.shapes[shape_id_a as usize];
                ShapeId { index1: shape_a.id + 1, world0: world_id, generation: shape_a.generation }
            };
            let shape_full_id_b = {
                let shape_b = &world.shapes[shape_id_b as usize];
                ShapeId { index1: shape_b.id + 1, world0: world_id, generation: shape_b.generation }
            };
            let contact_full_id = ContactId { index1: contact_id + 1, world0: world_id, generation };

            if (flags & SIM_DISJOINT) != 0 {
                // Bounding boxes no longer overlap
                crate::contact::destroy_contact(world, contact_id, false);
            } else if (flags & SIM_STARTED_TOUCHING) != 0 {
                b3_assert!(world.contacts[contact_id as usize].island_id == NULL_INDEX);

                if (flags & CONTACT_ENABLE_CONTACT_EVENTS) != 0 {
                    world.contact_begin_events.push(crate::types::ContactBeginTouchEvent {
                        shape_id_a: shape_full_id_a,
                        shape_id_b: shape_full_id_b,
                        contact_id: contact_full_id,
                    });
                }

                b3_assert!(world.contacts[contact_id as usize].manifold_count() > 0);
                b3_assert!(world.contacts[contact_id as usize].set_index == AWAKE_SET);

                // Link first because this wakes colliding bodies and ensures the body sims
                // are in the correct place.
                {
                    let contact = &mut world.contacts[contact_id as usize];
                    contact.flags &= !SIM_STARTED_TOUCHING;
                    contact.flags |= CONTACT_TOUCHING_FLAG;
                }
                link_contact(world, contact_id);

                // Make sure these didn't change
                b3_assert!(world.contacts[contact_id as usize].color_index == NULL_INDEX);

                let old_local_index = world.contacts[contact_id as usize].local_index;

                add_contact_to_graph(world, contact_id);
                remove_non_touching_contact(world, AWAKE_SET, old_local_index);
            } else if (flags & SIM_STOPPED_TOUCHING) != 0 {
                {
                    let contact = &mut world.contacts[contact_id as usize];
                    contact.flags &= !SIM_STOPPED_TOUCHING;
                    contact.flags &= !CONTACT_TOUCHING_FLAG;
                }

                if (world.contacts[contact_id as usize].flags & CONTACT_ENABLE_CONTACT_EVENTS) != 0 {
                    world.contact_end_events[end_event_array_index as usize].push(crate::types::ContactEndTouchEvent {
                        shape_id_a: shape_full_id_a,
                        shape_id_b: shape_full_id_b,
                        contact_id: contact_full_id,
                    });
                }

                b3_assert!(world.contacts[contact_id as usize].manifold_count() == 0);

                // Cache these here for the remove below
                let (color_index, local_index, body_id_a, body_id_b) = {
                    let contact = &world.contacts[contact_id as usize];
                    (contact.color_index, contact.local_index, contact.edges[0].body_id, contact.edges[1].body_id)
                };

                unlink_contact(world, contact_id);

                add_non_touching_contact(world, contact_id);

                let is_mesh_contact = (world.contacts[contact_id as usize].flags & SIM_MESH_CONTACT) != 0;
                remove_contact_from_graph(world, body_id_a, body_id_b, color_index, local_index, is_mesh_contact);
            }

            // Clear the smallest set bit
            bits &= bits - 1;
        }
    }

    world.task_contexts[0].contact_state_bit_set = bit_set;

    validate_solver_sets(world);
    validate_contacts(world);
}

/// C: b3World_Step(worldId, timeStep, subStepCount).
pub fn world_step(world: &mut World, time_step: f32, sub_step_count: i32) {
    if world.locked {
        b3_assert!(!world.locked);
        return;
    }

    world.locked = true;

    // Clear debug buffers
    for i in 0..world.worker_count as usize {
        world.task_contexts[i].points.clear();
        world.task_contexts[i].lines.clear();
    }

    // Prepare to capture events
    // Ensure user does not access stale data if there is an early return
    world.body_move_events.clear();
    world.sensor_begin_events.clear();
    world.contact_begin_events.clear();
    world.contact_hit_events.clear();
    world.joint_events.clear();

    world.profile = Profile::default();

    world.active_task_count = 0;
    world.task_count = 0;

    // C: b3ResetScheduler at step start — recycle the task ring and the
    // taskCount budget for this step (no tasks are pending here).
    world.task_system.reset();

    let step_ticks = get_ticks();

    {
        let c = &mut world.max_capacity;
        c.static_shape_count = max_int(
            c.static_shape_count,
            world.broad_phase.trees[BodyType::Static as usize].proxy_count,
        );
        c.dynamic_shape_count = max_int(
            c.dynamic_shape_count,
            world.broad_phase.trees[BodyType::Dynamic as usize].proxy_count,
        );

        let static_body_count = world.solver_sets[STATIC_SET as usize].body_sims.len() as i32;
        c.static_body_count = max_int(c.static_body_count, static_body_count);

        // this includes kinematic bodies
        let total_body_count = get_id_count(&world.body_id_pool);
        c.dynamic_body_count = max_int(c.dynamic_body_count, total_body_count - static_body_count);

        let total_contact_count = get_id_count(&world.contact_id_pool);
        c.contact_count = max_int(c.contact_count, total_contact_count);
    }

    // Update collision pairs and create contacts
    {
        let pair_ticks = get_ticks();
        crate::broad_phase::update_broad_phase_pairs(world);
        world.profile.pairs = get_milliseconds(pair_ticks);
    }

    let mut context = StepContext::default();
    // Reuse the persistent scratch capacity (C: arena allocations).
    world.solver_scratch.attach(&mut context);
    context.dt = time_step;
    context.sub_step_count = max_int(1, sub_step_count);
    context.worker_count = world.worker_count;

    if time_step > 0.0 {
        context.inv_dt = 1.0 / time_step;
        context.h = time_step / context.sub_step_count as f32;
        context.inv_h = context.sub_step_count as f32 * context.inv_dt;
    } else {
        context.inv_dt = 0.0;
        context.h = 0.0;
        context.inv_h = 0.0;
    }

    world.inv_h = context.inv_h;
    world.inv_dt = context.inv_dt;

    // Hertz values get reduced for large time steps
    let contact_hertz = min_float(world.contact_hertz, 0.125 * context.inv_h);
    context.contact_softness = make_soft(contact_hertz, world.contact_damping_ratio, context.h);
    context.static_softness = make_soft(2.0 * contact_hertz, 0.5 * world.contact_damping_ratio, context.h);

    context.restitution_threshold = world.restitution_threshold;
    context.max_linear_velocity = world.max_linear_speed;
    context.enable_warm_starting = world.enable_warm_starting;

    // Narrow phase : update contacts
    {
        let collide_ticks = get_ticks();
        collide(world, &mut context);
        world.profile.collide = get_milliseconds(collide_ticks);
    }

    // Finish the tree rebuild task before the solve: continuous collision
    // queries the dynamic/kinematic trees. C keeps the overlap window open
    // into b3Solve and finishes it there; the port closes it here, which
    // still overlaps the rebuild with contact creation and the narrow phase.
    crate::broad_phase::finish_tree_task(world);

    // Integrate velocities, solve velocity constraints, and integrate positions.
    if time_step > 0.0 {
        let solve_ticks = get_ticks();
        crate::solver::solve(world, &mut context);
        world.profile.solve = get_milliseconds(solve_ticks);
    }

    // Update sensors
    {
        let sensor_ticks = get_ticks();
        crate::sensor::overlap_sensors(world);
        world.profile.sensors = get_milliseconds(sensor_ticks);
    }

    world.profile.step = get_milliseconds(step_ticks);

    // Return the scratch buffers so their capacity is reused next step.
    world.solver_scratch.detach(&mut context);

    // Make sure all tasks that were started were also finished
    b3_assert!(world.active_task_count == 0);

    // Swap end event array buffers
    world.end_event_array_index = 1 - world.end_event_array_index;
    world.sensor_end_events[world.end_event_array_index as usize].clear();
    world.contact_end_events[world.end_event_array_index as usize].clear();
    world.locked = false;
}

/// C: b3World_GetBounds.
pub fn world_get_bounds(world: &World) -> AABB {
    b3_assert!(!world.locked);

    let mut world_bounds = AABB::default();
    let mut have_bounds = false;

    for i in 0..BODY_TYPE_COUNT {
        let tree = &world.broad_phase.trees[i];
        if dynamic_tree_get_proxy_count(tree) == 0 {
            continue;
        }

        let bounds = dynamic_tree_get_root_bounds(tree);

        if have_bounds {
            world_bounds = aabb_union(world_bounds, bounds);
        } else {
            world_bounds = bounds;
            have_bounds = true;
        }
    }

    world_bounds
}

/// C: b3World_GetBodyEvents.
pub fn world_get_body_events(world: &World) -> BodyEvents<'_> {
    b3_assert!(!world.locked);
    BodyEvents { move_events: &world.body_move_events }
}

/// C: b3World_GetSensorEvents.
pub fn world_get_sensor_events(world: &World) -> SensorEvents<'_> {
    b3_assert!(!world.locked);

    // Careful to use previous buffer
    let end_event_array_index = 1 - world.end_event_array_index;

    SensorEvents {
        begin_events: &world.sensor_begin_events,
        end_events: &world.sensor_end_events[end_event_array_index as usize],
    }
}

/// C: b3World_GetContactEvents.
pub fn world_get_contact_events(world: &World) -> ContactEvents<'_> {
    b3_assert!(!world.locked);

    // Careful to use previous buffer
    let end_event_array_index = 1 - world.end_event_array_index;

    ContactEvents {
        begin_events: &world.contact_begin_events,
        end_events: &world.contact_end_events[end_event_array_index as usize],
        hit_events: &world.contact_hit_events,
    }
}

/// C: b3World_GetJointEvents.
pub fn world_get_joint_events(world: &World) -> JointEvents<'_> {
    b3_assert!(!world.locked);
    JointEvents { joint_events: &world.joint_events }
}

/// C: b3World_IsValid. Single world: checks the id round-trips to this world.
pub fn world_is_valid(world: &World, id: WorldId) -> bool {
    if id.index1 != world.world_id + 1 {
        return false;
    }

    id.generation == world.generation
}

/// C: b3Body_IsValid.
pub fn body_is_valid(world: &World, id: BodyId) -> bool {
    if id.world0 != world.world_id {
        // invalid world
        return false;
    }

    if id.index1 < 1 || (world.bodies.len() as i32) < id.index1 {
        // invalid index
        return false;
    }

    let body = &world.bodies[(id.index1 - 1) as usize];
    if body.set_index == NULL_INDEX {
        // this was freed
        return false;
    }

    b3_assert!(body.local_index != NULL_INDEX);

    if body.generation != id.generation {
        // this id is orphaned
        return false;
    }

    true
}

/// C: b3Shape_IsValid.
pub fn shape_is_valid(world: &World, id: ShapeId) -> bool {
    if id.world0 != world.world_id {
        return false;
    }

    let shape_id = id.index1 - 1;
    if shape_id < 0 || world.shapes.len() as i32 <= shape_id {
        return false;
    }

    let shape = &world.shapes[shape_id as usize];
    if shape.id == NULL_INDEX {
        // shape is free
        return false;
    }

    b3_assert!(shape.id == shape_id);

    id.generation == shape.generation
}

/// C: b3Joint_IsValid.
pub fn joint_is_valid(world: &World, id: JointId) -> bool {
    if id.world0 != world.world_id {
        return false;
    }

    let joint_id = id.index1 - 1;
    if joint_id < 0 || world.joints.len() as i32 <= joint_id {
        return false;
    }

    let joint = &world.joints[joint_id as usize];
    if joint.joint_id == NULL_INDEX {
        // joint is free
        return false;
    }

    b3_assert!(joint.joint_id == joint_id);

    id.generation == joint.generation
}

/// C: b3Contact_IsValid.
pub fn contact_is_valid(world: &World, id: ContactId) -> bool {
    if id.world0 != world.world_id {
        return false;
    }

    let contact_id = id.index1 - 1;
    if contact_id < 0 || world.contacts.len() as i32 <= contact_id {
        return false;
    }

    let contact = &world.contacts[contact_id as usize];
    if contact.contact_id == NULL_INDEX {
        // contact is free
        return false;
    }

    b3_assert!(contact.contact_id == contact_id);

    id.generation == contact.generation
}

/// C: b3World_EnableSleeping.
pub fn world_enable_sleeping(world: &mut World, flag: bool) {
    if world.locked {
        b3_assert!(!world.locked);
        return;
    }

    if flag == world.enable_sleep {
        return;
    }

    world.enable_sleep = flag;

    if !flag {
        let set_count = world.solver_sets.len() as i32;
        for i in FIRST_SLEEPING_SET..set_count {
            if !world.solver_sets[i as usize].body_sims.is_empty() {
                wake_solver_set(world, i);
            }
        }
    }
}

/// C: b3World_IsSleepingEnabled.
pub fn world_is_sleeping_enabled(world: &World) -> bool {
    world.enable_sleep
}

/// C: b3World_EnableWarmStarting.
pub fn world_enable_warm_starting(world: &mut World, flag: bool) {
    if world.locked {
        b3_assert!(!world.locked);
        return;
    }
    world.enable_warm_starting = flag;
}

/// C: b3World_IsWarmStartingEnabled.
pub fn world_is_warm_starting_enabled(world: &World) -> bool {
    world.enable_warm_starting
}

/// C: b3World_GetAwakeBodyCount.
pub fn world_get_awake_body_count(world: &World) -> i32 {
    b3_assert!(!world.locked);
    world.solver_sets[AWAKE_SET as usize].body_sims.len() as i32
}

/// C: b3World_EnableContinuous.
pub fn world_enable_continuous(world: &mut World, flag: bool) {
    if world.locked {
        b3_assert!(!world.locked);
        return;
    }
    world.enable_continuous = flag;
}

/// C: b3World_IsContinuousEnabled.
pub fn world_is_continuous_enabled(world: &World) -> bool {
    world.enable_continuous
}

/// C: b3World_SetRestitutionThreshold.
pub fn world_set_restitution_threshold(world: &mut World, value: f32) {
    if world.locked {
        b3_assert!(!world.locked);
        return;
    }
    world.restitution_threshold = clamp_float(value, 0.0, f32::MAX);
}

/// C: b3World_GetRestitutionThreshold.
pub fn world_get_restitution_threshold(world: &World) -> f32 {
    world.restitution_threshold
}

/// C: b3World_SetHitEventThreshold.
pub fn world_set_hit_event_threshold(world: &mut World, value: f32) {
    if world.locked {
        b3_assert!(!world.locked);
        return;
    }
    world.hit_event_threshold = clamp_float(value, 0.0, f32::MAX);
}

/// C: b3World_GetHitEventThreshold.
pub fn world_get_hit_event_threshold(world: &World) -> f32 {
    world.hit_event_threshold
}

/// C: b3World_SetContactTuning.
pub fn world_set_contact_tuning(world: &mut World, hertz: f32, damping_ratio: f32, contact_speed: f32) {
    if world.locked {
        b3_assert!(!world.locked);
        return;
    }

    world.contact_hertz = clamp_float(hertz, 0.0, f32::MAX);
    world.contact_damping_ratio = clamp_float(damping_ratio, 0.0, f32::MAX);
    world.contact_speed = clamp_float(contact_speed, 0.0, f32::MAX);
}

/// C: b3World_SetContactRecycleDistance.
pub fn world_set_contact_recycle_distance(world: &mut World, recycle_distance: f32) {
    b3_assert!(!world.locked);
    if world.locked {
        return;
    }

    world.contact_recycle_distance = clamp_float(recycle_distance, 0.0, f32::MAX);
}

/// C: b3World_GetContactRecycleDistance.
pub fn world_get_contact_recycle_distance(world: &World) -> f32 {
    world.contact_recycle_distance
}

/// C: b3World_SetMaximumLinearSpeed.
pub fn world_set_maximum_linear_speed(world: &mut World, maximum_linear_speed: f32) {
    b3_assert!(is_valid_float(maximum_linear_speed) && maximum_linear_speed > 0.0);

    if world.locked {
        b3_assert!(!world.locked);
        return;
    }

    world.max_linear_speed = maximum_linear_speed;
}

/// C: b3World_GetMaximumLinearSpeed.
pub fn world_get_maximum_linear_speed(world: &World) -> f32 {
    world.max_linear_speed
}

/// C: b3World_GetProfile.
pub fn world_get_profile(world: &World) -> Profile {
    b3_assert!(!world.locked);
    world.profile
}

/// C: b3World_GetCounters. The stack/arena/byte counters are zero in the port
/// (no arena allocators, no allocation tracking).
pub fn world_get_counters(world: &World) -> Counters {
    b3_assert!(!world.locked);

    let mut s = Counters::default();
    s.body_count = get_id_count(&world.body_id_pool);
    s.shape_count = get_id_count(&world.shape_id_pool);
    s.contact_count = get_id_count(&world.contact_id_pool);
    s.joint_count = get_id_count(&world.joint_id_pool);
    s.island_count = get_id_count(&world.island_id_pool);

    let static_tree = &world.broad_phase.trees[BodyType::Static as usize];
    s.static_tree_height = dynamic_tree_get_height(static_tree);

    let dynamic_tree = &world.broad_phase.trees[BodyType::Dynamic as usize];
    let kinematic_tree = &world.broad_phase.trees[BodyType::Kinematic as usize];
    s.tree_height = max_int(dynamic_tree_get_height(dynamic_tree), dynamic_tree_get_height(kinematic_tree));

    s.sat_call_count = world.sat_call_count;
    s.sat_cache_hit_count = world.sat_cache_hit_count;
    s.manifold_counts = world.manifold_counts;
    s.stack_used = 0;
    s.byte_count = get_byte_count();
    s.task_count = world.task_count;

    s.awake_contact_count = 0;
    for i in 0..GRAPH_COLOR_COUNT {
        let color = &world.constraint_graph.colors[i];
        let color_contact_count = (color.convex_contacts.len() + color.contacts.len()) as i32;
        s.color_counts[i] = color_contact_count + color.joint_sims.len() as i32;
        s.awake_contact_count += color_contact_count;
    }
    s.awake_contact_count += world.solver_sets[AWAKE_SET as usize].contact_indices.len() as i32;

    s.recycled_contact_count = 0;
    s.arena_capacity = 0;
    s.distance_iterations = 0;
    s.push_back_iterations = 0;
    s.root_iterations = 0;
    for i in 0..world.worker_count as usize {
        s.recycled_contact_count += world.task_contexts[i].recycled_contact_count;

        s.distance_iterations = max_int(s.distance_iterations, world.task_contexts[i].distance_iterations);
        s.push_back_iterations = max_int(s.push_back_iterations, world.task_contexts[i].push_back_iterations);
        s.root_iterations = max_int(s.root_iterations, world.task_contexts[i].root_iterations);
    }

    s
}

/// C: b3World_GetMaxCapacity.
pub fn world_get_max_capacity(world: &World) -> Capacity {
    b3_assert!(!world.locked);
    world.max_capacity
}

/// C: b3World_SetUserData.
pub fn world_set_user_data(world: &mut World, user_data: u64) {
    world.user_data = user_data;
}

/// C: b3World_GetUserData.
pub fn world_get_user_data(world: &World) -> u64 {
    world.user_data
}

/// C: b3World_SetFrictionCallback.
pub fn world_set_friction_callback(world: &mut World, callback: Option<crate::types::FrictionCallback>) {
    if world.locked {
        b3_assert!(!world.locked);
        return;
    }
    world.friction_callback = Some(callback.unwrap_or(default_friction_callback));
}

/// C: b3World_SetRestitutionCallback.
pub fn world_set_restitution_callback(world: &mut World, callback: Option<crate::types::RestitutionCallback>) {
    if world.locked {
        b3_assert!(!world.locked);
        return;
    }
    world.restitution_callback = Some(callback.unwrap_or(default_restitution_callback));
}

/// C: b3World_SetWorkerCount. The Rust port is always serial: the count is
/// clamped to 1 and the worker contexts stay as they are.
pub fn world_set_worker_count(world: &mut World, count: i32) {
    if world.locked {
        b3_assert!(!world.locked);
        return;
    }

    let count = clamp_int(count, 1, MAX_WORKERS as i32);
    let _ = count;
    b3_assert!(world.worker_count == 1);
}

/// C: b3World_GetWorkerCount.
pub fn world_get_worker_count(world: &World) -> i32 {
    b3_assert!(!world.locked);
    world.worker_count
}

/// C: b3World_SetCustomFilterCallback. The context pointer is captured by the closure.
pub fn world_set_custom_filter_callback(world: &mut World, fcn: Option<Box<CustomFilterFcn>>) {
    if world.locked {
        b3_assert!(!world.locked);
        return;
    }
    world.custom_filter_fcn = fcn;
}

/// C: b3World_SetPreSolveCallback. The context pointer is captured by the closure.
pub fn world_set_pre_solve_callback(world: &mut World, fcn: Option<Box<crate::types::PreSolveFcn>>) {
    if world.locked {
        b3_assert!(!world.locked);
        return;
    }
    world.pre_solve_fcn = fcn;
}

/// C: b3World_SetGravity.
pub fn world_set_gravity(world: &mut World, gravity: MVec3) {
    world.gravity = gravity;
}

/// C: b3World_GetGravity.
pub fn world_get_gravity(world: &World) -> MVec3 {
    world.gravity
}

/// C: b3World_RebuildStaticTree.
pub fn world_rebuild_static_tree(world: &mut World) {
    if world.locked {
        b3_assert!(!world.locked);
        return;
    }

    let static_tree = &mut world.broad_phase.trees[BodyType::Static as usize];
    dynamic_tree_rebuild(static_tree, true);
}

/// C: b3World_EnableSpeculative.
pub fn world_enable_speculative(world: &mut World, flag: bool) {
    if world.locked {
        b3_assert!(!world.locked);
        return;
    }

    world.enable_speculative = flag;
}

// ---------------------------------------------------------------------------
// Queries and casts
// ---------------------------------------------------------------------------

/// C: b3World_OverlapAABB. The callback context is captured by the closure.
pub fn world_overlap_aabb(
    world: &World,
    aabb: AABB,
    filter: QueryFilter,
    fcn: &mut dyn FnMut(ShapeId) -> bool,
) -> TreeStats {
    let mut tree_stats = TreeStats::default();

    if world.locked {
        b3_assert!(!world.locked);
        return tree_stats;
    }

    b3_assert!(is_valid_aabb(aabb));

    for i in 0..BODY_TYPE_COUNT {
        let mut callback = |_proxy_id: i32, user_data: u64| -> bool {
            let shape_id = user_data as i32;
            let shape = &world.shapes[shape_id as usize];

            if !should_query_collide(shape.filter, filter) {
                return true;
            }

            let id = ShapeId { index1: shape_id + 1, world0: world.world_id, generation: shape.generation };
            fcn(id)
        };

        let tree_result = dynamic_tree_query(&world.broad_phase.trees[i], aabb, filter.mask_bits, false, &mut callback);

        tree_stats.node_visits += tree_result.node_visits;
        tree_stats.leaf_visits += tree_result.leaf_visits;
    }

    tree_stats
}

/// C: b3World_OverlapShape.
pub fn world_overlap_shape(
    world: &World,
    origin: crate::math_functions::Pos,
    proxy: &ShapeProxy,
    filter: QueryFilter,
    fcn: &mut dyn FnMut(ShapeId) -> bool,
) -> TreeStats {
    let mut tree_stats = TreeStats::default();

    if world.locked {
        b3_assert!(!world.locked);
        return tree_stats;
    }

    b3_assert!(is_valid_position(origin));

    // Bound the proxy in origin relative space then lift to a conservative world float box
    let aabb = offset_aabb(make_aabb(proxy.points, proxy.radius), origin);

    for i in 0..BODY_TYPE_COUNT {
        let mut callback = |_proxy_id: i32, user_data: u64| -> bool {
            let shape_id = user_data as i32;
            let shape = &world.shapes[shape_id as usize];

            if !should_query_collide(shape.filter, filter) {
                return true;
            }

            // Re-center on the query origin so the overlap test stays in float precision far from the origin
            let transform = to_relative_transform(get_body_transform_quick(world, shape.body_id), origin);

            let overlapping = overlap_shape(shape, transform, proxy);
            if !overlapping {
                return true;
            }

            let id = ShapeId { index1: shape.id + 1, world0: world.world_id, generation: shape.generation };
            fcn(id)
        };

        let tree_result = dynamic_tree_query(&world.broad_phase.trees[i], aabb, filter.mask_bits, false, &mut callback);

        tree_stats.node_visits += tree_result.node_visits;
        tree_stats.leaf_visits += tree_result.leaf_visits;
    }

    tree_stats
}

/// C: b3World_CollideMover. fcn receives (shapeId, planes) and returns true to continue.
pub fn world_collide_mover(
    world: &World,
    origin: crate::math_functions::Pos,
    mover: &crate::types::Capsule,
    filter: QueryFilter,
    fcn: &mut dyn FnMut(ShapeId, &[PlaneResult]) -> bool,
) -> () {
    if world.locked {
        b3_assert!(!world.locked);
        return;
    }

    b3_assert!(is_valid_position(origin));

    let r = vec3(mover.radius, mover.radius, mover.radius);

    // Relative box lifted to world float with outward rounding, conservative for the tree
    let rel_box = AABB {
        lower_bound: sub(min_vec3(mover.center1, mover.center2), r),
        upper_bound: add(max_vec3(mover.center1, mover.center2), r),
    };
    let aabb = offset_aabb(rel_box, origin);

    for i in 0..BODY_TYPE_COUNT {
        let mut callback = |_proxy_id: i32, user_data: u64| -> bool {
            let shape_id = user_data as i32;
            let shape = &world.shapes[shape_id as usize];

            if !should_query_collide(shape.filter, filter) {
                return true;
            }

            // Re-center on the query origin, the mover and the resulting planes are origin relative
            let body_transform = get_body_transform_quick(world, shape.body_id);
            let transform = to_relative_transform(body_transform, origin);

            let mut buffer = [PlaneResult::default(); 64];
            let count = crate::shape::collide_mover(&mut buffer, 64, shape, transform, mover);

            if count > 0 {
                let id = ShapeId { index1: shape.id + 1, world0: world.world_id, generation: shape.generation };
                return fcn(id, &buffer[..count as usize]);
            }

            true
        };

        dynamic_tree_query(&world.broad_phase.trees[i], aabb, filter.mask_bits, false, &mut callback);
    }
}

/// C: b3World_CastRay. fcn is the C b3CastResultFcn with the context captured:
/// (shapeId, point, normal, fraction, userMaterialId, triangleIndex, childIndex) -> f32.
pub fn world_cast_ray(
    world: &World,
    origin: crate::math_functions::Pos,
    translation: MVec3,
    filter: QueryFilter,
    fcn: &mut dyn FnMut(ShapeId, crate::math_functions::Pos, MVec3, f32, u64, i32, i32) -> f32,
) -> TreeStats {
    let mut tree_stats = TreeStats::default();

    if world.locked {
        b3_assert!(!world.locked);
        return tree_stats;
    }

    b3_assert!(is_valid_position(origin));
    b3_assert!(is_valid_vec3(translation));

    // The tree traverses in float relative to the world origin. Each shape is then re-differenced at
    // full precision against the origin, so a hit stays accurate far from the origin.
    let mut input = RayCastInput { origin: to_vec3(origin), translation, max_fraction: 1.0 };

    let mut fraction = 1.0f32;

    for i in 0..BODY_TYPE_COUNT {
        {
            let fraction = &mut fraction;
            let mut callback = |input: &RayCastInput, _proxy_id: i32, user_data: u64| -> f32 {
                let shape_id = user_data as i32;
                let shape = &world.shapes[shape_id as usize];

                if !should_query_collide(shape.filter, filter) {
                    return input.max_fraction;
                }

                let body_transform = get_body_transform_quick(world, shape.body_id);
                let transform = to_relative_transform(body_transform, origin);

                let mut local_input = *input;
                local_input.origin = MVec3::ZERO;
                let output = ray_cast_shape(shape, transform, &local_input);

                if output.hit {
                    b3_assert!(output.fraction <= input.max_fraction);

                    let id = ShapeId { index1: shape_id + 1, world0: world.world_id, generation: shape.generation };
                    let point = offset_pos(origin, output.point);
                    let material_index = clamp_int(output.material_index, 0, shape_material_count(shape) - 1);
                    let user_material_id = get_shape_materials(shape)[material_index as usize].user_material_id;

                    let triangle_index = output.triangle_index;
                    let child_index = output.child_index;
                    let new_fraction =
                        fcn(id, point, output.normal, output.fraction, user_material_id, triangle_index, child_index);

                    // The user may return -1 to skip this shape
                    if 0.0 <= new_fraction && new_fraction <= 1.0 {
                        *fraction = new_fraction;
                    }

                    return new_fraction;
                }

                input.max_fraction
            };

            let tree_result =
                dynamic_tree_ray_cast(&world.broad_phase.trees[i], &input, filter.mask_bits, false, &mut callback);
            tree_stats.node_visits += tree_result.node_visits;
            tree_stats.leaf_visits += tree_result.leaf_visits;
        }

        if fraction == 0.0 {
            break;
        }

        input.max_fraction = fraction;
    }

    tree_stats
}

/// C: b3World_CastRayClosest. This is the most common callback used in games.
pub fn world_cast_ray_closest(
    world: &World,
    origin: crate::math_functions::Pos,
    translation: MVec3,
    filter: QueryFilter,
) -> RayResult {
    let mut result = RayResult::default();

    if world.locked {
        b3_assert!(!world.locked);
        return result;
    }

    b3_assert!(is_valid_position(origin));
    b3_assert!(is_valid_vec3(translation));

    let node_visits;
    let leaf_visits;

    {
        // C: b3RayCastClosestFcn — ignore initial overlap, keep the closest hit.
        let result = &mut result;
        let mut closest_fcn = |shape_id: ShapeId,
                               point: crate::math_functions::Pos,
                               normal: MVec3,
                               fraction: f32,
                               user_material_id: u64,
                               triangle_index: i32,
                               child_index: i32|
         -> f32 {
            // Ignore initial overlap
            if fraction == 0.0 {
                return -1.0;
            }

            result.shape_id = shape_id;
            result.point = point;
            result.normal = normal;
            result.fraction = fraction;
            result.user_material_id = user_material_id;
            result.triangle_index = triangle_index;
            result.child_index = child_index;
            result.hit = true;
            fraction
        };

        let tree_stats = world_cast_ray(world, origin, translation, filter, &mut closest_fcn);
        node_visits = tree_stats.node_visits;
        leaf_visits = tree_stats.leaf_visits;
    }

    result.node_visits = node_visits;
    result.leaf_visits = leaf_visits;

    result
}

/// C: b3World_CastShape.
pub fn world_cast_shape(
    world: &World,
    origin: crate::math_functions::Pos,
    proxy: &ShapeProxy,
    translation: MVec3,
    filter: QueryFilter,
    fcn: &mut dyn FnMut(ShapeId, crate::math_functions::Pos, MVec3, f32, u64, i32, i32) -> f32,
) -> TreeStats {
    let mut tree_stats = TreeStats::default();

    if world.locked {
        b3_assert!(!world.locked);
        return tree_stats;
    }

    b3_assert!(is_valid_position(origin));
    b3_assert!(is_valid_vec3(translation));

    let mut fraction = 1.0f32;

    // Bound the proxy in origin relative space then lift to a conservative world float box.
    let local_box = make_aabb(proxy.points, proxy.radius);
    let mut tree_input = crate::types::BoxCastInput {
        box_: offset_aabb(local_box, origin),
        translation,
        max_fraction: 1.0,
    };

    for i in 0..BODY_TYPE_COUNT {
        {
            let fraction = &mut fraction;
            let mut callback = |input: &crate::types::BoxCastInput, _proxy_id: i32, user_data: u64| -> f32 {
                let shape_id = user_data as i32;
                let shape = &world.shapes[shape_id as usize];

                if !should_query_collide(shape.filter, filter) {
                    return input.max_fraction;
                }

                // Rebuild from the origin relative input, taking only the advancing fraction from the tree.
                // The tree box is world float and would lose the cast far from the origin.
                let local_input = ShapeCastInput {
                    proxy: *proxy,
                    translation,
                    max_fraction: input.max_fraction,
                    can_encroach: false,
                };

                // Re-center on the query origin so the per-shape cast stays in float precision far from the origin
                let transform = to_relative_transform(get_body_transform_quick(world, shape.body_id), origin);

                let output = shape_cast_shape(shape, transform, &local_input);

                if output.hit {
                    let id = ShapeId { index1: shape_id + 1, world0: world.world_id, generation: shape.generation };
                    let material_index = clamp_int(output.material_index, 0, shape_material_count(shape) - 1);
                    let user_material_id = get_shape_materials(shape)[material_index as usize].user_material_id;

                    let triangle_index = output.triangle_index;
                    let child_index = output.child_index;
                    let new_fraction = fcn(
                        id,
                        offset_pos(origin, output.point),
                        output.normal,
                        output.fraction,
                        user_material_id,
                        triangle_index,
                        child_index,
                    );

                    // The user may return -1 to skip this shape
                    if 0.0 <= new_fraction && new_fraction <= 1.0 {
                        *fraction = new_fraction;
                    }

                    return new_fraction;
                }

                input.max_fraction
            };

            let tree_result =
                dynamic_tree_box_cast(&world.broad_phase.trees[i], &tree_input, filter.mask_bits, false, &mut callback);
            tree_stats.node_visits += tree_result.node_visits;
            tree_stats.leaf_visits += tree_result.leaf_visits;
        }

        if fraction == 0.0 {
            break;
        }

        tree_input.max_fraction = fraction;
    }

    tree_stats
}

/// C: b3World_CastMover.
pub fn world_cast_mover(
    world: &World,
    origin: crate::math_functions::Pos,
    mover: &crate::types::Capsule,
    translation: MVec3,
    filter: QueryFilter,
    mut fcn: Option<&mut dyn FnMut(ShapeId) -> bool>,
) -> f32 {
    b3_assert!(is_valid_position(origin));
    b3_assert!(is_valid_vec3(translation));

    if world.locked {
        b3_assert!(!world.locked);
        return 1.0;
    }

    let mut fraction = 1.0f32;

    let centers = [mover.center1, mover.center2];
    let can_encroach = mover.radius > 0.0;

    // Bound the capsule in origin relative space then lift to a conservative world float box
    let mut tree_input = crate::types::BoxCastInput {
        box_: offset_aabb(make_aabb(&centers, mover.radius), origin),
        translation,
        max_fraction: 1.0,
    };

    for i in 0..BODY_TYPE_COUNT {
        {
            let fraction = &mut fraction;
            let fcn = &mut fcn;
            let mut callback = |input: &crate::types::BoxCastInput, _proxy_id: i32, user_data: u64| -> f32 {
                let shape_id = user_data as i32;
                let shape = &world.shapes[shape_id as usize];

                if !should_query_collide(shape.filter, filter) {
                    return *fraction;
                }

                if let Some(filter_fcn) = fcn.as_mut() {
                    let id = ShapeId { index1: shape_id + 1, world0: world.world_id, generation: shape.generation };
                    let should_collide = filter_fcn(id);
                    if !should_collide {
                        return *fraction;
                    }
                }

                // Rebuild from the origin relative input, taking only the advancing fraction from the tree
                let local_input = ShapeCastInput {
                    proxy: ShapeProxy { points: &centers, radius: mover.radius },
                    translation,
                    max_fraction: input.max_fraction,
                    can_encroach,
                };

                // Re-center on the query origin so the per-shape cast stays in float precision far from the origin
                let transform = to_relative_transform(get_body_transform_quick(world, shape.body_id), origin);

                let output = shape_cast_shape(shape, transform, &local_input);
                if output.fraction == 0.0 {
                    // Ignore overlapping shapes
                    return *fraction;
                }

                *fraction = output.fraction;
                output.fraction
            };

            dynamic_tree_box_cast(&world.broad_phase.trees[i], &tree_input, filter.mask_bits, false, &mut callback);
        }

        if fraction == 0.0 {
            break;
        }

        tree_input.max_fraction = fraction;
    }

    fraction
}

/// C: b3World_Explode. The C version processes shapes inside the tree query
/// callback; the port collects the candidate shapes first (the tree is not
/// modified by waking bodies), then applies the impulses in the same order.
pub fn world_explode(world: &mut World, explosion_def: &ExplosionDef) {
    let mask_bits = explosion_def.mask_bits;
    let position = explosion_def.position;
    let radius = explosion_def.radius;
    let falloff = explosion_def.falloff;
    let impulse_per_area = explosion_def.impulse_per_area;

    b3_assert!(is_valid_position(position));
    b3_assert!(is_valid_float(radius) && radius >= 0.0);
    b3_assert!(is_valid_float(falloff) && falloff >= 0.0);
    b3_assert!(is_valid_float(impulse_per_area));

    if world.locked {
        b3_assert!(!world.locked);
        return;
    }

    // Locked due to waking
    world.locked = true;

    // The broad-phase tree is float, so translate a local query box out to world with outward rounding
    let extent = radius + falloff;
    let local_box = AABB {
        lower_bound: vec3(-extent, -extent, -extent),
        upper_bound: vec3(extent, extent, extent),
    };
    let aabb = offset_aabb(local_box, position);

    let mut shape_ids: Vec<i32> = Vec::new();
    {
        let mut callback = |_proxy_id: i32, user_data: u64| -> bool {
            shape_ids.push(user_data as i32);
            true
        };
        dynamic_tree_query(
            &world.broad_phase.trees[BodyType::Dynamic as usize],
            aabb,
            mask_bits,
            false,
            &mut callback,
        );
    }

    for shape_id in shape_ids {
        // C: ExplosionCallback
        let shape = world.shapes[shape_id as usize].clone();
        if shape.explosion_scale == 0.0 {
            continue;
        }

        let body_id = shape.body_id;
        b3_assert!(world.bodies[body_id as usize].body_type == BodyType::Dynamic);

        let xf = get_body_transform_quick(world, body_id);

        // Re-center the explosion into the shape local frame so distance and direction stay precise
        // far from the origin. Everything below runs in that near-origin frame.
        let local_position = inv_transform_world_point(xf, position);

        let mut proxy_buffer = [MVec3::ZERO; 2];
        let local_points = [local_position];
        let input = DistanceInput {
            proxy_a: make_shape_proxy(&shape, &mut proxy_buffer),
            proxy_b: ShapeProxy { points: &local_points, radius: 0.0 },
            transform: Transform::IDENTITY,
            use_radii: true,
        };

        let mut cache = SimplexCache::default();
        let output = crate::distance::shape_distance(&input, &mut cache, None);

        if output.distance > radius + falloff {
            continue;
        }

        wake_body(world, body_id);

        if world.bodies[body_id as usize].set_index != AWAKE_SET {
            continue;
        }

        // Witness point is already in the body local query frame
        let mut closest_point = output.point_a;
        if output.distance == 0.0 {
            closest_point = get_shape_centroid(&shape);
        }

        let mut direction = sub(closest_point, local_position);
        if length_squared(direction) > 100.0 * f32::EPSILON * f32::EPSILON {
            direction = normalize(direction);
        } else {
            direction = vec3(1.0, 0.0, 0.0);
        }

        let area = get_shape_projected_area(&shape, direction);
        let mut scale = 1.0;
        if output.distance > radius && falloff > 0.0 {
            scale = clamp_float((radius + falloff - output.distance) / falloff, 0.0, 1.0);
        }

        let magnitude = impulse_per_area * area * scale * shape.explosion_scale;
        let impulse = mul_sv(magnitude, rotate_vector(xf.q, direction));

        let local_index = world.bodies[body_id as usize].local_index;
        let body_sim = world.solver_sets[AWAKE_SET as usize].body_sims[local_index as usize];
        let state = &mut world.solver_sets[AWAKE_SET as usize].body_states[local_index as usize];
        state.linear_velocity = mul_add(state.linear_velocity, body_sim.inv_mass, impulse);

        // Lever arm from the center of mass to the closest point, rotated to world
        let r = rotate_vector(xf.q, sub(closest_point, body_sim.local_center));
        state.angular_velocity = add(state.angular_velocity, mul_mv(body_sim.inv_inertia_world, cross(r, impulse)));
    }

    world.locked = false;
}

// ---------------------------------------------------------------------------
// Validation (C: B3_ENABLE_VALIDATION; the port runs these in debug builds)
// ---------------------------------------------------------------------------

/// This validates island graph connectivity for each body.
pub fn validate_connectivity(world: &World) {
    if !cfg!(debug_assertions) {
        return;
    }

    let body_capacity = world.bodies.len() as i32;

    for body_index in 0..body_capacity {
        let body = &world.bodies[body_index as usize];
        if body.id == NULL_INDEX {
            validate_free_id(&world.body_id_pool, body_index);
            continue;
        }

        b3_assert!(body_index == body.id);

        // Need to get the root island because islands are not merged until the next time step
        let body_island_id = body.island_id;
        let body_set_index = body.set_index;

        let mut contact_key = body.head_contact_key;
        while contact_key != NULL_INDEX {
            let contact_id = contact_key >> 1;
            let edge_index = contact_key & 1;

            let contact = &world.contacts[contact_id as usize];

            let touching = (contact.flags & CONTACT_TOUCHING_FLAG) != 0;
            if touching {
                if body_set_index != STATIC_SET {
                    let contact_island_id = contact.island_id;
                    b3_assert!(contact_island_id == body_island_id);
                }
            } else {
                b3_assert!(contact.island_id == NULL_INDEX);
            }

            contact_key = contact.edges[edge_index as usize].next_key;
        }

        let mut joint_key = body.head_joint_key;
        while joint_key != NULL_INDEX {
            let joint_id = joint_key >> 1;
            let edge_index = joint_key & 1;

            let joint = &world.joints[joint_id as usize];

            let other_edge_index = edge_index ^ 1;

            let other_body = &world.bodies[joint.edges[other_edge_index as usize].body_id as usize];

            if body_set_index == DISABLED_SET || other_body.set_index == DISABLED_SET {
                b3_assert!(joint.island_id == NULL_INDEX);
            } else if body_set_index == STATIC_SET {
                // Intentional nesting
                if other_body.set_index == STATIC_SET {
                    b3_assert!(joint.island_id == NULL_INDEX);
                }
            } else if body.body_type != BodyType::Dynamic && other_body.body_type != BodyType::Dynamic {
                b3_assert!(joint.island_id == NULL_INDEX);
            } else {
                let joint_island_id = joint.island_id;
                b3_assert!(joint_island_id == body_island_id);
            }

            joint_key = joint.edges[edge_index as usize].next_key;
        }
    }
}

/// Validates solver sets, but not island connectivity.
pub fn validate_solver_sets(world: &World) {
    if !cfg!(debug_assertions) {
        return;
    }

    b3_assert!(get_id_capacity(&world.body_id_pool) == world.bodies.len() as i32);
    b3_assert!(get_id_capacity(&world.contact_id_pool) == world.contacts.len() as i32);
    b3_assert!(get_id_capacity(&world.joint_id_pool) == world.joints.len() as i32);
    b3_assert!(get_id_capacity(&world.island_id_pool) == world.islands.len() as i32);
    b3_assert!(get_id_capacity(&world.solver_set_id_pool) == world.solver_sets.len() as i32);

    let mut active_set_count = 0;
    let mut total_body_count = 0;
    let mut total_joint_count = 0;
    let mut total_contact_count = 0;
    let mut total_island_count = 0;

    // Validate all solver sets
    let set_count = world.solver_sets.len() as i32;
    for set_index in 0..set_count {
        let set = &world.solver_sets[set_index as usize];
        if set.set_index != NULL_INDEX {
            active_set_count += 1;

            if set_index == STATIC_SET {
                b3_assert!(set.contact_indices.is_empty());
                b3_assert!(set.island_sims.is_empty());
                b3_assert!(set.body_states.is_empty());
            } else if set_index == DISABLED_SET {
                b3_assert!(set.island_sims.is_empty());
                b3_assert!(set.body_states.is_empty());
            } else if set_index == AWAKE_SET {
                b3_assert!(set.body_sims.len() == set.body_states.len());
                b3_assert!(set.joint_sims.is_empty());
            } else {
                b3_assert!(set.body_states.is_empty());
            }

            // Validate bodies
            {
                total_body_count += set.body_sims.len() as i32;
                for i in 0..set.body_sims.len() {
                    let body_sim = &set.body_sims[i];

                    let body_id = body_sim.body_id;
                    b3_assert!(0 <= body_id && body_id < world.bodies.len() as i32);
                    let body = &world.bodies[body_id as usize];
                    b3_assert!(body.set_index == set_index);
                    b3_assert!(body.local_index == i as i32);

                    let synced_flags = body.flags & !BODY_TRANSIENT_FLAGS;
                    b3_assert!((body_sim.flags & synced_flags) == synced_flags);

                    // C: b3GetBodyState(world, body) — only awake bodies have a state
                    if set_index == AWAKE_SET {
                        let body_state = &set.body_states[i];
                        b3_assert!((body_state.flags & synced_flags) == synced_flags);
                    }

                    if body.body_type == BodyType::Dynamic {
                        b3_assert!(body.flags & DYNAMIC_FLAG != 0);
                    }

                    if set_index == DISABLED_SET {
                        b3_assert!(body.head_contact_key == NULL_INDEX);
                    }

                    // Validate body shapes
                    let mut prev_shape_id = NULL_INDEX;
                    let mut shape_id = body.head_shape_id;
                    while shape_id != NULL_INDEX {
                        let shape = &world.shapes[shape_id as usize];
                        b3_assert!(shape.id == shape_id);
                        b3_assert!(shape.prev_shape_id == prev_shape_id);

                        if set_index == DISABLED_SET {
                            b3_assert!(shape.proxy_key == NULL_INDEX);
                        } else if set_index == STATIC_SET {
                            b3_assert!(crate::broad_phase::proxy_type(shape.proxy_key) == BodyType::Static);
                        } else {
                            let proxy_type = crate::broad_phase::proxy_type(shape.proxy_key);
                            b3_assert!(proxy_type == BodyType::Kinematic || proxy_type == BodyType::Dynamic);
                        }

                        prev_shape_id = shape_id;
                        shape_id = shape.next_shape_id;
                    }

                    // Validate body contacts
                    let mut contact_key = body.head_contact_key;
                    while contact_key != NULL_INDEX {
                        let contact_id = contact_key >> 1;
                        let edge_index = contact_key & 1;

                        let contact = &world.contacts[contact_id as usize];
                        b3_assert!(contact.set_index != STATIC_SET);
                        b3_assert!(contact.edges[0].body_id == body_id || contact.edges[1].body_id == body_id);
                        contact_key = contact.edges[edge_index as usize].next_key;
                    }

                    // Validate body joints
                    let mut joint_key = body.head_joint_key;
                    while joint_key != NULL_INDEX {
                        let joint_id = joint_key >> 1;
                        let edge_index = joint_key & 1;

                        let joint = &world.joints[joint_id as usize];

                        let other_edge_index = edge_index ^ 1;

                        let other_body = &world.bodies[joint.edges[other_edge_index as usize].body_id as usize];

                        if set_index == DISABLED_SET || other_body.set_index == DISABLED_SET {
                            b3_assert!(joint.set_index == DISABLED_SET);
                        } else if set_index == STATIC_SET && other_body.set_index == STATIC_SET {
                            b3_assert!(joint.set_index == STATIC_SET);
                        } else if body.body_type != BodyType::Dynamic && other_body.body_type != BodyType::Dynamic {
                            b3_assert!(joint.set_index == STATIC_SET);
                        } else if set_index == AWAKE_SET {
                            b3_assert!(joint.set_index == AWAKE_SET);
                        } else if set_index >= FIRST_SLEEPING_SET {
                            b3_assert!(joint.set_index == set_index);
                        }

                        let joint_sim = crate::joint::get_joint_sim_from_id(world, joint_id);
                        b3_assert!(joint_sim.joint_id == joint_id);
                        b3_assert!(joint_sim.body_id_a == joint.edges[0].body_id);
                        b3_assert!(joint_sim.body_id_b == joint.edges[1].body_id);

                        joint_key = joint.edges[edge_index as usize].next_key;
                    }
                }
            }

            // Validate contacts
            {
                total_contact_count += set.contact_indices.len() as i32;
                for i in 0..set.contact_indices.len() {
                    let contact_index = set.contact_indices[i];
                    let contact = &world.contacts[contact_index as usize];
                    if set_index == AWAKE_SET {
                        // contact should be non-touching if awake
                        // or it could be this contact hasn't been transferred yet
                        b3_assert!(contact.manifold_count() == 0 || (contact.flags & SIM_STARTED_TOUCHING) != 0);
                    }
                    b3_assert!(contact.set_index == set_index);
                    b3_assert!(contact.color_index == NULL_INDEX);
                    b3_assert!(contact.local_index == i as i32);
                }
            }

            // Validate joints
            {
                total_joint_count += set.joint_sims.len() as i32;
                for i in 0..set.joint_sims.len() {
                    let joint_sim = &set.joint_sims[i];
                    let joint = &world.joints[joint_sim.joint_id as usize];
                    b3_assert!(joint.set_index == set_index);
                    b3_assert!(joint.color_index == NULL_INDEX);
                    b3_assert!(joint.local_index == i as i32);
                }
            }

            // Validate islands
            {
                total_island_count += set.island_sims.len() as i32;
                for i in 0..set.island_sims.len() {
                    let island_sim = &set.island_sims[i];
                    let island = &world.islands[island_sim.island_id as usize];
                    b3_assert!(island.set_index == set_index);
                    b3_assert!(island.local_index == i as i32);
                }
            }
        } else {
            b3_assert!(set.body_sims.is_empty());
            b3_assert!(set.contact_indices.is_empty());
            b3_assert!(set.joint_sims.is_empty());
            b3_assert!(set.island_sims.is_empty());
            b3_assert!(set.body_states.is_empty());
        }
    }

    let set_id_count = get_id_count(&world.solver_set_id_pool);
    b3_assert!(active_set_count == set_id_count);

    let body_id_count = get_id_count(&world.body_id_pool);
    b3_assert!(total_body_count == body_id_count);

    let island_id_count = get_id_count(&world.island_id_pool);
    b3_assert!(total_island_count == island_id_count);

    // Validate constraint graph
    for color_index in 0..GRAPH_COLOR_COUNT as i32 {
        let color = &world.constraint_graph.colors[color_index as usize];
        let mut bit_count = 0;

        total_contact_count += color.convex_contacts.len() as i32;
        for i in 0..color.convex_contacts.len() {
            let contact_id = color.convex_contacts[i];
            let contact = &world.contacts[contact_id as usize];
            // contact should be touching in the constraint graph or awaiting transfer to non-touching
            b3_assert!(contact.manifold_count() > 0 || (contact.flags & (SIM_STOPPED_TOUCHING | SIM_DISJOINT)) != 0);
            b3_assert!(contact.set_index == AWAKE_SET);
            b3_assert!(contact.color_index == color_index);
            b3_assert!(contact.local_index == i as i32);

            let body_id_a = contact.edges[0].body_id;
            let body_id_b = contact.edges[1].body_id;

            if color_index < OVERFLOW_INDEX {
                let body_a = &world.bodies[body_id_a as usize];
                let body_b = &world.bodies[body_id_b as usize];
                b3_assert!(get_bit(&color.body_set, body_id_a as u32) == (body_a.body_type == BodyType::Dynamic));
                b3_assert!(get_bit(&color.body_set, body_id_b as u32) == (body_b.body_type == BodyType::Dynamic));

                bit_count += if body_a.body_type == BodyType::Dynamic { 1 } else { 0 };
                bit_count += if body_b.body_type == BodyType::Dynamic { 1 } else { 0 };
            }
        }

        total_contact_count += color.contacts.len() as i32;
        for i in 0..color.contacts.len() {
            let contact_id = color.contacts[i].contact_id;
            let contact = &world.contacts[contact_id as usize];
            // contact should be touching in the constraint graph or awaiting transfer to non-touching
            b3_assert!(contact.manifold_count() > 0 || (contact.flags & (SIM_STOPPED_TOUCHING | SIM_DISJOINT)) != 0);
            b3_assert!(contact.set_index == AWAKE_SET);
            b3_assert!(contact.color_index == color_index);
            b3_assert!(contact.local_index == i as i32);

            let body_id_a = contact.edges[0].body_id;
            let body_id_b = contact.edges[1].body_id;

            if color_index < OVERFLOW_INDEX {
                let body_a = &world.bodies[body_id_a as usize];
                let body_b = &world.bodies[body_id_b as usize];
                b3_assert!(get_bit(&color.body_set, body_id_a as u32) == (body_a.body_type == BodyType::Dynamic));
                b3_assert!(get_bit(&color.body_set, body_id_b as u32) == (body_b.body_type == BodyType::Dynamic));

                bit_count += if body_a.body_type == BodyType::Dynamic { 1 } else { 0 };
                bit_count += if body_b.body_type == BodyType::Dynamic { 1 } else { 0 };
            }
        }

        total_joint_count += color.joint_sims.len() as i32;
        for i in 0..color.joint_sims.len() {
            let joint_sim = &color.joint_sims[i];
            let joint = &world.joints[joint_sim.joint_id as usize];
            b3_assert!(joint.set_index == AWAKE_SET);
            b3_assert!(joint.color_index == color_index);
            b3_assert!(joint.local_index == i as i32);

            let body_id_a = joint.edges[0].body_id;
            let body_id_b = joint.edges[1].body_id;

            if color_index < OVERFLOW_INDEX {
                let body_a = &world.bodies[body_id_a as usize];
                let body_b = &world.bodies[body_id_b as usize];
                b3_assert!(get_bit(&color.body_set, body_id_a as u32) == (body_a.body_type == BodyType::Dynamic));
                b3_assert!(get_bit(&color.body_set, body_id_b as u32) == (body_b.body_type == BodyType::Dynamic));

                bit_count += if body_a.body_type == BodyType::Dynamic { 1 } else { 0 };
                bit_count += if body_b.body_type == BodyType::Dynamic { 1 } else { 0 };
            }
        }

        // Validate the bit population for this graph color
        b3_assert!(bit_count == count_set_bits(&color.body_set));
    }

    let contact_id_count = get_id_count(&world.contact_id_pool);
    b3_assert!(total_contact_count == contact_id_count);
    b3_assert!(total_contact_count == world.broad_phase.pair_set.count as i32);

    let joint_id_count = get_id_count(&world.joint_id_pool);
    b3_assert!(total_joint_count == joint_id_count);
}

/// Validate contact touching status.
pub fn validate_contacts(world: &World) {
    if !cfg!(debug_assertions) {
        return;
    }

    let contact_count = world.contacts.len() as i32;
    b3_assert!(contact_count == get_id_capacity(&world.contact_id_pool));
    let mut allocated_contact_count = 0;

    for contact_index in 0..contact_count {
        let contact = &world.contacts[contact_index as usize];
        if contact.contact_id == NULL_INDEX {
            continue;
        }

        b3_assert!(contact.contact_id == contact_index);

        allocated_contact_count += 1;

        let touching = (contact.flags & CONTACT_TOUCHING_FLAG) != 0;

        let set_id = contact.set_index;
        let set = &world.solver_sets[set_id as usize];

        if set_id == AWAKE_SET {
            if touching {
                b3_assert!(0 <= contact.color_index && contact.color_index < GRAPH_COLOR_COUNT as i32);
                // Validate body sim indices
                let shape_a = &world.shapes[contact.shape_id_a as usize];
                let shape_b = &world.shapes[contact.shape_id_b as usize];

                let body_a = &world.bodies[shape_a.body_id as usize];
                let body_b = &world.bodies[shape_b.body_id as usize];

                if body_a.body_type == BodyType::Static {
                    b3_assert!(contact.body_sim_index_a == NULL_INDEX);
                } else {
                    b3_assert!(contact.body_sim_index_a == body_a.local_index);
                }

                if body_b.body_type == BodyType::Static {
                    b3_assert!(contact.body_sim_index_b == NULL_INDEX);
                } else {
                    b3_assert!(contact.body_sim_index_b == body_b.local_index);
                }

                if (contact.flags & SIM_MESH_CONTACT) != 0 || contact.color_index == OVERFLOW_INDEX {
                    let color = &world.constraint_graph.colors[contact.color_index as usize];
                    let contact_id = color.contacts[contact.local_index as usize].contact_id;
                    b3_assert!(contact_id == contact_index);
                } else {
                    let color = &world.constraint_graph.colors[contact.color_index as usize];
                    let contact_id = color.convex_contacts[contact.local_index as usize];
                    b3_assert!(contact_id == contact_index);
                }
            } else {
                b3_assert!(contact.color_index == NULL_INDEX);
                b3_assert!(contact.manifolds.is_empty());

                let index = set.contact_indices[contact.local_index as usize];
                b3_assert!(index == contact_index);
            }
        } else if set_id >= FIRST_SLEEPING_SET {
            // Only touching contacts allowed in a sleeping set
            b3_assert!(touching);
            b3_assert!(!contact.manifolds.is_empty());
            let index = set.contact_indices[contact.local_index as usize];
            b3_assert!(index == contact_index);
        } else {
            // Sleeping and non-touching contacts belong in the disabled set
            b3_assert!(!touching && set_id == DISABLED_SET);
            b3_assert!(contact.manifolds.is_empty());
            let index = set.contact_indices[contact.local_index as usize];
            b3_assert!(index == contact_index);
        }

        if (contact.flags & SIM_MESH_CONTACT) != 0 {
            let cache_count = contact.mesh_contact.triangle_cache.len();
            if cache_count > 0 {
                let shape_a = &world.shapes[contact.shape_id_a as usize];
                match &shape_a.geom {
                    crate::shape::ShapeGeometry::Mesh(mesh) => {
                        let triangle_count = mesh.data.triangle_count();
                        for i in 0..cache_count {
                            let triangle_index = contact.mesh_contact.triangle_cache[i].triangle_index;
                            b3_assert!(0 <= triangle_index && triangle_index < triangle_count);
                        }
                    }
                    crate::shape::ShapeGeometry::HeightField(height_field) => {
                        let triangle_count = crate::height_field::get_height_field_triangle_count(height_field);
                        for i in 0..cache_count {
                            let triangle_index = contact.mesh_contact.triangle_cache[i].triangle_index;
                            b3_assert!(0 <= triangle_index && triangle_index < triangle_count);
                        }
                    }
                    crate::shape::ShapeGeometry::Compound(compound) => {
                        let child = crate::compound::get_compound_child(compound, contact.child_index);
                        b3_assert!(child.shape_type() == crate::types::ShapeType::Mesh);

                        let triangle_count = match &child.geom {
                            crate::types::ChildShapeGeom::Mesh(mesh) => mesh.data.triangle_count(),
                            _ => unreachable!(),
                        };
                        for i in 0..cache_count {
                            let triangle_index = contact.mesh_contact.triangle_cache[i].triangle_index;
                            b3_assert!(0 <= triangle_index && triangle_index < triangle_count);
                        }
                    }
                    _ => {
                        b3_assert!(false);
                    }
                }
            }
        }
    }

    let contact_id_count = get_id_count(&world.contact_id_pool);
    b3_assert!(allocated_contact_count == contact_id_count);
}
