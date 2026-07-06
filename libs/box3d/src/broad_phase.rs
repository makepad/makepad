// Port of box3d/src/broad_phase.h (+ broad_phase.c functions to be ported below).

use crate::bitset::BitSet;
use crate::types::{BodyType, DynamicTree, BODY_TYPE_COUNT};

// Store the proxy type in the lower 2 bits of the proxy key. This leaves 30 bits for the id.
#[inline]
pub fn proxy_type(key: i32) -> BodyType {
    match key & 3 {
        0 => BodyType::Static,
        1 => BodyType::Kinematic,
        2 => BodyType::Dynamic,
        _ => panic!("invalid proxy type"),
    }
}

#[inline]
pub fn proxy_id(key: i32) -> i32 {
    key >> 2
}

#[inline]
pub fn proxy_key(id: i32, body_type: BodyType) -> i32 {
    (id << 2) | (body_type as i32)
}

/// A candidate pair produced by the broad phase move query.
/// The C version chains pairs in a lock-free linked list (next/heap dropped).
#[derive(Clone, Copy, Debug, Default)]
pub struct MovePair {
    pub shape_index_a: i32,
    pub shape_index_b: i32,
    pub child_index: i32,
}

#[derive(Clone, Debug, Default)]
pub struct MoveResult {
    pub pair_list: Vec<MovePair>,
}

/// The broad-phase is used for computing pairs and performing volume queries and
/// ray casts. This broad-phase does not persist pairs. Instead, this reports
/// potentially new pairs. It is up to the client to consume the new pairs and to
/// track subsequent overlap.
#[derive(Debug, Default)]
pub struct BroadPhase {
    pub trees: [DynamicTree; BODY_TYPE_COUNT],

    /// Per body-type bit sets indexed by proxyId, marking proxies moved this step.
    /// Paired with move_array which preserves deterministic insertion order for
    /// pair queries.
    pub moved_proxies: [BitSet; BODY_TYPE_COUNT],
    pub move_array: Vec<i32>,

    /// These are the results from the pair query and are used to create new
    /// contacts in deterministic order.
    /// C allocates the result array from the arena each step; the port keeps
    /// the buffer (and the per-result pair list allocations) alive across
    /// steps to avoid allocation churn. Contents are only meaningful inside
    /// update_broad_phase_pairs — logically empty outside it, like the C NULL
    /// pointers. Not serialized by snapshots.
    pub move_results: Vec<MoveResult>,

    /// Reusable per-worker scratch for update_broad_phase_pairs. The pair
    /// filtering now runs inline in the tree-query callback (like C), so only
    /// the rare compound inner query needs a buffer (child_hits). Sized to the
    /// world worker count on first use. Same transient semantics as
    /// move_results. Not serialized by snapshots.
    pub(crate) pair_scratch: Vec<PairScratch>,

    /// Tracks shape pairs that have a Contact.
    pub pair_set: crate::table::HashSet,

    /// PORT EXTENSION — not in upstream C. Scratch for the adaptive broad-phase
    /// hybrid's batch path (BVTT traversal stack + collected candidate pairs).
    /// Same transient semantics as move_results — meaningful only inside
    /// update_broad_phase_pairs. Not serialized by snapshots.
    pub(crate) batch_stack: Vec<(i32, i32, bool)>,
    pub(crate) batch_candidates: Vec<MovePair>,
}

/// Per-worker scratch for the pair query pass (compound inner-query hits only).
#[derive(Debug, Default)]
pub(crate) struct PairScratch {
    pub(crate) child_hits: Vec<u64>,
}

/// The C b3UpdateTreesTask: rebuild the dynamic and kinematic trees, run as a
/// scheduler task overlapping contact creation and the narrow phase. The port
/// TAKES the two trees out of the world for the task's lifetime (so there is
/// no aliasing at all: the task owns them) and finish_tree_task puts them
/// back. Nothing queries these trees inside the overlap window.
#[derive(Default)]
pub struct TreeRebuildJob {
    pub dynamic_tree: DynamicTree,
    pub kinematic_tree: DynamicTree,
    /// PORT EXTENSION: take the refit path for the dynamic tree (see
    /// maintain_dynamic_tree). The kinematic tree is small and always rebuilt.
    pub use_refit: bool,
}

// C: b3UpdateTreesTask
pub(crate) unsafe fn tree_rebuild_trampoline(context: *mut ()) {
    // SAFETY: the job is heap-boxed and owned by world.tree_rebuild_task until
    // finish_tree_task joins it; the box address is stable.
    let job = unsafe { &mut *(context as *mut TreeRebuildJob) };
    maintain_dynamic_tree(&mut job.dynamic_tree, job.use_refit);
    dynamic_tree_rebuild(&mut job.kinematic_tree, false);
}

/// Finish the pending tree rebuild task and restore the trees.
/// C: world->finishTaskFcn(world->userTreeTask, ...). Must run before anything
/// queries the dynamic/kinematic trees (the solve's continuous collision, or
/// sensors).
pub fn finish_tree_task(world: &mut World) {
    if let Some((handle, mut job)) = world.tree_rebuild_task.take() {
        world.task_system.finish(handle);
        world.broad_phase.trees[BodyType::Dynamic as usize] = std::mem::take(&mut job.dynamic_tree);
        world.broad_phase.trees[BodyType::Kinematic as usize] = std::mem::take(&mut job.kinematic_tree);
    }
}

