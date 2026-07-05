// Port of box3d/src/solver.h (+ solver.c to be ported below the structs).
//
// Threading redesign (see PORTING.md): the C solver partitions work into blocks
// claimed by workers via atomic CAS. The Rust port keeps the stage/block
// structure but executes blocks serially in order, so the atomics
// (syncIndex/completionCount/atomicSyncBits/mainClaimed) are dropped.
//
// Pointer redesign: the C b3StepContext holds raw pointers into world data
// (states/sims shortcuts) and arena allocations (constraints, spans). The Rust
// StepContext OWNS those arrays: during solve the awake-set body_states and
// body_sims Vecs are moved (std::mem::take) from the world into the context and
// moved back afterwards. Per-color constraint pointers become (start, count)
// ranges into the context arrays, and span `contacts`/`joints` pointers become
// the owning color index.

use crate::math_functions::PI;

/// Solver stages
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SolverStageType {
    PrepareJoints,
    PrepareWideContacts,
    PrepareContacts,
    IntegrateVelocities,
    WarmStart,
    Solve,
    IntegratePositions,
    Relax,
    Restitution,
    StoreWideImpulses,
    StoreImpulses,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SolverBlockType {
    /// Block for iterating across bodies.
    Body,
    /// Block for iterating across joints. For prepare.
    Joint,
    /// Block for iterating across wide contacts. For prepare and store.
    WideContact,
    /// Block for iterating across contacts. For prepare and store.
    Contact,
    /// Block for iterating across joints of a single graph color.
    GraphJoint,
    /// Block for iterating across wide contacts of a single graph color.
    GraphWideContact,
    /// Block for iterating across contacts of a single graph color.
    GraphContact,
    /// Block for processing overflow constraints.
    Overflow,
}

/// Solver block describes a unit of work.
#[derive(Clone, Copy, Debug)]
pub struct SolverBlock {
    pub start_index: i32,
    pub count: u16,
    pub block_type: SolverBlockType,
    pub color_index: u8,
}

/// Each stage must be completed before going to the next stage.
/// The blocks are a (start, count) range into StepContext::blocks — the flat
/// block array is reused across steps (C allocates the b3SyncBlock arrays from
/// the arena; stages that share a block array in C share the range here).
#[derive(Clone, Copy, Debug)]
pub struct SolverStage {
    pub blocks_start: i32,
    pub blocks_count: i32,
    pub stage_type: SolverStageType,
    pub color_index: u8,
}

/// Constraint softness
#[derive(Clone, Copy, Debug, Default)]
pub struct Softness {
    pub bias_rate: f32,
    pub mass_scale: f32,
    pub impulse_scale: f32,
}

/// Prepare/store run as a flat parallel-for over the whole wide-constraint
/// range. Each span maps a slice of that range back to the owning color's
/// contacts. `color_index` replaces the C `int* contacts` pointer: the contacts
/// live at graph.colors[color_index].convex_contacts.
#[derive(Clone, Copy, Debug, Default)]
pub struct WidePrepareSpan {
    pub start: i32,
    pub count: i32,
    pub color_index: i32,
}

/// `color_index` replaces the C `b3ContactSpec* contacts` pointer: the specs
/// live at graph.colors[color_index].contacts.
#[derive(Clone, Copy, Debug, Default)]
pub struct ContactPrepareSpan {
    pub start: i32,
    pub count: i32,
    pub color_index: i32,
}

/// `color_index` replaces the C `b3JointSim* joints` pointer: the joints live
/// at graph.colors[color_index].joint_sims.
#[derive(Clone, Copy, Debug, Default)]
pub struct JointPrepareSpan {
    pub start: i32,
    pub count: i32,
    pub color_index: i32,
}

/// Context for a time step. Recreated each time step.
#[derive(Default)]
pub struct StepContext {
    /// time step
    pub dt: f32,

    /// inverse time step (0 if dt == 0).
    pub inv_dt: f32,

    /// sub-step
    pub h: f32,
    pub inv_h: f32,

    pub sub_step_count: i32,

    pub contact_softness: Softness,
    pub static_softness: Softness,

    pub restitution_threshold: f32,
    pub max_linear_velocity: f32,

    /// Shortcut to body states from the awake set: moved out of the world
    /// (std::mem::take) for the duration of the solve, then moved back.
    pub states: Vec<crate::body::BodyState>,

    /// Shortcut to body sims from the awake set: moved out of the world for
    /// the duration of the solve, then moved back.
    pub sims: Vec<crate::body::BodySim>,

    /// array of all shape ids for shapes that have enlarged AABBs
    pub enlarged_shapes: Vec<i32>,

    /// Array of bullet bodies that need continuous collision handling
    pub bullet_bodies: Vec<i32>,

    /// Contact ids for simplified parallel-for access. Used in narrow-phase.
    /// These contacts may or may not be touching. They are associated with awake bodies.
    pub awake_contact_indices: Vec<i32>,

    /// Flat wide contact constraint array used by prepare and store.
    /// prepare_spans has active_color_count + 1 entries, the last being a
    /// sentinel at wide_contact_count.
    pub wide_constraints: Vec<crate::contact_solver::ContactConstraintWide>,
    pub wide_prepare_spans: Vec<WidePrepareSpan>,
    pub wide_contact_count: i32,

    /// Similar for mesh/overflow contact constraints
    pub manifold_constraints: Vec<crate::contact_solver::ManifoldConstraint>,
    pub contact_constraints: Vec<crate::contact_solver::ContactConstraint>,
    pub contact_prepare_spans: Vec<ContactPrepareSpan>,
    pub overflow_spans: Vec<ContactPrepareSpan>,
    pub joint_prepare_spans: Vec<JointPrepareSpan>,

    pub active_color_count: i32,
    pub worker_count: i32,

    pub stages: Vec<SolverStage>,

    /// Flat solver block array referenced by the stage ranges.
    pub blocks: Vec<SolverBlock>,

    /// Per-block sync index, parallel to `blocks` (C: b3SyncBlock::syncIndex).
    /// Workers claim a block by CAS(previousSyncIndex, syncIndex); the indices
    /// grow monotonically per block group so blocks can be reused across
    /// sub-steps without resetting.
    pub block_sync: Vec<crate::sync::AtomicIndex>,

    /// Per-stage completion counter, parallel to `stages`
    /// (C: b3SolverStage::completionCount).
    pub stage_completion: Vec<crate::sync::AtomicIndex>,

    /// C: b3StepContext::atomicSyncBits — (sync index << 16) | stage index.
    /// Grows monotonically as the solve advances so a delayed worker catches up
    /// without repeating completed work. -1 is the shutdown sentinel
    /// (C uses UINT_MAX).
    pub atomic_sync_bits: crate::sync::AtomicIndex,

    /// C: b3StepContext::mainClaimed — the orchestrator slot race. The caller
    /// of world_step and the queued worker-0 task both race for this via CAS;
    /// the loser no-ops.
    pub main_claimed: crate::sync::AtomicIndex,

    pub enable_warm_starting: bool,
}

/// Persistent per-world solver scratch. The C engine allocates the step scratch
/// from a reusable arena; the port keeps the Vecs on the world so their
/// capacity survives across steps. world_step moves them into the StepContext
/// and back (see attach/detach). Contents are transient — everything is
/// cleared/overwritten each step, so reuse is value-identical to fresh
/// allocations.
#[derive(Default)]
pub struct SolverScratch {
    pub wide_constraints: Vec<crate::contact_solver::ContactConstraintWide>,
    pub contact_constraints: Vec<crate::contact_solver::ContactConstraint>,
    pub manifold_constraints: Vec<crate::contact_solver::ManifoldConstraint>,
    pub wide_prepare_spans: Vec<WidePrepareSpan>,
    pub contact_prepare_spans: Vec<ContactPrepareSpan>,
    pub overflow_spans: Vec<ContactPrepareSpan>,
    pub joint_prepare_spans: Vec<JointPrepareSpan>,
    pub bullet_bodies: Vec<i32>,
    pub awake_contact_indices: Vec<i32>,
    pub stages: Vec<SolverStage>,
    pub blocks: Vec<SolverBlock>,
    pub block_sync: Vec<crate::sync::AtomicIndex>,
    pub stage_completion: Vec<crate::sync::AtomicIndex>,
}

impl SolverScratch {
    /// Move the scratch buffers into a fresh step context (start of world_step).
    pub fn attach(&mut self, context: &mut StepContext) {
        context.wide_constraints = std::mem::take(&mut self.wide_constraints);
        context.contact_constraints = std::mem::take(&mut self.contact_constraints);
        context.manifold_constraints = std::mem::take(&mut self.manifold_constraints);
        context.wide_prepare_spans = std::mem::take(&mut self.wide_prepare_spans);
        context.contact_prepare_spans = std::mem::take(&mut self.contact_prepare_spans);
        context.overflow_spans = std::mem::take(&mut self.overflow_spans);
        context.joint_prepare_spans = std::mem::take(&mut self.joint_prepare_spans);
        context.bullet_bodies = std::mem::take(&mut self.bullet_bodies);
        context.awake_contact_indices = std::mem::take(&mut self.awake_contact_indices);
        context.stages = std::mem::take(&mut self.stages);
        context.blocks = std::mem::take(&mut self.blocks);
        context.block_sync = std::mem::take(&mut self.block_sync);
        context.stage_completion = std::mem::take(&mut self.stage_completion);
    }

    /// Move the scratch buffers back out of the context (end of world_step).
    pub fn detach(&mut self, context: &mut StepContext) {
        self.wide_constraints = std::mem::take(&mut context.wide_constraints);
        self.contact_constraints = std::mem::take(&mut context.contact_constraints);
        self.manifold_constraints = std::mem::take(&mut context.manifold_constraints);
        self.wide_prepare_spans = std::mem::take(&mut context.wide_prepare_spans);
        self.contact_prepare_spans = std::mem::take(&mut context.contact_prepare_spans);
        self.overflow_spans = std::mem::take(&mut context.overflow_spans);
        self.joint_prepare_spans = std::mem::take(&mut context.joint_prepare_spans);
        self.bullet_bodies = std::mem::take(&mut context.bullet_bodies);
        self.awake_contact_indices = std::mem::take(&mut context.awake_contact_indices);
        self.stages = std::mem::take(&mut context.stages);
        self.blocks = std::mem::take(&mut context.blocks);
        self.block_sync = std::mem::take(&mut context.block_sync);
        self.stage_completion = std::mem::take(&mut context.stage_completion);
    }
}

#[inline]
pub fn make_soft(hertz: f32, zeta: f32, h: f32) -> Softness {
    if hertz == 0.0 {
        return Softness { bias_rate: 0.0, mass_scale: 0.0, impulse_scale: 0.0 };
    }

    let omega = 2.0 * PI * hertz;
    let a1 = h.mul_add(omega, 2.0 * zeta);
    let a2 = h * omega * a1;
    let a3 = 1.0 / (1.0 + a2);

    // bias = w / (2 * z + hw)
    // massScale = hw * (2 * z + hw) / (1 + hw * (2 * z + hw))
    // impulseScale = 1 / (1 + hw * (2 * z + hw))
    // In all cases: massScale + impulseScale == 1

    Softness {
        bias_rate: omega / a1,
        mass_scale: a2 * a3,
        impulse_scale: a3,
    }
}

/// Copy-out/copy-in access to the awake body states shared across solver
/// workers. The C solver writes states through raw pointers; disjointness is
/// structural: within one graph-color stage no two constraints share a movable
/// body, and integrate/prepare stages partition the body/constraint ranges
/// into blocks each claimed by exactly one worker.
///
/// The accessors copy whole BodyState values (the pattern the stage code
/// already used), so no reference to shared memory outlives a call.
pub struct StateAccess<'a> {
    slice: crate::sync::SyncSlice<'a, crate::body::BodyState>,
}

