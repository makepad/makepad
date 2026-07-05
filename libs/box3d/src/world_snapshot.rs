// Port of box3d/src/world_snapshot.h + world_snapshot.c
//
// Serialize the live world into a buffer, interning shape geometry into a
// recording registry, and restore a snapshot image into a freshly-created
// (shell) world. Stepping the restored world is bit-identical to stepping the
// original.
//
// Deviations (see PORTING.md):
// - The C serializer memcpys raw structs; the Rust structs carry Vecs, Strings,
//   Arcs and enums, so every struct is written field by field in a canonical
//   little-endian order (following the C struct field order). The image format
//   is therefore port-specific: C snapshots cannot be loaded and vice versa.
//   The header magic/version/layout-hash guard incompatible images exactly
//   like C (b3ComputeLayoutHash becomes a hash over Rust struct sizes).
// - Free/live slot detection matches C (body.id == i, shape.id == i,
//   contact.contact_id == i); free slots write their POD fields (generations
//   must survive) but no geometry/heap payload, exactly like C.
// - userData is scrubbed to 0 on write like C scrubs pointers to NULL.
// - The C renderer-handle preservation in b3DesShapes (userShape) is not
//   ported: debug draw handles do not exist in the port.
// - b3FreeLiveSimElements collapses to releasing hull database references;
//   Vec/Arc drops replace the manual frees.
// - The transient per-color constraint ranges (the port's replacement for the
//   C constraint pointers) are reset to defaults on restore, matching C's
//   "transient pointers left at NULL/0 from shell".

use std::sync::Arc;

use crate::bitset::BitSet;
use crate::body::{Body, BodySim, BodyState};
use crate::constants::GRAPH_COLOR_COUNT;
use crate::constraint_graph::{GraphColor, OVERFLOW_INDEX};
use crate::contact::{Contact, ContactCache, ContactEdge, ContactSpec, ConvexContact, TriangleCache, SIM_MESH_CONTACT};
use crate::core::NULL_INDEX;
use crate::id_pool::IdPool;
use crate::island::{ContactLink, Island, IslandSim, JointLink};
use crate::joint::{
    DistanceJoint, Joint, JointEdge, JointSim, JointUnion, MotorJoint, ParallelJoint, PrismaticJoint,
    RevoluteJoint, SphericalJoint, WeldJoint, WheelJoint,
};
use crate::constants::MAX_MANIFOLD_POINTS;
use crate::math_functions::{Pos, Vec2, WorldTransform};
use crate::physics_world::World;
use crate::recording::{rec_intern_compound, rec_intern_height_field, rec_intern_hull, rec_intern_mesh, RecBuffer, Recording};
use crate::recording_replay::{RecCursor, RecReader, RegistryGeometry};
use crate::sensor::{Sensor, Visitor};
use crate::shape::{Shape, ShapeGeometry};
use crate::solver::Softness;
use crate::solver_set::SolverSet;
use crate::table::{HashSet, SetItem};
use crate::types::{
    BodyType, Capacity, DynamicTree, Filter, JointType, Manifold, ManifoldPoint, Mesh, SATCache,
    SimplexCache, TreeNode, TreeNodeChildren, BODY_TYPE_COUNT, DYNAMIC_TREE_VERSION,
};

// Snapshot image magic 'BNS3' and version
pub const SNAP_MAGIC: u32 = 0x33534E42;
pub const SNAP_VERSION: u32 = 1;

pub const SNAP_FLAG_VALIDATION: u32 = 0x1;
pub const SNAP_FLAG_DOUBLE_PRECISION: u32 = 0x2;

// Layout hash over all field-serialized structs + key constants.
// Changing a struct updates this, catching format drift early (the Rust
// equivalent of the C ABI-drift check).
fn compute_layout_hash() -> u32 {
    let mut h: u32 = 2166136261;
    let mut mix = |x: usize| {
        h ^= x as u32;
        h = h.wrapping_mul(16777619);
    };
    mix(std::mem::size_of::<Body>());
    mix(std::mem::size_of::<BodySim>());
    mix(std::mem::size_of::<BodyState>());
    mix(std::mem::size_of::<Shape>());
    mix(std::mem::size_of::<Contact>());
    mix(std::mem::size_of::<Manifold>());
    mix(std::mem::size_of::<Joint>());
    mix(std::mem::size_of::<JointSim>());
    mix(std::mem::size_of::<Island>());
    mix(std::mem::size_of::<IslandSim>());
    mix(std::mem::size_of::<ContactLink>());
    mix(std::mem::size_of::<JointLink>());
    mix(std::mem::size_of::<Sensor>());
    mix(std::mem::size_of::<Visitor>());
    mix(std::mem::size_of::<SolverSet>());
    mix(std::mem::size_of::<GraphColor>());
    mix(std::mem::size_of::<DynamicTree>());
    mix(std::mem::size_of::<TreeNode>());
    mix(std::mem::size_of::<SetItem>());
    mix(std::mem::size_of::<IdPool>());
    mix(std::mem::size_of::<crate::types::SurfaceMaterial>());
    mix(std::mem::size_of::<ContactSpec>());
    mix(std::mem::size_of::<TriangleCache>());
    mix(GRAPH_COLOR_COUNT);
    mix(BODY_TYPE_COUNT);
    mix(std::mem::size_of::<usize>());
    h
}

// ---------------------------------------------------------------------------
// Primitive helpers shared by the ser/des pairs below.
// Positions and world transforms get dedicated writers: in double-precision
// (large world) mode these carry f64 translations and must widen here.
// ---------------------------------------------------------------------------

// C: b3RecW_POSITION — three floats in the float build, three doubles in the
// double precision build (wire-identical to VEC3 in float mode).
#[cfg(not(feature = "double-precision"))]
fn w_position(buf: &mut RecBuffer, p: Pos) {
    buf.w_vec3(p);
}

#[cfg(feature = "double-precision")]
fn w_position(buf: &mut RecBuffer, p: Pos) {
    buf.w_f64(p.x);
    buf.w_f64(p.y);
    buf.w_f64(p.z);
}

#[cfg(not(feature = "double-precision"))]
fn r_position(r: &mut RecCursor) -> Pos {
    r.r_vec3()
}

#[cfg(feature = "double-precision")]
fn r_position(r: &mut RecCursor) -> Pos {
    Pos {
        x: r.r_f64(),
        y: r.r_f64(),
        z: r.r_f64(),
    }
}

#[cfg(not(feature = "double-precision"))]
fn w_world_transform(buf: &mut RecBuffer, t: WorldTransform) {
    buf.w_transform(t);
}

#[cfg(feature = "double-precision")]
fn w_world_transform(buf: &mut RecBuffer, t: WorldTransform) {
    w_position(buf, t.p);
    buf.w_quat(t.q);
}

#[cfg(not(feature = "double-precision"))]
fn r_world_transform(r: &mut RecCursor) -> WorldTransform {
    r.r_transform()
}

#[cfg(feature = "double-precision")]
fn r_world_transform(r: &mut RecCursor) -> WorldTransform {
    let p = r_position(r);
    let q = r.r_quat();
    WorldTransform { p, q }
}

fn w_vec2(buf: &mut RecBuffer, v: Vec2) {
    buf.w_f32(v.x);
    buf.w_f32(v.y);
}

fn r_vec2(r: &mut RecCursor) -> Vec2 {
    let x = r.r_f32();
    let y = r.r_f32();
    Vec2 { x, y }
}

fn w_softness(buf: &mut RecBuffer, s: Softness) {
    buf.w_f32(s.bias_rate);
    buf.w_f32(s.mass_scale);
    buf.w_f32(s.impulse_scale);
}

fn r_softness(r: &mut RecCursor) -> Softness {
    Softness {
        bias_rate: r.r_f32(),
        mass_scale: r.r_f32(),
        impulse_scale: r.r_f32(),
    }
}

fn w_filter(buf: &mut RecBuffer, f: Filter) {
    buf.w_u64(f.category_bits);
    buf.w_u64(f.mask_bits);
    buf.w_i32(f.group_index);
}

fn r_filter(r: &mut RecCursor) -> Filter {
    Filter {
        category_bits: r.r_u64(),
        mask_bits: r.r_u64(),
        group_index: r.r_i32(),
    }
}

fn body_type_to_u8(t: BodyType) -> u8 {
    t as u8
}

fn body_type_from_u8(v: u8, r: &mut RecCursor) -> BodyType {
    match v {
        0 => BodyType::Static,
        1 => BodyType::Kinematic,
        2 => BodyType::Dynamic,
        _ => {
            r.ok = false;
            BodyType::Static
        }
    }
}

// C b3JointType order
fn joint_type_to_u8(t: JointType) -> u8 {
    match t {
        JointType::Parallel => 0,
        JointType::Distance => 1,
        JointType::Filter => 2,
        JointType::Motor => 3,
        JointType::Prismatic => 4,
        JointType::Revolute => 5,
        JointType::Spherical => 6,
        JointType::Weld => 7,
        JointType::Wheel => 8,
    }
}

fn joint_type_from_u8(v: u8, r: &mut RecCursor) -> JointType {
    match v {
        0 => JointType::Parallel,
        1 => JointType::Distance,
        2 => JointType::Filter,
        3 => JointType::Motor,
        4 => JointType::Prismatic,
        5 => JointType::Revolute,
        6 => JointType::Spherical,
        7 => JointType::Weld,
        8 => JointType::Wheel,
        _ => {
            r.ok = false;
            JointType::Filter
        }
    }
}

// Id pool: nextIndex + freeArray
fn ser_id_pool(buf: &mut RecBuffer, pool: &IdPool) {
    buf.w_i32(pool.next_index);
    ser_i32_array(buf, &pool.free_array);
}

