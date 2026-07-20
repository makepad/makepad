// Port of box3d/src/solver_set.h + solver_set.c

use crate::b3_assert;
use crate::bitset::clear_bit;
use crate::body::{BodySim, BodyState, HAD_TIME_OF_IMPACT, IDENTITY_BODY_STATE, IS_FAST, IS_SPEED_CAPPED};
use crate::constants::GRAPH_COLOR_COUNT;
use crate::constraint_graph::OVERFLOW_INDEX;
use crate::contact::{CONTACT_TOUCHING_FLAG, SIM_MESH_CONTACT, SIM_TOUCHING_FLAG};
use crate::container::array_remove_swap;
use crate::core::NULL_INDEX;
use crate::id_pool::{alloc_id, free_id};
use crate::island::IslandSim;
use crate::joint::JointSim;
use crate::physics_world::{World, AWAKE_SET, DISABLED_SET, FIRST_SLEEPING_SET};

/// This holds solver set data. The following sets are used:
/// - static set for all static bodies and joints between static bodies
/// - active (awake) set for all active bodies with body states
/// - disabled set for disabled bodies and their joints
/// - all further sets are sleeping island sets along with their contacts and joints
/// The purpose of solver sets is to achieve high memory locality.
#[derive(Clone, Debug, Default)]
pub struct SolverSet {
    /// Body array. Empty for unused set.
    pub body_sims: Vec<BodySim>,

    /// Body state only exists for active set.
    pub body_states: Vec<BodyState>,

    /// This holds sleeping/disabled joints. Empty for static/active set.
    pub joint_sims: Vec<JointSim>,

    /// This holds all contacts for sleeping sets.
    /// This holds non-touching contacts for the awake set.
    /// This should be empty for the static and disabled sets.
    pub contact_indices: Vec<i32>,

    /// The awake set has an array of islands. Sleeping sets normally have a single island.
    /// The static and disabled sets have no islands.
    pub island_sims: Vec<IslandSim>,

    /// Aligns with World::solver_set_id_pool. Used to create a stable id for
    /// body/contact/joint/islands.
    pub set_index: i32,
}

// ---------------------------------------------------------------------------
// solver_set.c
// ---------------------------------------------------------------------------

pub fn destroy_solver_set(world: &mut World, set_index: i32) {
    {
        let set = &mut world.solver_sets[set_index as usize];
        *set = SolverSet::default();
        set.set_index = NULL_INDEX;
    }
    free_id(&mut world.solver_set_id_pool, set_index);
}