impl<'a> StateAccess<'a> {
    #[inline]
    pub fn new(states: &'a mut [crate::body::BodyState]) -> StateAccess<'a> {
        StateAccess { slice: crate::sync::SyncSlice::new(states) }
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.slice.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.slice.is_empty()
    }

    /// Read the state at `i`. Caller guarantees no concurrent writer (graph
    /// coloring / block partitioning).
    #[inline]
    pub fn get(&self, i: usize) -> crate::body::BodyState {
        // SAFETY: stage structure guarantees no other thread mutates `i`
        // while this stage may read it (see struct docs).
        unsafe { *self.slice.get_ref(i) }
    }

    /// Borrow the state at `i`. Same contract as `get` (no concurrent writer
    /// for the lifetime of the reference — in the solver, for the duration of
    /// the current stage). Used by the gather path so the four lane states
    /// stay behind references instead of being copied out whole (the copies
    /// spill ~20 registers in the zip loop).
    #[inline]
    pub fn get_ref(&self, i: usize) -> &crate::body::BodyState {
        // SAFETY: see get(); the returned borrow is tied to &self, and the
        // stage structure guarantees no writer for the stage's duration.
        unsafe { self.slice.get_ref(i) }
    }

    /// Write the state at `i`. Caller guarantees exclusive access to `i` for
    /// the current stage (see struct docs).
    #[inline]
    pub fn set(&self, i: usize, state: crate::body::BodyState) {
        // SAFETY: see get().
        unsafe {
            *self.slice.get_mut(i) = state;
        }
    }

    /// Write only the velocity fields at `i` — C's joint solvers store the
    /// two vectors in place. In the ~1000-instruction joint solve bodies the
    /// full 56-byte round trip forces the untouched delta/flags fields to
    /// stay live across the whole body and be re-stored (disassembly: +110
    /// loads/+54 stores per joint vs C, the entirety of the joint_grid gap).
    /// Same exclusive-access contract as `set`; the caller must not hold any
    /// borrow of `i` across this call.
    #[inline]
    pub fn set_velocities(&self, i: usize, v: crate::math_functions::Vec3, w: crate::math_functions::Vec3) {
        // SAFETY: see set(); the &mut is created and dropped inside this call.
        unsafe {
            let state = self.slice.get_mut(i);
            state.linear_velocity = v;
            state.angular_velocity = w;
        }
    }
}

/// Solve-stage profile accumulators written only by the orchestrator
/// (mainClaimed winner); applied to world.profile after all tasks finish.
#[derive(Default)]
pub struct SolveProfile {
    pub prepare_constraints: f32,
    pub integrate_velocities: f32,
    pub warm_start: f32,
    pub solve_impulses: f32,
    pub integrate_positions: f32,
    pub relax_impulses: f32,
    pub apply_restitution: f32,
    pub store_impulses: f32,
}

/// The shared handle every solver stage function receives. This is the port of
/// the C pattern where all workers share b3StepContext*/b3World* pointers; the
/// mutable arrays are taken out of the world/context for the duration of the
/// constraint solve and exposed through disjoint-access views:
///
/// - `states`: mutated by integrate/warm-start/solve/relax/restitution stages.
///   Disjoint by graph coloring (no two constraints in a color share a movable
///   body) and by block partitioning (integrate stages).
/// - `contacts`: manifolds read by prepare, written by store. Each contact id
///   appears in exactly one color/overflow slot, and blocks partition them.
/// - `joint_colors[i]`: the graph color's joint sims (overflow included).
///   Blocks partition each color's range; overflow runs orchestrator-only.
/// - constraint arrays: blocks partition the flat ranges.
/// - `task_contexts`: indexed by worker_index only.
/// - `world`/`context`: read-only during the stages (the taken Vecs are empty
///   in place; `context.sims` and the spans stay readable). The world's user
///   callback Options are parked (taken) for the duration.
pub struct SolverShared<'a> {
    pub world: &'a crate::physics_world::World,
    pub context: &'a StepContext,
    pub states: StateAccess<'a>,
    pub contacts: crate::sync::SyncSlice<'a, crate::contact::Contact>,
    pub joint_colors: Vec<crate::sync::SyncSlice<'a, crate::joint::JointSim>>,
    pub wide_constraints: crate::sync::SyncSlice<'a, crate::contact_solver::ContactConstraintWide>,
    pub manifold_constraints: crate::sync::SyncSlice<'a, crate::contact_solver::ManifoldConstraint>,
    pub contact_constraints: crate::sync::SyncSlice<'a, crate::contact_solver::ContactConstraint>,
    pub task_contexts: crate::sync::SyncSlice<'a, crate::physics_world::TaskContext>,
    pub profile: crate::sync::SyncPtr<SolveProfile>,
}

/// C: UINT_MAX sync-bits sentinel — workers exit their spin loop.
/// -1 can never collide with valid sync bits (sync index > 0 makes them positive).
pub const SYNC_SENTINEL: i32 = -1;

// ---------------------------------------------------------------------------
// Port of solver.c
// ---------------------------------------------------------------------------
//
// Threading collapse: the C b3SolverTask machinery (worker CAS block claiming,
// atomicSyncBits, mainClaimed, spinners) executes serially here. The stage and
// block building logic is preserved 1:1 so the ordering matches C with one
// worker: worker 0's home start index is 0, so blocks run in array order.
//
// The awake set body states/sims are MOVED into the StepContext at the top of
// solve() (C keeps raw pointers) and moved back after the bullet stage — the
// last C access through stepContext->sims is the serial bullet proxy
// enlargement. Everything after (sensor hits, island sleep) accesses body sims
// through the world again, matching C's aliased view.
//
// Skipped: Tracy zones, recording hooks, the CCD stall logging
// (b3GetStallThreshold is not ported), and world->userTreeTask finalization
// (the Rust broad phase rebuilds trees inline; see broad_phase.rs).

use crate::b3_assert;
use crate::b3_validate;
use crate::bitset::{get_bit, in_place_union, set_bit, set_bit_count_and_clear};
use crate::body::{
    make_relative_sweep, should_bodies_collide, BodySim, ALLOW_FAST_ROTATION, BODY_TRANSIENT_FLAGS,
    ENABLE_SLEEP, ENLARGE_BOUNDS, HAD_TIME_OF_IMPACT, IS_BULLET, IS_FAST, IS_SPEED_CAPPED,
    LOCK_ANGULAR_X, LOCK_ANGULAR_Y, LOCK_ANGULAR_Z, LOCK_LINEAR_X, LOCK_LINEAR_Y, LOCK_LINEAR_Z,
};
use crate::broad_phase::{
    broad_phase_enlarge_proxy, buffer_move, proxy_id, proxy_type, validate_broad_phase, validate_no_enlarged,
};
use crate::constants::{speculative_distance, GRAPH_COLOR_COUNT, MAX_ROTATION, TIME_TO_SLEEP};
use crate::constraint_graph::OVERFLOW_INDEX;
use crate::core::NULL_INDEX;
use crate::ctz::ctz64;
use crate::id::{BodyId, ContactId, JointId, ShapeId};
use crate::id_pool::get_id_capacity;
use crate::math_functions::{
    abs, aabb_contains, aabb_union, add, dot, inv_rotate_vector, is_valid_vec3, length, lerp, lerp_position,
    make_matrix_from_quat, max_float, min_int, mul_add, mul_mm, mul_mv, mul_quat, mul_sv, neg, nlerp,
    normalize_quat, offset_aabb, offset_pos, rotate_vector, solve3, sub, transpose, vec3, invert_matrix,
    blend2, Matrix3, Quat, Transform, Vec3, AABB,
};
use crate::math_internal::{integrate_rotation, modified_cross};
use crate::physics_world::{World, AWAKE_SET};
use crate::sensor::{SensorHit, Visitor};
use crate::shape::{
    compute_fat_shape_aabb, compute_shape_aabb, get_shape_user_material_id, shape_time_of_impact,
    should_shapes_collide,
};
use crate::timer::{get_milliseconds, get_milliseconds_and_reset, get_ticks};
use crate::types::{BodyMoveEvent, BodyType, ContactHitEvent, JointEvent, ShapeType, DEFAULT_MASK_BITS};

// these are useful for solver testing
const ITERATIONS: i32 = 1;
const RELAX_ITERATIONS: i32 = 1;

const MAX_CONTINUOUS_SENSOR_HITS: usize = 8;

// B3_SIMD_WIDTH == 4
const SIMD_SHIFT: i32 = 2;
const SIMD_WIDTH: i32 = 4;

fn zero_body_move_event() -> BodyMoveEvent {
    BodyMoveEvent {
        user_data: 0,
        transform: crate::math_functions::WORLD_TRANSFORM_IDENTITY,
        body_id: crate::id::NULL_BODY_ID,
        fell_asleep: false,
    }
}

// Integrate velocities, apply damping, and gyroscopic torque
fn integrate_velocities_task(block: SolverBlock, shared: &SolverShared) {
    let context = shared.context;
    b3_validate!(((block.start_index + block.count as i32) as usize) <= shared.states.len());

    let states = &shared.states;
    let sims = &context.sims;

    let gravity = shared.world.gravity;
    let h = context.h;

    for i in block.start_index..block.start_index + block.count as i32 {
        let sim = &sims[i as usize];
        // Body blocks partition the awake range: this worker exclusively owns i.
        let mut state = states.get(i as usize);

        let mut v = state.linear_velocity;
        let mut w = state.angular_velocity;

        // Damping math
        // Differential equation: dv/dt + c * v = 0
        // Solution: v(t) = v0 * exp(-c * t)
        // Pade approximation:
        // v2 = v1 * 1 / (1 + c * dt)
        let linear_damping = 1.0 / h.mul_add(sim.linear_damping, 1.0);
        let angular_damping = 1.0 / h.mul_add(sim.angular_damping, 1.0);

        // Gravity scale will be zero for kinematic bodies
        let gravity_scale = if sim.inv_mass > 0.0 { sim.gravity_scale } else { 0.0 };

        let linear_velocity_delta = blend2(h * sim.inv_mass, sim.force, h * gravity_scale, gravity);
        v = mul_add(linear_velocity_delta, linear_damping, v);

        let angular_velocity_delta = mul_sv(h, mul_mv(sim.inv_inertia_world, sim.torque));
        w = mul_add(angular_velocity_delta, angular_damping, w);

        // Gyroscopic torque by solving this nonlinear equation using Newton-Raphson.
        // I * (w2 - w1) + h * cross(w2, I * w2) = 0
        // This is all done in local coordinates where the Jacobian is easier to compute.
        // This improves the simulation of long skinny bodies.
        {
            // Get current rotation.
            let q0 = sim.transform.q;
            let q = mul_quat(state.delta_rotation, q0);

            // todo wasteful computation
            let inertia_local = invert_matrix(sim.inv_inertia_local);

            // Compute local angular velocity
            let omega1 = inv_rotate_vector(q, w);
            let mut omega2 = omega1;

            // Symmetric inertia tensor: 6 unique entries (column-major)
            let i00 = inertia_local.cx.x;
            let i01 = inertia_local.cy.x;
            let i02 = inertia_local.cz.x;
            let i11 = inertia_local.cy.y;
            let i12 = inertia_local.cz.y;
            let i22 = inertia_local.cz.z;

            for _gyro_iteration in 0..1 {
                let w1 = omega2.x;
                let w2 = omega2.y;
                let w3 = omega2.z;

                // Iw = I * omega2 (shared between residual and Jacobian)
                let iw1 = i02.mul_add(w3, i01.mul_add(w2, i00 * w1));
                let iw2 = i12.mul_add(w3, i11.mul_add(w2, i01 * w1));
                let iw3 = i22.mul_add(w3, i12.mul_add(w2, i02 * w1));

                // Residual: b = I*(omega2 - omega1) + h * (omega2 x I*omega2)
                let dw = sub(omega2, omega1);
                let b = vec3(
                    h.mul_add(w2.mul_add(iw3, -(w3 * iw2)), i02.mul_add(dw.z, i01.mul_add(dw.y, i00 * dw.x))),
                    h.mul_add(w3.mul_add(iw1, -(w1 * iw3)), i12.mul_add(dw.z, i11.mul_add(dw.y, i01 * dw.x))),
                    h.mul_add(w1.mul_add(iw2, -(w2 * iw1)), i22.mul_add(dw.z, i12.mul_add(dw.y, i02 * dw.x))),
                );

                // Jacobian J = I + h * (skew(omega2) * I - skew(I*omega2))
                // Jacobian derived by Erin Catto, Ph.D. Do not attempt to do this without a Ph.D.
                let j = Matrix3 {
                    cx: vec3(
                        h.mul_add(w2.mul_add(i02, -(w3 * i01)), i00),
                        h.mul_add(w3.mul_add(i00, -(w1 * i02)) - iw3, i01),
                        h.mul_add(w1.mul_add(i01, -(w2 * i00)) + iw2, i02),
                    ),
                    cy: vec3(
                        h.mul_add(w2.mul_add(i12, -(w3 * i11)) + iw3, i01),
                        h.mul_add(w3.mul_add(i01, -(w1 * i12)), i11),
                        h.mul_add(w1.mul_add(i11, -(w2 * i01)) - iw1, i12),
                    ),
                    cz: vec3(
                        h.mul_add(w2.mul_add(i22, -(w3 * i12)) - iw2, i02),
                        h.mul_add(w3.mul_add(i02, -(w1 * i22)) + iw1, i12),
                        h.mul_add(w1.mul_add(i12, -(w2 * i02)), i22),
                    ),
                };

                omega2 = sub(omega2, solve3(j, b));
            }

            w = rotate_vector(q, omega2);
        }

        state.linear_velocity = v;
        state.angular_velocity = w;
        states.set(i as usize, state);
    }
}

fn integrate_positions_task(block: SolverBlock, shared: &SolverShared) {
    let context = shared.context;
    b3_validate!(((block.start_index + block.count as i32) as usize) <= shared.states.len());

    let states = &shared.states;
    let h = context.h;
    let max_linear_speed = context.max_linear_velocity;
    let max_angular_speed = MAX_ROTATION * context.inv_dt;
    let max_linear_speed_squared = max_linear_speed * max_linear_speed;
    let max_angular_speed_squared = max_angular_speed * max_angular_speed;

    for i in block.start_index..block.start_index + block.count as i32 {
        // Body blocks partition the awake range: this worker exclusively owns i.
        let mut state = states.get(i as usize);

        let mut v = state.linear_velocity;
        let mut w = state.angular_velocity;

        // Motion locks - these can be viewed as a constraint that comes last
        v.x = if state.flags & LOCK_LINEAR_X != 0 { 0.0 } else { v.x };
        v.y = if state.flags & LOCK_LINEAR_Y != 0 { 0.0 } else { v.y };
        v.z = if state.flags & LOCK_LINEAR_Z != 0 { 0.0 } else { v.z };
        w.x = if state.flags & LOCK_ANGULAR_X != 0 { 0.0 } else { w.x };
        w.y = if state.flags & LOCK_ANGULAR_Y != 0 { 0.0 } else { w.y };
        w.z = if state.flags & LOCK_ANGULAR_Z != 0 { 0.0 } else { w.z };

        // Clamp to max linear speed
        if dot(v, v) > max_linear_speed_squared {
            let ratio = max_linear_speed / length(v);
            v = mul_sv(ratio, v);
            state.flags |= IS_SPEED_CAPPED;
        }

        // Clamp to max angular speed
        if dot(w, w) > max_angular_speed_squared && (state.flags & ALLOW_FAST_ROTATION) == 0 {
            let ratio = max_angular_speed / length(w);
            w = mul_sv(ratio, w);
            state.flags |= IS_SPEED_CAPPED;
        }

        state.linear_velocity = v;
        state.angular_velocity = w;
        state.delta_position = mul_add(state.delta_position, h, v);
        state.delta_rotation = integrate_rotation(state.delta_rotation, mul_sv(h, w));

        states.set(i as usize, state);
    }
}