/// This is what triggers new contact pairs to be created.
/// Warning: this must be called in deterministic order.
#[inline]
pub fn buffer_move(bp: &mut BroadPhase, query_proxy: i32) {
    let proxy_type = proxy_type(query_proxy);
    let proxy_id = proxy_id(query_proxy);
    let set = &mut bp.moved_proxies[proxy_type as usize];
    if !crate::bitset::get_bit(set, proxy_id as u32) {
        crate::bitset::set_bit_grow(set, proxy_id as u32);
        bp.move_array.push(query_proxy);
    }
}

// ---------------------------------------------------------------------------
// Port of broad_phase.c
// ---------------------------------------------------------------------------

use crate::b3_assert;
use crate::b3_validate;
use crate::bitset::{clear_bit, create_bit_set, destroy_bit_set, get_bit};
use crate::container::array_remove_swap;
use crate::core::NULL_INDEX;
use crate::dynamic_tree::{
    dynamic_tree_create, dynamic_tree_create_proxy, dynamic_tree_cross_pairs, dynamic_tree_destroy_proxy,
    dynamic_tree_enlarge_proxy, dynamic_tree_get_aabb, dynamic_tree_get_user_data, dynamic_tree_move_proxy,
    dynamic_tree_query, dynamic_tree_rebuild, dynamic_tree_self_pairs, dynamic_tree_validate,
    dynamic_tree_validate_no_enlarged, refit_and_clear_enlarged,
};

/// PORT EXTENSION — not in upstream C. Full median rebuilds between refits
/// while the broad-phase hybrid is active, to keep the dynamic tree balanced
/// under sustained churn. Tuned on the washer scene.
const HYBRID_REBUILD_CADENCE: i32 = 8;

/// PORT EXTENSION — not in upstream C. Maintain the dynamic tree after the
/// pair query. `use_refit` (high-churn step, hybrid enabled) takes the cheap
/// refit path with a periodic full rebuild for balance; otherwise this is
/// exactly the upstream per-step median rebuild.
fn maintain_dynamic_tree(tree: &mut DynamicTree, use_refit: bool) {
    if use_refit {
        if tree.rebuild_countdown <= 0 {
            dynamic_tree_rebuild(tree, false);
            tree.rebuild_countdown = HYBRID_REBUILD_CADENCE;
        } else {
            refit_and_clear_enlarged(tree);
            tree.rebuild_countdown -= 1;
        }
    } else {
        dynamic_tree_rebuild(tree, false);
        tree.rebuild_countdown = 0;
    }
}
use crate::id::ShapeId;
use crate::math_functions::{aabb_overlaps, aabb_transform, invert_transform, max_int, to_relative_transform, AABB, POS_ZERO};
use crate::physics_world::World;
use crate::shape::{should_shapes_collide, ShapeGeometry};
use crate::table::{contains_key, create_set, destroy_set, shape_pair_key};
use crate::types::{Capacity, CustomFilterFcn, ShapeType, DEFAULT_MASK_BITS};

/// C: b3CreateBroadPhase(bp, capacity) fills in place; the port constructs.
pub fn create_broad_phase(capacity: &Capacity) -> BroadPhase {
    const _: () = assert!(BODY_TYPE_COUNT == 3, "must be three body types");

    let mut bp = BroadPhase::default();

    bp.moved_proxies[BodyType::Static as usize] = create_bit_set(max_int(16, capacity.static_shape_count) as u32);
    bp.moved_proxies[BodyType::Kinematic as usize] = create_bit_set(16);
    bp.moved_proxies[BodyType::Dynamic as usize] = create_bit_set(max_int(16, capacity.dynamic_shape_count) as u32);
    bp.move_array = Vec::with_capacity(max_int(0, capacity.dynamic_shape_count) as usize);
    bp.move_results = Vec::new();
    bp.pair_set = create_set(2 * capacity.contact_count);

    let static_capacity = max_int(16, capacity.static_shape_count);
    bp.trees[BodyType::Static as usize] = dynamic_tree_create(static_capacity);

    let kinematic_capacity = 16;
    bp.trees[BodyType::Kinematic as usize] = dynamic_tree_create(kinematic_capacity);

    let dynamic_capacity = max_int(16, capacity.dynamic_shape_count);
    bp.trees[BodyType::Dynamic as usize] = dynamic_tree_create(dynamic_capacity);

    bp
}

pub fn destroy_broad_phase(bp: &mut BroadPhase) {
    for i in 0..BODY_TYPE_COUNT {
        crate::dynamic_tree::dynamic_tree_destroy(&mut bp.trees[i]);
    }

    for i in 0..BODY_TYPE_COUNT {
        destroy_bit_set(&mut bp.moved_proxies[i]);
    }
    bp.move_array = Vec::new();
    destroy_set(&mut bp.pair_set);

    *bp = BroadPhase::default();
}

fn un_buffer_move(bp: &mut BroadPhase, proxy_key: i32) {
    let ptype = proxy_type(proxy_key);
    let pid = proxy_id(proxy_key);
    let set = &mut bp.moved_proxies[ptype as usize];

    if get_bit(set, pid as u32) {
        clear_bit(set, pid as u32);

        // Purge from move buffer. Linear search.
        let count = bp.move_array.len();
        for i in 0..count {
            if bp.move_array[i] == proxy_key {
                array_remove_swap(&mut bp.move_array, i as i32);
                break;
            }
        }
    }
}

