// Port of box3d/src/recording_replay.h + recording_replay.c — READER SUBSTRATE
// SUBSET ONLY: bounds-checked typed readers, canonical geometry decoding, and
// the preloaded geometry registry (RecReader.slots).
//
// NOT ported: the op-stream player (b3RecPlayer), dispatch functions, query
// hit scratch, keyframe ring, tags, and file header handling.
//
// Deviations:
// - The C reader borrows raw blob bytes and lazily builds "live" objects
//   (b3RegistrySlot.bytes/live). The Rust slots decode geometry eagerly into
//   Arc-shared values on load; a decode failure fails the whole load, exactly
//   as a later lazy failure would fail the replay.
// - Read failure sets `ok = false` (C pattern) and reads return zero values;
//   callers check `ok`. No panics on corrupt input.

use std::sync::Arc;

use crate::math_functions::{Matrix3, Quat, Transform, Vec3, AABB};
use crate::recording::{geometry_kind_from_u8, GeometryKind};
use crate::types::{
    CompoundCapsule, CompoundData, CompoundHull, CompoundMesh, CompoundSphere, DynamicTree,
    HeightFieldData, HullData, HullFace, HullHalfEdge, HullVertex, MeshData, MeshNode, MeshTriangle,
    SurfaceMaterial, TreeNode, TreeNodeChildren, MAX_COMPOUND_MESH_MATERIALS,
};

/// Bounds-checked read cursor over a byte image (C: the b3RecReader cursor
/// fields; also used by the snapshot reader in world_snapshot.rs).
pub struct RecCursor<'a> {
    pub data: &'a [u8],
    pub cursor: usize,
    pub ok: bool,
}

impl<'a> RecCursor<'a> {
    pub fn new(data: &'a [u8]) -> RecCursor<'a> {
        RecCursor { data, cursor: 0, ok: true }
    }

    #[inline]
    pub fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.cursor)
    }

    #[inline]
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if !self.ok || self.cursor + n > self.data.len() {
            self.ok = false;
            return None;
        }
        let slice = &self.data[self.cursor..self.cursor + n];
        self.cursor += n;
        Some(slice)
    }

    #[inline]
    pub fn r_bytes(&mut self, dst: &mut [u8]) {
        if let Some(src) = self.take(dst.len()) {
            dst.copy_from_slice(src);
        }
    }

    #[inline]
    pub fn r_u8(&mut self) -> u8 {
        self.take(1).map(|b| b[0]).unwrap_or(0)
    }

    #[inline]
    pub fn r_u16(&mut self) -> u16 {
        self.take(2).map(|b| u16::from_le_bytes(b.try_into().unwrap())).unwrap_or(0)
    }

    #[inline]
    pub fn r_u32(&mut self) -> u32 {
        self.take(4).map(|b| u32::from_le_bytes(b.try_into().unwrap())).unwrap_or(0)
    }

    #[inline]
    pub fn r_u64(&mut self) -> u64 {
        self.take(8).map(|b| u64::from_le_bytes(b.try_into().unwrap())).unwrap_or(0)
    }

    #[inline]
    pub fn r_i32(&mut self) -> i32 {
        self.r_u32() as i32
    }

    #[inline]
    pub fn r_f32(&mut self) -> f32 {
        f32::from_bits(self.r_u32())
    }

    #[inline]
    pub fn r_f64(&mut self) -> f64 {
        f64::from_bits(self.r_u64())
    }

    #[inline]
    pub fn r_bool(&mut self) -> bool {
        self.r_u8() != 0
    }

    #[inline]
    pub fn r_vec3(&mut self) -> Vec3 {
        let x = self.r_f32();
        let y = self.r_f32();
        let z = self.r_f32();
        Vec3 { x, y, z }
    }

    #[inline]
    pub fn r_quat(&mut self) -> Quat {
        let v = self.r_vec3();
        let s = self.r_f32();
        Quat { v, s }
    }

    #[inline]
    pub fn r_transform(&mut self) -> Transform {
        let p = self.r_vec3();
        let q = self.r_quat();
        Transform { p, q }
    }

    #[inline]
    pub fn r_matrix3(&mut self) -> Matrix3 {
        let cx = self.r_vec3();
        let cy = self.r_vec3();
        let cz = self.r_vec3();
        Matrix3 { cx, cy, cz }
    }

    #[inline]
    pub fn r_aabb(&mut self) -> AABB {
        let lower_bound = self.r_vec3();
        let upper_bound = self.r_vec3();
        AABB { lower_bound, upper_bound }
    }

    /// C: b3RecR_STR (length-prefixed in the port).
    pub fn r_string(&mut self) -> String {
        let len = self.r_u32() as usize;
        // A string longer than the remaining stream is corrupt.
        if !self.ok || len > self.remaining() {
            self.ok = false;
            return String::new();
        }
        let bytes = self.take(len).unwrap_or(&[]);
        String::from_utf8_lossy(bytes).into_owned()
    }

    #[inline]
    pub fn r_material(&mut self) -> SurfaceMaterial {
        SurfaceMaterial {
            friction: self.r_f32(),
            restitution: self.r_f32(),
            rolling_resistance: self.r_f32(),
            tangent_velocity: self.r_vec3(),
            user_material_id: self.r_u64(),
            custom_color: self.r_u32(),
        }
    }

    /// Sanity bound for a count of elements that need at least `min_bytes`
    /// each from the remaining stream (C: b3SnapCheckCount).
    #[inline]
    pub fn check_count(&mut self, count: i32, min_bytes: usize) -> bool {
        if count < 0 {
            self.ok = false;
            return false;
        }
        if (count as u64) * (min_bytes as u64) > self.remaining() as u64 {
            self.ok = false;
            return false;
        }
        self.ok
    }
}

// ---------------------------------------------------------------------------
// Canonical geometry decoding (writers in recording.rs)
// ---------------------------------------------------------------------------

pub(crate) fn des_hull_data(r: &mut RecCursor) -> Option<HullData> {
    let mut hull = HullData::default();
    hull.hash = r.r_u32();
    hull.aabb = r.r_aabb();
    hull.surface_area = r.r_f32();
    hull.volume = r.r_f32();
    hull.inner_radius = r.r_f32();
    hull.center = r.r_vec3();
    hull.central_inertia = r.r_matrix3();

    let vertex_count = r.r_i32();
    if !r.check_count(vertex_count, 1) {
        return None;
    }
    hull.vertices.reserve(vertex_count as usize);
    for _ in 0..vertex_count {
        hull.vertices.push(HullVertex { edge: r.r_u8() });
    }

    let point_count = r.r_i32();
    if !r.check_count(point_count, 12) {
        return None;
    }
    hull.points.reserve(point_count as usize);
    for _ in 0..point_count {
        hull.points.push(r.r_vec3());
    }

    let edge_count = r.r_i32();
    if !r.check_count(edge_count, 4) {
        return None;
    }
    hull.edges.reserve(edge_count as usize);
    for _ in 0..edge_count {
        hull.edges.push(HullHalfEdge {
            next: r.r_u8(),
            twin: r.r_u8(),
            origin: r.r_u8(),
            face: r.r_u8(),
        });
    }

    let face_count = r.r_i32();
    if !r.check_count(face_count, 1) {
        return None;
    }
    hull.faces.reserve(face_count as usize);
    for _ in 0..face_count {
        hull.faces.push(HullFace { edge: r.r_u8() });
    }

    let plane_count = r.r_i32();
    if !r.check_count(plane_count, 16) {
        return None;
    }
    hull.planes.reserve(plane_count as usize);
    for _ in 0..plane_count {
        let normal = r.r_vec3();
        let offset = r.r_f32();
        hull.planes.push(crate::math_functions::Plane { normal, offset });
    }

    if r.ok {
        Some(hull)
    } else {
        None
    }
}

pub(crate) fn des_mesh_data(r: &mut RecCursor) -> Option<MeshData> {
    let mut mesh = MeshData::default();
    mesh.hash = r.r_u32();
    mesh.bounds = r.r_aabb();
    mesh.surface_area = r.r_f32();
    mesh.tree_height = r.r_i32();
    mesh.degenerate_count = r.r_i32();

    let node_count = r.r_i32();
    if !r.check_count(node_count, 32) {
        return None;
    }
    mesh.nodes.reserve(node_count as usize);
    for _ in 0..node_count {
        let lower_bound = r.r_vec3();
        let data = r.r_u32();
        let upper_bound = r.r_vec3();
        let triangle_offset = r.r_u32();
        mesh.nodes.push(MeshNode { lower_bound, data, upper_bound, triangle_offset });
    }

    let vertex_count = r.r_i32();
    if !r.check_count(vertex_count, 12) {
        return None;
    }
    mesh.vertices.reserve(vertex_count as usize);
    for _ in 0..vertex_count {
        mesh.vertices.push(r.r_vec3());
    }

    let triangle_count = r.r_i32();
    if !r.check_count(triangle_count, 12) {
        return None;
    }
    mesh.triangles.reserve(triangle_count as usize);
    for _ in 0..triangle_count {
        mesh.triangles.push(MeshTriangle {
            index1: r.r_i32(),
            index2: r.r_i32(),
            index3: r.r_i32(),
        });
    }

    let material_count = r.r_i32();
    if !r.check_count(material_count, 1) {
        return None;
    }
    mesh.materials = vec![0u8; material_count as usize];
    r.r_bytes(&mut mesh.materials);

    let flag_count = r.r_i32();
    if !r.check_count(flag_count, 1) {
        return None;
    }
    mesh.flags = vec![0u8; flag_count as usize];
    r.r_bytes(&mut mesh.flags);

    if r.ok {
        Some(mesh)
    } else {
        None
    }
}

pub(crate) fn des_height_field_data(r: &mut RecCursor) -> Option<HeightFieldData> {
    let mut hf = HeightFieldData::default();
    hf.hash = r.r_u32();
    hf.aabb = r.r_aabb();
    hf.min_height = r.r_f32();
    hf.max_height = r.r_f32();
    hf.height_scale = r.r_f32();
    hf.scale = r.r_vec3();
    hf.column_count = r.r_i32();
    hf.row_count = r.r_i32();

    let height_count = r.r_i32();
    if !r.check_count(height_count, 2) {
        return None;
    }
    hf.heights.reserve(height_count as usize);
    for _ in 0..height_count {
        hf.heights.push(r.r_u16());
    }

    let material_count = r.r_i32();
    if !r.check_count(material_count, 1) {
        return None;
    }
    hf.materials = vec![0u8; material_count as usize];
    r.r_bytes(&mut hf.materials);

    let flag_count = r.r_i32();
    if !r.check_count(flag_count, 1) {
        return None;
    }
    hf.flags = vec![0u8; flag_count as usize];
    r.r_bytes(&mut hf.flags);

    hf.clockwise = r.r_bool();

    if r.ok {
        Some(hf)
    } else {
        None
    }
}

