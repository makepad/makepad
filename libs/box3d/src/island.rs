// Port of box3d/src/island.h + island.c

use crate::b3_assert;
use crate::b3_validate;
use crate::container::array_remove_swap;
use crate::core::NULL_INDEX;
use crate::id_pool::{alloc_id, free_id};
use crate::physics_world::{World, AWAKE_SET, DISABLED_SET, FIRST_SLEEPING_SET, STATIC_SET};

/// Cached contact data stored in the island for fast contiguous iteration.
/// Avoids touching Contact during union-find in split_island.
#[derive(Clone, Copy, Debug, Default)]
pub struct ContactLink {
    pub contact_id: i32,
    pub body_id_a: i32,
    pub body_id_b: i32,
}

/// Cached joint data stored in the island for fast contiguous iteration.
#[derive(Clone, Copy, Debug, Default)]
pub struct JointLink {
    pub joint_id: i32,
    pub body_id_a: i32,
    pub body_id_b: i32,
}

/// Persistent island for awake bodies, joints, and contacts.
/// Contacts are touching. Contacts and joints may connect to static bodies,
/// but static bodies are not in the island.
#[derive(Clone, Debug, Default)]
pub struct Island {
    /// index of solver set stored in World. May be NULL_INDEX.
    pub set_index: i32,

    /// island index within set. May be NULL_INDEX.
    pub local_index: i32,

    pub island_id: i32,

    /// Keeps track of how many contacts have been removed from this island.
    /// This is used to determine if an island is a candidate for splitting.
    pub constraint_remove_count: i32,

    pub bodies: Vec<i32>,

    /// Contacts and joints that belong to this island. May connect to static
    /// bodies not in the island.
    pub contacts: Vec<ContactLink>,
    pub joints: Vec<JointLink>,
}

/// This is used to move islands across solver sets.
#[derive(Clone, Copy, Debug, Default)]
pub struct IslandSim {
    pub island_id: i32,
}

// ---------------------------------------------------------------------------
// island.c
// ---------------------------------------------------------------------------

/// C returns b3Island*; the port returns the island id.
pub fn create_island(world: &mut World, set_index: i32) -> i32 {
    b3_assert!(set_index == AWAKE_SET || set_index >= FIRST_SLEEPING_SET);

    let island_id = alloc_id(&mut world.island_id_pool);

    if island_id == world.islands.len() as i32 {
        world.islands.push(Island::default());
    } else {
        b3_assert!(world.islands[island_id as usize].set_index == NULL_INDEX);
    }

    let local_index = world.solver_sets[set_index as usize].island_sims.len() as i32;

    {
        let island = &mut world.islands[island_id as usize];
        island.set_index = set_index;
        island.local_index = local_index;
        island.island_id = island_id;
        island.bodies = Vec::new();
        island.contacts = Vec::new();
        island.joints = Vec::new();
        island.constraint_remove_count = 0;
    }

    world.solver_sets[set_index as usize].island_sims.push(IslandSim { island_id });

    island_id
}

pub fn destroy_island(world: &mut World, island_id: i32) {
    if world.split_island_id == island_id {
        world.split_island_id = NULL_INDEX;
    }

    // assume island is empty
    let (set_index, local_index) = {
        let island = &world.islands[island_id as usize];
        (island.set_index, island.local_index)
    };
    let move_island_id;
    {
        let set = &mut world.solver_sets[set_index as usize];
        let last_index = set.island_sims.len() as i32 - 1;
        b3_assert!(0 <= local_index && local_index <= last_index);
        move_island_id = set.island_sims[last_index as usize].island_id;
        set.island_sims[local_index as usize] = set.island_sims[last_index as usize];
        set.island_sims.pop();
    }
    world.islands[move_island_id as usize].local_index = local_index;

    // Free island and id (preserve island generation)
    {
        let island = &mut world.islands[island_id as usize];
        island.bodies = Vec::new();
        island.contacts = Vec::new();
        island.joints = Vec::new();
        island.constraint_remove_count = 0;
        island.local_index = NULL_INDEX;
        island.island_id = NULL_INDEX;
        island.set_index = NULL_INDEX;
    }

    free_id(&mut world.island_id_pool, island_id);
}

