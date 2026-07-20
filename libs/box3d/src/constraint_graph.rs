// Port of box3d/src/constraint_graph.h (+ constraint_graph.c functions to be ported below).

use crate::bitset::BitSet;
use crate::constants::GRAPH_COLOR_COUNT;
use crate::contact::ContactSpec;
use crate::joint::JointSim;

/// This holds constraints that cannot fit the graph color limit. This happens when a
/// single dynamic body is touching many other bodies.
pub const OVERFLOW_INDEX: i32 = GRAPH_COLOR_COUNT as i32 - 1;

/// This keeps constraints involving two dynamic bodies at a lower solver priority than
/// constraints involving a dynamic and static bodies. This reduces tunneling due to
/// push through.
pub const DYNAMIC_COLOR_COUNT: i32 = GRAPH_COLOR_COUNT as i32 - 4;

/// Pointer redesign (see solver.rs header): the C per-color constraint pointers
/// (wideConstraints, manifoldConstraints, contactConstraints — slices of arena
/// arrays owned by the step context) become (start, count) ranges into the
/// StepContext-owned arrays.
#[derive(Clone, Debug, Default)]
pub struct GraphColor {
    /// This bitset is indexed by bodyId so this is over-sized to encompass static
    /// bodies. This bitset is unused on the overflow color.
    pub body_set: BitSet,

    /// cache friendly arrays
    pub joint_sims: Vec<JointSim>,

    pub convex_contacts: Vec<i32>,
    pub contacts: Vec<ContactSpec>,

    /// Range into StepContext::wide_constraints (convex contacts).
    pub wide_constraint_start: i32,
    pub wide_constraint_count: i32,

    /// Range into StepContext::manifold_constraints (mesh and overflow contacts).
    pub manifold_constraint_start: i32,
    pub manifold_constraint_count: i32,

    /// Range into StepContext::contact_constraints (mesh and overflow contacts).
    pub contact_constraint_start: i32,
    pub contact_constraint_count: i32,
}

#[derive(Debug, Default)]
pub struct ConstraintGraph {
    /// including overflow at the end
    pub colors: Vec<GraphColor>,
}

// ---------------------------------------------------------------------------
// Port of constraint_graph.c
// ---------------------------------------------------------------------------
//
// Solver using graph coloring. Islands are only used for sleep.
// High-Performance Physical Simulations on Next-Generation Architecture with Many Cores
// http://web.eecs.umich.edu/~msmelyan/papers/physsim_onmanycore_itj.pdf
//
// Kinematic bodies have to be treated like dynamic bodies in graph coloring.

use crate::b3_assert;
use crate::bitset::{clear_bit, create_bit_set, destroy_bit_set, get_bit, set_bit_count_and_clear, set_bit_grow};
use crate::container::array_remove_swap;
use crate::contact::{CONTACT_TOUCHING_FLAG, SIM_MESH_CONTACT};
use crate::core::NULL_INDEX;
use crate::math_functions::max_int;
use crate::physics_world::{HexColor, World, AWAKE_SET};
use crate::types::BodyType;

// This is used for debugging by making all constraints be assigned to overflow.
const FORCE_OVERFLOW: bool = false;

const _: () = assert!(GRAPH_COLOR_COUNT >= 2, "must have at least two constraint graph colors");
const _: () = assert!(OVERFLOW_INDEX == GRAPH_COLOR_COUNT as i32 - 1, "bad overflow index");

// Values from the b3HexColor enum in types.h.
static GRAPH_COLORS: [HexColor; GRAPH_COLOR_COUNT] = [
    0xFF0000, // red
    0xFFA500, // orange
    0xFFFF00, // yellow
    0x32CD32, // lime green
    0x00FF7F, // spring green
    0x00FFFF, // aqua
    0x1E90FF, // dodger blue
    0x8A2BE2, // blue violet
    0xFF00FF, // magenta
    0xFF1493, // deep pink
    0xDC143C, // crimson
    0xFF7F50, // coral
    0xFFD700, // gold
    0xADFF2F, // green yellow
    0x3CB371, // medium sea green
    0x40E0D0, // turquoise
    0x00BFFF, // deep sky blue
    0x6495ED, // cornflower blue
    0x7B68EE, // medium slate blue
    0xBA55D3, // medium orchid
    0xFF69B4, // hot pink
    0xFF6347, // tomato
    0xF0E68C, // khaki
    0xC0C0C0, // silver
];