pub(crate) fn des_compound_data(r: &mut RecCursor) -> Option<CompoundData> {
    let mut compound = CompoundData::default();

    // Tree
    let mut tree = DynamicTree::default();
    tree.root = r.r_i32();
    tree.node_count = r.r_i32();
    tree.node_capacity = r.r_i32();
    tree.free_list = r.r_i32();
    tree.proxy_count = r.r_i32();
    let node_count = r.r_i32();
    if !r.check_count(node_count, 56) {
        return None;
    }
    tree.nodes.reserve(node_count as usize);
    for _ in 0..node_count {
        let aabb = r.r_aabb();
        let category_bits = r.r_u64();
        let child1 = r.r_i32();
        let child2 = r.r_i32();
        let user_data = r.r_u64();
        let parent = r.r_i32();
        let height = r.r_u16();
        let flags = r.r_u16();
        tree.nodes.push(TreeNode {
            aabb,
            category_bits,
            children: TreeNodeChildren { child1, child2 },
            user_data,
            parent,
            height,
            flags,
        });
    }
    compound.tree = tree;

    // Materials
    let material_count = r.r_i32();
    if !r.check_count(material_count, 36) {
        return None;
    }
    compound.materials.reserve(material_count as usize);
    for _ in 0..material_count {
        compound.materials.push(r.r_material());
    }

    // Capsules
    let capsule_count = r.r_i32();
    if !r.check_count(capsule_count, 32) {
        return None;
    }
    compound.capsules.reserve(capsule_count as usize);
    for _ in 0..capsule_count {
        let center1 = r.r_vec3();
        let center2 = r.r_vec3();
        let radius = r.r_f32();
        let material_index = r.r_i32();
        compound.capsules.push(CompoundCapsule {
            capsule: crate::types::Capsule { center1, center2, radius },
            material_index,
        });
    }

    // Unique hulls + instances
    let unique_hull_count = r.r_i32();
    if !r.check_count(unique_hull_count, 4) {
        return None;
    }
    let mut unique_hulls: Vec<Arc<HullData>> = Vec::with_capacity(unique_hull_count as usize);
    for _ in 0..unique_hull_count {
        unique_hulls.push(Arc::new(des_hull_data(r)?));
    }
    let hull_count = r.r_i32();
    if !r.check_count(hull_count, 36) {
        return None;
    }
    compound.hulls.reserve(hull_count as usize);
    for _ in 0..hull_count {
        let unique_index = r.r_i32();
        let transform = r.r_transform();
        let material_index = r.r_i32();
        if unique_index < 0 || unique_index >= unique_hulls.len() as i32 {
            r.ok = false;
            return None;
        }
        compound.hulls.push(CompoundHull {
            hull: Arc::clone(&unique_hulls[unique_index as usize]),
            transform,
            material_index,
        });
    }
    compound.shared_hull_count = r.r_i32();

    // Unique meshes + instances
    let unique_mesh_count = r.r_i32();
    if !r.check_count(unique_mesh_count, 4) {
        return None;
    }
    let mut unique_meshes: Vec<Arc<MeshData>> = Vec::with_capacity(unique_mesh_count as usize);
    for _ in 0..unique_mesh_count {
        unique_meshes.push(Arc::new(des_mesh_data(r)?));
    }
    let mesh_count = r.r_i32();
    if !r.check_count(mesh_count, 60) {
        return None;
    }
    compound.meshes.reserve(mesh_count as usize);
    for _ in 0..mesh_count {
        let unique_index = r.r_i32();
        let transform = r.r_transform();
        let scale = r.r_vec3();
        let mut material_indices = [0i32; MAX_COMPOUND_MESH_MATERIALS];
        for k in 0..MAX_COMPOUND_MESH_MATERIALS {
            material_indices[k] = r.r_i32();
        }
        if unique_index < 0 || unique_index >= unique_meshes.len() as i32 {
            r.ok = false;
            return None;
        }
        compound.meshes.push(CompoundMesh {
            mesh_data: Arc::clone(&unique_meshes[unique_index as usize]),
            transform,
            scale,
            material_indices,
        });
    }
    compound.shared_mesh_count = r.r_i32();

    // Spheres
    let sphere_count = r.r_i32();
    if !r.check_count(sphere_count, 20) {
        return None;
    }
    compound.spheres.reserve(sphere_count as usize);
    for _ in 0..sphere_count {
        let center = r.r_vec3();
        let radius = r.r_f32();
        let material_index = r.r_i32();
        compound.spheres.push(CompoundSphere {
            sphere: crate::types::Sphere { center, radius },
            material_index,
        });
    }

    if r.ok {
        Some(compound)
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Preloaded geometry registry (C: b3RegistrySlot / b3RecLoadSlots)
// ---------------------------------------------------------------------------

/// Decoded geometry for one registry slot (C keeps raw bytes + lazy live
/// object; the port decodes eagerly).
#[derive(Clone, Debug)]
pub enum RegistryGeometry {
    Hull(Arc<HullData>),
    Mesh(Arc<MeshData>),
    HeightField(Arc<HeightFieldData>),
    Compound(Arc<CompoundData>),
}

#[derive(Clone, Debug)]
pub struct RegistrySlot {
    pub kind: GeometryKind,
    pub geometry: RegistryGeometry,
}

/// Reader state for snapshot restore and replay: the preloaded geometry
/// registry plus the interned query-tag table (key -> id, name).
/// The C b3RecReader also carries the op-stream cursor and scratch buffers;
/// in the port those live on the Player below.
#[derive(Clone, Debug, Default)]
pub struct RecReader {
    pub slots: Vec<RegistrySlot>,

    /// Query tags loaded from the tail of the registry block. tag_map maps a
    /// tag key to its index for O(1) lookup (C: b3RecLoadTags/tagMap).
    pub tags: Vec<crate::recording::RecTag>,
    pub tag_map: std::collections::HashMap<u64, u32>,

    /// Raw slot bytes, kept so the keyframe registry can be seeded 1:1
    /// (C keeps b3RegistrySlot.bytes for the same reason).
    pub slot_bytes: Vec<Vec<u8>>,
}

impl RecReader {
    #[inline]
    pub fn slot_count(&self) -> i32 {
        self.slots.len() as i32
    }
}

/// C: b3RecLoadSlots — parse a registry block (as written by write_registry)
/// and decode every slot, then the trailing query-tag table. Returns None on
/// corrupt input.
pub fn load_registry(bytes: &[u8]) -> Option<RecReader> {
    let mut r = RecCursor::new(bytes);

    let count = r.r_u32();
    if !r.ok {
        return None;
    }

    // Each entry is at least 5 bytes (kind + 4-byte length). A count that
    // cannot fit the remaining bytes is a corrupt header.
    if count as u64 > (r.remaining() as u64) / 5 {
        return None;
    }

    let mut slots = Vec::with_capacity(count as usize);
    let mut slot_bytes = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let kind_byte = r.r_u8();
        let byte_count = r.r_u32() as usize;
        if !r.ok || byte_count > r.remaining() {
            return None;
        }
        let kind = geometry_kind_from_u8(kind_byte)?;
        let blob = &r.data[r.cursor..r.cursor + byte_count];
        r.cursor += byte_count;

        let mut gr = RecCursor::new(blob);
        let geometry = match kind {
            GeometryKind::Hull => RegistryGeometry::Hull(Arc::new(des_hull_data(&mut gr)?)),
            GeometryKind::Mesh => RegistryGeometry::Mesh(Arc::new(des_mesh_data(&mut gr)?)),
            GeometryKind::HeightField => {
                RegistryGeometry::HeightField(Arc::new(des_height_field_data(&mut gr)?))
            }
            GeometryKind::Compound => RegistryGeometry::Compound(Arc::new(des_compound_data(&mut gr)?)),
        };

        slots.push(RegistrySlot { kind, geometry });
        slot_bytes.push(blob.to_vec());
    }

    // Query-tag table (C: b3RecLoadTags): u32 tagCount then per tag
    // { u64 key, u64 id, STR name } in the port's u32-length string format.
    // A snapshot registry written before any tags carries a zero count; a
    // truncated tail loads the tags that fully fit, like C.
    let mut tags: Vec<crate::recording::RecTag> = Vec::new();
    let mut tag_map: std::collections::HashMap<u64, u32> = std::collections::HashMap::new();
    let tag_count = r.r_u32();
    if r.ok && tag_count > 0 {
        // Each tag is at least 20 bytes (8 key + 8 id + 4 length).
        if tag_count as u64 <= (r.remaining() as u64) / 20 {
            for _ in 0..tag_count {
                let key = r.r_u64();
                let id = r.r_u64();
                let name = r.r_string();
                if !r.ok {
                    break;
                }
                let index = tags.len() as u32;
                tags.push(crate::recording::RecTag { key, id, name });
                tag_map.insert(key, index);
            }
        }
    }

    Some(RecReader { slots, tags, tag_map, slot_bytes })
}

// ===========================================================================
// Op-stream replay: readers, dispatch, and the player
// (port of the rest of recording_replay.h/.c)
// ===========================================================================
//
// The byte format is the port capture format (see the op-stream section of
// recording.rs): 48-byte header, snapshot seed blob, u8 opcode + u24 payload
// framed records, trailing registry block with the query-tag table. Opcode
// values and record arg orders follow the capture manifest exactly.
//
// Port deviations from C:
// - The replay world is OWNED by the Player (no global world registry in the
//   port); tests reach it via player.world()/world_mut().
// - Strings are u32-length prefixed (port format), not the C rotating scratch.
// - The viewer draw path (b3RecPlayer_DrawFrameQueries) and debug-shape
//   callbacks are not ported (debug draw is not in the port); the per-frame
//   query store IS ported for the info accessors.

use crate::core::{get_length_units_per_meter, set_length_units_per_meter, NULL_INDEX};
use crate::id::{
    load_body_id, load_joint_id, load_shape_id, BodyId, JointId, ShapeId, NULL_BODY_ID,
};
use crate::math_functions::{Pos, WorldTransform, POS_ZERO};
use crate::physics_world::World;
use crate::recording::{
    append_geometry, hash64_blob, hash_world_state, RecBuffer, RecOp, RecTag, Recording,
    REC_HDR_REGISTRY_BYTE_COUNT_OFFSET, REC_HDR_REGISTRY_OFFSET_OFFSET, REC_HDR_SNAPSHOT_SIZE_OFFSET,
    REC_HEADER_SIZE, REC_MAGIC, REC_VERSION_MAJOR,
};
use crate::types::{
    BodyDef, BodyType, Capsule, DistanceJointDef, ExplosionDef, Filter, FilterJointDef, JointDef,
    MassData, MotionLocks, MotorJointDef, ParallelJointDef, PlaneResult, PrismaticJointDef,
    QueryFilter, RayResult, RevoluteJointDef, ShapeDef, ShapeProxy, Sphere, SphericalJointDef,
    TreeStats, WeldJointDef, WheelJointDef,
};

// ---------------------------------------------------------------------------
// Read primitives for record args (C: the remaining b3RecR_* set). Layouts
// mirror the RecBuffer writers in recording.rs field for field.
// ---------------------------------------------------------------------------

impl<'a> RecCursor<'a> {
    /// C: b3RecR_U24 (record payload size).
    #[inline]
    pub fn r_u24(&mut self) -> u32 {
        let b0 = self.r_u8() as u32;
        let b1 = self.r_u8() as u32;
        let b2 = self.r_u8() as u32;
        b0 | (b1 << 8) | (b2 << 16)
    }

    /// C: b3RecR_POSITION — full precision in large world mode.
    #[inline]
    pub fn r_position(&mut self) -> Pos {
        #[cfg(not(feature = "double-precision"))]
        {
            self.r_vec3()
        }
        #[cfg(feature = "double-precision")]
        {
            let x = self.r_f64();
            let y = self.r_f64();
            let z = self.r_f64();
            Pos { x, y, z }
        }
    }

    /// C: b3RecR_WORLDXF
    #[inline]
    pub fn r_worldxf(&mut self) -> WorldTransform {
        let p = self.r_position();
        let q = self.r_quat();
        WorldTransform { p, q }
    }

    /// C: b3RecR_WORLDID (u32 packed; only consumed, never used for lookup —
    /// the replay world is the player's).
    #[inline]
    pub fn r_worldid(&mut self) -> u32 {
        self.r_u32()
    }

    /// C: b3RecR_BODYID (u64 packed).
    #[inline]
    pub fn r_bodyid(&mut self) -> BodyId {
        load_body_id(self.r_u64())
    }

    /// C: b3RecR_SHAPEID
    #[inline]
    pub fn r_shapeid(&mut self) -> ShapeId {
        load_shape_id(self.r_u64())
    }

    /// C: b3RecR_JOINTID
    #[inline]
    pub fn r_jointid(&mut self) -> JointId {
        load_joint_id(self.r_u64())
    }

    /// C: b3RecR_SPHERE
    #[inline]
    pub fn r_sphere(&mut self) -> Sphere {
        Sphere { center: self.r_vec3(), radius: self.r_f32() }
    }

    /// C: b3RecR_CAPSULE
    #[inline]
    pub fn r_capsule(&mut self) -> Capsule {
        Capsule { center1: self.r_vec3(), center2: self.r_vec3(), radius: self.r_f32() }
    }

    /// C: b3RecR_GEOMID
    #[inline]
    pub fn r_geomid(&mut self) -> u32 {
        self.r_u32()
    }

    /// C: b3RecR_FILTER
    #[inline]
    pub fn r_filter(&mut self) -> Filter {
        Filter { category_bits: self.r_u64(), mask_bits: self.r_u64(), group_index: self.r_i32() }
    }

    /// C: b3RecR_MASSDATA
    #[inline]
    pub fn r_massdata(&mut self) -> MassData {
        MassData { mass: self.r_f32(), center: self.r_vec3(), inertia: self.r_matrix3() }
    }

    /// C: b3RecR_LOCKS
    #[inline]
    pub fn r_locks(&mut self) -> MotionLocks {
        MotionLocks {
            linear_x: self.r_bool(),
            linear_y: self.r_bool(),
            linear_z: self.r_bool(),
            angular_x: self.r_bool(),
            angular_y: self.r_bool(),
            angular_z: self.r_bool(),
        }
    }

    /// C: b3RecR_QUERYFILTER — category/mask only; tags travel separately.
    #[inline]
    pub fn r_queryfilter(&mut self) -> QueryFilter {
        let mut filter = crate::types::default_query_filter();
        filter.category_bits = self.r_u64();
        filter.mask_bits = self.r_u64();
        filter
    }

    /// C: b3RecR_SHAPEPROXY — returns owned points (ShapeProxy borrows).
    pub fn r_shapeproxy(&mut self) -> (Vec<crate::math_functions::Vec3>, f32) {
        let count = self.r_i32();
        if !self.check_count(count, 12) {
            return (Vec::new(), 0.0);
        }
        let mut points = Vec::with_capacity(count as usize);
        for _ in 0..count {
            points.push(self.r_vec3());
        }
        let radius = self.r_f32();
        (points, radius)
    }

    /// C: b3RecR_TREESTATS
    #[inline]
    pub fn r_treestats(&mut self) -> TreeStats {
        TreeStats { node_visits: self.r_i32(), leaf_visits: self.r_i32() }
    }

    /// C: b3RecR_RAYRESULT
    pub fn r_rayresult(&mut self) -> RayResult {
        let mut result = RayResult::default();
        result.shape_id = self.r_shapeid();
        result.point = self.r_position();
        result.normal = self.r_vec3();
        result.user_material_id = self.r_u64();
        result.fraction = self.r_f32();
        result.triangle_index = self.r_i32();
        result.child_index = self.r_i32();
        result.hit = self.r_bool();
        result
    }

    /// C: b3RecR_PLANERESULT
    #[inline]
    pub fn r_planeresult(&mut self) -> PlaneResult {
        PlaneResult {
            plane: crate::math_functions::Plane { normal: self.r_vec3(), offset: self.r_f32() },
            point: self.r_vec3(),
        }
    }

    /// C: b3RecR_EXPLOSIONDEF
    pub fn r_explosiondef(&mut self) -> ExplosionDef {
        let mut def = crate::types::default_explosion_def();
        def.mask_bits = self.r_u64();
        def.position = self.r_position();
        def.radius = self.r_f32();
        def.falloff = self.r_f32();
        def.impulse_per_area = self.r_f32();
        def
    }

    /// C: b3RecR_BODYDEF — starts from the default def like C so unrecorded
    /// fields (internal cookie) stay valid.
    pub fn r_bodydef(&mut self) -> BodyDef {
        let mut def = crate::types::default_body_def();
        def.body_type = match self.r_i32() {
            0 => BodyType::Static,
            1 => BodyType::Kinematic,
            2 => BodyType::Dynamic,
            _ => {
                self.ok = false;
                BodyType::Static
            }
        };
        def.position = self.r_position();
        def.rotation = self.r_quat();
        def.linear_velocity = self.r_vec3();
        def.angular_velocity = self.r_vec3();
        def.linear_damping = self.r_f32();
        def.angular_damping = self.r_f32();
        def.gravity_scale = self.r_f32();
        def.sleep_threshold = self.r_f32();
        def.name = self.r_string();
        let _user_data = self.r_u64(); // not preserved
        def.motion_locks = self.r_locks();
        def.enable_sleep = self.r_bool();
        def.is_awake = self.r_bool();
        def.is_bullet = self.r_bool();
        def.is_enabled = self.r_bool();
        def.allow_fast_rotation = self.r_bool();
        def.enable_contact_recycling = self.r_bool();
        def
    }

    /// C: b3RecR_SHAPEDEF
    pub fn r_shapedef(&mut self) -> ShapeDef {
        let mut def = crate::types::default_shape_def();
        let _user_data = self.r_u64(); // not preserved
        let material_count = self.r_i32();
        if !self.check_count(material_count, 36) {
            return def;
        }
        def.materials = Vec::with_capacity(material_count as usize);
        for _ in 0..material_count {
            def.materials.push(self.r_material());
        }
        def.base_material = self.r_material();
        def.density = self.r_f32();
        def.explosion_scale = self.r_f32();
        def.filter = self.r_filter();
        def.enable_custom_filtering = self.r_bool();
        def.is_sensor = self.r_bool();
        def.enable_sensor_events = self.r_bool();
        def.enable_contact_events = self.r_bool();
        def.enable_hit_events = self.r_bool();
        def.enable_pre_solve_events = self.r_bool();
        def.invoke_contact_creation = self.r_bool();
        def.update_body_mass = self.r_bool();
        def
    }

    /// C: b3RecR_JointBase — body ids are remapped by the caller.
    pub fn r_joint_base(&mut self, base: &mut JointDef) {
        let _user_data = self.r_u64(); // not preserved
        base.body_id_a = self.r_bodyid();
        base.body_id_b = self.r_bodyid();
        base.local_frame_a = self.r_transform();
        base.local_frame_b = self.r_transform();
        base.force_threshold = self.r_f32();
        base.torque_threshold = self.r_f32();
        base.constraint_hertz = self.r_f32();
        base.constraint_damping_ratio = self.r_f32();
        base.draw_scale = self.r_f32();
        base.collide_connected = self.r_bool();
    }

    pub fn r_paralleljointdef(&mut self) -> ParallelJointDef {
        let mut def = crate::joint::default_parallel_joint_def();
        self.r_joint_base(&mut def.base);
        def.hertz = self.r_f32();
        def.damping_ratio = self.r_f32();
        def.max_torque = self.r_f32();
        def
    }

    pub fn r_distancejointdef(&mut self) -> DistanceJointDef {
        let mut def = crate::joint::default_distance_joint_def();
        self.r_joint_base(&mut def.base);
        def.length = self.r_f32();
        def.enable_spring = self.r_bool();
        def.lower_spring_force = self.r_f32();
        def.upper_spring_force = self.r_f32();
        def.hertz = self.r_f32();
        def.damping_ratio = self.r_f32();
        def.enable_limit = self.r_bool();
        def.min_length = self.r_f32();
        def.max_length = self.r_f32();
        def.enable_motor = self.r_bool();
        def.max_motor_force = self.r_f32();
        def.motor_speed = self.r_f32();
        def
    }

    pub fn r_filterjointdef(&mut self) -> FilterJointDef {
        let mut def = crate::joint::default_filter_joint_def();
        self.r_joint_base(&mut def.base);
        def
    }

    pub fn r_motorjointdef(&mut self) -> MotorJointDef {
        let mut def = crate::joint::default_motor_joint_def();
        self.r_joint_base(&mut def.base);
        def.linear_velocity = self.r_vec3();
        def.max_velocity_force = self.r_f32();
        def.angular_velocity = self.r_vec3();
        def.max_velocity_torque = self.r_f32();
        def.linear_hertz = self.r_f32();
        def.linear_damping_ratio = self.r_f32();
        def.max_spring_force = self.r_f32();
        def.angular_hertz = self.r_f32();
        def.angular_damping_ratio = self.r_f32();
        def.max_spring_torque = self.r_f32();
        def
    }

    pub fn r_prismaticjointdef(&mut self) -> PrismaticJointDef {
        let mut def = crate::joint::default_prismatic_joint_def();
        self.r_joint_base(&mut def.base);
        def.enable_spring = self.r_bool();
        def.hertz = self.r_f32();
        def.damping_ratio = self.r_f32();
        def.target_translation = self.r_f32();
        def.enable_limit = self.r_bool();
        def.lower_translation = self.r_f32();
        def.upper_translation = self.r_f32();
        def.enable_motor = self.r_bool();
        def.max_motor_force = self.r_f32();
        def.motor_speed = self.r_f32();
        def
    }

    pub fn r_revolutejointdef(&mut self) -> RevoluteJointDef {
        let mut def = crate::joint::default_revolute_joint_def();
        self.r_joint_base(&mut def.base);
        def.target_angle = self.r_f32();
        def.enable_spring = self.r_bool();
        def.hertz = self.r_f32();
        def.damping_ratio = self.r_f32();
        def.enable_limit = self.r_bool();
        def.lower_angle = self.r_f32();
        def.upper_angle = self.r_f32();
        def.enable_motor = self.r_bool();
        def.max_motor_torque = self.r_f32();
        def.motor_speed = self.r_f32();
        def
    }

    pub fn r_sphericaljointdef(&mut self) -> SphericalJointDef {
        let mut def = crate::joint::default_spherical_joint_def();
        self.r_joint_base(&mut def.base);
        def.enable_spring = self.r_bool();
        def.hertz = self.r_f32();
        def.damping_ratio = self.r_f32();
        def.target_rotation = self.r_quat();
        def.enable_cone_limit = self.r_bool();
        def.cone_angle = self.r_f32();
        def.enable_twist_limit = self.r_bool();
        def.lower_twist_angle = self.r_f32();
        def.upper_twist_angle = self.r_f32();
        def.enable_motor = self.r_bool();
        def.max_motor_torque = self.r_f32();
        def.motor_velocity = self.r_vec3();
        def
    }

    pub fn r_weldjointdef(&mut self) -> WeldJointDef {
        let mut def = crate::joint::default_weld_joint_def();
        self.r_joint_base(&mut def.base);
        def.linear_hertz = self.r_f32();
        def.angular_hertz = self.r_f32();
        def.linear_damping_ratio = self.r_f32();
        def.angular_damping_ratio = self.r_f32();
        def
    }

    pub fn r_wheeljointdef(&mut self) -> WheelJointDef {
        let mut def = crate::joint::default_wheel_joint_def();
        self.r_joint_base(&mut def.base);
        def.enable_suspension_spring = self.r_bool();
        def.suspension_hertz = self.r_f32();
        def.suspension_damping_ratio = self.r_f32();
        def.enable_suspension_limit = self.r_bool();
        def.lower_suspension_limit = self.r_f32();
        def.upper_suspension_limit = self.r_f32();
        def.enable_spin_motor = self.r_bool();
        def.max_spin_torque = self.r_f32();
        def.spin_speed = self.r_f32();
        def.enable_steering = self.r_bool();
        def.steering_hertz = self.r_f32();
        def.steering_damping_ratio = self.r_f32();
        def.target_steering_angle = self.r_f32();
        def.max_steering_torque = self.r_f32();
        def.enable_steering_limit = self.r_bool();
        def.lower_steering_limit = self.r_f32();
        def.upper_steering_limit = self.r_f32();
        def
    }
}

// ---------------------------------------------------------------------------
// Replay comparison helpers. Bitwise float compare so the determinism check is
// exact, not within a tolerance (C: b3RecF32Differs / b3RecVec3Differs).
// ---------------------------------------------------------------------------

#[inline]
fn f32_differs(a: f32, b: f32) -> bool {
    a.to_bits() != b.to_bits()
}

#[inline]
fn vec3_differs(a: crate::math_functions::Vec3, b: crate::math_functions::Vec3) -> bool {
    f32_differs(a.x, b.x) || f32_differs(a.y, b.y) || f32_differs(a.z, b.z)
}

// Positions compared through a full-width delta; truncating both sides would
// pass vacuously far from the origin (C comment).
#[inline]
fn pos_differs(a: Pos, b: Pos) -> bool {
    vec3_differs(crate::math_functions::sub_pos(a, b), crate::math_functions::Vec3::ZERO)
}

/// A single recorded callback hit (C: b3RecRecordedHit).
#[derive(Clone, Copy, Debug, Default)]
pub struct RecordedHit {
    pub id: ShapeId,
    pub point: Pos,
    pub normal: crate::math_functions::Vec3,
    pub fraction: f32,
    pub user_material_id: u64,
    pub triangle_index: i32,
    pub child_index: i32,
    /// collide-mover: this plane
    pub plane: PlaneResult,
    /// collide-mover: planes in this hit's shape group (replicated)
    pub plane_count: i32,
    /// cast queries
    pub user_return_f: f32,
    /// overlap / collide-mover (per shape, replicated)
    pub user_return_b: bool,
}

/// Recorded query kind (C: b3RecQueryKind).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RecQueryKind {
    OverlapAabb,
    OverlapShape,
    CastRay,
    CastShape,
    CastRayClosest,
    CastMover,
    CollideMover,
}

