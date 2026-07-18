// Port of box3d/src/contact.h (+ contact.c functions to be ported below the structs).

use crate::math_functions::{Quat, Transform, Vec3, AABB};
use crate::types::{Manifold, SATCache, SimplexCache};

pub const FORCE_GHOST_COLLISIONS: bool = false;

/// The C b3ContactCache union. Both caches are stored; the collide function for
/// a shape pair decides which one it uses.
#[derive(Clone, Copy, Debug, Default)]
pub struct ContactCache {
    pub sat_cache: SATCache,
    pub simplex_cache: SimplexCache,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct TriangleCache {
    pub triangle_index: i32,
    pub cache: ContactCache,
}

// b3ContactFlags
// Set when the solid shapes are touching.
pub const CONTACT_TOUCHING_FLAG: u32 = 0x00000001;
// Contact has a hit event
pub const CONTACT_HIT_EVENT_FLAG: u32 = 0x00000002;
// This contact wants contact events
pub const CONTACT_ENABLE_CONTACT_EVENTS: u32 = 0x00000004;
// This contact is between a dynamic and static body
pub const CONTACT_STATIC_FLAG: u32 = 0x00000008;
pub const CONTACT_RECYCLE_FLAG: u32 = 0x00000010;
// Set when the shapes are touching
pub const SIM_TOUCHING_FLAG: u32 = 0x00010000;
// This contact no longer has overlapping AABBs
pub const SIM_DISJOINT: u32 = 0x00020000;
// This contact started touching
pub const SIM_STARTED_TOUCHING: u32 = 0x00040000;
// This contact stopped touching
pub const SIM_STOPPED_TOUCHING: u32 = 0x00080000;
// This contact has a hit event
pub const SIM_ENABLE_HIT_EVENT: u32 = 0x00100000;
// This contact wants pre-solve events
pub const SIM_ENABLE_PRE_SOLVE_EVENTS: u32 = 0x00200000;
// This is a mesh contact
pub const SIM_MESH_CONTACT: u32 = 0x00400000;
pub const RELATIVE_TRANSFORM_VALID: u32 = 0x00800000;

/// A contact edge is used to connect bodies and contacts together
/// in a contact graph where each body is a node and each contact
/// is an edge.
#[derive(Clone, Copy, Debug, Default)]
pub struct ContactEdge {
    pub body_id: i32,
    pub prev_key: i32,
    pub next_key: i32,
}

#[derive(Clone, Debug, Default)]
pub struct MeshContact {
    pub triangle_cache: Vec<TriangleCache>,
    pub query_bounds: AABB,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ConvexContact {
    pub cache: ContactCache,
}

/// Manifold storage for a contact. Convex contacts have exactly 0 or 1
/// manifold — stored inline so the hot paths (collide recycle, prepare,
/// store impulses) reach it without a heap dereference. Mesh contacts with
/// two or more manifolds spill to a Vec. This deviates from C's
/// `b3Manifold*` block allocation on purpose: the inline single manifold is
/// the Rust equivalent of C's arena locality (see the README's intentional
/// differences). Invariant: `Many` never holds fewer than two manifolds.
///
/// Derefs to `[Manifold]`, so indexing, `len`, `is_empty`, `iter`,
/// `iter_mut` and slice coercion all behave like the previous
/// `Vec<Manifold>` field.
#[derive(Clone, Debug, Default)]
pub enum Manifolds {
    #[default]
    None,
    One(Manifold),
    Many(Vec<Manifold>),
}

impl Manifolds {
    /// Replacement for the C b3AllocateManifolds: `count` default-initialized
    /// (zeroed) manifolds.
    pub fn with_count(count: i32) -> Manifolds {
        debug_assert!(count >= 0);
        match count {
            0 => Manifolds::None,
            1 => Manifolds::One(Manifold::default()),
            n => Manifolds::Many(vec![Manifold::default(); n as usize]),
        }
    }

    /// In-place equivalent of `*self = Manifolds::with_count(count)` that
    /// reuses the `Many` buffer when both the old and new counts need one
    /// (the mesh path rebuilds per update; C frees + reallocates zeroed).
    pub fn set_count(&mut self, count: i32) {
        debug_assert!(count >= 0);
        match (count, &mut *self) {
            (n, Manifolds::Many(v)) if n >= 2 => {
                v.clear();
                v.resize_with(n as usize, Manifold::default);
            }
            (n, _) => *self = Manifolds::with_count(n),
        }
    }

    pub fn clear(&mut self) {
        *self = Manifolds::None;
    }
}

impl std::ops::Deref for Manifolds {
    type Target = [Manifold];
    #[inline]
    fn deref(&self) -> &[Manifold] {
        match self {
            Manifolds::None => &[],
            Manifolds::One(m) => std::slice::from_ref(m),
            Manifolds::Many(v) => v,
        }
    }
}

impl std::ops::DerefMut for Manifolds {
    #[inline]
    fn deref_mut(&mut self) -> &mut [Manifold] {
        match self {
            Manifolds::None => &mut [],
            Manifolds::One(m) => std::slice::from_mut(m),
            Manifolds::Many(v) => v,
        }
    }
}

impl<'a> IntoIterator for &'a Manifolds {
    type Item = &'a Manifold;
    type IntoIter = std::slice::Iter<'a, Manifold>;
    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<'a> IntoIterator for &'a mut Manifolds {
    type Item = &'a mut Manifold;
    type IntoIter = std::slice::IterMut<'a, Manifold>;
    #[inline]
    fn into_iter(self) -> Self::IntoIter {
        self.iter_mut()
    }
}

/// Represents the persistent interaction between two shapes.
/// The C union of {convexContact, meshContact} is stored as two fields;
/// SIM_MESH_CONTACT in flags selects which one is active.
// repr(C) with `manifolds` LAST: the enum is 272 bytes; letting the compiler
// place it (or declaring it mid-struct) pushes the hot header and recycle
// cache fields across extra cache lines for the passes that never touch the
// manifold. Explicit layout keeps the first ~2 cache lines hot.
#[repr(C)]
#[derive(Clone, Debug, Default)]
pub struct Contact {
    /// index of simulation set stored in World. NULL_INDEX when slot is free.
    pub set_index: i32,