fn des_id_pool(r: &mut RecCursor, pool: &mut IdPool) {
    pool.next_index = r.r_i32();
    pool.free_array = des_i32_array(r);
}

fn ser_i32_array(buf: &mut RecBuffer, arr: &[i32]) {
    buf.w_i32(arr.len() as i32);
    for v in arr {
        buf.w_i32(*v);
    }
}

fn des_i32_array(r: &mut RecCursor) -> Vec<i32> {
    let count = r.r_i32();
    if !r.check_count(count, 4) {
        return Vec::new();
    }
    let mut arr = Vec::with_capacity(count as usize);
    for _ in 0..count {
        arr.push(r.r_i32());
    }
    arr
}

// BitSet: blockCount + raw words
fn ser_bit_set(buf: &mut RecBuffer, bs: &BitSet) {
    buf.w_u32(bs.block_count);
    for k in 0..bs.block_count as usize {
        buf.w_u64(bs.bits[k]);
    }
}

fn des_bit_set(r: &mut RecCursor, bs: &mut BitSet) {
    let block_count = r.r_u32();
    if !r.check_count(block_count as i32, 8) {
        *bs = BitSet::default();
        return;
    }
    let mut bits = vec![0u64; block_count.max(1) as usize];
    for k in 0..block_count as usize {
        bits[k] = r.r_u64();
    }
    bs.bits = bits;
    bs.block_count = block_count;
}

// HashSet: capacity + count + raw items (probe order depends on layout)
fn ser_hash_set(buf: &mut RecBuffer, hs: &HashSet) {
    buf.w_u32(hs.capacity());
    buf.w_u32(hs.count);
    for item in &hs.items {
        buf.w_u64(item.key);
        buf.w_u32(item.hash);
    }
}

fn des_hash_set(r: &mut RecCursor, hs: &mut HashSet) {
    let cap = r.r_u32();
    let cnt = r.r_u32();
    let valid = r.check_count(cap as i32, 12) && (cap & cap.wrapping_sub(1)) == 0 && cnt <= cap;
    if !valid && (cap != 0 || cnt != 0) {
        r.ok = false;
        *hs = HashSet::default();
        return;
    }
    let mut items = Vec::with_capacity(cap as usize);
    for _ in 0..cap {
        let key = r.r_u64();
        let hash = r.r_u32();
        items.push(SetItem { key, hash });
    }
    hs.items = items;
    hs.count = cnt;
}

// DynamicTree: version, scalars, full node array (rebuild scratch excluded)
fn ser_tree(buf: &mut RecBuffer, tree: &DynamicTree) {
    buf.w_u64(DYNAMIC_TREE_VERSION);
    buf.w_i32(tree.root);
    buf.w_i32(tree.node_count);
    buf.w_i32(tree.node_capacity);
    buf.w_i32(tree.free_list);
    buf.w_i32(tree.proxy_count);
    buf.w_i32(tree.nodes.len() as i32);
    for n in &tree.nodes {
        ser_tree_node(buf, n);
    }
}

fn des_tree(r: &mut RecCursor, tree: &mut DynamicTree) {
    let _version = r.r_u64();
    let root = r.r_i32();
    let node_count = r.r_i32();
    let node_capacity = r.r_i32();
    let free_list = r.r_i32();
    let proxy_count = r.r_i32();
    let stored_count = r.r_i32();

    // Free existing allocation including any rebuild scratch
    *tree = DynamicTree::default();

    if !r.check_count(stored_count, 56) {
        return;
    }

    tree.root = root;
    tree.node_count = node_count;
    tree.node_capacity = node_capacity;
    tree.free_list = free_list;
    tree.proxy_count = proxy_count;
    tree.nodes.reserve(stored_count as usize);
    for _ in 0..stored_count {
        tree.nodes.push(des_tree_node(r));
    }
}

fn ser_tree_node(buf: &mut RecBuffer, n: &TreeNode) {
    buf.w_aabb(n.aabb);
    buf.w_u64(n.category_bits);
    buf.w_i32(n.children.child1);
    buf.w_i32(n.children.child2);
    buf.w_u64(n.user_data);
    buf.w_i32(n.parent);
    buf.w_u16(n.height);
    buf.w_u16(n.flags);
}

fn des_tree_node(r: &mut RecCursor) -> TreeNode {
    TreeNode {
        aabb: r.r_aabb(),
        category_bits: r.r_u64(),
        children: TreeNodeChildren { child1: r.r_i32(), child2: r.r_i32() },
        user_data: r.r_u64(),
        parent: r.r_i32(),
        height: r.r_u16(),
        flags: r.r_u16(),
    }
}

// ---------------------------------------------------------------------------
// Body / body sim / body state
// ---------------------------------------------------------------------------

fn ser_body_sim(buf: &mut RecBuffer, s: &BodySim) {
    w_world_transform(buf, s.transform);
    w_position(buf, s.center);
    buf.w_quat(s.rotation0);
    w_position(buf, s.center0);
    buf.w_vec3(s.local_center);
    buf.w_vec3(s.force);
    buf.w_vec3(s.torque);
    buf.w_f32(s.inv_mass);
    buf.w_matrix3(s.inv_inertia_local);
    buf.w_matrix3(s.inv_inertia_world);
    buf.w_f32(s.min_extent);
    buf.w_vec3(s.max_extent);
    buf.w_f32(s.max_angular_velocity);
    buf.w_f32(s.linear_damping);
    buf.w_f32(s.angular_damping);
    buf.w_f32(s.gravity_scale);
    buf.w_i32(s.body_id);
    buf.w_u32(s.flags);
}

fn des_body_sim(r: &mut RecCursor) -> BodySim {
    BodySim {
        transform: r_world_transform(r),
        center: r_position(r),
        rotation0: r.r_quat(),
        center0: r_position(r),
        local_center: r.r_vec3(),
        force: r.r_vec3(),
        torque: r.r_vec3(),
        inv_mass: r.r_f32(),
        inv_inertia_local: r.r_matrix3(),
        inv_inertia_world: r.r_matrix3(),
        min_extent: r.r_f32(),
        max_extent: r.r_vec3(),
        max_angular_velocity: r.r_f32(),
        linear_damping: r.r_f32(),
        angular_damping: r.r_f32(),
        gravity_scale: r.r_f32(),
        body_id: r.r_i32(),
        flags: r.r_u32(),
    }
}

fn ser_body_state(buf: &mut RecBuffer, s: &BodyState) {
    buf.w_vec3(s.linear_velocity);
    buf.w_vec3(s.angular_velocity);
    buf.w_vec3(s.delta_position);
    buf.w_quat(s.delta_rotation);
    buf.w_u32(s.flags);
}

fn des_body_state(r: &mut RecCursor) -> BodyState {
    BodyState {
        linear_velocity: r.r_vec3(),
        angular_velocity: r.r_vec3(),
        delta_position: r.r_vec3(),
        delta_rotation: r.r_quat(),
        flags: r.r_u32(),
    }
}

fn ser_body(buf: &mut RecBuffer, b: &Body) {
    // userData is host wiring, zero it on the copy
    buf.w_u64(0);
    buf.w_i32(b.set_index);
    buf.w_i32(b.local_index);
    buf.w_i32(b.head_contact_key);
    buf.w_i32(b.contact_count);
    buf.w_i32(b.head_shape_id);
    buf.w_i32(b.shape_count);
    buf.w_i32(b.head_chain_id);
    buf.w_i32(b.head_joint_key);
    buf.w_i32(b.joint_count);
    buf.w_i32(b.island_id);
    buf.w_i32(b.island_index);
    buf.w_f32(b.sleep_threshold);
    buf.w_f32(b.sleep_time);
    buf.w_f32(b.mass);
    buf.w_matrix3(b.inertia);
    buf.w_i32(b.body_move_index);
    buf.w_i32(b.id);
    buf.w_u32(b.flags);
    buf.w_u8(body_type_to_u8(b.body_type));
    buf.w_u16(b.generation);
    buf.w_str(&b.name);
}

fn des_body(r: &mut RecCursor) -> Body {
    let user_data = r.r_u64();
    let set_index = r.r_i32();
    let local_index = r.r_i32();
    let head_contact_key = r.r_i32();
    let contact_count = r.r_i32();
    let head_shape_id = r.r_i32();
    let shape_count = r.r_i32();
    let head_chain_id = r.r_i32();
    let head_joint_key = r.r_i32();
    let joint_count = r.r_i32();
    let island_id = r.r_i32();
    let island_index = r.r_i32();
    let sleep_threshold = r.r_f32();
    let sleep_time = r.r_f32();
    let mass = r.r_f32();
    let inertia = r.r_matrix3();
    let body_move_index = r.r_i32();
    let id = r.r_i32();
    let flags = r.r_u32();
    let body_type = {
        let v = r.r_u8();
        body_type_from_u8(v, r)
    };
    let generation = r.r_u16();
    let name = r.r_string();

    Body {
        user_data,
        set_index,
        local_index,
        head_contact_key,
        contact_count,
        head_shape_id,
        shape_count,
        head_chain_id,
        head_joint_key,
        joint_count,
        island_id,
        island_index,
        sleep_threshold,
        sleep_time,
        mass,
        inertia,
        body_move_index,
        id,
        flags,
        body_type,
        generation,
        name,
    }
}

// ---------------------------------------------------------------------------
// Joint sim (with the C union) and joint
// ---------------------------------------------------------------------------