pub fn broad_phase_create_proxy(
    bp: &mut BroadPhase,
    proxy_type: BodyType,
    aabb: AABB,
    category_bits: u64,
    shape_index: i32,
    force_pair_creation: bool,
) -> i32 {
    let proxy_id = dynamic_tree_create_proxy(&mut bp.trees[proxy_type as usize], aabb, category_bits, shape_index as u64);
    let key = proxy_key(proxy_id, proxy_type);
    if proxy_type != BodyType::Static || force_pair_creation {
        buffer_move(bp, key);
    }
    key
}

pub fn broad_phase_destroy_proxy(bp: &mut BroadPhase, proxy_key: i32) {
    un_buffer_move(bp, proxy_key);

    let ptype = proxy_type(proxy_key);
    let pid = proxy_id(proxy_key);

    dynamic_tree_destroy_proxy(&mut bp.trees[ptype as usize], pid);
}

pub fn broad_phase_move_proxy(bp: &mut BroadPhase, proxy_key: i32, aabb: AABB) {
    let ptype = proxy_type(proxy_key);
    let pid = proxy_id(proxy_key);

    dynamic_tree_move_proxy(&mut bp.trees[ptype as usize], pid, aabb);
    buffer_move(bp, proxy_key);
}

pub fn broad_phase_enlarge_proxy(bp: &mut BroadPhase, proxy_key: i32, aabb: AABB) {
    b3_assert!(proxy_key != NULL_INDEX);
    let ptype = proxy_type(proxy_key);
    let pid = proxy_id(proxy_key);

    b3_assert!(ptype != BodyType::Static);

    dynamic_tree_enlarge_proxy(&mut bp.trees[ptype as usize], pid, aabb);
    buffer_move(bp, proxy_key);
}

// The filtering half of the C b3PairQueryCallback: everything after the tree
// hit has been resolved to (shape_index, proxy_id, child_index).
#[allow(clippy::too_many_arguments)]
fn try_add_pair(
    world: &World,
    custom_filter: &mut Option<Box<CustomFilterFcn>>,
    result: &mut MoveResult,
    pair_count: &crate::sync::AtomicIndex,
    pair_capacity: i32,
    shape_index: i32,
    proxy_id: i32,
    child_index: i32,
    query_tree_type: BodyType,
    query_proxy_key: i32,
    query_shape_index: i32,
) {
    let broad_phase = &world.broad_phase;

    let proxy_key = proxy_key(proxy_id, query_tree_type);

    // A proxy cannot form a pair with itself.
    b3_assert!(proxy_key != query_proxy_key);

    let tree_type = query_tree_type;
    let query_proxy_type = proxy_type(query_proxy_key);

    // De-duplication: is this proxy also moving?
    if query_proxy_type == BodyType::Dynamic {
        if tree_type == BodyType::Dynamic && proxy_key < query_proxy_key {
            let moved = get_bit(&broad_phase.moved_proxies[tree_type as usize], proxy_id as u32);
            if moved {
                // Both proxies are moving. Avoid duplicate pairs.
                return;
            }
        }
    } else {
        b3_assert!(tree_type == BodyType::Dynamic);
        let moved = get_bit(&broad_phase.moved_proxies[tree_type as usize], proxy_id as u32);
        if moved {
            // Both proxies are moving. Avoid duplicate pairs.
            return;
        }
    }

    // Shared post-dedup filtering + emit. The port extension (BVTT batch path)
    // calls this directly with the (a,b) order the per-mover path would have
    // produced, skipping only the dedup above (a BVTT visits each pair once).
    filter_and_emit_pair(
        world,
        custom_filter,
        result,
        pair_count,
        pair_capacity,
        shape_index,
        query_shape_index,
        child_index,
    );
}

// The filtering tail of try_add_pair (everything after the both-moving dedup):
// pair_set / same-body / sensor / filter / joint / custom, then the emit with
// the given (shape_id_a, shape_id_b) order. shape_id_a is the "hit" shape and
// shape_id_b the "query" shape in the per-mover path; the batch path passes the
// same order it computes (see bvtt_ordered_pair). Filtering is order-symmetric;
// only the pushed (a,b) and the custom-filter argument order depend on it.
#[allow(clippy::too_many_arguments)]
fn filter_and_emit_pair(
    world: &World,
    custom_filter: &mut Option<Box<CustomFilterFcn>>,
    result: &mut MoveResult,
    pair_count: &crate::sync::AtomicIndex,
    pair_capacity: i32,
    shape_id_a: i32,
    shape_id_b: i32,
    child_index: i32,
) {
    let broad_phase = &world.broad_phase;

    let pair_key = shape_pair_key(shape_id_a, shape_id_b, child_index);
    if contains_key(&broad_phase.pair_set, pair_key) {
        // contact exists
        return;
    }

    let shape_a = &world.shapes[shape_id_a as usize];
    let shape_b = &world.shapes[shape_id_b as usize];
    let body_id_a = shape_a.body_id;
    let body_id_b = shape_b.body_id;

    // Are the shapes on the same body?
    if body_id_a == body_id_b {
        return;
    }

    // Sensors are handled elsewhere
    if shape_a.sensor_index != NULL_INDEX || shape_b.sensor_index != NULL_INDEX {
        return;
    }

    if !should_shapes_collide(shape_a.filter, shape_b.filter) {
        return;
    }

    // Does a joint override collision?
    if !crate::body::should_bodies_collide(world, body_id_a, body_id_b) {
        return;
    }

    // Custom user filter
    if shape_a.enable_custom_filtering || shape_b.enable_custom_filtering {
        if let Some(fcn) = custom_filter.as_mut() {
            let id_a = ShapeId { index1: shape_id_a + 1, world0: world.world_id, generation: shape_a.generation };
            let id_b = ShapeId { index1: shape_id_b + 1, world0: world.world_id, generation: shape_b.generation };
            let should_collide = fcn(id_a, id_b);
            if !should_collide {
                return;
            }
        }
    }

    // C claims a slot with an atomic fetch-add and ignores pairs beyond capacity.
    // C: atomic movePairIndex fetch-add, then bounds check (over-budget pairs
    // are dropped; the 16x budget makes this effectively unreachable).
    let pair_index = pair_count.fetch_add(1);
    if pair_index >= pair_capacity {
        return;
    }

    result.pair_list.push(MovePair {
        shape_index_a: shape_id_a,
        shape_index_b: shape_id_b,
        child_index,
    });
}