pub fn get_graph_color(index: i32) -> HexColor {
    b3_assert!(0 <= index && index < GRAPH_COLOR_COUNT as i32);
    GRAPH_COLORS[index as usize]
}

/// C: b3CreateGraph(graph, bodyCapacity) fills in place; the port constructs.
pub fn create_graph(body_capacity: i32) -> ConstraintGraph {
    let body_capacity = max_int(body_capacity, 8);

    let mut graph = ConstraintGraph {
        colors: (0..GRAPH_COLOR_COUNT).map(|_| GraphColor::default()).collect(),
    };

    // Initialize graph color bit set.
    // No bitset for overflow color.
    for i in 0..OVERFLOW_INDEX as usize {
        let color = &mut graph.colors[i];
        color.body_set = create_bit_set(body_capacity as u32);
        set_bit_count_and_clear(&mut color.body_set, body_capacity as u32);
    }

    graph
}

pub fn destroy_graph(graph: &mut ConstraintGraph) {
    for i in 0..GRAPH_COLOR_COUNT {
        let color = &mut graph.colors[i];

        // The bit set should never be used on the overflow color
        b3_assert!(i != OVERFLOW_INDEX as usize || color.body_set.bits.is_empty());

        destroy_bit_set(&mut color.body_set);

        color.convex_contacts = Vec::new();
        color.contacts = Vec::new();
        color.joint_sims = Vec::new();
    }
}

// Contacts are always created as non-touching. They get cloned into the constraint
// graph once they are found to be touching.
pub fn add_contact_to_graph(world: &mut World, contact_id: i32) {
    let (body_id_a, body_id_b, contact_flags, manifold_count) = {
        let contact = &world.contacts[contact_id as usize];
        b3_assert!(contact.manifold_count() > 0);
        b3_assert!(contact.flags & CONTACT_TOUCHING_FLAG != 0);
        (
            contact.edges[0].body_id,
            contact.edges[1].body_id,
            contact.flags,
            contact.manifold_count(),
        )
    };

    let (type_a, local_index_a) = {
        let body_a = &world.bodies[body_id_a as usize];
        (body_a.body_type, body_a.local_index)
    };
    let (type_b, local_index_b) = {
        let body_b = &world.bodies[body_id_b as usize];
        (body_b.body_type, body_b.local_index)
    };
    b3_assert!(type_a == BodyType::Dynamic || type_b == BodyType::Dynamic);

    let graph = &mut world.constraint_graph;
    let mut color_index = OVERFLOW_INDEX;

    if !FORCE_OVERFLOW {
        if type_a == BodyType::Dynamic && type_b == BodyType::Dynamic {
            // Dynamic constraint colors cannot encroach on colors reserved for static constraints
            for i in 0..DYNAMIC_COLOR_COUNT {
                let color = &mut graph.colors[i as usize];
                if get_bit(&color.body_set, body_id_a as u32) || get_bit(&color.body_set, body_id_b as u32) {
                    continue;
                }

                set_bit_grow(&mut color.body_set, body_id_a as u32);
                set_bit_grow(&mut color.body_set, body_id_b as u32);
                color_index = i;
                break;
            }
        } else if type_a == BodyType::Dynamic {
            // Static constraint colors build from the end to get higher priority than dyn-dyn constraints
            for i in (1..OVERFLOW_INDEX).rev() {
                let color = &mut graph.colors[i as usize];
                if get_bit(&color.body_set, body_id_a as u32) {
                    continue;
                }

                set_bit_grow(&mut color.body_set, body_id_a as u32);
                color_index = i;
                break;
            }
        } else if type_b == BodyType::Dynamic {
            // Static constraint colors build from the end to get higher priority than dyn-dyn constraints
            for i in (1..OVERFLOW_INDEX).rev() {
                let color = &mut graph.colors[i as usize];
                if get_bit(&color.body_set, body_id_b as u32) {
                    continue;
                }

                set_bit_grow(&mut color.body_set, body_id_b as u32);
                color_index = i;
                break;
            }
        }
    }

    let is_scalar = (contact_flags & SIM_MESH_CONTACT != 0) || color_index == OVERFLOW_INDEX;

    let color = &mut graph.colors[color_index as usize];
    let local_index = if is_scalar {
        color.contacts.len() as i32
    } else {
        color.convex_contacts.len() as i32
    };

    if is_scalar {
        b3_assert!(manifold_count < u16::MAX as i32);
        let spec = crate::contact::ContactSpec {
            contact_id,
            manifold_start: 0,
            manifold_count: manifold_count as u16,
        };
        color.contacts.push(spec);
    } else {
        color.convex_contacts.push(contact_id);
    }

    let contact = &mut world.contacts[contact_id as usize];
    contact.color_index = color_index;
    contact.local_index = local_index;
    contact.body_sim_index_a = if type_a == BodyType::Static { NULL_INDEX } else { local_index_a };
    contact.body_sim_index_b = if type_b == BodyType::Static { NULL_INDEX } else { local_index_b };
}