    /// index into the constraint graph color array.
    /// NULL_INDEX for non-touching or sleeping contacts. NULL_INDEX when slot is free.
    pub color_index: i32,

    /// contact index within set or graph color. NULL_INDEX when slot is free.
    pub local_index: i32,

    pub edges: [ContactEdge; 2],
    pub shape_id_a: i32,
    pub shape_id_b: i32,
    pub child_index: i32,

    /// A contact only belongs to an island if touching, otherwise NULL_INDEX.
    pub island_id: i32,

    /// Index into the island's contacts array for O(1) swap-removal.
    pub island_index: i32,

    /// Back index into World::contacts
    pub contact_id: i32,

    /// These are transient and cached for improved performance. NULL_INDEX for static bodies.
    pub body_sim_index_a: i32,
    pub body_sim_index_b: i32,

    /// Contact flags (consts above)
    pub flags: u32,

    /// Cache for contact recycling.
    pub cached_rotation_a: Quat,
    pub cached_rotation_b: Quat,
    pub cached_relative_pose: Transform,

    /// Mixed friction
    pub friction: f32,

    /// Usage determined by SIM_MESH_CONTACT in flags (C union member).
    pub convex_contact: ConvexContact,
    /// Usage determined by SIM_MESH_CONTACT in flags (C union member).
    pub mesh_contact: MeshContact,

    pub restitution: f32,
    pub rolling_resistance: f32,
    pub tangent_velocity: Vec3,

    /// Monotonically advanced when a contact is allocated in this slot.
    pub generation: u32,