fn prepare_joints_task(block: SolverBlock, shared: &SolverShared) {
    let context = shared.context;
    let mut index = block.start_index;
    let end_index = block.start_index + block.count as i32;

    // Find color for start index. Linear search but fast.
    let mut color_span = 0usize;
    while context.joint_prepare_spans[color_span + 1].start <= index {
        color_span += 1;
    }

    // Loop over block
    while index < end_index {
        let color_start = context.joint_prepare_spans[color_span].start;
        let color_end_index = min_int(context.joint_prepare_spans[color_span + 1].start, end_index);
        let color_index = context.joint_prepare_spans[color_span].color_index;

        if index < color_end_index {
            let joints = &shared.joint_colors[color_index as usize];

            // Loop over color
            while index < color_end_index {
                b3_assert!(
                    0 <= index - color_start && index - color_start < context.joint_prepare_spans[color_span].count
                );
                // SAFETY: the prepare blocks partition the flat joint range,
                // so this worker exclusively owns this joint index.
                let joint = unsafe { joints.get_mut((index - color_start) as usize) };
                crate::joint::prepare_joint(joint, shared.world, context);
                index += 1;
            }
        }

        // Advance to next color
        color_span += 1;
    }
}

fn warm_start_joints_task(block: SolverBlock, shared: &SolverShared) {
    let joints = &shared.joint_colors[block.color_index as usize];

    for i in block.start_index..block.start_index + block.count as i32 {
        // SAFETY: blocks partition the color's joint range — exclusive index.
        let joint = unsafe { joints.get_mut(i as usize) };
        crate::joint::warm_start_joint(joint, &shared.states, shared.context);
    }
}

fn solve_joints_task(block: SolverBlock, shared: &SolverShared, use_bias: bool, worker_index: i32) {
    let context = shared.context;
    let joints = &shared.joint_colors[block.color_index as usize];

    b3_assert!(0 <= block.start_index && block.start_index + block.count as i32 <= joints.len() as i32);

    // SAFETY: worker_index identifies this task exclusively.
    let task_context = unsafe { shared.task_contexts.get_mut(worker_index as usize) };

    for i in block.start_index..block.start_index + block.count as i32 {
        // SAFETY: blocks partition the color's joint range — exclusive index.
        let joint = unsafe { joints.get_mut(i as usize) };
        crate::joint::solve_joint(joint, &shared.states, context, use_bias);

        if use_bias
            && (joint.force_threshold < f32::MAX || joint.torque_threshold < f32::MAX)
            && !get_bit(&task_context.joint_state_bit_set, joint.joint_id as u32)
        {
            let mut force = 0.0;
            let mut torque = 0.0;
            crate::joint::get_joint_reaction(shared.world, Some(context), joint, context.inv_h, &mut force, &mut torque);

            // Check thresholds. A zero threshold means all awake joints get reported.
            if force >= joint.force_threshold || torque >= joint.torque_threshold {
                // Flag this joint for processing.
                set_bit(&mut task_context.joint_state_bit_set, joint.joint_id as u32);
            }
        }
    }
}

// Continuous collision of dynamic versus static (and versus everything for bullets).
// C: b3SolveContinuous(world, bodySimIndex, taskContext). The fast body sim lives in
// context.sims during the solve; it is copied to a local and written back at the end.
fn solve_continuous(world: &mut World, context: &mut StepContext, body_sim_index: i32, worker_index: i32) {
    let mut fast_body_sim = context.sims[body_sim_index as usize];
    b3_assert!(fast_body_sim.flags & IS_FAST != 0);

    // Re-center the sweep on the fast body so the TOI and the swept query stay in float precision
    let base = fast_body_sim.center0;

    let sweep = make_relative_sweep(&fast_body_sim, base);

    // xf1 is only used by the dead centroid1 computation in C; kept for reference.
    let _xf1 = Transform {
        q: sweep.q1,
        p: sub(sweep.c1, rotate_vector(sweep.q1, sweep.local_center)),
    };

    let xf2 = Transform {
        q: sweep.q2,
        p: sub(sweep.c2, rotate_vector(sweep.q2, sweep.local_center)),
    };

    let fast_body_id = fast_body_sim.body_id;

    // b3ContinuousContext locals
    let mut ctx_fraction = 1.0f32;
    let mut sensor_hits = [SensorHit::default(); MAX_CONTINUOUS_SENSOR_HITS];
    let mut sensor_fractions = [0.0f32; MAX_CONTINUOUS_SENSOR_HITS];
    let mut sensor_count = 0usize;
    let mut distance_iterations = 0i32;
    let mut push_back_iterations = 0i32;
    let mut root_iterations = 0i32;

    let is_bullet = (fast_body_sim.flags & IS_BULLET) != 0;

    // The callbacks may run while the world is otherwise shared-borrowed.
    let mut pre_solve_fcn = world.pre_solve_fcn.take();
    let mut custom_filter_fcn = world.custom_filter_fcn.take();

    let mut shape_id = world.bodies[fast_body_id as usize].head_shape_id;
    while shape_id != NULL_INDEX {
        let fast_shape_id = shape_id;

        // Update the shape AABB first (C mutates fastShape->aabb before the queries).
        let (next_shape_id, box1, box2, fast_shape_type, fast_sensor_index) = {
            let shape = &world.shapes[fast_shape_id as usize];
            let box1 = shape.aabb;
            // xf2 is relative to the base, so translate the box back to world space, rounding outward
            let box2 = offset_aabb(compute_shape_aabb(shape, xf2), base);
            (shape.next_shape_id, box1, box2, shape.shape_type(), shape.sensor_index)
        };

        // Store this to avoid double computation in the case there is no impact event
        world.shapes[fast_shape_id as usize].aabb = box2;

        shape_id = next_shape_id;

        // Note: C computes context.centroid1/centroid2 here; they are never read.

        // No continuous collision for meshes
        if fast_shape_type == ShapeType::Mesh || fast_shape_type == ShapeType::Height {
            continue;
        }

        // No continuous collision for sensors
        if fast_sensor_index != NULL_INDEX {
            continue;
        }

        let swept_box = aabb_union(box1, box2);

        // This is called from dynamic_tree_query for continuous collision
        // (C: b3ContinuousQueryCallback)
        {
            let world_ref: &World = world;
            let sims: &[BodySim] = &context.sims;

            let mut callback = |_proxy_id: i32, user_data: u64| -> bool {
                let hit_shape_id = user_data as i32;

                let fast_shape = &world_ref.shapes[fast_shape_id as usize];
                b3_assert!(fast_shape.sensor_index == NULL_INDEX);

                // Skip same shape
                if hit_shape_id == fast_shape.id {
                    return true;
                }

                let shape = &world_ref.shapes[hit_shape_id as usize];

                // Skip same body
                if shape.body_id == fast_shape.body_id {
                    return true;
                }

                // Skip sensors unless the shapes want sensor events
                let is_sensor = shape.sensor_index != NULL_INDEX;
                if is_sensor && (!shape.enable_sensor_events || !fast_shape.enable_sensor_events) {
                    return true;
                }

                // Skip filtered shapes
                let can_collide = should_shapes_collide(fast_shape.filter, shape.filter);
                if !can_collide {
                    return true;
                }

                let body = &world_ref.bodies[shape.body_id as usize];

                let body_sim = if body.set_index == AWAKE_SET {
                    &sims[body.local_index as usize]
                } else {
                    &world_ref.solver_sets[body.set_index as usize].body_sims[body.local_index as usize]
                };
                b3_assert!(body.body_type == BodyType::Static || (fast_body_sim.flags & IS_BULLET != 0));

                // Skip bullets
                if body_sim.flags & IS_BULLET != 0 {
                    return true;
                }

                // Skip filtered bodies
                let can_collide = should_bodies_collide(world_ref, fast_body_id, shape.body_id);
                if !can_collide {
                    return true;
                }

                // Custom user filtering
                if shape.enable_custom_filtering || fast_shape.enable_custom_filtering {
                    if let Some(custom_filter) = custom_filter_fcn.as_mut() {
                        let id_a = ShapeId {
                            index1: shape.id + 1,
                            world0: world_ref.world_id,
                            generation: shape.generation,
                        };
                        let id_b = ShapeId {
                            index1: fast_shape.id + 1,
                            world0: world_ref.world_id,
                            generation: fast_shape.generation,
                        };
                        let can_collide = custom_filter(id_a, id_b);
                        if !can_collide {
                            return true;
                        }
                    }
                }

                // todo does having a sweep on shapeA help with bullets?
                let sweep_a = make_relative_sweep(body_sim, base);

                // Time of impact versus shape. Supports all shape types
                let output = shape_time_of_impact(shape, fast_shape, &sweep_a, &sweep, ctx_fraction);
                if is_sensor {
                    // Only accept a sensor hit that is sooner than the current solid hit.
                    if output.fraction <= ctx_fraction && sensor_count < MAX_CONTINUOUS_SENSOR_HITS {
                        // The hit shape is a sensor
                        sensor_hits[sensor_count] = SensorHit {
                            sensor_id: shape.id,
                            visitor_id: fast_shape.id,
                        };
                        sensor_fractions[sensor_count] = output.fraction;
                        sensor_count += 1;
                    }
                } else if 0.0 < output.fraction && output.fraction < ctx_fraction {
                    let mut did_hit = true;

                    if did_hit && (shape.enable_pre_solve_events || fast_shape.enable_pre_solve_events) {
                        if let Some(pre_solve) = pre_solve_fcn.as_mut() {
                            let shape_id_a = ShapeId {
                                index1: shape.id + 1,
                                world0: world_ref.world_id,
                                generation: shape.generation,
                            };
                            let shape_id_b = ShapeId {
                                index1: fast_shape.id + 1,
                                world0: world_ref.world_id,
                                generation: fast_shape.generation,
                            };
                            let point = offset_pos(base, output.point);
                            did_hit = pre_solve(shape_id_a, shape_id_b, point, output.normal);
                        }
                    }

                    if did_hit {
                        fast_body_sim.flags |= HAD_TIME_OF_IMPACT;
                        ctx_fraction = output.fraction;
                        distance_iterations = crate::math_functions::max_int(distance_iterations, output.distance_iterations);
                        push_back_iterations = crate::math_functions::max_int(push_back_iterations, output.push_back_iterations);
                        root_iterations = crate::math_functions::max_int(root_iterations, output.root_iterations);
                    }
                }

                // Continue query
                true
            };

            crate::dynamic_tree::dynamic_tree_query(
                &world_ref.broad_phase.trees[BodyType::Static as usize],
                swept_box,
                DEFAULT_MASK_BITS,
                false,
                &mut callback,
            );

            if is_bullet {
                crate::dynamic_tree::dynamic_tree_query(
                    &world_ref.broad_phase.trees[BodyType::Kinematic as usize],
                    swept_box,
                    DEFAULT_MASK_BITS,
                    false,
                    &mut callback,
                );
                crate::dynamic_tree::dynamic_tree_query(
                    &world_ref.broad_phase.trees[BodyType::Dynamic as usize],
                    swept_box,
                    DEFAULT_MASK_BITS,
                    false,
                    &mut callback,
                );
            }
        }
    }

    world.pre_solve_fcn = pre_solve_fcn;
    world.custom_filter_fcn = custom_filter_fcn;

    let speculative_scalar = speculative_distance();

    if ctx_fraction < 1.0 {
        // Handle time of impact event. The sweep is relative to the base, so re-add the base
        // to return the advanced pose to world space.
        let q = nlerp(sweep.q1, sweep.q2, ctx_fraction);
        let c = lerp(sweep.c1, sweep.c2, ctx_fraction);
        let origin = sub(c, rotate_vector(q, sweep.local_center));

        // Advance body
        let transform = crate::math_functions::WorldTransform { p: offset_pos(base, origin), q };
        let center = offset_pos(base, c);
        fast_body_sim.transform = transform;
        fast_body_sim.center = center;
        fast_body_sim.rotation0 = q;
        fast_body_sim.center0 = center;

        // The move event was written before CCD, so correct it with the impact pose
        world.body_move_events[body_sim_index as usize].transform = fast_body_sim.transform;

        // Prepare AABBs for broad-phase.
        // Even though a body is fast, it may not move much. So the AABB may not need enlargement.

        let mut shape_id = world.bodies[fast_body_id as usize].head_shape_id;
        while shape_id != NULL_INDEX {
            // Must recompute aabb at the interpolated transform
            let aabb = compute_fat_shape_aabb(&world.shapes[shape_id as usize], transform, speculative_scalar);

            let shape = &mut world.shapes[shape_id as usize];
            shape.aabb = aabb;

            if !aabb_contains(shape.fat_aabb, aabb) {
                let margin_scalar = shape.aabb_margin;
                let aabb_margin = vec3(margin_scalar, margin_scalar, margin_scalar);
                shape.fat_aabb = AABB {
                    lower_bound: sub(aabb.lower_bound, aabb_margin),
                    upper_bound: add(aabb.upper_bound, aabb_margin),
                };

                shape.enlarged_aabb = true;
                fast_body_sim.flags |= ENLARGE_BOUNDS;
            }

            shape_id = shape.next_shape_id;
        }
    } else {
        // No time of impact event

        // Advance body
        fast_body_sim.rotation0 = fast_body_sim.transform.q;
        fast_body_sim.center0 = fast_body_sim.center;

        // Prepare AABBs for broad-phase
        let mut shape_id = world.bodies[fast_body_id as usize].head_shape_id;
        while shape_id != NULL_INDEX {
            let shape = &mut world.shapes[shape_id as usize];

            // shape.aabb is still valid from above

            if !aabb_contains(shape.fat_aabb, shape.aabb) {
                let margin_scalar = shape.aabb_margin;
                let aabb_margin = vec3(margin_scalar, margin_scalar, margin_scalar);
                shape.fat_aabb = AABB {
                    lower_bound: sub(shape.aabb.lower_bound, aabb_margin),
                    upper_bound: add(shape.aabb.upper_bound, aabb_margin),
                };

                shape.enlarged_aabb = true;
                fast_body_sim.flags |= ENLARGE_BOUNDS;
            }

            shape_id = shape.next_shape_id;
        }
    }

    // Push sensor hits on the task context for serial processing.
    for i in 0..sensor_count {
        // Skip any sensor hits that occurred after a solid hit
        if sensor_fractions[i] < ctx_fraction {
            world.task_contexts[worker_index as usize].sensor_hits.push(sensor_hits[i]);
        }
    }

    {
        let task_context = &mut world.task_contexts[worker_index as usize];
        task_context.distance_iterations = crate::math_functions::max_int(task_context.distance_iterations, distance_iterations);
        task_context.push_back_iterations = crate::math_functions::max_int(task_context.push_back_iterations, push_back_iterations);
        task_context.root_iterations = crate::math_functions::max_int(task_context.root_iterations, root_iterations);
    }

    context.sims[body_sim_index as usize] = fast_body_sim;
}