pub fn remove_contact_from_graph(
    world: &mut World,
    body_id_a: i32,
    body_id_b: i32,
    color_index: i32,
    local_index: i32,
    mesh_contact: bool,
) {
    b3_assert!(0 <= color_index && color_index < GRAPH_COLOR_COUNT as i32);

    if color_index != OVERFLOW_INDEX {
        let color = &mut world.constraint_graph.colors[color_index as usize];
        // This might clear a bit for a static body, but this has no effect
        clear_bit(&mut color.body_set, body_id_a as u32);
        clear_bit(&mut color.body_set, body_id_b as u32);
    }

    if mesh_contact || color_index == OVERFLOW_INDEX {
        let moved_index =
            array_remove_swap(&mut world.constraint_graph.colors[color_index as usize].contacts, local_index);
        if moved_index != NULL_INDEX {
            // Fix index on swapped contact
            let moved_contact_id =
                world.constraint_graph.colors[color_index as usize].contacts[local_index as usize].contact_id;
            let moved_contact = &mut world.contacts[moved_contact_id as usize];
            b3_assert!(moved_contact.set_index == AWAKE_SET);
            b3_assert!(moved_contact.color_index == color_index);
            b3_assert!(moved_contact.local_index == moved_index);
            moved_contact.local_index = local_index;
        }
    } else {
        let moved_index =
            array_remove_swap(&mut world.constraint_graph.colors[color_index as usize].convex_contacts, local_index);
        if moved_index != NULL_INDEX {
            // Fix index on swapped contact
            let moved_contact_id =
                world.constraint_graph.colors[color_index as usize].convex_contacts[local_index as usize];
            let moved_contact = &mut world.contacts[moved_contact_id as usize];
            b3_assert!(moved_contact.set_index == AWAKE_SET);
            b3_assert!(moved_contact.color_index == color_index);
            b3_assert!(moved_contact.local_index == moved_index);
            b3_assert!(moved_contact.flags & SIM_MESH_CONTACT == 0);
            moved_contact.local_index = local_index;
        }
    }
}