fn merge_islands(world: &mut World, island_id_a: i32, island_id_b: i32) -> i32 {
    if island_id_a == island_id_b {
        return island_id_a;
    }

    if island_id_a == NULL_INDEX {
        b3_assert!(island_id_b != NULL_INDEX);
        return island_id_b;
    }

    if island_id_b == NULL_INDEX {
        b3_assert!(island_id_a != NULL_INDEX);
        return island_id_a;
    }

    // Keep the biggest island to reduce cache misses
    let (big_island_id, small_island_id) = {
        let island_a = &world.islands[island_id_a as usize];
        let island_b = &world.islands[island_id_b as usize];
        if island_a.bodies.len() >= island_b.bodies.len() {
            (island_id_a, island_id_b)
        } else {
            (island_id_b, island_id_a)
        }
    };

    // Detach the small island's arrays (C keeps pointers; the arrays are not
    // mutated while elements migrate).
    let small_bodies = std::mem::take(&mut world.islands[small_island_id as usize].bodies);
    let small_contacts = std::mem::take(&mut world.islands[small_island_id as usize].contacts);
    let small_joints = std::mem::take(&mut world.islands[small_island_id as usize].joints);
    let small_remove_count = world.islands[small_island_id as usize].constraint_remove_count;

    world.islands[big_island_id as usize].bodies.reserve(small_bodies.len());

    // Move bodies from smaller island to larger island
    for &body_id in &small_bodies {
        let big_count = world.islands[big_island_id as usize].bodies.len() as i32;
        {
            let body = &mut world.bodies[body_id as usize];
            b3_validate!(body.island_id == small_island_id);
            body.island_id = big_island_id;
            body.island_index = big_count;
        }
        world.islands[big_island_id as usize].bodies.push(body_id);
    }

    // Migrate contacts from smaller island to larger island
    if !small_contacts.is_empty() {
        world.islands[big_island_id as usize].contacts.reserve(small_contacts.len());

        for link in &small_contacts {
            let big_count = world.islands[big_island_id as usize].contacts.len() as i32;
            {
                let contact = &mut world.contacts[link.contact_id as usize];
                contact.island_id = big_island_id;
                contact.island_index = big_count;
            }
            world.islands[big_island_id as usize].contacts.push(*link);
        }
    }

    // Migrate joints from smaller island to larger island
    if !small_joints.is_empty() {
        world.islands[big_island_id as usize].joints.reserve(small_joints.len());

        for link in &small_joints {
            let big_count = world.islands[big_island_id as usize].joints.len() as i32;
            {
                let joint = &mut world.joints[link.joint_id as usize];
                joint.island_id = big_island_id;
                joint.island_index = big_count;
            }
            world.islands[big_island_id as usize].joints.push(*link);
        }
    }

    // Track removed constraints
    world.islands[big_island_id as usize].constraint_remove_count += small_remove_count;

    destroy_island(world, small_island_id);

    validate_island(world, big_island_id);

    big_island_id
}

fn add_contact_to_island(world: &mut World, island_id: i32, contact_id: i32) {
    {
        let contact = &world.contacts[contact_id as usize];
        b3_assert!(contact.island_id == NULL_INDEX);
        b3_assert!(contact.island_index == NULL_INDEX);
    }

    let island_count = world.islands[island_id as usize].contacts.len() as i32;

    let link;
    {
        let contact = &mut world.contacts[contact_id as usize];
        contact.island_id = island_id;
        contact.island_index = island_count;

        link = ContactLink {
            contact_id: contact.contact_id,
            body_id_a: contact.edges[0].body_id,
            body_id_b: contact.edges[1].body_id,
        };
    }
    world.islands[island_id as usize].contacts.push(link);

    validate_island(world, island_id);
}

