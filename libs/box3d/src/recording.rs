// Port of box3d/src/recording.h + recording.c — SUBSTRATE SUBSET ONLY.
//
// This module ports what world snapshots need from the recording layer:
// - RecBuffer: growable append-only byte buffer with typed little-endian writers
// - hash64_blob: the FNV-1a + finalizer content hash
// - GeometryRegistry: content-interned geometry blobs referenced by u32 id
// - rec_intern_hull/mesh/height_field/compound + write_registry
//
// NOT ported (op-stream recording/replay): record framing (BeginRecord /
// EndRecord / CommitRecord), opcodes and the recording_ops.inl codegen, query
// writers/trampolines, tags, the file header, bounds accumulation, mutexes
// (the port is single threaded), and the recording lifecycle hooks.
//
// Deviations:
// - The C geometry "bytes" are raw memcpys of the single-allocation blobs.
//   The Rust geometry structs are Vec/Arc-based, so each kind serializes to a
//   canonical little-endian byte form (ser_* below, readers in
//   recording_replay.rs). The byte format is port-specific; images are not
//   interchangeable with C recordings.
// - Compound blobs inline their unique hulls/meshes once (by Arc identity)
//   with per-instance indices, so Arc sharing is restored exactly.
// - Writer functions are RecBuffer methods (buf.w_u32(v)) instead of free
//   functions; names otherwise follow the C b3RecW_* set.

use std::sync::Arc;

use crate::core::NULL_INDEX;
use crate::math_functions::{Matrix3, Quat, Transform, Vec3, AABB};
use crate::types::{
    CompoundData, HeightFieldData, HullData, MeshData, MeshNode, MeshTriangle, SurfaceMaterial,
};

// Growable append-only byte buffer. Doubles on demand. count_only mode tallies
// size without storing, used to size a buffer cheaply before a filling pass.
#[derive(Clone, Debug, Default)]
pub struct RecBuffer {
    pub data: Vec<u8>,
    pub count_only_size: usize,
    pub count_only: bool,
}

impl RecBuffer {
    pub fn new() -> RecBuffer {
        RecBuffer::default()
    }

    /// C: b3RecBufAppend
    #[inline]
    pub fn append(&mut self, bytes: &[u8]) {
        if self.count_only {
            self.count_only_size += bytes.len();
        } else {
            self.data.extend_from_slice(bytes);
        }
    }

    /// C: buf->size
    #[inline]
    pub fn size(&self) -> usize {
        if self.count_only {
            self.count_only_size
        } else {
            self.data.len()
        }
    }

    /// C: b3RecBufFree
    pub fn free(&mut self) {
        self.data = Vec::new();
        self.count_only_size = 0;
    }

    // Write primitives (C: b3RecW_*)

    #[inline]
    pub fn w_u8(&mut self, v: u8) {
        self.append(&[v]);
    }

    #[inline]
    pub fn w_u16(&mut self, v: u16) {
        self.append(&v.to_le_bytes());
    }

    #[inline]
    pub fn w_u32(&mut self, v: u32) {
        self.append(&v.to_le_bytes());
    }

    #[inline]
    pub fn w_u64(&mut self, v: u64) {
        self.append(&v.to_le_bytes());
    }

    #[inline]
    pub fn w_i32(&mut self, v: i32) {
        self.append(&v.to_le_bytes());
    }

    #[inline]
    pub fn w_f32(&mut self, v: f32) {
        self.append(&v.to_le_bytes());
    }

    #[inline]
    pub fn w_f64(&mut self, v: f64) {
        self.append(&v.to_le_bytes());
    }

    #[inline]
    pub fn w_bool(&mut self, v: bool) {
        self.w_u8(if v { 1 } else { 0 });
    }

    #[inline]
    pub fn w_vec3(&mut self, v: Vec3) {
        self.w_f32(v.x);
        self.w_f32(v.y);
        self.w_f32(v.z);
    }

    #[inline]
    pub fn w_quat(&mut self, q: Quat) {
        self.w_vec3(q.v);
        self.w_f32(q.s);
    }

    #[inline]
    pub fn w_transform(&mut self, t: Transform) {
        self.w_vec3(t.p);
        self.w_quat(t.q);
    }

    #[inline]
    pub fn w_matrix3(&mut self, m: Matrix3) {
        self.w_vec3(m.cx);
        self.w_vec3(m.cy);
        self.w_vec3(m.cz);
    }

    #[inline]
    pub fn w_aabb(&mut self, a: AABB) {
        self.w_vec3(a.lower_bound);
        self.w_vec3(a.upper_bound);
    }

    /// C: b3RecW_STR — length-prefixed, no null terminator on the wire.
    #[inline]
    pub fn w_str(&mut self, s: &str) {
        let bytes = s.as_bytes();
        self.w_u32(bytes.len() as u32);
        self.append(bytes);
    }

    /// C: b3RecW_MATERIAL
    #[inline]
    pub fn w_material(&mut self, m: &SurfaceMaterial) {
        self.w_f32(m.friction);
        self.w_f32(m.restitution);
        self.w_f32(m.rolling_resistance);
        self.w_vec3(m.tangent_velocity);
        self.w_u64(m.user_material_id);
        self.w_u32(m.custom_color);
    }
}

/// C: b3Hash64Blob — FNV-1a over 8-byte words (little-endian value semantics)
/// with a splitmix-style finalizer.
pub fn hash64_blob(bytes: &[u8]) -> u64 {
    let n = bytes.len();
    let mut h: u64 = 0xcbf29ce484222325 ^ (n as u32 as u64);
    let prime: u64 = 0x100000001b3;
    let mut i = 0usize;

    while i + 8 <= n {
        let word = u64::from_le_bytes(bytes[i..i + 8].try_into().unwrap());
        h = (h ^ word).wrapping_mul(prime);
        i += 8;
    }

    while i < n {
        h = (h ^ bytes[i] as u64).wrapping_mul(prime);
        i += 1;
    }

    h ^= h >> 30;
    h = h.wrapping_mul(0xbf58476d1ce4e5b9);
    h ^= h >> 27;
    h = h.wrapping_mul(0x94d049bb133111eb);
    h ^= h >> 31;
    h
}

/// Geometry kinds for the trailing registry section.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GeometryKind {
    Hull = 0,
    Mesh = 1,
    HeightField = 2,
    Compound = 3,
}

pub fn geometry_kind_from_u8(v: u8) -> Option<GeometryKind> {
    match v {
        0 => Some(GeometryKind::Hull),
        1 => Some(GeometryKind::Mesh),
        2 => Some(GeometryKind::HeightField),
        3 => Some(GeometryKind::Compound),
        _ => None,
    }
}

/// One entry per unique geometry blob. id == index in the entries array.
/// hash_next chains entries that share a content hash so dedup stays exact
/// under a hash collision. NULL_INDEX ends the chain.
#[derive(Clone, Debug)]
pub struct GeometryEntry {
    pub content_hash: u64,
    pub id: u32,
    pub kind: GeometryKind,
    pub bytes: Vec<u8>,
    pub hash_next: i32,
}

/// Growable array of geometry entries. Ids are array indices, so the array is
/// serialized in order. dedup_map maps a content hash to the chain head id.
#[derive(Clone, Debug, Default)]
pub struct GeometryRegistry {
    pub entries: Vec<GeometryEntry>,
    pub dedup_map: std::collections::HashMap<u64, u32>,
}