fn assign_joint_color(
    graph: &mut ConstraintGraph,
    body_id_a: i32,
    body_id_b: i32,
    type_a: BodyType,
    type_b: BodyType,
) -> i32 {
    b3_assert!(type_a == BodyType::Dynamic || type_b == BodyType::Dynamic);

    if !FORCE_OVERFLOW {
        if type_a == BodyType::Dynamic && type_b == BodyType::Dynamic {
            // Dynamic constraint colors cannot encroach on colors reserved for static constraints
            for i in 0..DYNAMIC_COLOR_COUNT {
                let color = &mut graph.colors[i as usize];
                if get_bit(&color.body_set, body_id_a as u32) || get_bit(&color.body_set, body_id_b as u32) {
                    continue;
                }

                set_bit_grow(&mut color.body_set, body_id_a as u32);
                set_bit_grow(&mut color.body_set, body_id_b as u32);
                return i;
            }
        } else if type_a == BodyType::Dynamic {
            // Static constraint colors build from the end to get higher priority than dyn-dyn constraints
            for i in (1..OVERFLOW_INDEX).rev() {
                let color = &mut graph.colors[i as usize];
                if get_bit(&color.body_set, body_id_a as u32) {
                    continue;
                }

                set_bit_grow(&mut color.body_set, body_id_a as u32);
                return i;
            }
        } else if type_b == BodyType::Dynamic {
            // Static constraint colors build from the end to get higher priority than dyn-dyn constraints
            for i in (1..OVERFLOW_INDEX).rev() {
                let color = &mut graph.colors[i as usize];
                if get_bit(&color.body_set, body_id_b as u32) {
                    continue;
                }

                set_bit_grow(&mut color.body_set, body_id_b as u32);
                return i;
            }
        }
    }

    OVERFLOW_INDEX
}

/// C: b3CreateJointInGraph returns a pointer to a zeroed b3JointSim emplaced in
/// the assigned color. The port returns (color_index, local_index); the caller
/// accesses world.constraint_graph.colors[color].joint_sims[local].
/// Note: the slot is JointSim::default(), not all-zeroes; callers must fill it.
pub fn create_joint_in_graph(world: &mut World, joint_id: i32) -> (i32, i32) {
    let (body_id_a, body_id_b) = {
        let joint = &world.joints[joint_id as usize];
        (joint.edges[0].body_id, joint.edges[1].body_id)
    };
    let type_a = world.bodies[body_id_a as usize].body_type;
    let type_b = world.bodies[body_id_b as usize].body_type;

    let color_index = assign_joint_color(&mut world.constraint_graph, body_id_a, body_id_b, type_a, type_b);

    let color = &mut world.constraint_graph.colors[color_index as usize];
    color.joint_sims.push(crate::joint::JointSim::default());
    let local_index = color.joint_sims.len() as i32 - 1;

    let joint = &mut world.joints[joint_id as usize];
    joint.color_index = color_index;
    joint.local_index = local_index;
    (color_index, local_index)
}

/// C: b3AddJointToGraph(world, jointSim, joint) — the sim is copied by value.
pub fn add_joint_to_graph(world: &mut World, joint_sim: crate::joint::JointSim, joint_id: i32) {
    let (color_index, local_index) = create_joint_in_graph(world, joint_id);
    world.constraint_graph.colors[color_index as usize].joint_sims[local_index as usize] = joint_sim;
}

pub fn remove_joint_from_graph(world: &mut World, body_id_a: i32, body_id_b: i32, color_index: i32, local_index: i32) {
    b3_assert!(0 <= color_index && color_index < GRAPH_COLOR_COUNT as i32);

    if color_index != OVERFLOW_INDEX {
        let color = &mut world.constraint_graph.colors[color_index as usize];
        // May clear static bodies, no effect
        clear_bit(&mut color.body_set, body_id_a as u32);
        clear_bit(&mut color.body_set, body_id_b as u32);
    }

    let moved_index = array_remove_swap(&mut world.constraint_graph.colors[color_index as usize].joint_sims, local_index);
    if moved_index != NULL_INDEX {
        // Fix moved joint
        let moved_id = world.constraint_graph.colors[color_index as usize].joint_sims[local_index as usize].joint_id;
        let moved_joint = &mut world.joints[moved_id as usize];
        b3_assert!(moved_joint.set_index == AWAKE_SET);
        b3_assert!(moved_joint.color_index == color_index);
        b3_assert!(moved_joint.local_index == moved_index);
        moved_joint.local_index = local_index;
    }
}
