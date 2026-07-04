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
    pub move_results: Vec<MoveResult>,

    /// Tracks shape pairs that have a Contact.
    pub pair_set: crate::table::HashSet,
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
    dynamic_tree_create, dynamic_tree_create_proxy, dynamic_tree_destroy_proxy, dynamic_tree_enlarge_proxy,
    dynamic_tree_get_aabb, dynamic_tree_get_user_data, dynamic_tree_move_proxy, dynamic_tree_query,
    dynamic_tree_rebuild, dynamic_tree_validate, dynamic_tree_validate_no_enlarged,
};
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
    pair_count: &mut i32,
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

    let pair_key = shape_pair_key(shape_index, query_shape_index, child_index);
    if contains_key(&broad_phase.pair_set, pair_key) {
        // contact exists
        return;
    }

    // Order shapes so that the shape pair key works correctly
    let shape_id_a = shape_index;
    let shape_id_b = query_shape_index;
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
    let pair_index = *pair_count;
    *pair_count += 1;
    if pair_index >= pair_capacity {
        return;
    }

    result.pair_list.push(MovePair {
        shape_index_a: shape_id_a,
        shape_index_b: shape_id_b,
        child_index,
    });
}

// One tree query of the C b3FindPairsTask. The C code filters inside the tree
// query callback; the port collects the raw hits first (same discovery order),
// then filters, so the tree borrow does not overlap the world reads. Compound
// shapes expand at their discovery position, matching the C recursion.
#[allow(clippy::too_many_arguments)]
fn query_tree_for_pairs(
    world: &World,
    custom_filter: &mut Option<Box<CustomFilterFcn>>,
    result: &mut MoveResult,
    pair_count: &mut i32,
    pair_capacity: i32,
    fat_aabb: AABB,
    query_tree_type: BodyType,
    query_proxy_key: i32,
    query_shape_index: i32,
) {
    let require_all_bits = false;

    let mut hits: Vec<(i32, u64)> = Vec::new();
    dynamic_tree_query(
        &world.broad_phase.trees[query_tree_type as usize],
        fat_aabb,
        DEFAULT_MASK_BITS,
        require_all_bits,
        &mut |pid, user_data| {
            hits.push((pid, user_data));
            true
        },
    );

    for (proxy_id, user_data) in hits {
        // Outer query: userData is a shape index.
        let shape_index = user_data as i32;

        // A proxy cannot form a pair with itself.
        if shape_index == query_shape_index {
            continue;
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
            let mut child_hits: Vec<u64> = Vec::new();
            dynamic_tree_query(&compound.tree, local_aabb, DEFAULT_MASK_BITS, require_all_bits, &mut |_child_proxy,
                                                                                                      child_user_data| {
                child_hits.push(child_user_data);
                true
            });

            for child_user_data in child_hits {
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
            continue;
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
    }
}

pub fn update_broad_phase_pairs(world: &mut World) {
    let move_count = world.broad_phase.move_array.len() as i32;

    if move_count == 0 {
        return;
    }

    // C allocates moveResults/movePairs from the arena and links pairs into
    // per-result lists; the port uses local Vecs (BroadPhase::move_results
    // stays empty outside this function, like the C NULL pointers).
    let move_pair_capacity = 16 * move_count;
    let mut pair_count: i32 = 0;
    let mut move_results: Vec<MoveResult> = Vec::with_capacity(move_count as usize);

    // Take the custom filter so the query phase can read the world immutably.
    let mut custom_filter = world.custom_filter_fcn.take();

    for i in 0..move_count as usize {
        let mut result = MoveResult::default();

        let query_proxy_key = world.broad_phase.move_array[i];
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
                &mut custom_filter,
                &mut result,
                &mut pair_count,
                move_pair_capacity,
                fat_aabb,
                BodyType::Kinematic,
                query_proxy_key,
                query_shape_index,
            );
            query_tree_for_pairs(
                world,
                &mut custom_filter,
                &mut result,
                &mut pair_count,
                move_pair_capacity,
                fat_aabb,
                BodyType::Static,
                query_proxy_key,
                query_shape_index,
            );
        }

        // All proxies collide with dynamic proxies
        query_tree_for_pairs(
            world,
            &mut custom_filter,
            &mut result,
            &mut pair_count,
            move_pair_capacity,
            fat_aabb,
            BodyType::Dynamic,
            query_proxy_key,
            query_shape_index,
        );

        move_results.push(result);
    }

    world.custom_filter_fcn = custom_filter;

    // Task that in C can run in parallel with contact creation:
    // rebuild the collision tree for dynamic and kinematic bodies to keep their
    // query performance good. Serial port runs it inline (the C fallback path).
    dynamic_tree_rebuild(&mut world.broad_phase.trees[BodyType::Dynamic as usize], false);
    dynamic_tree_rebuild(&mut world.broad_phase.trees[BodyType::Kinematic as usize], false);

    // Single-threaded work
    // - Create contacts in deterministic order
    // C builds each pair list with head insertion and then walks it, so pairs
    // are consumed in reverse discovery order; .rev() replicates that.
    for result in &move_results {
        for pair in result.pair_list.iter().rev() {
            crate::contact::create_contact(world, pair.shape_index_a, pair.shape_index_b, pair.child_index);
        }
    }

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