// Append a fresh entry and splice it onto the front of its hash chain.
fn registry_push(
    reg: &mut GeometryRegistry,
    kind: GeometryKind,
    content_hash: u64,
    bytes: Vec<u8>,
) -> u32 {
    let id = reg.entries.len() as u32;
    let hash_next = match reg.dedup_map.get(&content_hash) {
        Some(&head) => head as i32,
        None => NULL_INDEX,
    };
    reg.entries.push(GeometryEntry { content_hash, id, kind, bytes, hash_next });
    reg.dedup_map.insert(content_hash, id);
    id
}

/// C: b3InternGeometry. Walk every entry sharing this hash so a collision
/// still finds the identical blob.
pub fn intern_geometry(reg: &mut GeometryRegistry, kind: GeometryKind, content_hash: u64, bytes: Vec<u8>) -> u32 {
    if let Some(&head) = reg.dedup_map.get(&content_hash) {
        let mut idx = head as i32;
        while idx != NULL_INDEX {
            let e = &reg.entries[idx as usize];
            if e.bytes == bytes {
                // Duplicate: return the existing id
                return e.id;
            }
            idx = e.hash_next;
        }
    }

    registry_push(reg, kind, content_hash, bytes)
}

/// C: b3AppendGeometry. Never deduplicates, so a keyframe seed can mirror
/// slots 1:1 even with byte-identical duplicate slots.
pub fn append_geometry(reg: &mut GeometryRegistry, kind: GeometryKind, content_hash: u64, bytes: Vec<u8>) -> u32 {
    registry_push(reg, kind, content_hash, bytes)
}

/// C: b3FreeRegistry
pub fn free_registry(reg: &mut GeometryRegistry) {
    reg.entries = Vec::new();
    reg.dedup_map = std::collections::HashMap::new();
}

/// The recording: op-stream buffer + geometry registry + query tags + bounds.
/// (C b3Recording; the mutex is a RefCell at the World field, see the
/// op-stream section below.)
#[derive(Clone, Debug, Default)]
pub struct Recording {
    pub buffer: RecBuffer,
    pub registry: GeometryRegistry,

    /// Offset of the 3-byte size field for the u24 backpatch (C: recordStart).
    pub record_start: usize,

    /// Interned query tags accumulated during capture, written to the tail of
    /// the registry block at stop. tag_map maps a tag key to its index.
    pub tags: Vec<RecTag>,
    pub tag_map: std::collections::HashMap<u64, u32>,

    /// Union of world bounds over every recorded step, written at stop.
    pub accumulated_bounds: AABB,
    pub have_bounds: bool,
}

/// Query tag from QueryFilter (C: b3RecTag). Stored once per key in the
/// trailing block so a tagged query carries only the 8 byte key on the wire.
#[derive(Clone, Debug, Default)]
pub struct RecTag {
    /// hash of (id, name)
    pub key: u64,
    /// entity/actor id
    pub id: u64,
    /// query label
    pub name: String,
}

impl Recording {
    pub fn new() -> Recording {
        Recording::default()
    }
}

/// C: b3RecWriteRegistry — u32 entryCount then per-entry
/// { u8 kind, u32 byteCount, bytes }, followed by the query-tag table
/// (always empty in the port: u32 0).
pub fn write_registry(rec: &mut Recording) {
    let count = rec.registry.entries.len() as u32;
    rec.buffer.w_u32(count);
    for i in 0..rec.registry.entries.len() {
        let (kind, byte_count) = {
            let e = &rec.registry.entries[i];
            (e.kind, e.bytes.len() as u32)
        };
        rec.buffer.w_u8(kind as u8);
        rec.buffer.w_u32(byte_count);
        let bytes = std::mem::take(&mut rec.registry.entries[i].bytes);
        rec.buffer.append(&bytes);
        rec.registry.entries[i].bytes = bytes;
    }

    // Query-tag table: { u32 tagCount, per-tag u64 key, u64 id, STR name }.
    // Empty for standalone snapshots, populated during op-stream capture.
    rec.buffer.w_u32(rec.tags.len() as u32);
    for i in 0..rec.tags.len() {
        let (key, id) = (rec.tags[i].key, rec.tags[i].id);
        let name = std::mem::take(&mut rec.tags[i].name);
        rec.buffer.w_u64(key);
        rec.buffer.w_u64(id);
        rec.buffer.w_str(&name);
        rec.tags[i].name = name;
    }
}

// ---------------------------------------------------------------------------
// Canonical geometry serialization (readers in recording_replay.rs).
// The stored content hash fields ride along so restore is bit-faithful and
// no private hashing helpers are needed cross-module.
// ---------------------------------------------------------------------------

pub(crate) fn ser_hull_data(buf: &mut RecBuffer, hull: &HullData) {
    buf.w_u32(hull.hash);
    buf.w_aabb(hull.aabb);
    buf.w_f32(hull.surface_area);
    buf.w_f32(hull.volume);
    buf.w_f32(hull.inner_radius);
    buf.w_vec3(hull.center);
    buf.w_matrix3(hull.central_inertia);

    buf.w_i32(hull.vertices.len() as i32);
    for v in &hull.vertices {
        buf.w_u8(v.edge);
    }
    buf.w_i32(hull.points.len() as i32);
    for p in &hull.points {
        buf.w_vec3(*p);
    }
    buf.w_i32(hull.edges.len() as i32);
    for e in &hull.edges {
        buf.w_u8(e.next);
        buf.w_u8(e.twin);
        buf.w_u8(e.origin);
        buf.w_u8(e.face);
    }
    buf.w_i32(hull.faces.len() as i32);
    for f in &hull.faces {
        buf.w_u8(f.edge);
    }
    buf.w_i32(hull.planes.len() as i32);
    for p in &hull.planes {
        buf.w_vec3(p.normal);
        buf.w_f32(p.offset);
    }
}

pub(crate) fn ser_mesh_data(buf: &mut RecBuffer, mesh: &MeshData) {
    buf.w_u32(mesh.hash);
    buf.w_aabb(mesh.bounds);
    buf.w_f32(mesh.surface_area);
    buf.w_i32(mesh.tree_height);
    buf.w_i32(mesh.degenerate_count);

    buf.w_i32(mesh.nodes.len() as i32);
    for n in &mesh.nodes {
        ser_mesh_node(buf, n);
    }
    buf.w_i32(mesh.vertices.len() as i32);
    for v in &mesh.vertices {
        buf.w_vec3(*v);
    }
    buf.w_i32(mesh.triangles.len() as i32);
    for t in &mesh.triangles {
        ser_mesh_triangle(buf, t);
    }
    buf.w_i32(mesh.materials.len() as i32);
    buf.append(&mesh.materials);
    buf.w_i32(mesh.flags.len() as i32);
    buf.append(&mesh.flags);
}

fn ser_mesh_node(buf: &mut RecBuffer, n: &MeshNode) {
    buf.w_vec3(n.lower_bound);
    buf.w_u32(n.data);
    buf.w_vec3(n.upper_bound);
    buf.w_u32(n.triangle_offset);
}

fn ser_mesh_triangle(buf: &mut RecBuffer, t: &MeshTriangle) {
    buf.w_i32(t.index1);
    buf.w_i32(t.index2);
    buf.w_i32(t.index3);
}

pub(crate) fn ser_height_field_data(buf: &mut RecBuffer, hf: &HeightFieldData) {
    buf.w_u32(hf.hash);
    buf.w_aabb(hf.aabb);
    buf.w_f32(hf.min_height);
    buf.w_f32(hf.max_height);
    buf.w_f32(hf.height_scale);
    buf.w_vec3(hf.scale);
    buf.w_i32(hf.column_count);
    buf.w_i32(hf.row_count);

    buf.w_i32(hf.heights.len() as i32);
    for h in &hf.heights {
        buf.w_u16(*h);
    }
    buf.w_i32(hf.materials.len() as i32);
    buf.append(&hf.materials);
    buf.w_i32(hf.flags.len() as i32);
    buf.append(&hf.flags);
    buf.w_bool(hf.clockwise);
}