/// Per-frame record for one query call (C: b3RecDrawQuery minus the draw-only
/// geometry stash; the info accessors keep what the tests inspect).
#[derive(Clone, Debug)]
pub struct DrawQuery {
    pub kind: RecQueryKind,
    /// identity key (hash of caller id+name), 0 = untagged
    pub key: u64,
    pub filter: QueryFilter,
    pub aabb: crate::math_functions::AABB,
    pub origin: Pos,
    pub translation: crate::math_functions::Vec3,
    pub mover: Capsule,
    pub cast_fraction: f32,
    pub ray_result: RayResult,
    pub hit_start: i32,
    pub hit_count: i32,
}

impl Default for DrawQuery {
    fn default() -> DrawQuery {
        DrawQuery {
            kind: RecQueryKind::OverlapAabb,
            key: 0,
            filter: crate::types::default_query_filter(),
            aabb: crate::math_functions::AABB::default(),
            origin: POS_ZERO,
            translation: crate::math_functions::Vec3::ZERO,
            mover: Capsule::default(),
            cast_fraction: 0.0,
            ray_result: RayResult::default(),
            hit_start: 0,
            hit_count: 0,
        }
    }
}

/// Resolved info for one recorded query (C: b3RecQueryInfo).
#[derive(Clone, Debug)]
pub struct RecQueryInfo {
    pub kind: RecQueryKind,
    pub key: u64,
    /// caller id resolved from the tag table, 0 = untagged
    pub id: u64,
    /// caller label resolved from the tag table, None = untagged
    pub name: Option<String>,
    pub hit_count: i32,
}

/// Recorded tuning info (C: b3RecPlayerInfo).
#[derive(Clone, Copy, Debug)]
pub struct RecPlayerInfo {
    pub frame_count: i32,
    pub time_step: f32,
    pub sub_step_count: i32,
    pub bounds: crate::math_functions::AABB,
}

/// Stored snapshot for fast backward seek (C: b3RecKeyframe).
struct Keyframe {
    image: Vec<u8>,
    frame: i32,
    /// op-stream cursor for the frame AFTER this one
    cursor: usize,
    diverge_frame: i32,
    diverged: bool,
    body_ids: Vec<BodyId>,
}

const KEYFRAME_INTERVAL_DEFAULT: i32 = 16;
const KEYFRAME_BUDGET_DEFAULT: usize = 512 * 1024 * 1024;

/// The replay player (C: b3RecPlayer). Owns its replay world.
pub struct Player {
    data: Vec<u8>,
    /// first byte of op stream (past header + snapshot blob)
    header_end: usize,
    /// end of op stream = start of registry block (or data.len())
    registry_end: usize,
    length_scale: f32,
    previous_length_scale: f32,
    frame: i32,
    frame_count: i32,
    recorded_dt: f32,
    recorded_sub_step_count: i32,
    recorded_worker_count: i32,
    bounds: crate::math_functions::AABB,
    at_end: bool,
    /// first frame that diverged, -1 until then
    diverge_frame: i32,

    // Reader state (C: b3RecReader)
    cursor: usize,
    ok: bool,
    diverged: bool,
    pending_query_key: u64,
    reader: RecReader,

    // Outliner body list, indexed by creation ordinal; holes mark destroys.
    body_ids: Vec<BodyId>,
    frame0_body_ids: Vec<BodyId>,

    // Per-frame query store
    frame_queries: Vec<DrawQuery>,
    frame_hits: Vec<RecordedHit>,

    // Frame-0 restore image (offset, size) into `data`.
    frame0_snap: (usize, usize),

    // Keyframe ring
    keyframes: Vec<Keyframe>,
    keyframe_budget: usize,
    keyframe_bytes: usize,
    keyframe_min_interval: i32,
    keyframe_interval: i32,
    last_keyframe_frame: i32,

    // Pre-populated recording used by serialize_world during keyframe capture.
    keyframe_rec: Recording,

    /// The replay world (owned; C worlds live in a global registry).
    world: Option<World>,
}

// Dispatch context: the per-op mutable state threaded through the dispatchers
// while the world and the data buffer are moved out of the player.
struct DispatchSink<'a> {
    diverged: &'a mut bool,
    pending_query_key: &'a mut u64,
    reader: &'a RecReader,
    frame_queries: &'a mut Vec<DrawQuery>,
    frame_hits: &'a mut Vec<RecordedHit>,
    body_ids: &'a mut Vec<BodyId>,
    bounds: &'a mut crate::math_functions::AABB,
}

impl<'a> DispatchSink<'a> {
    // C: b3RecStashQueryBegin — push a draw record and copy hits.
    fn stash_query(&mut self, kind: RecQueryKind, hits: &[RecordedHit]) -> &mut DrawQuery {
        let mut q = DrawQuery::default();
        q.kind = kind;
        // Pair the query with the key from its preceding QueryTag op, if any.
        q.key = *self.pending_query_key;
        *self.pending_query_key = 0;
        q.hit_start = self.frame_hits.len() as i32;
        q.hit_count = hits.len() as i32;
        self.frame_hits.extend_from_slice(hits);
        self.frame_queries.push(q);
        self.frame_queries.last_mut().unwrap()
    }

    // C: b3RecTrackBodyCreate / b3RecTrackBodyDestroy.
    fn track_body_create(&mut self, id: BodyId) {
        self.body_ids.push(id);
    }

    fn track_body_destroy(&mut self, id: BodyId) {
        for slot in self.body_ids.iter_mut() {
            if *slot == id {
                *slot = NULL_BODY_ID;
                return;
            }
        }
    }
}

// Id retargeting: replace world0 with the replay world's slot (C: b3RecMake*Id).
#[inline]
fn remap_body_id(world: &World, recorded: BodyId) -> BodyId {
    BodyId { index1: recorded.index1, world0: world.world_id, generation: recorded.generation }
}

#[inline]
fn remap_shape_id(world: &World, recorded: ShapeId) -> ShapeId {
    ShapeId { index1: recorded.index1, world0: world.world_id, generation: recorded.generation }
}

#[inline]
fn remap_joint_id(world: &World, recorded: JointId) -> JointId {
    JointId { index1: recorded.index1, world0: world.world_id, generation: recorded.generation }
}

// A create op appends the returned id after args. index1 and generation must
// match; world0 always differs so it is ignored (C: b3RecCheckId).
fn check_id(ok: &mut bool, kind: &str, got_index: i32, got_gen: u32, rec_index: i32, rec_gen: u32) {
    if got_index != rec_index || got_gen != rec_gen {
        crate::core::log(&format!(
            "replay: {} id mismatch (rec index1={} gen={}, got index1={} gen={})",
            kind, rec_index, rec_gen, got_index, got_gen
        ));
        *ok = false;
    }
}

// Decode a RecOp from its wire value.
macro_rules! rec_op_table {
    ($v:expr; $($name:ident),* $(,)?) => {
        match $v {
            $(x if x == RecOp::$name as u8 => Some(RecOp::$name),)*
            _ => None,
        }
    };
}

fn rec_op_from_u8(v: u8) -> Option<RecOp> {
    rec_op_table!(v;
        DestroyWorld, WorldEnableSleeping, WorldEnableContinuous, WorldSetRestitutionThreshold,
        WorldSetHitEventThreshold, WorldSetGravity, WorldExplode, WorldSetContactTuning,
        WorldSetContactRecycleDistance, WorldSetMaximumLinearSpeed, WorldEnableWarmStarting,
        WorldRebuildStaticTree, WorldEnableSpeculative,
        CreateBody, DestroyBody, BodySetTransform, BodySetLinearVelocity, BodySetType, BodySetName,
        BodySetAngularVelocity, BodySetTargetTransform, BodyApplyForce, BodyApplyForceToCenter,
        BodyApplyTorque, BodyApplyLinearImpulse, BodyApplyLinearImpulseToCenter,
        BodyApplyAngularImpulse, BodySetMassData, BodyApplyMassFromShapes, BodySetLinearDamping,
        BodySetAngularDamping, BodySetGravityScale, BodySetAwake, BodyEnableSleep,
        BodySetSleepThreshold, BodyDisable, BodyEnable, BodySetMotionLocks, BodySetBullet,
        BodyEnableContactRecycling, BodyEnableHitEvents,
        CreateSphereShape, CreateCapsuleShape, CreateHullShape, CreateMeshShape,
        CreateHeightFieldShape, CreateCompoundShape, DestroyShape,
        ShapeSetDensity, ShapeSetFriction, ShapeSetRestitution, ShapeSetSurfaceMaterial,
        ShapeSetFilter, ShapeEnableSensorEvents, ShapeEnableContactEvents, ShapeEnablePreSolveEvents,
        ShapeEnableHitEvents, ShapeSetSphere, ShapeSetCapsule, ShapeApplyWind,
        Step,
        CreateParallelJoint, CreateDistanceJoint, CreateFilterJoint, CreateMotorJoint,
        CreatePrismaticJoint, CreateRevoluteJoint, CreateSphericalJoint, CreateWeldJoint,
        CreateWheelJoint, DestroyJoint,
        JointSetLocalFrameA, JointSetLocalFrameB, JointSetCollideConnected, JointWakeBodies,
        JointSetConstraintTuning, JointSetForceThreshold, JointSetTorqueThreshold,
        ParallelJointSetSpringHertz, ParallelJointSetSpringDampingRatio, ParallelJointSetMaxTorque,
        DistanceJointSetLength, DistanceJointEnableSpring, DistanceJointSetSpringForceRange,
        DistanceJointSetSpringHertz, DistanceJointSetSpringDampingRatio, DistanceJointEnableLimit,
        DistanceJointSetLengthRange, DistanceJointEnableMotor, DistanceJointSetMotorSpeed,
        DistanceJointSetMaxMotorForce,
        MotorJointSetLinearVelocity, MotorJointSetAngularVelocity, MotorJointSetMaxVelocityForce,
        MotorJointSetMaxVelocityTorque, MotorJointSetLinearHertz, MotorJointSetLinearDampingRatio,
        MotorJointSetAngularHertz, MotorJointSetAngularDampingRatio, MotorJointSetMaxSpringForce,
        MotorJointSetMaxSpringTorque,
        PrismaticJointEnableSpring, PrismaticJointSetSpringHertz, PrismaticJointSetSpringDampingRatio,
        PrismaticJointSetTargetTranslation, PrismaticJointEnableLimit, PrismaticJointSetLimits,
        PrismaticJointEnableMotor, PrismaticJointSetMotorSpeed, PrismaticJointSetMaxMotorForce,
        RevoluteJointEnableSpring, RevoluteJointSetSpringHertz, RevoluteJointSetSpringDampingRatio,
        RevoluteJointSetTargetAngle, RevoluteJointEnableLimit, RevoluteJointSetLimits,
        RevoluteJointEnableMotor, RevoluteJointSetMotorSpeed, RevoluteJointSetMaxMotorTorque,
        SphericalJointEnableConeLimit, SphericalJointSetConeLimit, SphericalJointEnableTwistLimit,
        SphericalJointSetTwistLimits, SphericalJointEnableSpring, SphericalJointSetSpringHertz,
        SphericalJointSetSpringDampingRatio, SphericalJointSetTargetRotation,
        SphericalJointEnableMotor, SphericalJointSetMotorVelocity, SphericalJointSetMaxMotorTorque,
        WeldJointSetLinearHertz, WeldJointSetLinearDampingRatio, WeldJointSetAngularHertz,
        WeldJointSetAngularDampingRatio,
        WheelJointEnableSuspension, WheelJointSetSuspensionHertz, WheelJointSetSuspensionDampingRatio,
        WheelJointEnableSuspensionLimit, WheelJointSetSuspensionLimits, WheelJointEnableSpinMotor,
        WheelJointSetSpinMotorSpeed, WheelJointSetMaxSpinTorque, WheelJointEnableSteering,
        WheelJointSetSteeringHertz, WheelJointSetSteeringDampingRatio, WheelJointSetMaxSteeringTorque,
        WheelJointEnableSteeringLimit, WheelJointSetSteeringLimits, WheelJointSetTargetSteeringAngle,
        QueryOverlapAABB, QueryOverlapShape, QueryCastRay, QueryCastShape, QueryCastRayClosest,
        QueryCastMover, QueryCollideMover, QueryTag,
        StateHash, RecordingBounds,
    )
}