// Wake a solver set. Does not merge islands.
// Contacts can be in several places:
// 1. non-touching contacts in the disabled set
// 2. non-touching contacts already in the awake set
// 3. touching contacts in the sleeping set
// This handles contact types 1 and 3. Type 2 doesn't need any action.
pub fn wake_solver_set(world: &mut World, set_index: i32) {
    b3_assert!(set_index >= FIRST_SLEEPING_SET);

    let body_count = world.solver_sets[set_index as usize].body_sims.len();
    for i in 0..body_count {
        let sim_src = world.solver_sets[set_index as usize].body_sims[i];

        let awake_body_count = world.solver_sets[AWAKE_SET as usize].body_sims.len() as i32;
        let head_contact_key;
        let body_flags;
        {
            let body = &mut world.bodies[sim_src.body_id as usize];
            b3_assert!(body.set_index == set_index);
            body.set_index = AWAKE_SET;
            body.local_index = awake_body_count;

            // Reset sleep timer
            body.sleep_time = 0.0;

            head_contact_key = body.head_contact_key;
            body_flags = body.flags;
        }

        world.solver_sets[AWAKE_SET as usize].body_sims.push(sim_src);

        let mut state = IDENTITY_BODY_STATE;
        state.flags = body_flags;
        world.solver_sets[AWAKE_SET as usize].body_states.push(state);

        // move non-touching contacts from disabled set to awake set
        let mut contact_key = head_contact_key;
        while contact_key != NULL_INDEX {
            let edge_index = contact_key & 1;
            let contact_id = contact_key >> 1;

            let (next_key, contact_set_index, local_index) = {
                let contact = &world.contacts[contact_id as usize];
                (
                    contact.edges[edge_index as usize].next_key,
                    contact.set_index,
                    contact.local_index,
                )
            };
            contact_key = next_key;

            if contact_set_index != DISABLED_SET {
                b3_assert!(contact_set_index == AWAKE_SET || contact_set_index == set_index);
                continue;
            }

            b3_assert!(
                0 <= local_index
                    && (local_index as usize) < world.solver_sets[DISABLED_SET as usize].contact_indices.len()
            );
            b3_assert!(
                world.solver_sets[DISABLED_SET as usize].contact_indices[local_index as usize] == contact_id
            );

            {
                let contact = &world.contacts[contact_id as usize];
                b3_assert!((contact.flags & CONTACT_TOUCHING_FLAG) == 0 && contact.manifolds.is_empty());
            }

            let awake_contact_count = world.solver_sets[AWAKE_SET as usize].contact_indices.len() as i32;
            {
                let contact = &mut world.contacts[contact_id as usize];
                contact.set_index = AWAKE_SET;
                contact.local_index = awake_contact_count;
            }
            world.solver_sets[AWAKE_SET as usize].contact_indices.push(contact_id);

            let moved_local_index =
                array_remove_swap(&mut world.solver_sets[DISABLED_SET as usize].contact_indices, local_index);
            if moved_local_index != NULL_INDEX {
                // fix moved element
                let moved_contact_index =
                    world.solver_sets[DISABLED_SET as usize].contact_indices[local_index as usize];
                let moved_contact = &mut world.contacts[moved_contact_index as usize];
                b3_assert!(moved_contact.local_index == moved_local_index);
                moved_contact.local_index = local_index;
            }
        }
    }

    // Transfer touching contacts from sleeping set to constraint graph.
    {
        let contact_count = world.solver_sets[set_index as usize].contact_indices.len();
        for i in 0..contact_count {
            let contact_index = world.solver_sets[set_index as usize].contact_indices[i];
            {
                let contact = &world.contacts[contact_index as usize];
                b3_assert!((contact.flags & CONTACT_TOUCHING_FLAG) != 0);
                b3_assert!((contact.flags & SIM_TOUCHING_FLAG) != 0);
                b3_assert!(contact.set_index == set_index);
            }
            crate::constraint_graph::add_contact_to_graph(world, contact_index);
            world.contacts[contact_index as usize].set_index = AWAKE_SET;
        }
    }

    // transfer joints from sleeping set to awake set
    {
        let joint_count = world.solver_sets[set_index as usize].joint_sims.len();
        for i in 0..joint_count {
            let joint_sim = world.solver_sets[set_index as usize].joint_sims[i];
            let joint_id = joint_sim.joint_id;
            b3_assert!(world.joints[joint_id as usize].set_index == set_index);
            crate::constraint_graph::add_joint_to_graph(world, joint_sim, joint_id);
            world.joints[joint_id as usize].set_index = AWAKE_SET;
        }
    }

    // transfer island from sleeping set to awake set
    // Usually a sleeping set has only one island, but it is possible
    // that joints are created between sleeping islands and they
    // are moved to the same sleeping set.
    {
        let island_count = world.solver_sets[set_index as usize].island_sims.len();
        for i in 0..island_count {
            let island_src = world.solver_sets[set_index as usize].island_sims[i];
            let awake_island_count = world.solver_sets[AWAKE_SET as usize].island_sims.len() as i32;
            {
                let island = &mut world.islands[island_src.island_id as usize];
                island.set_index = AWAKE_SET;
                island.local_index = awake_island_count;
            }
            world.solver_sets[AWAKE_SET as usize].island_sims.push(island_src);
        }
    }

    // destroy the sleeping set
    destroy_solver_set(world, set_index);
}