// The compound blob is self-contained like C's ConvertCompoundToBytes image:
// the immutable tree, materials, child arrays, plus the unique nested
// hulls/meshes written once (Arc identity) with per-instance indices.
pub(crate) fn ser_compound_data(buf: &mut RecBuffer, compound: &CompoundData) {
    // Tree (rebuild scratch excluded; it is empty on a baked compound tree)
    let tree = &compound.tree;
    buf.w_i32(tree.root);
    buf.w_i32(tree.node_count);
    buf.w_i32(tree.node_capacity);
    buf.w_i32(tree.free_list);
    buf.w_i32(tree.proxy_count);
    buf.w_i32(tree.nodes.len() as i32);
    for n in &tree.nodes {
        buf.w_aabb(n.aabb);
        buf.w_u64(n.category_bits);
        buf.w_i32(n.children.child1);
        buf.w_i32(n.children.child2);
        buf.w_u64(n.user_data);
        buf.w_i32(n.parent);
        buf.w_u16(n.height);
        buf.w_u16(n.flags);
    }

    // Materials
    buf.w_i32(compound.materials.len() as i32);
    for m in &compound.materials {
        buf.w_material(m);
    }

    // Capsules
    buf.w_i32(compound.capsules.len() as i32);
    for c in &compound.capsules {
        buf.w_vec3(c.capsule.center1);
        buf.w_vec3(c.capsule.center2);
        buf.w_f32(c.capsule.radius);
        buf.w_i32(c.material_index);
    }

    // Unique hulls (Arc identity), then hull instances by unique index
    let mut unique_hulls: Vec<&Arc<HullData>> = Vec::new();
    for h in &compound.hulls {
        if !unique_hulls.iter().any(|u| Arc::ptr_eq(u, &h.hull)) {
            unique_hulls.push(&h.hull);
        }
    }
    buf.w_i32(unique_hulls.len() as i32);
    for u in &unique_hulls {
        ser_hull_data(buf, u);
    }
    buf.w_i32(compound.hulls.len() as i32);
    for h in &compound.hulls {
        let unique_index = unique_hulls.iter().position(|u| Arc::ptr_eq(u, &h.hull)).unwrap() as i32;
        buf.w_i32(unique_index);
        buf.w_transform(h.transform);
        buf.w_i32(h.material_index);
    }
    buf.w_i32(compound.shared_hull_count);

    // Unique meshes, then mesh instances
    let mut unique_meshes: Vec<&Arc<MeshData>> = Vec::new();
    for m in &compound.meshes {
        if !unique_meshes.iter().any(|u| Arc::ptr_eq(u, &m.mesh_data)) {
            unique_meshes.push(&m.mesh_data);
        }
    }
    buf.w_i32(unique_meshes.len() as i32);
    for u in &unique_meshes {
        ser_mesh_data(buf, u);
    }
    buf.w_i32(compound.meshes.len() as i32);
    for m in &compound.meshes {
        let unique_index = unique_meshes.iter().position(|u| Arc::ptr_eq(u, &m.mesh_data)).unwrap() as i32;
        buf.w_i32(unique_index);
        buf.w_transform(m.transform);
        buf.w_vec3(m.scale);
        for k in 0..crate::types::MAX_COMPOUND_MESH_MATERIALS {
            buf.w_i32(m.material_indices[k]);
        }
    }
    buf.w_i32(compound.shared_mesh_count);

    // Spheres
    buf.w_i32(compound.spheres.len() as i32);
    for s in &compound.spheres {
        buf.w_vec3(s.sphere.center);
        buf.w_f32(s.sphere.radius);
        buf.w_i32(s.material_index);
    }
}

// ---------------------------------------------------------------------------
// Geometry interning (C: b3RecInternHull etc.)
// ---------------------------------------------------------------------------

pub fn rec_intern_hull(rec: &mut Recording, hull: &Arc<HullData>) -> u32 {
    let mut buf = RecBuffer::new();
    ser_hull_data(&mut buf, hull);
    let bytes = buf.data;
    let h = hash64_blob(&bytes);
    intern_geometry(&mut rec.registry, GeometryKind::Hull, h, bytes)
}

pub fn rec_intern_mesh(rec: &mut Recording, mesh: &Arc<MeshData>) -> u32 {
    let mut buf = RecBuffer::new();
    ser_mesh_data(&mut buf, mesh);
    let bytes = buf.data;
    let h = hash64_blob(&bytes);
    intern_geometry(&mut rec.registry, GeometryKind::Mesh, h, bytes)
}

pub fn rec_intern_height_field(rec: &mut Recording, hf: &Arc<HeightFieldData>) -> u32 {
    let mut buf = RecBuffer::new();
    ser_height_field_data(&mut buf, hf);
    let bytes = buf.data;
    let h = hash64_blob(&bytes);
    intern_geometry(&mut rec.registry, GeometryKind::HeightField, h, bytes)
}

pub fn rec_intern_compound(rec: &mut Recording, compound: &Arc<CompoundData>) -> u32 {
    let mut buf = RecBuffer::new();
    ser_compound_data(&mut buf, compound);
    let bytes = buf.data;
    let h = hash64_blob(&bytes);
    intern_geometry(&mut rec.registry, GeometryKind::Compound, h, bytes)
}

// ===========================================================================
// Op-stream capture (port of the rest of recording.h/.c)
// ===========================================================================
//
// Format note: the logical structure mirrors C exactly — 48-byte header,
// snapshot seed blob, framed records (u8 opcode + u24 payload size + payload),
// trailing registry block (geometry entries + query-tag table) located by
// backpatched header offsets. The byte encoding uses this port's primitives
// (notably: strings are u32 length prefixed everywhere, geometry blobs are the
// port's canonical serialization, and the snapshot seed is the port snapshot
// format), so recordings are port-specific and NOT interchangeable with C
// recording files. Opcode values and record layouts otherwise follow
// recording_ops.inl so a faithful port of recording_replay.c can read them.
//
// Threading: C serializes record writes with a mutex because queries may run
// on user threads. The port stores the recording in a RefCell; queries take
// &World which is !Sync (the world holds raw task pointers), so concurrent
// query recording is unreachable in safe Rust and the RefCell suffices.

use std::cell::RefCell;

use crate::id::{store_body_id, store_joint_id, store_shape_id, store_world_id, BodyId, JointId, ShapeId, WorldId};
use crate::math_functions::{aabb_union, Pos, WorldTransform};
use crate::physics_world::World;
use crate::types::{
    BodyDef, Capsule, DistanceJointDef, ExplosionDef, Filter, FilterJointDef, JointDef, MassData,
    MotionLocks, MotorJointDef, ParallelJointDef, PlaneResult, PrismaticJointDef, QueryFilter,
    RayResult, RevoluteJointDef, ShapeDef, ShapeProxy, Sphere, SphericalJointDef, TreeStats,
    WeldJointDef, WheelJointDef,
};

// FNV-1a 64-bit constants (C: B3_SNAP_FNV_INIT / B3_SNAP_FNV_PRIME)
pub const SNAP_FNV_INIT: u64 = 14695981039346656037;
pub const SNAP_FNV_PRIME: u64 = 1099511628211;

// Magic 'B3RC' in little-endian (C: B3_REC_MAGIC)
pub const REC_MAGIC: u32 = 0x43523342;
pub const REC_VERSION_MAJOR: u16 = 2;
pub const REC_VERSION_MINOR: u16 = 2;