fn ser_joint_union(buf: &mut RecBuffer, u: &JointUnion) {
    match u {
        JointUnion::Distance(j) => {
            buf.w_u8(1);
            ser_distance_joint(buf, j);
        }
        JointUnion::Motor(j) => {
            buf.w_u8(3);
            ser_motor_joint(buf, j);
        }
        JointUnion::Parallel(j) => {
            buf.w_u8(0);
            ser_parallel_joint(buf, j);
        }
        JointUnion::Revolute(j) => {
            buf.w_u8(5);
            ser_revolute_joint(buf, j);
        }
        JointUnion::Spherical(j) => {
            buf.w_u8(6);
            ser_spherical_joint(buf, j);
        }
        JointUnion::Prismatic(j) => {
            buf.w_u8(4);
            ser_prismatic_joint(buf, j);
        }
        JointUnion::Weld(j) => {
            buf.w_u8(7);
            ser_weld_joint(buf, j);
        }
        JointUnion::Wheel(j) => {
            buf.w_u8(8);
            ser_wheel_joint(buf, j);
        }
        JointUnion::Filter => {
            buf.w_u8(2);
        }
    }
}

fn des_joint_union(r: &mut RecCursor) -> JointUnion {
    let tag = r.r_u8();
    match tag {
        0 => JointUnion::Parallel(des_parallel_joint(r)),
        1 => JointUnion::Distance(des_distance_joint(r)),
        2 => JointUnion::Filter,
        3 => JointUnion::Motor(des_motor_joint(r)),
        4 => JointUnion::Prismatic(des_prismatic_joint(r)),
        5 => JointUnion::Revolute(des_revolute_joint(r)),
        6 => JointUnion::Spherical(des_spherical_joint(r)),
        7 => JointUnion::Weld(des_weld_joint(r)),
        8 => JointUnion::Wheel(des_wheel_joint(r)),
        _ => {
            r.ok = false;
            JointUnion::Filter
        }
    }
}

fn ser_distance_joint(buf: &mut RecBuffer, j: &DistanceJoint) {
    buf.w_f32(j.length);
    buf.w_f32(j.hertz);
    buf.w_f32(j.damping_ratio);
    buf.w_f32(j.lower_spring_force);
    buf.w_f32(j.upper_spring_force);
    buf.w_f32(j.min_length);
    buf.w_f32(j.max_length);
    buf.w_f32(j.max_motor_force);
    buf.w_f32(j.motor_speed);
    buf.w_f32(j.impulse);
    buf.w_f32(j.lower_impulse);
    buf.w_f32(j.upper_impulse);
    buf.w_f32(j.motor_impulse);
    buf.w_i32(j.index_a);
    buf.w_i32(j.index_b);
    buf.w_vec3(j.anchor_a);
    buf.w_vec3(j.anchor_b);
    buf.w_vec3(j.delta_center);
    w_softness(buf, j.distance_softness);
    buf.w_f32(j.axial_mass);
    buf.w_bool(j.enable_spring);
    buf.w_bool(j.enable_limit);
    buf.w_bool(j.enable_motor);
}

fn des_distance_joint(r: &mut RecCursor) -> DistanceJoint {
    DistanceJoint {
        length: r.r_f32(),
        hertz: r.r_f32(),
        damping_ratio: r.r_f32(),
        lower_spring_force: r.r_f32(),
        upper_spring_force: r.r_f32(),
        min_length: r.r_f32(),
        max_length: r.r_f32(),
        max_motor_force: r.r_f32(),
        motor_speed: r.r_f32(),
        impulse: r.r_f32(),
        lower_impulse: r.r_f32(),
        upper_impulse: r.r_f32(),
        motor_impulse: r.r_f32(),
        index_a: r.r_i32(),
        index_b: r.r_i32(),
        anchor_a: r.r_vec3(),
        anchor_b: r.r_vec3(),
        delta_center: r.r_vec3(),
        distance_softness: r_softness(r),
        axial_mass: r.r_f32(),
        enable_spring: r.r_bool(),
        enable_limit: r.r_bool(),
        enable_motor: r.r_bool(),
    }
}

fn ser_motor_joint(buf: &mut RecBuffer, j: &MotorJoint) {
    buf.w_vec3(j.linear_velocity);
    buf.w_vec3(j.angular_velocity);
    buf.w_f32(j.max_velocity_force);
    buf.w_f32(j.max_velocity_torque);
    buf.w_f32(j.linear_hertz);
    buf.w_f32(j.linear_damping_ratio);
    buf.w_f32(j.max_spring_force);
    buf.w_f32(j.angular_hertz);
    buf.w_f32(j.angular_damping_ratio);
    buf.w_f32(j.max_spring_torque);
    buf.w_vec3(j.linear_velocity_impulse);
    buf.w_vec3(j.angular_velocity_impulse);
    buf.w_vec3(j.linear_spring_impulse);
    buf.w_vec3(j.angular_spring_impulse);
    w_softness(buf, j.linear_spring);
    w_softness(buf, j.angular_spring);
    buf.w_i32(j.index_a);
    buf.w_i32(j.index_b);
    buf.w_transform(j.frame_a);
    buf.w_transform(j.frame_b);
    buf.w_vec3(j.delta_center);
    buf.w_matrix3(j.angular_mass);
}

fn des_motor_joint(r: &mut RecCursor) -> MotorJoint {
    MotorJoint {
        linear_velocity: r.r_vec3(),
        angular_velocity: r.r_vec3(),
        max_velocity_force: r.r_f32(),
        max_velocity_torque: r.r_f32(),
        linear_hertz: r.r_f32(),
        linear_damping_ratio: r.r_f32(),
        max_spring_force: r.r_f32(),
        angular_hertz: r.r_f32(),
        angular_damping_ratio: r.r_f32(),
        max_spring_torque: r.r_f32(),
        linear_velocity_impulse: r.r_vec3(),
        angular_velocity_impulse: r.r_vec3(),
        linear_spring_impulse: r.r_vec3(),
        angular_spring_impulse: r.r_vec3(),
        linear_spring: r_softness(r),
        angular_spring: r_softness(r),
        index_a: r.r_i32(),
        index_b: r.r_i32(),
        frame_a: r.r_transform(),
        frame_b: r.r_transform(),
        delta_center: r.r_vec3(),
        angular_mass: r.r_matrix3(),
    }
}

fn ser_parallel_joint(buf: &mut RecBuffer, j: &ParallelJoint) {
    buf.w_f32(j.hertz);
    buf.w_f32(j.damping_ratio);
    buf.w_f32(j.max_torque);
    w_vec2(buf, j.perp_impulse);
    buf.w_vec3(j.perp_axis_x);
    buf.w_vec3(j.perp_axis_y);
    buf.w_quat(j.quat_a);
    buf.w_quat(j.quat_b);
    buf.w_i32(j.index_a);
    buf.w_i32(j.index_b);
    w_softness(buf, j.softness);
}

fn des_parallel_joint(r: &mut RecCursor) -> ParallelJoint {
    ParallelJoint {
        hertz: r.r_f32(),
        damping_ratio: r.r_f32(),
        max_torque: r.r_f32(),
        perp_impulse: r_vec2(r),
        perp_axis_x: r.r_vec3(),
        perp_axis_y: r.r_vec3(),
        quat_a: r.r_quat(),
        quat_b: r.r_quat(),
        index_a: r.r_i32(),
        index_b: r.r_i32(),
        softness: r_softness(r),
    }
}

fn ser_prismatic_joint(buf: &mut RecBuffer, j: &PrismaticJoint) {
    w_vec2(buf, j.perp_impulse);
    buf.w_vec3(j.angular_impulse);
    buf.w_f32(j.spring_impulse);
    buf.w_f32(j.motor_impulse);
    buf.w_f32(j.lower_impulse);
    buf.w_f32(j.upper_impulse);
    buf.w_f32(j.hertz);
    buf.w_f32(j.damping_ratio);
    buf.w_f32(j.max_motor_force);
    buf.w_f32(j.motor_speed);
    buf.w_f32(j.target_translation);
    buf.w_f32(j.lower_translation);
    buf.w_f32(j.upper_translation);
    buf.w_i32(j.index_a);
    buf.w_i32(j.index_b);
    buf.w_transform(j.frame_a);
    buf.w_transform(j.frame_b);
    buf.w_vec3(j.joint_axis);
    buf.w_vec3(j.perp_axis_y);
    buf.w_vec3(j.perp_axis_z);
    buf.w_vec3(j.delta_center);
    buf.w_f32(j.delta_angle);
    buf.w_matrix3(j.rotation_mass);
    w_softness(buf, j.spring_softness);
    buf.w_bool(j.enable_spring);
    buf.w_bool(j.enable_limit);
    buf.w_bool(j.enable_motor);
}

fn des_prismatic_joint(r: &mut RecCursor) -> PrismaticJoint {
    PrismaticJoint {
        perp_impulse: r_vec2(r),
        angular_impulse: r.r_vec3(),
        spring_impulse: r.r_f32(),
        motor_impulse: r.r_f32(),
        lower_impulse: r.r_f32(),
        upper_impulse: r.r_f32(),
        hertz: r.r_f32(),
        damping_ratio: r.r_f32(),
        max_motor_force: r.r_f32(),
        motor_speed: r.r_f32(),
        target_translation: r.r_f32(),
        lower_translation: r.r_f32(),
        upper_translation: r.r_f32(),
        index_a: r.r_i32(),
        index_b: r.r_i32(),
        frame_a: r.r_transform(),
        frame_b: r.r_transform(),
        joint_axis: r.r_vec3(),
        perp_axis_y: r.r_vec3(),
        perp_axis_z: r.r_vec3(),
        delta_center: r.r_vec3(),
        delta_angle: r.r_f32(),
        rotation_mass: r.r_matrix3(),
        spring_softness: r_softness(r),
        enable_spring: r.r_bool(),
        enable_limit: r.r_bool(),
        enable_motor: r.r_bool(),
    }
}