// One tree query of the C b3FindPairsTask. Mirrors C's b3PairQueryCallback:
// the pair filtering (try_add_pair) runs INLINE inside the tree-query callback
// in discovery order, exactly like C, instead of materializing every hit into
// a Vec and re-iterating (that intermediate store+reload was the hottest cost
// on broad-phase-bound scenes like washer — query_tree_for_pairs dominates the
// profile there). `child_hits` stays as scratch only for the rare compound
// inner query (its callback just collects child indices, then try_add_pair
// runs on them in place — same interleaved order as C's recursion). Non-
// compound scenes never touch child_hits.
#[allow(clippy::too_many_arguments)]
fn query_tree_for_pairs(
    world: &World,
    custom_filter: &mut Option<Box<CustomFilterFcn>>,
    result: &mut MoveResult,
    pair_count: &crate::sync::AtomicIndex,
    pair_capacity: i32,
    fat_aabb: AABB,
    query_tree_type: BodyType,
    query_proxy_key: i32,
    query_shape_index: i32,
    child_hits: &mut Vec<u64>,
) {
    let require_all_bits = false;

    dynamic_tree_query(
        &world.broad_phase.trees[query_tree_type as usize],
        fat_aabb,
        DEFAULT_MASK_BITS,
        require_all_bits,
        &mut |proxy_id, user_data| {
            // Outer query: userData is a shape index.
            let shape_index = user_data as i32;

            // A proxy cannot form a pair with itself.
            if shape_index == query_shape_index {
                return true;
            }

            let shape = &world.shapes[shape_index as usize];
            if let ShapeGeometry::Compound(compound) = &shape.geom {
                // Query bounds are float world space, so the demoted transform is the matching float frame
                let compound_transform =
                    to_relative_transform(crate::body::get_body_transform(world, shape.body_id), POS_ZERO);
                let local_aabb = aabb_transform(invert_transform(compound_transform), fat_aabb);

                // recurse: inner query into the compound. userData is the compound
                // child index, not a shape index.
                let compound = compound.clone();
                child_hits.clear();
                dynamic_tree_query(&compound.tree, local_aabb, DEFAULT_MASK_BITS, require_all_bits, &mut |_child_proxy,
                                                                                                          child_user_data| {
                    child_hits.push(child_user_data);
                    true
                });

                for k2 in 0..child_hits.len() {
                    let child_user_data = child_hits[k2];
                    try_add_pair(
                        world,
                        custom_filter,
                        result,
                        pair_count,
                        pair_capacity,
                        shape_index,
                        proxy_id,
                        child_user_data as i32,
                        query_tree_type,
                        query_proxy_key,
                        query_shape_index,
                    );
                }
                return true;
            }

            try_add_pair(
                world,
                custom_filter,
                result,
                pair_count,
                pair_capacity,
                shape_index,
                proxy_id,
                0,
                query_tree_type,
                query_proxy_key,
                query_shape_index,
            );
            true
        },
    );
}

/// Shared context for the parallel pair-finding pass. Each moved-proxy index
/// is owned by exactly one worker (block partitioning), each worker_index is
/// exclusive per task, and the pair budget is claimed atomically like the C
/// movePairIndex. The custom filter slot is Some only on the serial fallback.
struct FindPairsCtx<'a> {
    world: &'a World,
    move_results: &'a crate::sync::SyncSlice<'a, MoveResult>,
    scratch: &'a crate::sync::SyncSlice<'a, PairScratch>,
    pair_count: &'a crate::sync::AtomicIndex,
    move_pair_capacity: i32,
    custom_filter: crate::sync::SyncPtr<Option<Box<CustomFilterFcn>>>,
    use_custom_filter: bool,
}

// C: b3FindPairsTask trampoline.
unsafe fn find_pairs_trampoline(start_index: i32, end_index: i32, worker_index: i32, context: *mut ()) {
    // SAFETY: the FindPairsCtx lives on the update_broad_phase_pairs stack
    // frame, which blocks in parallel_for until every block completes.
    let ctx = unsafe { &*(context as *const FindPairsCtx) };
    find_pairs_task(ctx, start_index, end_index, worker_index);
}