/// Fixed header size (C: sizeof(b3RecHeader) == 48).
pub const REC_HEADER_SIZE: usize = 48;
// Header field offsets used for backpatching.
pub const REC_HDR_SNAPSHOT_SIZE_OFFSET: usize = 24;
pub const REC_HDR_REGISTRY_OFFSET_OFFSET: usize = 32;
pub const REC_HDR_REGISTRY_BYTE_COUNT_OFFSET: usize = 40;

/// Mix a world position at full width so the determinism gate validates past
/// float precision when the body is far from the origin. (C: b3FnvMixPosition)
#[cfg(not(feature = "double-precision"))]
pub fn fnv_mix_position(mut hash: u64, p: Pos) -> u64 {
    let bx = p.x.to_bits() as u64;
    let by = p.y.to_bits() as u64;
    let bz = p.z.to_bits() as u64;
    hash = (hash ^ bx).wrapping_mul(SNAP_FNV_PRIME);
    hash = (hash ^ by).wrapping_mul(SNAP_FNV_PRIME);
    hash = (hash ^ bz).wrapping_mul(SNAP_FNV_PRIME);
    hash
}

#[cfg(feature = "double-precision")]
pub fn fnv_mix_position(mut hash: u64, p: Pos) -> u64 {
    let bx = p.x.to_bits();
    let by = p.y.to_bits();
    let bz = p.z.to_bits();
    hash = (hash ^ bx).wrapping_mul(SNAP_FNV_PRIME);
    hash = (hash ^ by).wrapping_mul(SNAP_FNV_PRIME);
    hash = (hash ^ bz).wrapping_mul(SNAP_FNV_PRIME);
    hash
}

// ---------------------------------------------------------------------------
// Opcodes (values from recording_ops.inl — keep in sync with the C manifest)
// ---------------------------------------------------------------------------

#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RecOp {
    DestroyWorld = 0x01,
    WorldEnableSleeping = 0x02,
    WorldEnableContinuous = 0x03,
    WorldSetRestitutionThreshold = 0x04,
    WorldSetHitEventThreshold = 0x05,
    WorldSetGravity = 0x06,
    WorldExplode = 0x07,
    WorldSetContactTuning = 0x08,
    WorldSetContactRecycleDistance = 0x09,
    WorldSetMaximumLinearSpeed = 0x0A,
    WorldEnableWarmStarting = 0x0B,
    WorldRebuildStaticTree = 0x0C,
    WorldEnableSpeculative = 0x0D,

    CreateBody = 0x10,
    DestroyBody = 0x11,
    BodySetTransform = 0x20,
    BodySetLinearVelocity = 0x21,
    BodySetType = 0x22,
    BodySetName = 0x23,
    BodySetAngularVelocity = 0x24,
    BodySetTargetTransform = 0x25,
    BodyApplyForce = 0x26,
    BodyApplyForceToCenter = 0x27,
    BodyApplyTorque = 0x28,
    BodyApplyLinearImpulse = 0x29,
    BodyApplyLinearImpulseToCenter = 0x2A,
    BodyApplyAngularImpulse = 0x2B,
    BodySetMassData = 0x2C,
    BodyApplyMassFromShapes = 0x2D,
    BodySetLinearDamping = 0x2E,
    BodySetAngularDamping = 0x2F,
    BodySetGravityScale = 0x30,
    BodySetAwake = 0x31,
    BodyEnableSleep = 0x32,
    BodySetSleepThreshold = 0x33,
    BodyDisable = 0x34,
    BodyEnable = 0x35,
    BodySetMotionLocks = 0x36,
    BodySetBullet = 0x37,
    BodyEnableContactRecycling = 0x38,
    BodyEnableHitEvents = 0x39,

    CreateSphereShape = 0x40,
    CreateCapsuleShape = 0x41,
    CreateHullShape = 0x42,
    CreateMeshShape = 0x43,
    CreateHeightFieldShape = 0x44,
    CreateCompoundShape = 0x45,
    DestroyShape = 0x46,

    ShapeSetDensity = 0x50,
    ShapeSetFriction = 0x51,
    ShapeSetRestitution = 0x52,
    ShapeSetSurfaceMaterial = 0x53,
    ShapeSetFilter = 0x54,
    ShapeEnableSensorEvents = 0x55,
    ShapeEnableContactEvents = 0x56,
    ShapeEnablePreSolveEvents = 0x57,
    ShapeEnableHitEvents = 0x58,
    ShapeSetSphere = 0x59,
    ShapeSetCapsule = 0x5A,
    ShapeApplyWind = 0x5B,

    Step = 0x80,

    CreateParallelJoint = 0x90,
    CreateDistanceJoint = 0x91,
    CreateFilterJoint = 0x92,
    CreateMotorJoint = 0x93,
    CreatePrismaticJoint = 0x94,
    CreateRevoluteJoint = 0x95,
    CreateSphericalJoint = 0x96,
    CreateWeldJoint = 0x97,
    CreateWheelJoint = 0x98,
    DestroyJoint = 0x99,

    JointSetLocalFrameA = 0x9A,
    JointSetLocalFrameB = 0x9B,
    JointSetCollideConnected = 0x9C,
    JointWakeBodies = 0x9D,
    JointSetConstraintTuning = 0x9E,
    JointSetForceThreshold = 0x9F,
    JointSetTorqueThreshold = 0xA0,

    ParallelJointSetSpringHertz = 0xA1,
    ParallelJointSetSpringDampingRatio = 0xA2,
    ParallelJointSetMaxTorque = 0xA3,

    DistanceJointSetLength = 0xA4,
    DistanceJointEnableSpring = 0xA5,
    DistanceJointSetSpringForceRange = 0xA6,
    DistanceJointSetSpringHertz = 0xA7,
    DistanceJointSetSpringDampingRatio = 0xA8,
    DistanceJointEnableLimit = 0xA9,
    DistanceJointSetLengthRange = 0xAA,
    DistanceJointEnableMotor = 0xAB,
    DistanceJointSetMotorSpeed = 0xAC,
    DistanceJointSetMaxMotorForce = 0xAD,

    MotorJointSetLinearVelocity = 0xAE,
    MotorJointSetAngularVelocity = 0xAF,
    MotorJointSetMaxVelocityForce = 0xB0,
    MotorJointSetMaxVelocityTorque = 0xB1,
    MotorJointSetLinearHertz = 0xB2,
    MotorJointSetLinearDampingRatio = 0xB3,
    MotorJointSetAngularHertz = 0xB4,
    MotorJointSetAngularDampingRatio = 0xB5,
    MotorJointSetMaxSpringForce = 0xB6,
    MotorJointSetMaxSpringTorque = 0xB7,

    PrismaticJointEnableSpring = 0xB8,
    PrismaticJointSetSpringHertz = 0xB9,
    PrismaticJointSetSpringDampingRatio = 0xBA,
    PrismaticJointSetTargetTranslation = 0xBB,
    PrismaticJointEnableLimit = 0xBC,
    PrismaticJointSetLimits = 0xBD,
    PrismaticJointEnableMotor = 0xBE,
    PrismaticJointSetMotorSpeed = 0xBF,
    PrismaticJointSetMaxMotorForce = 0xC0,

    RevoluteJointEnableSpring = 0xC1,
    RevoluteJointSetSpringHertz = 0xC2,
    RevoluteJointSetSpringDampingRatio = 0xC3,
    RevoluteJointSetTargetAngle = 0xC4,
    RevoluteJointEnableLimit = 0xC5,
    RevoluteJointSetLimits = 0xC6,
    RevoluteJointEnableMotor = 0xC7,
    RevoluteJointSetMotorSpeed = 0xC8,
    RevoluteJointSetMaxMotorTorque = 0xC9,

    SphericalJointEnableConeLimit = 0xCA,
    SphericalJointSetConeLimit = 0xCB,
    SphericalJointEnableTwistLimit = 0xCC,
    SphericalJointSetTwistLimits = 0xCD,
    SphericalJointEnableSpring = 0xCE,
    SphericalJointSetSpringHertz = 0xCF,
    SphericalJointSetSpringDampingRatio = 0xD0,
    SphericalJointSetTargetRotation = 0xD1,
    SphericalJointEnableMotor = 0xD2,
    SphericalJointSetMotorVelocity = 0xD3,
    SphericalJointSetMaxMotorTorque = 0xD4,

    WeldJointSetLinearHertz = 0xD5,
    WeldJointSetLinearDampingRatio = 0xD6,
    WeldJointSetAngularHertz = 0xD7,
    WeldJointSetAngularDampingRatio = 0xD8,

    WheelJointEnableSuspension = 0xD9,
    WheelJointSetSuspensionHertz = 0xDA,
    WheelJointSetSuspensionDampingRatio = 0xDB,
    WheelJointEnableSuspensionLimit = 0xDC,
    WheelJointSetSuspensionLimits = 0xDD,
    WheelJointEnableSpinMotor = 0xDE,
    WheelJointSetSpinMotorSpeed = 0xDF,
    WheelJointSetMaxSpinTorque = 0xE0,
    WheelJointEnableSteering = 0xE1,
    WheelJointSetSteeringHertz = 0xE2,
    WheelJointSetSteeringDampingRatio = 0xE3,
    WheelJointSetMaxSteeringTorque = 0xE4,
    WheelJointEnableSteeringLimit = 0xE5,
    WheelJointSetSteeringLimits = 0xE6,
    WheelJointSetTargetSteeringAngle = 0xE7,

    QueryOverlapAABB = 0xE8,
    QueryOverlapShape = 0xE9,
    QueryCastRay = 0xEA,
    QueryCastShape = 0xEB,
    QueryCastRayClosest = 0xEC,
    QueryCastMover = 0xED,
    QueryCollideMover = 0xEE,
    QueryTag = 0xEF,

    StateHash = 0xF1,
    RecordingBounds = 0xF2,
}