fn ser_revolute_joint(buf: &mut RecBuffer, j: &RevoluteJoint) {
    buf.w_vec3(j.linear_impulse);
    w_vec2(buf, j.perp_impulse);
    buf.w_f32(j.spring_impulse);
    buf.w_f32(j.motor_impulse);
    buf.w_f32(j.lower_impulse);
    buf.w_f32(j.upper_impulse);
    buf.w_f32(j.hertz);
    buf.w_f32(j.damping_ratio);
    buf.w_f32(j.max_motor_torque);
    buf.w_f32(j.motor_speed);
    buf.w_f32(j.target_angle);
    buf.w_f32(j.lower_angle);
    buf.w_f32(j.upper_angle);
    buf.w_i32(j.index_a);
    buf.w_i32(j.index_b);
    buf.w_transform(j.frame_a);
    buf.w_transform(j.frame_b);
    buf.w_vec3(j.rotation_axis_z);
    buf.w_vec3(j.perp_axis_x);
    buf.w_vec3(j.perp_axis_y);
    buf.w_vec3(j.delta_center);
    buf.w_f32(j.delta_angle);
    buf.w_f32(j.axial_mass);
    w_softness(buf, j.spring_softness);
    buf.w_bool(j.enable_spring);
    buf.w_bool(j.enable_motor);
    buf.w_bool(j.enable_limit);
}

fn des_revolute_joint(r: &mut RecCursor) -> RevoluteJoint {
    RevoluteJoint {
        linear_impulse: r.r_vec3(),
        perp_impulse: r_vec2(r),
        spring_impulse: r.r_f32(),
        motor_impulse: r.r_f32(),
        lower_impulse: r.r_f32(),
        upper_impulse: r.r_f32(),
        hertz: r.r_f32(),
        damping_ratio: r.r_f32(),
        max_motor_torque: r.r_f32(),
        motor_speed: r.r_f32(),
        target_angle: r.r_f32(),
        lower_angle: r.r_f32(),
        upper_angle: r.r_f32(),
        index_a: r.r_i32(),
        index_b: r.r_i32(),
        frame_a: r.r_transform(),
        frame_b: r.r_transform(),
        rotation_axis_z: r.r_vec3(),
        perp_axis_x: r.r_vec3(),
        perp_axis_y: r.r_vec3(),
        delta_center: r.r_vec3(),
        delta_angle: r.r_f32(),
        axial_mass: r.r_f32(),
        spring_softness: r_softness(r),
        enable_spring: r.r_bool(),
        enable_motor: r.r_bool(),
        enable_limit: r.r_bool(),
    }
}

fn ser_spherical_joint(buf: &mut RecBuffer, j: &SphericalJoint) {
    buf.w_vec3(j.linear_impulse);
    buf.w_vec3(j.spring_impulse);
    buf.w_vec3(j.motor_impulse);
    buf.w_f32(j.lower_twist_impulse);
    buf.w_f32(j.upper_twist_impulse);
    buf.w_f32(j.swing_impulse);
    buf.w_f32(j.hertz);
    buf.w_f32(j.damping_ratio);
    buf.w_f32(j.max_motor_torque);
    buf.w_vec3(j.motor_velocity);
    buf.w_f32(j.lower_twist_angle);
    buf.w_f32(j.upper_twist_angle);
    buf.w_f32(j.cone_angle);
    buf.w_quat(j.target_rotation);
    buf.w_i32(j.index_a);
    buf.w_i32(j.index_b);
    buf.w_transform(j.frame_a);
    buf.w_transform(j.frame_b);
    buf.w_vec3(j.delta_center);
    buf.w_vec3(j.swing_axis);
    buf.w_vec3(j.twist_jacobian);
    buf.w_matrix3(j.rotation_mass);
    buf.w_f32(j.swing_mass);
    buf.w_f32(j.twist_mass);
    w_softness(buf, j.spring_softness);
    buf.w_bool(j.enable_spring);
    buf.w_bool(j.enable_motor);
    buf.w_bool(j.enable_cone_limit);
    buf.w_bool(j.enable_twist_limit);
}

fn des_spherical_joint(r: &mut RecCursor) -> SphericalJoint {
    SphericalJoint {
        linear_impulse: r.r_vec3(),
        spring_impulse: r.r_vec3(),
        motor_impulse: r.r_vec3(),
        lower_twist_impulse: r.r_f32(),
        upper_twist_impulse: r.r_f32(),
        swing_impulse: r.r_f32(),
        hertz: r.r_f32(),
        damping_ratio: r.r_f32(),
        max_motor_torque: r.r_f32(),
        motor_velocity: r.r_vec3(),
        lower_twist_angle: r.r_f32(),
        upper_twist_angle: r.r_f32(),
        cone_angle: r.r_f32(),
        target_rotation: r.r_quat(),
        index_a: r.r_i32(),
        index_b: r.r_i32(),
        frame_a: r.r_transform(),
        frame_b: r.r_transform(),
        delta_center: r.r_vec3(),
        swing_axis: r.r_vec3(),
        twist_jacobian: r.r_vec3(),
        rotation_mass: r.r_matrix3(),
        swing_mass: r.r_f32(),
        twist_mass: r.r_f32(),
        spring_softness: r_softness(r),
        enable_spring: r.r_bool(),
        enable_motor: r.r_bool(),
        enable_cone_limit: r.r_bool(),
        enable_twist_limit: r.r_bool(),
    }
}

fn ser_weld_joint(buf: &mut RecBuffer, j: &WeldJoint) {
    buf.w_f32(j.linear_hertz);
    buf.w_f32(j.linear_damping_ratio);
    buf.w_f32(j.angular_hertz);
    buf.w_f32(j.angular_damping_ratio);
    w_softness(buf, j.linear_spring);
    w_softness(buf, j.angular_spring);
    buf.w_vec3(j.linear_impulse);
    buf.w_vec3(j.angular_impulse);
    buf.w_i32(j.index_a);
    buf.w_i32(j.index_b);
    buf.w_transform(j.frame_a);
    buf.w_transform(j.frame_b);
    buf.w_vec3(j.delta_center);
    buf.w_matrix3(j.angular_mass);
}

fn des_weld_joint(r: &mut RecCursor) -> WeldJoint {
    WeldJoint {
        linear_hertz: r.r_f32(),
        linear_damping_ratio: r.r_f32(),
        angular_hertz: r.r_f32(),
        angular_damping_ratio: r.r_f32(),
        linear_spring: r_softness(r),
        angular_spring: r_softness(r),
        linear_impulse: r.r_vec3(),
        angular_impulse: r.r_vec3(),
        index_a: r.r_i32(),
        index_b: r.r_i32(),
        frame_a: r.r_transform(),
        frame_b: r.r_transform(),
        delta_center: r.r_vec3(),
        angular_mass: r.r_matrix3(),
    }
}

fn ser_wheel_joint(buf: &mut RecBuffer, j: &WheelJoint) {
    w_vec2(buf, j.linear_impulse);
    w_vec2(buf, j.angular_impulse);
    buf.w_f32(j.spin_impulse);
    buf.w_f32(j.max_spin_torque);
    buf.w_f32(j.spin_speed);
    buf.w_f32(j.suspension_spring_impulse);
    buf.w_f32(j.lower_suspension_impulse);
    buf.w_f32(j.upper_suspension_impulse);
    buf.w_f32(j.lower_suspension_limit);
    buf.w_f32(j.upper_suspension_limit);
    buf.w_f32(j.suspension_hertz);
    buf.w_f32(j.suspension_damping_ratio);
    buf.w_f32(j.steering_spring_impulse);
    buf.w_f32(j.lower_steering_impulse);
    buf.w_f32(j.upper_steering_impulse);
    buf.w_f32(j.lower_steering_limit);
    buf.w_f32(j.upper_steering_limit);
    buf.w_f32(j.target_steering_angle);
    buf.w_f32(j.max_steering_torque);
    buf.w_f32(j.steering_hertz);
    buf.w_f32(j.steering_damping_ratio);
    buf.w_i32(j.index_a);
    buf.w_i32(j.index_b);
    buf.w_transform(j.frame_a);
    buf.w_transform(j.frame_b);
    buf.w_vec3(j.delta_center);
    buf.w_f32(j.spin_mass);
    buf.w_f32(j.suspension_mass);
    buf.w_f32(j.steering_mass);
    w_softness(buf, j.suspension_softness);
    w_softness(buf, j.steering_softness);
    buf.w_bool(j.enable_spin_motor);
    buf.w_bool(j.enable_suspension_spring);
    buf.w_bool(j.enable_suspension_limit);
    buf.w_bool(j.enable_steering);
    buf.w_bool(j.enable_steering_limit);
    buf.w_bool(j.enable_steering_motor);
}

fn des_wheel_joint(r: &mut RecCursor) -> WheelJoint {
    WheelJoint {
        linear_impulse: r_vec2(r),
        angular_impulse: r_vec2(r),
        spin_impulse: r.r_f32(),
        max_spin_torque: r.r_f32(),
        spin_speed: r.r_f32(),
        suspension_spring_impulse: r.r_f32(),
        lower_suspension_impulse: r.r_f32(),
        upper_suspension_impulse: r.r_f32(),
        lower_suspension_limit: r.r_f32(),
        upper_suspension_limit: r.r_f32(),
        suspension_hertz: r.r_f32(),
        suspension_damping_ratio: r.r_f32(),
        steering_spring_impulse: r.r_f32(),
        lower_steering_impulse: r.r_f32(),
        upper_steering_impulse: r.r_f32(),
        lower_steering_limit: r.r_f32(),
        upper_steering_limit: r.r_f32(),
        target_steering_angle: r.r_f32(),
        max_steering_torque: r.r_f32(),
        steering_hertz: r.r_f32(),
        steering_damping_ratio: r.r_f32(),
        index_a: r.r_i32(),
        index_b: r.r_i32(),
        frame_a: r.r_transform(),
        frame_b: r.r_transform(),
        delta_center: r.r_vec3(),
        spin_mass: r.r_f32(),
        suspension_mass: r.r_f32(),
        steering_mass: r.r_f32(),
        suspension_softness: r_softness(r),
        steering_softness: r_softness(r),
        enable_spin_motor: r.r_bool(),
        enable_suspension_spring: r.r_bool(),
        enable_suspension_limit: r.r_bool(),
        enable_steering: r.r_bool(),
        enable_steering_limit: r.r_bool(),
        enable_steering_motor: r.r_bool(),
    }
}