// Read the shared overlap-style hit list: { shapeid, bool } * n.
fn read_overlap_hits(r: &mut RecCursor, world: &World) -> Option<Vec<RecordedHit>> {
    let n = r.r_u32();
    if !r.ok || n as usize > r.remaining() {
        r.ok = false;
        return None;
    }
    let mut hits = Vec::with_capacity(n as usize);
    for _ in 0..n {
        let mut h = RecordedHit::default();
        h.id = remap_shape_id(world, r.r_shapeid());
        h.user_return_b = r.r_bool();
        hits.push(h);
    }
    if r.ok {
        Some(hits)
    } else {
        None
    }
}

// Read the shared cast-style hit list.
fn read_cast_hits(r: &mut RecCursor, world: &World) -> Option<Vec<RecordedHit>> {
    let n = r.r_u32();
    if !r.ok || n as usize > r.remaining() {
        r.ok = false;
        return None;
    }
    let mut hits = Vec::with_capacity(n as usize);
    for _ in 0..n {
        let mut h = RecordedHit::default();
        h.id = remap_shape_id(world, r.r_shapeid());
        h.point = r.r_position();
        h.normal = r.r_vec3();
        h.fraction = r.r_f32();
        h.user_material_id = r.r_u64();
        h.triangle_index = r.r_i32();
        h.child_index = r.r_i32();
        h.user_return_f = r.r_f32();
        hits.push(h);
    }
    if r.ok {
        Some(hits)
    } else {
        None
    }
}

// The overlap replay trampoline body (C: b3RecReplayOverlapTrampoline).
fn replay_overlap_hit(hits: &[RecordedHit], cursor: &mut usize, diverged: &mut bool, id: ShapeId) -> bool {
    if *cursor >= hits.len() {
        *diverged = true;
        return false;
    }
    let h = &hits[*cursor];
    *cursor += 1;
    if id.index1 != h.id.index1 || id.generation != h.id.generation {
        *diverged = true;
    }
    h.user_return_b
}