// ---------------------------------------------------------------------------
// Write primitives for the op stream (C: the remaining b3RecW_* set)
// ---------------------------------------------------------------------------

impl RecBuffer {
    /// C: b3RecW_POSITION — full precision so recordings reproduce far from
    /// the origin. In the float build this is wire-identical to VEC3.
    #[inline]
    pub fn w_position(&mut self, p: Pos) {
        #[cfg(not(feature = "double-precision"))]
        {
            self.w_f32(p.x);
            self.w_f32(p.y);
            self.w_f32(p.z);
        }
        #[cfg(feature = "double-precision")]
        {
            self.w_f64(p.x);
            self.w_f64(p.y);
            self.w_f64(p.z);
        }
    }

    /// C: b3RecW_WORLDXF
    #[inline]
    pub fn w_worldxf(&mut self, t: WorldTransform) {
        self.w_position(t.p);
        self.w_quat(t.q);
    }

    /// C: b3RecW_QUERYFILTER — category/mask only; id/name go to the tag table.
    #[inline]
    pub fn w_queryfilter(&mut self, f: QueryFilter) {
        self.w_u64(f.category_bits);
        self.w_u64(f.mask_bits);
    }

    /// C: b3RecW_SHAPEPROXY — count, points, radius (variable length).
    pub fn w_shapeproxy(&mut self, proxy: &ShapeProxy) {
        let mut count = proxy.count();
        if count < 0 {
            count = 0;
        }
        if count > crate::constants::MAX_SHAPE_CAST_POINTS as i32 {
            count = crate::constants::MAX_SHAPE_CAST_POINTS as i32;
        }
        self.w_i32(count);
        for i in 0..count {
            self.w_vec3(proxy.points[i as usize]);
        }
        self.w_f32(proxy.radius);
    }

    /// C: b3RecW_TREESTATS
    #[inline]
    pub fn w_treestats(&mut self, s: TreeStats) {
        self.w_i32(s.node_visits);
        self.w_i32(s.leaf_visits);
    }

    /// C: b3RecW_RAYRESULT
    pub fn w_rayresult(&mut self, r: &RayResult) {
        self.w_shapeid(r.shape_id);
        self.w_position(r.point);
        self.w_vec3(r.normal);
        self.w_u64(r.user_material_id);
        self.w_f32(r.fraction);
        self.w_i32(r.triangle_index);
        self.w_i32(r.child_index);
        self.w_bool(r.hit);
    }

    /// C: b3RecW_PLANERESULT
    #[inline]
    pub fn w_planeresult(&mut self, p: &PlaneResult) {
        self.w_vec3(p.plane.normal);
        self.w_f32(p.plane.offset);
        self.w_vec3(p.point);
    }

    /// C: b3RecW_WORLDID
    #[inline]
    pub fn w_worldid(&mut self, id: WorldId) {
        self.w_u32(store_world_id(id));
    }

    /// C: b3RecW_BODYID
    #[inline]
    pub fn w_bodyid(&mut self, id: BodyId) {
        self.w_u64(store_body_id(id));
    }

    /// C: b3RecW_SHAPEID
    #[inline]
    pub fn w_shapeid(&mut self, id: ShapeId) {
        self.w_u64(store_shape_id(id));
    }

    /// C: b3RecW_JOINTID
    #[inline]
    pub fn w_jointid(&mut self, id: JointId) {
        self.w_u64(store_joint_id(id));
    }

    /// C: b3RecW_SPHERE (POD memcpy in C; identical LE bytes here)
    #[inline]
    pub fn w_sphere(&mut self, s: &Sphere) {
        self.w_vec3(s.center);
        self.w_f32(s.radius);
    }

    /// C: b3RecW_CAPSULE
    #[inline]
    pub fn w_capsule(&mut self, c: &Capsule) {
        self.w_vec3(c.center1);
        self.w_vec3(c.center2);
        self.w_f32(c.radius);
    }

    /// C: b3RecW_GEOMID
    #[inline]
    pub fn w_geomid(&mut self, v: u32) {
        self.w_u32(v);
    }

    /// C: b3RecW_FILTER
    #[inline]
    pub fn w_filter(&mut self, f: Filter) {
        self.w_u64(f.category_bits);
        self.w_u64(f.mask_bits);
        self.w_i32(f.group_index);
    }

    /// C: b3RecW_MASSDATA
    #[inline]
    pub fn w_massdata(&mut self, m: &MassData) {
        self.w_f32(m.mass);
        self.w_vec3(m.center);
        self.w_matrix3(m.inertia);
    }

    /// C: b3RecW_LOCKS
    #[inline]
    pub fn w_locks(&mut self, l: MotionLocks) {
        self.w_bool(l.linear_x);
        self.w_bool(l.linear_y);
        self.w_bool(l.linear_z);
        self.w_bool(l.angular_x);
        self.w_bool(l.angular_y);
        self.w_bool(l.angular_z);
    }

    /// C: b3RecReserveU32
    pub fn reserve_u32(&mut self) -> usize {
        let offset = self.size();
        self.w_u32(0);
        offset
    }

    /// C: b3RecPatchU32
    pub fn patch_u32(&mut self, offset: usize, v: u32) {
        crate::b3_assert!(offset + 4 <= self.data.len());
        self.data[offset..offset + 4].copy_from_slice(&v.to_le_bytes());
    }