fn ser_joint_sim(buf: &mut RecBuffer, s: &JointSim) {
    buf.w_i32(s.joint_id);
    buf.w_i32(s.body_id_a);
    buf.w_i32(s.body_id_b);
    buf.w_u8(joint_type_to_u8(s.joint_type));
    buf.w_transform(s.local_frame_a);
    buf.w_transform(s.local_frame_b);
    buf.w_f32(s.inv_mass_a);
    buf.w_f32(s.inv_mass_b);
    buf.w_matrix3(s.inv_i_a);
    buf.w_matrix3(s.inv_i_b);
    buf.w_f32(s.constraint_hertz);
    buf.w_f32(s.constraint_damping_ratio);
    w_softness(buf, s.constraint_softness);
    buf.w_f32(s.force_threshold);
    buf.w_f32(s.torque_threshold);
    buf.w_bool(s.fixed_rotation);
    ser_joint_union(buf, &s.joint);
}

fn des_joint_sim(r: &mut RecCursor) -> JointSim {
    let joint_id = r.r_i32();
    let body_id_a = r.r_i32();
    let body_id_b = r.r_i32();
    let joint_type = {
        let v = r.r_u8();
        joint_type_from_u8(v, r)
    };
    let local_frame_a = r.r_transform();
    let local_frame_b = r.r_transform();
    let inv_mass_a = r.r_f32();
    let inv_mass_b = r.r_f32();
    let inv_i_a = r.r_matrix3();
    let inv_i_b = r.r_matrix3();
    let constraint_hertz = r.r_f32();
    let constraint_damping_ratio = r.r_f32();
    let constraint_softness = r_softness(r);
    let force_threshold = r.r_f32();
    let torque_threshold = r.r_f32();
    let fixed_rotation = r.r_bool();
    let joint = des_joint_union(r);

    JointSim {
        joint_id,
        body_id_a,
        body_id_b,
        joint_type,
        local_frame_a,
        local_frame_b,
        inv_mass_a,
        inv_mass_b,
        inv_i_a,
        inv_i_b,
        constraint_hertz,
        constraint_damping_ratio,
        constraint_softness,
        force_threshold,
        torque_threshold,
        fixed_rotation,
        joint,
    }
}

fn ser_joint(buf: &mut RecBuffer, j: &Joint) {
    // userData scrubbed
    buf.w_u64(0);
    buf.w_i32(j.set_index);
    buf.w_i32(j.color_index);
    buf.w_i32(j.local_index);
    for e in &j.edges {
        buf.w_i32(e.body_id);
        buf.w_i32(e.prev_key);
        buf.w_i32(e.next_key);
    }
    buf.w_i32(j.joint_id);
    buf.w_i32(j.island_id);
    buf.w_i32(j.island_index);
    buf.w_f32(j.draw_scale);
    buf.w_u8(joint_type_to_u8(j.joint_type));
    buf.w_u16(j.generation);
    buf.w_bool(j.collide_connected);
}

fn des_joint(r: &mut RecCursor) -> Joint {
    let user_data = r.r_u64();
    let set_index = r.r_i32();
    let color_index = r.r_i32();
    let local_index = r.r_i32();
    let mut edges = [JointEdge::default(); 2];
    for e in &mut edges {
        e.body_id = r.r_i32();
        e.prev_key = r.r_i32();
        e.next_key = r.r_i32();
    }
    let joint_id = r.r_i32();
    let island_id = r.r_i32();
    let island_index = r.r_i32();
    let draw_scale = r.r_f32();
    let joint_type = {
        let v = r.r_u8();
        joint_type_from_u8(v, r)
    };
    let generation = r.r_u16();
    let collide_connected = r.r_bool();

    Joint {
        user_data,
        set_index,
        color_index,
        local_index,
        edges,
        joint_id,
        island_id,
        island_index,
        draw_scale,
        joint_type,
        generation,
        collide_connected,
    }
}

// ---------------------------------------------------------------------------
// Contacts (manifolds and mesh triangle cache ride behind the struct image)
// ---------------------------------------------------------------------------

fn ser_manifold_point(buf: &mut RecBuffer, p: &ManifoldPoint) {
    buf.w_vec3(p.anchor_a);
    buf.w_vec3(p.anchor_b);
    buf.w_f32(p.separation);
    buf.w_f32(p.base_separation);
    buf.w_f32(p.normal_impulse);
    buf.w_f32(p.total_normal_impulse);
    buf.w_f32(p.normal_velocity);
    buf.w_u32(p.feature_id);
    buf.w_i32(p.triangle_index);
    buf.w_bool(p.persisted);
}

fn des_manifold_point(r: &mut RecCursor) -> ManifoldPoint {
    ManifoldPoint {
        anchor_a: r.r_vec3(),
        anchor_b: r.r_vec3(),
        separation: r.r_f32(),
        base_separation: r.r_f32(),
        normal_impulse: r.r_f32(),
        total_normal_impulse: r.r_f32(),
        normal_velocity: r.r_f32(),
        feature_id: r.r_u32(),
        triangle_index: r.r_i32(),
        persisted: r.r_bool(),
    }
}

fn ser_manifold(buf: &mut RecBuffer, m: &Manifold) {
    for p in &m.points {
        ser_manifold_point(buf, p);
    }
    buf.w_vec3(m.normal);
    buf.w_f32(m.twist_impulse);
    buf.w_vec3(m.friction_impulse);
    buf.w_vec3(m.rolling_impulse);
    buf.w_i32(m.point_count);
}

fn des_manifold(r: &mut RecCursor) -> Manifold {
    let mut points = [ManifoldPoint::default(); MAX_MANIFOLD_POINTS];
    for p in &mut points {
        *p = des_manifold_point(r);
    }
    Manifold {
        points,
        normal: r.r_vec3(),
        twist_impulse: r.r_f32(),
        friction_impulse: r.r_vec3(),
        rolling_impulse: r.r_vec3(),
        point_count: r.r_i32(),
    }
}

fn ser_contact_cache(buf: &mut RecBuffer, c: &ContactCache) {
    buf.w_f32(c.sat_cache.separation);
    buf.w_u8(c.sat_cache.type_);
    buf.w_u8(c.sat_cache.index_a);
    buf.w_u8(c.sat_cache.index_b);
    buf.w_u8(c.sat_cache.hit);
    buf.w_f32(c.simplex_cache.metric);
    buf.w_u16(c.simplex_cache.count);
    buf.append(&c.simplex_cache.index_a);
    buf.append(&c.simplex_cache.index_b);
}

fn des_contact_cache(r: &mut RecCursor) -> ContactCache {
    let mut cache = ContactCache {
        sat_cache: SATCache {
            separation: r.r_f32(),
            type_: r.r_u8(),
            index_a: r.r_u8(),
            index_b: r.r_u8(),
            hit: r.r_u8(),
        },
        simplex_cache: SimplexCache {
            metric: r.r_f32(),
            count: r.r_u16(),
            index_a: [0; 4],
            index_b: [0; 4],
        },
    };
    r.r_bytes(&mut cache.simplex_cache.index_a);
    r.r_bytes(&mut cache.simplex_cache.index_b);
    cache
}

fn ser_contacts(buf: &mut RecBuffer, world: &World) {
    let count = world.contacts.len() as i32;
    buf.w_i32(count);

    for i in 0..count {
        let c = &world.contacts[i as usize];
        let is_live = c.contact_id == i;

        // Struct image with transient/pointer fields scrubbed
        buf.w_i32(c.set_index);
        buf.w_i32(c.color_index);
        buf.w_i32(c.local_index);
        for e in &c.edges {
            buf.w_i32(e.body_id);
            buf.w_i32(e.prev_key);
            buf.w_i32(e.next_key);
        }
        buf.w_i32(c.shape_id_a);
        buf.w_i32(c.shape_id_b);
        buf.w_i32(c.child_index);
        buf.w_i32(c.island_id);
        buf.w_i32(c.island_index);
        buf.w_i32(c.contact_id);
        buf.w_i32(NULL_INDEX); // bodySimIndexA (transient)
        buf.w_i32(NULL_INDEX); // bodySimIndexB (transient)
        buf.w_u32(c.flags);
        buf.w_quat(c.cached_rotation_a);
        buf.w_quat(c.cached_rotation_b);
        buf.w_transform(c.cached_relative_pose);
        buf.w_f32(c.friction);
        ser_contact_cache(buf, &c.convex_contact.cache);
        buf.w_aabb(c.mesh_contact.query_bounds);
        buf.w_f32(c.restitution);
        buf.w_f32(c.rolling_resistance);
        buf.w_vec3(c.tangent_velocity);
        buf.w_u32(c.generation);

        if !is_live {
            // Free slot: no heap data
            buf.w_i32(0); // manifoldCount
            continue;
        }

        // Manifolds
        buf.w_i32(c.manifolds.len() as i32);
        for m in &c.manifolds {
            ser_manifold(buf, m);
        }

        // Mesh triangleCache
        if c.flags & SIM_MESH_CONTACT != 0 {
            buf.w_i32(c.mesh_contact.triangle_cache.len() as i32);
            for t in &c.mesh_contact.triangle_cache {
                buf.w_i32(t.triangle_index);
                ser_contact_cache(buf, &t.cache);
            }
        }
    }
}