pub fn try_sleep_island(world: &mut World, island_id: i32) {
    {
        let island = &world.islands[island_id as usize];
        b3_assert!(island.set_index == AWAKE_SET);

        // Cannot put an island to sleep while it has a pending split and more than one body.
        if island.constraint_remove_count > 0 && island.bodies.len() > 1 {
            return;
        }
    }

    // island is sleeping
    // - create new sleeping solver set
    // - move island to sleeping solver set
    // - identify non-touching contacts that should move to sleeping solver set or disabled set
    // - remove old island
    // - fix island
    let sleep_set_id = alloc_id(&mut world.solver_set_id_pool);
    if sleep_set_id == world.solver_sets.len() as i32 {
        let set = SolverSet { set_index: NULL_INDEX, ..Default::default() };
        world.solver_sets.push(set);
    }

    {
        let island = &world.islands[island_id as usize];
        let body_count = island.bodies.len();
        let contact_count = island.contacts.len();
        let joint_count = island.joints.len();

        let sleep_set = &mut world.solver_sets[sleep_set_id as usize];
        *sleep_set = SolverSet::default();
        sleep_set.set_index = sleep_set_id;
        sleep_set.body_sims.reserve(body_count);
        sleep_set.contact_indices.reserve(contact_count);
        sleep_set.joint_sims.reserve(joint_count);
    }

    b3_assert!(
        0 <= world.islands[island_id as usize].local_index
            && (world.islands[island_id as usize].local_index as usize)
                < world.solver_sets[AWAKE_SET as usize].island_sims.len()
    );

    // move awake bodies to sleeping set
    // this shuffles around bodies in the awake set
    {
        let island_body_count = world.islands[island_id as usize].bodies.len();
        for i in 0..island_body_count {
            let body_id = world.islands[island_id as usize].bodies[i];

            let (body_move_index, awake_body_index, head_contact_key) = {
                let body = &world.bodies[body_id as usize];
                b3_assert!(body.set_index == AWAKE_SET);
                b3_assert!(body.island_id == island_id);
                b3_assert!(body.island_index == i as i32);
                (body.body_move_index, body.local_index, body.head_contact_key)
            };

            // Update the body move event to indicate this body fell asleep
            // It could happen the body is forced asleep before it ever moves.
            if body_move_index != NULL_INDEX {
                let generation = world.bodies[body_id as usize].generation;
                let move_event = &mut world.body_move_events[body_move_index as usize];
                b3_assert!(move_event.body_id.index1 - 1 == body_id);
                b3_assert!(move_event.body_id.generation == generation);
                move_event.fell_asleep = true;
                world.bodies[body_id as usize].body_move_index = NULL_INDEX;
            }

            let awake_sim = world.solver_sets[AWAKE_SET as usize].body_sims[awake_body_index as usize];

            // move body sim to sleep set
            let sleep_body_index = world.solver_sets[sleep_set_id as usize].body_sims.len() as i32;
            world.solver_sets[sleep_set_id as usize].body_sims.push(awake_sim);

            let moved_index =
                array_remove_swap(&mut world.solver_sets[AWAKE_SET as usize].body_sims, awake_body_index);
            if moved_index != NULL_INDEX {
                // fix local index on moved element
                let moved_id =
                    world.solver_sets[AWAKE_SET as usize].body_sims[awake_body_index as usize].body_id;
                let moved_body = &mut world.bodies[moved_id as usize];
                b3_assert!(moved_body.local_index == moved_index);
                moved_body.local_index = awake_body_index;
            }

            // destroy state, no need to clone
            array_remove_swap(&mut world.solver_sets[AWAKE_SET as usize].body_states, awake_body_index);

            {
                let body = &mut world.bodies[body_id as usize];
                body.set_index = sleep_set_id;
                body.local_index = sleep_body_index;
            }

            // Move non-touching contacts to the disabled set.
            // Non-touching contacts may exist between sleeping islands and there is no clear ownership.
            let mut contact_key = head_contact_key;
            while contact_key != NULL_INDEX {
                let contact_id = contact_key >> 1;
                let edge_index = contact_key & 1;

                let (next_key, contact_set_index, color_index, other_body_id, local_index) = {
                    let contact = &world.contacts[contact_id as usize];
                    b3_assert!(contact.set_index == AWAKE_SET || contact.set_index == DISABLED_SET);
                    let other_edge_index = edge_index ^ 1;
                    (
                        contact.edges[edge_index as usize].next_key,
                        contact.set_index,
                        contact.color_index,
                        contact.edges[other_edge_index as usize].body_id,
                        contact.local_index,
                    )
                };
                contact_key = next_key;

                if contact_set_index == DISABLED_SET {
                    // already moved to disabled set by another body in the island
                    continue;
                }

                if color_index != NULL_INDEX {
                    // contact is touching and will be moved separately
                    b3_assert!((world.contacts[contact_id as usize].flags & CONTACT_TOUCHING_FLAG) != 0);
                    continue;
                }

                // the other body may still be awake, it still may go to sleep and then it will be responsible
                // for moving this contact to the disabled set.
                if world.bodies[other_body_id as usize].set_index == AWAKE_SET {
                    continue;
                }

                b3_assert!(
                    world.solver_sets[AWAKE_SET as usize].contact_indices[local_index as usize] == contact_id
                );

                {
                    let contact = &world.contacts[contact_id as usize];
                    b3_assert!(contact.manifolds.is_empty());
                    b3_assert!((contact.flags & CONTACT_TOUCHING_FLAG) == 0);
                }

                // Move the non-touching contact to the disabled set.
                let disabled_count = world.solver_sets[DISABLED_SET as usize].contact_indices.len() as i32;
                {
                    let contact = &mut world.contacts[contact_id as usize];
                    contact.set_index = DISABLED_SET;

                    // This is mandatory for validation to work correctly
                    contact.local_index = disabled_count;
                }
                let self_contact_id = world.contacts[contact_id as usize].contact_id;
                world.solver_sets[DISABLED_SET as usize].contact_indices.push(self_contact_id);

                let moved_local_index =
                    array_remove_swap(&mut world.solver_sets[AWAKE_SET as usize].contact_indices, local_index);
                if moved_local_index != NULL_INDEX {
                    // fix moved element
                    let moved_contact_index =
                        world.solver_sets[AWAKE_SET as usize].contact_indices[local_index as usize];
                    let moved_contact = &mut world.contacts[moved_contact_index as usize];
                    b3_assert!(moved_contact.local_index == moved_local_index);
                    moved_contact.local_index = local_index;
                }
            }
        }
    }

    // move touching contacts to sleeping set
    // this shuffles contacts in the awake set
    {
        let island_contact_count = world.islands[island_id as usize].contacts.len();
        for i in 0..island_contact_count {
            let contact_id = world.islands[island_id as usize].contacts[i].contact_id;

            let (flags, color_index, local_index, edge_body_a, edge_body_b) = {
                let contact = &world.contacts[contact_id as usize];
                b3_assert!(contact.set_index == AWAKE_SET);
                b3_assert!(contact.island_id == island_id);
                b3_assert!(contact.island_index == i as i32);
                (
                    contact.flags,
                    contact.color_index,
                    contact.local_index,
                    contact.edges[0].body_id,
                    contact.edges[1].body_id,
                )
            };
            b3_assert!(0 <= color_index && color_index < GRAPH_COLOR_COUNT as i32);

            // Remove bodies from graph coloring associated with this constraint
            if color_index != OVERFLOW_INDEX {
                // might clear a bit for a static body, but this has no effect
                let color = &mut world.constraint_graph.colors[color_index as usize];
                clear_bit(&mut color.body_set, edge_body_a as u32);
                clear_bit(&mut color.body_set, edge_body_b as u32);
            }

            let sleep_contact_index = world.solver_sets[sleep_set_id as usize].contact_indices.len() as i32;
            world.solver_sets[sleep_set_id as usize].contact_indices.push(contact_id);

            if (flags & SIM_MESH_CONTACT) != 0 || color_index == OVERFLOW_INDEX {
                let moved_local_index = array_remove_swap(
                    &mut world.constraint_graph.colors[color_index as usize].contacts,
                    local_index,
                );
                if moved_local_index != NULL_INDEX {
                    // fix moved element
                    let moved_contact_id = world.constraint_graph.colors[color_index as usize].contacts
                        [local_index as usize]
                        .contact_id;
                    let moved_contact = &mut world.contacts[moved_contact_id as usize];
                    b3_assert!(moved_contact.local_index == moved_local_index);
                    moved_contact.local_index = local_index;
                }
            } else {
                let moved_local_index = array_remove_swap(
                    &mut world.constraint_graph.colors[color_index as usize].convex_contacts,
                    local_index,
                );
                if moved_local_index != NULL_INDEX {
                    // fix moved element
                    let moved_contact_id =
                        world.constraint_graph.colors[color_index as usize].convex_contacts[local_index as usize];
                    let moved_contact = &mut world.contacts[moved_contact_id as usize];
                    b3_assert!(moved_contact.local_index == moved_local_index);
                    moved_contact.local_index = local_index;
                }
            }

            {
                let contact = &mut world.contacts[contact_id as usize];
                contact.set_index = sleep_set_id;
                contact.color_index = NULL_INDEX;
                contact.local_index = sleep_contact_index;
            }
        }
    }

    // move joints
    // this shuffles joints in the awake set
    {
        let island_joint_count = world.islands[island_id as usize].joints.len();
        for i in 0..island_joint_count {
            let joint_id = world.islands[island_id as usize].joints[i].joint_id;

            let (color_index, local_index, edge_body_a, edge_body_b) = {
                let joint = &world.joints[joint_id as usize];
                b3_assert!(joint.set_index == AWAKE_SET);
                b3_assert!(joint.island_id == island_id);
                b3_assert!(joint.island_index == i as i32);
                (
                    joint.color_index,
                    joint.local_index,
                    joint.edges[0].body_id,
                    joint.edges[1].body_id,
                )
            };

            b3_assert!(0 <= color_index && color_index < GRAPH_COLOR_COUNT as i32);

            let awake_joint_sim =
                world.constraint_graph.colors[color_index as usize].joint_sims[local_index as usize];

            if color_index != OVERFLOW_INDEX {
                // might clear a bit for a static body, but this has no effect
                let color = &mut world.constraint_graph.colors[color_index as usize];
                clear_bit(&mut color.body_set, edge_body_a as u32);
                clear_bit(&mut color.body_set, edge_body_b as u32);
            }

            let sleep_joint_index = world.solver_sets[sleep_set_id as usize].joint_sims.len() as i32;
            world.solver_sets[sleep_set_id as usize].joint_sims.push(awake_joint_sim);

            let moved_index = array_remove_swap(
                &mut world.constraint_graph.colors[color_index as usize].joint_sims,
                local_index,
            );
            if moved_index != NULL_INDEX {
                // fix moved element
                let moved_id = world.constraint_graph.colors[color_index as usize].joint_sims
                    [local_index as usize]
                    .joint_id;
                let moved_joint = &mut world.joints[moved_id as usize];
                b3_assert!(moved_joint.local_index == moved_index);
                moved_joint.local_index = local_index;
            }

            {
                let joint = &mut world.joints[joint_id as usize];
                joint.set_index = sleep_set_id;
                joint.color_index = NULL_INDEX;
                joint.local_index = sleep_joint_index;
            }
        }
    }

    // move island struct
    {
        b3_assert!(world.islands[island_id as usize].set_index == AWAKE_SET);

        let island_index = world.islands[island_id as usize].local_index;
        world.solver_sets[sleep_set_id as usize].island_sims.push(IslandSim { island_id });

        let moved_island_index =
            array_remove_swap(&mut world.solver_sets[AWAKE_SET as usize].island_sims, island_index);
        if moved_island_index != NULL_INDEX {
            // fix index on moved element
            let moved_island_id =
                world.solver_sets[AWAKE_SET as usize].island_sims[island_index as usize].island_id;
            let moved_island = &mut world.islands[moved_island_id as usize];
            b3_assert!(moved_island.local_index == moved_island_index);
            moved_island.local_index = island_index;
        }

        let island = &mut world.islands[island_id as usize];
        island.set_index = sleep_set_id;
        island.local_index = 0;
    }

    if world.split_island_id == island_id {
        world.split_island_id = NULL_INDEX;
    }

    crate::physics_world::validate_solver_sets(world);
}