    // Def writers. userData is not preserved (written as 0), the internal
    // cookie is omitted — readers start from default_*_def() like C.

    /// C: b3RecW_EXPLOSIONDEF
    pub fn w_explosiondef(&mut self, v: &ExplosionDef) {
        self.w_u64(v.mask_bits);
        self.w_position(v.position);
        self.w_f32(v.radius);
        self.w_f32(v.falloff);
        self.w_f32(v.impulse_per_area);
    }

    /// C: b3RecW_BODYDEF
    pub fn w_bodydef(&mut self, v: &BodyDef) {
        self.w_i32(v.body_type as i32);
        self.w_position(v.position);
        self.w_quat(v.rotation);
        self.w_vec3(v.linear_velocity);
        self.w_vec3(v.angular_velocity);
        self.w_f32(v.linear_damping);
        self.w_f32(v.angular_damping);
        self.w_f32(v.gravity_scale);
        self.w_f32(v.sleep_threshold);
        self.w_str(&v.name);
        // userData: not preserved
        self.w_u64(0);
        self.w_locks(v.motion_locks);
        self.w_bool(v.enable_sleep);
        self.w_bool(v.is_awake);
        self.w_bool(v.is_bullet);
        self.w_bool(v.is_enabled);
        self.w_bool(v.allow_fast_rotation);
        self.w_bool(v.enable_contact_recycling);
        // internal_value omitted
    }

    /// C: b3RecW_SHAPEDEF
    pub fn w_shapedef(&mut self, v: &ShapeDef) {
        // userData: not preserved
        self.w_u64(0);
        // Per-triangle materials: length-prefixed so the reader can rebuild the array.
        self.w_i32(v.materials.len() as i32);
        for m in &v.materials {
            self.w_material(m);
        }
        self.w_material(&v.base_material);
        self.w_f32(v.density);
        self.w_f32(v.explosion_scale);
        self.w_filter(v.filter);
        self.w_bool(v.enable_custom_filtering);
        self.w_bool(v.is_sensor);
        self.w_bool(v.enable_sensor_events);
        self.w_bool(v.enable_contact_events);
        self.w_bool(v.enable_hit_events);
        self.w_bool(v.enable_pre_solve_events);
        self.w_bool(v.invoke_contact_creation);
        self.w_bool(v.update_body_mass);
        // internal_value omitted
    }

    /// C: b3RecW_JointBase — body ids are written as packed ids for replay remapping.
    pub fn w_joint_base(&mut self, base: &JointDef) {
        // userData: not preserved
        self.w_u64(0);
        self.w_bodyid(base.body_id_a);
        self.w_bodyid(base.body_id_b);
        self.w_transform(base.local_frame_a);
        self.w_transform(base.local_frame_b);
        self.w_f32(base.force_threshold);
        self.w_f32(base.torque_threshold);
        self.w_f32(base.constraint_hertz);
        self.w_f32(base.constraint_damping_ratio);
        self.w_f32(base.draw_scale);
        self.w_bool(base.collide_connected);
        // internal_value omitted
    }

    /// C: b3RecW_PARALLELJOINTDEF
    pub fn w_paralleljointdef(&mut self, v: &ParallelJointDef) {
        self.w_joint_base(&v.base);
        self.w_f32(v.hertz);
        self.w_f32(v.damping_ratio);
        self.w_f32(v.max_torque);
    }

    /// C: b3RecW_DISTANCEJOINTDEF
    pub fn w_distancejointdef(&mut self, v: &DistanceJointDef) {
        self.w_joint_base(&v.base);
        self.w_f32(v.length);
        self.w_bool(v.enable_spring);
        self.w_f32(v.lower_spring_force);
        self.w_f32(v.upper_spring_force);
        self.w_f32(v.hertz);
        self.w_f32(v.damping_ratio);
        self.w_bool(v.enable_limit);
        self.w_f32(v.min_length);
        self.w_f32(v.max_length);
        self.w_bool(v.enable_motor);
        self.w_f32(v.max_motor_force);
        self.w_f32(v.motor_speed);
    }

    /// C: b3RecW_FILTERJOINTDEF
    pub fn w_filterjointdef(&mut self, v: &FilterJointDef) {
        self.w_joint_base(&v.base);
    }

    /// C: b3RecW_MOTORJOINTDEF
    pub fn w_motorjointdef(&mut self, v: &MotorJointDef) {
        self.w_joint_base(&v.base);
        self.w_vec3(v.linear_velocity);
        self.w_f32(v.max_velocity_force);
        self.w_vec3(v.angular_velocity);
        self.w_f32(v.max_velocity_torque);
        self.w_f32(v.linear_hertz);
        self.w_f32(v.linear_damping_ratio);
        self.w_f32(v.max_spring_force);
        self.w_f32(v.angular_hertz);
        self.w_f32(v.angular_damping_ratio);
        self.w_f32(v.max_spring_torque);
    }

    /// C: b3RecW_PRISMATICJOINTDEF
    pub fn w_prismaticjointdef(&mut self, v: &PrismaticJointDef) {
        self.w_joint_base(&v.base);
        self.w_bool(v.enable_spring);
        self.w_f32(v.hertz);
        self.w_f32(v.damping_ratio);
        self.w_f32(v.target_translation);
        self.w_bool(v.enable_limit);
        self.w_f32(v.lower_translation);
        self.w_f32(v.upper_translation);
        self.w_bool(v.enable_motor);
        self.w_f32(v.max_motor_force);
        self.w_f32(v.motor_speed);
    }

    /// C: b3RecW_REVOLUTEJOINTDEF
    pub fn w_revolutejointdef(&mut self, v: &RevoluteJointDef) {
        self.w_joint_base(&v.base);
        self.w_f32(v.target_angle);
        self.w_bool(v.enable_spring);
        self.w_f32(v.hertz);
        self.w_f32(v.damping_ratio);
        self.w_bool(v.enable_limit);
        self.w_f32(v.lower_angle);
        self.w_f32(v.upper_angle);
        self.w_bool(v.enable_motor);
        self.w_f32(v.max_motor_torque);
        self.w_f32(v.motor_speed);
    }

    /// C: b3RecW_SPHERICALJOINTDEF
    pub fn w_sphericaljointdef(&mut self, v: &SphericalJointDef) {
        self.w_joint_base(&v.base);
        self.w_bool(v.enable_spring);
        self.w_f32(v.hertz);
        self.w_f32(v.damping_ratio);
        self.w_quat(v.target_rotation);
        self.w_bool(v.enable_cone_limit);
        self.w_f32(v.cone_angle);
        self.w_bool(v.enable_twist_limit);
        self.w_f32(v.lower_twist_angle);
        self.w_f32(v.upper_twist_angle);
        self.w_bool(v.enable_motor);
        self.w_f32(v.max_motor_torque);
        self.w_vec3(v.motor_velocity);
    }

    /// C: b3RecW_WELDJOINTDEF
    pub fn w_weldjointdef(&mut self, v: &WeldJointDef) {
        self.w_joint_base(&v.base);
        self.w_f32(v.linear_hertz);
        self.w_f32(v.angular_hertz);
        self.w_f32(v.linear_damping_ratio);
        self.w_f32(v.angular_damping_ratio);
    }