fn des_contacts(r: &mut RecCursor, world: &mut World) {
    let count = r.r_i32();
    // Each contact image is at least the fixed struct bytes.
    if !r.check_count(count, 64) {
        return;
    }

    let mut contacts: Vec<Contact> = Vec::with_capacity(count as usize);

    for i in 0..count {
        if !r.ok {
            break;
        }

        let mut c = Contact::default();
        c.set_index = r.r_i32();
        c.color_index = r.r_i32();
        c.local_index = r.r_i32();
        for k in 0..2 {
            c.edges[k] = ContactEdge {
                body_id: r.r_i32(),
                prev_key: r.r_i32(),
                next_key: r.r_i32(),
            };
        }
        c.shape_id_a = r.r_i32();
        c.shape_id_b = r.r_i32();
        c.child_index = r.r_i32();
        c.island_id = r.r_i32();
        c.island_index = r.r_i32();
        c.contact_id = r.r_i32();
        c.body_sim_index_a = r.r_i32();
        c.body_sim_index_b = r.r_i32();
        c.flags = r.r_u32();
        c.cached_rotation_a = r.r_quat();
        c.cached_rotation_b = r.r_quat();
        c.cached_relative_pose = r.r_transform();
        c.friction = r.r_f32();
        c.convex_contact = ConvexContact { cache: des_contact_cache(r) };
        c.mesh_contact.query_bounds = r.r_aabb();
        c.restitution = r.r_f32();
        c.rolling_resistance = r.r_f32();
        c.tangent_velocity = r.r_vec3();
        c.generation = r.r_u32();

        c.body_sim_index_a = NULL_INDEX;
        c.body_sim_index_b = NULL_INDEX;

        let is_live = c.contact_id == i;

        let manifold_count = r.r_i32();
        if !r.check_count(manifold_count, 32) {
            break;
        }

        if is_live && manifold_count > 0 {
            c.manifolds = crate::contact::Manifolds::with_count(manifold_count);
            for m in c.manifolds.iter_mut() {
                *m = des_manifold(r);
            }
        }

        // Mesh triangleCache
        if is_live && (c.flags & SIM_MESH_CONTACT) != 0 {
            let cache_count = r.r_i32();
            if !r.check_count(cache_count, 20) {
                break;
            }
            c.mesh_contact.triangle_cache.reserve(cache_count as usize);
            for _ in 0..cache_count {
                let triangle_index = r.r_i32();
                let cache = des_contact_cache(r);
                c.mesh_contact.triangle_cache.push(TriangleCache { triangle_index, cache });
            }
        }

        contacts.push(c);
    }

    // Commit even on failure (C reads directly into the world arrays), so a
    // partial restore leaves a consistent world for destroy_world.
    world.contacts = contacts;
}

// ---------------------------------------------------------------------------
// Shapes (geometry interned into the registry)
// ---------------------------------------------------------------------------

// C b3ShapeType values for the geometry kind on the wire
fn shape_geo_kind(geom: &ShapeGeometry) -> i32 {
    match geom {
        ShapeGeometry::Capsule(_) => 0,
        ShapeGeometry::Compound(_) => 1,
        ShapeGeometry::HeightField(_) => 2,
        ShapeGeometry::Hull(_) => 3,
        ShapeGeometry::Mesh(_) => 4,
        ShapeGeometry::Sphere(_) => 5,
    }
}

fn ser_shapes(buf: &mut RecBuffer, world: &World, rec: &mut Recording) {
    let count = world.shapes.len() as i32;
    buf.w_i32(count);

    for i in 0..count {
        let shape = &world.shapes[i as usize];
        let is_live = shape.id == i;

        // POD scalars with pointers scrubbed (generation must survive free slots)
        buf.w_i32(shape.id);
        buf.w_i32(shape.body_id);
        buf.w_i32(shape.prev_shape_id);
        buf.w_i32(shape.next_shape_id);
        buf.w_i32(shape.sensor_index);
        buf.w_i32(shape.proxy_key);
        buf.w_f32(shape.density);
        buf.w_f32(shape.explosion_scale);
        buf.w_f32(shape.aabb_margin);
        buf.w_aabb(shape.aabb);
        buf.w_aabb(shape.fat_aabb);
        buf.w_vec3(shape.local_centroid);
        buf.w_material(&shape.material);
        w_filter(buf, shape.filter);
        buf.w_u64(0); // userData scrubbed
        buf.w_u16(shape.generation);
        buf.w_bool(shape.enable_sensor_events);
        buf.w_bool(shape.enable_contact_events);
        buf.w_bool(shape.enable_custom_filtering);
        buf.w_bool(shape.enable_hit_events);
        buf.w_bool(shape.enable_pre_solve_events);
        buf.w_bool(shape.enlarged_aabb);

        if !is_live {
            // Free slot: no materials or geometry
            buf.w_i32(0); // materialCount
            buf.w_i32(-1); // geometry kind sentinel
            continue;
        }

        // Owned material array. A single material rode along inline above, so
        // write a zero length for it (C convention).
        buf.w_i32(shape.materials.len() as i32);
        for m in &shape.materials {
            buf.w_material(m);
        }

        // Geometry
        buf.w_i32(shape_geo_kind(&shape.geom));
        match &shape.geom {
            ShapeGeometry::Sphere(sphere) => {
                buf.w_vec3(sphere.center);
                buf.w_f32(sphere.radius);
            }
            ShapeGeometry::Capsule(capsule) => {
                buf.w_vec3(capsule.center1);
                buf.w_vec3(capsule.center2);
                buf.w_f32(capsule.radius);
            }
            ShapeGeometry::Hull(hull) => {
                let gid = rec_intern_hull(rec, hull);
                buf.w_u32(gid);
            }
            ShapeGeometry::Mesh(mesh) => {
                let gid = rec_intern_mesh(rec, &mesh.data);
                buf.w_u32(gid);
                buf.w_vec3(mesh.scale);
            }
            ShapeGeometry::HeightField(hf) => {
                let gid = rec_intern_height_field(rec, hf);
                buf.w_u32(gid);
            }
            ShapeGeometry::Compound(compound) => {
                let gid = rec_intern_compound(rec, compound);
                buf.w_u32(gid);
            }
        }
    }
}

fn des_shapes(r: &mut RecCursor, world: &mut World, rdr: &RecReader) {
    let count = r.r_i32();
    if !r.check_count(count, 100) {
        return;
    }

    let mut shapes: Vec<Shape> = Vec::with_capacity(count as usize);

    for i in 0..count {
        if !r.ok {
            break;
        }

        let mut shape = Shape::default();
        shape.id = r.r_i32();
        shape.body_id = r.r_i32();
        shape.prev_shape_id = r.r_i32();
        shape.next_shape_id = r.r_i32();
        shape.sensor_index = r.r_i32();
        shape.proxy_key = r.r_i32();
        shape.density = r.r_f32();
        shape.explosion_scale = r.r_f32();
        shape.aabb_margin = r.r_f32();
        shape.aabb = r.r_aabb();
        shape.fat_aabb = r.r_aabb();
        shape.local_centroid = r.r_vec3();
        shape.material = r.r_material();
        shape.filter = r_filter(r);
        shape.user_data = r.r_u64();
        shape.generation = r.r_u16();
        shape.enable_sensor_events = r.r_bool();
        shape.enable_contact_events = r.r_bool();
        shape.enable_custom_filtering = r.r_bool();
        shape.enable_hit_events = r.r_bool();
        shape.enable_pre_solve_events = r.r_bool();
        shape.enlarged_aabb = r.r_bool();

        let is_live = shape.id == i;

        // Serializer writes: matCount, matData, geoKind, geoData
        let mat_count = r.r_i32();
        if !r.ok {
            break;
        }

        if !is_live {
            // Free slot: matCount=0, geoKind=-1 (consume the sentinel)
            let _ = mat_count;
            let _ = r.r_i32();
            shapes.push(shape);
            continue;
        }

        if !r.check_count(mat_count, 36) {
            break;
        }
        shape.materials.reserve(mat_count as usize);
        for _ in 0..mat_count {
            shape.materials.push(r.r_material());
        }

        let geo_kind = r.r_i32();

        match geo_kind {
            5 => {
                // sphere
                let center = r.r_vec3();
                let radius = r.r_f32();
                shape.geom = ShapeGeometry::Sphere(crate::types::Sphere { center, radius });
            }
            0 => {
                // capsule
                let center1 = r.r_vec3();
                let center2 = r.r_vec3();
                let radius = r.r_f32();
                shape.geom = ShapeGeometry::Capsule(crate::types::Capsule { center1, center2, radius });
            }
            3 => {
                // hull: cloned into the world DB via the registry slot
                let gid = r.r_u32();
                if !r.ok || gid >= rdr.slot_count() as u32 {
                    r.ok = false;
                    break;
                }
                match &rdr.slots[gid as usize].geometry {
                    RegistryGeometry::Hull(hull) => {
                        let owned = crate::shape::add_hull_to_database(world, hull);
                        shape.geom = ShapeGeometry::Hull(owned);
                    }
                    _ => {
                        r.ok = false;
                        break;
                    }
                }
            }
            4 => {
                // mesh: self-contained blob used by reference
                let gid = r.r_u32();
                let scale = r.r_vec3();
                if !r.ok || gid >= rdr.slot_count() as u32 {
                    r.ok = false;
                    break;
                }
                match &rdr.slots[gid as usize].geometry {
                    RegistryGeometry::Mesh(mesh) => {
                        shape.geom = ShapeGeometry::Mesh(Mesh { data: Arc::clone(mesh), scale });
                    }
                    _ => {
                        r.ok = false;
                        break;
                    }
                }
            }
            2 => {
                // height field
                let gid = r.r_u32();
                if !r.ok || gid >= rdr.slot_count() as u32 {
                    r.ok = false;
                    break;
                }
                match &rdr.slots[gid as usize].geometry {
                    RegistryGeometry::HeightField(hf) => {
                        shape.geom = ShapeGeometry::HeightField(Arc::clone(hf));
                    }
                    _ => {
                        r.ok = false;
                        break;
                    }
                }
            }
            1 => {
                // compound
                let gid = r.r_u32();
                if !r.ok || gid >= rdr.slot_count() as u32 {
                    r.ok = false;
                    break;
                }
                match &rdr.slots[gid as usize].geometry {
                    RegistryGeometry::Compound(compound) => {
                        shape.geom = ShapeGeometry::Compound(Arc::clone(compound));
                    }
                    _ => {
                        r.ok = false;
                        break;
                    }
                }
            }
            _ => {
                // Unknown geometry kind means a corrupt or unsupported snapshot.
                r.ok = false;
                break;
            }
        }

        shapes.push(shape);
    }

    // Commit even on failure (C reads directly into the world arrays): the
    // hull database references taken above belong to the shapes committed
    // here, so destroy_world stays balanced after a rejected image.
    world.shapes = shapes;
}