// This is called when joints are created between sets. I want to allow the sets
// to continue sleeping if both are asleep. Otherwise one set is waked.
// Islands will get merged when the set is woken.
pub fn merge_solver_sets(world: &mut World, set_id1: i32, set_id2: i32) {
    b3_assert!(set_id1 >= FIRST_SLEEPING_SET);
    b3_assert!(set_id2 >= FIRST_SLEEPING_SET);

    // Move the fewest number of bodies
    let (set_id1, set_id2) = {
        let count1 = world.solver_sets[set_id1 as usize].body_sims.len();
        let count2 = world.solver_sets[set_id2 as usize].body_sims.len();
        if count1 < count2 {
            (set_id2, set_id1)
        } else {
            (set_id1, set_id2)
        }
    };

    // transfer bodies
    {
        let body_count = world.solver_sets[set_id2 as usize].body_sims.len();
        for i in 0..body_count {
            let sim_src = world.solver_sets[set_id2 as usize].body_sims[i];

            let count1 = world.solver_sets[set_id1 as usize].body_sims.len() as i32;
            {
                let body = &mut world.bodies[sim_src.body_id as usize];
                b3_assert!(body.set_index == set_id2);
                body.set_index = set_id1;
                body.local_index = count1;
            }
            world.solver_sets[set_id1 as usize].body_sims.push(sim_src);
        }
    }

    // transfer contacts
    {
        let contact_count = world.solver_sets[set_id2 as usize].contact_indices.len();
        for i in 0..contact_count {
            let contact_index = world.solver_sets[set_id2 as usize].contact_indices[i];

            let count1 = world.solver_sets[set_id1 as usize].contact_indices.len() as i32;
            {
                let contact = &mut world.contacts[contact_index as usize];
                b3_assert!(contact.set_index == set_id2);
                contact.set_index = set_id1;
                contact.local_index = count1;
            }
            world.solver_sets[set_id1 as usize].contact_indices.push(contact_index);
        }
    }

    // transfer joints
    {
        let joint_count = world.solver_sets[set_id2 as usize].joint_sims.len();
        for i in 0..joint_count {
            let joint_src = world.solver_sets[set_id2 as usize].joint_sims[i];

            let count1 = world.solver_sets[set_id1 as usize].joint_sims.len() as i32;
            {
                let joint = &mut world.joints[joint_src.joint_id as usize];
                b3_assert!(joint.set_index == set_id2);
                joint.set_index = set_id1;
                joint.local_index = count1;
            }
            world.solver_sets[set_id1 as usize].joint_sims.push(joint_src);
        }
    }

    // transfer islands
    {
        let island_count = world.solver_sets[set_id2 as usize].island_sims.len();
        for i in 0..island_count {
            let island_src = world.solver_sets[set_id2 as usize].island_sims[i];
            let island_id = island_src.island_id;

            let count1 = world.solver_sets[set_id1 as usize].island_sims.len() as i32;
            {
                let island = &mut world.islands[island_id as usize];
                island.set_index = set_id1;
                island.local_index = count1;
            }
            world.solver_sets[set_id1 as usize].island_sims.push(island_src);
        }
    }

    // destroy the merged set
    // Warning: need to be careful not to destroy things that got transferred, like triangle caches.
    destroy_solver_set(world, set_id2);

    crate::physics_world::validate_solver_sets(world);
}