/// Dispatch one record against the world (C: b3RecDispatchOne). Returns the
/// opcode dispatched, or -1 when the stream is exhausted or broken.
#[allow(clippy::too_many_lines)]
fn dispatch_one(
    data: &[u8],
    cursor: &mut usize,
    registry_end: usize,
    ok: &mut bool,
    world: &mut World,
    sink: &mut DispatchSink,
) -> i32 {
    if *cursor >= registry_end || !*ok {
        return -1;
    }

    let mut r = RecCursor::new(&data[..registry_end]);
    r.cursor = *cursor;
    let opcode = r.r_u8();
    let payload_size = r.r_u24() as usize;
    if !r.ok {
        *ok = false;
        return -1;
    }
    let payload_start = r.cursor;

    let op = match rec_op_from_u8(opcode) {
        Some(op) => op,
        None => {
            crate::core::log(&format!("replay: unknown opcode 0x{:02X}, skipping {} bytes", opcode, payload_size));
            if payload_size > registry_end - payload_start {
                *ok = false;
            } else {
                *cursor = payload_start + payload_size;
            }
            return opcode as i32;
        }
    };

    match op {
        RecOp::DestroyWorld => {
            let _wid = r.r_worldid();
            // End-of-session marker; the caller stops the frame loop.
        }
        RecOp::Step => {
            let _wid = r.r_worldid();
            let dt = r.r_f32();
            let sub_step_count = r.r_i32();
            if r.ok {
                crate::physics_world::world_step(world, dt, sub_step_count);
            }
        }
        RecOp::StateHash => {
            let _wid = r.r_worldid();
            let recorded = r.r_u64();
            if r.ok {
                let computed = hash_world_state(world);
                if computed != recorded {
                    crate::core::log(&format!(
                        "replay: StateHash mismatch (recorded=0x{:016X}, computed=0x{:016X})",
                        recorded, computed
                    ));
                    *sink.diverged = true;
                }
            }
        }
        RecOp::RecordingBounds => {
            let bounds = r.r_aabb();
            if r.ok {
                *sink.bounds = bounds;
            }
        }

        // World config
        RecOp::WorldEnableSleeping => {
            let _wid = r.r_worldid();
            let flag = r.r_bool();
            if r.ok {
                crate::physics_world::world_enable_sleeping(world, flag);
            }
        }
        RecOp::WorldEnableContinuous => {
            let _wid = r.r_worldid();
            let flag = r.r_bool();
            if r.ok {
                crate::physics_world::world_enable_continuous(world, flag);
            }
        }
        RecOp::WorldSetRestitutionThreshold => {
            let _wid = r.r_worldid();
            let value = r.r_f32();
            if r.ok {
                crate::physics_world::world_set_restitution_threshold(world, value);
            }
        }
        RecOp::WorldSetHitEventThreshold => {
            let _wid = r.r_worldid();
            let value = r.r_f32();
            if r.ok {
                crate::physics_world::world_set_hit_event_threshold(world, value);
            }
        }
        RecOp::WorldSetGravity => {
            let _wid = r.r_worldid();
            let gravity = r.r_vec3();
            if r.ok {
                crate::physics_world::world_set_gravity(world, gravity);
            }
        }
        RecOp::WorldExplode => {
            let _wid = r.r_worldid();
            let def = r.r_explosiondef();
            if r.ok {
                crate::physics_world::world_explode(world, &def);
            }
        }
        RecOp::WorldSetContactTuning => {
            let _wid = r.r_worldid();
            let hertz = r.r_f32();
            let damping_ratio = r.r_f32();
            let contact_speed = r.r_f32();
            if r.ok {
                crate::physics_world::world_set_contact_tuning(world, hertz, damping_ratio, contact_speed);
            }
        }
        RecOp::WorldSetContactRecycleDistance => {
            let _wid = r.r_worldid();
            let value = r.r_f32();
            if r.ok {
                crate::physics_world::world_set_contact_recycle_distance(world, value);
            }
        }
        RecOp::WorldSetMaximumLinearSpeed => {
            let _wid = r.r_worldid();
            let value = r.r_f32();
            if r.ok {
                crate::physics_world::world_set_maximum_linear_speed(world, value);
            }
        }
        RecOp::WorldEnableWarmStarting => {
            let _wid = r.r_worldid();
            let flag = r.r_bool();
            if r.ok {
                crate::physics_world::world_enable_warm_starting(world, flag);
            }
        }
        RecOp::WorldRebuildStaticTree => {
            let _wid = r.r_worldid();
            if r.ok {
                crate::physics_world::world_rebuild_static_tree(world);
            }
        }
        RecOp::WorldEnableSpeculative => {
            let _wid = r.r_worldid();
            let flag = r.r_bool();
            if r.ok {
                crate::physics_world::world_enable_speculative(world, flag);
            }
        }

        // Bodies
        RecOp::CreateBody => {
            let _wid = r.r_worldid();
            let def = r.r_bodydef();
            let rec_id = r.r_bodyid();
            if r.ok {
                let got = crate::body::create_body(world, &def);
                check_id(ok, "body", got.index1, got.generation as u32, rec_id.index1, rec_id.generation as u32);
                sink.track_body_create(got);
            }
        }
        RecOp::DestroyBody => {
            let id = remap_body_id(world, r.r_bodyid());
            if r.ok {
                sink.track_body_destroy(id);
                crate::body::destroy_body(world, id);
            }
        }
        RecOp::BodySetTransform => {
            let id = remap_body_id(world, r.r_bodyid());
            let position = r.r_position();
            let rotation = r.r_quat();
            if r.ok {
                crate::body::body_set_transform(world, id, position, rotation);
            }
        }
        RecOp::BodySetLinearVelocity => {
            let id = remap_body_id(world, r.r_bodyid());
            let v = r.r_vec3();
            if r.ok {
                crate::body::body_set_linear_velocity(world, id, v);
            }
        }
        RecOp::BodySetAngularVelocity => {
            let id = remap_body_id(world, r.r_bodyid());
            let v = r.r_vec3();
            if r.ok {
                crate::body::body_set_angular_velocity(world, id, v);
            }
        }
        RecOp::BodySetType => {
            let id = remap_body_id(world, r.r_bodyid());
            let t = r.r_i32();
            let body_type = match t {
                0 => BodyType::Static,
                1 => BodyType::Kinematic,
                2 => BodyType::Dynamic,
                _ => {
                    r.ok = false;
                    BodyType::Static
                }
            };
            if r.ok {
                crate::body::body_set_type(world, id, body_type);
            }
        }
        RecOp::BodySetName => {
            let id = remap_body_id(world, r.r_bodyid());
            let name = r.r_string();
            if r.ok {
                crate::body::body_set_name(world, id, &name);
            }
        }
        RecOp::BodySetTargetTransform => {
            let id = remap_body_id(world, r.r_bodyid());
            let target = r.r_worldxf();
            let time_step = r.r_f32();
            let wake = r.r_bool();
            if r.ok {
                crate::body::body_set_target_transform(world, id, target, time_step, wake);
            }
        }
        RecOp::BodyApplyForce => {
            let id = remap_body_id(world, r.r_bodyid());
            let force = r.r_vec3();
            let point = r.r_position();
            let wake = r.r_bool();
            if r.ok {
                crate::body::body_apply_force(world, id, force, point, wake);
            }
        }
        RecOp::BodyApplyForceToCenter => {
            let id = remap_body_id(world, r.r_bodyid());
            let force = r.r_vec3();
            let wake = r.r_bool();
            if r.ok {
                crate::body::body_apply_force_to_center(world, id, force, wake);
            }
        }
        RecOp::BodyApplyTorque => {
            let id = remap_body_id(world, r.r_bodyid());
            let torque = r.r_vec3();
            let wake = r.r_bool();
            if r.ok {
                crate::body::body_apply_torque(world, id, torque, wake);
            }
        }
        RecOp::BodyApplyLinearImpulse => {
            let id = remap_body_id(world, r.r_bodyid());
            let impulse = r.r_vec3();
            let point = r.r_position();
            let wake = r.r_bool();
            if r.ok {
                crate::body::body_apply_linear_impulse(world, id, impulse, point, wake);
            }
        }
        RecOp::BodyApplyLinearImpulseToCenter => {
            let id = remap_body_id(world, r.r_bodyid());
            let impulse = r.r_vec3();
            let wake = r.r_bool();
            if r.ok {
                crate::body::body_apply_linear_impulse_to_center(world, id, impulse, wake);
            }
        }
        RecOp::BodyApplyAngularImpulse => {
            let id = remap_body_id(world, r.r_bodyid());
            let impulse = r.r_vec3();
            let wake = r.r_bool();
            if r.ok {
                crate::body::body_apply_angular_impulse(world, id, impulse, wake);
            }
        }
        RecOp::BodySetMassData => {
            let id = remap_body_id(world, r.r_bodyid());
            let mass_data = r.r_massdata();
            if r.ok {
                crate::body::body_set_mass_data(world, id, mass_data);
            }
        }
        RecOp::BodyApplyMassFromShapes => {
            let id = remap_body_id(world, r.r_bodyid());
            if r.ok {
                crate::body::body_apply_mass_from_shapes(world, id);
            }
        }
        RecOp::BodySetLinearDamping => {
            let id = remap_body_id(world, r.r_bodyid());
            let v = r.r_f32();
            if r.ok {
                crate::body::body_set_linear_damping(world, id, v);
            }
        }
        RecOp::BodySetAngularDamping => {
            let id = remap_body_id(world, r.r_bodyid());
            let v = r.r_f32();
            if r.ok {
                crate::body::body_set_angular_damping(world, id, v);
            }
        }
        RecOp::BodySetGravityScale => {
            let id = remap_body_id(world, r.r_bodyid());
            let v = r.r_f32();
            if r.ok {
                crate::body::body_set_gravity_scale(world, id, v);
            }
        }
        RecOp::BodySetAwake => {
            let id = remap_body_id(world, r.r_bodyid());
            let flag = r.r_bool();
            if r.ok {
                crate::body::body_set_awake(world, id, flag);
            }
        }
        RecOp::BodyEnableSleep => {
            let id = remap_body_id(world, r.r_bodyid());
            let flag = r.r_bool();
            if r.ok {
                crate::body::body_enable_sleep(world, id, flag);
            }
        }
        RecOp::BodySetSleepThreshold => {
            let id = remap_body_id(world, r.r_bodyid());
            let v = r.r_f32();
            if r.ok {
                crate::body::body_set_sleep_threshold(world, id, v);
            }
        }
        RecOp::BodyDisable => {
            let id = remap_body_id(world, r.r_bodyid());
            if r.ok {
                crate::body::body_disable(world, id);
            }
        }
        RecOp::BodyEnable => {
            let id = remap_body_id(world, r.r_bodyid());
            if r.ok {
                crate::body::body_enable(world, id);
            }
        }
        RecOp::BodySetMotionLocks => {
            let id = remap_body_id(world, r.r_bodyid());
            let locks = r.r_locks();
            if r.ok {
                crate::body::body_set_motion_locks(world, id, locks);
            }
        }
        RecOp::BodySetBullet => {
            let id = remap_body_id(world, r.r_bodyid());
            let flag = r.r_bool();
            if r.ok {
                crate::body::body_set_bullet(world, id, flag);
            }
        }
        RecOp::BodyEnableContactRecycling => {
            let id = remap_body_id(world, r.r_bodyid());
            let flag = r.r_bool();
            if r.ok {
                crate::body::body_enable_contact_recycling(world, id, flag);
            }
        }
        RecOp::BodyEnableHitEvents => {
            let id = remap_body_id(world, r.r_bodyid());
            let flag = r.r_bool();
            if r.ok {
                crate::body::body_enable_hit_events(world, id, flag);
            }
        }

        // Shapes
        RecOp::CreateSphereShape => {
            let body_id = remap_body_id(world, r.r_bodyid());
            let def = r.r_shapedef();
            let sphere = r.r_sphere();
            let rec_id = r.r_shapeid();
            if r.ok {
                let got = crate::shape::create_sphere_shape(world, body_id, &def, &sphere);
                check_id(ok, "shape", got.index1, got.generation as u32, rec_id.index1, rec_id.generation as u32);
            }
        }
        RecOp::CreateCapsuleShape => {
            let body_id = remap_body_id(world, r.r_bodyid());
            let def = r.r_shapedef();
            let capsule = r.r_capsule();
            let rec_id = r.r_shapeid();
            if r.ok {
                let got = crate::shape::create_capsule_shape(world, body_id, &def, &capsule);
                check_id(ok, "shape", got.index1, got.generation as u32, rec_id.index1, rec_id.generation as u32);
            }
        }
        RecOp::CreateHullShape => {
            let body_id = remap_body_id(world, r.r_bodyid());
            let def = r.r_shapedef();
            let geom_id = r.r_geomid() as usize;
            let rec_id = r.r_shapeid();
            if r.ok {
                if geom_id >= sink.reader.slots.len() {
                    *ok = false;
                } else if let RegistryGeometry::Hull(hull) = &sink.reader.slots[geom_id].geometry {
                    let hull = Arc::clone(hull);
                    let got = crate::shape::create_hull_shape(world, body_id, &def, &hull);
                    check_id(ok, "shape", got.index1, got.generation as u32, rec_id.index1, rec_id.generation as u32);
                } else {
                    *ok = false;
                }
            }
        }
        RecOp::CreateMeshShape => {
            let body_id = remap_body_id(world, r.r_bodyid());
            let def = r.r_shapedef();
            let geom_id = r.r_geomid() as usize;
            let scale = r.r_vec3();
            let rec_id = r.r_shapeid();
            if r.ok {
                if geom_id >= sink.reader.slots.len() {
                    *ok = false;
                } else if let RegistryGeometry::Mesh(mesh) = &sink.reader.slots[geom_id].geometry {
                    let mesh = Arc::clone(mesh);
                    let got = crate::shape::create_mesh_shape(world, body_id, &def, &mesh, scale);
                    check_id(ok, "shape", got.index1, got.generation as u32, rec_id.index1, rec_id.generation as u32);
                } else {
                    *ok = false;
                }
            }
        }
        RecOp::CreateHeightFieldShape => {
            let body_id = remap_body_id(world, r.r_bodyid());
            let def = r.r_shapedef();
            let geom_id = r.r_geomid() as usize;
            let rec_id = r.r_shapeid();
            if r.ok {
                if geom_id >= sink.reader.slots.len() {
                    *ok = false;
                } else if let RegistryGeometry::HeightField(hf) = &sink.reader.slots[geom_id].geometry {
                    let hf = Arc::clone(hf);
                    let got = crate::shape::create_height_field_shape(world, body_id, &def, &hf);
                    check_id(ok, "shape", got.index1, got.generation as u32, rec_id.index1, rec_id.generation as u32);
                } else {
                    *ok = false;
                }
            }
        }
        RecOp::CreateCompoundShape => {
            let body_id = remap_body_id(world, r.r_bodyid());
            let def = r.r_shapedef();
            let geom_id = r.r_geomid() as usize;
            let rec_id = r.r_shapeid();
            if r.ok {
                if geom_id >= sink.reader.slots.len() {
                    *ok = false;
                } else if let RegistryGeometry::Compound(compound) = &sink.reader.slots[geom_id].geometry {
                    let compound = Arc::clone(compound);
                    let got = crate::shape::create_compound_shape(world, body_id, &def, &compound);
                    check_id(ok, "shape", got.index1, got.generation as u32, rec_id.index1, rec_id.generation as u32);
                } else {
                    *ok = false;
                }
            }
        }
        RecOp::DestroyShape => {
            let id = remap_shape_id(world, r.r_shapeid());
            let update_body_mass = r.r_bool();
            if r.ok {
                crate::shape::destroy_shape(world, id, update_body_mass);
            }
        }
        RecOp::ShapeSetDensity => {
            let id = remap_shape_id(world, r.r_shapeid());
            let density = r.r_f32();
            let update_body_mass = r.r_bool();
            if r.ok {
                crate::shape::shape_set_density(world, id, density, update_body_mass);
            }
        }
        RecOp::ShapeSetFriction => {
            let id = remap_shape_id(world, r.r_shapeid());
            let v = r.r_f32();
            if r.ok {
                crate::shape::shape_set_friction(world, id, v);
            }
        }
        RecOp::ShapeSetRestitution => {
            let id = remap_shape_id(world, r.r_shapeid());
            let v = r.r_f32();
            if r.ok {
                crate::shape::shape_set_restitution(world, id, v);
            }
        }
        RecOp::ShapeSetSurfaceMaterial => {
            let id = remap_shape_id(world, r.r_shapeid());
            let material = r.r_material();
            if r.ok {
                crate::shape::shape_set_surface_material(world, id, material);
            }
        }
        RecOp::ShapeSetFilter => {
            let id = remap_shape_id(world, r.r_shapeid());
            let filter = r.r_filter();
            let invoke_contacts = r.r_bool();
            if r.ok {
                crate::shape::shape_set_filter(world, id, filter, invoke_contacts);
            }
        }
        RecOp::ShapeEnableSensorEvents => {
            let id = remap_shape_id(world, r.r_shapeid());
            let flag = r.r_bool();
            if r.ok {
                crate::shape::shape_enable_sensor_events(world, id, flag);
            }
        }
        RecOp::ShapeEnableContactEvents => {
            let id = remap_shape_id(world, r.r_shapeid());
            let flag = r.r_bool();
            if r.ok {
                crate::shape::shape_enable_contact_events(world, id, flag);
            }
        }
        RecOp::ShapeEnablePreSolveEvents => {
            let id = remap_shape_id(world, r.r_shapeid());
            let flag = r.r_bool();
            if r.ok {
                crate::shape::shape_enable_pre_solve_events(world, id, flag);
            }
        }
        RecOp::ShapeEnableHitEvents => {
            let id = remap_shape_id(world, r.r_shapeid());
            let flag = r.r_bool();
            if r.ok {
                crate::shape::shape_enable_hit_events(world, id, flag);
            }
        }
        RecOp::ShapeSetSphere => {
            let id = remap_shape_id(world, r.r_shapeid());
            let sphere = r.r_sphere();
            if r.ok {
                crate::shape::shape_set_sphere(world, id, &sphere);
            }
        }
        RecOp::ShapeSetCapsule => {
            let id = remap_shape_id(world, r.r_shapeid());
            let capsule = r.r_capsule();
            if r.ok {
                crate::shape::shape_set_capsule(world, id, &capsule);
            }
        }
        RecOp::ShapeApplyWind => {
            let id = remap_shape_id(world, r.r_shapeid());
            let wind = r.r_vec3();
            let drag = r.r_f32();
            let lift = r.r_f32();
            let max_speed = r.r_f32();
            let wake = r.r_bool();
            if r.ok {
                crate::shape::shape_apply_wind(world, id, wind, drag, lift, max_speed, wake);
            }
        }

        // Joints
        RecOp::CreateParallelJoint => {
            let _wid = r.r_worldid();
            let mut def = r.r_paralleljointdef();
            let rec_id = r.r_jointid();
            if r.ok {
                def.base.body_id_a = remap_body_id(world, def.base.body_id_a);
                def.base.body_id_b = remap_body_id(world, def.base.body_id_b);
                let got = crate::joint::create_parallel_joint(world, &def);
                check_id(ok, "joint", got.index1, got.generation as u32, rec_id.index1, rec_id.generation as u32);
            }
        }
        RecOp::CreateDistanceJoint => {
            let _wid = r.r_worldid();
            let mut def = r.r_distancejointdef();
            let rec_id = r.r_jointid();
            if r.ok {
                def.base.body_id_a = remap_body_id(world, def.base.body_id_a);
                def.base.body_id_b = remap_body_id(world, def.base.body_id_b);
                let got = crate::joint::create_distance_joint(world, &def);
                check_id(ok, "joint", got.index1, got.generation as u32, rec_id.index1, rec_id.generation as u32);
            }
        }
        RecOp::CreateFilterJoint => {
            let _wid = r.r_worldid();
            let mut def = r.r_filterjointdef();
            let rec_id = r.r_jointid();
            if r.ok {
                def.base.body_id_a = remap_body_id(world, def.base.body_id_a);
                def.base.body_id_b = remap_body_id(world, def.base.body_id_b);
                let got = crate::joint::create_filter_joint(world, &def);
                check_id(ok, "joint", got.index1, got.generation as u32, rec_id.index1, rec_id.generation as u32);
            }
        }
        RecOp::CreateMotorJoint => {
            let _wid = r.r_worldid();
            let mut def = r.r_motorjointdef();
            let rec_id = r.r_jointid();
            if r.ok {
                def.base.body_id_a = remap_body_id(world, def.base.body_id_a);
                def.base.body_id_b = remap_body_id(world, def.base.body_id_b);
                let got = crate::joint::create_motor_joint(world, &def);
                check_id(ok, "joint", got.index1, got.generation as u32, rec_id.index1, rec_id.generation as u32);
            }
        }
        RecOp::CreatePrismaticJoint => {
            let _wid = r.r_worldid();
            let mut def = r.r_prismaticjointdef();
            let rec_id = r.r_jointid();
            if r.ok {
                def.base.body_id_a = remap_body_id(world, def.base.body_id_a);
                def.base.body_id_b = remap_body_id(world, def.base.body_id_b);
                let got = crate::joint::create_prismatic_joint(world, &def);
                check_id(ok, "joint", got.index1, got.generation as u32, rec_id.index1, rec_id.generation as u32);
            }
        }
        RecOp::CreateRevoluteJoint => {
            let _wid = r.r_worldid();
            let mut def = r.r_revolutejointdef();
            let rec_id = r.r_jointid();
            if r.ok {
                def.base.body_id_a = remap_body_id(world, def.base.body_id_a);
                def.base.body_id_b = remap_body_id(world, def.base.body_id_b);
                let got = crate::joint::create_revolute_joint(world, &def);
                check_id(ok, "joint", got.index1, got.generation as u32, rec_id.index1, rec_id.generation as u32);
            }
        }
        RecOp::CreateSphericalJoint => {
            let _wid = r.r_worldid();
            let mut def = r.r_sphericaljointdef();
            let rec_id = r.r_jointid();
            if r.ok {
                def.base.body_id_a = remap_body_id(world, def.base.body_id_a);
                def.base.body_id_b = remap_body_id(world, def.base.body_id_b);
                let got = crate::joint::create_spherical_joint(world, &def);
                check_id(ok, "joint", got.index1, got.generation as u32, rec_id.index1, rec_id.generation as u32);
            }
        }
        RecOp::CreateWeldJoint => {
            let _wid = r.r_worldid();
            let mut def = r.r_weldjointdef();
            let rec_id = r.r_jointid();
            if r.ok {
                def.base.body_id_a = remap_body_id(world, def.base.body_id_a);
                def.base.body_id_b = remap_body_id(world, def.base.body_id_b);
                let got = crate::joint::create_weld_joint(world, &def);
                check_id(ok, "joint", got.index1, got.generation as u32, rec_id.index1, rec_id.generation as u32);
            }
        }
        RecOp::CreateWheelJoint => {
            let _wid = r.r_worldid();
            let mut def = r.r_wheeljointdef();
            let rec_id = r.r_jointid();
            if r.ok {
                def.base.body_id_a = remap_body_id(world, def.base.body_id_a);
                def.base.body_id_b = remap_body_id(world, def.base.body_id_b);
                let got = crate::joint::create_wheel_joint(world, &def);
                check_id(ok, "joint", got.index1, got.generation as u32, rec_id.index1, rec_id.generation as u32);
            }
        }
        RecOp::DestroyJoint => {
            let id = remap_joint_id(world, r.r_jointid());
            let wake = r.r_bool();
            if r.ok {
                crate::joint::destroy_joint(world, id, wake);
            }
        }
        RecOp::JointSetLocalFrameA => {
            let id = remap_joint_id(world, r.r_jointid());
            let frame = r.r_transform();
            if r.ok {
                crate::joint::joint_set_local_frame_a(world, id, frame);
            }
        }
        RecOp::JointSetLocalFrameB => {
            let id = remap_joint_id(world, r.r_jointid());
            let frame = r.r_transform();
            if r.ok {
                crate::joint::joint_set_local_frame_b(world, id, frame);
            }
        }
        RecOp::JointSetCollideConnected => {
            let id = remap_joint_id(world, r.r_jointid());
            let flag = r.r_bool();
            if r.ok {
                crate::joint::joint_set_collide_connected(world, id, flag);
            }
        }
        RecOp::JointWakeBodies => {
            let id = remap_joint_id(world, r.r_jointid());
            if r.ok {
                crate::joint::joint_wake_bodies(world, id);
            }
        }
        RecOp::JointSetConstraintTuning => {
            let id = remap_joint_id(world, r.r_jointid());
            let hertz = r.r_f32();
            let damping_ratio = r.r_f32();
            if r.ok {
                crate::joint::joint_set_constraint_tuning(world, id, hertz, damping_ratio);
            }
        }
        RecOp::JointSetForceThreshold => {
            let id = remap_joint_id(world, r.r_jointid());
            let v = r.r_f32();
            if r.ok {
                crate::joint::joint_set_force_threshold(world, id, v);
            }
        }
        RecOp::JointSetTorqueThreshold => {
            let id = remap_joint_id(world, r.r_jointid());
            let v = r.r_f32();
            if r.ok {
                crate::joint::joint_set_torque_threshold(world, id, v);
            }
        }

        // Typed joint setters: { jointid, args... } -> the typed public API.
        RecOp::ParallelJointSetSpringHertz => {
            let id = remap_joint_id(world, r.r_jointid());
            let v = r.r_f32();
            if r.ok {
                crate::parallel_joint::parallel_joint_set_spring_hertz(world, id, v);
            }
        }
        RecOp::ParallelJointSetSpringDampingRatio => {
            let id = remap_joint_id(world, r.r_jointid());
            let v = r.r_f32();
            if r.ok {
                crate::parallel_joint::parallel_joint_set_spring_damping_ratio(world, id, v);
            }
        }
        RecOp::ParallelJointSetMaxTorque => {
            let id = remap_joint_id(world, r.r_jointid());
            let v = r.r_f32();
            if r.ok {
                crate::parallel_joint::parallel_joint_set_max_torque(world, id, v);
            }
        }
        RecOp::DistanceJointSetLength => {
            let id = remap_joint_id(world, r.r_jointid());
            let v = r.r_f32();
            if r.ok {
                crate::distance_joint::distance_joint_set_length(world, id, v);
            }
        }
        RecOp::DistanceJointEnableSpring => {
            let id = remap_joint_id(world, r.r_jointid());
            let flag = r.r_bool();
            if r.ok {
                crate::distance_joint::distance_joint_enable_spring(world, id, flag);
            }
        }
        RecOp::DistanceJointSetSpringForceRange => {
            let id = remap_joint_id(world, r.r_jointid());
            let lower = r.r_f32();
            let upper = r.r_f32();
            if r.ok {
                crate::distance_joint::distance_joint_set_spring_force_range(world, id, lower, upper);
            }
        }
        RecOp::DistanceJointSetSpringHertz => {
            let id = remap_joint_id(world, r.r_jointid());
            let v = r.r_f32();
            if r.ok {
                crate::distance_joint::distance_joint_set_spring_hertz(world, id, v);
            }
        }
        RecOp::DistanceJointSetSpringDampingRatio => {
            let id = remap_joint_id(world, r.r_jointid());
            let v = r.r_f32();
            if r.ok {
                crate::distance_joint::distance_joint_set_spring_damping_ratio(world, id, v);
            }
        }
        RecOp::DistanceJointEnableLimit => {
            let id = remap_joint_id(world, r.r_jointid());
            let flag = r.r_bool();
            if r.ok {
                crate::distance_joint::distance_joint_enable_limit(world, id, flag);
            }
        }
        RecOp::DistanceJointSetLengthRange => {
            let id = remap_joint_id(world, r.r_jointid());
            let min_length = r.r_f32();
            let max_length = r.r_f32();
            if r.ok {
                crate::distance_joint::distance_joint_set_length_range(world, id, min_length, max_length);
            }
        }
        RecOp::DistanceJointEnableMotor => {
            let id = remap_joint_id(world, r.r_jointid());
            let flag = r.r_bool();
            if r.ok {
                crate::distance_joint::distance_joint_enable_motor(world, id, flag);
            }
        }
        RecOp::DistanceJointSetMotorSpeed => {
            let id = remap_joint_id(world, r.r_jointid());
            let v = r.r_f32();
            if r.ok {
                crate::distance_joint::distance_joint_set_motor_speed(world, id, v);
            }
        }
        RecOp::DistanceJointSetMaxMotorForce => {
            let id = remap_joint_id(world, r.r_jointid());
            let v = r.r_f32();
            if r.ok {
                crate::distance_joint::distance_joint_set_max_motor_force(world, id, v);
            }
        }
        RecOp::MotorJointSetLinearVelocity => {
            let id = remap_joint_id(world, r.r_jointid());
            let v = r.r_vec3();
            if r.ok {
                crate::motor_joint::motor_joint_set_linear_velocity(world, id, v);
            }
        }
        RecOp::MotorJointSetAngularVelocity => {
            let id = remap_joint_id(world, r.r_jointid());
            let v = r.r_vec3();
            if r.ok {
                crate::motor_joint::motor_joint_set_angular_velocity(world, id, v);
            }
        }
        RecOp::MotorJointSetMaxVelocityForce => {
            let id = remap_joint_id(world, r.r_jointid());
            let v = r.r_f32();
            if r.ok {
                crate::motor_joint::motor_joint_set_max_velocity_force(world, id, v);
            }
        }
        RecOp::MotorJointSetMaxVelocityTorque => {
            let id = remap_joint_id(world, r.r_jointid());
            let v = r.r_f32();
            if r.ok {
                crate::motor_joint::motor_joint_set_max_velocity_torque(world, id, v);
            }
        }
        RecOp::MotorJointSetLinearHertz => {
            let id = remap_joint_id(world, r.r_jointid());
            let v = r.r_f32();
            if r.ok {
                crate::motor_joint::motor_joint_set_linear_hertz(world, id, v);
            }
        }
        RecOp::MotorJointSetLinearDampingRatio => {
            let id = remap_joint_id(world, r.r_jointid());
            let v = r.r_f32();
            if r.ok {
                crate::motor_joint::motor_joint_set_linear_damping_ratio(world, id, v);
            }
        }
        RecOp::MotorJointSetAngularHertz => {
            let id = remap_joint_id(world, r.r_jointid());
            let v = r.r_f32();
            if r.ok {
                crate::motor_joint::motor_joint_set_angular_hertz(world, id, v);
            }
        }
        RecOp::MotorJointSetAngularDampingRatio => {
            let id = remap_joint_id(world, r.r_jointid());
            let v = r.r_f32();
            if r.ok {
                crate::motor_joint::motor_joint_set_angular_damping_ratio(world, id, v);
            }
        }
        RecOp::MotorJointSetMaxSpringForce => {
            let id = remap_joint_id(world, r.r_jointid());
            let v = r.r_f32();
            if r.ok {
                crate::motor_joint::motor_joint_set_max_spring_force(world, id, v);
            }
        }
        RecOp::MotorJointSetMaxSpringTorque => {
            let id = remap_joint_id(world, r.r_jointid());
            let v = r.r_f32();
            if r.ok {
                crate::motor_joint::motor_joint_set_max_spring_torque(world, id, v);
            }
        }
        RecOp::PrismaticJointEnableSpring => {
            let id = remap_joint_id(world, r.r_jointid());
            let flag = r.r_bool();
            if r.ok {
                crate::prismatic_joint::prismatic_joint_enable_spring(world, id, flag);
            }
        }
        RecOp::PrismaticJointSetSpringHertz => {
            let id = remap_joint_id(world, r.r_jointid());
            let v = r.r_f32();
            if r.ok {
                crate::prismatic_joint::prismatic_joint_set_spring_hertz(world, id, v);
            }
        }
        RecOp::PrismaticJointSetSpringDampingRatio => {
            let id = remap_joint_id(world, r.r_jointid());
            let v = r.r_f32();
            if r.ok {
                crate::prismatic_joint::prismatic_joint_set_spring_damping_ratio(world, id, v);
            }
        }
        RecOp::PrismaticJointSetTargetTranslation => {
            let id = remap_joint_id(world, r.r_jointid());
            let v = r.r_f32();
            if r.ok {
                crate::prismatic_joint::prismatic_joint_set_target_translation(world, id, v);
            }
        }
        RecOp::PrismaticJointEnableLimit => {
            let id = remap_joint_id(world, r.r_jointid());
            let flag = r.r_bool();
            if r.ok {
                crate::prismatic_joint::prismatic_joint_enable_limit(world, id, flag);
            }
        }
        RecOp::PrismaticJointSetLimits => {
            let id = remap_joint_id(world, r.r_jointid());
            let lower = r.r_f32();
            let upper = r.r_f32();
            if r.ok {
                crate::prismatic_joint::prismatic_joint_set_limits(world, id, lower, upper);
            }
        }
        RecOp::PrismaticJointEnableMotor => {
            let id = remap_joint_id(world, r.r_jointid());
            let flag = r.r_bool();
            if r.ok {
                crate::prismatic_joint::prismatic_joint_enable_motor(world, id, flag);
            }
        }
        RecOp::PrismaticJointSetMotorSpeed => {
            let id = remap_joint_id(world, r.r_jointid());
            let v = r.r_f32();
            if r.ok {
                crate::prismatic_joint::prismatic_joint_set_motor_speed(world, id, v);
            }
        }
        RecOp::PrismaticJointSetMaxMotorForce => {
            let id = remap_joint_id(world, r.r_jointid());
            let v = r.r_f32();
            if r.ok {
                crate::prismatic_joint::prismatic_joint_set_max_motor_force(world, id, v);
            }
        }
        RecOp::RevoluteJointEnableSpring => {
            let id = remap_joint_id(world, r.r_jointid());
            let flag = r.r_bool();
            if r.ok {
                crate::revolute_joint::revolute_joint_enable_spring(world, id, flag);
            }
        }
        RecOp::RevoluteJointSetSpringHertz => {
            let id = remap_joint_id(world, r.r_jointid());
            let v = r.r_f32();
            if r.ok {
                crate::revolute_joint::revolute_joint_set_spring_hertz(world, id, v);
            }
        }
        RecOp::RevoluteJointSetSpringDampingRatio => {
            let id = remap_joint_id(world, r.r_jointid());
            let v = r.r_f32();
            if r.ok {
                crate::revolute_joint::revolute_joint_set_spring_damping_ratio(world, id, v);
            }
        }
        RecOp::RevoluteJointSetTargetAngle => {
            let id = remap_joint_id(world, r.r_jointid());
            let v = r.r_f32();
            if r.ok {
                crate::revolute_joint::revolute_joint_set_target_angle(world, id, v);
            }
        }
        RecOp::RevoluteJointEnableLimit => {
            let id = remap_joint_id(world, r.r_jointid());
            let flag = r.r_bool();
            if r.ok {
                crate::revolute_joint::revolute_joint_enable_limit(world, id, flag);
            }
        }
        RecOp::RevoluteJointSetLimits => {
            let id = remap_joint_id(world, r.r_jointid());
            let lower = r.r_f32();
            let upper = r.r_f32();
            if r.ok {
                crate::revolute_joint::revolute_joint_set_limits(world, id, lower, upper);
            }
        }
        RecOp::RevoluteJointEnableMotor => {
            let id = remap_joint_id(world, r.r_jointid());
            let flag = r.r_bool();
            if r.ok {
                crate::revolute_joint::revolute_joint_enable_motor(world, id, flag);
            }
        }
        RecOp::RevoluteJointSetMotorSpeed => {
            let id = remap_joint_id(world, r.r_jointid());
            let v = r.r_f32();
            if r.ok {
                crate::revolute_joint::revolute_joint_set_motor_speed(world, id, v);
            }
        }
        RecOp::RevoluteJointSetMaxMotorTorque => {
            let id = remap_joint_id(world, r.r_jointid());
            let v = r.r_f32();
            if r.ok {
                crate::revolute_joint::revolute_joint_set_max_motor_torque(world, id, v);
            }
        }
        RecOp::SphericalJointEnableConeLimit => {
            let id = remap_joint_id(world, r.r_jointid());
            let flag = r.r_bool();
            if r.ok {
                crate::spherical_joint::spherical_joint_enable_cone_limit(world, id, flag);
            }
        }
        RecOp::SphericalJointSetConeLimit => {
            let id = remap_joint_id(world, r.r_jointid());
            let v = r.r_f32();
            if r.ok {
                crate::spherical_joint::spherical_joint_set_cone_limit(world, id, v);
            }
        }
        RecOp::SphericalJointEnableTwistLimit => {
            let id = remap_joint_id(world, r.r_jointid());
            let flag = r.r_bool();
            if r.ok {
                crate::spherical_joint::spherical_joint_enable_twist_limit(world, id, flag);
            }
        }
        RecOp::SphericalJointSetTwistLimits => {
            let id = remap_joint_id(world, r.r_jointid());
            let lower = r.r_f32();
            let upper = r.r_f32();
            if r.ok {
                crate::spherical_joint::spherical_joint_set_twist_limits(world, id, lower, upper);
            }
        }
        RecOp::SphericalJointEnableSpring => {
            let id = remap_joint_id(world, r.r_jointid());
            let flag = r.r_bool();
            if r.ok {
                crate::spherical_joint::spherical_joint_enable_spring(world, id, flag);
            }
        }
        RecOp::SphericalJointSetSpringHertz => {
            let id = remap_joint_id(world, r.r_jointid());
            let v = r.r_f32();
            if r.ok {
                crate::spherical_joint::spherical_joint_set_spring_hertz(world, id, v);
            }
        }
        RecOp::SphericalJointSetSpringDampingRatio => {
            let id = remap_joint_id(world, r.r_jointid());
            let v = r.r_f32();
            if r.ok {
                crate::spherical_joint::spherical_joint_set_spring_damping_ratio(world, id, v);
            }
        }
        RecOp::SphericalJointSetTargetRotation => {
            let id = remap_joint_id(world, r.r_jointid());
            let q = r.r_quat();
            if r.ok {
                crate::spherical_joint::spherical_joint_set_target_rotation(world, id, q);
            }
        }
        RecOp::SphericalJointEnableMotor => {
            let id = remap_joint_id(world, r.r_jointid());
            let flag = r.r_bool();
            if r.ok {
                crate::spherical_joint::spherical_joint_enable_motor(world, id, flag);
            }
        }
        RecOp::SphericalJointSetMotorVelocity => {
            let id = remap_joint_id(world, r.r_jointid());
            let v = r.r_vec3();
            if r.ok {
                crate::spherical_joint::spherical_joint_set_motor_velocity(world, id, v);
            }
        }
        RecOp::SphericalJointSetMaxMotorTorque => {
            let id = remap_joint_id(world, r.r_jointid());
            let v = r.r_f32();
            if r.ok {
                crate::spherical_joint::spherical_joint_set_max_motor_torque(world, id, v);
            }
        }
        RecOp::WeldJointSetLinearHertz => {
            let id = remap_joint_id(world, r.r_jointid());
            let v = r.r_f32();
            if r.ok {
                crate::weld_joint::weld_joint_set_linear_hertz(world, id, v);
            }
        }
        RecOp::WeldJointSetLinearDampingRatio => {
            let id = remap_joint_id(world, r.r_jointid());
            let v = r.r_f32();
            if r.ok {
                crate::weld_joint::weld_joint_set_linear_damping_ratio(world, id, v);
            }
        }
        RecOp::WeldJointSetAngularHertz => {
            let id = remap_joint_id(world, r.r_jointid());
            let v = r.r_f32();
            if r.ok {
                crate::weld_joint::weld_joint_set_angular_hertz(world, id, v);
            }
        }
        RecOp::WeldJointSetAngularDampingRatio => {
            let id = remap_joint_id(world, r.r_jointid());
            let v = r.r_f32();
            if r.ok {
                crate::weld_joint::weld_joint_set_angular_damping_ratio(world, id, v);
            }
        }
        RecOp::WheelJointEnableSuspension => {
            let id = remap_joint_id(world, r.r_jointid());
            let flag = r.r_bool();
            if r.ok {
                crate::wheel_joint::wheel_joint_enable_suspension(world, id, flag);
            }
        }
        RecOp::WheelJointSetSuspensionHertz => {
            let id = remap_joint_id(world, r.r_jointid());
            let v = r.r_f32();
            if r.ok {
                crate::wheel_joint::wheel_joint_set_suspension_hertz(world, id, v);
            }
        }
        RecOp::WheelJointSetSuspensionDampingRatio => {
            let id = remap_joint_id(world, r.r_jointid());
            let v = r.r_f32();
            if r.ok {
                crate::wheel_joint::wheel_joint_set_suspension_damping_ratio(world, id, v);
            }
        }
        RecOp::WheelJointEnableSuspensionLimit => {
            let id = remap_joint_id(world, r.r_jointid());
            let flag = r.r_bool();
            if r.ok {
                crate::wheel_joint::wheel_joint_enable_suspension_limit(world, id, flag);
            }
        }
        RecOp::WheelJointSetSuspensionLimits => {
            let id = remap_joint_id(world, r.r_jointid());
            let lower = r.r_f32();
            let upper = r.r_f32();
            if r.ok {
                crate::wheel_joint::wheel_joint_set_suspension_limits(world, id, lower, upper);
            }
        }
        RecOp::WheelJointEnableSpinMotor => {
            let id = remap_joint_id(world, r.r_jointid());
            let flag = r.r_bool();
            if r.ok {
                crate::wheel_joint::wheel_joint_enable_spin_motor(world, id, flag);
            }
        }
        RecOp::WheelJointSetSpinMotorSpeed => {
            let id = remap_joint_id(world, r.r_jointid());
            let v = r.r_f32();
            if r.ok {
                crate::wheel_joint::wheel_joint_set_spin_motor_speed(world, id, v);
            }
        }
        RecOp::WheelJointSetMaxSpinTorque => {
            let id = remap_joint_id(world, r.r_jointid());
            let v = r.r_f32();
            if r.ok {
                crate::wheel_joint::wheel_joint_set_max_spin_torque(world, id, v);
            }
        }
        RecOp::WheelJointEnableSteering => {
            let id = remap_joint_id(world, r.r_jointid());
            let flag = r.r_bool();
            if r.ok {
                crate::wheel_joint::wheel_joint_enable_steering(world, id, flag);
            }
        }
        RecOp::WheelJointSetSteeringHertz => {
            let id = remap_joint_id(world, r.r_jointid());
            let v = r.r_f32();
            if r.ok {
                crate::wheel_joint::wheel_joint_set_steering_hertz(world, id, v);
            }
        }
        RecOp::WheelJointSetSteeringDampingRatio => {
            let id = remap_joint_id(world, r.r_jointid());
            let v = r.r_f32();
            if r.ok {
                crate::wheel_joint::wheel_joint_set_steering_damping_ratio(world, id, v);
            }
        }
        RecOp::WheelJointSetMaxSteeringTorque => {
            let id = remap_joint_id(world, r.r_jointid());
            let v = r.r_f32();
            if r.ok {
                crate::wheel_joint::wheel_joint_set_max_steering_torque(world, id, v);
            }
        }
        RecOp::WheelJointEnableSteeringLimit => {
            let id = remap_joint_id(world, r.r_jointid());
            let flag = r.r_bool();
            if r.ok {
                crate::wheel_joint::wheel_joint_enable_steering_limit(world, id, flag);
            }
        }
        RecOp::WheelJointSetSteeringLimits => {
            let id = remap_joint_id(world, r.r_jointid());
            let lower = r.r_f32();
            let upper = r.r_f32();
            if r.ok {
                crate::wheel_joint::wheel_joint_set_steering_limits(world, id, lower, upper);
            }
        }
        RecOp::WheelJointSetTargetSteeringAngle => {
            let id = remap_joint_id(world, r.r_jointid());
            let v = r.r_f32();
            if r.ok {
                crate::wheel_joint::wheel_joint_set_target_steering_angle(world, id, v);
            }
        }

        // Queries: read the recorded inputs and hit tail, re-issue against the
        // replay world, compare each callback hit, latch divergence.
        RecOp::QueryTag => {
            let key = r.r_u64();
            if r.ok {
                *sink.pending_query_key = key;
            }
        }
        RecOp::QueryOverlapAABB => {
            let _wid = r.r_worldid();
            let aabb = r.r_aabb();
            let filter = r.r_queryfilter();
            let hits = match read_overlap_hits(&mut r, world) {
                Some(h) => h,
                None => {
                    *ok = false;
                    return -1;
                }
            };
            let _stats = r.r_treestats();
            if r.ok {
                let mut hit_cursor = 0usize;
                let mut diverged = false;
                {
                    let mut fcn = |id: ShapeId| -> bool {
                        replay_overlap_hit(&hits, &mut hit_cursor, &mut diverged, id)
                    };
                    crate::physics_world::world_overlap_aabb(world, aabb, filter, &mut fcn);
                }
                if hit_cursor != hits.len() {
                    diverged = true;
                }
                *sink.diverged |= diverged;
                let q = sink.stash_query(RecQueryKind::OverlapAabb, &hits);
                q.filter = filter;
                q.aabb = aabb;
            }
        }
        RecOp::QueryOverlapShape => {
            let _wid = r.r_worldid();
            let origin = r.r_position();
            let (points, radius) = r.r_shapeproxy();
            let filter = r.r_queryfilter();
            let hits = match read_overlap_hits(&mut r, world) {
                Some(h) => h,
                None => {
                    *ok = false;
                    return -1;
                }
            };
            let _stats = r.r_treestats();
            if r.ok {
                let proxy = ShapeProxy { points: &points, radius };
                let mut hit_cursor = 0usize;
                let mut diverged = false;
                {
                    let mut fcn = |id: ShapeId| -> bool {
                        replay_overlap_hit(&hits, &mut hit_cursor, &mut diverged, id)
                    };
                    crate::physics_world::world_overlap_shape(world, origin, &proxy, filter, &mut fcn);
                }
                if hit_cursor != hits.len() {
                    diverged = true;
                }
                *sink.diverged |= diverged;
                let q = sink.stash_query(RecQueryKind::OverlapShape, &hits);
                q.filter = filter;
                q.origin = origin;
            }
        }
        RecOp::QueryCastRay => {
            let _wid = r.r_worldid();
            let origin = r.r_position();
            let translation = r.r_vec3();
            let filter = r.r_queryfilter();
            let hits = match read_cast_hits(&mut r, world) {
                Some(h) => h,
                None => {
                    *ok = false;
                    return -1;
                }
            };
            let _stats = r.r_treestats();
            if r.ok {
                let mut hit_cursor = 0usize;
                let mut diverged = false;
                {
                    let mut fcn = |id: ShapeId,
                                   point: Pos,
                                   normal: crate::math_functions::Vec3,
                                   fraction: f32,
                                   user_material_id: u64,
                                   triangle_index: i32,
                                   child_index: i32|
                     -> f32 {
                        if hit_cursor >= hits.len() {
                            diverged = true;
                            return 0.0;
                        }
                        let h = &hits[hit_cursor];
                        hit_cursor += 1;
                        if id.index1 != h.id.index1
                            || id.generation != h.id.generation
                            || pos_differs(point, h.point)
                            || vec3_differs(normal, h.normal)
                            || f32_differs(fraction, h.fraction)
                            || user_material_id != h.user_material_id
                            || triangle_index != h.triangle_index
                            || child_index != h.child_index
                        {
                            diverged = true;
                        }
                        h.user_return_f
                    };
                    crate::physics_world::world_cast_ray(world, origin, translation, filter, &mut fcn);
                }
                if hit_cursor != hits.len() {
                    diverged = true;
                }
                *sink.diverged |= diverged;
                let q = sink.stash_query(RecQueryKind::CastRay, &hits);
                q.filter = filter;
                q.origin = origin;
                q.translation = translation;
            }
        }
        RecOp::QueryCastShape => {
            let _wid = r.r_worldid();
            let origin = r.r_position();
            let (points, radius) = r.r_shapeproxy();
            let translation = r.r_vec3();
            let filter = r.r_queryfilter();
            let hits = match read_cast_hits(&mut r, world) {
                Some(h) => h,
                None => {
                    *ok = false;
                    return -1;
                }
            };
            let _stats = r.r_treestats();
            if r.ok {
                let proxy = ShapeProxy { points: &points, radius };
                let mut hit_cursor = 0usize;
                let mut diverged = false;
                {
                    let mut fcn = |id: ShapeId,
                                   point: Pos,
                                   normal: crate::math_functions::Vec3,
                                   fraction: f32,
                                   user_material_id: u64,
                                   triangle_index: i32,
                                   child_index: i32|
                     -> f32 {
                        if hit_cursor >= hits.len() {
                            diverged = true;
                            return 0.0;
                        }
                        let h = &hits[hit_cursor];
                        hit_cursor += 1;
                        if id.index1 != h.id.index1
                            || id.generation != h.id.generation
                            || pos_differs(point, h.point)
                            || vec3_differs(normal, h.normal)
                            || f32_differs(fraction, h.fraction)
                            || user_material_id != h.user_material_id
                            || triangle_index != h.triangle_index
                            || child_index != h.child_index
                        {
                            diverged = true;
                        }
                        h.user_return_f
                    };
                    crate::physics_world::world_cast_shape(world, origin, &proxy, translation, filter, &mut fcn);
                }
                if hit_cursor != hits.len() {
                    diverged = true;
                }
                *sink.diverged |= diverged;
                let q = sink.stash_query(RecQueryKind::CastShape, &hits);
                q.filter = filter;
                q.origin = origin;
                q.translation = translation;
            }
        }
        RecOp::QueryCastRayClosest => {
            let _wid = r.r_worldid();
            let origin = r.r_position();
            let translation = r.r_vec3();
            let filter = r.r_queryfilter();
            let mut rec = r.r_rayresult();
            if r.ok {
                rec.shape_id = remap_shape_id(world, rec.shape_id);
                let got = crate::physics_world::world_cast_ray_closest(world, origin, translation, filter);
                let mut diverged = false;
                if got.hit != rec.hit
                    || (got.hit
                        && (got.shape_id.index1 != rec.shape_id.index1
                            || got.shape_id.generation != rec.shape_id.generation
                            || pos_differs(got.point, rec.point)
                            || vec3_differs(got.normal, rec.normal)
                            || f32_differs(got.fraction, rec.fraction)
                            || got.user_material_id != rec.user_material_id))
                {
                    diverged = true;
                }
                *sink.diverged |= diverged;
                // Stash the closest result as a single pooled hit.
                let mut h = RecordedHit::default();
                h.id = rec.shape_id;
                h.point = rec.point;
                h.normal = rec.normal;
                h.fraction = rec.fraction;
                let hits: &[RecordedHit] = if rec.hit { std::slice::from_ref(&h) } else { &[] };
                let hits: Vec<RecordedHit> = hits.to_vec();
                let q = sink.stash_query(RecQueryKind::CastRayClosest, &hits);
                q.filter = filter;
                q.origin = origin;
                q.translation = translation;
                q.ray_result = rec;
            }
        }
        RecOp::QueryCastMover => {
            let _wid = r.r_worldid();
            let origin = r.r_position();
            let mover = r.r_capsule();
            let translation = r.r_vec3();
            let filter = r.r_queryfilter();
            let hits = match read_overlap_hits(&mut r, world) {
                Some(h) => h,
                None => {
                    *ok = false;
                    return -1;
                }
            };
            let rec_fraction = r.r_f32();
            if r.ok {
                let mut hit_cursor = 0usize;
                let mut diverged = false;
                let got = {
                    let mut fcn = |id: ShapeId| -> bool {
                        replay_overlap_hit(&hits, &mut hit_cursor, &mut diverged, id)
                    };
                    crate::physics_world::world_cast_mover(world, origin, &mover, translation, filter, Some(&mut fcn))
                };
                if hit_cursor != hits.len() || f32_differs(got, rec_fraction) {
                    diverged = true;
                }
                *sink.diverged |= diverged;
                let q = sink.stash_query(RecQueryKind::CastMover, &[]);
                q.filter = filter;
                q.origin = origin;
                q.mover = mover;
                q.translation = translation;
                q.cast_fraction = rec_fraction;
            }
        }
        RecOp::QueryCollideMover => {
            let _wid = r.r_worldid();
            let origin = r.r_position();
            let mover = r.r_capsule();
            let filter = r.r_queryfilter();
            // Recorded as shapeCount groups, each: shapeId, planeCount, planes,
            // user return. Flatten into one hit per plane with the group's count
            // and return replicated (C comment).
            let shape_count = r.r_u32();
            if !r.ok || shape_count as usize > r.remaining() {
                *ok = false;
                return -1;
            }
            let mut hits: Vec<RecordedHit> = Vec::new();
            for _ in 0..shape_count {
                let id = remap_shape_id(world, r.r_shapeid());
                let mut plane_count = r.r_i32();
                if plane_count < 0 {
                    plane_count = 0;
                }
                if !r.check_count(plane_count, 28) {
                    *ok = false;
                    return -1;
                }
                let start = hits.len();
                for _ in 0..plane_count {
                    let mut h = RecordedHit::default();
                    h.plane = r.r_planeresult();
                    hits.push(h);
                }
                let ret = r.r_bool();
                for h in hits[start..].iter_mut() {
                    h.id = id;
                    h.plane_count = plane_count;
                    h.user_return_b = ret;
                }
            }
            if r.ok {
                let mut hit_cursor = 0usize;
                let mut diverged = false;
                {
                    // C: b3RecReplayPlaneTrampoline — compare the batch and
                    // advance by the recorded count to stay aligned.
                    let mut fcn = |id: ShapeId, planes: &[PlaneResult]| -> bool {
                        if hit_cursor >= hits.len() {
                            diverged = true;
                            return true;
                        }
                        let head = hits[hit_cursor];
                        let recorded_count = head.plane_count as usize;
                        let ret = head.user_return_b;
                        if id.index1 != head.id.index1
                            || id.generation != head.id.generation
                            || recorded_count != planes.len()
                        {
                            diverged = true;
                        }
                        let n = recorded_count.min(planes.len());
                        for i in 0..n {
                            let h = &hits[hit_cursor + i];
                            if vec3_differs(h.plane.plane.normal, planes[i].plane.normal)
                                || f32_differs(h.plane.plane.offset, planes[i].plane.offset)
                                || vec3_differs(h.plane.point, planes[i].point)
                            {
                                diverged = true;
                            }
                        }
                        hit_cursor += recorded_count;
                        ret
                    };
                    crate::physics_world::world_collide_mover(world, origin, &mover, filter, &mut fcn);
                }
                if hit_cursor != hits.len() {
                    diverged = true;
                }
                *sink.diverged |= diverged;
                let q = sink.stash_query(RecQueryKind::CollideMover, &hits);
                q.filter = filter;
                q.origin = origin;
                q.mover = mover;
            }
        }
    }

    if !r.ok {
        *ok = false;
        return -1;
    }
    *cursor = r.cursor;
    let _ = payload_start;
    opcode as i32
}