// ---------------------------------------------------------------------------
// Solver sets, graph colors, world config
// ---------------------------------------------------------------------------

fn ser_solver_set(buf: &mut RecBuffer, set: &SolverSet) {
    buf.w_i32(set.set_index);

    buf.w_i32(set.body_sims.len() as i32);
    for s in &set.body_sims {
        ser_body_sim(buf, s);
    }
    buf.w_i32(set.body_states.len() as i32);
    for s in &set.body_states {
        ser_body_state(buf, s);
    }
    buf.w_i32(set.joint_sims.len() as i32);
    for s in &set.joint_sims {
        ser_joint_sim(buf, s);
    }
    ser_i32_array(buf, &set.contact_indices);
    buf.w_i32(set.island_sims.len() as i32);
    for s in &set.island_sims {
        buf.w_i32(s.island_id);
    }
}

fn des_solver_set(r: &mut RecCursor) -> SolverSet {
    let mut set = SolverSet::default();
    set.set_index = r.r_i32();

    let body_sim_count = r.r_i32();
    if !r.check_count(body_sim_count, 100) {
        return set;
    }
    set.body_sims.reserve(body_sim_count as usize);
    for _ in 0..body_sim_count {
        set.body_sims.push(des_body_sim(r));
    }

    let body_state_count = r.r_i32();
    if !r.check_count(body_state_count, 44) {
        return set;
    }
    set.body_states.reserve(body_state_count as usize);
    for _ in 0..body_state_count {
        set.body_states.push(des_body_state(r));
    }

    let joint_sim_count = r.r_i32();
    if !r.check_count(joint_sim_count, 100) {
        return set;
    }
    set.joint_sims.reserve(joint_sim_count as usize);
    for _ in 0..joint_sim_count {
        set.joint_sims.push(des_joint_sim(r));
    }

    set.contact_indices = des_i32_array(r);

    let island_sim_count = r.r_i32();
    if !r.check_count(island_sim_count, 4) {
        return set;
    }
    set.island_sims.reserve(island_sim_count as usize);
    for _ in 0..island_sim_count {
        set.island_sims.push(IslandSim { island_id: r.r_i32() });
    }

    set
}

// Graph color: bodySet (non-overflow only) + jointSims + convexContacts + contacts
fn ser_graph_color(buf: &mut RecBuffer, color: &GraphColor, is_overflow: bool) {
    if !is_overflow {
        ser_bit_set(buf, &color.body_set);
    }
    buf.w_i32(color.joint_sims.len() as i32);
    for s in &color.joint_sims {
        ser_joint_sim(buf, s);
    }
    ser_i32_array(buf, &color.convex_contacts);
    buf.w_i32(color.contacts.len() as i32);
    for c in &color.contacts {
        buf.w_i32(c.contact_id);
        buf.w_i32(c.manifold_start);
        buf.w_u16(c.manifold_count);
    }
    // The transient constraint ranges (C: constraint pointers) are not serialized
}

fn des_graph_color(r: &mut RecCursor, color: &mut GraphColor, is_overflow: bool) {
    // Transient constraint ranges left at shell defaults
    let body_set = std::mem::take(&mut color.body_set);
    *color = GraphColor::default();
    color.body_set = body_set;

    if !is_overflow {
        des_bit_set(r, &mut color.body_set);
    }

    let joint_sim_count = r.r_i32();
    if !r.check_count(joint_sim_count, 100) {
        return;
    }
    color.joint_sims.reserve(joint_sim_count as usize);
    for _ in 0..joint_sim_count {
        color.joint_sims.push(des_joint_sim(r));
    }

    color.convex_contacts = des_i32_array(r);

    let contact_count = r.r_i32();
    if !r.check_count(contact_count, 10) {
        return;
    }
    color.contacts.reserve(contact_count as usize);
    for _ in 0..contact_count {
        color.contacts.push(ContactSpec {
            contact_id: r.r_i32(),
            manifold_start: r.r_i32(),
            manifold_count: r.r_u16(),
        });
    }
}

// World simulation scalars (never host/callback/worker state)
fn ser_world_config(buf: &mut RecBuffer, world: &World) {
    buf.w_vec3(world.gravity);
    buf.w_f32(world.hit_event_threshold);
    buf.w_f32(world.restitution_threshold);
    buf.w_f32(world.max_linear_speed);
    buf.w_f32(world.contact_speed);
    buf.w_f32(world.contact_hertz);
    buf.w_f32(world.contact_damping_ratio);
    buf.w_f32(world.contact_recycle_distance);
    buf.w_u64(world.step_index);
    buf.w_i32(world.split_island_id);
    buf.w_f32(world.inv_h);
    buf.w_f32(world.inv_dt);
    buf.w_i32(world.end_event_array_index);
    buf.w_i32(world.max_capacity.static_shape_count);
    buf.w_i32(world.max_capacity.dynamic_shape_count);
    buf.w_i32(world.max_capacity.static_body_count);
    buf.w_i32(world.max_capacity.dynamic_body_count);
    buf.w_i32(world.max_capacity.contact_count);
    let mut flags: u8 = 0;
    flags |= if world.enable_sleep { 0x01 } else { 0 };
    flags |= if world.enable_warm_starting { 0x02 } else { 0 };
    flags |= if world.enable_continuous { 0x04 } else { 0 };
    flags |= if world.enable_speculative { 0x08 } else { 0 };
    buf.w_u8(flags);
}

fn des_world_config(r: &mut RecCursor, world: &mut World) {
    world.gravity = r.r_vec3();
    world.hit_event_threshold = r.r_f32();
    world.restitution_threshold = r.r_f32();
    world.max_linear_speed = r.r_f32();
    world.contact_speed = r.r_f32();
    world.contact_hertz = r.r_f32();
    world.contact_damping_ratio = r.r_f32();
    world.contact_recycle_distance = r.r_f32();
    world.step_index = r.r_u64();
    world.split_island_id = r.r_i32();
    world.inv_h = r.r_f32();
    world.inv_dt = r.r_f32();
    world.end_event_array_index = r.r_i32();
    world.max_capacity = Capacity {
        static_shape_count: r.r_i32(),
        dynamic_shape_count: r.r_i32(),
        static_body_count: r.r_i32(),
        dynamic_body_count: r.r_i32(),
        contact_count: r.r_i32(),
    };
    let flags = r.r_u8();
    world.enable_sleep = (flags & 0x01) != 0;
    world.enable_warm_starting = (flags & 0x02) != 0;
    world.enable_continuous = (flags & 0x04) != 0;
    world.enable_speculative = (flags & 0x08) != 0;
}

