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

/// Reader state for snapshot restore: the preloaded geometry registry.
/// The C b3RecReader also carries the op-stream cursor, scratch buffers, tags
/// and the player back-pointer — not ported.
#[derive(Clone, Debug, Default)]
pub struct RecReader {
    pub slots: Vec<RegistrySlot>,
}

impl RecReader {
    #[inline]
    pub fn slot_count(&self) -> i32 {
        self.slots.len() as i32
    }
}

/// C: b3RecLoadSlots — parse a registry block (as written by write_registry)
/// and decode every slot. Returns None on corrupt input.
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
    }

    // Query-tag table follows; the substrate ignores it (reads may stop here).

    Some(RecReader { slots })
}