// C: b3FindPairsTask.
fn find_pairs_task(ctx: &FindPairsCtx, start_index: i32, end_index: i32, worker_index: i32) {
    let world = ctx.world;

    // SAFETY: worker_index is exclusive to this task (parallel_for contract).
    let scratch = unsafe { ctx.scratch.get_mut(worker_index as usize) };

    let mut no_filter: Option<Box<CustomFilterFcn>> = None;
    let custom_filter: &mut Option<Box<CustomFilterFcn>> = if ctx.use_custom_filter {
        // SAFETY: use_custom_filter forces a single worker; exclusive access.
        unsafe { ctx.custom_filter.get() }
    } else {
        &mut no_filter
    };

    for i in start_index..end_index {
        // SAFETY: each moved-proxy index is visited by exactly one worker.
        let result = unsafe { ctx.move_results.get_mut(i as usize) };

        let query_proxy_key = world.broad_phase.move_array[i as usize];
        let query_proxy_type = proxy_type(query_proxy_key);
        let query_proxy_id = proxy_id(query_proxy_key);

        // We have to query the tree with the fat AABB so that
        // we don't fail to create a contact that may touch later.
        let base_tree = &world.broad_phase.trees[query_proxy_type as usize];
        let fat_aabb = dynamic_tree_get_aabb(base_tree, query_proxy_id);
        let query_shape_index = dynamic_tree_get_user_data(base_tree, query_proxy_id) as i32;

        // Compound shape collision invocation is not supported
        b3_validate!(world.shapes[query_shape_index as usize].shape_type() != ShapeType::Compound);

        // Query trees. Only dynamic proxies collide with kinematic and static proxies.
        // Using DEFAULT_MASK_BITS so that Filter::group_index works.
        if query_proxy_type == BodyType::Dynamic {
            query_tree_for_pairs(
                world,
                custom_filter,
                result,
                ctx.pair_count,
                ctx.move_pair_capacity,
                fat_aabb,
                BodyType::Kinematic,
                query_proxy_key,
                query_shape_index,
                &mut scratch.child_hits,
            );
            query_tree_for_pairs(
                world,
                custom_filter,
                result,
                ctx.pair_count,
                ctx.move_pair_capacity,
                fat_aabb,
                BodyType::Static,
                query_proxy_key,
                query_shape_index,
                &mut scratch.child_hits,
            );
        }

        // All proxies collide with dynamic proxies
        query_tree_for_pairs(
            world,
            custom_filter,
            result,
            ctx.pair_count,
            ctx.move_pair_capacity,
            fat_aabb,
            BodyType::Dynamic,
            query_proxy_key,
            query_shape_index,
            &mut scratch.child_hits,
        );
    }
}

// PORT EXTENSION — not in upstream C. Emit one candidate (a=hit/non-mover,
// b=owner/mover), expanding a compound `a` via its inner tree exactly like the
// per-mover query_tree_for_pairs compound branch (a compound is never a mover,
// so it only ever appears as `a`). `b_*` identifies the mover whose fat AABB
// drives the inner query.
#[allow(clippy::too_many_arguments)]
fn emit_batch_candidate(
    world: &World,
    out: &mut Vec<MovePair>,
    a_shape: i32,
    b_shape: i32,
    b_id: i32,
    b_type: BodyType,
) {
    let shape_a = &world.shapes[a_shape as usize];
    if let ShapeGeometry::Compound(compound) = &shape_a.geom {
        let b_tree = &world.broad_phase.trees[b_type as usize];
        let fat_aabb = dynamic_tree_get_aabb(b_tree, b_id);
        let compound_transform =
            to_relative_transform(crate::body::get_body_transform(world, shape_a.body_id), POS_ZERO);
        let local_aabb = aabb_transform(invert_transform(compound_transform), fat_aabb);
        let compound = compound.clone();
        dynamic_tree_query(&compound.tree, local_aabb, DEFAULT_MASK_BITS, false, &mut |_child, child_ud| {
            out.push(MovePair { shape_index_a: a_shape, shape_index_b: b_shape, child_index: child_ud as i32 });
            true
        });
    } else {
        out.push(MovePair { shape_index_a: a_shape, shape_index_b: b_shape, child_index: 0 });
    }
}

// PORT EXTENSION — not in upstream C. The three emit rules, one per BVTT
// traversal. Each maps an overlapping leaf pair to the candidate MovePair(s)
// in the (a=hit/non-mover, b=owner/mover) ordering create_contact expects,
// mirroring the per-mover query path's ordering so the batch and per-mover
// candidate SETs match (debug-asserted).

// dynamic-vs-dynamic leaf pair: b = the mover (or the lower proxy key if both
// moved), a = the other. Mirrors try_add_pair's both-moving dedup.
fn emit_dyn_self(world: &World, out: &mut Vec<MovePair>, id1: i32, id2: i32) {
    let bp = &world.broad_phase;
    let dyn_i = BodyType::Dynamic as usize;
    let moved1 = get_bit(&bp.moved_proxies[dyn_i], id1 as u32);
    let moved2 = get_bit(&bp.moved_proxies[dyn_i], id2 as u32);
    if !moved1 && !moved2 {
        return;
    }
    let dyn_tree = &bp.trees[dyn_i];
    let s1 = dynamic_tree_get_user_data(dyn_tree, id1) as i32;
    let s2 = dynamic_tree_get_user_data(dyn_tree, id2) as i32;
    let (a_shape, b_shape, b_id) = if moved1 && moved2 {
        if proxy_key(id1, BodyType::Dynamic) < proxy_key(id2, BodyType::Dynamic) {
            (s2, s1, id1)
        } else {
            (s1, s2, id2)
        }
    } else if moved1 {
        (s2, s1, id1)
    } else {
        (s1, s2, id2)
    };
    emit_batch_candidate(world, out, a_shape, b_shape, b_id, BodyType::Dynamic);
}