// Implements b3ParallelForCallback (serial: one call over the whole range).
fn finalize_bodies_task(start_index: i32, end_index: i32, worker_index: i32, world: &mut World, context: &mut StepContext) {
    b3_assert!(end_index as usize <= world.body_move_events.len());

    let enable_sleep = world.enable_sleep;
    let enable_continuous = world.enable_continuous;
    let time_step = context.dt;
    let inv_time_step = context.inv_dt;
    let world_id = world.world_id;

    let speculative_scalar = speculative_distance();

    for sim_index in start_index..end_index {
        // C uses pointers into the state/sim arrays; the port copies the PODs out
        // and writes them back (also around solve_continuous, which mutates the sim).
        let mut state = context.states[sim_index as usize];
        let mut sim = context.sims[sim_index as usize];

        let v = state.linear_velocity;
        let w = state.angular_velocity;
        let local_omega = inv_rotate_vector(sim.transform.q, w);
        let local_delta_rotation = inv_rotate_vector(sim.transform.q, state.delta_rotation.v);

        if !is_valid_vec3(v) || !is_valid_vec3(w) {
            crate::core::log(&format!("unstable: {}", world.bodies[sim.body_id as usize].name));
        }

        b3_assert!(is_valid_vec3(v));
        b3_assert!(is_valid_vec3(w));

        sim.center = offset_pos(sim.center, state.delta_position);
        sim.transform.q = normalize_quat(mul_quat(state.delta_rotation, sim.transform.q));

        // Use the velocity of the farthest point on the body to account for rotation.
        let velocity_arc = modified_cross(abs(local_omega), sim.max_extent);
        let max_velocity = length(v) + length(velocity_arc);

        // Sleep needs to observe position correction as well as true velocity.
        // q = [sin(theta/2) * v, cos(theta/2)]
        // for small angles abs(theta) ~= 2 * length(sin(theta/2) * v)
        let rotation_arc = modified_cross(abs(local_delta_rotation), sim.max_extent);
        let max_delta_position = length(state.delta_position) + 2.0 * length(rotation_arc);

        // Position correction is not as important for sleep as true velocity.
        let position_sleep_factor = 0.5;
        let sleep_velocity = max_float(max_velocity, position_sleep_factor * inv_time_step * max_delta_position);

        // reset state deltas
        state.delta_position = Vec3::ZERO;
        state.delta_rotation = Quat::IDENTITY;

        sim.transform.p = offset_pos(sim.center, neg(rotate_vector(sim.transform.q, sim.local_center)));

        // cache miss here, however I need the shape list below
        let body_id = sim.body_id;
        world.bodies[body_id as usize].body_move_index = sim_index;
        world.body_move_events[sim_index as usize] = BodyMoveEvent {
            user_data: world.bodies[body_id as usize].user_data,
            transform: sim.transform,
            body_id: BodyId {
                index1: body_id + 1,
                world0: world_id,
                generation: world.bodies[body_id as usize].generation,
            },
            fell_asleep: false,
        };

        // reset applied force and torque
        sim.force = Vec3::ZERO;
        sim.torque = Vec3::ZERO;

        {
            let body = &mut world.bodies[body_id as usize];
            body.flags &= !BODY_TRANSIENT_FLAGS;
            body.flags |= sim.flags & (IS_SPEED_CAPPED | HAD_TIME_OF_IMPACT);
            body.flags |= state.flags & (IS_SPEED_CAPPED | HAD_TIME_OF_IMPACT);
        }
        sim.flags &= !BODY_TRANSIENT_FLAGS;
        state.flags &= !BODY_TRANSIENT_FLAGS;

        let body_flags = world.bodies[body_id as usize].flags;
        let body_type = world.bodies[body_id as usize].body_type;
        let sleep_threshold = world.bodies[body_id as usize].sleep_threshold;

        if !enable_sleep || (body_flags & ENABLE_SLEEP) == 0 || sleep_velocity > sleep_threshold {
            // Body is not sleepy
            world.bodies[body_id as usize].sleep_time = 0.0;

            let safety_factor = 0.5;
            let max_motion = max_float(max_delta_position, max_velocity * time_step);
            if body_type == BodyType::Dynamic && enable_continuous && max_motion > safety_factor * sim.min_extent {
                // This flag is only retained for debug draw
                sim.flags |= IS_FAST;

                // Store in fast array for the continuous collision stage
                // This is deterministic because the order of TOI sweeps doesn't matter
                if sim.flags & IS_BULLET != 0 {
                    context.bullet_bodies.push(sim_index);
                } else {
                    // solve_continuous reads and mutates the sim through the context,
                    // so write the locals back first and re-read after.
                    context.sims[sim_index as usize] = sim;
                    context.states[sim_index as usize] = state;
                    solve_continuous(world, context, sim_index, worker_index);
                    sim = context.sims[sim_index as usize];
                    state = context.states[sim_index as usize];
                }
            } else {
                // Body is safe to advance
                sim.center0 = sim.center;
                sim.rotation0 = sim.transform.q;
            }
        } else {
            // Body is safe to advance and is falling asleep
            sim.center0 = sim.center;
            sim.rotation0 = sim.transform.q;
            world.bodies[body_id as usize].sleep_time += time_step;
        }

        // Update world space inverse inertia tensor.
        let rotation_matrix = make_matrix_from_quat(sim.transform.q);
        sim.inv_inertia_world = mul_mm(mul_mm(rotation_matrix, sim.inv_inertia_local), transpose(rotation_matrix));

        // Any single body in an island can keep it awake
        let island_id = world.bodies[body_id as usize].island_id;
        let sleep_time = world.bodies[body_id as usize].sleep_time;
        let island_local_index = world.islands[island_id as usize].local_index;
        let island_constraint_remove_count = world.islands[island_id as usize].constraint_remove_count;
        if sleep_time < TIME_TO_SLEEP {
            // keep island awake
            set_bit(
                &mut world.task_contexts[worker_index as usize].awake_island_bit_set,
                island_local_index as u32,
            );
        } else if island_constraint_remove_count > 0 {
            // Body wants to sleep but its island needs splitting first. Track the sleepiest candidate.
            // Break sleep time ties using the island id to ensure determinism. The cross worker reduction
            // breaks ties the same way.
            let task_context = &mut world.task_contexts[worker_index as usize];
            if sleep_time > task_context.split_sleep_time
                || (sleep_time == task_context.split_sleep_time && island_id > task_context.split_island_id)
            {
                // pick the sleepiest candidate
                task_context.split_island_id = island_id;
                task_context.split_sleep_time = sleep_time;
            }
        }

        // Update shapes AABBs
        let transform = sim.transform;
        let is_fast = (sim.flags & IS_FAST) != 0;
        let mut shape_id = world.bodies[body_id as usize].head_shape_id;
        while shape_id != NULL_INDEX {
            if is_fast {
                // For fast non-bullet bodies the AABB has already been updated in solve_continuous
                // For fast bullet bodies the AABB will be updated at a later stage

                // Add to enlarged shapes regardless of AABB changes.
                // Bit-set to keep the move array sorted
                set_bit(
                    &mut world.task_contexts[worker_index as usize].enlarged_sim_bit_set,
                    sim_index as u32,
                );

                shape_id = world.shapes[shape_id as usize].next_shape_id;
            } else {
                let aabb = compute_fat_shape_aabb(&world.shapes[shape_id as usize], transform, speculative_scalar);

                let shape = &mut world.shapes[shape_id as usize];
                shape.aabb = aabb;

                b3_assert!(!shape.enlarged_aabb);

                let next_shape_id = shape.next_shape_id;
                if !aabb_contains(shape.fat_aabb, aabb) {
                    let margin_scalar = shape.aabb_margin;
                    let aabb_margin = vec3(margin_scalar, margin_scalar, margin_scalar);
                    shape.fat_aabb = AABB {
                        lower_bound: sub(aabb.lower_bound, aabb_margin),
                        upper_bound: add(aabb.upper_bound, aabb_margin),
                    };
                    shape.enlarged_aabb = true;

                    // Bit-set to keep the move array sorted
                    set_bit(
                        &mut world.task_contexts[worker_index as usize].enlarged_sim_bit_set,
                        sim_index as u32,
                    );
                }

                shape_id = next_shape_id;
            }
        }

        context.sims[sim_index as usize] = sim;
        context.states[sim_index as usize] = state;
    }
}

#[derive(Clone, Copy, Default)]
struct BlockDim {
    // number of items per block (except last block)
    size: i32,

    // total number of blocks
    count: i32,
}

// A block is a range of tasks, a start index and count as a sub-array. Each worker receives at
// most M blocks of work. The block size is computed from the same parameters as C so the
// partitioning (and therefore the execution grouping) matches exactly.
#[inline]
fn compute_block_count(item_count: i32, min_size: i32, max_block_count: i32) -> BlockDim {
    let mut dim = BlockDim::default();
    if item_count == 0 {
        return dim;
    }

    if item_count <= min_size * max_block_count {
        dim.size = min_size;
    } else {
        dim.size = (item_count + max_block_count - 1) / max_block_count;
    }

    dim.count = (item_count + dim.size - 1) / dim.size;

    b3_assert!(dim.count >= 1);
    b3_assert!(dim.size * dim.count >= item_count);

    dim
}

// Initialize solver blocks for a contiguous range of items, appended to the
// flat block array. Returns the (start, count) range. (C: b3InitBlocks into an
// arena array; the atomic claim counters are dropped.)
fn init_blocks(
    blocks: &mut Vec<SolverBlock>,
    dim: BlockDim,
    item_count: i32,
    block_type: SolverBlockType,
    color_index: u8,
) -> (i32, i32) {
    let start = blocks.len() as i32;

    if dim.count == 0 {
        return (start, 0);
    }

    b3_assert!(item_count >= dim.count);

    // Compute the number of elements per block
    let block_size = dim.size;

    // Simulation too big
    b3_assert!(block_size <= u16::MAX as i32);

    for i in 0..dim.count {
        blocks.push(SolverBlock {
            start_index: i * block_size,
            count: block_size as u16,
            block_type,
            color_index,
        });
    }

    // The last block may not be full
    let last = (start + dim.count - 1) as usize;
    blocks[last].count = (item_count - (dim.count - 1) * block_size) as u16;

    b3_validate!(blocks[last].count as i32 <= block_size);
    b3_validate!((dim.count - 1) * dim.size + blocks[last].count as i32 == item_count);

    (start, dim.count)
}