/// Link a contact into an island.
pub fn link_contact(world: &mut World, contact_id: i32) {
    let (body_id_a, body_id_b) = {
        let contact = &world.contacts[contact_id as usize];
        b3_assert!((contact.flags & crate::contact::CONTACT_TOUCHING_FLAG) != 0);
        (contact.edges[0].body_id, contact.edges[1].body_id)
    };

    {
        let set_a = world.bodies[body_id_a as usize].set_index;
        let set_b = world.bodies[body_id_b as usize].set_index;
        b3_assert!(set_a != DISABLED_SET && set_b != DISABLED_SET);
        b3_assert!(set_a != STATIC_SET || set_b != STATIC_SET);

        // Wake bodyB if bodyA is awake and bodyB is sleeping
        if set_a == AWAKE_SET && set_b >= FIRST_SLEEPING_SET {
            crate::solver_set::wake_solver_set(world, set_b);
        }
    }

    {
        // Re-read: the wake above may have changed set indices.
        let set_a = world.bodies[body_id_a as usize].set_index;
        let set_b = world.bodies[body_id_b as usize].set_index;

        // Wake bodyA if bodyB is awake and bodyA is sleeping
        if set_b == AWAKE_SET && set_a >= FIRST_SLEEPING_SET {
            crate::solver_set::wake_solver_set(world, set_a);
        }
    }

    let island_id_a = world.bodies[body_id_a as usize].island_id;
    let island_id_b = world.bodies[body_id_b as usize].island_id;

    // Static bodies have null island indices.
    b3_assert!(world.bodies[body_id_a as usize].set_index != STATIC_SET || island_id_a == NULL_INDEX);
    b3_assert!(world.bodies[body_id_b as usize].set_index != STATIC_SET || island_id_b == NULL_INDEX);
    b3_assert!(island_id_a != NULL_INDEX || island_id_b != NULL_INDEX);

    // Merge islands. This will destroy one of the islands.
    let final_island_id = merge_islands(world, island_id_a, island_id_b);

    // Add contact to the island that survived
    add_contact_to_island(world, final_island_id, contact_id);
}

/// This is called when a contact no longer has contact points or when a contact is destroyed.
pub fn unlink_contact(world: &mut World, contact_id: i32) {
    let (island_id, remove_index, self_contact_id) = {
        let contact = &world.contacts[contact_id as usize];
        b3_assert!(contact.island_id != NULL_INDEX);
        (contact.island_id, contact.island_index, contact.contact_id)
    };

    // remove from island
    let moved_index;
    {
        let island = &mut world.islands[island_id as usize];
        b3_assert!(0 <= remove_index && (remove_index as usize) < island.contacts.len());
        b3_assert!(island.contacts[remove_index as usize].contact_id == self_contact_id);
        moved_index = array_remove_swap(&mut island.contacts, remove_index);
    }
    if moved_index != NULL_INDEX {
        // Fix islandIndex on the contact that was swapped into removeIndex
        let moved_contact_id = world.islands[island_id as usize].contacts[remove_index as usize].contact_id;
        let moved_contact = &mut world.contacts[moved_contact_id as usize];
        b3_assert!(moved_contact.island_index == moved_index);
        moved_contact.island_index = remove_index;
    }

    {
        let contact = &mut world.contacts[contact_id as usize];
        contact.island_id = NULL_INDEX;
        contact.island_index = NULL_INDEX;
    }
    world.islands[island_id as usize].constraint_remove_count += 1;

    validate_island(world, island_id);
}

fn add_joint_to_island(world: &mut World, island_id: i32, joint_id: i32) {
    {
        let joint = &world.joints[joint_id as usize];
        b3_assert!(joint.island_id == NULL_INDEX);
        b3_assert!(joint.island_index == NULL_INDEX);
    }

    let island_count = world.islands[island_id as usize].joints.len() as i32;

    let link;
    {
        let joint = &mut world.joints[joint_id as usize];
        joint.island_id = island_id;
        joint.island_index = island_count;

        link = JointLink {
            joint_id: joint.joint_id,
            body_id_a: joint.edges[0].body_id,
            body_id_b: joint.edges[1].body_id,
        };
    }
    world.islands[island_id as usize].joints.push(link);

    validate_island(world, island_id);
}