// dynamic-vs-static: static never moves, so the pair exists iff the dynamic
// side moved; per-mover emits (a=static, b=dynamic mover).
fn emit_dyn_static(world: &World, out: &mut Vec<MovePair>, dyn_id: i32, stat_id: i32) {
    let bp = &world.broad_phase;
    let dyn_i = BodyType::Dynamic as usize;
    if !get_bit(&bp.moved_proxies[dyn_i], dyn_id as u32) {
        return;
    }
    let dyn_shape = dynamic_tree_get_user_data(&bp.trees[dyn_i], dyn_id) as i32;
    let stat_shape = dynamic_tree_get_user_data(&bp.trees[BodyType::Static as usize], stat_id) as i32;
    emit_batch_candidate(world, out, stat_shape, dyn_shape, dyn_id, BodyType::Dynamic);
}

// dynamic-vs-kinematic: pair exists iff either moved. If the dynamic side
// moved, per-mover emits (a=kinematic, b=dynamic); else (only kinematic moved)
// (a=dynamic, b=kinematic).
fn emit_dyn_kin(world: &World, out: &mut Vec<MovePair>, dyn_id: i32, kin_id: i32) {
    let bp = &world.broad_phase;
    let dyn_i = BodyType::Dynamic as usize;
    let kin_i = BodyType::Kinematic as usize;
    let moved_d = get_bit(&bp.moved_proxies[dyn_i], dyn_id as u32);
    let moved_k = get_bit(&bp.moved_proxies[kin_i], kin_id as u32);
    if !moved_d && !moved_k {
        return;
    }
    let dyn_shape = dynamic_tree_get_user_data(&bp.trees[dyn_i], dyn_id) as i32;
    let kin_shape = dynamic_tree_get_user_data(&bp.trees[kin_i], kin_id) as i32;
    if moved_d {
        emit_batch_candidate(world, out, kin_shape, dyn_shape, dyn_id, BodyType::Dynamic);
    } else {
        emit_batch_candidate(world, out, dyn_shape, kin_shape, kin_id, BodyType::Kinematic);
    }
}

// PORT EXTENSION — not in upstream C. Collect the batch candidate pairs by
// running the three BVTT self/cross traversals (dynamic self + dynamic×static +
// dynamic×kinematic) and emitting each overlapping leaf pair through its
// per-traversal emit rule into `out`. The upper tree is walked once per
// traversal (a shared descent) in place of one root-to-leaf query per moved
// proxy — the high-churn win. The pair SET is identical to the per-mover path
// (debug-asserted by the caller); the caller canonically sorts `out` for a
// deterministic contact-creation order. Single-threaded: this is an opt-in
// accelerator for churn-heavy scenes on one worker; the per-mover path (which
// filters inline in the query callback) stays the multi-threaded default.
fn collect_batch_candidates(world: &mut World) {
    let mut out = std::mem::take(&mut world.broad_phase.batch_candidates);
    let mut stack = std::mem::take(&mut world.broad_phase.batch_stack);
    out.clear();

    let dyn_i = BodyType::Dynamic as usize;
    let stat_i = BodyType::Static as usize;
    let kin_i = BodyType::Kinematic as usize;

    // `world` is read immutably here; `out`/`stack` are taken-out locals, so the
    // emit closures capturing `&mut out` do not alias the tree reads. The
    // self/cross traversals clear the stack and guard empty trees internally.
    {
        let w = &*world;
        let dyn_tree = &w.broad_phase.trees[dyn_i];
        let stat_tree = &w.broad_phase.trees[stat_i];
        let kin_tree = &w.broad_phase.trees[kin_i];

        dynamic_tree_self_pairs(dyn_tree, &mut stack, &mut |id1, id2| emit_dyn_self(w, &mut out, id1, id2));
        dynamic_tree_cross_pairs(dyn_tree, stat_tree, &mut stack, &mut |d, s| emit_dyn_static(w, &mut out, d, s));
        dynamic_tree_cross_pairs(dyn_tree, kin_tree, &mut stack, &mut |d, k| emit_dyn_kin(w, &mut out, d, k));
    }

    world.broad_phase.batch_candidates = out;
    world.broad_phase.batch_stack = stack;
}

// PORT EXTENSION — not in upstream C. Debug-only correctness oracle: the exact
// SET of shape-pair keys the per-mover path would create this step, computed
// serially into a throwaway. The hash legitimately changes under the hybrid
// (creation order differs), so this SET comparison — not the hash — is what
// proves the batch BVTT finds the identical contacts.
#[cfg(debug_assertions)]
fn per_mover_pair_keys(world: &World) -> std::collections::BTreeSet<u64> {
    let move_count = world.broad_phase.move_array.len();
    let capacity = 16 * move_count as i32;
    let pair_count = crate::sync::AtomicIndex::new(0);
    let mut result = MoveResult::default();
    let mut child_hits: Vec<u64> = Vec::new();
    let mut no_filter: Option<Box<CustomFilterFcn>> = None;
    for i in 0..move_count {
        let query_proxy_key = world.broad_phase.move_array[i];
        let query_proxy_type = proxy_type(query_proxy_key);
        let query_proxy_id = proxy_id(query_proxy_key);
        let base_tree = &world.broad_phase.trees[query_proxy_type as usize];
        let fat_aabb = dynamic_tree_get_aabb(base_tree, query_proxy_id);
        let query_shape_index = dynamic_tree_get_user_data(base_tree, query_proxy_id) as i32;
        if query_proxy_type == BodyType::Dynamic {
            query_tree_for_pairs(world, &mut no_filter, &mut result, &pair_count, capacity, fat_aabb, BodyType::Kinematic, query_proxy_key, query_shape_index, &mut child_hits);
            query_tree_for_pairs(world, &mut no_filter, &mut result, &pair_count, capacity, fat_aabb, BodyType::Static, query_proxy_key, query_shape_index, &mut child_hits);
        }
        query_tree_for_pairs(world, &mut no_filter, &mut result, &pair_count, capacity, fat_aabb, BodyType::Dynamic, query_proxy_key, query_shape_index, &mut child_hits);
    }
    result.pair_list.iter().map(|p| shape_pair_key(p.shape_index_a, p.shape_index_b, p.child_index)).collect()
}