    /// C: b3RecW_WHEELJOINTDEF
    pub fn w_wheeljointdef(&mut self, v: &WheelJointDef) {
        self.w_joint_base(&v.base);
        self.w_bool(v.enable_suspension_spring);
        self.w_f32(v.suspension_hertz);
        self.w_f32(v.suspension_damping_ratio);
        self.w_bool(v.enable_suspension_limit);
        self.w_f32(v.lower_suspension_limit);
        self.w_f32(v.upper_suspension_limit);
        self.w_bool(v.enable_spin_motor);
        self.w_f32(v.max_spin_torque);
        self.w_f32(v.spin_speed);
        self.w_bool(v.enable_steering);
        self.w_f32(v.steering_hertz);
        self.w_f32(v.steering_damping_ratio);
        self.w_f32(v.target_steering_angle);
        self.w_f32(v.max_steering_torque);
        self.w_bool(v.enable_steering_limit);
        self.w_f32(v.lower_steering_limit);
        self.w_f32(v.upper_steering_limit);
    }
}

// ---------------------------------------------------------------------------
// Record framing (C: b3RecBeginRecord/b3RecEndRecord/b3RecCommitRecord)
// ---------------------------------------------------------------------------

/// Frame start: u8 opcode + reserved u24 payload size (backpatched at end).
pub fn rec_begin_record(rec: &mut Recording, opcode: u8) {
    rec.buffer.w_u8(opcode);
    rec.record_start = rec.buffer.size();
    rec.buffer.append(&[0, 0, 0]);
}

pub fn rec_end_record(rec: &mut Recording) {
    let payload_size = rec.buffer.size() - rec.record_start - 3;
    crate::b3_assert!(payload_size < (1 << 24));
    let p = rec.record_start;
    rec.buffer.data[p] = payload_size as u8;
    rec.buffer.data[p + 1] = (payload_size >> 8) as u8;
    rec.buffer.data[p + 2] = (payload_size >> 16) as u8;
}

/// Frame and append one complete record.
pub fn rec_commit_record(rec: &mut Recording, opcode: u8, payload: &[u8]) {
    crate::b3_assert!(payload.len() < (1 << 24));
    rec.buffer.w_u8(opcode);
    let n = payload.len();
    rec.buffer.append(&[n as u8, (n >> 8) as u8, (n >> 16) as u8]);
    rec.buffer.append(payload);
}

// ---------------------------------------------------------------------------
// Capture hooks (C: the B3_REC / B3_REC_CREATE macros)
// ---------------------------------------------------------------------------

/// The world id C writes into records: { worldId + 1, generation }.
#[inline]
pub fn rec_world_id(world: &World) -> WorldId {
    WorldId { index1: world.world_id + 1, generation: world.generation }
}

/// True when a recording session is active. The C hook macros branch on
/// world->recording != NULL; call sites use this to skip argument setup.
#[inline(always)]
pub fn rec_active(world: &World) -> bool {
    world.recording.is_some()
}

/// Run `f` against the active recording, if any (C: the body of B3_REC).
#[inline(always)]
pub fn with_recording(world: &World, f: impl FnOnce(&mut Recording)) {
    if let Some(cell) = &world.recording {
        f(&mut cell.borrow_mut());
    }
}

/// Record one framed op (C: B3_REC / B3_REC_CREATE — the create id is written
/// by the closure as its last field, matching b3RecWriteRet_*).
#[inline(always)]
pub fn rec_op(world: &World, op: RecOp, write_args: impl FnOnce(&mut RecBuffer)) {
    if let Some(cell) = &world.recording {
        let rec = &mut *cell.borrow_mut();
        rec_begin_record(rec, op as u8);
        write_args(&mut rec.buffer);
        rec_end_record(rec);
    }
}

// ---------------------------------------------------------------------------
// Query tags (C: b3HashQueryTag / b3RecInternTag)
// ---------------------------------------------------------------------------

/// Hash a query (id, name) pair into the stable key the viewer tracks the
/// query by. Never returns 0, so the key doubles as a tagged/untagged flag.
pub fn hash_query_tag(id: u64, name: &str) -> u64 {
    let mut h = SNAP_FNV_INIT;
    for i in 0..8 {
        h = (h ^ ((id >> (8 * i)) & 0xFF)).wrapping_mul(SNAP_FNV_PRIME);
    }
    for &b in name.as_bytes() {
        h = (h ^ b as u64).wrapping_mul(SNAP_FNV_PRIME);
    }
    if h != 0 {
        h
    } else {
        1
    }
}

/// Record a key->(id, name) mapping once, deduped by key (first id/name wins).
/// The name is clamped to BODY_NAME_LENGTH like C.
pub fn rec_intern_tag(rec: &mut Recording, key: u64, id: u64, name: &str) {
    if rec.tag_map.contains_key(&key) {
        return;
    }
    let index = rec.tags.len() as u32;
    let mut clamped = String::new();
    for (n, ch) in name.chars().enumerate() {
        if n >= crate::constants::BODY_NAME_LENGTH {
            break;
        }
        clamped.push(ch);
    }
    rec.tags.push(RecTag { key, id, name: clamped });
    rec.tag_map.insert(key, index);
}

// ---------------------------------------------------------------------------
// Query recording (C: b3RecQueryWriter + trampolines)
//
// A query collects a variable number of hits through a user callback, so the
// count is not known until the callback stops firing. The record is built in a
// local buffer with a reserved hit-count slot, then committed whole. The C
// trampolines become wrapper closures at the query call sites; the per-hit
// write helpers live here so the payload layout stays in one place.
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct RecQueryWriter {
    /// per-call local payload
    pub buf: RecBuffer,
    /// offset of the reserved u32 hit-count slot
    pub count_offset: usize,
    pub hit_count: u32,
    /// caller query id, 0 = untagged
    pub tag_id: u64,
    /// caller query name, interned by key ("" = none)
    pub tag_name: &'static str,
}

impl RecQueryWriter {
    pub fn begin(tag_id: u64, tag_name: &'static str) -> RecQueryWriter {
        RecQueryWriter { tag_id, tag_name, ..Default::default() }
    }

    /// C: b3RecOverlapTrampoline payload (also used by the mover filter).
    #[inline]
    pub fn write_overlap_hit(&mut self, id: ShapeId, ret: bool) {
        self.buf.w_shapeid(id);
        self.buf.w_bool(ret);
        self.hit_count += 1;
    }

    /// C: b3RecCastTrampoline payload.
    #[inline]
    pub fn write_cast_hit(
        &mut self,
        id: ShapeId,
        point: Pos,
        normal: crate::math_functions::Vec3,
        fraction: f32,
        user_material_id: u64,
        triangle_index: i32,
        child_index: i32,
        ret: f32,
    ) {
        self.buf.w_shapeid(id);
        self.buf.w_position(point);
        self.buf.w_vec3(normal);
        self.buf.w_f32(fraction);
        self.buf.w_u64(user_material_id);
        self.buf.w_i32(triangle_index);
        self.buf.w_i32(child_index);
        self.buf.w_f32(ret);
        self.hit_count += 1;
    }

    /// C: b3RecPlaneTrampoline payload — one hit per shape, all planes batched.
    #[inline]
    pub fn write_plane_hit(&mut self, id: ShapeId, planes: &[PlaneResult], ret: bool) {
        self.buf.w_shapeid(id);
        self.buf.w_i32(planes.len() as i32);
        for p in planes {
            self.buf.w_planeresult(p);
        }
        self.buf.w_bool(ret);
        self.hit_count += 1;
    }
}