pub fn link_joint(world: &mut World, joint_id: i32) {
    let (body_id_a, body_id_b) = {
        let joint = &world.joints[joint_id as usize];
        (joint.edges[0].body_id, joint.edges[1].body_id)
    };

    {
        let body_a = &world.bodies[body_id_a as usize];
        let body_b = &world.bodies[body_id_b as usize];
        b3_assert!(
            body_a.body_type == crate::types::BodyType::Dynamic
                || body_b.body_type == crate::types::BodyType::Dynamic
        );
    }

    let set_a = world.bodies[body_id_a as usize].set_index;
    let set_b = world.bodies[body_id_b as usize].set_index;

    if set_a == AWAKE_SET && set_b >= FIRST_SLEEPING_SET {
        crate::solver_set::wake_solver_set(world, set_b);
    } else if set_b == AWAKE_SET && set_a >= FIRST_SLEEPING_SET {
        crate::solver_set::wake_solver_set(world, set_a);
    }

    let island_id_a = world.bodies[body_id_a as usize].island_id;
    let island_id_b = world.bodies[body_id_b as usize].island_id;

    b3_assert!(island_id_a != NULL_INDEX || island_id_b != NULL_INDEX);

    // Merge islands. This will destroy one of the islands.
    let final_island_id = merge_islands(world, island_id_a, island_id_b);

    // Add joint to the island that survived
    add_joint_to_island(world, final_island_id, joint_id);
}

pub fn unlink_joint(world: &mut World, joint_id: i32) {
    let (island_id, remove_index, self_joint_id) = {
        let joint = &world.joints[joint_id as usize];
        if joint.island_id == NULL_INDEX {
            return;
        }
        (joint.island_id, joint.island_index, joint.joint_id)
    };

    // remove from island
    let moved_index;
    {
        let island = &mut world.islands[island_id as usize];
        b3_assert!(0 <= remove_index && (remove_index as usize) < island.joints.len());
        b3_assert!(island.joints[remove_index as usize].joint_id == self_joint_id);
        moved_index = array_remove_swap(&mut island.joints, remove_index);
    }
    if moved_index != NULL_INDEX {
        // Fix islandIndex on the joint that was swapped into removeIndex
        let moved_joint_id = world.islands[island_id as usize].joints[remove_index as usize].joint_id;
        let moved_joint = &mut world.joints[moved_joint_id as usize];
        b3_assert!(moved_joint.island_index == moved_index);
        moved_joint.island_index = remove_index;
    }

    {
        let joint = &mut world.joints[joint_id as usize];
        joint.island_id = NULL_INDEX;
        joint.island_index = NULL_INDEX;
    }
    world.islands[island_id as usize].constraint_remove_count += 1;

    validate_island(world, island_id);
}

// Find parent of a node. Use path halving to speed up further queries.
#[inline]
fn island_find_parent(parents: &mut [i32], mut node: i32) -> i32 {
    // Walk the chain of parents to find the node that is its own parent (the root)
    while parents[node as usize] != node {
        let grand_parent = parents[parents[node as usize] as usize];
        parents[node as usize] = grand_parent;
        node = grand_parent;
    }

    node
}