pub fn update_broad_phase_pairs(world: &mut World) {
    let move_count = world.broad_phase.move_array.len() as i32;

    if move_count == 0 {
        return;
    }

    // PORT EXTENSION — not in upstream C. High-churn step? When a large
    // fraction of proxies moved (coherent-motion scenes like a rotating drum),
    // the per-step near-full median rebuild dominates the broad phase; take the
    // cheap refit path instead (with a periodic rebuild for balance). Settled
    // scenes stay below the threshold and keep the exact upstream rebuild, so
    // they are byte-identical. The candidate pair SET is unchanged either way.
    let use_refit_dynamic = world.enable_broad_phase_hybrid
        && move_count * 4 > world.broad_phase.trees[BodyType::Dynamic as usize].proxy_count;

    // C allocates moveResults/movePairs from the arena and links pairs into
    // per-result lists; the port reuses the persistent buffers on BroadPhase
    // (taken out for the duration of the update so the world can be read
    // immutably). Logical content is only valid inside this function, like
    // the C arena pointers.
    let move_pair_capacity = 16 * move_count;
    let pair_count = crate::sync::AtomicIndex::new(0);
    let mut move_results = std::mem::take(&mut world.broad_phase.move_results);
    let mut pair_scratch = std::mem::take(&mut world.broad_phase.pair_scratch);

    // Reuse the per-result pair list allocations from previous steps. The batch
    // path filters serially into move_results[0], so move_count entries (>= 1
    // here, since move_count == 0 returned early) covers both paths.
    if move_results.len() < move_count as usize {
        move_results.resize_with(move_count as usize, MoveResult::default);
    }
    for result in &mut move_results[..move_count as usize] {
        result.pair_list.clear();
    }
    if pair_scratch.len() < world.worker_count as usize {
        pair_scratch.resize_with(world.worker_count as usize, PairScratch::default);
    }

    // Take the custom filter so the query phase can read the world immutably.
    // A world with a custom filter falls back to a single worker because
    // Box<dyn FnMut> is not Sync (C requires the callback to be thread-safe).
    let mut custom_filter = world.custom_filter_fcn.take();

    if use_refit_dynamic {
        // PORT EXTENSION — not in upstream C. Batch BVTT path: three shared-tree
        // traversals (dynamic self + dynamic×static + dynamic×kinematic) in
        // place of the per-moved-proxy queries. Produces the IDENTICAL pair SET
        // (debug-asserted below against the per-mover path — the hash cannot
        // prove this since creation order, and thus contact ids, legitimately
        // change). The candidates are canonically sorted before filtering, so
        // the contact-creation order is deterministic run-to-run and matches
        // across builds.
        #[cfg(debug_assertions)]
        let per_mover_keys = per_mover_pair_keys(&*world);

        collect_batch_candidates(world);
        // Take the candidates out to sort + consume them while the world is
        // borrowed immutably by the filter.
        let mut batch_candidates = std::mem::take(&mut world.broad_phase.batch_candidates);
        // Canonical order → deterministic contact-creation order.
        batch_candidates.sort_unstable_by_key(|p| (p.shape_index_a, p.shape_index_b, p.child_index));

        let result = &mut move_results[0];
        result.pair_list.clear();
        for cand in &batch_candidates {
            filter_and_emit_pair(
                &*world,
                &mut custom_filter,
                result,
                &pair_count,
                move_pair_capacity,
                cand.shape_index_a,
                cand.shape_index_b,
                cand.child_index,
            );
        }
        world.broad_phase.batch_candidates = batch_candidates;

        #[cfg(debug_assertions)]
        {
            let batch_keys: std::collections::BTreeSet<u64> = move_results[0]
                .pair_list
                .iter()
                .map(|p| shape_pair_key(p.shape_index_a, p.shape_index_b, p.child_index))
                .collect();
            assert_eq!(
                batch_keys, per_mover_keys,
                "broad-phase hybrid: batch BVTT pair set differs from the per-mover set"
            );
        }
    } else {
        // C: b3ParallelFor(world, b3FindPairsTask, moveCount, 64, world, "pairs").
        let use_custom_filter = custom_filter.is_some();
        let effective_workers = if use_custom_filter { 1 } else { world.worker_count };

        let results_slice = crate::sync::SyncSlice::new(&mut move_results[..move_count as usize]);
        let scratch_slice = crate::sync::SyncSlice::new(&mut pair_scratch);
        let find_ctx = FindPairsCtx {
            world: &*world,
            move_results: &results_slice,
            scratch: &scratch_slice,
            pair_count: &pair_count,
            move_pair_capacity,
            custom_filter: crate::sync::SyncPtr::new(&mut custom_filter),
            use_custom_filter,
        };

        let min_range = 64;
        // SAFETY: each moved-proxy index is visited by exactly one worker
        // (block partitioning), each worker_index is exclusive per task, and
        // the context outlives parallel_for (which blocks).
        unsafe {
            crate::parallel_for::parallel_for(
                &find_ctx.world.task_system,
                effective_workers,
                find_pairs_trampoline,
                move_count,
                min_range,
                &find_ctx as *const FindPairsCtx as *mut (),
                "pairs",
            );
        }
    }

    world.custom_filter_fcn = custom_filter;
    world.broad_phase.pair_scratch = pair_scratch;

    // Task that can be done in parallel with contact creation and the narrow
    // phase: rebuild the collision tree for dynamic and kinematic bodies to
    // keep their query performance good. The port takes the trees out of the
    // world so the task owns them; world_step restores them via
    // finish_tree_task before the solve (C keeps the window open until the
    // solve's continuous pass finishes it).
    // (C gates on world->taskCount < B3_MAX_TASKS; TaskSystem::enqueue
    // centralizes the budget and runs the task inline when exhausted, which is
    // still correct here: the job completes synchronously and finish_tree_task
    // restores the trees.)
    if world.worker_count > 1 && world.task_system.is_parallel() {
        b3_assert!(world.tree_rebuild_task.is_none());
        let mut job = Box::new(TreeRebuildJob {
            dynamic_tree: std::mem::take(&mut world.broad_phase.trees[BodyType::Dynamic as usize]),
            kinematic_tree: std::mem::take(&mut world.broad_phase.trees[BodyType::Kinematic as usize]),
            use_refit: use_refit_dynamic,
        });
        let job_ptr = &mut *job as *mut TreeRebuildJob as *mut ();
        // SAFETY: the boxed job is stored on the world and outlives the
        // task; finish_tree_task joins before the trees are used again.
        let handle = unsafe {
            world.task_system.enqueue(tree_rebuild_trampoline, job_ptr, "rebuild tree")
        };
        world.tree_rebuild_task = Some((handle, job));
    } else {
        // Serial fallback, exactly the C else-branch (plus the PORT EXTENSION
        // refit path for the dynamic tree on high-churn steps).
        maintain_dynamic_tree(&mut world.broad_phase.trees[BodyType::Dynamic as usize], use_refit_dynamic);
        dynamic_tree_rebuild(&mut world.broad_phase.trees[BodyType::Kinematic as usize], false);
    }

    // Single-threaded work
    // - Create contacts in deterministic order
    if use_refit_dynamic {
        // PORT EXTENSION — batch path: the filtered list is already in
        // canonical (shape_a, shape_b, child) order, so creation order is
        // deterministic. move_results is a local (disjoint from world) so
        // create_contact can borrow world mutably while we read the list.
        for idx in 0..move_results[0].pair_list.len() {
            let pair = move_results[0].pair_list[idx];
            crate::contact::create_contact(world, pair.shape_index_a, pair.shape_index_b, pair.child_index);
        }
    } else {
        // C builds each pair list with head insertion and then walks it, so
        // pairs are consumed in reverse discovery order; .rev() replicates that.
        // Only the first move_count entries are valid this step (the buffer may
        // retain more from a previous, larger step).
        for result in &move_results[..move_count as usize] {
            for pair in result.pair_list.iter().rev() {
                crate::contact::create_contact(world, pair.shape_index_a, pair.shape_index_b, pair.child_index);
            }
        }
    }

    // Return the reusable buffer (capacity retained for the next step).
    world.broad_phase.move_results = move_results;

    // Reset move buffer: clear only the bits that were set this step.
    // Invariant: bit set in moved_proxies[type] iff proxyKey is present in move_array.
    for i in 0..world.broad_phase.move_array.len() {
        let key = world.broad_phase.move_array[i];
        let ptype = proxy_type(key);
        let pid = proxy_id(key);
        clear_bit(&mut world.broad_phase.moved_proxies[ptype as usize], pid as u32);
    }
    world.broad_phase.move_array.clear();

    crate::physics_world::validate_solver_sets(world);
}

