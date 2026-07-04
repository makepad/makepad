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

/// The recording substrate: buffer + geometry registry. The C b3Recording also
/// carries the op-stream lock, tags and bounds accumulation — not ported.
#[derive(Clone, Debug, Default)]
pub struct Recording {
    pub buffer: RecBuffer,
    pub registry: GeometryRegistry,
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

    // Query-tag table (not ported): zero tags.
    rec.buffer.w_u32(0);
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