/// C: b3RecQueryCommit — a tagged query writes its identity key right before
/// the query record so the pair stays adjacent.
pub fn rec_query_commit(world: &World, op: RecOp, w: RecQueryWriter) {
    if let Some(cell) = &world.recording {
        let rec = &mut *cell.borrow_mut();
        let tagged = w.tag_id != 0 || !w.tag_name.is_empty();
        if tagged {
            let key = hash_query_tag(w.tag_id, w.tag_name);
            rec_intern_tag(rec, key, w.tag_id, w.tag_name);
            let mut tag_buf = RecBuffer::new();
            tag_buf.w_u64(key);
            rec_commit_record(rec, RecOp::QueryTag as u8, &tag_buf.data);
        }
        rec_commit_record(rec, op as u8, &w.buf.data);
    }
}

/// Fold one step's world bounds into the running union (C: b3RecAccumulateBounds).
pub fn rec_accumulate_bounds(rec: &mut Recording, bounds: AABB) {
    rec.accumulated_bounds =
        if rec.have_bounds { aabb_union(rec.accumulated_bounds, bounds) } else { bounds };
    rec.have_bounds = true;
}

// ---------------------------------------------------------------------------
// Deterministic world state hash (C: b3HashWorldState)
// Called by both recorder and replayer to verify simulation reproduces exactly.
// ---------------------------------------------------------------------------

pub fn hash_world_state(world: &World) -> u64 {
    let mut hash = SNAP_FNV_INIT;
    let prime = SNAP_FNV_PRIME;

    let body_count = world.bodies.len();
    for i in 0..body_count {
        let body = &world.bodies[i];
        if body.id != i as i32 {
            // Free or never-used slot
            continue;
        }

        let set = &world.solver_sets[body.set_index as usize];
        let sim = &set.body_sims[body.local_index as usize];

        hash = fnv_mix_position(hash, sim.transform.p);
        hash = (hash ^ sim.transform.q.v.x.to_bits() as u64).wrapping_mul(prime);
        hash = (hash ^ sim.transform.q.v.y.to_bits() as u64).wrapping_mul(prime);
        hash = (hash ^ sim.transform.q.v.z.to_bits() as u64).wrapping_mul(prime);
        hash = (hash ^ sim.transform.q.s.to_bits() as u64).wrapping_mul(prime);

        // Body state only exists in the awake set (C: b3GetBodyState).
        if body.set_index == crate::physics_world::AWAKE_SET {
            let state = &set.body_states[body.local_index as usize];
            hash = (hash ^ state.linear_velocity.x.to_bits() as u64).wrapping_mul(prime);
            hash = (hash ^ state.linear_velocity.y.to_bits() as u64).wrapping_mul(prime);
            hash = (hash ^ state.linear_velocity.z.to_bits() as u64).wrapping_mul(prime);
            hash = (hash ^ state.angular_velocity.x.to_bits() as u64).wrapping_mul(prime);
            hash = (hash ^ state.angular_velocity.y.to_bits() as u64).wrapping_mul(prime);
            hash = (hash ^ state.angular_velocity.z.to_bits() as u64).wrapping_mul(prime);
        }
    }

    hash
}

// ---------------------------------------------------------------------------
// Recording lifecycle (C: b3StartRecordingIntoBuffer / b3StopRecordingInternal
// + the b3World_StartRecording / b3World_StopRecording public API)
// ---------------------------------------------------------------------------

impl Recording {
    /// C: b3Recording_GetData/GetSize.
    pub fn data(&self) -> &[u8] {
        &self.buffer.data
    }
}

fn write_header(
    buf: &mut RecBuffer,
    snapshot_size: u64,
) {
    // Fixed 48 bytes, little-endian (C: b3RecHeader).
    buf.w_u32(REC_MAGIC);
    buf.w_u16(REC_VERSION_MAJOR);
    buf.w_u16(REC_VERSION_MINOR);
    buf.w_u8(8); // pointerWidth
    buf.w_u8(0); // bigEndian
    buf.w_u8(if cfg!(debug_assertions) { 1 } else { 0 }); // validationEnabled
    buf.w_u8(0); // reserved
    buf.w_f32(crate::core::get_length_units_per_meter());
    buf.w_u32(0); // reserved2
    buf.w_u32(0); // reserved3
    buf.w_u64(snapshot_size);
    buf.w_u64(0); // registryOffset, backpatched at stop
    buf.w_u64(0); // registryByteCount, backpatched at stop
}

/// Start recording into the world. Fails silently (like C) if the world is
/// locked or already recording. The recording is created internally; stop
/// returns it (deviation: C takes a user-owned b3Recording handle).
pub fn world_start_recording(world: &mut World) {
    if world.locked || world.recording.is_some() {
        crate::b3_assert!(!world.locked);
        return;
    }

    let mut rec = Recording::new();

    // Every recording is snapshot-seeded. The seed blob follows the header so
    // replay restores in place and the world id stays stable across a restart
    // or backward scrub. An empty world still serializes a valid blob.
    let mut snap_buf = RecBuffer::new();
    crate::world_snapshot::serialize_world(world, &mut snap_buf, &mut rec);

    write_header(&mut rec.buffer, snap_buf.size() as u64);
    let snap_data = std::mem::take(&mut snap_buf.data);
    rec.buffer.append(&snap_data);

    // Anchor the recording with the current world state hash so replay can
    // assert determinism from the very first step.
    let wid = rec_world_id(world);
    let state_hash = hash_world_state(world);
    rec_begin_record(&mut rec, RecOp::StateHash as u8);
    rec.buffer.w_worldid(wid);
    rec.buffer.w_u64(state_hash);
    rec_end_record(&mut rec);

    world.recording = Some(Box::new(RefCell::new(rec)));
}

/// Stop recording and return the finished, self-contained recording.
/// Returns None if no recording is active.
pub fn world_stop_recording(world: &mut World) -> Option<Recording> {
    let cell = world.recording.take()?;
    let mut rec = cell.into_inner();

    // Write accumulated bounds so a viewer can frame the whole recorded motion.
    let bounds = if rec.have_bounds { rec.accumulated_bounds } else { AABB::default() };
    rec_begin_record(&mut rec, RecOp::RecordingBounds as u8);
    rec.buffer.w_aabb(bounds);
    rec_end_record(&mut rec);

    // End-of-stream marker; the buffer is now self-contained.
    let wid = rec_world_id(world);
    rec_begin_record(&mut rec, RecOp::DestroyWorld as u8);
    rec.buffer.w_worldid(wid);
    rec_end_record(&mut rec);

    // Write the trailing registry block and backpatch its locator.
    let registry_offset = rec.buffer.size();
    write_registry(&mut rec);
    let registry_byte_count = rec.buffer.size() - registry_offset;

    let off_bytes = (registry_offset as u64).to_le_bytes();
    let count_bytes = (registry_byte_count as u64).to_le_bytes();
    rec.buffer.data[REC_HDR_REGISTRY_OFFSET_OFFSET..REC_HDR_REGISTRY_OFFSET_OFFSET + 8]
        .copy_from_slice(&off_bytes);
    rec.buffer.data[REC_HDR_REGISTRY_BYTE_COUNT_OFFSET..REC_HDR_REGISTRY_BYTE_COUNT_OFFSET + 8]
        .copy_from_slice(&count_bytes);

    Some(rec)
}

/// C: b3SaveRecordingToFile.
pub fn save_recording_to_file(recording: &Recording, path: &str) -> bool {
    std::fs::write(path, &recording.buffer.data).is_ok()
}

/// C: b3LoadRecordingFromFile — validates the magic so a wrong file fails at
/// load rather than deep in the player.
pub fn load_recording_from_file(path: &str) -> Option<Recording> {
    let data = std::fs::read(path).ok()?;
    if data.len() < REC_HEADER_SIZE {
        return None;
    }
    let magic = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
    if magic != REC_MAGIC {
        return None;
    }
    let mut rec = Recording::new();
    rec.buffer.data = data;
    Some(rec)
}