// C: b3ExecuteBlock. Dispatch of one block; runs on any worker.
fn execute_block(stage_type: SolverStageType, block: SolverBlock, shared: &SolverShared, worker_index: i32) {
    let block_type = block.block_type;

    match stage_type {
        SolverStageType::PrepareJoints => prepare_joints_task(block, shared),

        SolverStageType::PrepareWideContacts => crate::contact_solver::prepare_contacts_convex(block, shared),

        SolverStageType::PrepareContacts => crate::contact_solver::prepare_contacts_mesh(block, shared),

        SolverStageType::IntegrateVelocities => integrate_velocities_task(block, shared),

        SolverStageType::WarmStart => {
            if block_type == SolverBlockType::GraphJoint {
                warm_start_joints_task(block, shared);
            } else if block_type == SolverBlockType::GraphWideContact {
                crate::contact_solver::warm_start_contacts_convex(block, shared);
            } else {
                crate::contact_solver::warm_start_contacts_mesh(block, shared);
            }
        }

        SolverStageType::Solve => {
            if block_type == SolverBlockType::GraphJoint {
                let use_bias = true;
                solve_joints_task(block, shared, use_bias, worker_index);
            } else if block_type == SolverBlockType::GraphWideContact {
                let use_bias = true;
                crate::contact_solver::solve_contacts_convex(block, shared, use_bias);
            } else {
                let use_bias = true;
                crate::contact_solver::solve_contacts_mesh(block, shared, use_bias);
            }
        }

        SolverStageType::IntegratePositions => integrate_positions_task(block, shared),

        SolverStageType::Relax => {
            if block_type == SolverBlockType::GraphJoint {
                let use_bias = false;
                solve_joints_task(block, shared, use_bias, worker_index);
            } else if block_type == SolverBlockType::GraphWideContact {
                let use_bias = false;
                crate::contact_solver::solve_contacts_convex(block, shared, use_bias);
            } else {
                let use_bias = false;
                crate::contact_solver::solve_contacts_mesh(block, shared, use_bias);
            }
        }

        SolverStageType::Restitution => {
            if block_type == SolverBlockType::GraphWideContact {
                crate::contact_solver::apply_restitution_convex(block, shared);
            } else if block_type == SolverBlockType::GraphContact {
                crate::contact_solver::apply_restitution_mesh(block, shared);
            }
            // Joint blocks are mixed into the color stages but have no restitution.
        }

        SolverStageType::StoreWideImpulses => crate::contact_solver::store_impulses_convex(block, shared, worker_index),

        SolverStageType::StoreImpulses => crate::contact_solver::store_impulses_mesh(block, shared, worker_index),
    }
}

// This staggers the worker start indices so they avoid touching the same solver blocks.
// C: GetWorkerStartIndex.
#[inline]
fn get_worker_start_index(worker_index: i32, block_count: i32, worker_count: i32) -> i32 {
    if block_count <= worker_count {
        return if worker_index < block_count { worker_index } else { NULL_INDEX };
    }

    let blocks_per_worker = block_count / worker_count;
    let remainder = block_count - blocks_per_worker * worker_count;
    blocks_per_worker * worker_index + min_int(remainder, worker_index)
}

// Execute a stage, which is an array of solver blocks, each controlled with an
// atomic sync index. Each worker starts at its home index and sweeps the ring,
// CAS-claiming any unclaimed blocks. C: b3ExecuteStage.
fn execute_stage(shared: &SolverShared, stage_index: usize, previous_sync_index: i32, sync_index: i32, worker_index: i32) {
    let context = shared.context;
    let stage = context.stages[stage_index];
    let block_count = stage.blocks_count;

    let start_index = get_worker_start_index(worker_index, block_count, context.worker_count);
    if start_index == NULL_INDEX {
        return;
    }

    b3_assert!(0 <= start_index && start_index < block_count);

    let mut completed_count = 0;
    let mut block_index = start_index;
    for _ in 0..block_count {
        let flat_index = (stage.blocks_start + block_index) as usize;
        if context.block_sync[flat_index].compare_exchange(previous_sync_index, sync_index) {
            b3_assert!(completed_count < block_count);

            // Pass the descriptor by value — the atomic sync index lives in the
            // parallel block_sync array, so the copy never aliases the CAS target.
            execute_block(stage.stage_type, context.blocks[flat_index], shared, worker_index);
            completed_count += 1;
        }

        block_index += 1;
        if block_index >= block_count {
            block_index = 0;
        }
    }

    context.stage_completion[stage_index].fetch_add(completed_count);
}

// Execute a stage on worker 0 (the orchestrator). C: b3ExecuteMainStage.
fn execute_main_stage(shared: &SolverShared, stage_index: usize, sync_bits: i32) {
    let context = shared.context;
    let stage = context.stages[stage_index];
    let block_count = stage.blocks_count;
    if block_count == 0 {
        return;
    }

    let worker_index = 0;

    if block_count == 1 {
        execute_block(stage.stage_type, context.blocks[stage.blocks_start as usize], shared, worker_index);
    } else {
        context.atomic_sync_bits.store(sync_bits);

        let sync_index = (sync_bits >> 16) & 0xFFFF;
        b3_assert!(sync_index > 0);
        let previous_sync_index = sync_index - 1;

        execute_stage(shared, stage_index, previous_sync_index, sync_index, worker_index);

        // Spin waiting for thieves to finish
        while context.stage_completion[stage_index].load() != block_count {
            std::hint::spin_loop();
        }

        context.stage_completion[stage_index].store(0);
    }
}

// Parallel solver task. C: b3SolverTask — worker 0 races for the orchestrator
// slot; other workers spin on the sync bits and steal stage blocks.
fn solver_task(shared: &SolverShared, worker_index: i32) {
    let context = shared.context;
    let active_color_count = context.active_color_count as usize;

    if worker_index == 0 {
        // The orchestrator slot is a race. The calling thread of world_step also
        // enters here as worker 0, so progress is guaranteed even if the queued
        // worker-0 task runs late (or first). Whoever wins the CAS becomes the
        // orchestrator; the loser returns.
        if !context.main_claimed.compare_exchange(0, 1) {
            return;
        }

        // Main thread synchronizes the workers and does work itself.
        //
        // Stages are re-used by loops so that more stages aren't needed for large
        // substep counts. The sync indices grow monotonically for the
        // body/graph/constraint groupings because they share solver blocks.
        // SAFETY (profile): only the orchestrator (unique CAS winner) writes it.
        let profile = unsafe { shared.profile.get() };

        let mut ticks = get_ticks();

        let mut body_sync_index = 1i32;
        let mut stage_index = 0usize;

        // Prepare joint constraints
        let mut joint_sync_index = 1i32;
        let mut sync_bits = (joint_sync_index << 16) | stage_index as i32;
        b3_assert!(context.stages[stage_index].stage_type == SolverStageType::PrepareJoints);
        execute_main_stage(shared, stage_index, sync_bits);
        stage_index += 1;
        joint_sync_index += 1;
        let _ = joint_sync_index; // C keeps the symmetric increment; only convex/mesh indices are reused

        // Prepare convex contact constraints
        let mut convex_sync_index = 1i32;
        sync_bits = (convex_sync_index << 16) | stage_index as i32;
        b3_assert!(context.stages[stage_index].stage_type == SolverStageType::PrepareWideContacts);
        execute_main_stage(shared, stage_index, sync_bits);
        stage_index += 1;
        convex_sync_index += 1;

        // Prepare mesh contact constraints
        let mut mesh_sync_index = 1i32;
        sync_bits = (mesh_sync_index << 16) | stage_index as i32;
        b3_assert!(context.stages[stage_index].stage_type == SolverStageType::PrepareContacts);
        execute_main_stage(shared, stage_index, sync_bits);
        stage_index += 1;
        mesh_sync_index += 1;

        // Single-threaded overflow work. These constraints don't fit in the graph coloring.
        crate::joint::prepare_joints_overflow(shared);
        crate::contact_solver::prepare_contacts_overflow(shared);

        profile.prepare_constraints += get_milliseconds_and_reset(&mut ticks);

        let mut graph_sync_index = 1i32;
        let sub_step_count = context.sub_step_count;
        for _sub_step_index in 0..sub_step_count {
            // stage_index restarted each iteration
            // sync bits still increase monotonically because the upper bits increase each iteration
            let mut iteration_stage_index = stage_index;

            // Integrate velocities
            sync_bits = (body_sync_index << 16) | iteration_stage_index as i32;
            b3_assert!(context.stages[iteration_stage_index].stage_type == SolverStageType::IntegrateVelocities);
            execute_main_stage(shared, iteration_stage_index, sync_bits);
            iteration_stage_index += 1;
            body_sync_index += 1;

            profile.integrate_velocities += get_milliseconds_and_reset(&mut ticks);

            // Warm start constraints
            crate::joint::warm_start_joints_overflow(shared);
            crate::contact_solver::warm_start_contacts_overflow(shared);

            for _color_index in 0..active_color_count {
                sync_bits = (graph_sync_index << 16) | iteration_stage_index as i32;
                b3_assert!(context.stages[iteration_stage_index].stage_type == SolverStageType::WarmStart);
                execute_main_stage(shared, iteration_stage_index, sync_bits);
                iteration_stage_index += 1;
            }
            graph_sync_index += 1;

            profile.warm_start += get_milliseconds_and_reset(&mut ticks);

            // Solve constraints
            let use_bias = true;
            for _j in 0..ITERATIONS {
                // Overflow constraints have lower priority. Typically these are dynamic-vs-dynamic.
                crate::joint::solve_joints_overflow(shared, use_bias);
                crate::contact_solver::solve_contacts_overflow(shared, use_bias);

                for _color_index in 0..active_color_count {
                    sync_bits = (graph_sync_index << 16) | iteration_stage_index as i32;
                    b3_assert!(context.stages[iteration_stage_index].stage_type == SolverStageType::Solve);
                    execute_main_stage(shared, iteration_stage_index, sync_bits);
                    iteration_stage_index += 1;
                }
                graph_sync_index += 1;
            }

            profile.solve_impulses += get_milliseconds_and_reset(&mut ticks);

            // Integrate positions
            b3_assert!(context.stages[iteration_stage_index].stage_type == SolverStageType::IntegratePositions);
            sync_bits = (body_sync_index << 16) | iteration_stage_index as i32;
            execute_main_stage(shared, iteration_stage_index, sync_bits);
            iteration_stage_index += 1;
            body_sync_index += 1;

            profile.integrate_positions += get_milliseconds_and_reset(&mut ticks);

            // Relax constraints
            let use_bias = false;
            for _j in 0..RELAX_ITERATIONS {
                crate::joint::solve_joints_overflow(shared, use_bias);
                crate::contact_solver::solve_contacts_overflow(shared, use_bias);

                for _color_index in 0..active_color_count {
                    sync_bits = (graph_sync_index << 16) | iteration_stage_index as i32;
                    b3_assert!(context.stages[iteration_stage_index].stage_type == SolverStageType::Relax);
                    execute_main_stage(shared, iteration_stage_index, sync_bits);
                    iteration_stage_index += 1;
                }
                graph_sync_index += 1;
            }

            profile.relax_impulses += get_milliseconds_and_reset(&mut ticks);
        }

        // Advance the stage according to the sub-stepping tasks just completed
        // integrate velocities / warm start / solve / integrate positions / relax
        stage_index += 1
            + active_color_count
            + ITERATIONS as usize * active_color_count
            + 1
            + RELAX_ITERATIONS as usize * active_color_count;

        // Restitution
        {
            crate::contact_solver::apply_restitution_overflow(shared);

            let mut iter_stage_index = stage_index;
            for _color_index in 0..active_color_count {
                sync_bits = (graph_sync_index << 16) | iter_stage_index as i32;
                b3_assert!(context.stages[iter_stage_index].stage_type == SolverStageType::Restitution);
                execute_main_stage(shared, iter_stage_index, sync_bits);
                iter_stage_index += 1;
            }
            // graph_sync_index += 1;
            stage_index += active_color_count;
        }

        profile.apply_restitution += get_milliseconds_and_reset(&mut ticks);

        // Store impulses
        crate::contact_solver::store_impulses_overflow(shared);

        sync_bits = (convex_sync_index << 16) | stage_index as i32;
        b3_assert!(context.stages[stage_index].stage_type == SolverStageType::StoreWideImpulses);
        execute_main_stage(shared, stage_index, sync_bits);
        stage_index += 1;

        sync_bits = (mesh_sync_index << 16) | stage_index as i32;
        b3_assert!(context.stages[stage_index].stage_type == SolverStageType::StoreImpulses);
        execute_main_stage(shared, stage_index, sync_bits);
        stage_index += 1;

        profile.store_impulses += get_milliseconds_and_reset(&mut ticks);

        // Signal workers to finish
        context.atomic_sync_bits.store(SYNC_SENTINEL);

        b3_assert!(stage_index == context.stages.len());
        return;
    }

    // Worker spins and waits for work
    let mut last_sync_bits = 0i32;
    loop {
        // Spin until the orchestrator bumps the sync bits. This can waste
        // significant time overall, but it is necessary for parallel simulation
        // with graph coloring.
        let mut sync_bits;
        let mut spin_count = 0;
        loop {
            sync_bits = context.atomic_sync_bits.load();
            if sync_bits != last_sync_bits {
                break;
            }
            if spin_count > 5 {
                std::thread::yield_now();
                spin_count = 0;
            } else {
                std::hint::spin_loop();
                std::hint::spin_loop();
                spin_count += 1;
            }
        }

        if sync_bits == SYNC_SENTINEL {
            // sentinel hit
            break;
        }

        let stage_index = (sync_bits & 0xFFFF) as usize;
        b3_assert!(stage_index < context.stages.len());

        let sync_index = (sync_bits >> 16) & 0xFFFF;
        b3_assert!(sync_index > 0);

        let previous_sync_index = sync_index - 1;

        execute_stage(shared, stage_index, previous_sync_index, sync_index, worker_index);

        last_sync_bits = sync_bits;
    }
}