impl Player {
    /// C: b3RecPlayer_Create. Returns None on a corrupt or incompatible image.
    pub fn create(data: &[u8], worker_count: i32) -> Option<Player> {
        if data.len() < REC_HEADER_SIZE {
            crate::core::log("Player::create: recording too small");
            return None;
        }

        let magic = u32::from_le_bytes(data[0..4].try_into().unwrap());
        if magic != REC_MAGIC {
            crate::core::log(&format!("Player::create: bad magic 0x{:08X}", magic));
            return None;
        }
        let version_major = u16::from_le_bytes(data[4..6].try_into().unwrap());
        if version_major != REC_VERSION_MAJOR {
            crate::core::log("Player::create: version mismatch");
            return None;
        }
        // pointerWidth (byte 8) is always written as 8 by the port; bigEndian
        // (byte 9) must be 0.
        if data[8] != 8 || data[9] != 0 {
            crate::core::log("Player::create: incompatible recording");
            return None;
        }
        let length_scale = f32::from_le_bytes(data[12..16].try_into().unwrap());

        let snapshot_size =
            u64::from_le_bytes(data[REC_HDR_SNAPSHOT_SIZE_OFFSET..REC_HDR_SNAPSHOT_SIZE_OFFSET + 8].try_into().unwrap());
        let registry_offset = u64::from_le_bytes(
            data[REC_HDR_REGISTRY_OFFSET_OFFSET..REC_HDR_REGISTRY_OFFSET_OFFSET + 8].try_into().unwrap(),
        );
        let registry_byte_count = u64::from_le_bytes(
            data[REC_HDR_REGISTRY_BYTE_COUNT_OFFSET..REC_HDR_REGISTRY_BYTE_COUNT_OFFSET + 8].try_into().unwrap(),
        );

        // Every recording is snapshot-seeded.
        if snapshot_size == 0 {
            crate::core::log("Player::create: missing snapshot seed");
            return None;
        }

        // Validate offsets in 64-bit so hostile values cannot wrap.
        let header_end64 = REC_HEADER_SIZE as u64 + snapshot_size;
        let registry_end64 = if registry_offset != 0 { registry_offset } else { data.len() as u64 };
        if header_end64 < REC_HEADER_SIZE as u64
            || header_end64 > registry_end64
            || registry_end64 > data.len() as u64
        {
            crate::core::log("Player::create: corrupt offsets");
            return None;
        }
        let header_end = header_end64 as usize;
        let registry_end = registry_end64 as usize;

        // Load the trailing geometry registry + tag table.
        let reader = if registry_offset != 0 && registry_byte_count != 0 {
            let reg_start = registry_offset as usize;
            let reg_end = reg_start.checked_add(registry_byte_count as usize)?;
            if reg_end > data.len() {
                crate::core::log("Player::create: registry block out of bounds");
                return None;
            }
            load_registry(&data[reg_start..reg_end])?
        } else {
            RecReader::default()
        };

        let previous_length_scale = get_length_units_per_meter();
        if length_scale > 0.0 {
            set_length_units_per_meter(length_scale);
        }

        // Create the replay world carrying the requested worker count, so a
        // rebuild on restart keeps the same graph partitioning. Replaying at a
        // different count than recorded is a determinism check (C comment).
        let mut world_def = crate::types::default_world_def();
        world_def.worker_count = crate::math_functions::max_int(1, worker_count) as u32;
        let mut world = crate::physics_world::create_world(&world_def);

        // Restore the seed snapshot to stand up the replay world.
        let snap_start = REC_HEADER_SIZE;
        let snap = &data[snap_start..snap_start + snapshot_size as usize];
        if !crate::world_snapshot::deserialize_into_shell(snap, &mut world, &reader) {
            crate::core::log("Player::create: snapshot deserialization failed");
            crate::physics_world::destroy_world(world);
            set_length_units_per_meter(previous_length_scale);
            return None;
        }

        let mut player = Player {
            data: data.to_vec(),
            header_end,
            registry_end,
            length_scale,
            previous_length_scale,
            frame: 0,
            frame_count: 0,
            recorded_dt: 0.0,
            recorded_sub_step_count: 0,
            recorded_worker_count: worker_count,
            bounds: crate::math_functions::AABB::default(),
            at_end: false,
            diverge_frame: -1,
            cursor: header_end,
            ok: true,
            diverged: false,
            pending_query_key: 0,
            reader,
            body_ids: Vec::new(),
            frame0_body_ids: Vec::new(),
            frame_queries: Vec::new(),
            frame_hits: Vec::new(),
            frame0_snap: (snap_start, snapshot_size as usize),
            keyframes: Vec::new(),
            keyframe_budget: KEYFRAME_BUDGET_DEFAULT,
            keyframe_bytes: 0,
            keyframe_min_interval: KEYFRAME_INTERVAL_DEFAULT,
            keyframe_interval: KEYFRAME_INTERVAL_DEFAULT,
            last_keyframe_frame: 0,
            keyframe_rec: Recording::new(),
            world: Some(world),
        };

        // Count frames and read the first step's tuning + trailing bounds.
        player.scan_file();

        // Seed the outliner from the restored world (snapshot bodies bypass the
        // create hook) and save the frame-0 restore copy.
        player.seed_frame0_body_ids();

        // Pre-populate the keyframe registry to mirror the slots so geometry
        // ids stay stable during serialize_world (C: b3RecSeedKeyframeRegistry).
        for (i, bytes) in player.reader.slot_bytes.iter().enumerate() {
            let h = hash64_blob(bytes);
            let id = append_geometry(
                &mut player.keyframe_rec.registry,
                player.reader.slots[i].kind,
                h,
                bytes.clone(),
            );
            crate::b3_assert!(id == i as u32);
            let _ = id;
        }

        Some(player)
    }