// Connect the components containing node1 and node2.
// Uses rank to keep tree balanced. Tracks per-component contact and joint counts.
#[inline]
fn island_union(
    parents: &mut [i32],
    ranks: &mut [i32],
    node1: i32,
    node2: i32,
    contact_counts: &mut [i32],
    joint_counts: &mut [i32],
) {
    let root1 = island_find_parent(parents, node1);
    let root2 = island_find_parent(parents, node2);
    if root1 != root2 {
        if ranks[root1 as usize] < ranks[root2 as usize] {
            parents[root1 as usize] = root2;
            contact_counts[root2 as usize] += contact_counts[root1 as usize];
            joint_counts[root2 as usize] += joint_counts[root1 as usize];
        } else if ranks[root1 as usize] > ranks[root2 as usize] {
            parents[root2 as usize] = root1;
            contact_counts[root1 as usize] += contact_counts[root2 as usize];
            joint_counts[root1 as usize] += joint_counts[root2 as usize];
        } else {
            parents[root2 as usize] = root1;
            ranks[root1 as usize] += 1;
            contact_counts[root1 as usize] += contact_counts[root2 as usize];
            joint_counts[root1 as usize] += joint_counts[root2 as usize];
        }
    }
}

// This uses union-find.
// https://en.wikipedia.org/wiki/Disjoint-set_data_structure
pub fn split_island(world: &mut World, base_id: i32) {
    {
        let base_island = &world.islands[base_id as usize];
        b3_assert!(base_island.constraint_remove_count > 0);
        b3_assert!(base_island.set_index == AWAKE_SET);
    }

    validate_island(world, base_id);

    let base_body_count = world.islands[base_id as usize].bodies.len() as i32;
    let base_contact_count = world.islands[base_id as usize].contacts.len() as i32;
    let base_joint_count = world.islands[base_id as usize].joints.len() as i32;

    // The C version uses the world arena; plain Vecs here.
    let mut parents: Vec<i32> = Vec::with_capacity(base_body_count as usize);
    let mut ranks: Vec<i32> = Vec::with_capacity(base_body_count as usize);
    let mut contact_counts: Vec<i32> = Vec::with_capacity(base_body_count as usize);
    let mut joint_counts: Vec<i32> = Vec::with_capacity(base_body_count as usize);
    for i in 0..base_body_count {
        parents.push(i);
        ranks.push(0);
        contact_counts.push(0);
        joint_counts.push(0);
    }

    // Union over contacts, tracking per-component contact counts
    for i in 0..base_contact_count {
        let link = world.islands[base_id as usize].contacts[i as usize];
        let body_id_a = link.body_id_a;
        let body_id_b = link.body_id_b;
        b3_validate!(0 <= body_id_a && (body_id_a as usize) < world.bodies.len());
        b3_validate!(0 <= body_id_b && (body_id_b as usize) < world.bodies.len());
        let island_index_a = world.bodies[body_id_a as usize].island_index;
        let island_index_b = world.bodies[body_id_b as usize].island_index;

        // Only connect non-static bodies
        if island_index_a != NULL_INDEX && island_index_b != NULL_INDEX {
            b3_validate!(0 <= island_index_a && island_index_a < base_body_count);
            b3_validate!(0 <= island_index_b && island_index_b < base_body_count);
            island_union(
                &mut parents,
                &mut ranks,
                island_index_a,
                island_index_b,
                &mut contact_counts,
                &mut joint_counts,
            );
            let root = island_find_parent(&mut parents, island_index_a);
            contact_counts[root as usize] += 1;
        } else {
            let island_index = if island_index_a != NULL_INDEX { island_index_a } else { island_index_b };
            let root = island_find_parent(&mut parents, island_index);
            contact_counts[root as usize] += 1;
        }
    }

    // Union over joints, tracking per-component joint counts
    for i in 0..base_joint_count {
        let link = world.islands[base_id as usize].joints[i as usize];
        let body_id_a = link.body_id_a;
        let body_id_b = link.body_id_b;
        b3_validate!(0 <= body_id_a && (body_id_a as usize) < world.bodies.len());
        b3_validate!(0 <= body_id_b && (body_id_b as usize) < world.bodies.len());
        let island_index_a = world.bodies[body_id_a as usize].island_index;
        let island_index_b = world.bodies[body_id_b as usize].island_index;

        // Only connect non-static bodies
        if island_index_a != NULL_INDEX && island_index_b != NULL_INDEX {
            b3_validate!(0 <= island_index_a && island_index_a < base_body_count);
            b3_validate!(0 <= island_index_b && island_index_b < base_body_count);
            island_union(
                &mut parents,
                &mut ranks,
                island_index_a,
                island_index_b,
                &mut contact_counts,
                &mut joint_counts,
            );
            let root = island_find_parent(&mut parents, island_index_a);
            joint_counts[root as usize] += 1;
        } else {
            let island_index = if island_index_a != NULL_INDEX { island_index_a } else { island_index_b };
            let root = island_find_parent(&mut parents, island_index);
            joint_counts[root as usize] += 1;
        }
    }

    // Done with ranks
    drop(ranks);

    // Flatten all parent indices and count connected components.
    let mut component_count = 0;
    for i in 0..base_body_count {
        parents[i as usize] = island_find_parent(&mut parents, i);
        if parents[i as usize] == i {
            component_count += 1;
        }
    }

    // Early return — island is still fully connected, no split needed.
    if component_count == 1 {
        world.islands[base_id as usize].constraint_remove_count = 0;
        return;
    }

    // Detach body/contact/joint arrays from base island so destroy_island won't free them
    let base_body_ids = std::mem::take(&mut world.islands[base_id as usize].bodies);
    let base_contacts = std::mem::take(&mut world.islands[base_id as usize].contacts);
    let base_joints = std::mem::take(&mut world.islands[base_id as usize].joints);

    // Map from body index to new island index. Only set for root bodies.
    let mut root_map = vec![NULL_INDEX; base_body_count as usize];

    let mut component_body_counts = vec![0i32; component_count as usize];
    let mut component_contact_counts = vec![0i32; component_count as usize];
    let mut component_joint_counts = vec![0i32; component_count as usize];
    let mut island_count: i32 = 0;

    // Find the root body for each body and create islands as needed.
    // Extract per-component counts from the root nodes' accumulated counts.
    for i in 0..base_body_count {
        let root_index = parents[i as usize];
        if root_map[root_index as usize] == NULL_INDEX {
            root_map[root_index as usize] = island_count;
            component_body_counts[island_count as usize] = 0;
            component_contact_counts[island_count as usize] = contact_counts[root_index as usize];
            component_joint_counts[island_count as usize] = joint_counts[root_index as usize];
            island_count += 1;
        }

        component_body_counts[root_map[root_index as usize] as usize] += 1;
    }

    b3_assert!(island_count == component_count);

    // Map from new island index to island id
    let mut island_ids: Vec<i32> = Vec::with_capacity(island_count as usize);

    // Create new islands and reserve body/contact/joint arrays
    for i in 0..island_count {
        let new_island_id = create_island(world, AWAKE_SET);
        island_ids.push(new_island_id);

        // Reserve arrays to avoid wasteful growth and memcpy.
        let new_island = &mut world.islands[new_island_id as usize];
        new_island.bodies.reserve(component_body_counts[i as usize] as usize);
        new_island.contacts.reserve(component_contact_counts[i as usize] as usize);
        new_island.joints.reserve(component_joint_counts[i as usize] as usize);
    }

    // Assign bodies to new islands
    for i in 0..base_body_count {
        let body_id = base_body_ids[i as usize];
        let root = island_find_parent(&mut parents, i);
        let new_island_id = island_ids[root_map[root as usize] as usize];

        let new_count = world.islands[new_island_id as usize].bodies.len() as i32;
        {
            let body = &mut world.bodies[body_id as usize];
            body.island_id = new_island_id;
            body.island_index = new_count;
        }

        // Ensure the array has the correct capacity
        b3_validate!(
            world.islands[new_island_id as usize].bodies.len()
                < world.islands[new_island_id as usize].bodies.capacity()
        );
        world.islands[new_island_id as usize].bodies.push(body_id);
    }

    // Assign contacts to the island of their bodies
    for i in 0..base_contact_count {
        let link = base_contacts[i as usize];

        // Static bodies don't have an island id.
        let island_id_a = world.bodies[link.body_id_a as usize].island_id;
        let island_id_b = world.bodies[link.body_id_b as usize].island_id;
        let target_island_id = if island_id_a != NULL_INDEX { island_id_a } else { island_id_b };

        let target_count = world.islands[target_island_id as usize].contacts.len() as i32;
        {
            let contact = &mut world.contacts[link.contact_id as usize];
            contact.island_id = target_island_id;
            contact.island_index = target_count;
        }

        // Ensure the array has the correct capacity
        b3_validate!(
            world.islands[target_island_id as usize].contacts.len()
                < world.islands[target_island_id as usize].contacts.capacity()
        );
        world.islands[target_island_id as usize].contacts.push(link);
    }

    // Assign joints to the island of their bodies
    for i in 0..base_joint_count {
        let link = base_joints[i as usize];

        // Static bodies don't have an island id.
        let island_id_a = world.bodies[link.body_id_a as usize].island_id;
        let island_id_b = world.bodies[link.body_id_b as usize].island_id;
        let target_island_id = if island_id_a != NULL_INDEX { island_id_a } else { island_id_b };

        let target_count = world.islands[target_island_id as usize].joints.len() as i32;
        {
            let joint = &mut world.joints[link.joint_id as usize];
            joint.island_id = target_island_id;
            joint.island_index = target_count;
        }

        // Ensure the array has the correct capacity
        b3_validate!(
            world.islands[target_island_id as usize].joints.len()
                < world.islands[target_island_id as usize].joints.capacity()
        );
        world.islands[target_island_id as usize].joints.push(link);
    }

    // Destroy the base island
    destroy_island(world, base_id);

    // The detached arrays drop here (C frees them manually).
}