/// C signature: b3TransferBody(world, b3SolverSet* targetSet, b3SolverSet* sourceSet, b3Body* body).
/// The port passes set indices and the body id.
pub fn transfer_body(world: &mut World, target_set_index: i32, source_set_index: i32, body_id: i32) {
    if target_set_index == source_set_index {
        return;
    }

    let source_index = world.bodies[body_id as usize].local_index;

    let mut sim = world.solver_sets[source_set_index as usize].body_sims[source_index as usize];

    // Clear transient body flags
    sim.flags &= !(IS_FAST | IS_SPEED_CAPPED | HAD_TIME_OF_IMPACT);

    let target_index = world.solver_sets[target_set_index as usize].body_sims.len() as i32;
    world.solver_sets[target_set_index as usize].body_sims.push(sim);

    // Remove body sim from solver set that owns it
    let moved_index = array_remove_swap(
        &mut world.solver_sets[source_set_index as usize].body_sims,
        source_index,
    );
    if moved_index != NULL_INDEX {
        // Fix moved body index
        let moved_id = world.solver_sets[source_set_index as usize].body_sims[source_index as usize].body_id;
        let moved_body = &mut world.bodies[moved_id as usize];
        b3_assert!(moved_body.local_index == moved_index);
        moved_body.local_index = source_index;
    }

    if source_set_index == AWAKE_SET {
        array_remove_swap(
            &mut world.solver_sets[source_set_index as usize].body_states,
            source_index,
        );
    } else if target_set_index == AWAKE_SET {
        let mut state = IDENTITY_BODY_STATE;
        state.flags = world.bodies[body_id as usize].flags;
        world.solver_sets[target_set_index as usize].body_states.push(state);
    }

    let body = &mut world.bodies[body_id as usize];
    body.set_index = target_set_index;
    body.local_index = target_index;
}