    /// The replay world (owned by the player).
    pub fn world(&self) -> &World {
        self.world.as_ref().unwrap()
    }

    pub fn world_mut(&mut self) -> &mut World {
        self.world.as_mut().unwrap()
    }

    // C: b3RecScanFile — walk the op stream once without dispatching.
    fn scan_file(&mut self) {
        let data = &self.data;
        let size = self.registry_end;
        let mut cursor = self.header_end;
        let mut frame_count = 0;
        let mut got_step = false;

        while cursor + 4 <= size {
            let opcode = data[cursor];
            let payload_size = data[cursor + 1] as usize
                | ((data[cursor + 2] as usize) << 8)
                | ((data[cursor + 3] as usize) << 16);
            let payload_start = cursor + 4;
            if payload_start + payload_size > size {
                break;
            }
            if opcode == RecOp::Step as u8 {
                frame_count += 1;
                if !got_step && payload_size >= 12 {
                    self.recorded_dt = f32::from_le_bytes(
                        data[payload_start + 4..payload_start + 8].try_into().unwrap(),
                    );
                    self.recorded_sub_step_count = i32::from_le_bytes(
                        data[payload_start + 8..payload_start + 12].try_into().unwrap(),
                    );
                    got_step = true;
                }
            } else if opcode == RecOp::RecordingBounds as u8 && payload_size >= 24 {
                let mut r = RecCursor::new(&data[payload_start..payload_start + payload_size]);
                self.bounds = r.r_aabb();
            }
            cursor = payload_start + payload_size;
        }
        self.frame_count = frame_count;
    }