    /// The C `b3Manifold* manifolds` block allocation, inline when single
    /// (see `Manifolds`). Kept as the last field on purpose — see the
    /// repr(C) note above.
    pub manifolds: Manifolds,
}

impl Contact {
    #[inline]
    pub fn manifold_count(&self) -> i32 {
        self.manifolds.len() as i32
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ContactSpec {
    pub contact_id: i32,

    /// Start of the global manifold constraint array
    pub manifold_start: i32,
    pub manifold_count: u16,
}

// ---------------------------------------------------------------------------
// Port of contact.c
// ---------------------------------------------------------------------------
//
// Contacts and determinism
// A deterministic simulation requires contacts to exist in the same order in
// Island no matter the thread count. The order must reproduce from run to run.
// See the discussion at the top of contact.c.
//
// Deviations:
// - The contact register table {supported, primary} is a pure function
//   (contact_register) instead of a lazily initialized static.
// - The arena point buffers are local Vecs.
// - b3AllocateManifolds/b3FreeManifolds become plain Vec operations on
//   Contact::manifolds.
// - The friction/restitution mixing falls back to the C defaults
//   (sqrt(a*b) / max(a,b)) when the world callback is None; the C world always
//   installs a default callback at creation.

use crate::b3_assert;
use crate::constants::MAX_MANIFOLD_POINTS;
use crate::container::array_remove_swap;
use crate::core::NULL_INDEX;
use crate::id::{ContactId, ShapeId};
use crate::id_pool::{alloc_id, free_id};
use crate::manifold::make_feature_id;
use crate::math_functions::{
    add, inv_mul_world_transforms, make_matrix_from_quat, max_float, mul_mv, mul_world_transforms, neg, offset_pos,
    rotate_vector, sub, sub_pos, Vec3 as MVec3, WorldTransform,
};
use crate::physics_world::{World, AWAKE_SET, DISABLED_SET, STATIC_SET};
use crate::shape::{get_shape_materials, Shape, ShapeGeometry};
use crate::types::{
    BodyType, ContactData, ContactEndTouchEvent, ManifoldPoint, ShapeType,
};

pub(crate) fn get_contact_full_id(world: &World, contact_id: ContactId) -> i32 {
    let id = contact_id.index1 - 1;
    let contact = &world.contacts[id as usize];
    b3_assert!(contact.contact_id == id && contact.generation == contact_id.generation);
    id
}

pub fn contact_get_data(world: &World, contact_id: ContactId) -> ContactData<'_> {
    let id = get_contact_full_id(world, contact_id);
    let contact = &world.contacts[id as usize];

    let shape_a = &world.shapes[contact.shape_id_a as usize];
    let shape_b = &world.shapes[contact.shape_id_b as usize];

    ContactData {
        contact_id,
        shape_id_a: ShapeId {
            index1: shape_a.id + 1,
            world0: contact_id.world0,
            generation: shape_a.generation,
        },
        shape_id_b: ShapeId {
            index1: shape_b.id + 1,
            world0: contact_id.world0,
            generation: shape_b.generation,
        },
        manifolds: &contact.manifolds,
    }
}

// The C default friction/restitution callbacks from physics_world.c. The world
// installs these at creation in C; the Rust port also handles None here.
pub(crate) fn mix_friction(world: &World, friction_a: f32, material_a: u64, friction_b: f32, material_b: u64) -> f32 {
    match world.friction_callback {
        Some(fcn) => fcn(friction_a, material_a, friction_b, material_b),
        None => (friction_a * friction_b).sqrt(),
    }
}

pub(crate) fn mix_restitution(
    world: &World,
    restitution_a: f32,
    material_a: u64,
    restitution_b: f32,
    material_b: u64,
) -> f32 {
    match world.restitution_callback {
        Some(fcn) => fcn(restitution_a, material_a, restitution_b, material_b),
        None => max_float(restitution_a, restitution_b),
    }
}

// The C b3ContactRegister table: (supported, primary). Built from the
// b3AddType calls in b3InitializeContactRegisters.
fn contact_register(type_a: ShapeType, type_b: ShapeType) -> (bool, bool) {
    use ShapeType::*;
    const PAIRS: [(ShapeType, ShapeType); 15] = [
        (Sphere, Sphere),
        (Capsule, Sphere),
        (Capsule, Capsule),
        (Compound, Sphere),
        (Compound, Capsule),
        (Compound, Hull),
        (Hull, Sphere),
        (Hull, Capsule),
        (Hull, Hull),
        (Mesh, Sphere),
        (Mesh, Capsule),
        (Mesh, Hull),
        (Height, Sphere),
        (Height, Capsule),
        (Height, Hull),
    ];

    for &(t1, t2) in PAIRS.iter() {
        if type_a == t1 && type_b == t2 {
            return (true, true);
        }
        if t1 != t2 && type_a == t2 && type_b == t1 {
            return (true, false);
        }
    }

    (false, false)
}

/// C: b3InitializeContactRegisters. The Rust table is a pure function; kept for
/// API parity with world creation.
pub fn initialize_contact_registers() {}

pub fn create_contact(world: &mut World, shape_id_a: i32, shape_id_b: i32, child_index: i32) {
    let type_a = world.shapes[shape_id_a as usize].shape_type();
    let type_b = world.shapes[shape_id_b as usize].shape_type();

    let (supported, primary) = contact_register(type_a, type_b);

    if !supported {
        // For example, no mesh vs mesh collision
        return;
    }

    if !primary {
        // flip order
        create_contact(world, shape_id_b, shape_id_a, child_index);
        return;
    }

    // Copy shape data needed below.
    let (body_id_a, sensor_index_a, enable_contact_events_a, enable_pre_solve_events_a, rolling_resistance_a, radius_a) = {
        let shape_a = &world.shapes[shape_id_a as usize];
        let radius_a = match &shape_a.geom {
            ShapeGeometry::Sphere(sphere) => sphere.radius,
            ShapeGeometry::Capsule(capsule) => capsule.radius,
            _ => 0.0,
        };
        (
            shape_a.body_id,
            shape_a.sensor_index,
            shape_a.enable_contact_events,
            shape_a.enable_pre_solve_events,
            get_shape_materials(shape_a)[0].rolling_resistance,
            radius_a,
        )
    };
    let (body_id_b, sensor_index_b, enable_contact_events_b, enable_pre_solve_events_b, rolling_resistance_b, radius_b) = {
        let shape_b = &world.shapes[shape_id_b as usize];
        let radius_b = match &shape_b.geom {
            ShapeGeometry::Sphere(sphere) => sphere.radius,
            ShapeGeometry::Capsule(capsule) => capsule.radius,
            _ => 0.0,
        };
        (
            shape_b.body_id,
            shape_b.sensor_index,
            shape_b.enable_contact_events,
            shape_b.enable_pre_solve_events,
            get_shape_materials(shape_b)[0].rolling_resistance,
            radius_b,
        )
    };

    let (set_index_a, flags_a, body_type_a) = {
        let body_a = &world.bodies[body_id_a as usize];
        (body_a.set_index, body_a.flags, body_a.body_type)
    };
    let (set_index_b, flags_b, body_type_b) = {
        let body_b = &world.bodies[body_id_b as usize];
        (body_b.set_index, body_b.flags, body_b.body_type)
    };

    b3_assert!(set_index_a != DISABLED_SET && set_index_b != DISABLED_SET);
    b3_assert!(set_index_a != STATIC_SET || set_index_b != STATIC_SET);

    let set_index = if set_index_a == AWAKE_SET || set_index_b == AWAKE_SET {
        AWAKE_SET
    } else {
        // sleeping and non-touching contacts live in the disabled set
        // later if this set is found to be touching then the sleeping
        // islands will be linked and the contact moved to the merged island

        // This is possible if a shape moves slightly then falls asleep
        DISABLED_SET
    };

    // Create contact key and contact
    let contact_id = alloc_id(&mut world.contact_id_pool);
    if contact_id == world.contacts.len() as i32 {
        world.contacts.push(Contact::default());
    }

    let local_index = world.solver_sets[set_index as usize].contact_indices.len() as i32;

    {
        let contact = &mut world.contacts[contact_id as usize];
        let generation = contact.generation;
        *contact = Contact::default();
        contact.contact_id = contact_id;
        contact.generation = generation + 1;
        contact.set_index = set_index;
        contact.color_index = NULL_INDEX;
        contact.local_index = local_index;
        contact.island_id = NULL_INDEX;
        contact.island_index = NULL_INDEX;
        contact.shape_id_a = shape_id_a;
        contact.shape_id_b = shape_id_b;
        contact.child_index = child_index;

        // Both bodies must enable recycling
        if (flags_a & crate::body::BODY_ENABLE_CONTACT_RECYCLING) != 0
            && (flags_b & crate::body::BODY_ENABLE_CONTACT_RECYCLING) != 0
        {
            contact.flags |= CONTACT_RECYCLE_FLAG;
        }
    }

    if type_a == ShapeType::Mesh || type_a == ShapeType::Height {
        world.contacts[contact_id as usize].flags |= SIM_MESH_CONTACT;
    } else if type_a == ShapeType::Compound {
        let child = {
            let shape_a = &world.shapes[shape_id_a as usize];
            crate::compound::get_compound_child(shape_a.as_compound(), child_index)
        };
        if child.shape_type() == ShapeType::Mesh {
            world.contacts[contact_id as usize].flags |= SIM_MESH_CONTACT;
        }
    }

    // todo impose these restrictions to make life easier
    b3_assert!(type_b == ShapeType::Sphere || type_b == ShapeType::Capsule || type_b == ShapeType::Hull);

    // Is either body static?
    // Note: it is possible to have a dynamic mesh collide with a static convex shape.
    if body_type_a == BodyType::Static || body_type_b == BodyType::Static {
        world.contacts[contact_id as usize].flags |= CONTACT_STATIC_FLAG;
    }

    b3_assert!(sensor_index_a == NULL_INDEX && sensor_index_b == NULL_INDEX);

    if enable_contact_events_a || enable_contact_events_b {
        world.contacts[contact_id as usize].flags |= CONTACT_ENABLE_CONTACT_EVENTS;
    }

    // Connect to body A
    {
        let head_contact_key = world.bodies[body_id_a as usize].head_contact_key;

        {
            let contact = &mut world.contacts[contact_id as usize];
            contact.edges[0].body_id = body_id_a;
            contact.edges[0].prev_key = NULL_INDEX;
            contact.edges[0].next_key = head_contact_key;
        }

        let key_a = (contact_id << 1) | 0;
        if head_contact_key != NULL_INDEX {
            let head_contact = &mut world.contacts[(head_contact_key >> 1) as usize];
            head_contact.edges[(head_contact_key & 1) as usize].prev_key = key_a;
        }
        let body_a = &mut world.bodies[body_id_a as usize];
        body_a.head_contact_key = key_a;
        body_a.contact_count += 1;
    }

    // Connect to body B
    {
        let head_contact_key = world.bodies[body_id_b as usize].head_contact_key;

        {
            let contact = &mut world.contacts[contact_id as usize];
            contact.edges[1].body_id = body_id_b;
            contact.edges[1].prev_key = NULL_INDEX;
            contact.edges[1].next_key = head_contact_key;
        }

        let key_b = (contact_id << 1) | 1;
        if head_contact_key != NULL_INDEX {
            let head_contact = &mut world.contacts[(head_contact_key >> 1) as usize];
            head_contact.edges[(head_contact_key & 1) as usize].prev_key = key_b;
        }
        let body_b = &mut world.bodies[body_id_b as usize];
        body_b.head_contact_key = key_b;
        body_b.contact_count += 1;
    }

    // Add to pair set for fast lookup
    let pair_key = crate::table::shape_pair_key(shape_id_a, shape_id_b, child_index);
    crate::table::add_key(&mut world.broad_phase.pair_set, pair_key);

    // Contacts are created as non-touching. Later if they are found to be touching
    // they will link islands and be moved into the constraint graph.
    world.solver_sets[set_index as usize].contact_indices.push(contact_id);

    let max_radius = max_float(radius_a, radius_b);

    // Assuming the rolling resistance doesn't change
    world.contacts[contact_id as usize].rolling_resistance =
        max_float(rolling_resistance_a, rolling_resistance_b) * max_radius;

    if enable_pre_solve_events_a || enable_pre_solve_events_b {
        world.contacts[contact_id as usize].flags |= SIM_ENABLE_PRE_SOLVE_EVENTS;
    }
}

// A contact is destroyed when:
// - broad-phase proxies stop overlapping
// - a body is destroyed
// - a body is disabled
// - a body changes type from dynamic to kinematic or static
// - a shape is destroyed
// - contact filtering is modified
pub fn destroy_contact(world: &mut World, contact_id: i32, wake_bodies: bool) {
    let (shape_id_a, shape_id_b, child_index, edge_a, edge_b, flags, generation) = {
        let contact = &world.contacts[contact_id as usize];
        b3_assert!(contact.contact_id == contact_id);
        (
            contact.shape_id_a,
            contact.shape_id_b,
            contact.child_index,
            contact.edges[0],
            contact.edges[1],
            contact.flags,
            contact.generation,
        )
    };

    // Remove pair from set
    let pair_key = crate::table::shape_pair_key(shape_id_a, shape_id_b, child_index);
    crate::table::remove_key(&mut world.broad_phase.pair_set, pair_key);

    world.contacts[contact_id as usize].manifolds = Manifolds::None;

    let body_id_a = edge_a.body_id;
    let body_id_b = edge_b.body_id;

    let touching = (flags & CONTACT_TOUCHING_FLAG) != 0;

    // End touch event
    if touching && (flags & CONTACT_ENABLE_CONTACT_EVENTS) != 0 {
        let world_id = world.world_id;
        let shape_a = &world.shapes[shape_id_a as usize];
        let shape_b = &world.shapes[shape_id_b as usize];
        let shape_id_a_pub = ShapeId { index1: shape_a.id + 1, world0: world_id, generation: shape_a.generation };
        let shape_id_b_pub = ShapeId { index1: shape_b.id + 1, world0: world_id, generation: shape_b.generation };

        let contact_id_pub = ContactId { index1: contact_id + 1, world0: world_id, generation };

        let event = ContactEndTouchEvent {
            shape_id_a: shape_id_a_pub,
            shape_id_b: shape_id_b_pub,
            contact_id: contact_id_pub,
        };

        world.contact_end_events[world.end_event_array_index as usize].push(event);
    }

    // Remove from body A
    if edge_a.prev_key != NULL_INDEX {
        let prev_contact = &mut world.contacts[(edge_a.prev_key >> 1) as usize];
        prev_contact.edges[(edge_a.prev_key & 1) as usize].next_key = edge_a.next_key;
    }

    if edge_a.next_key != NULL_INDEX {
        let next_contact = &mut world.contacts[(edge_a.next_key >> 1) as usize];
        next_contact.edges[(edge_a.next_key & 1) as usize].prev_key = edge_a.prev_key;
    }

    let edge_key_a = (contact_id << 1) | 0;
    {
        let body_a = &mut world.bodies[body_id_a as usize];
        if body_a.head_contact_key == edge_key_a {
            body_a.head_contact_key = edge_a.next_key;
        }
        body_a.contact_count -= 1;
    }

    // Remove from body B
    if edge_b.prev_key != NULL_INDEX {
        let prev_contact = &mut world.contacts[(edge_b.prev_key >> 1) as usize];
        prev_contact.edges[(edge_b.prev_key & 1) as usize].next_key = edge_b.next_key;
    }

    if edge_b.next_key != NULL_INDEX {
        let next_contact = &mut world.contacts[(edge_b.next_key >> 1) as usize];
        next_contact.edges[(edge_b.next_key & 1) as usize].prev_key = edge_b.prev_key;
    }

    let edge_key_b = (contact_id << 1) | 1;
    {
        let body_b = &mut world.bodies[body_id_b as usize];
        if body_b.head_contact_key == edge_key_b {
            body_b.head_contact_key = edge_b.next_key;
        }
        body_b.contact_count -= 1;
    }

    if (flags & SIM_MESH_CONTACT) != 0 {
        world.contacts[contact_id as usize].mesh_contact.triangle_cache = Vec::new();
    }

    // Remove contact from the array that owns it
    if world.contacts[contact_id as usize].island_id != NULL_INDEX {
        crate::island::unlink_contact(world, contact_id);
    }

    let (color_index, local_index, set_index) = {
        let contact = &world.contacts[contact_id as usize];
        (contact.color_index, contact.local_index, contact.set_index)
    };

    if color_index != NULL_INDEX {
        // contact is an active constraint
        b3_assert!(set_index == AWAKE_SET);
        let mesh_contact = (flags & SIM_MESH_CONTACT) != 0;
        crate::constraint_graph::remove_contact_from_graph(world, body_id_a, body_id_b, color_index, local_index, mesh_contact);
    } else {
        // contact is non-touching or is sleeping or is a sensor
        b3_assert!(set_index != AWAKE_SET || (flags & CONTACT_TOUCHING_FLAG) == 0);
        let moved_index = array_remove_swap(
            &mut world.solver_sets[set_index as usize].contact_indices,
            local_index,
        );
        if moved_index != NULL_INDEX {
            let moved_contact_index = world.solver_sets[set_index as usize].contact_indices[local_index as usize];
            let moved_contact = &mut world.contacts[moved_contact_index as usize];
            moved_contact.local_index = local_index;
        }
    }

    // Free contact and id (preserve generation)
    {
        let contact = &mut world.contacts[contact_id as usize];
        contact.contact_id = NULL_INDEX;
        contact.set_index = NULL_INDEX;
        contact.color_index = NULL_INDEX;
        contact.local_index = NULL_INDEX;
    }
    free_id(&mut world.contact_id_pool, contact_id);

    if wake_bodies && touching {
        crate::body::wake_body(world, body_id_a);
        crate::body::wake_body(world, body_id_b);
    }
}

// Standalone like C (see the note on collide_hulls).
#[inline(never)]
fn compute_convex_manifold(
    world: &World,
    task_context: &mut crate::physics_world::TaskContext,
    contact: &mut Contact,
    shape_a: &Shape,
    xf_a: WorldTransform,
    shape_b: &Shape,
    xf_b: WorldTransform,
) -> bool {
    let type_a = shape_a.shape_type();
    let type_b = shape_b.shape_type();

    // Copy the cache out of the contact (C mutates it in place through a pointer).
    let mut cache = contact.convex_contact.cache;

    let point_capacity = 32;

    // C builds the local manifold in a caller-provided stack buffer. The port
    // reuses the per-worker scratch (taken out of the task context for the
    // duration so borrows don't overlap) — the point Vec keeps its capacity
    // across contacts, so this allocates only on the first use per worker.
    let mut geom_manifold = std::mem::take(&mut task_context.geom_manifold_scratch);
    geom_manifold.points.clear();
    geom_manifold.points.reserve(point_capacity as usize);
    geom_manifold.normal = Vec3::ZERO;
    geom_manifold.triangle_normal = Vec3::ZERO;
    geom_manifold.triangle_index = 0;
    geom_manifold.i1 = 0;
    geom_manifold.i2 = 0;
    geom_manifold.i3 = 0;
    geom_manifold.squared_distance = 0.0;
    geom_manifold.feature = Default::default();
    geom_manifold.triangle_flags = 0;

    let transform_b_to_a = inv_mul_world_transforms(xf_a, xf_b);

    if type_a == ShapeType::Sphere {
        b3_assert!(type_b == ShapeType::Sphere);
        crate::convex_manifold::collide_spheres(
            &mut geom_manifold,
            point_capacity,
            shape_a.as_sphere(),
            shape_b.as_sphere(),
            transform_b_to_a,
        );
    } else if type_a == ShapeType::Capsule {
        if type_b == ShapeType::Sphere {
            crate::convex_manifold::collide_capsule_and_sphere(
                &mut geom_manifold,
                point_capacity,
                shape_a.as_capsule(),
                shape_b.as_sphere(),
                transform_b_to_a,
            );
        } else {
            b3_assert!(type_b == ShapeType::Capsule);
            crate::convex_manifold::collide_capsules(
                &mut geom_manifold,
                point_capacity,
                shape_a.as_capsule(),
                shape_b.as_capsule(),
                transform_b_to_a,
            );
        }
    } else {
        b3_assert!(type_a == ShapeType::Hull);

        if type_b == ShapeType::Sphere {
            crate::convex_manifold::collide_hull_and_sphere(
                &mut geom_manifold,
                point_capacity,
                shape_a.as_hull(),
                shape_b.as_sphere(),
                transform_b_to_a,
                &mut cache.simplex_cache,
            );
        } else if type_b == ShapeType::Capsule {
            crate::convex_manifold::collide_hull_and_capsule(
                &mut geom_manifold,
                point_capacity,
                shape_a.as_hull(),
                shape_b.as_capsule(),
                transform_b_to_a,
                &mut cache.simplex_cache,
            );
        } else {
            b3_assert!(type_b == ShapeType::Hull);
            // PORT EXTENSION — feature-recycling tier (not in upstream C):
            // serve the contact from the cached winning SAT feature under
            // drift/refresh bounds; 0 means fall through to the full SAT.
            let mut handled = 0u8;
            if world.enable_feature_recycling {
                let was_touching = contact.manifold_count() > 0;
                handled = crate::convex_manifold::collide_hulls_feature_recycled(
                    &mut geom_manifold,
                    point_capacity,
                    shape_a.as_hull(),
                    shape_b.as_hull(),
                    transform_b_to_a,
                    &mut cache.sat_cache,
                    world.contact_recycle_distance,
                    was_touching,
                );
                match handled {
                    1 => task_context.feature_separated_skip_count += 1,
                    2 => task_context.feature_recycled_contact_count += 1,
                    _ => {}
                }
            }
            if handled == 0 {
                crate::convex_manifold::collide_hulls(
                    &mut geom_manifold,
                    point_capacity,
                    shape_a.as_hull(),
                    shape_b.as_hull(),
                    transform_b_to_a,
                    &mut cache.sat_cache,
                );
                task_context.sat_call_count += 1;
                task_context.sat_cache_hit_count += cache.sat_cache.hit as i32;
                if world.enable_feature_recycling {
                    // New drift reference for the tier above.
                    cache.sat_cache.sat_pose = transform_b_to_a;
                    cache.sat_cache.steps_since_sat = 0;
                }
            }
        }
    }

    // Write the cache back.
    contact.convex_contact.cache = cache;

    if geom_manifold.point_count() == 0 {
        if contact.manifold_count() > 0 {
            contact.manifolds = Manifolds::None;
        }

        task_context.geom_manifold_scratch = geom_manifold;
        return false;
    }

    let mut old_points = [ManifoldPoint::default(); MAX_MANIFOLD_POINTS];
    let mut old_count = 0;

    if contact.manifold_count() == 0 {
        contact.manifolds = Manifolds::with_count(1);
    } else {
        old_count = contact.manifolds[0].point_count;
        old_points[..old_count as usize].copy_from_slice(&contact.manifolds[0].points[..old_count as usize]);
    }

    let manifold = &mut contact.manifolds[0];
    manifold.point_count = geom_manifold.point_count();

    let matrix_a = make_matrix_from_quat(xf_a.q);
    manifold.normal = mul_mv(matrix_a, geom_manifold.normal);

    // Store point data in contact
    for i in 0..geom_manifold.point_count() {
        let source = &geom_manifold.points[i as usize];
        let target = &mut manifold.points[i as usize];

        // Contact points are computed in frame A
        target.anchor_a = mul_mv(matrix_a, source.point);
        target.anchor_b = add(target.anchor_a, sub_pos(xf_a.p, xf_b.p));
        target.separation = source.separation;
        target.feature_id = make_feature_id(source.pair);
        target.triangle_index = NULL_INDEX;
        target.normal_velocity = 0.0;
    }

    // Copy impulses from old points
    for i in 0..geom_manifold.point_count() {
        let pt2 = &mut manifold.points[i as usize];
        pt2.total_normal_impulse = 0.0;
        pt2.persisted = false;

        for j in 0..old_count {
            let pt1 = &mut old_points[j as usize];

            if pt2.feature_id == pt1.feature_id {
                pt2.normal_impulse = pt1.normal_impulse;
                pt2.persisted = true;

                // claimed
                pt1.feature_id = u32::MAX;

                break;
            }
        }

        if !pt2.persisted {
            pt2.normal_impulse = 0.0;
        }
    }

    task_context.geom_manifold_scratch = geom_manifold;
    true
}

fn update_convex_contact(
    world: &World,
    task_context: &mut crate::physics_world::TaskContext,
    contact: &mut Contact,
    pre_solve: Option<&mut Option<Box<crate::types::PreSolveFcn>>>,
    shape_a: &Shape,
    xf_a: WorldTransform,
    shape_b: &Shape,
    xf_b: WorldTransform,
    flip: bool,
) -> bool {
    // Compute new manifold
    let mut touching = compute_convex_manifold(world, task_context, contact, shape_a, xf_a, shape_b, xf_b);

    if !touching {
        b3_assert!(contact.manifolds.is_empty());
        return false;
    }

    b3_assert!(contact.manifold_count() == 1);

    if flip {
        // Not flipping the feature ids because they just need to match and flipping is consistent.
        let manifold = &mut contact.manifolds[0];
        manifold.normal = neg(manifold.normal);
        let point_count = manifold.point_count;
        for i in 0..point_count {
            let mp = &mut manifold.points[i as usize];
            std::mem::swap(&mut mp.anchor_a, &mut mp.anchor_b);
        }
    }

    let material_a = get_shape_materials(shape_a)[0];
    let material_b = get_shape_materials(shape_b)[0];

    // Keep these updated in case the values on the shapes are modified
    let friction = mix_friction(world, material_a.friction, material_a.user_material_id, material_b.friction, material_b.user_material_id);
    let restitution = mix_restitution(
        world,
        material_a.restitution,
        material_a.user_material_id,
        material_b.restitution,
        material_b.user_material_id,
    );
    contact.friction = friction;
    contact.restitution = restitution;

    if material_a.rolling_resistance > 0.0 || material_b.rolling_resistance > 0.0 {
        let radius_a = match &shape_a.geom {
            ShapeGeometry::Sphere(sphere) => sphere.radius,
            ShapeGeometry::Capsule(capsule) => capsule.radius,
            ShapeGeometry::Hull(hull) => 0.25 * hull.inner_radius,
            _ => 0.0,
        };

        let radius_b = match &shape_b.geom {
            ShapeGeometry::Sphere(sphere) => sphere.radius,
            ShapeGeometry::Capsule(capsule) => capsule.radius,
            ShapeGeometry::Hull(hull) => 0.25 * hull.inner_radius,
            _ => 0.0,
        };

        let max_radius = max_float(radius_a, radius_b);
        contact.rolling_resistance =
            max_float(material_a.rolling_resistance, material_b.rolling_resistance) * max_radius;
    } else {
        contact.rolling_resistance = 0.0;
    }

    let tangent_velocity_a = rotate_vector(xf_a.q, material_a.tangent_velocity);
    let tangent_velocity_b = rotate_vector(xf_b.q, material_b.tangent_velocity);
    contact.tangent_velocity = sub(tangent_velocity_a, tangent_velocity_b);

    // The pre-solve slot is Some only on the serial collide path (the world
    // callback is taken by the caller); when a callback is installed the
    // collide pass falls back to a single worker, so this access is exclusive.
    if let Some(pre_solve_slot) = pre_solve {
        if pre_solve_slot.is_some() && (contact.flags & SIM_ENABLE_PRE_SOLVE_EVENTS) != 0 {
            let shape_id_a_pub = ShapeId { index1: shape_a.id + 1, world0: world.world_id, generation: shape_a.generation };
            let shape_id_b_pub = ShapeId { index1: shape_b.id + 1, world0: world.world_id, generation: shape_b.generation };

            // this call assumes thread safety
            let (point, normal) = {
                let manifold = &contact.manifolds[0];
                (offset_pos(xf_a.p, manifold.points[0].anchor_a), manifold.normal)
            };
            let mut fcn = pre_solve_slot.take().unwrap();
            touching = fcn(shape_id_a_pub, shape_id_b_pub, point, normal);
            *pre_solve_slot = Some(fcn);
            if !touching {
                // disable contact
                contact.manifolds = Manifolds::None;
                return false;
            }
        }
    }

    if shape_a.enable_hit_events || shape_b.enable_hit_events {
        contact.flags |= SIM_ENABLE_HIT_EVENT;
    } else {
        contact.flags &= !SIM_ENABLE_HIT_EVENT;
    }

    true
}

// Update the contact manifold and touching status.
// Note: do not assume the shape AABBs are overlapping or are valid.
#[allow(clippy::too_many_arguments)]
// Standalone like C (b3UpdateContact / the b3*_Convex stage functions are
// separate symbols): inlining these into the solver dispatch/collide loop
// makes LLVM allocate registers across the merged body and the inner loops
// pay constant spill traffic. inline(never) restores per-function register
// allocation and the C code layout.
#[inline(never)]
pub fn update_contact(
    world: &World,
    task_context: &mut crate::physics_world::TaskContext,
    contact: &mut Contact,
    pre_solve: Option<&mut Option<Box<crate::types::PreSolveFcn>>>,
    shape_id_a: i32,
    local_center_a: MVec3,
    xf_a: WorldTransform,
    shape_id_b: i32,
    local_center_b: MVec3,
    xf_b: WorldTransform,
    is_fast: bool,
) -> bool {
    let touching;

    // By reference — cloning here deep-copied both shapes (materials Vec +
    // geometry, including whole compound child trees) per contact update, and
    // the Arc refcount traffic contended across workers.
    let shape_a = &world.shapes[shape_id_a as usize];
    let shape_b = &world.shapes[shape_id_b as usize];

    b3_assert!(shape_b.shape_type() != ShapeType::Compound);

    if shape_a.shape_type() == ShapeType::Compound {
        let child_index = contact.child_index;
        let child = crate::compound::get_compound_child(shape_a.as_compound(), child_index);

        // Temporary child shape to match existing function signatures.
        // C stack-copies the compound shape; the port reuses the per-worker
        // scratch (materials capacity survives) and never clones the compound
        // geometry the arms below immediately replace. Taken out of the task
        // context so the borrows below don't overlap.
        let mut child_shape_a = std::mem::take(&mut task_context.child_shape_scratch);
        child_shape_a.copy_non_geom_from(shape_a);

        match &child.geom {
            crate::types::ChildShapeGeom::Capsule(capsule) => {
                child_shape_a.geom = ShapeGeometry::Capsule(*capsule);
                if shape_b.shape_type() == ShapeType::Hull {
                    // Flip
                    let flip = true;
                    touching =
                        update_convex_contact(world, task_context, contact, pre_solve, shape_b, xf_b, &child_shape_a, xf_a, flip);
                } else {
                    let flip = false;
                    touching =
                        update_convex_contact(world, task_context, contact, pre_solve, &child_shape_a, xf_a, shape_b, xf_b, flip);
                }
            }
            crate::types::ChildShapeGeom::Hull(hull) => {
                child_shape_a.geom = ShapeGeometry::Hull(hull.clone());
                let xf_child = mul_world_transforms(xf_a, child.transform);
                let flip = false;
                touching =
                    update_convex_contact(world, task_context, contact, pre_solve, &child_shape_a, xf_child, shape_b, xf_b, flip);
            }
            crate::types::ChildShapeGeom::Mesh(mesh) => {
                child_shape_a.geom = ShapeGeometry::Mesh(mesh.clone());
                let xf_child = mul_world_transforms(xf_a, child.transform);

                touching = crate::mesh_contact::compute_mesh_manifolds(
                    world,
                    task_context,
                    contact,
                    &child_shape_a,
                    Some(&child.material_indices),
                    xf_child,
                    shape_b,
                    xf_b,
                    is_fast,
                );

                if touching && (shape_a.enable_hit_events || shape_b.enable_hit_events) {
                    contact.flags |= SIM_ENABLE_HIT_EVENT;
                } else {
                    contact.flags &= !SIM_ENABLE_HIT_EVENT;
                }

                b3_assert!(
                    (touching && contact.manifold_count() > 0) || (!touching && contact.manifold_count() == 0)
                );
            }
            crate::types::ChildShapeGeom::Sphere(sphere) => {
                child_shape_a.geom = ShapeGeometry::Sphere(*sphere);
                if shape_b.shape_type() == ShapeType::Capsule || shape_b.shape_type() == ShapeType::Hull {
                    // Flip
                    let flip = true;
                    touching =
                        update_convex_contact(world, task_context, contact, pre_solve, shape_b, xf_b, &child_shape_a, xf_a, flip);
                } else {
                    let flip = false;
                    touching =
                        update_convex_contact(world, task_context, contact, pre_solve, &child_shape_a, xf_a, shape_b, xf_b, flip);
                }
            }
        }

        // The anchor is relative to the child origin but oriented in world space.
        // Offset the anchor to be relative to the compound origin.
        let offset = rotate_vector(xf_a.q, child.transform.p);
        for manifold in contact.manifolds.iter_mut() {
            let point_count = manifold.point_count;
            for j in 0..point_count {
                let mp = &mut manifold.points[j as usize];
                mp.anchor_a = add(mp.anchor_a, offset);
            }
        }

        // Return the scratch; reset geom so no Arc to child geometry outlives
        // this update (materials keeps its capacity for the next contact).
        child_shape_a.geom = ShapeGeometry::default();
        task_context.child_shape_scratch = child_shape_a;
    } else if shape_a.shape_type() == ShapeType::Mesh || shape_a.shape_type() == ShapeType::Height {
        // Does this contact touch a mesh or height-field?

        // Compute mesh manifolds
        touching = crate::mesh_contact::compute_mesh_manifolds(
            world,
            task_context,
            contact,
            shape_a,
            None,
            xf_a,
            shape_b,
            xf_b,
            is_fast,
        );

        if touching && (shape_a.enable_hit_events || shape_b.enable_hit_events) {
            contact.flags |= SIM_ENABLE_HIT_EVENT;
        } else {
            contact.flags &= !SIM_ENABLE_HIT_EVENT;
        }

        b3_assert!((touching && contact.manifold_count() > 0) || (!touching && contact.manifold_count() == 0));
    } else {
        // Convex-vs-convex
        let flip = false;
        touching = update_convex_contact(world, task_context, contact, pre_solve, shape_a, xf_a, shape_b, xf_b, flip);
    }

    if touching {
        let center_a = rotate_vector(xf_a.q, local_center_a);
        let center_b = rotate_vector(xf_b.q, local_center_b);

        // Adjust anchors to be relative to center of mass
        for manifold in contact.manifolds.iter_mut() {
            for j in 0..manifold.point_count {
                let mp = &mut manifold.points[j as usize];
                mp.anchor_a = sub(mp.anchor_a, center_a);
                mp.anchor_b = sub(mp.anchor_b, center_b);
            }
        }

        contact.flags |= SIM_TOUCHING_FLAG;
    } else {
        contact.flags &= !SIM_TOUCHING_FLAG;
    }

    touching
}