// Scheduler task wrapper. The context/shared structs live on solve()'s stack,
// which blocks in scheduler_finish_task until every solver task completes.
struct SolverWorkerContext<'a, 'b> {
    shared: &'b SolverShared<'a>,
    worker_index: i32,
}

unsafe fn solver_task_trampoline(task_context: *mut ()) {
    // SAFETY: enqueued by run_solver_tasks below; the pointee outlives the
    // finish loop there.
    let worker = unsafe { &*(task_context as *const SolverWorkerContext) };
    solver_task(worker.shared, worker.worker_index);
}

// Enqueue workerCount solver tasks, participate as worker 0, and wait for all
// of them. C: the enqueue/caller-race/finish block of b3Solve.
fn run_solver_tasks(shared: &SolverShared, worker_count: i32) {
    let task_system = &shared.world.task_system;

    if worker_count <= 1 || !task_system.is_parallel() {
        // Serial fast path: the caller is the orchestrator (wins the race
        // unopposed) and executes every block in array order, bit-identically
        // to the pre-threading port.
        solver_task(shared, 0);
        return;
    }

    let worker_contexts: Vec<SolverWorkerContext> = (0..worker_count)
        .map(|worker_index| SolverWorkerContext { shared, worker_index })
        .collect();

    // An external system may run tasks synchronously inside enqueue (or the
    // ring budget may force inline execution) — either way the mainClaimed
    // race below keeps the orchestration correct (C solver.c comment).
    let mut handles: Vec<crate::scheduler::TaskHandle> =
        vec![crate::scheduler::TaskHandle::Inline; worker_count as usize];
    for i in 0..worker_count as usize {
        // SAFETY: worker_contexts (and everything shared references) stay
        // alive until the finish loop below returns.
        handles[i] = unsafe {
            task_system.enqueue(
                solver_task_trampoline,
                &worker_contexts[i] as *const SolverWorkerContext as *mut (),
                "solve",
            )
        };
    }

    // The calling thread also enters as worker 0 and races for the orchestrator
    // slot via the CAS inside; this guarantees progress even if the queued
    // worker-0 task is delayed. The loser of the race no-ops.
    solver_task(shared, 0);

    // Finish constraint solve
    for handle in handles.iter().take(worker_count as usize) {
        task_system.finish(*handle);
    }
}