    // C: b3RecSeedBodyIds / b3RecSeedFrame0BodyIds.
    fn seed_body_ids(&mut self) {
        let world = self.world.as_ref().unwrap();
        self.body_ids.clear();
        for i in 0..world.bodies.len() {
            if world.bodies[i].id != i as i32 {
                continue; // free slot
            }
            self.body_ids.push(crate::body::make_body_id(world, i as i32));
        }
    }

    fn seed_frame0_body_ids(&mut self) {
        self.seed_body_ids();
        self.frame0_body_ids = self.body_ids.clone();
    }

    // Dispatch one record. Moves data + world out to satisfy the borrow checker.
    fn dispatch_one(&mut self) -> i32 {
        let data = std::mem::take(&mut self.data);
        let mut world = self.world.take().unwrap();
        let mut cursor = self.cursor;
        let mut ok = self.ok;
        let op;
        {
            let mut sink = DispatchSink {
                diverged: &mut self.diverged,
                pending_query_key: &mut self.pending_query_key,
                reader: &self.reader,
                frame_queries: &mut self.frame_queries,
                frame_hits: &mut self.frame_hits,
                body_ids: &mut self.body_ids,
                bounds: &mut self.bounds,
            };
            op = dispatch_one(&data, &mut cursor, self.registry_end, &mut ok, &mut world, &mut sink);
        }
        self.cursor = cursor;
        self.ok = ok;
        self.world = Some(world);
        self.data = data;
        op
    }

    /// C: b3RecPlayer_StepFrame. A frame is its leading inputs (queries and
    /// between-step mutators), one Step, and the Step's trailing StateHash.
    pub fn step_frame(&mut self) -> bool {
        if self.at_end {
            return false;
        }

        // Reset the per-frame query store before this frame's records dispatch.
        self.frame_queries.clear();
        self.frame_hits.clear();

        let mut stepped = false;
        loop {
            if self.cursor >= self.registry_end || !self.ok {
                self.at_end = true;
                return stepped;
            }

            // Once stepped, the StateHash is the only record still belonging to
            // this frame. Anything else begins the next frame. Capture a
            // keyframe at the boundary.
            if stepped && self.data[self.cursor] != RecOp::StateHash as u8 {
                if self.frame > self.last_keyframe_frame && self.frame % self.keyframe_interval == 0 {
                    self.capture_keyframe();
                }
                return true;
            }

            let op = self.dispatch_one();
            if op < 0 {
                self.at_end = true;
                return stepped;
            }
            if op == RecOp::DestroyWorld as u8 as i32 {
                self.at_end = true;
                return stepped;
            }
            if op == RecOp::Step as u8 as i32 {
                self.frame += 1;
                stepped = true;
            } else if op == RecOp::StateHash as u8 as i32 {
                // Latch the first frame whose state hash diverged.
                if self.diverge_frame < 0 && self.diverged {
                    self.diverge_frame = self.frame;
                }
            }
        }
    }

    // C: b3RecCaptureKeyframe — capture a restore point for the just-completed
    // frame; the cursor already points at the next frame's first record.
    fn capture_keyframe(&mut self) {
        let world = self.world.as_ref().unwrap();
        let mut buf = RecBuffer::new();

        let reg_count_before = self.keyframe_rec.registry.entries.len();
        crate::world_snapshot::serialize_world(world, &mut buf, &mut self.keyframe_rec);
        // Registry must not grow: geometry was pre-seeded and dedups exactly.
        crate::b3_assert!(self.keyframe_rec.registry.entries.len() == reg_count_before);
        let _ = reg_count_before;

        let body_bytes = self.body_ids.len() * std::mem::size_of::<BodyId>();
        let new_bytes = buf.data.len() + body_bytes;

        // Make room under the budget by doubling the spacing and evicting
        // off-grid keyframes.
        while !self.keyframes.is_empty() && self.keyframe_bytes + new_bytes > self.keyframe_budget {
            self.keyframe_interval *= 2;
            let interval = self.keyframe_interval;
            let before = self.keyframes.len();
            let mut kept_bytes = 0usize;
            self.keyframes.retain(|kf| kf.frame % interval == 0);
            for kf in &self.keyframes {
                kept_bytes += kf.image.len() + kf.body_ids.len() * std::mem::size_of::<BodyId>();
            }
            self.keyframe_bytes = kept_bytes;
            if self.keyframes.len() == before {
                break;
            }
        }

        self.keyframes.push(Keyframe {
            image: buf.data,
            frame: self.frame,
            cursor: self.cursor,
            diverge_frame: self.diverge_frame,
            diverged: self.diverged,
            body_ids: self.body_ids.clone(),
        });
        self.keyframe_bytes += new_bytes;
        self.last_keyframe_frame = self.frame;
    }

    // C: b3RecPlayerRestoreKeyframe.
    fn restore_keyframe(&mut self, index: usize) {
        let (frame, cursor, diverge_frame, diverged) = {
            let kf = &self.keyframes[index];
            (kf.frame, kf.cursor, kf.diverge_frame, kf.diverged)
        };
        let image = std::mem::take(&mut self.keyframes[index].image);
        let world = self.world.as_mut().unwrap();
        if !crate::world_snapshot::deserialize_into_shell(&image, world, &self.reader) {
            self.ok = false;
            self.keyframes[index].image = image;
            return;
        }
        self.keyframes[index].image = image;
        self.cursor = cursor;
        self.ok = true;
        self.diverged = diverged;
        self.frame = frame;
        self.diverge_frame = diverge_frame;
        self.at_end = false;
        self.body_ids = self.keyframes[index].body_ids.clone();
    }

    /// C: b3RecPlayer_Restart — restore the frame-0 image in place so the
    /// replay world id stays stable across a restart or backward scrub.
    pub fn restart(&mut self) {
        let (snap_start, snap_size) = self.frame0_snap;
        let data = std::mem::take(&mut self.data);
        let world = self.world.as_mut().unwrap();
        let ok = crate::world_snapshot::deserialize_into_shell(
            &data[snap_start..snap_start + snap_size],
            world,
            &self.reader,
        );
        self.data = data;
        if !ok {
            self.ok = false;
            return;
        }
        self.cursor = self.header_end;
        self.ok = true;
        self.diverged = false;
        self.frame = 0;
        self.diverge_frame = -1;
        self.at_end = false;

        // Frame 0 is the pre-step snapshot with no recorded queries.
        self.frame_queries.clear();
        self.frame_hits.clear();

        // Roll the outliner body list back to its frame-0 contents.
        self.body_ids = self.frame0_body_ids.clone();
    }

    /// C: b3RecPlayer_SeekFrame.
    pub fn seek_frame(&mut self, target_frame: i32) {
        let target_frame = if target_frame < 0 { 0 } else { target_frame };

        // Find the best keyframe strictly before the target.
        let mut best: Option<usize> = None;
        for (i, kf) in self.keyframes.iter().enumerate() {
            if kf.frame < target_frame && (best.is_none() || kf.frame > self.keyframes[best.unwrap()].frame) {
                best = Some(i);
            }
        }

        if target_frame < self.frame {
            // Backward seek: restore keyframe or restart from frame 0.
            match best {
                Some(i) => self.restore_keyframe(i),
                None => self.restart(),
            }
        } else if let Some(i) = best {
            if self.keyframes[i].frame > self.frame {
                // Forward seek that can skip ahead via a keyframe.
                self.restore_keyframe(i);
            }
        }

        while self.frame < target_frame && self.step_frame() {}
    }

    // Accessors (C: b3RecPlayer_Get*)

    pub fn get_frame(&self) -> i32 {
        self.frame
    }

    pub fn get_frame_count(&self) -> i32 {
        self.frame_count
    }

    pub fn is_at_end(&self) -> bool {
        self.at_end
    }

    pub fn has_diverged(&self) -> bool {
        self.diverged
    }

    pub fn get_diverge_frame(&self) -> i32 {
        self.diverge_frame
    }

    pub fn is_ok(&self) -> bool {
        self.ok
    }

    pub fn get_info(&self) -> RecPlayerInfo {
        RecPlayerInfo {
            frame_count: self.frame_count,
            time_step: self.recorded_dt,
            sub_step_count: self.recorded_sub_step_count,
            bounds: self.bounds,
        }
    }

    pub fn get_body_count(&self) -> i32 {
        self.body_ids.len() as i32
    }

    /// Returns NULL_BODY_ID out of range or for a destroyed ordinal.
    pub fn get_body_id(&self, index: i32) -> BodyId {
        if index < 0 || index as usize >= self.body_ids.len() {
            return NULL_BODY_ID;
        }
        self.body_ids[index as usize]
    }

    pub fn get_frame_query_count(&self) -> i32 {
        self.frame_queries.len() as i32
    }

    /// Resolve one recorded query's info, with the caller id/label resolved
    /// through the tag table (C: b3RecPlayer_GetFrameQuery).
    pub fn get_frame_query(&self, index: i32) -> RecQueryInfo {
        let q = &self.frame_queries[index as usize];
        let mut info = RecQueryInfo { kind: q.kind, key: q.key, id: 0, name: None, hit_count: q.hit_count };
        if q.key != 0 {
            if let Some(&tag_index) = self.reader.tag_map.get(&q.key) {
                let tag: &RecTag = &self.reader.tags[tag_index as usize];
                info.id = tag.id;
                info.name = Some(tag.name.clone());
            }
        }
        info
    }

    // Keyframe policy (C: b3RecPlayer_SetKeyframePolicy — clears the ring).
    pub fn set_keyframe_policy(&mut self, budget_bytes: usize, min_interval_frames: i32) {
        let min_interval = crate::math_functions::max_int(1, min_interval_frames);
        self.keyframe_budget = budget_bytes;
        self.keyframe_min_interval = min_interval;
        self.keyframe_interval = min_interval;
        self.keyframes.clear();
        self.keyframe_bytes = 0;
    }

    pub fn get_keyframe_budget(&self) -> usize {
        self.keyframe_budget
    }

    pub fn get_keyframe_min_interval(&self) -> i32 {
        self.keyframe_min_interval
    }

    pub fn get_keyframe_interval(&self) -> i32 {
        self.keyframe_interval
    }

    pub fn get_keyframe_bytes(&self) -> usize {
        self.keyframe_bytes
    }

    /// C: b3RecPlayer_Destroy (also runs on Drop).
    pub fn destroy(mut self) {
        self.teardown();
    }

    fn teardown(&mut self) {
        if let Some(world) = self.world.take() {
            crate::physics_world::destroy_world(world);
        }
        // Restore the global length scale.
        set_length_units_per_meter(self.previous_length_scale);
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        if self.world.is_some() {
            self.teardown();
        }
    }
}

/// C: b3ValidateReplay — headless replay of a recording image; true when the
/// stream read cleanly and no state hash or query result diverged.
pub fn validate_replay(data: &[u8], worker_count: i32) -> bool {
    let mut player = match Player::create(data, worker_count) {
        Some(p) => p,
        None => return false,
    };

    while player.step_frame() {
        if player.has_diverged() {
            break;
        }
    }

    let ok = player.is_ok() && !player.has_diverged();
    player.destroy();
    ok
}

// NULL_INDEX is referenced by the substrate section above; silence the unused
// import lint when the dispatcher section does not use it directly.
const _: i32 = NULL_INDEX;