pub fn broad_phase_test_overlap(bp: &BroadPhase, proxy_key_a: i32, proxy_key_b: i32) -> bool {
    let type_index_a = proxy_type(proxy_key_a);
    let proxy_id_a = proxy_id(proxy_key_a);
    let type_index_b = proxy_type(proxy_key_b);
    let proxy_id_b = proxy_id(proxy_key_b);

    let aabb_a = dynamic_tree_get_aabb(&bp.trees[type_index_a as usize], proxy_id_a);
    let aabb_b = dynamic_tree_get_aabb(&bp.trees[type_index_b as usize], proxy_id_b);
    aabb_overlaps(aabb_a, aabb_b)
}

pub fn broad_phase_get_shape_index(bp: &BroadPhase, proxy_key: i32) -> i32 {
    let type_index = proxy_type(proxy_key);
    let pid = proxy_id(proxy_key);

    dynamic_tree_get_user_data(&bp.trees[type_index as usize], pid) as i32
}

pub fn validate_broad_phase(bp: &BroadPhase) {
    dynamic_tree_validate(&bp.trees[BodyType::Dynamic as usize]);
    dynamic_tree_validate(&bp.trees[BodyType::Kinematic as usize]);

    // todo validate every shape AABB is contained in tree AABB
}

pub fn validate_no_enlarged(bp: &BroadPhase) {
    // C gates this behind B3_ENABLE_VALIDATION; the port runs it in debug builds.
    if cfg!(debug_assertions) {
        for j in 0..BODY_TYPE_COUNT {
            dynamic_tree_validate_no_enlarged(&bp.trees[j]);
        }
    }
}