// Release per-object references that the restore will overwrite, so restoring
// over a populated world stays consistent (C: b3FreeLiveSimElements; the
// manual frees collapse to Vec/Arc drops, only the hull database reference
// counts need explicit release).
fn free_live_sim_elements(world: &mut World) {
    for i in 0..world.shapes.len() {
        if world.shapes[i].id != i as i32 {
            continue;
        }
        if let ShapeGeometry::Hull(hull) = &world.shapes[i].geom {
            let hull = Arc::clone(hull);
            crate::shape::remove_hull_from_database(world, &hull);
            // The shape's Arc itself drops with the array overwrite.
        }
    }
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// C: b3SerializeWorld. Serialize the live world into buf, interning shape
/// geometry into rec.registry. On success buf holds a self-contained snapshot
/// image (geometry lives in the registry). Returns the byte count.
pub fn serialize_world(world: &World, buf: &mut RecBuffer, rec: &mut Recording) -> i32 {
    let start_size = buf.size();

    // Snapshot header
    buf.w_u32(SNAP_MAGIC);
    buf.w_u32(SNAP_VERSION);
    buf.w_u32(compute_layout_hash());
    let mut flags: u32 = 0;
    if cfg!(debug_assertions) {
        flags |= SNAP_FLAG_VALIDATION;
    }
    if cfg!(feature = "double-precision") {
        flags |= SNAP_FLAG_DOUBLE_PRECISION;
    }
    buf.w_u32(flags);

    // World scalars
    ser_world_config(buf, world);

    // 6 id pools (Box3D has no chainIdPool)
    ser_id_pool(buf, &world.body_id_pool);
    ser_id_pool(buf, &world.shape_id_pool);
    ser_id_pool(buf, &world.contact_id_pool);
    ser_id_pool(buf, &world.joint_id_pool);
    ser_id_pool(buf, &world.island_id_pool);
    ser_id_pool(buf, &world.solver_set_id_pool);

    // Solver sets
    buf.w_i32(world.solver_sets.len() as i32);
    for set in &world.solver_sets {
        ser_solver_set(buf, set);
    }

    // Sparse body array (userData is host wiring, zeroed on the copy)
    buf.w_i32(world.bodies.len() as i32);
    for body in &world.bodies {
        ser_body(buf, body);
    }

    // Shape sparse array with geometry interning
    ser_shapes(buf, world, rec);

    // Contact sparse array with manifolds and mesh triangleCache
    ser_contacts(buf, world);

    // Joint sparse array (userData scrubbed)
    buf.w_i32(world.joints.len() as i32);
    for joint in &world.joints {
        ser_joint(buf, joint);
    }

    // Sensors: shapeId + 3 inner arrays each
    buf.w_i32(world.sensors.len() as i32);
    for s in &world.sensors {
        buf.w_i32(s.shape_id);
        ser_visitor_array(buf, &s.hits);
        ser_visitor_array(buf, &s.overlaps1);
        ser_visitor_array(buf, &s.overlaps2);
    }

    // Islands: 4 scalars + 3 inner arrays each
    buf.w_i32(world.islands.len() as i32);
    for island in &world.islands {
        buf.w_i32(island.set_index);
        buf.w_i32(island.local_index);
        buf.w_i32(island.island_id);
        buf.w_i32(island.constraint_remove_count);
        ser_i32_array(buf, &island.bodies);
        buf.w_i32(island.contacts.len() as i32);
        for link in &island.contacts {
            buf.w_i32(link.contact_id);
            buf.w_i32(link.body_id_a);
            buf.w_i32(link.body_id_b);
        }
        buf.w_i32(island.joints.len() as i32);
        for link in &island.joints {
            buf.w_i32(link.joint_id);
            buf.w_i32(link.body_id_a);
            buf.w_i32(link.body_id_b);
        }
    }

    // Broad phase
    let bp = &world.broad_phase;
    for t in 0..BODY_TYPE_COUNT {
        ser_tree(buf, &bp.trees[t]);
    }
    for t in 0..BODY_TYPE_COUNT {
        ser_bit_set(buf, &bp.moved_proxies[t]);
    }
    ser_i32_array(buf, &bp.move_array);
    ser_hash_set(buf, &bp.pair_set);

    // Constraint graph
    for c in 0..GRAPH_COLOR_COUNT {
        ser_graph_color(buf, &world.constraint_graph.colors[c], c == OVERFLOW_INDEX as usize);
    }

    (buf.size() - start_size) as i32
}

fn ser_visitor_array(buf: &mut RecBuffer, arr: &[Visitor]) {
    buf.w_i32(arr.len() as i32);
    for v in arr {
        buf.w_i32(v.shape_id);
        buf.w_u16(v.generation);
    }
}

fn des_visitor_array(r: &mut RecCursor) -> Vec<Visitor> {
    let count = r.r_i32();
    if !r.check_count(count, 6) {
        return Vec::new();
    }
    let mut arr = Vec::with_capacity(count as usize);
    for _ in 0..count {
        arr.push(Visitor { shape_id: r.r_i32(), generation: r.r_u16() });
    }
    arr
}

/// C: b3DeserializeIntoShell. Overwrite a freshly-created (shell) world with
/// the simulation state held in the snapshot image. Geometry references are
/// resolved via the shared registry slots in rdr. Returns false on a corrupt
/// or incompatible image.
pub fn deserialize_into_shell(data: &[u8], world: &mut World, rdr: &RecReader) -> bool {
    if data.len() < 16 {
        return false;
    }

    // Validate header
    let mut r = RecCursor::new(data);
    let magic = r.r_u32();
    let version = r.r_u32();
    let layout_hash = r.r_u32();
    let flags = r.r_u32();
    if magic != SNAP_MAGIC || version != SNAP_VERSION {
        crate::core::log("deserialize_into_shell: bad magic/version");
        return false;
    }
    let image_double = (flags & SNAP_FLAG_DOUBLE_PRECISION) != 0;
    let build_double = cfg!(feature = "double-precision");
    if image_double != build_double {
        crate::core::log("deserialize_into_shell: precision mismatch");
        return false;
    }
    if layout_hash != compute_layout_hash() {
        crate::core::log("deserialize_into_shell: layout hash mismatch");
        return false;
    }

    // Free existing per-object references before overwriting
    free_live_sim_elements(world);

    // 1. World scalars
    des_world_config(&mut r, world);

    // 2. 6 id pools
    des_id_pool(&mut r, &mut world.body_id_pool);
    des_id_pool(&mut r, &mut world.shape_id_pool);
    des_id_pool(&mut r, &mut world.contact_id_pool);
    des_id_pool(&mut r, &mut world.joint_id_pool);
    des_id_pool(&mut r, &mut world.island_id_pool);
    des_id_pool(&mut r, &mut world.solver_set_id_pool);

    // 3. Solver sets
    let set_count = r.r_i32();
    if !r.check_count(set_count, 24) {
        return false;
    }
    if r.ok {
        let mut sets = Vec::with_capacity(set_count as usize);
        for _ in 0..set_count {
            sets.push(des_solver_set(&mut r));
        }
        world.solver_sets = sets;
    }

    if !r.ok {
        return false;
    }

    // 4. Body sparse array
    {
        let body_count = r.r_i32();
        if !r.check_count(body_count, 90) {
            return false;
        }
        let mut bodies = Vec::with_capacity(body_count as usize);
        for _ in 0..body_count {
            bodies.push(des_body(&mut r));
        }
        if r.ok {
            world.bodies = bodies;
        }
    }

    if !r.ok {
        return false;
    }

    // 5. Shape sparse array
    des_shapes(&mut r, world, rdr);

    if !r.ok {
        return false;
    }

    // 6. Contact sparse array
    des_contacts(&mut r, world);

    if !r.ok {
        return false;
    }

    // 7. Joint sparse array
    {
        let joint_count = r.r_i32();
        if !r.check_count(joint_count, 50) {
            return false;
        }
        let mut joints = Vec::with_capacity(joint_count as usize);
        for _ in 0..joint_count {
            joints.push(des_joint(&mut r));
        }
        if r.ok {
            world.joints = joints;
        }
    }

    // 8. Sensors
    {
        let sensor_count = r.r_i32();
        if !r.check_count(sensor_count, 16) {
            return false;
        }
        let mut sensors = Vec::with_capacity(sensor_count as usize);
        for _ in 0..sensor_count {
            if !r.ok {
                break;
            }
            let shape_id = r.r_i32();
            let hits = des_visitor_array(&mut r);
            let overlaps1 = des_visitor_array(&mut r);
            let overlaps2 = des_visitor_array(&mut r);
            sensors.push(Sensor { hits, overlaps1, overlaps2, shape_id });
        }
        if r.ok {
            world.sensors = sensors;
        }
    }

    // 9. Islands
    {
        let island_count = r.r_i32();
        if !r.check_count(island_count, 28) {
            return false;
        }
        let mut islands = Vec::with_capacity(island_count as usize);
        for _ in 0..island_count {
            if !r.ok {
                break;
            }
            let mut island = Island::default();
            island.set_index = r.r_i32();
            island.local_index = r.r_i32();
            island.island_id = r.r_i32();
            island.constraint_remove_count = r.r_i32();
            island.bodies = des_i32_array(&mut r);

            let contact_count = r.r_i32();
            if !r.check_count(contact_count, 12) {
                return false;
            }
            island.contacts.reserve(contact_count as usize);
            for _ in 0..contact_count {
                island.contacts.push(ContactLink {
                    contact_id: r.r_i32(),
                    body_id_a: r.r_i32(),
                    body_id_b: r.r_i32(),
                });
            }

            let joint_count = r.r_i32();
            if !r.check_count(joint_count, 12) {
                return false;
            }
            island.joints.reserve(joint_count as usize);
            for _ in 0..joint_count {
                island.joints.push(JointLink {
                    joint_id: r.r_i32(),
                    body_id_a: r.r_i32(),
                    body_id_b: r.r_i32(),
                });
            }

            islands.push(island);
        }
        if r.ok {
            world.islands = islands;
        }
    }

    // 10. Broad phase
    {
        for t in 0..BODY_TYPE_COUNT {
            let mut tree = std::mem::take(&mut world.broad_phase.trees[t]);
            des_tree(&mut r, &mut tree);
            world.broad_phase.trees[t] = tree;
        }
        for t in 0..BODY_TYPE_COUNT {
            let mut set = std::mem::take(&mut world.broad_phase.moved_proxies[t]);
            des_bit_set(&mut r, &mut set);
            world.broad_phase.moved_proxies[t] = set;
        }
        world.broad_phase.move_array = des_i32_array(&mut r);
        let mut pair_set = std::mem::take(&mut world.broad_phase.pair_set);
        des_hash_set(&mut r, &mut pair_set);
        world.broad_phase.pair_set = pair_set;
        // Transient move_results stay at the shell's empty state
        world.broad_phase.move_results = Vec::new();
    }

    // 11. Constraint graph
    for c in 0..GRAPH_COLOR_COUNT {
        let is_overflow = c == OVERFLOW_INDEX as usize;
        let mut color = std::mem::take(&mut world.constraint_graph.colors[c]);
        des_graph_color(&mut r, &mut color, is_overflow);
        world.constraint_graph.colors[c] = color;
    }

    // No validation here: contact body-sim indices are transient (nulled in the
    // image, C does the same) and only become consistent again after the first
    // collide pass. C likewise returns without validating.

    r.ok
}