// Solve with graph coloring
pub fn solve(world: &mut World, step_context: &mut StepContext) {
    // Only count steps that advance the simulation
    world.step_index += 1;

    let awake_body_count = world.solver_sets[AWAKE_SET as usize].body_sims.len() as i32;
    if awake_body_count == 0 {
        validate_no_enlarged(&world.broad_phase);
        return;
    }

    // Solve constraints using graph coloring
    {
        let mut setup_ticks = get_ticks();

        // Prepare buffers for continuous collision (fast bodies).
        // (C: atomic count + stack array; the port reuses the context Vec.)
        step_context.bullet_bodies.clear();

        // C: stepContext->sims/states point at the awake set arrays. The port moves
        // the arrays into the context; they move back after the bullet stage.
        step_context.sims = std::mem::take(&mut world.solver_sets[AWAKE_SET as usize].body_sims);
        step_context.states = std::mem::take(&mut world.solver_sets[AWAKE_SET as usize].body_states);

        // count contacts, joints, and colors
        // (C computes this preliminary count and then overwrites it below; kept for fidelity.)
        let mut active_color_count = 0;
        for i in 0..GRAPH_COLOR_COUNT - 1 {
            let color = &world.constraint_graph.colors[i];
            let per_color_contact_count = color.convex_contacts.len() + color.contacts.len();
            let per_color_joint_count = color.joint_sims.len();
            let occupancy_count = per_color_contact_count + per_color_joint_count;
            active_color_count += if occupancy_count > 0 { 1 } else { 0 };
        }
        let _ = active_color_count;

        // prepare for move events
        world.body_move_events.resize(awake_body_count as usize, zero_body_move_event());

        let worker_count = world.worker_count;

        // Target 4 blocks per worker to allow work stealing
        let max_block_count = 4 * worker_count;

        // Body blocks are for parallel iteration over bodies directly (integration, update transforms)
        let min_bodies_per_block = 32;
        let body_dim = compute_block_count(awake_body_count, min_bodies_per_block, max_block_count);

        const MIN_CONTACTS_PER_BLOCK: i32 = 4;
        const MIN_JOINTS_PER_BLOCK: i32 = 4;

        // Configure blocks for tasks parallel-for each active graph color
        // The blocks are a mix of convex contact, mesh contact, and joint blocks
        let mut active_color_indices = [0i32; GRAPH_COLOR_COUNT];
        let mut color_wide_contact_counts = [0i32; GRAPH_COLOR_COUNT];
        let mut color_contact_counts = [0i32; GRAPH_COLOR_COUNT];
        let mut color_joint_counts = [0i32; GRAPH_COLOR_COUNT];
        let mut graph_wide_contact_dims = [BlockDim::default(); GRAPH_COLOR_COUNT];
        let mut graph_contact_dims = [BlockDim::default(); GRAPH_COLOR_COUNT];
        let mut graph_joint_dims = [BlockDim::default(); GRAPH_COLOR_COUNT];
        let mut graph_block_count = 0i32;

        // c is the active color index
        let mut wide_contact_count = 0i32;
        let mut contact_count = 0i32;
        let mut manifold_count = 0i32;
        let mut joint_count = 0i32;
        let mut c = 0usize;
        for i in 0..GRAPH_COLOR_COUNT - 1 {
            let color = &mut world.constraint_graph.colors[i];
            let color_convex_contact_count = color.convex_contacts.len() as i32;
            let color_contact_count = color.contacts.len() as i32;
            let color_joint_count = color.joint_sims.len() as i32;

            if color_convex_contact_count + color_contact_count + color_joint_count == 0 {
                continue;
            }

            active_color_indices[c] = i as i32;

            // Ceiling for wide constraint count
            let color_wide_constraint_count = if color_convex_contact_count > 0 {
                ((color_convex_contact_count - 1) >> SIMD_SHIFT) + 1
            } else {
                0
            };
            wide_contact_count += color_wide_constraint_count;
            color_wide_contact_counts[c] = color_wide_constraint_count;

            color_contact_counts[c] = color_contact_count;
            contact_count += color_contact_count;

            // Compute manifold starts and accumulate manifold count.
            // Layout contract: manifold_start is FLAT into context.manifold_constraints.
            // Track this color's flat manifold base for GraphColor bookkeeping.
            let color_manifold_start = manifold_count;
            for j in 0..color_contact_count {
                color.contacts[j as usize].manifold_start = manifold_count;
                manifold_count += color.contacts[j as usize].manifold_count as i32;
            }
            color.manifold_constraint_start = color_manifold_start;
            color.manifold_constraint_count = manifold_count - color_manifold_start;

            color_joint_counts[c] = color_joint_count;
            joint_count += color_joint_count;

            // Solver block dimensions
            graph_wide_contact_dims[c] = compute_block_count(color_wide_constraint_count, MIN_CONTACTS_PER_BLOCK, max_block_count);
            graph_contact_dims[c] = compute_block_count(color_contact_count, MIN_CONTACTS_PER_BLOCK, max_block_count);
            graph_joint_dims[c] = compute_block_count(color_joint_count, MIN_JOINTS_PER_BLOCK, max_block_count);
            graph_block_count += graph_wide_contact_dims[c].count + graph_contact_dims[c].count + graph_joint_dims[c].count;

            c += 1;
        }
        let active_color_count = c;

        // Prepare and store run as one flat parallel-for over the entire constraint range.
        let convex_prepare_dim = compute_block_count(wide_contact_count, MIN_CONTACTS_PER_BLOCK, max_block_count);
        let mesh_prepare_dim = compute_block_count(contact_count, MIN_CONTACTS_PER_BLOCK, max_block_count);
        let joint_prepare_dim = compute_block_count(joint_count, MIN_JOINTS_PER_BLOCK, max_block_count);

        // Overflow constraints follow the color constraints in the flat arrays
        // (layout contract with contact_solver.rs). ContactSpec.manifold_start is
        // flat for overflow specs too (C uses a separate 0-based array).
        let overflow_count;
        let mut overflow_manifold_count = 0i32;
        {
            let overflow = &mut world.constraint_graph.colors[OVERFLOW_INDEX as usize];
            overflow_count = overflow.contacts.len() as i32;
            for i in 0..overflow_count {
                overflow.contacts[i as usize].manifold_start = manifold_count + overflow_manifold_count;
                overflow_manifold_count += overflow.contacts[i as usize].manifold_count as i32;
            }

            overflow.contact_constraint_start = contact_count;
            overflow.contact_constraint_count = overflow_count;
            overflow.manifold_constraint_start = manifold_count;
            overflow.manifold_constraint_count = overflow_manifold_count;
        }

        // Allocate the flat constraint arrays. Default-initialized wide constraints
        // reproduce the C memset of remainder lanes. clear + resize keeps the
        // scratch capacity from previous steps while producing the same
        // all-default contents as a fresh vec![default; n].
        step_context.wide_constraints.clear();
        step_context
            .wide_constraints
            .resize(wide_contact_count as usize, crate::contact_solver::ContactConstraintWide::default());
        step_context.contact_constraints.clear();
        step_context
            .contact_constraints
            .resize((contact_count + overflow_count) as usize, crate::contact_solver::ContactConstraint::default());
        step_context.manifold_constraints.clear();
        step_context.manifold_constraints.resize(
            (manifold_count + overflow_manifold_count) as usize,
            crate::contact_solver::ManifoldConstraint::default(),
        );
        step_context.wide_contact_count = wide_contact_count;

        // Build the span table for the flat prepare/store parallel-for while slicing the
        // constraint ranges across colors. One entry per active color plus a sentinel.
        step_context.wide_prepare_spans.clear();
        step_context.contact_prepare_spans.clear();
        step_context.joint_prepare_spans.clear();

        {
            let mut wide_base = 0i32;
            let mut contact_base = 0i32;
            let mut joint_base = 0i32;
            for i in 0..active_color_count {
                let j = active_color_indices[i];
                let color = &mut world.constraint_graph.colors[j as usize];

                let color_convex_contact_count = color.convex_contacts.len() as i32;
                step_context.wide_prepare_spans.push(WidePrepareSpan {
                    start: wide_base,
                    count: color_convex_contact_count,
                    color_index: j,
                });

                if color_convex_contact_count == 0 {
                    color.wide_constraint_start = NULL_INDEX;
                    color.wide_constraint_count = 0;
                } else {
                    color.wide_constraint_start = wide_base;

                    let color_contact_count_w = ((color_convex_contact_count - 1) >> SIMD_SHIFT) + 1;
                    color.wide_constraint_count = color_contact_count_w;

                    // C zeroes the remainder lanes of the tail wide slot here; the flat
                    // array was default-initialized above, which covers it.

                    wide_base += color_contact_count_w;
                }

                let color_contact_count = color.contacts.len() as i32;
                step_context.contact_prepare_spans.push(ContactPrepareSpan {
                    start: contact_base,
                    count: color_contact_count,
                    color_index: j,
                });

                if color_contact_count == 0 {
                    color.contact_constraint_start = NULL_INDEX;
                    color.contact_constraint_count = 0;
                } else {
                    color.contact_constraint_start = contact_base;
                    color.contact_constraint_count = color_contact_count;
                    contact_base += color_contact_count;
                }

                step_context.joint_prepare_spans.push(JointPrepareSpan {
                    start: joint_base,
                    count: color.joint_sims.len() as i32,
                    color_index: j,
                });
                joint_base += color.joint_sims.len() as i32;
            }

            // Sentinels
            step_context.wide_prepare_spans.push(WidePrepareSpan {
                start: wide_contact_count,
                count: 0,
                color_index: NULL_INDEX,
            });
            b3_assert!(wide_base == wide_contact_count);

            step_context.contact_prepare_spans.push(ContactPrepareSpan {
                start: contact_count,
                count: 0,
                color_index: NULL_INDEX,
            });
            b3_assert!(contact_base == contact_count);

            step_context.joint_prepare_spans.push(JointPrepareSpan {
                start: joint_count,
                count: 0,
                color_index: NULL_INDEX,
            });
            b3_assert!(joint_base == joint_count);
        }

        // Special span for overflow to allow for function re-use (starts are relative
        // to the overflow range per the layout contract).
        step_context.overflow_spans.clear();
        step_context.overflow_spans.push(ContactPrepareSpan {
            start: 0,
            count: overflow_count,
            color_index: OVERFLOW_INDEX,
        });
        step_context.overflow_spans.push(ContactPrepareSpan {
            start: overflow_count,
            count: 0,
            color_index: NULL_INDEX,
        });

        let mut stage_count = 0usize;

        // PrepareJoints
        stage_count += 1;
        // PrepareWideContacts
        stage_count += 1;
        // PrepareContacts
        stage_count += 1;
        // IntegrateVelocities
        stage_count += 1;
        // WarmStart
        stage_count += active_color_count;
        // Solve
        stage_count += ITERATIONS as usize * active_color_count;
        // IntegratePositions
        stage_count += 1;
        // Relax
        stage_count += RELAX_ITERATIONS as usize * active_color_count;
        // Restitution
        stage_count += active_color_count;
        // StoreWideImpulses
        stage_count += 1;
        // StoreImpulses
        stage_count += 1;

        // Block arrays (C allocates b3SyncBlock arrays; the sync counters are
        // dropped). All blocks live in one flat reused array; stages reference
        // (start, count) ranges, so the C pattern of several stages sharing one
        // block array becomes shared ranges.
        step_context.blocks.clear();
        let body_blocks = init_blocks(&mut step_context.blocks, body_dim, awake_body_count, SolverBlockType::Body, u8::MAX);
        let convex_blocks =
            init_blocks(&mut step_context.blocks, convex_prepare_dim, wide_contact_count, SolverBlockType::WideContact, u8::MAX);
        let mesh_blocks = init_blocks(&mut step_context.blocks, mesh_prepare_dim, contact_count, SolverBlockType::Contact, u8::MAX);
        let joint_blocks = init_blocks(&mut step_context.blocks, joint_prepare_dim, joint_count, SolverBlockType::Joint, u8::MAX);

        // Split an awake island. This modifies:
        // - world island array and solver set
        // - island indices on bodies, contacts, and joints
        // C enqueues this as a task that may run concurrent with the constraint solve
        // (but never with FinalizeBodies); the serial port runs it inline here, which
        // matches the C fallback path when the task queue is full.
        if world.split_island_id != NULL_INDEX {
            crate::island::split_island_task(world);
        }

        // Prepare graph work blocks. Each color gets joint blocks followed by
        // wide contact blocks followed by contact blocks, appended contiguously
        // to the flat block array so one (start, count) range covers the color.
        let mut graph_color_ranges = [(0i32, 0i32); GRAPH_COLOR_COUNT];
        {
            let mut total = 0i32;
            for i in 0..active_color_count {
                let color_index = active_color_indices[i] as u8;

                let start = step_context.blocks.len() as i32;
                init_blocks(&mut step_context.blocks, graph_joint_dims[i], color_joint_counts[i], SolverBlockType::GraphJoint, color_index);
                init_blocks(
                    &mut step_context.blocks,
                    graph_wide_contact_dims[i],
                    color_wide_contact_counts[i],
                    SolverBlockType::GraphWideContact,
                    color_index,
                );
                init_blocks(
                    &mut step_context.blocks,
                    graph_contact_dims[i],
                    color_contact_counts[i],
                    SolverBlockType::GraphContact,
                    color_index,
                );
                let count = step_context.blocks.len() as i32 - start;

                total += count;
                graph_color_ranges[i] = (start, count);
            }
            b3_assert!(total == graph_block_count);
        }

        // Build the stage array in the C order. The Vec is reused across steps.
        step_context.stages.clear();
        let stages = &mut step_context.stages;
        stages.push(SolverStage {
            blocks_start: joint_blocks.0,
            blocks_count: joint_blocks.1,
            stage_type: SolverStageType::PrepareJoints,
            color_index: u8::MAX,
        });
        stages.push(SolverStage {
            blocks_start: convex_blocks.0,
            blocks_count: convex_blocks.1,
            stage_type: SolverStageType::PrepareWideContacts,
            color_index: u8::MAX,
        });
        stages.push(SolverStage {
            blocks_start: mesh_blocks.0,
            blocks_count: mesh_blocks.1,
            stage_type: SolverStageType::PrepareContacts,
            color_index: u8::MAX,
        });
        stages.push(SolverStage {
            blocks_start: body_blocks.0,
            blocks_count: body_blocks.1,
            stage_type: SolverStageType::IntegrateVelocities,
            color_index: u8::MAX,
        });
        for i in 0..active_color_count {
            stages.push(SolverStage {
                blocks_start: graph_color_ranges[i].0,
                blocks_count: graph_color_ranges[i].1,
                stage_type: SolverStageType::WarmStart,
                color_index: active_color_indices[i] as u8,
            });
        }
        for _ in 0..ITERATIONS {
            for i in 0..active_color_count {
                stages.push(SolverStage {
                    blocks_start: graph_color_ranges[i].0,
                    blocks_count: graph_color_ranges[i].1,
                    stage_type: SolverStageType::Solve,
                    color_index: active_color_indices[i] as u8,
                });
            }
        }
        stages.push(SolverStage {
            blocks_start: body_blocks.0,
            blocks_count: body_blocks.1,
            stage_type: SolverStageType::IntegratePositions,
            color_index: u8::MAX,
        });
        for _ in 0..RELAX_ITERATIONS {
            for i in 0..active_color_count {
                stages.push(SolverStage {
                    blocks_start: graph_color_ranges[i].0,
                    blocks_count: graph_color_ranges[i].1,
                    stage_type: SolverStageType::Relax,
                    color_index: active_color_indices[i] as u8,
                });
            }
        }
        // Note: joint blocks mixed in, could have joint limit restitution
        for i in 0..active_color_count {
            stages.push(SolverStage {
                blocks_start: graph_color_ranges[i].0,
                blocks_count: graph_color_ranges[i].1,
                stage_type: SolverStageType::Restitution,
                color_index: active_color_indices[i] as u8,
            });
        }
        stages.push(SolverStage {
            blocks_start: convex_blocks.0,
            blocks_count: convex_blocks.1,
            stage_type: SolverStageType::StoreWideImpulses,
            color_index: u8::MAX,
        });
        stages.push(SolverStage {
            blocks_start: mesh_blocks.0,
            blocks_count: mesh_blocks.1,
            stage_type: SolverStageType::StoreImpulses,
            color_index: u8::MAX,
        });

        b3_assert!(stages.len() == stage_count);

        step_context.active_color_count = active_color_count as i32;
        step_context.worker_count = worker_count;

        // Reset the stage synchronization (C allocates fresh SyncBlocks and
        // stores the atomics in the setup). The atomic arrays are reused across
        // steps; every slot in range is reset to zero.
        {
            let block_count = step_context.blocks.len();
            if step_context.block_sync.len() < block_count {
                step_context
                    .block_sync
                    .resize_with(block_count, || crate::sync::AtomicIndex::new(0));
            }
            for sync in step_context.block_sync.iter().take(block_count) {
                sync.store(0);
            }

            let stage_count = step_context.stages.len();
            if step_context.stage_completion.len() < stage_count {
                step_context
                    .stage_completion
                    .resize_with(stage_count, || crate::sync::AtomicIndex::new(0));
            }
            for completion in step_context.stage_completion.iter().take(stage_count) {
                completion.store(0);
            }

            step_context.atomic_sync_bits.store(0);
            step_context.main_claimed.store(0);
        }

        world.profile.solver_setup = get_milliseconds_and_reset(&mut setup_ticks);

        // === Constraint solve (C: worker tasks + orchestrator race) ===
        let mut constraint_ticks = get_ticks();

        let joint_id_capacity = get_id_capacity(&world.joint_id_pool);
        let contact_id_capacity = get_id_capacity(&world.contact_id_pool);
        for i in 0..worker_count {
            let task_context = &mut world.task_contexts[i as usize];
            set_bit_count_and_clear(&mut task_context.joint_state_bit_set, joint_id_capacity as u32);
            set_bit_count_and_clear(&mut task_context.hit_event_bit_set, contact_id_capacity as u32);
            task_context.has_hit_events = false;
        }

        // C resets the task ring once at step start (physics_world.rs), not here.

        let mut solve_profile = SolveProfile::default();
        {
            // Take the shared-mutable arrays out of the world/context for the
            // duration of the constraint stages (see SolverShared docs). The
            // world then presents a read-only view to all workers; disjoint
            // mutation goes through the SyncSlice/StateAccess views.
            let mut states = std::mem::take(&mut step_context.states);
            let mut wide_constraints = std::mem::take(&mut step_context.wide_constraints);
            let mut manifold_constraints = std::mem::take(&mut step_context.manifold_constraints);
            let mut contact_constraints = std::mem::take(&mut step_context.contact_constraints);
            let mut contacts = std::mem::take(&mut world.contacts);
            let mut task_contexts = std::mem::take(&mut world.task_contexts);
            let mut joint_colors: Vec<Vec<crate::joint::JointSim>> = world
                .constraint_graph
                .colors
                .iter_mut()
                .map(|color| std::mem::take(&mut color.joint_sims))
                .collect();

            // The user callbacks are not called during the constraint stages;
            // park them so no thread can reach them through the shared world.
            let pre_solve_fcn = world.pre_solve_fcn.take();
            let custom_filter_fcn = world.custom_filter_fcn.take();

            {
                let world_ref: &World = world;
                let context_ref: &StepContext = step_context;

                let shared = SolverShared {
                    world: world_ref,
                    context: context_ref,
                    states: StateAccess::new(&mut states),
                    contacts: crate::sync::SyncSlice::new(&mut contacts),
                    joint_colors: joint_colors
                        .iter_mut()
                        .map(|joints| crate::sync::SyncSlice::new(joints.as_mut_slice()))
                        .collect(),
                    wide_constraints: crate::sync::SyncSlice::new(&mut wide_constraints),
                    manifold_constraints: crate::sync::SyncSlice::new(&mut manifold_constraints),
                    contact_constraints: crate::sync::SyncSlice::new(&mut contact_constraints),
                    task_contexts: crate::sync::SyncSlice::new(&mut task_contexts),
                    profile: crate::sync::SyncPtr::new(&mut solve_profile),
                };

                run_solver_tasks(&shared, worker_count);
            }

            // Restore the taken arrays.
            world.contacts = contacts;
            world.task_contexts = task_contexts;
            for (color, joints) in world.constraint_graph.colors.iter_mut().zip(joint_colors) {
                color.joint_sims = joints;
            }
            world.pre_solve_fcn = pre_solve_fcn;
            world.custom_filter_fcn = custom_filter_fcn;
            step_context.states = states;
            step_context.wide_constraints = wide_constraints;
            step_context.manifold_constraints = manifold_constraints;
            step_context.contact_constraints = contact_constraints;
        }

        // The orchestrator accumulated the per-stage times; apply them now that
        // the world is exclusively borrowed again.
        world.profile.prepare_constraints += solve_profile.prepare_constraints;
        world.profile.integrate_velocities += solve_profile.integrate_velocities;
        world.profile.warm_start += solve_profile.warm_start;
        world.profile.solve_impulses += solve_profile.solve_impulses;
        world.profile.integrate_positions += solve_profile.integrate_positions;
        world.profile.relax_impulses += solve_profile.relax_impulses;
        world.profile.apply_restitution += solve_profile.apply_restitution;
        world.profile.store_impulses += solve_profile.store_impulses;

        world.split_island_id = NULL_INDEX;

        world.profile.constraints = get_milliseconds_and_reset(&mut constraint_ticks);

        // === Update transforms ===
        let transform_ticks = get_ticks();

        // Prepare contact, enlarged body, and island bit sets used in body finalization.
        let awake_island_count = world.solver_sets[AWAKE_SET as usize].island_sims.len() as i32;
        for i in 0..world.worker_count {
            let task_context = &mut world.task_contexts[i as usize];
            task_context.sensor_hits.clear();
            set_bit_count_and_clear(&mut task_context.enlarged_sim_bit_set, awake_body_count as u32);
            set_bit_count_and_clear(&mut task_context.awake_island_bit_set, awake_island_count as u32);
            task_context.split_island_id = NULL_INDEX;
            task_context.split_sleep_time = 0.0;
        }

        // Finalize bodies. Must happen after the constraint solver and after island splitting.
        // (C: b3ParallelFor with min block 16; block partitioning does not affect results.)
        finalize_bodies_task(0, awake_body_count, 0, world, step_context);

        // C frees the arena allocations here (blocks, stages, constraints). The stages
        // were consumed by solver_task_main; the constraint arrays are cleared at the
        // end of solve() after the store/hit-event consumers are done with the contacts.

        world.profile.transforms = get_milliseconds(transform_ticks);
    }

    // Report joint events
    {
        let joint_event_ticks = get_ticks();

        // Gather bits for all joints that have force/torque events
        let mut joint_state_bit_set = std::mem::take(&mut world.task_contexts[0].joint_state_bit_set);
        for i in 1..world.worker_count {
            in_place_union(&mut joint_state_bit_set, &world.task_contexts[i as usize].joint_state_bit_set);
        }

        {
            let word_count = joint_state_bit_set.block_count;
            let world_index0 = world.world_id;

            for k in 0..word_count {
                let mut word = joint_state_bit_set.bits[k as usize];
                while word != 0 {
                    let ctz = ctz64(word);
                    let joint_id = (64 * k + ctz) as i32;

                    b3_assert!((joint_id as usize) < world.joints.len());

                    let joint = &world.joints[joint_id as usize];

                    b3_assert!(joint.set_index == AWAKE_SET);

                    let event = JointEvent {
                        joint_id: JointId {
                            index1: joint_id + 1,
                            world0: world_index0,
                            generation: joint.generation,
                        },
                        user_data: joint.user_data,
                    };

                    world.joint_events.push(event);

                    // Clear the smallest set bit
                    word &= word - 1;
                }
            }
        }

        world.task_contexts[0].joint_state_bit_set = joint_state_bit_set;

        world.profile.joint_events = get_milliseconds(joint_event_ticks);
    }

    // Report hit events
    {
        let hit_ticks = get_ticks();

        b3_assert!(world.contact_hit_events.is_empty());

        // Fast path: if no worker flagged any hit-event candidates during the store stage, skip entirely.
        let mut any_hit_events = false;
        for i in 0..world.worker_count {
            if world.task_contexts[i as usize].has_hit_events {
                any_hit_events = true;
                break;
            }
        }

        if any_hit_events {
            // Union per-worker bits into worker 0's bit set.
            let mut hit_event_bit_set = std::mem::take(&mut world.task_contexts[0].hit_event_bit_set);
            for i in 1..world.worker_count {
                if world.task_contexts[i as usize].has_hit_events {
                    in_place_union(&mut hit_event_bit_set, &world.task_contexts[i as usize].hit_event_bit_set);
                }
            }

            let threshold = world.hit_event_threshold;
            let world_id = world.world_id;

            let word_count = hit_event_bit_set.block_count;
            for k in 0..word_count {
                let mut word = hit_event_bit_set.bits[k as usize];
                while word != 0 {
                    let ctz = ctz64(word);
                    let contact_id = (64 * k + ctz) as i32;

                    let contact = &world.contacts[contact_id as usize];
                    b3_assert!(contact.set_index == AWAKE_SET && contact.color_index != NULL_INDEX);

                    let shape_id_a = contact.shape_id_a;
                    let shape_id_b = contact.shape_id_b;
                    let child_index = contact.child_index;
                    let contact_generation = contact.generation;

                    let shape_a = &world.shapes[shape_id_a as usize];
                    let shape_b = &world.shapes[shape_id_b as usize];
                    let body_a = &world.bodies[shape_a.body_id as usize];
                    let body_b = &world.bodies[shape_b.body_id as usize];
                    let sim_a = crate::joint::get_solve_body_sim(world, step_context, body_a.set_index, body_a.local_index);
                    let sim_b = crate::joint::get_solve_body_sim(world, step_context, body_b.set_index, body_b.local_index);
                    let mid_center = lerp_position(sim_a.center, sim_b.center, 0.5);

                    let mut event = ContactHitEvent {
                        shape_id_a: ShapeId::default(),
                        shape_id_b: ShapeId::default(),
                        contact_id: ContactId::default(),
                        point: crate::math_functions::POS_ZERO,
                        normal: Vec3::ZERO,
                        approach_speed: threshold,
                        user_material_id_a: 0,
                        user_material_id_b: 0,
                    };

                    let mut found = false;
                    let mut triangle_index = 0i32;
                    let contact = &world.contacts[contact_id as usize];
                    for manifold in &contact.manifolds {
                        let point_count = manifold.point_count;
                        for p in 0..point_count {
                            let mp = &manifold.points[p as usize];
                            let approach_speed = -mp.normal_velocity;

                            // Need to check total impulse because the point may be speculative and not colliding
                            if approach_speed > event.approach_speed && mp.total_normal_impulse > 0.0 {
                                event.approach_speed = approach_speed;
                                event.point = offset_pos(mid_center, lerp(mp.anchor_a, mp.anchor_b, 0.5));
                                event.normal = manifold.normal;
                                triangle_index = mp.triangle_index;
                                found = true;
                            }
                        }
                    }

                    if found {
                        let shape_a = &world.shapes[shape_id_a as usize];
                        let shape_b = &world.shapes[shape_id_b as usize];

                        event.shape_id_a = ShapeId { index1: shape_a.id + 1, world0: world_id, generation: shape_a.generation };
                        event.shape_id_b = ShapeId { index1: shape_b.id + 1, world0: world_id, generation: shape_b.generation };

                        event.contact_id = ContactId {
                            index1: contact_id + 1,
                            world0: world_id,
                            generation: contact_generation,
                        };

                        // shapeB is never a compound today (asserted in create_contact), so the
                        // childIndex argument is irrelevant for it. shapeA carries the compound.
                        event.user_material_id_a = get_shape_user_material_id(shape_a, child_index, triangle_index);
                        event.user_material_id_b = get_shape_user_material_id(shape_b, 0, triangle_index);

                        world.contact_hit_events.push(event);
                    }

                    // Clear the smallest set bit
                    word &= word - 1;
                }
            }

            world.task_contexts[0].hit_event_bit_set = hit_event_bit_set;
        }

        world.profile.hit_events = get_milliseconds(hit_ticks);
    }

    {
        let refit_ticks = get_ticks();

        // C finishes world->userTreeTask here; the Rust port rebuilds trees inline
        // during the broad phase update, so there is nothing to finish.

        validate_no_enlarged(&world.broad_phase);

        // Gather bits for all sim bodies that have enlarged AABBs
        let mut enlarged_body_bit_set = std::mem::take(&mut world.task_contexts[0].enlarged_sim_bit_set);
        for i in 1..world.worker_count {
            in_place_union(&mut enlarged_body_bit_set, &world.task_contexts[i as usize].enlarged_sim_bit_set);
        }

        // Enlarge broad-phase proxies and build move array.
        // Apply shape AABB changes to broad-phase. This also creates the move array which must be
        // in deterministic order. Tracking sim bodies because the number of shape ids can be huge.
        // This has to happen before bullets are processed.
        {
            let word_count = enlarged_body_bit_set.block_count;

            for k in 0..word_count {
                let mut word = enlarged_body_bit_set.bits[k as usize];
                while word != 0 {
                    let ctz = ctz64(word);
                    let body_sim_index = (64 * k + ctz) as usize;

                    let body_sim = step_context.sims[body_sim_index];
                    let body = &world.bodies[body_sim.body_id as usize];

                    let mut shape_id = body.head_shape_id;
                    if (body_sim.flags & (IS_BULLET | IS_FAST)) == (IS_BULLET | IS_FAST) {
                        // Fast bullet bodies don't have their final AABB yet
                        while shape_id != NULL_INDEX {
                            let (proxy_key, next_shape_id) = {
                                let shape = &world.shapes[shape_id as usize];
                                (shape.proxy_key, shape.next_shape_id)
                            };

                            // Shape is fast. Its aabb will be enlarged in continuous collision.
                            // Update the move array here for determinism because bullets are processed
                            // below in non-deterministic order.
                            buffer_move(&mut world.broad_phase, proxy_key);

                            shape_id = next_shape_id;
                        }
                    } else {
                        while shape_id != NULL_INDEX {
                            let (enlarged_aabb, proxy_key, fat_aabb, next_shape_id) = {
                                let shape = &world.shapes[shape_id as usize];
                                (shape.enlarged_aabb, shape.proxy_key, shape.fat_aabb, shape.next_shape_id)
                            };

                            // The AABB may not have been enlarged, despite the body being flagged as enlarged.
                            if enlarged_aabb {
                                broad_phase_enlarge_proxy(&mut world.broad_phase, proxy_key, fat_aabb);
                                world.shapes[shape_id as usize].enlarged_aabb = false;
                            }

                            shape_id = next_shape_id;
                        }
                    }

                    // Clear the smallest set bit
                    word &= word - 1;
                }
            }
        }

        world.task_contexts[0].enlarged_sim_bit_set = enlarged_body_bit_set;

        validate_broad_phase(&world.broad_phase);

        world.profile.refit = get_milliseconds(refit_ticks);
    }

    let bullet_body_count = step_context.bullet_bodies.len() as i32;
    if bullet_body_count > 0 {
        let bullet_ticks = get_ticks();

        // Fast bullet bodies
        // Note: a bullet body may be moving slow
        // (C: b3ParallelFor over the bullet array; serial here.)
        for i in 0..bullet_body_count {
            let sim_index = step_context.bullet_bodies[i as usize];
            solve_continuous(world, step_context, sim_index, 0);
        }

        // Serially enlarge broad-phase proxies for bullet shapes.
        // This loop has non-deterministic order in C but it shouldn't affect the result.
        for i in 0..bullet_body_count {
            let bullet_index = step_context.bullet_bodies[i as usize] as usize;
            if step_context.sims[bullet_index].flags & ENLARGE_BOUNDS == 0 {
                continue;
            }

            // Clear flag
            step_context.sims[bullet_index].flags &= !ENLARGE_BOUNDS;

            let body_id = step_context.sims[bullet_index].body_id;
            b3_assert!(0 <= body_id && (body_id as usize) < world.bodies.len());

            let mut shape_id = world.bodies[body_id as usize].head_shape_id;
            while shape_id != NULL_INDEX {
                let (enlarged_aabb, proxy_key, fat_aabb, next_shape_id) = {
                    let shape = &world.shapes[shape_id as usize];
                    (shape.enlarged_aabb, shape.proxy_key, shape.fat_aabb, shape.next_shape_id)
                };

                if !enlarged_aabb {
                    shape_id = next_shape_id;
                    continue;
                }

                // clear flag
                world.shapes[shape_id as usize].enlarged_aabb = false;

                let pid = proxy_id(proxy_key);
                b3_assert!(proxy_type(proxy_key) == BodyType::Dynamic);

                // all fast bullet shapes should already be in the move buffer
                b3_assert!(get_bit(
                    &world.broad_phase.moved_proxies[BodyType::Dynamic as usize],
                    pid as u32
                ));

                crate::dynamic_tree::dynamic_tree_enlarge_proxy(
                    &mut world.broad_phase.trees[BodyType::Dynamic as usize],
                    pid,
                    fat_aabb,
                );

                shape_id = next_shape_id;
            }
        }

        world.profile.bullets = get_milliseconds(bullet_ticks);
    }

    step_context.bullet_bodies.clear();

    // Move the awake set body sims/states back into the world (C: the pointers
    // simply go out of scope). Everything below accesses body sims through the world.
    world.solver_sets[AWAKE_SET as usize].body_sims = std::mem::take(&mut step_context.sims);
    world.solver_sets[AWAKE_SET as usize].body_states = std::mem::take(&mut step_context.states);

    // Report sensor hits. This may include bullet sensor hits.
    {
        let sensor_hit_ticks = get_ticks();

        let worker_count = world.worker_count;
        b3_assert!(worker_count == world.task_contexts.len() as i32);

        for i in 0..worker_count {
            // (C reads the per-worker arrays without clearing them; take/put-back.)
            let hits = std::mem::take(&mut world.task_contexts[i as usize].sensor_hits);

            for hit in &hits {
                let sensor_index = world.shapes[hit.sensor_id as usize].sensor_index;
                let visitor_generation = world.shapes[hit.visitor_id as usize].generation;

                let shape_ref = Visitor {
                    shape_id: hit.visitor_id,
                    generation: visitor_generation,
                };
                world.sensors[sensor_index as usize].hits.push(shape_ref);
            }

            world.task_contexts[i as usize].sensor_hits = hits;
        }

        world.profile.sensor_hits = get_milliseconds(sensor_hit_ticks);
    }

    // Island sleeping
    // This must be done last because putting islands to sleep invalidates the enlarged body bits.
    if world.enable_sleep {
        let sleep_ticks = get_ticks();

        // Collect split island candidate for the next time step. No need to split if sleeping is disabled.
        b3_assert!(world.split_island_id == NULL_INDEX);
        let mut split_island_id = world.split_island_id;
        let mut split_sleep_timer = 0.0f32;
        for i in 0..world.worker_count {
            let task_context = &world.task_contexts[i as usize];
            if task_context.split_island_id != NULL_INDEX && task_context.split_sleep_time >= split_sleep_timer {
                b3_assert!(task_context.split_sleep_time > 0.0);

                // Tie breaking for determinism. Largest island id wins. Needed due to work stealing.
                if task_context.split_sleep_time == split_sleep_timer && task_context.split_island_id < split_island_id {
                    continue;
                }

                split_island_id = task_context.split_island_id;
                split_sleep_timer = task_context.split_sleep_time;
            }
        }
        world.split_island_id = split_island_id;

        let mut awake_island_bit_set = std::mem::take(&mut world.task_contexts[0].awake_island_bit_set);
        for i in 1..world.worker_count {
            in_place_union(&mut awake_island_bit_set, &world.task_contexts[i as usize].awake_island_bit_set);
        }

        // Need to process in reverse because this moves islands to sleeping solver sets.
        let count = world.solver_sets[AWAKE_SET as usize].island_sims.len() as i32;
        for island_index in (0..count).rev() {
            if get_bit(&awake_island_bit_set, island_index as u32) {
                // this island is still awake
                continue;
            }

            let island_id = world.solver_sets[AWAKE_SET as usize].island_sims[island_index as usize].island_id;

            crate::solver_set::try_sleep_island(world, island_id);
        }

        world.task_contexts[0].awake_island_bit_set = awake_island_bit_set;

        crate::physics_world::validate_solver_sets(world);

        world.profile.sleep_islands = get_milliseconds(sleep_ticks);
    }

    // C frees the constraint stack allocations at the end of the constraint section;
    // the consumers (store impulses, hit events) are done. The port clears the
    // arrays but keeps their capacity — they return to the world scratch at the
    // end of world_step (arena-style reuse).
    step_context.wide_constraints.clear();
    step_context.contact_constraints.clear();
    step_context.manifold_constraints.clear();
    step_context.wide_prepare_spans.clear();
    step_context.contact_prepare_spans.clear();
    step_context.overflow_spans.clear();
    step_context.joint_prepare_spans.clear();
}