/// C signature: b3TransferJoint(world, b3SolverSet* targetSet, b3SolverSet* sourceSet, b3Joint* joint).
/// The port passes set indices and the joint id.
pub fn transfer_joint(world: &mut World, target_set_index: i32, source_set_index: i32, joint_id: i32) {
    if target_set_index == source_set_index {
        return;
    }

    let (local_index, color_index, edge_body_a, edge_body_b) = {
        let joint = &world.joints[joint_id as usize];
        (
            joint.local_index,
            joint.color_index,
            joint.edges[0].body_id,
            joint.edges[1].body_id,
        )
    };

    // Retrieve source (JointSim is Copy; C keeps a pointer).
    let source_sim = if source_set_index == AWAKE_SET {
        b3_assert!(0 <= color_index && color_index < GRAPH_COLOR_COUNT as i32);
        world.constraint_graph.colors[color_index as usize].joint_sims[local_index as usize]
    } else {
        b3_assert!(color_index == NULL_INDEX);
        world.solver_sets[source_set_index as usize].joint_sims[local_index as usize]
    };

    // Create target and copy. Fix joint.
    if target_set_index == AWAKE_SET {
        crate::constraint_graph::add_joint_to_graph(world, source_sim, joint_id);
        world.joints[joint_id as usize].set_index = AWAKE_SET;
    } else {
        let target_count = world.solver_sets[target_set_index as usize].joint_sims.len() as i32;
        {
            let joint = &mut world.joints[joint_id as usize];
            joint.set_index = target_set_index;
            joint.local_index = target_count;
            joint.color_index = NULL_INDEX;
        }
        world.solver_sets[target_set_index as usize].joint_sims.push(source_sim);
    }

    // Destroy source.
    if source_set_index == AWAKE_SET {
        crate::constraint_graph::remove_joint_from_graph(world, edge_body_a, edge_body_b, color_index, local_index);
    } else {
        let moved_index = array_remove_swap(
            &mut world.solver_sets[source_set_index as usize].joint_sims,
            local_index,
        );
        if moved_index != NULL_INDEX {
            // fix swapped element
            let moved_id =
                world.solver_sets[source_set_index as usize].joint_sims[local_index as usize].joint_id;
            let moved_joint = &mut world.joints[moved_id as usize];
            moved_joint.local_index = local_index;
        }
    }
}