// Split an island because some contacts and/or joints have been removed.
// This is called during the constraint solve while islands are not being touched.
// The C task wrapper (b3SplitIslandTask) is folded into a direct call.
pub fn split_island_task(world: &mut World) {
    let ticks = crate::timer::get_ticks();

    b3_assert!(world.split_island_id != NULL_INDEX);

    split_island(world, world.split_island_id);

    world.split_island_id = NULL_INDEX;
    world.profile.split_islands += crate::timer::get_milliseconds(ticks);
}

pub fn validate_island(world: &World, island_id: i32) {
    #[cfg(debug_assertions)]
    {
        use crate::id_pool::get_id_count;

        if island_id == NULL_INDEX {
            return;
        }

        let island = &world.islands[island_id as usize];
        b3_assert!(island.island_id == island_id);
        b3_assert!(island.set_index != NULL_INDEX);

        {
            b3_assert!(!island.bodies.is_empty());
            b3_assert!(island.bodies.len() as i32 <= get_id_count(&world.body_id_pool));

            for (i, &body_id) in island.bodies.iter().enumerate() {
                let body = &world.bodies[body_id as usize];
                b3_assert!(body.island_id == island_id);
                b3_assert!(body.island_index == i as i32);
                b3_assert!(body.set_index == island.set_index);
            }
        }

        if !island.contacts.is_empty() {
            b3_assert!(island.contacts.len() as i32 <= get_id_count(&world.contact_id_pool));

            for (i, link) in island.contacts.iter().enumerate() {
                let contact = &world.contacts[link.contact_id as usize];
                b3_assert!(contact.set_index == island.set_index);
                b3_assert!(contact.island_id == island_id);
                b3_assert!(contact.island_index == i as i32);
            }
        }

        if !island.joints.is_empty() {
            b3_assert!(island.joints.len() as i32 <= get_id_count(&world.joint_id_pool));

            for (i, link) in island.joints.iter().enumerate() {
                let joint = &world.joints[link.joint_id as usize];
                b3_assert!(joint.set_index == island.set_index);
                b3_assert!(joint.island_id == island_id);
                b3_assert!(joint.island_index == i as i32);
            }
        }
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = (world, island_id);
    }
}
